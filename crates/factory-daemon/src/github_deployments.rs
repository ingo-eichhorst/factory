//! L4's opt-in, explicitly approved GitHub metadata publisher. Neither
//! recording/finishing a deployment nor reading a page ever invokes gh.
use crate::{access::Caller, engine::Engine, facts::Facts};
use chrono::Utc;
use factory_core::error::{FactoryError, Result};
use factory_kernel::{
    DeploymentMirrorFact, DeploymentMirrorPhase as Phase, DeploymentMirrorPlan,
    DeploymentPublicationFact, L4,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{path::Path, process::Stdio, sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const TASK: &str = "factory:mirror";
const MAX_PAGES: usize = 32;
const MAX_RESPONSE: u64 = 2 * 1024 * 1024;

fn bad(message: &str) -> FactoryError {
    FactoryError::BadRequest(message.into())
}
fn remote_error(message: &str) -> FactoryError {
    FactoryError::adapter("GitHub deployment mirror", message)
}

fn plan_from(fact: DeploymentPublicationFact) -> Result<DeploymentMirrorPlan> {
    let repository = fact
        .repository
        .ok_or_else(|| bad("this environment has not opted into github_deployments"))?;
    if !factory_core::environments::valid_github_repository(&repository) {
        return Err(bad("invalid GitHub owner/repo"));
    }
    if fact.dirty
        || !matches!(fact.commit.len(), 40 | 64)
        || !fact.commit.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(bad(
            "mirroring requires a clean release with its full immutable commit SHA",
        ));
    }
    if !matches!(
        fact.state.as_str(),
        "in_progress" | "success" | "failure" | "inactive"
    ) {
        return Err(bad("unsupported recorded deployment state"));
    }
    let mut plan = DeploymentMirrorPlan {
        deployment: fact.deployment,
        scope: fact.scope,
        repository,
        environment: fact.environment,
        // Git object ids are case-insensitive, while GitHub returns lowercase.
        commit: fact.commit.to_ascii_lowercase(),
        state: fact.state,
        verified: fact.verified,
        transient: fact.transient,
        production: fact.production,
        approval: String::new(),
    };
    let bytes =
        serde_json::to_vec(&plan).map_err(|_| bad("could not encode the mirror approval"))?;
    plan.approval = format!("{:x}", Sha256::digest(bytes));
    Ok(plan)
}

fn receipt(
    plan: &DeploymentMirrorPlan,
    caller: &Caller,
    phase: Phase,
    remote_id: Option<u64>,
    status_id: Option<u64>,
    error: Option<String>,
) -> DeploymentMirrorFact {
    DeploymentMirrorFact {
        id: uuid::Uuid::new_v4().to_string(),
        plan: plan.clone(),
        phase,
        at: Utc::now(),
        approved_by: caller.describe(),
        remote_id,
        status_id,
        error,
    }
}

impl Engine {
    pub(crate) async fn deployment_mirror_plan(&self, id: &str) -> Result<DeploymentMirrorPlan> {
        let fact = Facts::<L4>::new(self)
            .get::<DeploymentPublicationFact>(&id.to_owned())
            .await?
            .ok_or_else(|| bad("no such deployment"))?;
        plan_from(fact)
    }

    pub(crate) async fn publish_deployment(
        self: &Arc<Self>,
        caller: &Caller,
        id: &str,
        approval: &str,
    ) -> Result<DeploymentMirrorFact> {
        self.publish_deployment_with_gh(caller, id, approval, Path::new("gh"))
            .await
    }

    /// Injectable executable for isolated fixtures, never a configurable API host.
    pub(crate) async fn publish_deployment_with_gh(
        self: &Arc<Self>,
        caller: &Caller,
        id: &str,
        approval: &str,
        gh: &Path,
    ) -> Result<DeploymentMirrorFact> {
        self.authorize(
            caller,
            &factory_core::protocol::Request::DeployPublish {
                id: id.into(),
                approval: approval.into(),
            },
        )
        .await?;
        let _busy = self.deployment_mirror_busy.try_lock().map_err(|_| {
            bad("another deployment mirror is being published; retry after it finishes")
        })?;
        let plan = self.deployment_mirror_plan(id).await?;
        if approval != plan.approval {
            return Err(bad(
                "mirror metadata or destination changed; inspect and approve its new plan",
            ));
        }
        // A retry of a stored successful effect does not issue another write.
        if let Some(prior) = self
            .workflows
            .mirror_receipts(id, 200)
            .await?
            .into_iter()
            .find(|prior| prior.phase == Phase::Published && prior.plan == plan)
        {
            return Ok(prior);
        }
        self.workflows
            .record_mirror(&receipt(&plan, caller, Phase::Approved, None, None, None))
            .await?;
        let mut remote_id = None;
        let outcome = tokio::time::timeout(Duration::from_secs(90), self.write_mirror(caller, &plan, gh, &mut remote_id)).await
            .unwrap_or_else(|_| Err(remote_error("deployment mirror exceeded its total 90-second limit; retry with the current approval")));
        let final_receipt = match outcome {
            Ok(status_id) => receipt(
                &plan,
                caller,
                Phase::Published,
                remote_id,
                Some(status_id),
                None,
            ),
            Err(error) => receipt(
                &plan,
                caller,
                Phase::Failed,
                remote_id,
                None,
                Some(error.to_string()),
            ),
        };
        self.workflows.record_mirror(&final_receipt).await?;
        Ok(final_receipt)
    }

    async fn check_mirror_approval(
        &self,
        caller: &Caller,
        plan: &DeploymentMirrorPlan,
    ) -> Result<()> {
        self.authorize(
            caller,
            &factory_core::protocol::Request::DeployPublish {
                id: plan.deployment.clone(),
                approval: plan.approval.clone(),
            },
        )
        .await?;
        if self.deployment_mirror_plan(&plan.deployment).await? != *plan {
            return Err(bad(
                "mirror approval became stale; no further outbound write was attempted",
            ));
        }
        Ok(())
    }

    async fn write_mirror(
        &self,
        caller: &Caller,
        plan: &DeploymentMirrorPlan,
        gh: &Path,
        remote_id: &mut Option<u64>,
    ) -> Result<u64> {
        let endpoint = format!("repos/{}/deployments", plan.repository);
        let found = find_deployment(gh, &endpoint, plan).await?;
        let id = if let Some(id) = found {
            id
        } else {
            self.check_mirror_approval(caller, plan).await?;
            let created: RemoteDeployment = serde_json::from_value(api(gh, &endpoint, Some(json!({
                "ref": plan.commit, "task": TASK, "environment": plan.environment,
                "auto_merge": false, "required_contexts": [],
                "production_environment": plan.production, "transient_environment": plan.transient,
                "description": format!("Factory recorded deployment {}", plan.deployment),
                "payload": { "factory_deployment": plan.deployment, "scope": plan.scope, "verified": plan.verified }
            }))).await?).map_err(|_| remote_error("invalid GitHub deployment response"))?;
            if !created.matches(plan) {
                return Err(remote_error(
                    "GitHub returned a deployment with conflicting identity",
                ));
            }
            *remote_id = Some(created.id);
            self.workflows
                .record_mirror(&receipt(
                    plan,
                    caller,
                    Phase::Created,
                    *remote_id,
                    None,
                    None,
                ))
                .await?;
            created.id
        };
        *remote_id = Some(id);
        let endpoint = format!("{endpoint}/{id}/statuses");
        let marker = format!("Factory mirror {}", plan.approval);
        if let Some(status) = find_status(gh, &endpoint, &marker, &plan.state).await? {
            return Ok(status);
        }
        self.check_mirror_approval(caller, plan).await?;
        let published: RemoteStatus = serde_json::from_value(
            api(
                gh,
                &endpoint,
                Some(json!({
                    "state": plan.state, "description": marker, "auto_inactive": false
                })),
            )
            .await?,
        )
        .map_err(|_| remote_error("invalid GitHub deployment status response"))?;
        if published.id == 0
            || published.state != plan.state
            || published.description.as_deref() != Some(&marker)
        {
            return Err(remote_error(
                "GitHub returned a conflicting deployment status",
            ));
        }
        Ok(published.id)
    }
}

#[derive(Deserialize)]
struct RemoteDeployment {
    id: u64,
    sha: String,
    environment: String,
    task: String,
    production_environment: bool,
    transient_environment: bool,
    payload: Value,
}
impl RemoteDeployment {
    fn matches(&self, plan: &DeploymentMirrorPlan) -> bool {
        self.id != 0
            && self.sha == plan.commit
            && self.environment == plan.environment
            && self.task == TASK
            && self.production_environment == plan.production
            && self.transient_environment == plan.transient
            && self.payload["factory_deployment"].as_str() == Some(plan.deployment.as_str())
            && self.payload["scope"].as_str() == Some(plan.scope.as_str())
    }
}
#[derive(Deserialize)]
struct RemoteStatus {
    id: u64,
    state: String,
    description: Option<String>,
}

/// Exhaust every bounded page before creating, rather than guessing that an
/// older/crash-created mirror was absent after only the first page.
async fn find_deployment(
    gh: &Path,
    endpoint: &str,
    plan: &DeploymentMirrorPlan,
) -> Result<Option<u64>> {
    let mut found = None;
    for page in 1..=MAX_PAGES {
        let query = format!(
            "{endpoint}?sha={}&task=factory%3Amirror&per_page=100&page={page}",
            plan.commit
        );
        let rows: Vec<RemoteDeployment> = serde_json::from_value(api(gh, &query, None).await?)
            .map_err(|_| remote_error("invalid GitHub deployment listing"))?;
        let complete = rows.len() < 100;
        for row in rows {
            if row.payload["factory_deployment"].as_str() == Some(plan.deployment.as_str()) {
                if found.is_some() || !row.matches(plan) {
                    return Err(remote_error(
                        "ambiguous or conflicting GitHub deployment identity",
                    ));
                }
                found = Some(row.id);
            }
        }
        if complete {
            return Ok(found);
        }
    }
    Err(remote_error(
        "deployment lookup exceeded its bounded history limit; no duplicate was created",
    ))
}

async fn find_status(gh: &Path, endpoint: &str, marker: &str, state: &str) -> Result<Option<u64>> {
    for page in 1..=MAX_PAGES {
        let rows: Vec<RemoteStatus> = serde_json::from_value(
            api(gh, &format!("{endpoint}?per_page=100&page={page}"), None).await?,
        )
        .map_err(|_| remote_error("invalid GitHub deployment status listing"))?;
        let complete = rows.len() < 100;
        for row in rows {
            if row.description.as_deref() == Some(marker) {
                if row.id == 0 || row.state != state {
                    return Err(remote_error("conflicting GitHub status identity"));
                }
                return Ok(Some(row.id));
            }
        }
        if complete {
            return Ok(None);
        }
    }
    Err(remote_error(
        "status lookup exceeded its bounded history limit; no duplicate was created",
    ))
}

/// gh obtains its own GitHub credentials. Never pass them in arguments or
/// persist/copy its stderr: it may contain private provider output.
async fn api(gh: &Path, endpoint: &str, body: Option<Value>) -> Result<Value> {
    let operation = async {
        let mut command = tokio::process::Command::new(gh);
        command
            .kill_on_drop(true)
            .env("GH_HOST", "github.com")
            .env("GH_PROMPT_DISABLED", "1")
            .env_remove("FACTORY_TOKEN")
            .env_remove("FACTORY_TASK_TOKEN")
            .env_remove("FACTORY_RUN_TOKEN")
            .args([
                "api",
                endpoint,
                "--method",
                if body.is_some() { "POST" } else { "GET" },
                "-H",
                "Accept: application/vnd.github+json",
                "-H",
                "X-GitHub-Api-Version: 2026-03-10",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if body.is_some() {
            command.args(["--input", "-"]).stdin(Stdio::piped());
        }
        let mut child = command.spawn().map_err(|_| {
            remote_error("could not start gh; check its installation and authentication")
        })?;
        if let Some(body) = body {
            let bytes = serde_json::to_vec(&body)
                .map_err(|_| remote_error("could not encode publication metadata"))?;
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| remote_error("gh stdin unavailable"))?;
            stdin
                .write_all(&bytes)
                .await
                .map_err(|_| remote_error("could not send publication metadata to gh"))?;
            drop(stdin);
        }
        let mut bytes = Vec::new();
        child
            .stdout
            .take()
            .ok_or_else(|| remote_error("gh stdout unavailable"))?
            .take(MAX_RESPONSE + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| remote_error("could not read GitHub response"))?;
        if bytes.len() as u64 > MAX_RESPONSE {
            return Err(remote_error(
                "GitHub response exceeded the bounded size limit",
            ));
        }
        let status = child
            .wait()
            .await
            .map_err(|_| remote_error("could not wait for gh"))?;
        if !status.success() {
            return Err(remote_error("GitHub rejected the mirror request; check gh authentication and repository deployment permissions"));
        }
        serde_json::from_slice(&bytes).map_err(|_| remote_error("GitHub returned invalid JSON"))
    };
    tokio::time::timeout(Duration::from_secs(30), operation)
        .await
        .map_err(|_| remote_error("GitHub mirror request timed out"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_core::environments::{DeployFinish, DeployStart, DeployStatus, ReleaseFacts};
    use factory_core::role::{Grant, GrantExpansion, Reach, Role, RoleSpec};
    use std::os::unix::fs::PermissionsExt;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
    const SECRET: &str = "private-provider-error-not-for-journal";

    fn engine(opted_in: bool) -> (Arc<Engine>, std::path::PathBuf) {
        let mirror = if opted_in {
            "    github_deployments: { repository: test-owner/test-repo }\n"
        } else {
            ""
        };
        let (engine, root) = crate::environments::tests::engine_with(&format!("  - name: prod\n    tier: production\n    checks: [{{kind: command, command: true}}]\n{mirror}"));
        let workflows =
            crate::workflows::WorkflowStore::open(&root.join(".factory/factory.sqlite")).unwrap();
        let engine = Arc::new(
            Engine::new(
                engine.factory_snapshot(),
                factory_plugins::Registry::with_builtins(),
                engine.store.clone(),
                "factory".into(),
                Vec::new(),
            )
            .with_environment_store(engine.environments.clone())
            .with_workflow_store(workflows),
        );
        (engine, root)
    }
    async fn start(engine: &Arc<Engine>, commit: &str, dirty: bool) -> String {
        engine
            .deploy_start(
                &Caller::Owner,
                DeployStart {
                    environment: "prod".into(),
                    scope: None,
                    release: ReleaseFacts {
                        commit: commit.into(),
                        dirty,
                        source: Some(SECRET.into()),
                        ..Default::default()
                    },
                    via: Some(SECRET.into()),
                    started_at: None,
                    strict_verification: false,
                },
            )
            .await
            .unwrap()
            .id
    }
    fn fake_gh(root: &Path, plan: &DeploymentMirrorPlan, status_id: u64) -> std::path::PathBuf {
        let directory = root.join("gh-fixture");
        std::fs::create_dir_all(&directory).unwrap();
        let deployment = json!({"id": 71, "sha": plan.commit, "environment": plan.environment, "task": TASK,
            "production_environment": plan.production, "transient_environment": plan.transient,
            "payload": {"factory_deployment": plan.deployment, "scope": plan.scope}});
        let status = json!({"id": status_id, "state": plan.state, "description": format!("Factory mirror {}", plan.approval)});
        std::fs::write(
            directory.join("new-deployment.json"),
            deployment.to_string(),
        )
        .unwrap();
        std::fs::write(directory.join("new-status.json"), status.to_string()).unwrap();
        let script = directory.join("gh");
        std::fs::write(&script, format!(r#"#!/bin/sh
set -eu
data='{}'
printf '%s\n' "$*" >> "$data/calls"
case "$2" in
  *'/statuses?'*)
    if test -f "$data/status.json"; then printf '['; cat "$data/status.json"; printf ']'; else printf '[]'; fi ;;
  *'/statuses')
    cat > "$data/status-input.json"
    if test -f "$data/fail-status"; then printf '{}' >&2; exit 1; fi
    cp "$data/new-status.json" "$data/status.json"; cat "$data/status.json" ;;
  *'/deployments?'*)
    if test -f "$data/page-1.json" && echo "$2" | grep -q 'page=1$'; then cat "$data/page-1.json"
    elif test -f "$data/deployment.json"; then printf '['; cat "$data/deployment.json"; printf ']'; else printf '[]'; fi ;;
  *'/deployments')
    cat > "$data/deployment-input.json"
    cp "$data/new-deployment.json" "$data/deployment.json"; cat "$data/deployment.json" ;;
  *) exit 3 ;;
esac
"#, directory.display(), SECRET)).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    #[tokio::test]
    async fn approved_mirror_is_private_by_default_crash_retryable_and_restart_persistent() {
        let (engine, root) = engine(true);
        let id = start(&engine, SHA, false).await;
        let plan = engine.deployment_mirror_plan(&id).await.unwrap();
        let gh = fake_gh(&root, &plan, 81);
        let fixture = gh.parent().unwrap();
        assert!(
            !fixture.join("calls").exists(),
            "recording and reading never publish"
        );
        assert!(engine
            .publish_deployment_with_gh(&Caller::Owner, &id, "not-approved", &gh)
            .await
            .is_err());
        assert!(!fixture.join("calls").exists());
        let agent = Caller::Agent {
            scope: "company".into(),
            name: "coordinator".into(),
            role: Role::foreman(),
            run_id: None,
        };
        assert!(matches!(
            engine
                .publish_deployment_with_gh(&agent, &id, &plan.approval, &gh)
                .await,
            Err(FactoryError::Denied(_))
        ));
        assert!(!fixture.join("calls").exists());
        std::fs::write(fixture.join("fail-status"), "1").unwrap();
        let failed = engine
            .publish_deployment_with_gh(&Caller::Owner, &id, &plan.approval, &gh)
            .await
            .unwrap();
        assert_eq!(failed.phase, Phase::Failed);
        assert_eq!(failed.remote_id, Some(71));
        assert!(!serde_json::to_string(&failed).unwrap().contains(SECRET));
        std::fs::remove_file(fixture.join("fail-status")).unwrap();
        let published = engine
            .publish_deployment_with_gh(&Caller::Owner, &id, &plan.approval, &gh)
            .await
            .unwrap();
        assert_eq!(published.phase, Phase::Published);
        assert_eq!(published.status_id, Some(81));
        let calls = std::fs::read_to_string(fixture.join("calls")).unwrap();
        assert_eq!(
            calls
                .lines()
                .filter(|line| line.contains("/deployments --method POST"))
                .count(),
            1,
            "retry reuses crash-created remote deployment"
        );
        let duplicate = engine
            .publish_deployment_with_gh(&Caller::Owner, &id, &plan.approval, &gh)
            .await
            .unwrap();
        assert_eq!(duplicate, published);
        assert_eq!(
            std::fs::read_to_string(fixture.join("calls")).unwrap(),
            calls
        );
        let posted: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("deployment-input.json")).unwrap())
                .unwrap();
        assert_eq!(posted["ref"], SHA);
        assert_eq!(posted["task"], TASK);
        assert_eq!(posted["auto_merge"], false);
        assert_eq!(posted["required_contexts"], json!([]));
        assert!(!posted.to_string().contains(SECRET));
        let status: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("status-input.json")).unwrap())
                .unwrap();
        assert_eq!(status["auto_inactive"], false);
        let reopened =
            crate::workflows::WorkflowStore::open(&root.join(".factory/factory.sqlite")).unwrap();
        assert_eq!(
            reopened.mirror_receipts(&id, 200).await.unwrap()[0],
            published
        );
        assert!(engine
            .store
            .list(&factory_core::TaskFilter::default())
            .await
            .unwrap()
            .is_empty());
        assert_eq!(engine.environments.deployments().await.unwrap().len(), 1);
        let report = engine.environments_report(None).await.unwrap();
        assert_eq!(report.deployment_mirrors[&id].receipt, Some(published));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_new_recorded_status_requires_fresh_approval_without_recreating_the_deployment() {
        let (engine, root) = engine(true);
        let id = start(&engine, SHA, false).await;
        let before = engine.deployment_mirror_plan(&id).await.unwrap();
        let gh = fake_gh(&root, &before, 81);
        engine
            .publish_deployment_with_gh(&Caller::Owner, &id, &before.approval, &gh)
            .await
            .unwrap();
        engine
            .deploy_finish(DeployFinish {
                id: id.clone(),
                status: DeployStatus::Failed,
                reason: Some(SECRET.into()),
                verify: false,
            })
            .await
            .unwrap();
        let after = engine.deployment_mirror_plan(&id).await.unwrap();
        assert_ne!(before.approval, after.approval);
        assert_eq!(after.state, "failure");
        assert!(engine
            .publish_deployment_with_gh(&Caller::Owner, &id, &before.approval, &gh)
            .await
            .is_err());
        let gh = fake_gh(&root, &after, 82);
        let result = engine
            .publish_deployment_with_gh(&Caller::Owner, &id, &after.approval, &gh)
            .await
            .unwrap();
        assert_eq!(result.phase, Phase::Published);
        assert_eq!(result.remote_id, Some(71));
        assert_eq!(result.status_id, Some(82));
        let calls = std::fs::read_to_string(gh.parent().unwrap().join("calls")).unwrap();
        assert_eq!(
            calls
                .lines()
                .filter(|line| line.contains("/deployments --method POST"))
                .count(),
            1
        );
        assert!(!serde_json::to_string(&result).unwrap().contains(SECRET));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn opt_in_clean_immutable_identity_and_explicit_grants_are_required() {
        let (engine, root) = engine(false);
        let id = start(&engine, SHA, false).await;
        assert!(engine.deployment_mirror_plan(&id).await.is_err());
        assert!(engine
            .environments_report(None)
            .await
            .unwrap()
            .deployment_mirrors
            .is_empty());
        std::fs::remove_dir_all(root).unwrap();
        let (engine, root) = self::engine(true);
        let id = start(&engine, "moving-branch", false).await;
        assert!(engine.deployment_mirror_plan(&id).await.is_err());
        let uppercase = start(&engine, &SHA.to_uppercase(), false).await;
        assert_eq!(
            engine
                .deployment_mirror_plan(&uppercase)
                .await
                .unwrap()
                .commit,
            SHA
        );
        let mut deployment = engine.environments.deployment(&id).await.unwrap().unwrap();
        deployment.id = uuid::Uuid::new_v4().to_string();
        deployment.release.commit = SHA.into();
        deployment.release.dirty = true;
        engine.environments.started(&deployment).await.unwrap();
        assert!(engine.deployment_mirror_plan(&deployment.id).await.is_err());
        assert!(!Grant::expand("*").unwrap().contains(&Grant::DeployPublish));
        assert!(!Grant::expand("deploy.*")
            .unwrap()
            .contains(&Grant::DeployPublish));
        assert_eq!(
            Grant::expand("deploy.publish").unwrap(),
            vec![Grant::DeployPublish]
        );
        assert!(factory_core::environments::valid_github_repository(
            "owner/repo"
        ));
        for bad in [
            "https://github.com/owner/repo",
            "owner/repo/extra",
            "owner/../repo",
            "owner/repo?token=secret",
        ] {
            assert!(!factory_core::environments::valid_github_repository(bad));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn remote_marker_identity_and_paginated_crash_recovery_are_not_guessed() {
        let (engine, root) = engine(true);
        let id = start(&engine, SHA, false).await;
        let plan = engine.deployment_mirror_plan(&id).await.unwrap();
        let gh = fake_gh(&root, &plan, 81);
        let fixture = gh.parent().unwrap();
        std::fs::copy(
            fixture.join("new-deployment.json"),
            fixture.join("deployment.json"),
        )
        .unwrap();
        let row = json!({"id": 1, "sha": SHA, "environment": "other", "task": TASK,
            "production_environment": true, "transient_environment": false, "payload": {}});
        std::fs::write(
            fixture.join("page-1.json"),
            serde_json::to_vec(&vec![row; 100]).unwrap(),
        )
        .unwrap();
        let published = engine
            .publish_deployment_with_gh(&Caller::Owner, &id, &plan.approval, &gh)
            .await
            .unwrap();
        assert_eq!(published.phase, Phase::Published);
        let calls = std::fs::read_to_string(fixture.join("calls")).unwrap();
        assert!(calls.contains("page=2"));
        assert!(!fixture.join("deployment-input.json").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn explicit_publish_grant_is_scope_bound_and_not_a_foreman_side_effect() {
        let (engine, root) = engine(true);
        let id = start(&engine, SHA, false).await;
        let mut snapshot = engine.factory_snapshot();
        snapshot.config.scope = None;
        snapshot.config.scopes[0].roles.insert(
            "publisher".into(),
            RoleSpec {
                describe: Some("outbound approver".into()),
                grants: vec!["deploy.publish".into()],
                reach: Reach::Scope,
            },
        );
        snapshot.config.scopes[0].roles.insert(
            "own-publisher".into(),
            RoleSpec {
                describe: None,
                grants: vec!["deploy.publish".into()],
                reach: Reach::Own,
            },
        );
        let engine = Arc::new(
            Engine::new(
                snapshot,
                factory_plugins::Registry::with_builtins(),
                engine.store.clone(),
                "factory".into(),
                Vec::new(),
            )
            .with_environment_store(engine.environments.clone()),
        );
        let request = factory_core::protocol::Request::DeployPublish {
            id,
            approval: "digest".into(),
        };
        let caller = |scope: &str, role: Role| Caller::Agent {
            scope: scope.into(),
            name: "publisher".into(),
            role,
            run_id: None,
        };
        assert!(engine
            .authorize(&caller("company", Role::new("publisher")), &request)
            .await
            .is_ok());
        assert!(engine
            .authorize(&caller("company", Role::foreman()), &request)
            .await
            .is_err());
        assert!(engine
            .authorize(&caller("unknown", Role::new("publisher")), &request)
            .await
            .is_err());
        assert!(engine
            .authorize(&caller("company", Role::new("own-publisher")), &request)
            .await
            .is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn conflicting_remote_identity_never_creates_or_updates_a_deployment() {
        let (engine, root) = engine(true);
        let id = start(&engine, SHA, false).await;
        let plan = engine.deployment_mirror_plan(&id).await.unwrap();
        let gh = fake_gh(&root, &plan, 81);
        let fixture = gh.parent().unwrap();
        let mut remote: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("new-deployment.json")).unwrap())
                .unwrap();
        remote["payload"]["scope"] = json!("another-scope");
        std::fs::write(fixture.join("deployment.json"), remote.to_string()).unwrap();
        let failed = engine
            .publish_deployment_with_gh(&Caller::Owner, &id, &plan.approval, &gh)
            .await
            .unwrap();
        assert_eq!(failed.phase, Phase::Failed);
        assert!(failed.error.unwrap().contains("conflicting"));
        assert!(!fixture.join("deployment-input.json").exists());
        assert!(!fixture.join("status-input.json").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn crash_after_remote_status_is_recovered_without_another_post() {
        let (engine, root) = engine(true);
        let id = start(&engine, SHA, false).await;
        let plan = engine.deployment_mirror_plan(&id).await.unwrap();
        let gh = fake_gh(&root, &plan, 81);
        let fixture = gh.parent().unwrap();
        std::fs::copy(
            fixture.join("new-deployment.json"),
            fixture.join("deployment.json"),
        )
        .unwrap();
        std::fs::copy(fixture.join("new-status.json"), fixture.join("status.json")).unwrap();
        let published = engine
            .publish_deployment_with_gh(&Caller::Owner, &id, &plan.approval, &gh)
            .await
            .unwrap();
        assert_eq!(published.phase, Phase::Published);
        assert_eq!(published.remote_id, Some(71));
        assert_eq!(published.status_id, Some(81));
        assert!(!std::fs::read_to_string(fixture.join("calls"))
            .unwrap()
            .contains("POST"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn changed_repository_or_revoked_opt_in_invalidates_approval_before_any_api_call() {
        let (engine, root) = engine(true);
        let id = start(&engine, SHA, false).await;
        let plan = engine.deployment_mirror_plan(&id).await.unwrap();
        let gh = fake_gh(&root, &plan, 81);
        for repository in [Some("another-owner/another-repo"), None] {
            let mut snapshot = engine.factory_snapshot();
            snapshot.config.scope = None;
            let env = &mut snapshot.config.scopes[0].environments[0];
            env.github_deployments =
                repository.map(
                    |repository| factory_core::environments::GitHubDeploymentMirror {
                        repository: repository.into(),
                    },
                );
            let changed = Arc::new(
                Engine::new(
                    snapshot,
                    factory_plugins::Registry::with_builtins(),
                    engine.store.clone(),
                    "factory".into(),
                    Vec::new(),
                )
                .with_environment_store(engine.environments.clone()),
            );
            assert!(changed
                .publish_deployment_with_gh(&Caller::Owner, &id, &plan.approval, &gh)
                .await
                .is_err());
            assert!(!gh.parent().unwrap().join("calls").exists());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn concurrent_publication_is_refused_before_any_api_call() {
        let (engine, root) = engine(true);
        let id = start(&engine, SHA, false).await;
        let plan = engine.deployment_mirror_plan(&id).await.unwrap();
        let gh = fake_gh(&root, &plan, 81);
        let guard = engine.deployment_mirror_busy.lock().await;
        assert!(engine
            .publish_deployment_with_gh(&Caller::Owner, &id, &plan.approval, &gh)
            .await
            .is_err());
        assert!(!gh.parent().unwrap().join("calls").exists());
        drop(guard);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn oversized_or_invalid_responses_are_bounded_and_sanitized() {
        let (engine, root) = engine(true);
        let id = start(&engine, SHA, false).await;
        let plan = engine.deployment_mirror_plan(&id).await.unwrap();
        let gh = fake_gh(&root, &plan, 81);
        let fixture = gh.parent().unwrap();
        for bytes in [
            vec![b'x'; MAX_RESPONSE as usize + 1],
            SECRET.as_bytes().to_vec(),
        ] {
            std::fs::write(fixture.join("page-1.json"), bytes).unwrap();
            let failed = engine
                .publish_deployment_with_gh(&Caller::Owner, &id, &plan.approval, &gh)
                .await
                .unwrap();
            assert_eq!(failed.phase, Phase::Failed);
            assert!(!serde_json::to_string(&failed).unwrap().contains(SECRET));
            assert!(!fixture.join("deployment-input.json").exists());
            assert!(!fixture.join("status-input.json").exists());
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
