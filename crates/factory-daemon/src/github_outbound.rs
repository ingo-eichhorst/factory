//! Outbound GitHub effects for intake (`#171`): posting a decided item's
//! triage comment and applying its labels to the issue it came from.
//!
//! This is deliberately the daemon's own opt-in action, never something a
//! decision does by itself. `factory_core::intake::check_decision` and
//! `intake.rs`'s `intake_decide` only ever record `OutboundState::AwaitingApproval`
//! (`awaiting_approval_outbound`, below) -- the daemon never calls `gh` on
//! its own initiative. `intake_publish`/`intake_publish_with_gh` is the one
//! path that does, and only when a caller with `intake.publish` (the owner,
//! always, or a role naming the grant exactly -- never a wildcard, never
//! `foreman` or `triager`) asks for it.
//!
//! The GitHub calls follow the poller's own shape (`github_intake.rs`): the
//! `gh` program's path is injectable, exactly so tests can point it at a
//! fixture script instead of the real binary, and every call gets the same
//! 30-second ceiling. Four calls, in order: `gh label list` (so a label
//! that does not exist is skipped, never created), `gh api .../comments
//! --paginate` (to find the comment carrying this item's hidden marker,
//! surviving a crash between an earlier GitHub write and the local one),
//! then a PATCH of that comment or a POST of a new one, and finally `gh
//! issue edit` for the labels that exist. A `gh` failure at any point is
//! caught, journaled as `outbound_failed`, and returned on the card --
//! never a panic, and never something that takes the engine down with it.
//!
//! `needs-triage` is left alone throughout: the poller keys on it, and
//! removing it is a person's own decision.

use crate::access::Caller;
use crate::engine::Engine;
use crate::github_intake::{parse_canonical_issue_url, GH_TIMEOUT};
use crate::operations::Asked;
use chrono::Utc;
use factory_core::error::{FactoryError, Result};
use factory_core::intake::{
    self, Decision, DecisionRecord, Intake, OutboundRecord, OutboundState, SecurityState, SourceKind,
};
use factory_core::task::{Task, TaskPatch};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

/// The fresh `awaiting_approval` record a decision on `record` (`decided`,
/// just made) leaves behind, or `None` when this decision has nothing to
/// publish: not from GitHub, a split (which makes new items rather than
/// deciding this one), no assessment to render, or a possible or confirmed
/// security report (`#170`) -- a vulnerability report a person must never
/// see reflected in a public comment. Called from `intake.rs`'s
/// `intake_decide` for `Ready`, `NeedsInfo` and `Wontfix` alike.
///
/// Recomputed fresh on every decision, replacing whatever the item's
/// outbound state was, rather than merged with it: `intake_publish`'s own
/// marker-based lookup finds the issue's existing comment by its hidden
/// marker regardless of what is stored locally, so nothing here needs to
/// carry a prior `comment_id` forward for the replay to stay idempotent.
pub(crate) fn awaiting_approval_outbound(record: &Intake, decided: &DecisionRecord) -> Option<Box<OutboundRecord>> {
    if record.source.kind != SourceKind::Github {
        return None;
    }
    if matches!(decided.decision, Decision::Split { .. }) {
        return None;
    }
    record.triage.as_ref()?;
    if matches!(
        record.security.as_deref().map(|flag| flag.state),
        Some(SecurityState::Possible | SecurityState::Confirmed)
    ) {
        return None;
    }
    Some(Box::new(OutboundRecord {
        state: OutboundState::AwaitingApproval,
        comment_id: None,
        comment_url: None,
        labels_applied: Vec::new(),
        labels_skipped: Vec::new(),
        digest: None,
        last_error: None,
        by: decided.by.clone(),
        at: decided.at,
    }))
}

impl Engine {
    /// `Request::IntakePublish`, against the real `gh` on `PATH`.
    pub(crate) async fn intake_publish(self: &Arc<Self>, caller: &Caller, id: &str) -> Result<Task> {
        self.intake_publish_with_gh(caller, id, Path::new("gh")).await
    }

    /// The same, with the `gh` program path injectable -- exactly how
    /// `github_intake.rs`'s poller tests bypass the real binary, so these
    /// can too.
    ///
    /// A refusal (not from GitHub, no canonical reference, a possible or
    /// confirmed security report, nothing decided, no assessment, or a
    /// split) is returned before any `gh` call is made and before anything
    /// is journaled. Past that point, every outcome -- success or a `gh`
    /// failure -- writes the item's `outbound` record and journals
    /// `outbound_completed` or `outbound_failed`; this never returns an
    /// error for a `gh` failure; the failure is on the record instead.
    pub(crate) async fn intake_publish_with_gh(self: &Arc<Self>, caller: &Caller, id: &str, gh: &Path) -> Result<Task> {
        let item = self.require(id).await?;
        let record = item
            .intake
            .clone()
            .ok_or_else(|| FactoryError::BadRequest(format!("{} was not handed in through intake", item.id)))?;

        if record.source.kind != SourceKind::Github {
            return Err(FactoryError::BadRequest(format!(
                "{} is a {} item, not a GitHub one; only a GitHub-sourced item can be published",
                item.id,
                record.source.kind.as_str()
            )));
        }
        let reference = record
            .source
            .reference
            .as_deref()
            .ok_or_else(|| FactoryError::BadRequest(format!("{} carries no GitHub reference to publish to", item.id)))?;
        let (owner, repo, number) = parse_canonical_issue_url(reference)
            .ok_or_else(|| FactoryError::BadRequest(format!("{reference:?} is not a canonical GitHub issue URL")))?;
        // A vulnerability report is never disclosed in a public comment,
        // possible or confirmed alike (`#170`). The daemon also never
        // records `awaiting_approval` for one (`awaiting_approval_outbound`,
        // above), but this is the check that actually stops the GitHub
        // write, whatever got the item here.
        if let Some(state) = record.security.as_ref().map(|flag| flag.state) {
            if matches!(state, SecurityState::Possible | SecurityState::Confirmed) {
                return Err(FactoryError::BadRequest(format!(
                    "{} carries a {} security report; publishing it to a public GitHub comment would disclose it",
                    item.id,
                    state.as_str()
                )));
            }
        }
        let decided = record
            .decision
            .clone()
            .ok_or_else(|| FactoryError::BadRequest(format!("{} has not been decided yet; nothing to publish", item.id)))?;
        if matches!(decided.decision, Decision::Split { .. }) {
            return Err(FactoryError::BadRequest(format!(
                "{} was split, not decided; a split has nothing of its own to publish",
                item.id
            )));
        }
        let triage = record
            .triage
            .clone()
            .ok_or_else(|| FactoryError::BadRequest(format!("{} carries no assessment to publish", item.id)))?;

        let comment = intake::triage_comment(&item, &triage, &decided.decision);
        let plan = intake::github_labels(&triage, &decided.decision);
        let marker = intake::outbound_marker(&item.id);
        let digest = digest_of(&comment, &plan);
        let repo_full = format!("{owner}/{repo}");

        let asked = Asked::new(caller, None);
        self.entry(
            &item.id,
            asked.entry(
                "outbound_attempted",
                format!("attempting to publish {} to GitHub {repo_full}#{number} {}", item.id, asked.words()),
                serde_json::json!({ "digest": digest, "repo": repo_full, "number": number }),
            ),
        )
        .await;

        let now = Utc::now();
        let by = caller.describe();
        let outbound = match publish_to_github(gh, &repo_full, number, reference, &comment, &marker, &plan).await {
            Ok(outcome) => {
                self.entry(
                    &item.id,
                    asked.entry(
                        "outbound_completed",
                        format!("published {} to GitHub: {}", item.id, outcome.comment_url),
                        serde_json::json!({
                            "comment_url": outcome.comment_url,
                            "labels_applied": outcome.labels_applied,
                            "labels_skipped": outcome.labels_skipped,
                        }),
                    ),
                )
                .await;
                Box::new(OutboundRecord {
                    state: OutboundState::Published,
                    comment_id: Some(outcome.comment_id),
                    comment_url: Some(outcome.comment_url),
                    labels_applied: outcome.labels_applied,
                    labels_skipped: outcome.labels_skipped,
                    digest: Some(digest),
                    last_error: None,
                    by,
                    at: now,
                })
            }
            Err(error) => {
                self.entry(
                    &item.id,
                    asked.entry(
                        "outbound_failed",
                        format!("publishing {} to GitHub failed: {error}", item.id),
                        serde_json::json!({ "error": error }),
                    ),
                )
                .await;
                let prior = record.outbound.as_deref();
                Box::new(OutboundRecord {
                    state: OutboundState::Failed,
                    comment_id: prior.and_then(|o| o.comment_id),
                    comment_url: prior.and_then(|o| o.comment_url.clone()),
                    labels_applied: prior.map(|o| o.labels_applied.clone()).unwrap_or_default(),
                    labels_skipped: prior.map(|o| o.labels_skipped.clone()).unwrap_or_default(),
                    digest: Some(digest),
                    last_error: Some(error),
                    by,
                    at: now,
                })
            }
        };
        let mut next = record;
        next.outbound = Some(outbound);
        self.write_intake(&item.id, next, TaskPatch::default()).await
    }
}

/// What one successful publish leaves behind.
struct PublishOutcome {
    comment_id: u64,
    comment_url: String,
    labels_applied: Vec<String>,
    labels_skipped: Vec<String>,
}

/// The four `gh` calls, in order: labels that exist, the comment carrying
/// `marker` (PATCH) or none (POST), then the label edit -- skipped
/// entirely when there is nothing to add or remove, since `gh issue edit`
/// itself refuses a call with neither flag.
async fn publish_to_github(
    gh: &Path,
    repo: &str,
    number: u64,
    issue_url: &str,
    comment: &str,
    marker: &str,
    plan: &intake::LabelPlan,
) -> std::result::Result<PublishOutcome, String> {
    let existing: HashSet<String> = gh_label_list(gh, repo).await?.into_iter().collect();
    let mut labels_applied = Vec::new();
    let mut labels_skipped = Vec::new();
    let mut add = Vec::new();
    for label in &plan.add {
        if existing.contains(label) {
            add.push(label.clone());
            labels_applied.push(label.clone());
        } else {
            labels_skipped.push(label.clone());
        }
    }
    let remove: Vec<String> = plan.remove.iter().filter(|label| existing.contains(*label)).cloned().collect();

    let comments = gh_list_comments(gh, repo, number).await?;
    let found = comments.into_iter().find(|c| c.body.contains(marker));
    let (comment_id, comment_url) = match found {
        Some(existing) => {
            gh_patch_comment(gh, repo, existing.id, comment).await?;
            (existing.id, format!("{issue_url}#issuecomment-{}", existing.id))
        }
        None => {
            let id = gh_post_comment(gh, repo, number, comment).await?;
            (id, format!("{issue_url}#issuecomment-{id}"))
        }
    };
    if !add.is_empty() || !remove.is_empty() {
        gh_issue_edit_labels(gh, repo, number, &add, &remove).await?;
    }
    Ok(PublishOutcome { comment_id, comment_url, labels_applied, labels_skipped })
}

async fn run_gh(gh: &Path, args: &[&str]) -> std::result::Result<std::process::Output, String> {
    let mut command = tokio::process::Command::new(gh);
    command.kill_on_drop(true);
    command.args(args);
    tokio::time::timeout(GH_TIMEOUT, command.output())
        .await
        .map_err(|_| format!("gh {} timed out after {} seconds", args.join(" "), GH_TIMEOUT.as_secs()))?
        .map_err(|error| format!("starting gh {}: {error}", args.join(" ")))
}

fn gh_ok(args: &[&str], output: &std::process::Output) -> std::result::Result<(), String> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if stderr.is_empty() {
        format!("gh {} exited with {}", args.join(" "), output.status)
    } else {
        format!("gh {} exited with {}: {stderr}", args.join(" "), output.status)
    })
}

#[derive(Debug, Deserialize)]
struct GhLabel {
    name: String,
}

/// Every label the repository already has -- `--limit 1000` so a
/// label-heavy repository is not silently truncated to `gh`'s own default
/// of 30.
async fn gh_label_list(gh: &Path, repo: &str) -> std::result::Result<Vec<String>, String> {
    let args = ["label", "list", "--repo", repo, "--json", "name", "--limit", "1000"];
    let output = run_gh(gh, &args).await?;
    gh_ok(&args, &output)?;
    let labels: Vec<GhLabel> =
        serde_json::from_slice(&output.stdout).map_err(|error| format!("invalid gh label list JSON: {error}"))?;
    Ok(labels.into_iter().map(|label| label.name).collect())
}

#[derive(Debug, Deserialize)]
struct GhComment {
    id: u64,
    body: String,
}

async fn gh_list_comments(gh: &Path, repo: &str, number: u64) -> std::result::Result<Vec<GhComment>, String> {
    let endpoint = format!("repos/{repo}/issues/{number}/comments");
    let args = ["api", endpoint.as_str(), "--paginate"];
    let output = run_gh(gh, &args).await?;
    gh_ok(&args, &output)?;
    parse_paginated_comments(&output.stdout)
}

/// `gh api --paginate` over an array endpoint prints one JSON array per
/// page, concatenated -- not one combined array -- so every document in the
/// output is read rather than assuming a single page.
fn parse_paginated_comments(bytes: &[u8]) -> std::result::Result<Vec<GhComment>, String> {
    let mut out = Vec::new();
    for page in serde_json::Deserializer::from_slice(bytes).into_iter::<Vec<GhComment>>() {
        out.extend(page.map_err(|error| format!("invalid gh api comments JSON: {error}"))?);
    }
    Ok(out)
}

async fn gh_post_comment(gh: &Path, repo: &str, number: u64, body: &str) -> std::result::Result<u64, String> {
    let endpoint = format!("repos/{repo}/issues/{number}/comments");
    let field = format!("body={body}");
    let args = ["api", endpoint.as_str(), "--method", "POST", "-f", field.as_str()];
    let output = run_gh(gh, &args).await?;
    gh_ok(&args, &output)?;
    #[derive(Deserialize)]
    struct Created {
        id: u64,
    }
    let created: Created =
        serde_json::from_slice(&output.stdout).map_err(|error| format!("invalid gh api comment-creation JSON: {error}"))?;
    Ok(created.id)
}

async fn gh_patch_comment(gh: &Path, repo: &str, comment_id: u64, body: &str) -> std::result::Result<(), String> {
    let endpoint = format!("repos/{repo}/issues/comments/{comment_id}");
    let field = format!("body={body}");
    let args = ["api", endpoint.as_str(), "--method", "PATCH", "-f", field.as_str()];
    let output = run_gh(gh, &args).await?;
    gh_ok(&args, &output)
}

async fn gh_issue_edit_labels(
    gh: &Path,
    repo: &str,
    number: u64,
    add: &[String],
    remove: &[String],
) -> std::result::Result<(), String> {
    let number = number.to_string();
    let add_joined = add.join(",");
    let remove_joined = remove.join(",");
    let mut args: Vec<&str> = vec!["issue", "edit", number.as_str(), "--repo", repo];
    if !add.is_empty() {
        args.push("--add-label");
        args.push(add_joined.as_str());
    }
    if !remove.is_empty() {
        args.push("--remove-label");
        args.push(remove_joined.as_str());
    }
    let output = run_gh(gh, &args).await?;
    gh_ok(&args, &output)
}

/// A digest of the comment text and label plan, so a re-triage that changed
/// nothing is easy to tell from one that did (`OutboundRecord::digest`).
fn digest_of(comment: &str, plan: &intake::LabelPlan) -> String {
    let mut hasher = Sha256::new();
    hasher.update(comment.as_bytes());
    hasher.update(b"\0add\0");
    for label in &plan.add {
        hasher.update(label.as_bytes());
        hasher.update(b"\0");
    }
    hasher.update(b"\0remove\0");
    for label in &plan.remove {
        hasher.update(label.as_bytes());
        hasher.update(b"\0");
    }
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stores::ScopedStores;
    use factory_core::adapter::TaskStore;
    use factory_core::config::{Config, DaemonConfig, Factory, Instance, Scope};
    use factory_core::intake::{
        Assessment, Axis, AxisCheck, IntakeSource, IntakeStage, Level, Routing, SecurityVerdict, WontfixReason,
    };
    use factory_core::task::NewTask;
    use factory_plugins::{Registry, SqliteStore};
    use std::collections::HashMap;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("factory-github-outbound-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        /// A fake `gh` that logs every call's argv (one line, shell-split
        /// away) to `log` and dispatches on `$1 $2` (`label list`, `api`,
        /// `issue edit`) -- a script per test, not per call, since a
        /// publish makes several calls in sequence.
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

    fn scope(name: &str) -> Scope {
        Scope {
            id: format!("{name}-id"),
            name: name.into(),
            path: PathBuf::new(),
            agent: None,
            agents: Vec::new(),
            runtime: Some("herdr".into()),
            git: Some(format!("https://github.com/acme/{name}.git")),
            task_store: None,
            max_sessions: None,
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
                instance: Instance { id: "test".into(), name: "test".into() },
                daemon: DaemonConfig { power_assertion: false, default_agent: "shell".into(), ..Default::default() },
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

    fn engine(factory: Factory, store: Arc<dyn TaskStore>) -> Arc<Engine> {
        Arc::new(Engine::new(factory, Registry::with_builtins(), store, PathBuf::from("/bin/factory"), Vec::new()))
    }

    fn test_engine() -> Arc<Engine> {
        engine(factory(vec![scope("widgets")]), store())
    }

    /// A GitHub-sourced item, received the way the poller would (`#171`'s
    /// task: "construct one the way poller tests do") -- straight through
    /// `receive_intake`, never through `intake add`, which refuses a
    /// caller-claimed GitHub source.
    async fn github_item(engine: &Arc<Engine>, scope: &str, number: u64) -> Task {
        let reference = format!("https://github.com/acme/{scope}/issues/{number}");
        let record = Intake {
            stage: IntakeStage::Received,
            source: Box::new(IntakeSource { kind: SourceKind::Github, reference: Some(reference), provider: None, relayed_by: None }),
            requester: "octocat".into(),
            received_at: Utc::now(),
            triage: None,
            triage_task: None,
            questions: Vec::new(),
            decision: None,
            candidates: Vec::new(),
            security: None,
            outbound: None,
        };
        engine
            .receive_intake(
                NewTask { title: "Widget fails".into(), instructions: "It falls over".into(), scope: Some(scope.into()), ..Default::default() },
                record,
            )
            .await
            .unwrap()
    }

    /// A full, valid assessment (all seven axes pass) with no agent named,
    /// so `intake_assess` never has to resolve one -- irrelevant to
    /// publishing, which only reads the recorded `Triage`.
    fn full_assessment(scope: &str) -> Assessment {
        Assessment {
            axes: Axis::ALL.into_iter().map(|axis| AxisCheck { axis, pass: true, evidence: "holds".into(), cost: None }).collect(),
            category: "bugfix".into(),
            impact: Level::High,
            urgency: Level::Medium,
            complexity: 3,
            estimate: None,
            routing: Routing { scope: scope.into(), ..Default::default() },
            summary: "A bounded fix.".into(),
            questions: vec![],
            split: vec![],
            duplicates: vec![],
            checks: vec![],
            areas: vec!["process".into()],
        }
    }

    /// Assess (recording a `Triage`) and decide `NeedsInfo` -- the simplest
    /// path to a GitHub item with an assessment and a decision, needing no
    /// agent or workflow resolution.
    async fn assessed_and_sent_back(engine: &Arc<Engine>, id: &str, scope: &str) -> Task {
        engine.intake_assess(&Caller::Owner, id, full_assessment(scope), false).await.unwrap();
        engine
            .intake_decide(&Caller::Owner, id, Decision::NeedsInfo { questions: vec!["what browser?".into()] })
            .await
            .unwrap()
    }

    fn outbound_of(task: &Task) -> OutboundRecord {
        (**task.intake.as_ref().unwrap().outbound.as_ref().unwrap()).clone()
    }

    /// A fake `gh` that logs its full argv (one line per call) and
    /// dispatches on a comma-joined key built from it, so `label list`, the
    /// paginated comment read, a POST, a PATCH (a different endpoint,
    /// `issues/comments/<id>`) and `issue edit` are all distinguishable in
    /// one script -- a plain "first two words" prefix collides between the
    /// paginated read and a POST, since both hit
    /// `issues/<n>/comments`. Each `cases` pattern is a comma-joined prefix
    /// of that key (e.g. `"api,repos/o/r/issues/1/comments,--paginate,"`);
    /// the trailing comma anchors it to a token boundary.
    fn gh_script(log: &Path, cases: &[(&str, &str, i32)]) -> String {
        let mut body = format!(
            "#!/bin/sh\nprintf '@@CALL@@ %s\\n' \"$*\" >> '{}'\nkey=$(printf '%s,' \"$@\")\ncase \"$key\" in\n",
            log.display()
        );
        for (pattern, output, status) in cases {
            body.push_str(&format!(
                "  {pattern}*) printf '%s' '{}' ; exit {status} ;;\n",
                output.replace('\'', "'\\''")
            ));
        }
        body.push_str("  *) printf 'unexpected call: '\"$key\" >&2; exit 2 ;;\nesac\n");
        body
    }

    fn labels_json(names: &[&str]) -> String {
        serde_json::to_string(&names.iter().map(|n| serde_json::json!({ "name": n })).collect::<Vec<_>>()).unwrap()
    }

    /// The logged calls, each argv exactly as `"$*"` saw it (a comment's
    /// body carries embedded newlines, so a plain `.lines()` over the log
    /// would split one call into several -- this splits on the script's own
    /// `@@CALL@@` marker instead, whatever a call's own argv contains).
    fn calls_in(log: &Path) -> Vec<String> {
        std::fs::read_to_string(log)
            .unwrap()
            .split("@@CALL@@ ")
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_end_matches('\n').to_string())
            .collect()
    }

    // Access control itself -- who may call `IntakePublish` at all -- is
    // `access.rs`'s own matrix (`intake_publish_needs_its_own_grant...`),
    // exercised through `authorize`, which every wire request passes
    // through before `Engine::dispatch_request` ever reaches
    // `intake_publish`. Nothing here re-tests that; these cover what
    // `intake_publish_with_gh` itself refuses, and how it talks to `gh`.

    #[tokio::test]
    async fn a_first_publish_posts_once_with_the_marker_and_the_expected_labels() {
        let engine = test_engine();
        let item = github_item(&engine, "widgets", 42).await;
        let decided = assessed_and_sent_back(&engine, &item.id, "widgets").await;
        assert_eq!(outbound_of(&decided).state, OutboundState::AwaitingApproval);

        let scratch = Scratch::new();
        let log = scratch.0.join("args");
        let gh = scratch.fixture(&gh_script(
            &log,
            &[
                ("label,list,", &labels_json(&["needs-info", "ready-for-agent", "wontfix", "bug", "enhancement", "process"]), 0),
                ("api,repos/acme/widgets/issues/42/comments,--paginate,", "[]", 0),
                ("api,repos/acme/widgets/issues/42/comments,--method,POST,", r#"{"id": 501}"#, 0),
                ("issue,edit,42,", "", 0),
            ],
        ));

        let published = engine.intake_publish_with_gh(&Caller::Owner, &item.id, &gh).await.unwrap();
        let outbound = outbound_of(&published);
        assert_eq!(outbound.state, OutboundState::Published);
        assert_eq!(outbound.comment_id, Some(501));
        assert_eq!(outbound.comment_url.as_deref(), Some("https://github.com/acme/widgets/issues/42#issuecomment-501"));
        assert!(outbound.labels_skipped.is_empty(), "{:?}", outbound.labels_skipped);
        assert!(outbound.labels_applied.contains(&"needs-info".to_string()));

        let calls = calls_in(&log);
        assert_eq!(calls.len(), 4, "{calls:?}");
        assert!(calls[0].starts_with("label list --repo acme/widgets"), "{}", calls[0]);
        assert!(calls[1].starts_with("api repos/acme/widgets/issues/42/comments --paginate"), "{}", calls[1]);
        assert!(calls[2].contains("--method POST"), "{}", calls[2]);
        assert!(calls[2].contains(&intake::outbound_marker(&item.id)), "the marker is in the posted body: {}", calls[2]);
        assert!(calls[3].starts_with("issue edit 42 --repo acme/widgets"), "{}", calls[3]);
        assert!(calls[3].contains("--add-label"), "{}", calls[3]);
        assert!(calls[3].contains("--remove-label"), "{}", calls[3]);
    }

    #[tokio::test]
    async fn a_re_triage_and_second_publish_patches_the_same_comment_and_never_posts_again() {
        let engine = test_engine();
        let item = github_item(&engine, "widgets", 7).await;
        assessed_and_sent_back(&engine, &item.id, "widgets").await;

        let scratch = Scratch::new();
        let log = scratch.0.join("args");
        let gh = scratch.fixture(&gh_script(
            &log,
            &[
                ("label,list,", &labels_json(&["needs-info", "ready-for-agent", "wontfix"]), 0),
                ("api,repos/acme/widgets/issues/7/comments,--paginate,", "[]", 0),
                ("api,repos/acme/widgets/issues/7/comments,--method,POST,", r#"{"id": 900}"#, 0),
                ("issue,edit,7,", "", 0),
            ],
        ));
        let first = engine.intake_publish_with_gh(&Caller::Owner, &item.id, &gh).await.unwrap();
        assert_eq!(outbound_of(&first).comment_id, Some(900));

        // Re-triage: a fresh assessment and a fresh `NeedsInfo`, which clears
        // the stored `comment_id` (a new `awaiting_approval`) even though
        // the GitHub comment itself has not moved.
        engine.intake_assess(&Caller::Owner, &item.id, full_assessment("widgets"), false).await.unwrap();
        let redecided = engine
            .intake_decide(&Caller::Owner, &item.id, Decision::NeedsInfo { questions: vec!["still reproducing?".into()] })
            .await
            .unwrap();
        assert_eq!(outbound_of(&redecided).state, OutboundState::AwaitingApproval);
        assert_eq!(outbound_of(&redecided).comment_id, None, "cleared, not carried forward");

        let existing_comment = serde_json::json!([{ "id": 900, "body": format!("stale text\n\n{}", intake::outbound_marker(&item.id)) }]).to_string();
        let log2 = scratch.0.join("args2");
        let gh2 = scratch.fixture(&gh_script(
            &log2,
            &[
                ("label,list,", &labels_json(&["needs-info", "ready-for-agent", "wontfix"]), 0),
                ("api,repos/acme/widgets/issues/7/comments,--paginate,", &existing_comment, 0),
                ("api,repos/acme/widgets/issues/comments/900,--method,PATCH,", r#"{"id": 900}"#, 0),
                ("issue,edit,7,", "", 0),
            ],
        ));
        let second = engine.intake_publish_with_gh(&Caller::Owner, &item.id, &gh2).await.unwrap();
        let outbound = outbound_of(&second);
        assert_eq!(outbound.state, OutboundState::Published);
        assert_eq!(outbound.comment_id, Some(900), "the marker found the existing comment");

        let calls = calls_in(&log2);
        assert_eq!(calls.len(), 4, "label list, the paginated read, PATCH, then the label edit -- no POST: {calls:?}");
        assert!(calls[2].contains("--method PATCH"), "{}", calls[2]);
        assert!(calls[2].contains("/issues/comments/900"), "{}", calls[2]);
        assert!(!calls.iter().any(|c| c.contains("--method POST")), "no POST on replay: {calls:?}");
    }

    #[tokio::test]
    async fn a_crash_before_the_local_write_still_replays_as_a_patch_not_a_second_post() {
        let engine = test_engine();
        let item = github_item(&engine, "widgets", 3).await;
        assessed_and_sent_back(&engine, &item.id, "widgets").await;
        // No prior publish attempt ever completed locally (comment_id is
        // None), but GitHub already carries the comment -- as if an earlier
        // attempt posted it and the daemon crashed before writing the
        // record back.
        let scratch = Scratch::new();
        let log = scratch.0.join("args");
        let existing_comment =
            serde_json::json!([{ "id": 77, "body": format!("earlier attempt\n\n{}", intake::outbound_marker(&item.id)) }]).to_string();
        let gh = scratch.fixture(&gh_script(
            &log,
            &[
                ("label,list,", &labels_json(&["needs-info", "ready-for-agent", "wontfix"]), 0),
                ("api,repos/acme/widgets/issues/3/comments,--paginate,", &existing_comment, 0),
                ("api,repos/acme/widgets/issues/comments/77,--method,PATCH,", r#"{"id": 77}"#, 0),
                ("issue,edit,3,", "", 0),
            ],
        ));
        let replayed = engine.intake_publish_with_gh(&Caller::Owner, &item.id, &gh).await.unwrap();
        let outbound = outbound_of(&replayed);
        assert_eq!(outbound.state, OutboundState::Published);
        assert_eq!(outbound.comment_id, Some(77));
        let calls = calls_in(&log);
        assert!(!calls.iter().any(|c| c.contains("--method POST")), "the marker was found; nothing is posted twice: {calls:?}");
        assert!(calls.iter().any(|c| c.contains("--method PATCH")), "{calls:?}");
    }

    #[tokio::test]
    async fn missing_labels_are_skipped_and_journaled_not_created() {
        let engine = test_engine();
        let item = github_item(&engine, "widgets", 11).await;
        assessed_and_sent_back(&engine, &item.id, "widgets").await;
        let scratch = Scratch::new();
        let log = scratch.0.join("args");
        // The repository has none of the labels intake would apply.
        let gh = scratch.fixture(&gh_script(
            &log,
            &[
                ("label,list,", &labels_json(&[]), 0),
                ("api,repos/acme/widgets/issues/11/comments,--paginate,", "[]", 0),
                ("api,repos/acme/widgets/issues/11/comments,--method,POST,", r#"{"id": 1}"#, 0),
            ],
        ));
        let published = engine.intake_publish_with_gh(&Caller::Owner, &item.id, &gh).await.unwrap();
        let outbound = outbound_of(&published);
        assert_eq!(outbound.state, OutboundState::Published);
        assert!(outbound.labels_applied.is_empty());
        assert!(outbound.labels_skipped.contains(&"needs-info".to_string()));
        let calls = calls_in(&log);
        assert!(!calls.iter().any(|c| c.starts_with("issue edit")), "no label call at all when nothing exists to add or remove: {calls:?}");

        let entries = engine.store.entries(&item.id, 200).await.unwrap();
        assert!(entries.iter().any(|e| e.kind == "outbound_completed"));
    }

    #[tokio::test]
    async fn a_gh_failure_is_recorded_as_failed_never_panics_and_the_engine_keeps_serving() {
        let engine = test_engine();
        let item = github_item(&engine, "widgets", 13).await;
        assessed_and_sent_back(&engine, &item.id, "widgets").await;
        let scratch = Scratch::new();
        let log = scratch.0.join("args");
        let gh = scratch.fixture(&gh_script(&log, &[("label,list,", "not json", 1)]));

        let result = engine.intake_publish_with_gh(&Caller::Owner, &item.id, &gh).await.unwrap();
        let outbound = outbound_of(&result);
        assert_eq!(outbound.state, OutboundState::Failed);
        assert!(outbound.last_error.is_some());

        let entries = engine.store.entries(&item.id, 200).await.unwrap();
        assert!(entries.iter().any(|e| e.kind == "outbound_failed"));
        // The engine is still there to ask.
        assert!(engine.store.list(&Default::default()).await.is_ok());
    }

    #[tokio::test]
    async fn a_non_github_item_is_refused_before_any_gh_call() {
        let engine = test_engine();
        let item = engine
            .intake_add(&Caller::Owner, factory_core::intake::NewIntake { title: "x".into(), scope: Some("widgets".into()), ..Default::default() })
            .await
            .unwrap();
        assessed_and_sent_back(&engine, &item.id, "widgets").await;
        let scratch = Scratch::new();
        let log = scratch.0.join("args");
        let gh = scratch.fixture("#!/bin/sh\nexit 9\n");
        let why = engine.intake_publish_with_gh(&Caller::Owner, &item.id, &gh).await.unwrap_err().to_string();
        assert!(why.contains("not a GitHub"), "{why}");
        assert!(!log.exists());
    }

    #[tokio::test]
    async fn a_possible_or_confirmed_security_report_is_refused_before_any_gh_call() {
        for (label, confirm) in [("possible", false), ("confirmed", true)] {
            // A fresh engine per case: two items with the same title in one
            // scope would otherwise flag each other as a possible duplicate
            // at receipt, and `intake_assess` refuses one left unanswered --
            // irrelevant to what this test checks.
            let engine = test_engine();
            let item = github_item(&engine, "widgets", if confirm { 20 } else { 21 }).await;
            engine.intake_flag_security(&Caller::Owner, &item.id, "looks like an injection").await.unwrap();
            if confirm {
                engine.intake_security_decision(&Caller::Owner, &item.id, SecurityVerdict::Confirm, "").await.unwrap();
            }
            // NeedsInfo is the one decision allowed while `possible`, and
            // `Confirmed` allows it too -- only wontfix is refused for a
            // confirmed report (`check_decision`).
            engine.intake_assess(&Caller::Owner, &item.id, full_assessment("widgets"), false).await.unwrap();
            engine
                .intake_decide(&Caller::Owner, &item.id, Decision::NeedsInfo { questions: vec!["more detail?".into()] })
                .await
                .unwrap();

            let scratch = Scratch::new();
            let gh = scratch.fixture("#!/bin/sh\nexit 9\n");
            let why = engine.intake_publish_with_gh(&Caller::Owner, &item.id, &gh).await.unwrap_err().to_string();
            assert!(why.contains("security report"), "{label}: {why}");
        }
    }

    #[tokio::test]
    async fn wontfix_labels_add_the_reason_and_remove_the_other_two_state_labels() {
        let engine = test_engine();
        let item = github_item(&engine, "widgets", 55).await;
        engine.intake_assess(&Caller::Owner, &item.id, full_assessment("widgets"), false).await.unwrap();
        let decided = engine
            .intake_decide(
                &Caller::Owner,
                &item.id,
                Decision::Wontfix { reason: WontfixReason::Duplicate, evidence: "same as #12".into(), duplicate_of: Some("#12".into()) },
            )
            .await
            .unwrap();
        assert_eq!(outbound_of(&decided).state, OutboundState::AwaitingApproval);

        let scratch = Scratch::new();
        let log = scratch.0.join("args");
        let gh = scratch.fixture(&gh_script(
            &log,
            &[
                ("label,list,", &labels_json(&["needs-info", "ready-for-agent", "wontfix", "duplicate", "bug", "process"]), 0),
                ("api,repos/acme/widgets/issues/55/comments,--paginate,", "[]", 0),
                ("api,repos/acme/widgets/issues/55/comments,--method,POST,", r#"{"id": 2}"#, 0),
                ("issue,edit,55,", "", 0),
            ],
        ));
        engine.intake_publish_with_gh(&Caller::Owner, &item.id, &gh).await.unwrap();
        let calls = calls_in(&log);
        let edit_call = calls.iter().find(|c| c.starts_with("issue edit")).unwrap();
        assert!(edit_call.contains("wontfix"), "{edit_call}");
        assert!(edit_call.contains("duplicate"), "{edit_call}");
        assert!(edit_call.contains("--remove-label ready-for-agent,needs-info"), "{edit_call}");
    }
}
