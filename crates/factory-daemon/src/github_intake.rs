//! Receipt and pre-release synchronization of labelled GitHub issues into
//! Intake (`#119`, `#180`).
//!
//! This is deliberately its own daemon loop rather than scheduler work: a
//! slow or unavailable GitHub CLI must not delay task dispatch. GitHub is
//! only read here. While an item remains inside Intake, title/body/comment
//! edits synchronize and an answered needs-info item returns to Received.

use crate::engine::Engine;
use chrono::{DateTime, Utc};
use factory_core::intake::{Intake, IntakeSource, IntakeStage, SourceKind};
use factory_core::{NewTask, Task, TaskEntry, TaskFilter, TaskPatch, TaskStatus};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const POLL_INTERVAL: Duration = Duration::from_secs(60);
/// Shared with `github_outbound.rs`: every `gh` call, wherever it is made
/// from, gets the same 30-second ceiling.
pub(crate) const GH_TIMEOUT: Duration = Duration::from_secs(30);
const INTAKE_LABEL: &str = "factory:intake";
const ISSUE_ARGS: [&str; 11] = [
    "issue",
    "list",
    "--repo",
    // repository is inserted here
    "--state",
    "open",
    "--label",
    INTAKE_LABEL,
    "--limit",
    "1000",
    "--json",
    "id,number,title,body,url,author,createdAt,comments",
];

#[derive(Debug, Deserialize)]
struct GithubAuthor {
    login: String,
}

#[derive(Debug, Deserialize)]
struct GithubIssue {
    id: String,
    number: u64,
    title: String,
    #[serde(default)]
    body: String,
    url: String,
    author: Option<GithubAuthor>,
    #[serde(rename = "createdAt")]
    created_at: DateTime<Utc>,
    #[serde(default)]
    comments: Vec<GithubComment>,
}

#[derive(Debug, Deserialize)]
struct GithubComment {
    #[serde(default)]
    body: String,
    author: Option<GithubAuthor>,
    #[serde(rename = "createdAt")]
    created_at: DateTime<Utc>,
}

pub async fn run(engine: Arc<Engine>, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let mut ticker = tokio::time::interval(POLL_INTERVAL);
    let mut etags = HashMap::new();
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = ticker.tick() => poll_once_conditional(&engine, Path::new("gh"), &mut etags).await,
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { return; }
            }
        }
    }
}

/// Poll every GitHub-backed scope once. Failures are deliberately contained
/// to one scope so the next repository, and the next interval, still run.
#[cfg(test)]
async fn poll_once(engine: &Engine, gh: &Path) {
    poll_once_inner(engine, gh, None).await;
}

async fn poll_once_conditional(engine: &Engine, gh: &Path, etags: &mut HashMap<String, String>) {
    poll_once_inner(engine, gh, Some(etags)).await;
}

async fn poll_once_inner(
    engine: &Engine,
    gh: &Path,
    mut etags: Option<&mut HashMap<String, String>>,
) {
    let tasks = match engine.store.list(&TaskFilter::default()).await {
        Ok(tasks) => tasks,
        Err(error) => {
            tracing::warn!("could not list intake items before polling GitHub: {error}");
            return;
        }
    };
    let mut received: HashMap<String, Task> = tasks
        .into_iter()
        .filter_map(|task| {
            let source = &task
                .intake
                .as_ref()
                .filter(|intake| intake.source.kind == SourceKind::Github)?
                .source;
            let key = source
                .external_id
                .as_ref()
                .map(|id| format!("id:{id}"))
                .or_else(|| source.reference.as_ref().map(|url| format!("url:{url}")))?;
            Some((key, task))
        })
        .collect();

    let scopes = engine.factory_snapshot().config.scopes;
    for scope in scopes {
        let Some(repo) = scope.git.as_deref().and_then(github_repo) else {
            continue;
        };
        if let Some(cache) = etags.as_deref_mut() {
            match repository_changed(gh, &repo, cache.get(&repo).map(String::as_str)).await {
                Ok(ConditionalPoll::Unchanged) => continue,
                Ok(ConditionalPoll::Changed(etag)) => {
                    if let Some(etag) = etag {
                        cache.insert(repo.clone(), etag);
                    }
                }
                Err(error) => {
                    tracing::warn!(scope = %scope.name, repository = %repo, "could not conditionally poll GitHub intake: {error}");
                    continue;
                }
            }
        }
        let issues = match list_issues(gh, &repo).await {
            Ok(issues) => issues,
            Err(error) => {
                tracing::warn!(scope = %scope.name, repository = %repo, "could not poll GitHub intake: {error}");
                continue;
            }
        };
        for issue in issues {
            let Some(reference) = github_issue_reference(&issue.url, issue.number) else {
                tracing::warn!(
                    scope = %scope.name,
                    repository = %repo,
                    issue = issue.number,
                    url = %issue.url,
                    "GitHub returned a non-canonical issue URL; skipping it"
                );
                continue;
            };
            let instructions = issue_text(&issue);
            let key = format!("id:{}", issue.id);
            let legacy_key = format!("url:{reference}");
            if let Some(existing) = received.get(&key).or_else(|| received.get(&legacy_key)) {
                sync_open_item(engine, existing, &repo, &reference, &issue, &instructions).await;
                continue;
            }
            let requester = issue
                .author
                .map(|author| author.login)
                .filter(|login| !login.trim().is_empty())
                .unwrap_or_else(|| "unknown GitHub user".into());
            let record = Intake {
                stage: IntakeStage::Received,
                source: Box::new(IntakeSource {
                    kind: SourceKind::Github,
                    reference: Some(reference.clone()),
                    provider: None,
                    relayed_by: None,
                    repository: Some(repo.clone()),
                    number: Some(issue.number),
                    external_id: Some(issue.id.clone()),
                }),
                requester,
                received_at: issue.created_at,
                triage: None,
                triage_task: None,
                questions: Vec::new(),
                decision: None,
                candidates: Vec::new(),
                security: None,
                outbound: None,
            };
            let new = NewTask {
                title: issue.title,
                instructions,
                scope: Some(scope.name.clone()),
                ..Default::default()
            };
            match engine.receive_intake(new, record).await {
                Ok(task) => {
                    received.insert(key, task);
                }
                Err(error) => {
                    tracing::warn!(
                        scope = %scope.name,
                        repository = %repo,
                        issue = issue.number,
                        "could not receive GitHub issue into intake: {error}"
                    );
                }
            }
        }
    }
}

enum ConditionalPoll {
    Unchanged,
    Changed(Option<String>),
}

/// Probe the fixed labelled-issues URL with an ETag.  A 304 stops the poll
/// before the heavier issue/comment query and does not consume GitHub's core
/// REST rate limit; after a change, the existing detail path performs the
/// paged/comment-rich read.
async fn repository_changed(
    gh: &Path,
    repo: &str,
    etag: Option<&str>,
) -> Result<ConditionalPoll, String> {
    // The newest updated match invalidates this cache regardless of how many
    // labelled issues exist. The detail read still pages the complete set.
    let endpoint = format!(
        "repos/{repo}/issues?state=open&labels=factory%3Aintake&sort=updated&direction=desc&per_page=1"
    );
    let mut command = tokio::process::Command::new(gh);
    command
        .kill_on_drop(true)
        .args(["api", "--include", "--method", "GET"]);
    if let Some(etag) = etag {
        command.args(["-H", &format!("If-None-Match: {etag}")]);
    }
    command.arg(endpoint);
    let output = tokio::time::timeout(GH_TIMEOUT, command.output())
        .await
        .map_err(|_| format!("gh api timed out after {} seconds", GH_TIMEOUT.as_secs()))?
        .map_err(|error| format!("starting gh: {error}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let status = text
        .lines()
        .find_map(|line| {
            line.strip_prefix("HTTP/")
                .and_then(|rest| rest.split_whitespace().nth(1))
        })
        .and_then(|code| code.parse::<u16>().ok());
    if status == Some(304) {
        return Ok(ConditionalPoll::Unchanged);
    }
    if !output.status.success() || status.is_some_and(|code| !(200..300).contains(&code)) {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("gh api conditional request failed with {}", output.status)
        } else {
            format!(
                "gh api conditional request failed with {}: {stderr}",
                output.status
            )
        });
    }
    let etag = text.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("etag")
            .then(|| value.trim().to_string())
    });
    Ok(ConditionalPoll::Changed(etag))
}

fn issue_text(issue: &GithubIssue) -> String {
    let mut text = issue.body.trim().to_string();
    for comment in &issue.comments {
        let author = comment
            .author
            .as_ref()
            .map(|author| author.login.as_str())
            .filter(|login| !login.trim().is_empty())
            .unwrap_or("unknown GitHub user");
        text.push_str(&format!(
            "\n\n---\nGitHub comment by @{author} at {}:\n\n{}",
            comment.created_at.to_rfc3339(),
            comment.body.trim()
        ));
    }
    text
}

async fn sync_open_item(
    engine: &Engine,
    existing: &Task,
    repo: &str,
    reference: &str,
    issue: &GithubIssue,
    instructions: &str,
) {
    if existing.status != TaskStatus::Intake {
        return;
    }
    let mut intake = existing.intake.as_ref().cloned().expect("GitHub map contains only intake records");
    let content_changed = existing.title != issue.title || existing.instructions != instructions;
    let provenance_changed = intake.source.reference.as_deref() != Some(reference)
        || intake.source.repository.as_deref() != Some(repo)
        || intake.source.number != Some(issue.number)
        || intake.source.external_id.as_deref() != Some(issue.id.as_str());
    intake.source.reference = Some(reference.to_string());
    intake.source.repository = Some(repo.to_string());
    intake.source.number = Some(issue.number);
    intake.source.external_id = Some(issue.id.clone());
    let answered = intake.stage == IntakeStage::NeedsInfo && content_changed;
    if answered {
        intake.stage = IntakeStage::Received;
        intake.questions.clear();
    }
    if !content_changed && !provenance_changed {
        return;
    }
    let patch = TaskPatch {
        title: (existing.title != issue.title).then(|| issue.title.clone()),
        instructions: (existing.instructions != instructions).then(|| instructions.to_string()),
        intake: Some(intake),
        ..Default::default()
    };
    match engine.store.update(&existing.id, &patch).await {
        Ok(_) => {
            engine
                .entry(
                    &existing.id,
                    TaskEntry::new(
                        "github",
                        "github_intake_synced",
                        if answered {
                            "GitHub changed while waiting for information; returned to intake"
                        } else {
                            "synchronized GitHub title, body or comments"
                        },
                    ),
                )
                .await;
            engine.publish_task(&existing.id).await;
        }
        Err(error) => tracing::warn!(task = existing.id, "could not synchronize GitHub intake item: {error}"),
    }
}

async fn list_issues(gh: &Path, repo: &str) -> Result<Vec<GithubIssue>, String> {
    let mut command = tokio::process::Command::new(gh);
    command.kill_on_drop(true);
    command
        .args(&ISSUE_ARGS[..3])
        .arg(repo)
        .args(&ISSUE_ARGS[3..]);
    let output = tokio::time::timeout(GH_TIMEOUT, command.output())
        .await
        .map_err(|_| {
            format!(
                "gh issue list timed out after {} seconds",
                GH_TIMEOUT.as_secs()
            )
        })?
        .map_err(|error| format!("starting gh: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("gh issue list exited with {}", output.status)
        } else {
            format!("gh issue list exited with {}: {stderr}", output.status)
        });
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("invalid gh issue list JSON: {error}"))
}

/// Normalize the two public GitHub remote forms a scope can declare. Other
/// hosts and local/branch-like values are not intake sources.
fn github_repo(remote: &str) -> Option<String> {
    let remote = remote.trim();
    let path = if let Some(path) = remote.strip_prefix("https://github.com/") {
        path
    } else if let Some(path) = remote.strip_prefix("git@github.com:") {
        path
    } else if let Some(path) = remote.strip_prefix("ssh://git@github.com/") {
        path
    } else {
        return None;
    };
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    if owner.is_empty()
        || repo.is_empty()
        || parts.next().is_some()
        || owner.chars().any(char::is_whitespace)
        || repo.chars().any(char::is_whitespace)
    {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

/// Validate the canonical public issue URL returned by GitHub without tying
/// it to the spelling of the configured remote. Repository names are
/// case-insensitive and old remote names may redirect, while `gh` returns the
/// repository's current canonical URL.
fn github_issue_reference(url: &str, number: u64) -> Option<String> {
    let (_, _, url_number) = parse_canonical_issue_url(url)?;
    if url_number != number {
        return None;
    }
    Some(url.to_owned())
}

/// Owner, repository and issue number out of a canonical
/// `https://github.com/<owner>/<repo>/issues/<n>` URL, with nothing past the
/// number -- the same shape check `github_issue_reference` runs against the
/// number GitHub itself just returned. Shared with `github_outbound.rs`
/// (`#171`), which has only the stored reference to go on and no fresh
/// number to check it against.
pub(crate) fn parse_canonical_issue_url(url: &str) -> Option<(String, String, u64)> {
    let path = url.strip_prefix("https://github.com/")?;
    let mut parts = path.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    let issues = parts.next()?;
    let number_str = parts.next()?;
    let number: u64 = number_str.parse().ok()?;
    if owner.is_empty()
        || repo.is_empty()
        || issues != "issues"
        || parts.next().is_some()
        || owner.chars().any(char::is_whitespace)
        || repo.chars().any(char::is_whitespace)
    {
        return None;
    }
    Some((owner.to_string(), repo.to_string(), number))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stores::ScopedStores;
    use factory_core::adapter::TaskStore;
    use factory_core::config::{Config, DaemonConfig, Factory, Instance, Scope};
    use factory_plugins::{Registry, SqliteStore};
    use std::collections::HashMap;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "factory-github-intake-test-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn fixture(&self, script: &str) -> PathBuf {
            let path = self.0.join("gh");
            std::fs::write(&path, script).unwrap();
            let mut permissions = std::fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions).unwrap();
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scope(name: &str, remote: &str) -> Scope {
        Scope {
            id: format!("{name}-id"),
            name: name.into(),
            path: PathBuf::new(),
            agent: None,
            agents: Vec::new(),
            runtime: Some("herdr".into()),
            git: Some(remote.into()),
            task_store: None,
            max_sessions: None,
            roles: Default::default(),
            dashboard: None,
            policies: Default::default(),
            quality: Default::default(),
            intake: Default::default(),
            dependencies: Default::default(),
            environments: Vec::new(),
            renewals: Vec::new(),
            backup: None,
        }
    }

    fn factory(scopes: Vec<Scope>) -> Factory {
        Factory {
            root: std::env::temp_dir(),
            config: Config {
                version: 1,
                instance: Instance {
                    id: "test".into(),
                    name: "test".into(),
                },
                daemon: DaemonConfig {
                    power_assertion: false,
                    default_agent: "shell".into(),
                    ..Default::default()
                },
                roles: Default::default(),
                dashboard: None,
                policies: Default::default(),
                quality: Default::default(),
                scope: None,
                scopes,
                infrastructure: Default::default(),
                secrets: Vec::new(),
                plugins_dir: None,
                renewals: Vec::new(),
                renewals_notify: None,
            },
        }
    }

    fn store() -> Arc<dyn TaskStore> {
        let ledger: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        Arc::new(ScopedStores::new(ledger, HashMap::new()))
    }

    fn engine(factory: Factory, store: Arc<dyn TaskStore>) -> Engine {
        Engine::new(
            factory,
            Registry::with_builtins(),
            store,
            PathBuf::from("/bin/factory"),
            Vec::new(),
        )
    }

    fn issue(repo: &str, number: u64, title: &str) -> String {
        serde_json::json!([{
            "id": format!("I_{number}"),
            "number": number,
            "title": title,
            "body": "The issue body",
            "url": format!("https://github.com/{repo}/issues/{number}"),
            "author": { "login": "octocat" },
            "createdAt": "2026-09-20T12:34:56Z"
        }])
        .to_string()
    }

    fn script(log: &Path, cases: &[(&str, &str, i32)]) -> String {
        let mut body = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\ncase \"$4\" in\n",
            log.display()
        );
        for (repo, output, status) in cases {
            body.push_str(&format!(
                "  '{repo}') printf '%s' '{}' ; exit {status} ;;\n",
                output.replace('\'', "'\\''")
            ));
        }
        body.push_str("  *) exit 2 ;;\nesac\n");
        body
    }

    #[test]
    fn github_intake_normalizes_https_and_ssh_remotes_only() {
        assert_eq!(
            github_repo("https://github.com/acme/widgets.git"),
            Some("acme/widgets".into())
        );
        assert_eq!(
            github_repo("https://github.com/acme/widgets.git/"),
            Some("acme/widgets".into())
        );
        assert_eq!(
            github_repo("git@github.com:acme/widgets.git"),
            Some("acme/widgets".into())
        );
        assert_eq!(
            github_repo("ssh://git@github.com/acme/widgets"),
            Some("acme/widgets".into())
        );
        assert_eq!(github_repo("https://gitlab.com/acme/widgets.git"), None);
        assert_eq!(github_repo("main"), None);
    }

    #[tokio::test]
    async fn conditional_poll_reuses_the_fixed_url_etag_and_accepts_not_modified() {
        let scratch = Scratch::new();
        let log = scratch.0.join("args");
        let gh = scratch.fixture(&format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\ncase \"$*\" in\n  *If-None-Match*) printf 'HTTP/2 304\\n\\n' ;;\n  *) printf 'HTTP/2 200\\netag: \\\"version-1\\\"\\n\\n[]' ;;\nesac\n",
            log.display()
        ));

        let first = repository_changed(&gh, "acme/widgets", None).await.unwrap();
        let etag = match first {
            ConditionalPoll::Changed(Some(etag)) => etag,
            _ => panic!("the initial response should retain its ETag"),
        };
        assert_eq!(etag, "\"version-1\"");
        assert!(matches!(
            repository_changed(&gh, "acme/widgets", Some(&etag))
                .await
                .unwrap(),
            ConditionalPoll::Unchanged
        ));

        let calls = std::fs::read_to_string(log).unwrap();
        assert_eq!(
            calls
                .matches("repos/acme/widgets/issues?state=open&labels=factory%3Aintake&sort=updated&direction=desc&per_page=1")
                .count(),
            2,
            "the cache key is the same fixed URL on every poll: {calls}"
        );
        assert!(calls.contains("If-None-Match: \"version-1\""), "{calls}");
    }

    #[test]
    fn github_intake_validates_returned_public_issue_urls() {
        assert_eq!(
            github_issue_reference("https://github.com/acme/widgets/issues/17", 17),
            Some("https://github.com/acme/widgets/issues/17".into())
        );
        assert_eq!(
            github_issue_reference("https://github.com/acme/widgets/issues/18", 17),
            None
        );
        assert_eq!(
            github_issue_reference("https://example.com/acme/widgets/issues/17", 17),
            None
        );
        assert_eq!(
            github_issue_reference("https://github.com/acme/widgets/pulls/17", 17),
            None
        );
    }

    #[tokio::test]
    async fn github_intake_uses_the_read_only_labelled_command_and_receives_once() {
        let scratch = Scratch::new();
        let log = scratch.0.join("args");
        let output = issue("acme/widgets", 17, "Widget fails");
        let gh = scratch.fixture(&script(&log, &[("acme/widgets", &output, 0)]));
        let store = store();
        let engine = engine(
            factory(vec![scope("web", "git@github.com:acme/widgets.git")]),
            store.clone(),
        );

        poll_once(&engine, &gh).await;
        poll_once(&engine, &gh).await;

        let tasks = store.list(&TaskFilter::default()).await.unwrap();
        assert_eq!(tasks.len(), 1);
        let task = &tasks[0];
        assert_eq!(task.scope, "web");
        assert_eq!(task.title, "Widget fails");
        assert_eq!(task.instructions, "The issue body");
        assert_eq!(task.runs, 0);
        let intake = task.intake.as_ref().unwrap();
        assert_eq!(intake.source.kind, SourceKind::Github);
        assert_eq!(
            intake.source.reference.as_deref(),
            Some("https://github.com/acme/widgets/issues/17")
        );
        assert_eq!(intake.requester, "octocat");
        assert_eq!(intake.source.repository.as_deref(), Some("acme/widgets"));
        assert_eq!(intake.source.number, Some(17));
        assert_eq!(intake.source.external_id.as_deref(), Some("I_17"));
        assert_eq!(
            intake.received_at,
            "2026-09-20T12:34:56Z".parse::<DateTime<Utc>>().unwrap()
        );

        let calls = std::fs::read_to_string(log).unwrap();
        let expected = "issue list --repo acme/widgets --state open --label factory:intake --limit 1000 --json id,number,title,body,url,author,createdAt,comments\n";
        assert_eq!(calls, expected.repeat(2));
    }

    #[tokio::test]
    async fn github_edits_and_comments_resync_an_open_item_and_requeue_needs_info() {
        let scratch = Scratch::new();
        let log = scratch.0.join("args");
        let first = issue("acme/widgets", 17, "Old title");
        let gh = scratch.fixture(&script(&log, &[("acme/widgets", &first, 0)]));
        let shared = store();
        let engine = engine(factory(vec![scope("web", "git@github.com:acme/widgets.git")]), shared.clone());
        poll_once(&engine, &gh).await;

        let task = shared.list(&TaskFilter::default()).await.unwrap().pop().unwrap();
        let mut intake = task.intake.clone().unwrap();
        intake.stage = IntakeStage::NeedsInfo;
        intake.questions = vec!["Which version?".into()];
        shared
            .update(&task.id, &TaskPatch { intake: Some(intake), ..Default::default() })
            .await
            .unwrap();
        poll_once(&engine, &gh).await;
        assert_eq!(
            shared.get(&task.id).await.unwrap().unwrap().intake.unwrap().stage,
            IntakeStage::NeedsInfo,
            "an unchanged poll is not evidence that the requester answered"
        );

        let changed = serde_json::json!([{
            "id": "I_17",
            "number": 17,
            "title": "New title",
            "body": "Updated body",
            "url": "https://github.com/acme/widgets/issues/17",
            "author": { "login": "octocat" },
            "createdAt": "2026-09-20T12:34:56Z",
            "comments": [{
                "body": "It affects version 2.",
                "author": { "login": "answerer" },
                "createdAt": "2026-09-21T10:00:00Z"
            }]
        }])
        .to_string();
        scratch.fixture(&script(&log, &[("acme/widgets", &changed, 0)]));
        poll_once(&engine, &gh).await;

        let synced = shared.get(&task.id).await.unwrap().unwrap();
        assert_eq!(synced.title, "New title");
        assert!(synced.instructions.contains("Updated body"));
        assert!(synced.instructions.contains("GitHub comment by @answerer"));
        assert!(synced.instructions.contains("It affects version 2."));
        let intake = synced.intake.unwrap();
        assert_eq!(intake.stage, IntakeStage::Received);
        assert!(intake.questions.is_empty());
        assert_eq!(shared.list(&TaskFilter::default()).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn github_intake_uses_canonical_url_across_mixed_case_remote_and_restart() {
        let scratch = Scratch::new();
        let log = scratch.0.join("args");
        let output = issue("acme/widgets", 17, "Widget fails");
        let gh = scratch.fixture(&script(&log, &[("ACME/Widgets", &output, 0)]));
        let shared = store();
        let configured = factory(vec![scope("web", "https://github.com/ACME/Widgets.git")]);
        let running = engine(configured.clone(), shared.clone());
        poll_once(&running, &gh).await;
        poll_once(&running, &gh).await;
        poll_once(&engine(configured.clone(), shared.clone()), &gh).await;
        let tasks = shared.list(&TaskFilter::default()).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(
            tasks[0]
                .intake
                .as_ref()
                .and_then(|intake| intake.source.reference.as_deref()),
            Some("https://github.com/acme/widgets/issues/17")
        );
    }

    #[tokio::test]
    async fn github_intake_contains_repo_failures_and_recovers_without_duplicates() {
        let scratch = Scratch::new();
        let log = scratch.0.join("args");
        let good = issue("acme/good", 2, "Good issue");
        let gh = scratch.fixture(&script(
            &log,
            &[
                ("acme/bad", "authentication failed", 1),
                ("acme/good", &good, 0),
            ],
        ));
        let shared = store();
        let configured = factory(vec![
            scope("api", "https://github.com/acme/bad.git"),
            scope("web", "https://github.com/acme/good.git"),
        ]);
        let engine = engine(configured, shared.clone());

        poll_once(&engine, &gh).await;
        let tasks = shared.list(&TaskFilter::default()).await.unwrap();
        assert_eq!(tasks.len(), 1, "the valid repository proceeds");
        assert_eq!(tasks[0].scope, "web");

        scratch.fixture(&script(
            &log,
            &[("acme/bad", "not json", 0), ("acme/good", &good, 0)],
        ));
        poll_once(&engine, &gh).await;
        assert_eq!(shared.list(&TaskFilter::default()).await.unwrap().len(), 1);

        let recovered = issue("acme/bad", 1, "Recovered issue");
        scratch.fixture(&script(
            &log,
            &[("acme/bad", &recovered, 0), ("acme/good", &good, 0)],
        ));
        poll_once(&engine, &gh).await;
        poll_once(&engine, &gh).await;
        let tasks = shared.list(&TaskFilter::default()).await.unwrap();
        assert_eq!(
            tasks.len(),
            2,
            "recovery adds the missing issue exactly once"
        );
        assert!(tasks
            .iter()
            .any(|task| task.scope == "api" && task.title == "Recovered issue"));
    }
}
