//! Read-only receipt of labelled GitHub issues into Intake (`#119`).
//!
//! This is deliberately its own daemon loop rather than scheduler work: a
//! slow or unavailable GitHub CLI must not delay task dispatch. GitHub is
//! only read here; later edits to an issue never synchronize or withdraw the
//! intake item already received.

use crate::engine::Engine;
use chrono::{DateTime, Utc};
use factory_core::intake::{Intake, IntakeSource, IntakeStage, SourceKind};
use factory_core::{NewTask, TaskFilter};
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const POLL_INTERVAL: Duration = Duration::from_secs(60);
const GH_TIMEOUT: Duration = Duration::from_secs(30);
const ISSUE_ARGS: [&str; 11] = [
    "issue",
    "list",
    "--repo",
    // repository is inserted here
    "--state",
    "open",
    "--label",
    "needs-triage",
    "--limit",
    "1000",
    "--json",
    "number,title,body,url,author,createdAt",
];

#[derive(Debug, Deserialize)]
struct GithubAuthor {
    login: String,
}

#[derive(Debug, Deserialize)]
struct GithubIssue {
    number: u64,
    title: String,
    #[serde(default)]
    body: String,
    url: String,
    author: Option<GithubAuthor>,
    #[serde(rename = "createdAt")]
    created_at: DateTime<Utc>,
}

pub async fn run(engine: Arc<Engine>, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let mut ticker = tokio::time::interval(POLL_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = ticker.tick() => poll_once(&engine, Path::new("gh")).await,
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { return; }
            }
        }
    }
}

/// Poll every GitHub-backed scope once. Failures are deliberately contained
/// to one scope so the next repository, and the next interval, still run.
async fn poll_once(engine: &Engine, gh: &Path) {
    let tasks = match engine.store.list(&TaskFilter::default()).await {
        Ok(tasks) => tasks,
        Err(error) => {
            tracing::warn!("could not list intake items before polling GitHub: {error}");
            return;
        }
    };
    let mut received: HashSet<String> = tasks
        .iter()
        .filter_map(|task| task.intake.as_ref())
        .filter(|intake| intake.source.kind == SourceKind::Github)
        .filter_map(|intake| intake.source.reference.clone())
        .collect();

    let scopes = engine.factory_snapshot().config.scopes;
    for scope in scopes {
        let Some(repo) = scope.git.as_deref().and_then(github_repo) else {
            continue;
        };
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
            if received.contains(&reference) {
                continue;
            }
            let requester = issue
                .author
                .map(|author| author.login)
                .filter(|login| !login.trim().is_empty())
                .unwrap_or_else(|| "unknown GitHub user".into());
            let record = Intake {
                stage: IntakeStage::Received,
                source: IntakeSource {
                    kind: SourceKind::Github,
                    reference: Some(reference.clone()),
                },
                requester,
                received_at: issue.created_at,
                triage: None,
                triage_task: None,
                questions: Vec::new(),
                decision: None,
                candidates: Vec::new(),
            };
            let new = NewTask {
                title: issue.title,
                instructions: issue.body,
                scope: Some(scope.name.clone()),
                ..Default::default()
            };
            match engine.receive_intake(new, record).await {
                Ok(_) => {
                    received.insert(reference);
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
    let path = url.strip_prefix("https://github.com/")?;
    let mut parts = path.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    let issues = parts.next()?;
    let url_number = parts.next()?;
    if owner.is_empty()
        || repo.is_empty()
        || issues != "issues"
        || url_number.parse::<u64>().ok()? != number
        || parts.next().is_some()
        || owner.chars().any(char::is_whitespace)
        || repo.chars().any(char::is_whitespace)
    {
        return None;
    }
    Some(url.to_owned())
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
            roles: Default::default(),
            dashboard: None,
            policies: Default::default(),
            quality: Default::default(),
            intake: Default::default(),
            dependencies: Default::default(),
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
                plugins_dir: None,
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
        assert_eq!(
            intake.received_at,
            "2026-09-20T12:34:56Z".parse::<DateTime<Utc>>().unwrap()
        );

        let calls = std::fs::read_to_string(log).unwrap();
        let expected = "issue list --repo acme/widgets --state open --label needs-triage --limit 1000 --json number,title,body,url,author,createdAt\n";
        assert_eq!(calls, expected.repeat(2));
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
