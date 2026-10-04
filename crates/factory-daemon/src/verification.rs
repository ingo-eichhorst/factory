//! `#118` v1: the `done` gate. A run whose control plan requires steps does
//! not become `done` because its agent said so. It becomes `verifying`, the
//! daemon runs every required gate itself -- in the run's own worktree, with
//! the bench gate runner, exit 0 = pass -- appends one attestation per step,
//! and only then settles the run: `done` when every required attestation
//! exists and passed, `blocked` with the reason (and so in the Inbox) when
//! one is missing or failed.
//!
//! One code path for both kinds of work. What a run is held to is
//! `Run::required_steps`, fixed at dispatch from gate nodes: the injected
//! snapshot of the workflow run a node belongs to, or the same injection
//! over the implicit one-node workflow a standalone task is planned as
//! (`WorkflowDefinition::implicit`). This module runs them; the workflow's
//! gate nodes only mirror what it found.
//!
//! The session stays open while a run is verified and while it sits blocked
//! on a failed gate, so an answer from the Inbox reaches the agent that did
//! the work, and its next `done` verifies again. Nothing here trusts an
//! attestation the executing agent produced (`control_plan::judge`).

use crate::engine::Engine;
use chrono::Utc;
#[cfg(test)]
use factory_core::conformance::AttestedRun;
use factory_core::control_plan::{
    self, AttestationVerdict, ControlPlan, RequiredStep, StepAttestation, StepKind, GATE_ACTOR,
};
use factory_core::error::{FactoryError, Result};
use factory_core::event::Event;
#[cfg(test)]
use factory_core::operations::Window;
use factory_core::policy;
use factory_core::quality;
use factory_core::run::{BlockSource, Run, RunPatch, RunStatus};
use factory_core::task::{NewTask, Task, TaskEntry, TaskFilter, WorkflowOrigin};
use factory_core::workflow::{
    WorkflowDefinition, WorkflowLint, WorkflowNodeKind, WorkflowNodeStatus, IMPLICIT_NODE,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
#[cfg(test)]
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

/// Ten minutes -- a gate's timeout when its requirement names none, the same
/// fallback the bench gate runner has always used.
pub(crate) const DEFAULT_GATE_TIMEOUT_SECS: u64 = 600;
pub(crate) const REVIEW_RUN_LABEL: &str = "factory.review_run";
pub(crate) const REVIEW_STEP_LABEL: &str = "factory.review_step";
pub(crate) const REVIEW_DIGEST_LABEL: &str = "factory.review_digest";

/// Run `sh -c command` in `dir`, combined stdout+stderr, bounded by
/// `timeout_secs`. `(None, ...)` on a timeout or a failure to even start the
/// process -- both read as "could not confirm this passed", never as a pass.
/// The bench gate runner, shared: a bench case's gate and a required step
/// are judged by exactly the same code.
pub(crate) async fn run_shell_capture(dir: &Path, command: &str, timeout_secs: u64) -> (Option<i32>, String) {
    let attempt = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), attempt).await {
        Ok(Ok(output)) => {
            let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
            combined.push_str(&String::from_utf8_lossy(&output.stderr));
            (output.status.code(), combined)
        }
        Ok(Err(e)) => (None, format!("could not run the command: {e}")),
        Err(_) => (None, format!("did not finish within {timeout_secs}s")),
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GitState {
    pub(crate) commit: Option<String>,
    pub(crate) dirty: Option<bool>,
    pub(crate) digest: Option<String>,
}

fn directory_digest(root: &Path) -> std::io::Result<String> {
    fn visit(root: &Path, dir: &Path, hasher: &mut Sha256) -> std::io::Result<()> {
        let mut entries = std::fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap_or(&path);
            let metadata = std::fs::symlink_metadata(&path)?;
            hasher.update(relative.to_string_lossy().as_bytes());
            hasher.update(b"\0");
            if metadata.file_type().is_symlink() {
                hasher.update(b"link\0");
                hasher.update(std::fs::read_link(&path)?.to_string_lossy().as_bytes());
            } else if metadata.is_dir() {
                hasher.update(b"dir\0");
                visit(root, &path, hasher)?;
            } else {
                hasher.update(b"file\0");
                hasher.update(std::fs::read(&path)?);
            }
            hasher.update(b"\0");
        }
        Ok(())
    }
    let mut hasher = Sha256::new();
    hasher.update(b"factory-directory-v1\0");
    visit(root, root, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// HEAD plus the exact tracked diff and untracked bytes an attestation judges.
pub(crate) async fn git_state(dir: &Path) -> GitState {
    let head = tokio::process::Command::new("git").arg("-C").arg(dir).args(["rev-parse", "HEAD"]).output().await;
    let commit = match head {
        Ok(out) if out.status.success() => Some(String::from_utf8_lossy(&out.stdout).trim().to_string()),
        _ => return GitState {
            digest: directory_digest(dir).ok(),
            ..Default::default()
        },
    };
    let status = tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .output()
        .await;
    let status = match status {
        Ok(out) if out.status.success() => out.stdout,
        _ => return GitState { commit, ..Default::default() },
    };
    let diff = tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["diff", "--binary", "--no-ext-diff", "HEAD", "--"])
        .output()
        .await;
    let untracked = tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["ls-files", "--others", "--exclude-standard", "-z"])
        .output()
        .await;
    let (diff, untracked) = match (diff, untracked) {
        (Ok(diff), Ok(untracked)) if diff.status.success() && untracked.status.success() => {
            (diff.stdout, untracked.stdout)
        }
        _ => {
            return GitState {
                commit,
                dirty: Some(!status.is_empty()),
                digest: None,
            };
        }
    };
    let mut hasher = Sha256::new();
    hasher.update(b"factory-worktree-v1\0");
    hasher.update(commit.as_deref().unwrap_or_default().as_bytes());
    hasher.update(b"\0status\0");
    hasher.update(&status);
    hasher.update(b"\0diff\0");
    hasher.update(&diff);
    hasher.update(b"\0untracked\0");
    for raw in untracked.split(|byte| *byte == 0).filter(|path| !path.is_empty()) {
        hasher.update(raw);
        hasher.update(b"\0");
        let relative = String::from_utf8_lossy(raw);
        match tokio::fs::read(dir.join(relative.as_ref())).await {
            Ok(bytes) => hasher.update(bytes),
            Err(error) => hasher.update(format!("<unreadable:{error}>").as_bytes()),
        }
        hasher.update(b"\0");
    }
    GitState {
        commit,
        dirty: Some(!status.is_empty()),
        digest: Some(format!("{:x}", hasher.finalize())),
    }
}

impl Engine {
    fn decision_actor(caller: &crate::access::Caller) -> String {
        match caller {
            crate::access::Caller::Owner => "owner".into(),
            crate::access::Caller::Agent { name, .. } => name.clone(),
        }
    }

    /// Append a person/functionary approval decision. Rejections are durable
    /// evidence too and deliberately keep the line stopped.
    pub(crate) async fn decide_approval(
        self: &Arc<Self>,
        caller: &crate::access::Caller,
        run_id: &str,
        verdict: AttestationVerdict,
        reason: &str,
    ) -> Result<Run> {
        if reason.trim().is_empty() {
            return Err(FactoryError::BadRequest(
                "an approval decision needs a reason".into(),
            ));
        }
        let run = self.require_run(run_id).await?;
        let task = self.require(&run.task_id).await?;
        let actor = Self::decision_actor(caller);
        if actor == run.agent {
            return Err(FactoryError::Denied(
                "the executing agent cannot approve or reject its own work".into(),
            ));
        }
        let evidence = self.policies.step_attestations(run_id).await?;
        let step = run
            .required_steps
            .iter()
            .filter(|s| s.kind == StepKind::Approval)
            .find(|step| {
                !evidence
                    .iter()
                    .rev()
                    .find(|a| a.kind == StepKind::Approval && a.step == step.step)
                    .is_some_and(|a| a.verdict == AttestationVerdict::Pass)
            })
            .ok_or_else(|| FactoryError::BadRequest(format!("run {run_id} needs no pending approval")))?;
        if let Some(expected) = step.actor.as_deref().filter(|expected| *expected != actor) {
            return Err(FactoryError::Denied(format!(
                "this approval is bound to {expected}, not {actor}"
            )));
        }
        let dir = run.worktree_path.clone().unwrap_or_else(|| {
            self.factory_snapshot()
                .scope_path(&task.scope)
                .unwrap_or_default()
                .display()
                .to_string()
        });
        let state = git_state(Path::new(&dir)).await;
        let attestation = StepAttestation {
            id: uuid::Uuid::new_v4().to_string(),
            run_id: run.id.clone(),
            task_id: task.id.clone(),
            scope: task.scope.clone(),
            category: control_plan::effective_category(task.category.as_deref()).into(),
            step: step.step.clone(),
            kind: StepKind::Approval,
            actor: actor.clone(),
            verdict,
            findings: Some(reason.trim().to_string()),
            round: 0,
            required_by: step.required_by.clone(),
            command: None,
            exit_code: None,
            output: None,
            dir,
            commit: state.commit,
            dirty: state.dirty,
            worktree_digest: state.digest,
            node_id: step.node_id.clone(),
            at: Utc::now(),
        };
        self.policies.append_step_attestation(&attestation).await?;
        self.entry(
            &task.id,
            TaskEntry::new(
                actor,
                if verdict == AttestationVerdict::Pass {
                    "approved"
                } else {
                    "rejected"
                },
                reason.trim(),
            )
            .in_run(&run.id)
            .with_data(serde_json::json!({ "attestation": attestation.id })),
        )
        .await;
        if verdict == AttestationVerdict::Pass
            && run.session.is_none()
            && run.status == RunStatus::Blocked
        {
            let resumed = match self
                .dispatch(
                    &task.id,
                    run.trigger,
                    crate::engine::Due {
                        queued_at: run.queued_at.unwrap_or(run.started_at),
                        scheduled_for: run.scheduled_for,
                    },
                    None,
                )
                .await
            {
                Ok(run) => run,
                Err(FactoryError::DispatchSuperseded(_)) => {
                    return self.require_run(&run.id).await;
                }
                Err(FactoryError::CapacityHeld { agent, in_use, max }) => {
                    // Approval is durable; a busy slot is a queue, not a
                    // failed attempt. The normal release/sweep worker resumes
                    // this same frozen run when capacity becomes available.
                    self.store.update(&task.id, &factory_core::task::TaskPatch {
                        status: Some(factory_core::task::TaskStatus::Pending),
                        slot_wait: Some(factory_core::task::SlotWait {
                            agent: agent.clone(),
                            scope: task.scope.clone(),
                            trigger: run.trigger,
                            queued_at: run.queued_at.unwrap_or(run.started_at),
                            scheduled_for: run.scheduled_for,
                            since: Utc::now(),
                        }),
                        ..Default::default()
                    }).await?;
                    self.entry(&task.id, TaskEntry::new(
                        "daemon", "capacity_held",
                        format!("approved; waiting for a {agent} slot ({in_use}/{max} in use)"),
                    ).in_run(&run.id)).await;
                    self.publish_task(&task.id).await;
                    self.sync_workflow_for_task(&task.id).await;
                    return Ok(run);
                }
                Err(error) => {
                    // This is the same dispatch path as `start_run_due`, but
                    // a caller is waiting for its answer. Once approval has
                    // been recorded, any failure must settle the held run so
                    // it cannot remain dispatching (or blocked with a pass
                    // that makes the approval action unusable). A retry then
                    // starts a fresh, approvable attempt.
                    self.fail_run(
                        &run.id,
                        factory_core::run::FailKind::DispatchFailed,
                        &format!("dispatch failed after approval: {error}"),
                    )
                    .await;
                    return Err(error);
                }
            };
            self.sync_workflow_for_task(&task.id).await;
            return Ok(resumed);
        }
        if verdict == AttestationVerdict::Pass
            && (run.status == RunStatus::Verifying
                || (run.status == RunStatus::Blocked
                    && run.blocked_source == Some(BlockSource::Verification)))
        {
            let updated = self
                .store
                .update_run(
                    &run.id,
                    &RunPatch {
                        status: Some(RunStatus::Verifying),
                        clear_blocked: true,
                        ..Default::default()
                    },
                )
                .await?;
            self.bus.publish(Event::RunUpdated {
                run: updated.clone(),
            });
            self.mirror_to_task(&updated).await;
            self.enqueue_verification(&run.id);
            return Ok(updated);
        }
        self.sync_workflow_for_task(&task.id).await;
        Ok(run)
    }

    /// Accept a verifier-created rework proposal. Workflow work uses its
    /// injected review node's bounded send-back; standalone work ends the
    /// rejected attempt and starts a same-task retry.
    pub(crate) async fn accept_rework(
        self: &Arc<Self>,
        caller: &crate::access::Caller,
        run_id: &str,
    ) -> Result<Run> {
        let run = self.require_run(run_id).await?;
        if run.status != RunStatus::Blocked || run.blocked_source != Some(BlockSource::Verification)
        {
            return Err(FactoryError::BadRequest(
                "only a run blocked by verification has rework to accept".into(),
            ));
        }
        let task = self.require(&run.task_id).await?;
        let workflow_guard = if task.workflow_origin.is_some() {
            Some(self.workflow_edit.lock().await)
        } else { None };
        let actor = Self::decision_actor(caller);
        if actor == run.agent {
            return Err(FactoryError::Denied(
                "the executing agent cannot accept rework for its own work".into(),
            ));
        }
        if task.workflow_origin.is_none() {
            let used = self
                .store
                .runs(&task.id, 100)
                .await?
                .into_iter()
                .filter(|attempt| {
                    attempt
                        .error
                        .as_deref()
                        .is_some_and(|e| e.starts_with("rework requested:"))
                })
                .count();
            if used >= 5 {
                return Err(FactoryError::BadRequest(
                    "five rework rounds are exhausted; a person must resolve the run".into(),
                ));
            }
        }
        let attestations = self.policies.step_attestations(run_id).await?;
        let failed = attestations
            .iter()
            .rev()
            .find(|a| a.verdict == AttestationVerdict::Fail);
        let finding = failed
            .and_then(|a| {
                a.findings
                    .as_deref()
                    .filter(|text| !text.trim().is_empty())
                    .map(str::to_string)
                    .or_else(|| {
                        a.output
                            .as_deref()
                            .filter(|text| !text.trim().is_empty())
                            .map(str::to_string)
                    })
                    .or_else(|| a.exit_code.map(|code| format!("{} exit {code}", a.step)))
            })
            .unwrap_or_else(|| "verification did not pass".into());
        // Validate and apply the bounded workflow transition to an in-memory
        // snapshot before consuming the blocked subject. Exhausted and
        // missing exits remain blocked and actionable for a person.
        let workflow_rework = if let Some(origin) = task.workflow_origin.as_ref() {
            let mut workflow = self.workflow_run(&origin.workflow_run_id).await?;
            let from = failed
                .and_then(|a| a.node_id.clone())
                .or_else(|| {
                    workflow
                        .definition
                        .nodes
                        .iter()
                        .find(|n| {
                            n.kind == factory_core::workflow::WorkflowNodeKind::Review
                                && n.gate.as_ref().and_then(|g| g.subject.as_deref())
                                    == Some(origin.node_id.as_str())
                        })
                        .map(|n| n.id.clone())
                })
                .ok_or_else(|| {
                    FactoryError::BadRequest(
                        "the workflow snapshot has no failed control node to send back from".into(),
                    )
                })?;
            // A gate has no task result of its own for `upstream_outputs` to
            // carry. Freeze its concrete finding on this feedback round,
            // without changing the standing task or immutable definition.
            match workflow.send_back(&from, &origin.node_id) {
                factory_core::workflow::SendBack::Sent { .. } => {
                    if let Some(request) = workflow.nodes.iter_mut().find(|node| node.node_id == origin.node_id)
                        .and_then(|node| node.rework_request.as_mut()) {
                        request.feedback = Some(finding.clone());
                    }
                    Some(workflow)
                }
                factory_core::workflow::SendBack::Exhausted { max_rounds } => {
                    return Err(FactoryError::BadRequest(format!(
                        "verification rework exhausted its {max_rounds} rounds; a person must resolve it"
                    )));
                }
                factory_core::workflow::SendBack::NoExit => {
                    return Err(FactoryError::BadRequest(
                        "the failed control has no bounded rework path".into(),
                    ));
                }
            }
        } else {
            None
        };

        self.entry(
            &task.id,
            TaskEntry::new(
                actor,
                "rework_accepted",
                format!("accepted rework: {finding}"),
            )
            .in_run(&run.id),
        )
        .await;
        self.close_session(&run).await;
        let ended = self
            .finish_run(
                &run.id,
                RunStatus::Failed,
                RunPatch {
                    status: Some(RunStatus::Failed),
                    error: Some(format!("rework requested: {finding}")),
                    ..Default::default()
                },
                "verification rework accepted",
            )
            .await?;

        if let Some(workflow) = workflow_rework {
            self.workflows.put_run(&workflow).await?;
            self.bus.publish(Event::WorkflowRunUpdated {
                run: workflow.clone(),
            });
            drop(workflow_guard);
            self.advance_workflow(&workflow.id).await?;
        } else {
            let instructions = format!(
                "{}\n\nRework requested from run {}:\n{}",
                task.instructions.trim_end(),
                run.id,
                finding
            );
            let _ = self
                .store
                .update(
                    &task.id,
                    &factory_core::task::TaskPatch {
                        instructions: Some(instructions),
                        ..Default::default()
                    },
                )
                .await?;
            let engine = self.clone();
            let task_id = task.id.clone();
            tokio::spawn(async move {
                engine
                    .start_run(&task_id, factory_core::run::Trigger::Manual)
                    .await;
            });
        }
        Ok(ended)
    }

    /// A scope's control plan for one category, from what applies there
    /// right now: the policy chain folded by `policy::applicable`, and the
    /// quality chain folded by `quality::applicable`. Read fresh off disk
    /// every time, like every other L6 read.
    pub(crate) async fn control_plan(&self, scope: &str, category: &str) -> Result<ControlPlan> {
        control_plan::check_category(category).map_err(FactoryError::BadRequest)?;
        let snapshot = self.factory_snapshot();
        let scope = snapshot.scope(scope)?.name.clone();
        let policies_dir = snapshot.policies_dir();
        let (catalogues, _) = tokio::task::spawn_blocking(move || policy::load_all(&policies_dir))
            .await
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("policy catalogue walk: {e}")))?;
        let (applied, _) = policy::applicable(&catalogues, &self.policy_chain(&scope));
        let inputs = self.quality_inputs(Some(&scope), true).await?;
        let quality: Vec<_> = inputs.trees.iter().flat_map(|(_, tree)| quality::requirements_of(tree)).collect();
        Ok(control_plan::resolve(&scope, category, &applied, &quality))
    }

    /// One plan per category `definition`'s task nodes are planned as.
    pub(crate) async fn control_plans(&self, definition: &WorkflowDefinition) -> Result<BTreeMap<String, ControlPlan>> {
        let mut plans = BTreeMap::new();
        for category in definition.categories() {
            let plan = self.control_plan(&definition.scope, &category).await?;
            plans.insert(category, plan);
        }
        Ok(plans)
    }

    /// What a run of `task` will be held to, decided at dispatch. A workflow
    /// node's gates are already in its run's snapshot; a standalone task is
    /// planned as its implicit one-node workflow, through the same
    /// injection. A bench attempt is held to nothing here -- its own case
    /// gate judges it, and a plan on top would judge it twice.
    pub(crate) async fn required_steps_for_task(
        &self,
        task: &Task,
        executor: &str,
    ) -> Result<Vec<RequiredStep>> {
        if task.bench_origin.is_some() {
            return Ok(Vec::new());
        }
        let review_labels = [REVIEW_RUN_LABEL, REVIEW_STEP_LABEL, REVIEW_DIGEST_LABEL]
            .iter()
            .filter(|label| task.labels.contains_key(**label))
            .count();
        if review_labels != 0 {
            if review_labels != 3 {
                return Err(FactoryError::BadRequest(
                    "an internal review task needs its complete daemon-owned provenance".into(),
                ));
            }
            let subject_id = task.labels.get(REVIEW_RUN_LABEL).expect("counted above");
            let step_name = task.labels.get(REVIEW_STEP_LABEL).expect("counted above");
            if task
                .labels
                .get(REVIEW_DIGEST_LABEL)
                .is_none_or(|digest| digest.trim().is_empty())
            {
                return Err(FactoryError::BadRequest(
                    "an internal review task needs a worktree digest".into(),
                ));
            }
            let subject = self.require_run(subject_id).await?;
            if !matches!(subject.status, RunStatus::Verifying | RunStatus::Blocked)
                || (subject.status == RunStatus::Blocked && subject.blocked_source != Some(BlockSource::Verification))
            {
                return Err(FactoryError::BadRequest("the referenced subject is not awaiting verification".into()));
            }
            let step = subject
                .required_steps
                .iter()
                .find(|step| step.kind == StepKind::Review && step.step == *step_name)
                .ok_or_else(|| {
                    FactoryError::BadRequest(
                        "the referenced subject does not require this review step".into(),
                    )
                })?;
            if step.actor.as_deref() != Some(executor) || executor == subject.agent {
                return Err(FactoryError::Denied(
                    "the review task is not bound to this independent executor".into(),
                ));
            }
            let subject_task = self.require(&subject.task_id).await?;
            match (&subject_task.workflow_origin, &task.workflow_origin, &step.node_id) {
                (None, None, _) => {}
                (Some(subject_origin), Some(review_origin), Some(node_id))
                    if subject_origin.workflow_run_id == review_origin.workflow_run_id
                        && review_origin.node_id == *node_id => {}
                _ => return Err(FactoryError::BadRequest(
                    "the review task provenance does not match the subject workflow step".into(),
                )),
            }
            return Ok(Vec::new());
        }
        if let Some(origin) = &task.workflow_origin {
            let run = self.workflow_run(&origin.workflow_run_id).await?;
            return Ok(run.definition.required_steps_for(&origin.node_id));
        }
        let definition = WorkflowDefinition::implicit(task);
        let plans = self.control_plans(&definition).await?;
        let (mut injected, _) = definition.inject(&plans);
        self.bind_functionaries(&mut injected)?;
        let mut steps = injected.required_steps_for(IMPLICIT_NODE);
        for step in &mut steps {
            if step.kind == control_plan::StepKind::Review
                && step.actor.as_deref() == Some(executor)
            {
                step.actor = None;
            }
        }
        Ok(steps)
    }

    /// Freeze review and approval functionaries into an immutable workflow
    /// snapshot. Scope declaration order is stable and deliberate; the first
    /// concrete agent different from the subject executor is the checker.
    pub(crate) fn bind_functionaries(&self, definition: &mut WorkflowDefinition) -> Result<()> {
        let factory = self.factory_snapshot();
        let scope = factory.scope(&definition.scope)?;
        let roster = scope.agents_with(&factory.config.daemon.foreman);
        let subjects: Vec<(String, String)> = definition
            .nodes
            .iter()
            .filter(|n| n.kind == factory_core::workflow::WorkflowNodeKind::Task)
            .map(|n| {
                let requested = n
                    .task
                    .agent
                    .clone()
                    .or_else(|| scope.agent_adapter().map(str::to_string))
                    .unwrap_or_else(|| factory.config.daemon.default_agent.clone());
                let executor = self
                    .resolve_agent(&definition.scope, &requested)
                    .map(|v| v.0)
                    .unwrap_or(requested);
                (n.id.clone(), executor)
            })
            .collect();
        for node in &mut definition.nodes {
            let Some(spec) = node.gate.as_mut() else {
                continue;
            };
            let Some((_, executor)) = subjects
                .iter()
                .find(|(id, _)| spec.subject.as_deref() == Some(id))
            else {
                continue;
            };
            spec.actor = match node.kind {
                factory_core::workflow::WorkflowNodeKind::Review => roster
                    .iter()
                    .map(|a| a.name())
                    .find(|name| name != executor),
                factory_core::workflow::WorkflowNodeKind::Approval => Some("owner".into()),
                _ => spec.actor.clone(),
            };
        }
        Ok(())
    }

    /// The agent reported `done` on a run with required steps: hold it in
    /// `verifying` and hand it to the verifier. `patch` is the report's own
    /// (result, message); the status is this function's to set.
    pub(crate) async fn begin_verification(&self, run: &Run, patch: RunPatch) -> Result<Run> {
        let run = self
            .store
            .update_run(
                &run.id,
                &RunPatch { status: Some(RunStatus::Verifying), clear_blocked: true, fail_kind: None, ..patch },
            )
            .await?;
        self.bus.publish(Event::RunUpdated { run: run.clone() });
        self.mirror_to_task(&run).await;
        let steps: Vec<&str> = run.required_steps.iter().filter(|s| s.kind.enforced()).map(|s| s.step.as_str()).collect();
        self.entry(
            &run.task_id,
            TaskEntry::new(
                "daemon",
                "verifying",
                format!("reported done; verifying before it counts: {}", steps.join(", ")),
            )
            .in_run(&run.id),
        )
        .await;
        self.enqueue_verification(&run.id);
        Ok(run)
    }

    fn release_verification(&self, run_id: &str) {
        let replay = {
            let mut verifying = self.verifying.lock().unwrap();
            match verifying.get_mut(run_id) {
                Some(requested) if *requested => {
                    *requested = false;
                    true
                }
                Some(_) => {
                    verifying.remove(run_id);
                    false
                }
                None => false,
            }
        };
        if replay {
            let _ = self.verify_tx.send(run_id.to_string());
        }
    }

    pub(crate) fn enqueue_verification(&self, run_id: &str) {
        let enqueue = {
            let mut verifying = self.verifying.lock().unwrap();
            match verifying.get_mut(run_id) {
                Some(requested) => {
                    *requested = true;
                    false
                }
                None => {
                    verifying.insert(run_id.to_string(), false);
                    true
                }
            }
        };
        if enqueue {
            let _ = self.verify_tx.send(run_id.to_string());
        }
    }

    /// Start the verifier: every enqueued run is verified on a task of its
    /// own, so one slow gate never holds another run's verification up.
    /// Called once, at startup, like `spawn_bench_judge`; a second call is a
    /// no-op.
    pub fn spawn_verifier(self: &Arc<Self>) {
        let Some(mut rx) = self.verify_rx.lock().unwrap().take() else {
            return;
        };
        let engine = self.clone();
        tokio::spawn(async move {
            while let Some(run_id) = rx.recv().await {
                let engine = engine.clone();
                tokio::spawn(async move {
                    // `verify_run` lets go of the run itself on every way it
                    // finishes; only an error leaves that to here.
                    if let Err(error) = engine.verify_run(&run_id).await {
                        tracing::warn!(run = %run_id, "verification failed to complete: {error}");
                        engine.release_verification(&run_id);
                    }
                });
            }
        });
    }

    /// Runs still `verifying` when the daemon last stopped resume against the
    /// exact current digest. Complete gate evidence for that digest is reused;
    /// a changed tree starts a fresh gate-then-review round.
    pub(crate) async fn recover_verifications(&self) {
        match self.store.active_runs().await {
            Ok(runs) => {
                for run in runs.into_iter().filter(|r| r.status == RunStatus::Verifying) {
                    self.enqueue_verification(&run.id);
                }
            }
            Err(error) => tracing::warn!("could not look for runs left verifying: {error}"),
        }
    }

    /// Ensure every independent-review step has exactly one task. Returns
    /// true while review evidence is still outstanding.
    async fn ensure_review_tasks(
        self: &Arc<Self>,
        subject: &Run,
        task: &Task,
        dir: &Path,
        state: &GitState,
    ) -> Result<bool> {
        let digest = state.digest.as_deref().ok_or_else(|| {
            FactoryError::Other(anyhow::anyhow!(
                "could not compute the worktree digest for independent review"
            ))
        })?;
        let attestations = self.policies.step_attestations(&subject.id).await?;
        let all_tasks = self.store.list(&TaskFilter::default()).await?;
        let mut waiting = false;
        for step in subject
            .required_steps
            .iter()
            .filter(|s| s.kind == StepKind::Review)
        {
            if attestations
                .iter()
                .any(|a| {
                    a.step == step.step
                        && a.kind == StepKind::Review
                        && a.worktree_digest.as_deref() == Some(digest)
                })
            {
                continue;
            }
            let Some(actor) = step.actor.clone() else {
                continue; // judge names the missing independent functionary
            };
            if actor == subject.agent {
                continue; // never manufacture self-review
            }
            let existing = all_tasks
                .iter()
                .filter(|candidate| {
                    candidate.labels.get(REVIEW_RUN_LABEL) == Some(&subject.id)
                        && candidate.labels.get(REVIEW_STEP_LABEL) == Some(&step.step)
                        && candidate
                            .labels
                            .get(REVIEW_DIGEST_LABEL)
                            .map(String::as_str)
                            == Some(digest)
                })
                .max_by_key(|candidate| candidate.created_at);
            if let Some(existing) = existing {
                if let Some(review_run) = self.store.runs(&existing.id, 1).await?.into_iter().next() {
                    if review_run.status == RunStatus::Done {
                        match self
                            .record_review_attestation(existing, &review_run, subject, step)
                            .await
                        {
                            Ok(()) => continue,
                            Err(error) => {
                                self.entry(&task.id, TaskEntry::new(
                                    "daemon", "review_unusable",
                                    format!("review task {} ended without usable evidence: {error}; fix or retry the subject to request another review", existing.id),
                                ).in_run(&subject.id)).await;
                                continue;
                            }
                        }
                    }
                    if review_run.status.is_terminal() {
                        self.entry(&task.id, TaskEntry::new(
                            "daemon", "review_unusable",
                            format!("review task {} ended {} without a verdict; retry that review task, or resolve it as a person", existing.id, review_run.status.as_str()),
                        ).in_run(&subject.id)).await;
                        continue;
                    } else {
                        waiting = true;
                        continue;
                    }
                }
            }

            let mut labels = std::collections::BTreeMap::new();
            labels.insert(REVIEW_RUN_LABEL.into(), subject.id.clone());
            labels.insert(REVIEW_STEP_LABEL.into(), step.step.clone());
            labels.insert(REVIEW_DIGEST_LABEL.into(), digest.to_string());
            let instructions = format!(
                "Independently review task {} (run {}).\n\nSubject result:\n{}\n\nWorktree: {}\nCommit: {}\nDirty: {}\n\nReport plain done to pass. Report done --send-to {} with --result containing concrete findings to reject and propose rework.",
                task.id,
                subject.id,
                subject.result.as_deref().unwrap_or("(no result supplied)"),
                dir.display(),
                state.commit.as_deref().unwrap_or("unknown"),
                state.dirty.map(|v| v.to_string()).unwrap_or_else(|| "unknown".into()),
                task.workflow_origin.as_ref().map(|o| o.node_id.as_str()).unwrap_or("rework"),
            );
            let new = NewTask {
                title: format!("Review: {}", task.title),
                instructions,
                scope: Some(task.scope.clone()),
                agent: Some(actor),
                runtime: Some(task.runtime.clone()),
                worktree: Some(false),
                labels,
                ..Default::default()
            };
            let review_id = uuid::Uuid::new_v4().to_string();
            let review_origin = match (&task.workflow_origin, &step.node_id) {
                (Some(origin), Some(node_id)) => Some(WorkflowOrigin {
                    workflow_id: origin.workflow_id.clone(),
                    workflow_run_id: origin.workflow_run_id.clone(),
                    node_id: node_id.clone(),
                    workspace: None,
                }),
                _ => None,
            };
            let previous_task = if let (Some(origin), Some(node_id)) = (&task.workflow_origin, &step.node_id) {
                self.workflow_run(&origin.workflow_run_id).await?.nodes.iter()
                    .find(|node| node.node_id == *node_id).and_then(|node| node.task_id.clone())
            } else { None };
            let review = if let Some(previous_id) = previous_task {
                // A control node also owns one standing task. Its next
                // independent review is a fresh conversation on a new run.
                if self.store.active_run(&previous_id).await?.is_some() {
                    waiting = true;
                    continue;
                }
                self.store.update(&previous_id, &factory_core::task::TaskPatch {
                    instructions: Some(new.instructions),
                    agent: new.agent, runtime: new.runtime,
                    labels: Some(new.labels),
                    status: Some(factory_core::task::TaskStatus::Pending),
                    clear_result: true, clear_error: true, clear_routed_to: true,
                    clear_failure: true, clear_closure: true,
                    clear_after: true,
                    ..Default::default()
                }).await?
            } else {
                self.create_review_task(new, review_origin, review_id).await?
            };
            if let (Some(origin), Some(node_id)) = (&task.workflow_origin, &step.node_id) {
                let mut workflow = self.workflow_run(&origin.workflow_run_id).await?;
                if let Some(node) = workflow.nodes.iter_mut().find(|n| n.node_id == *node_id) {
                    node.task_id = Some(review.id.clone());
                    node.task_created = true;
                    node.status = WorkflowNodeStatus::Pending;
                }
                self.workflows.put_run(&workflow).await?;
                self.bus
                    .publish(Event::WorkflowRunUpdated { run: workflow });
            }
            self.entry(
                &task.id,
                TaskEntry::new(
                    "daemon",
                    "review_requested",
                    format!("{} will independently review {}", review.agent, step.step),
                )
                .in_run(&subject.id)
                .with_data(serde_json::json!({ "review_task": review.id, "step": step.step })),
            )
            .await;
            let engine = self.clone();
            let review_task = review.id.clone();
            tokio::spawn(async move {
                engine
                    .start_run(&review_task, factory_core::run::Trigger::Workflow)
                    .await;
            });
            waiting = true;
        }
        Ok(waiting)
    }

    async fn record_review_attestation(
        &self,
        review_task: &Task,
        review_run: &Run,
        subject: &Run,
        step: &RequiredStep,
    ) -> Result<()> {
        if self
            .policies
            .step_attestations(&subject.id)
            .await?
            .iter()
            .any(|a| {
                a.kind == StepKind::Review && a.step == step.step && a.actor == review_run.agent
            })
        {
            return Ok(());
        }
        if review_run.agent == subject.agent {
            return Err(FactoryError::Denied(
                "the executing agent cannot attest its own review".into(),
            ));
        }
        let rejected = review_run.routed_to.is_some();
        if rejected
            && review_run
                .result
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .is_empty()
        {
            return Err(FactoryError::BadRequest(
                "a rejected review needs concrete findings in --result".into(),
            ));
        }
        let subject_task = self.require(&subject.task_id).await?;
        let dir = subject.worktree_path.clone().unwrap_or_else(|| {
            self.factory_snapshot()
                .scope_path(&subject_task.scope)
                .unwrap_or_default()
                .display()
                .to_string()
        });
        let state = git_state(Path::new(&dir)).await;
        let expected_digest = review_task
            .labels
            .get(REVIEW_DIGEST_LABEL)
            .ok_or_else(|| {
                FactoryError::BadRequest("the review task has no worktree digest".into())
            })?;
        if state.digest.as_deref() != Some(expected_digest.as_str()) {
            return Err(FactoryError::BadRequest(
                "the subject worktree changed while review was outstanding; a fresh gate and review round is required".into(),
            ));
        }
        let attestation = StepAttestation {
            id: uuid::Uuid::new_v4().to_string(),
            run_id: subject.id.clone(),
            task_id: subject.task_id.clone(),
            scope: subject_task.scope.clone(),
            category: control_plan::effective_category(subject_task.category.as_deref()).into(),
            step: step.step.clone(),
            kind: StepKind::Review,
            actor: review_run.agent.clone(),
            verdict: if rejected {
                AttestationVerdict::Fail
            } else {
                AttestationVerdict::Pass
            },
            findings: review_run.result.clone(),
            round: 0,
            required_by: step.required_by.clone(),
            command: None,
            exit_code: None,
            output: review_run.result.clone(),
            dir,
            commit: state.commit,
            dirty: state.dirty,
            worktree_digest: state.digest,
            node_id: step.node_id.clone(),
            at: review_run.ended_at.unwrap_or_else(Utc::now),
        };
        self.policies.append_step_attestation(&attestation).await?;
        self.entry(
            &subject.task_id,
            TaskEntry::new(
                &review_run.agent,
                "reviewed",
                format!("{}: {}", step.step, attestation.verdict.as_str()),
            )
            .in_run(&subject.id)
            .with_data(
                serde_json::json!({ "attestation": attestation.id, "review_task": review_task.id }),
            ),
        )
        .await;
        self.enqueue_verification(&subject.id);
        Ok(())
    }

    /// Called after a review task reaches done; turns its report into the
    /// subject run's independent attestation and wakes the verifier.
    pub(crate) async fn settle_review_task(
        &self,
        review_task: &Task,
        review_run: &Run,
    ) -> Result<()> {
        let Some(subject_id) = review_task.labels.get(REVIEW_RUN_LABEL) else {
            return Ok(());
        };
        let mut subject = self.require_run(subject_id).await?;
        if subject.status == RunStatus::Blocked
            && subject.blocked_source == Some(BlockSource::Verification)
        {
            subject = self
                .store
                .update_run(
                    &subject.id,
                    &RunPatch {
                        status: Some(RunStatus::Verifying),
                        clear_blocked: true,
                        ..Default::default()
                    },
                )
                .await?;
            self.bus.publish(Event::RunUpdated { run: subject.clone() });
            self.mirror_to_task(&subject).await;
        }
        let step_name = review_task
            .labels
            .get(REVIEW_STEP_LABEL)
            .cloned()
            .unwrap_or_else(|| "review".into());
        let step = subject
            .required_steps
            .iter()
            .find(|s| s.kind == StepKind::Review && s.step == step_name)
            .ok_or_else(|| {
                FactoryError::BadRequest("the subject run no longer requires this review".into())
            })?;
        if let Err(error) = self
            .record_review_attestation(review_task, review_run, &subject, step)
            .await
        {
            self.enqueue_verification(&subject.id);
            return Err(error);
        }
        // A rejected review proposes rework; it does not take the route by
        // itself. Clear the task mirror before workflow reconciliation so
        // only `run rework` performs the bounded send-back.
        if review_run.routed_to.is_some() {
            let _ = self
                .store
                .update(
                    &review_task.id,
                    &factory_core::task::TaskPatch {
                        clear_routed_to: true,
                        ..Default::default()
                    },
                )
                .await?;
        }
        Ok(())
    }

    /// Run a verifying run's required gates in order, attest each, and
    /// settle it. Stops at the first failure -- the andon: a step ordered
    /// after a failed one (a publish after a failed scan) must not run on
    /// work that already failed. Re-reads the run before settling, so a
    /// cancel that landed while a gate ran stands.
    pub(crate) async fn verify_run(self: &Arc<Self>, run_id: &str) -> Result<()> {
        let run = self.require_run(run_id).await?;
        if run.status != RunStatus::Verifying {
            self.release_verification(run_id);
            return Ok(());
        }
        let task = self.require(&run.task_id).await?;
        let dir = match &run.worktree_path {
            Some(path) => std::path::PathBuf::from(path),
            None => self.factory_snapshot().scope_path(&task.scope)?,
        };
        let category = control_plan::effective_category(task.category.as_deref()).to_string();
        let state = git_state(&dir).await;
        let mut attestations = self.policies.step_attestations(&run.id).await?;
        let round_evidence = |all: &[StepAttestation]| {
            all.iter()
                .filter(|evidence| {
                    evidence.kind == StepKind::Approval
                        || state.digest.as_deref().is_some_and(|digest| {
                            evidence.worktree_digest.as_deref() == Some(digest)
                        })
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        let gate_steps = run
            .required_steps
            .iter()
            .filter(|step| step.kind == StepKind::Gate)
            .cloned()
            .collect::<Vec<_>>();
        let existing_gates = control_plan::judge(
            &gate_steps,
            &round_evidence(&attestations),
            &run.agent,
            chrono::DateTime::<Utc>::MIN_UTC,
        );

        for step in run.required_steps.iter().filter(|s| s.kind.enforced()) {
            if existing_gates.passed {
                break;
            }
            if step.kind != StepKind::Gate {
                continue;
            }
            let Some(command) = step.command.as_deref() else {
                continue; // nothing to run: `judge` reports it missing
            };
            let timeout = step.timeout_seconds.unwrap_or(DEFAULT_GATE_TIMEOUT_SECS);
            let (exit_code, output) = run_shell_capture(&dir, command, timeout).await;
            let verdict = if exit_code == Some(0) {
                AttestationVerdict::Pass
            } else {
                AttestationVerdict::Fail
            };
            let attestation = StepAttestation {
                id: uuid::Uuid::new_v4().to_string(),
                run_id: run.id.clone(),
                task_id: task.id.clone(),
                scope: task.scope.clone(),
                category: category.clone(),
                step: step.step.clone(),
                kind: step.kind,
                actor: GATE_ACTOR.to_string(),
                verdict,
                findings: (verdict == AttestationVerdict::Fail)
                    .then(|| factory_core::bench::tail_4kib(&output)),
                round: 0,
                required_by: step.required_by.clone(),
                command: Some(command.to_string()),
                exit_code,
                output: Some(factory_core::bench::tail_4kib(&output)),
                dir: dir.display().to_string(),
                commit: state.commit.clone(),
                dirty: state.dirty,
                worktree_digest: state.digest.clone(),
                node_id: step.node_id.clone(),
                at: Utc::now(),
            };
            self.policies.append_step_attestation(&attestation).await?;
            let code = exit_code
                .map(|c| format!("exit {c}"))
                .unwrap_or_else(|| "did not finish".into());
            self.entry(
                &task.id,
                TaskEntry::new("daemon", "attested", format!("{}: {} ({code})", step.step, verdict.as_str()))
                    .in_run(&run.id)
                    .with_data(serde_json::json!({ "attestation": attestation.id })),
            )
            .await;
            if verdict == AttestationVerdict::Fail {
                break;
            }
            // A cancel while this gate ran: the rest are not worth running.
            if self.require_run(run_id).await?.status != RunStatus::Verifying {
                self.release_verification(run_id);
                return Ok(());
            }
        }

        let after_gates = git_state(&dir).await;
        if after_gates != state {
            self.enqueue_verification(run_id);
            self.release_verification(run_id);
            self.sync_workflow_for_task(&task.id).await;
            return Ok(());
        }

        // Deterministic gates pass before a model review is ever spent. The
        // subject remains `verifying`; the review task's report wakes this
        // same coordinator, and labels make restart recovery idempotent.
        attestations = self.policies.step_attestations(&run.id).await?;
        let current_evidence = round_evidence(&attestations);
        let gates = control_plan::judge(
            &gate_steps,
            &current_evidence,
            &run.agent,
            chrono::DateTime::<Utc>::MIN_UTC,
        );
        if gates.passed && self.ensure_review_tasks(&run, &task, &dir, &state).await? {
            self.release_verification(run_id);
            self.sync_workflow_for_task(&task.id).await;
            return Ok(());
        }

        let attestations = self.policies.step_attestations(&run.id).await?;
        let verdict = control_plan::judge(
            &run.required_steps,
            &round_evidence(&attestations),
            &run.agent,
            chrono::DateTime::<Utc>::MIN_UTC,
        );
        // Free the run for its next `done` before settling it: once it reads
        // `blocked`, a report may re-enqueue it at any moment.
        self.release_verification(run_id);
        let run = self.require_run(run_id).await?;
        if run.status != RunStatus::Verifying {
            return Ok(());
        }
        if verdict.passed {
            // Keep the session available if artifact validation/publication
            // blocks completion. `finish_run` journals and mirrors the hold.
            if let Err(error) = self.finish_run(
                &run.id,
                RunStatus::Done,
                RunPatch { status: Some(RunStatus::Done), ..Default::default() },
                "verified",
            ).await {
                self.sync_workflow_for_task(&task.id).await;
                return Err(error);
            }
            self.entry(
                &task.id,
                TaskEntry::new("daemon", "verified", "every required step attested and passed").in_run(&run.id),
            )
            .await;
        } else {
            let reason = verdict.reason();
            let blocked = self
                .store
                .update_run(
                    &run.id,
                    &RunPatch {
                        status: Some(RunStatus::Blocked),
                        blocked_since: Some(Utc::now()),
                        blocked_source: Some(BlockSource::Verification),
                        ..Default::default()
                    },
                )
                .await?;
            // `kind: "blocked"` is what the Inbox reads a block's reason from.
            self.entry(
                &task.id,
                TaskEntry::new(
                    "daemon",
                    "blocked",
                    format!("{reason}. Fix it and report done again to re-verify, or cancel the run."),
                )
                .in_run(&run.id)
                .with_data(serde_json::to_value(&verdict).unwrap_or_default()),
            )
            .await;
            self.bus.publish(Event::RunUpdated { run: blocked.clone() });
            self.mirror_to_task(&blocked).await;
        }
        self.sync_workflow_for_task(&task.id).await;
        Ok(())
    }

    /// Every attestation a run has collected, oldest first.
    pub(crate) async fn run_attestations(&self, run_id: &str) -> Result<Vec<StepAttestation>> {
        self.require_run(run_id).await?;
        self.policies.step_attestations(run_id).await
    }

    /// `#158` phase 1: the one L4-owned read the `attested` policy check and
    /// both `conformance_rate.<category>`/`gate_fail_rate` share -- every
    /// finished run whose `ended_at` falls in `window` (`(from, to]`,
    /// `operations::is_finished`), narrowed to `scopes` (canonical scope
    /// names, no ancestor roll-up -- the caller already decided whether that
    /// is one exact scope or a whole subtree) and to `categories` when
    /// either is given, together with its `required_steps` (frozen at
    /// dispatch, `Run::required_steps`), its `fail_kind` (`None` for a run
    /// that ended `Done` -- `conformance_rate` reads this to exclude an
    /// infrastructure failure from its ratio), and every attestation it
    /// collected (`PolicyStore::step_attestations_for`, one batch read). A
    /// bench attempt's task (`Task::bench_origin`) is always left out -- its
    /// own case gate judges it, the same rule `required_steps_for_task`
    /// already applies, so a plan on top would never apply to it anyway.
    /// Test compatibility entry point: production reads this through the
    /// L4-owned fact provider in `facts/l4.rs`.
    #[cfg(test)]
    pub(crate) async fn attested_runs(
        &self, scopes: Option<&BTreeSet<String>>, categories: Option<&BTreeSet<String>>, window: Window,
    ) -> Result<Vec<AttestedRun>> {
        crate::facts::Facts::<factory_kernel::L6>::new(self).get::<AttestedRun>(&crate::facts::AttestedQuery {
            scopes: scopes.cloned(), categories: categories.cloned(), window,
        }).await
    }

    /// `factory workflow lint`: the plan, the injection and the ordering
    /// violations for a stored workflow, a task's implicit workflow, or --
    /// with neither -- just a scope's plan for one category.
    pub(crate) async fn workflow_lint(
        &self,
        workflow: Option<String>,
        task: Option<String>,
        scope: Option<String>,
        category: Option<String>,
    ) -> Result<WorkflowLint> {
        let definition = match (workflow, task) {
            (Some(_), Some(_)) => {
                return Err(FactoryError::BadRequest("lint a workflow or a task, not both".into()));
            }
            (Some(id), None) => {
                let mut definition = self.workflow_definition(&id).await?;
                if category.is_some() {
                    definition.category = category.clone();
                }
                Some(definition)
            }
            (None, Some(id)) => {
                let mut task = self.require(&id).await?;
                if category.is_some() {
                    task.category = category.clone();
                }
                Some(WorkflowDefinition::implicit(&task))
            }
            (None, None) => None,
        };
        let Some(definition) = definition else {
            let scope = scope.ok_or_else(|| {
                FactoryError::BadRequest("name a workflow, a task, or a scope to lint".into())
            })?;
            let category = control_plan::effective_category(category.as_deref()).to_string();
            let plan = self.control_plan(&scope, &category).await?;
            return Ok(WorkflowLint {
                subject: String::new(),
                scope: plan.scope.clone(),
                plans: vec![plan],
                injections: Vec::new(),
                violations: Vec::new(),
                injected: None,
                part: None,
            });
        };
        definition.validate().map_err(FactoryError::BadRequest)?;
        // `#235`: a part workflow never runs as itself, only copied once per
        // part into a decomposition run -- so that is what is injected and
        // shown, over two sample parts.
        let part = match &definition.part {
            Some(_) => Some(definition.part_shape().map_err(FactoryError::BadRequest)?),
            None => None,
        };
        let subject = definition.id.clone();
        let scope = definition.scope.clone();
        let definition = match &part {
            Some(shape) => definition.part_preview(shape),
            None => definition,
        };
        let plans = self.control_plans(&definition).await?;
        let (mut injected, injections) = definition.inject(&plans);
        self.bind_functionaries(&mut injected)?;
        let mut violations = injected.ordering_violations(&plans);
        for node in injected
            .nodes
            .iter()
            .filter(|n| n.kind == WorkflowNodeKind::Review)
        {
            if node.gate.as_ref().and_then(|g| g.actor.as_ref()).is_none() {
                violations.push(format!(
                    "review {} has no independent functionary: declare another concrete task agent in scope {}",
                    node.id, injected.scope
                ));
            }
        }
        Ok(WorkflowLint {
            subject,
            scope,
            plans: plans.into_values().collect(),
            injections,
            violations,
            injected: Some(injected),
            part,
        })
    }
}

#[cfg(test)]
mod tests {
    //! The done gate end to end through the real `Engine`: a report, the
    //! verifier, the attestation store, the Inbox and the workflow graph.
    //! Every run uses the `shell` agent on a runtime that does nothing, so no
    //! model and no terminal is involved -- only the gate commands run.

    use super::*;
    use crate::access::Caller;
    use factory_core::adapter::store::task_from_new;
    use factory_core::adapter::{AgentRuntime, StartRequest};
    use factory_core::bench::BenchOrigin;
    use factory_core::config::{Config, DaemonConfig, Factory, Instance, PolicyDeclaration, Scope};
    use factory_core::operations::{ExceptionKind, HealthWindow};
    use factory_core::run::{NewRun, Trigger};
    use factory_core::task::{NewTask, SessionRef, TaskReport};
    use factory_core::workflow::{CanvasPoint, WorkflowDraft, WorkflowEdge, WorkflowNode, WorkflowNodeKind, WorkflowNodeStatus};
    use factory_plugins::{HarnessAgent, Registry, SqliteStore};
    use std::path::PathBuf;

    struct QuietRuntime;
    #[async_trait::async_trait]
    impl AgentRuntime for QuietRuntime {
        fn name(&self) -> &str {
            "quiet"
        }
        async fn start(&self, req: &StartRequest) -> Result<SessionRef> {
            Ok(SessionRef { runtime: "quiet".into(), handle: req.id.clone(), meta: Default::default() })
        }
        async fn submit(&self, _: &SessionRef, _: &str) -> Result<()> {
            Ok(())
        }
        async fn status(&self, _: &SessionRef) -> Result<factory_core::adapter::RuntimeStatus> {
            Ok(factory_core::adapter::RuntimeStatus::Working)
        }
        async fn send_text(&self, _: &SessionRef, _: &str) -> Result<()> {
            Ok(())
        }
        async fn send_keys(&self, _: &SessionRef, _: &[String]) -> Result<()> {
            Ok(())
        }
        async fn read(&self, _: &SessionRef, _: u32) -> Result<String> {
            Ok(String::new())
        }
        async fn stop(&self, _: &SessionRef) -> Result<()> {
            Ok(())
        }
    }

    struct FailingRuntime;
    #[async_trait::async_trait]
    impl AgentRuntime for FailingRuntime {
        fn name(&self) -> &str {
            "failing"
        }
        async fn start(&self, _: &StartRequest) -> Result<SessionRef> {
            Err(FactoryError::BadRequest("runtime start failed for the test".into()))
        }
        async fn submit(&self, _: &SessionRef, _: &str) -> Result<()> {
            Ok(())
        }
        async fn status(&self, _: &SessionRef) -> Result<factory_core::adapter::RuntimeStatus> {
            Ok(factory_core::adapter::RuntimeStatus::Working)
        }
        async fn send_text(&self, _: &SessionRef, _: &str) -> Result<()> {
            Ok(())
        }
        async fn send_keys(&self, _: &SessionRef, _: &[String]) -> Result<()> {
            Ok(())
        }
        async fn read(&self, _: &SessionRef, _: u32) -> Result<String> {
            Ok(String::new())
        }
        async fn stop(&self, _: &SessionRef) -> Result<()> {
            Ok(())
        }
    }

    /// An instance committed to one framework, `house`, whose single control
    /// requires `requires` -- YAML for one `requires:` list -- and whose
    /// only scope, `demo`, is a plain directory the gates run in.
    fn engine_with_verifier(requires: &str, spawn_verifier: bool) -> (Arc<Engine>, PathBuf) {
        let root = std::env::temp_dir().join(format!("factory-verify-test-{}", uuid::Uuid::new_v4()));
        let work = root.join("demo");
        std::fs::create_dir_all(root.join(".factory/policies")).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(
            root.join(".factory/policies/house.yaml"),
            format!(
                "framework: house\ntitle: House rules\nkind: best-practice\ncontrols:\n  - id: tested\n    title: Changes are tested\n    requires:\n{requires}\n"
            ),
        )
        .unwrap();
        let fake_harness = work.join("fake-harness");
        std::fs::write(&fake_harness, "#!/bin/sh\necho fake 1.0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&fake_harness).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&fake_harness, permissions).unwrap();
        }
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig {
                power_assertion: false,
                harness_health: factory_core::config::HarnessHealthConfig {
                    cache_seconds: 0,
                    ..Default::default()
                },
                ..DaemonConfig::default()
            },
            roles: Default::default(),
            dashboard: None,
            policies: PolicyDeclaration { frameworks: vec!["house".into()], ..Default::default() },
            quality: Default::default(),
            scope: None,
            scopes: vec![Scope {
                id: "demo-id".into(),
                name: "demo".into(),
                path: work.clone(),
                agent: None,
                agents: vec![factory_core::config::ScopeAgent {
                    name: Some("checker".into()),
                    harness: "shell".into(),
                    lifetime: Default::default(),
                    role: Default::default(),
                    autostart: None,
                    args: Vec::new(),
                    sandbox: Default::default(),
                    openshell: None,
                    provider: None,
                    max_sessions: None,
                }],
                runtime: Some("quiet".into()),
                git: None,
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
            }],
            infrastructure: Default::default(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let mut registry = Registry::with_builtins();
        registry.add_runtime(Arc::new(QuietRuntime), "test");
        registry.add_runtime(Arc::new(FailingRuntime), "test");
        registry.add_agent(
            Arc::new(HarnessAgent::new(
                "fake",
                fake_harness.display().to_string(),
                "test harness",
            )),
            "test",
        );
        let engine = Arc::new(Engine::new(
            Factory { root, config },
            registry,
            Arc::new(SqliteStore::in_memory().unwrap()),
            PathBuf::from("factory"),
            Vec::new(),
        ));
        if spawn_verifier {
            engine.spawn_verifier();
        }
        (engine, work)
    }

    fn engine(requires: &str) -> (Arc<Engine>, PathBuf) {
        engine_with_verifier(requires, true)
    }

    const TESTS_FOR_FEATURES: &str = "      - { applies_to: [feature], step: tests, gate: \"test -f built.txt\" }";

    async fn task(engine: &Arc<Engine>, category: Option<&str>) -> Task {
        engine
            .create(NewTask {
                title: "add the thing".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                runtime: Some("quiet".into()),
                worktree: Some(false),
                category: category.map(str::to_string),
                ..Default::default()
            })
            .await
            .unwrap()
    }

    async fn report_done(engine: &Arc<Engine>, task_id: &str) -> Run {
        let run = engine.store.active_run(task_id).await.unwrap().expect("an active run");
        let run = engine
            .report(
                task_id,
                TaskReport {
                    artifacts: Vec::new(),
                    status: Some(RunStatus::Done),
                    message: None,
                    result: Some("built it".into()),
                    send_to: None,
                    error: None,
                    token: run.token,
                },
            )
            .await
            .unwrap();
        engine.sync_workflow_for_task(task_id).await;
        run
    }

    /// Poll until the run leaves `verifying` -- the verifier works on a task
    /// of its own.
    async fn settled(engine: &Arc<Engine>, run_id: &str) -> Run {
        for _ in 0..400 {
            let run = engine.require_run(run_id).await.unwrap();
            if run.status != RunStatus::Verifying {
                return run;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("run {run_id} is still verifying");
    }

    async fn review_task(engine: &Arc<Engine>, subject_run: &str) -> Task {
        for _ in 0..400 {
            if let Some(task) = engine
                .store
                .list(&TaskFilter::default())
                .await
                .unwrap()
                .into_iter()
                .find(|t| {
                    t.labels
                        .get(REVIEW_RUN_LABEL)
                        .is_some_and(|id| id == subject_run)
                })
            {
                if engine.store.active_run(&task.id).await.unwrap().is_some() {
                    return task;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("no review task for {subject_run}");
    }

    #[test]
    fn a_review_wakeup_arriving_before_verifier_release_is_replayed() {
        let (engine, _) = engine_with_verifier("", false);
        engine.enqueue_verification("subject");
        // This is what `record_review_attestation` does when a very fast
        // review finishes while `ensure_review_tasks` still owns the run.
        engine.enqueue_verification("subject");
        engine.release_verification("subject");

        let mut receiver = engine.verify_rx.lock().unwrap().take().unwrap();
        assert_eq!(receiver.try_recv().unwrap(), "subject");
        assert_eq!(receiver.try_recv().unwrap(), "subject");
        assert!(receiver.try_recv().is_err());

        // Completing the replay releases ownership normally.
        engine.release_verification("subject");
        assert!(!engine.verifying.lock().unwrap().contains_key("subject"));
    }

    #[tokio::test]
    async fn approval_blocks_before_agent_launch_and_an_owner_decision_resumes_the_same_run() {
        let (engine, _) = engine("      - { applies_to: [feature], step: approval, by: person }");
        let task = task(&engine, Some("feature")).await;
        engine.start_run(&task.id, Trigger::Manual).await;
        let held = engine.store.active_run(&task.id).await.unwrap().unwrap();
        assert_eq!(held.status, RunStatus::Blocked);
        assert!(held.session.is_none(), "approval is before launch");
        assert_eq!(held.required_steps[0].actor.as_deref(), Some("owner"));

        let resumed = engine
            .decide_approval(
                &Caller::Owner,
                &held.id,
                AttestationVerdict::Pass,
                "release approved",
            )
            .await
            .unwrap();
        assert_eq!(resumed.id, held.id, "approval resumes the frozen attempt");
        assert!(resumed.session.is_some());
        let attestations = engine.run_attestations(&held.id).await.unwrap();
        assert_eq!(attestations.len(), 1);
        assert_eq!(attestations[0].actor, "owner");
    }

    #[tokio::test]
    async fn an_approved_run_waits_for_capacity_and_resumes_without_a_second_approval() {
        let (engine, _) = engine("      - { applies_to: [feature], step: approval, by: person }");
        let mut scope = engine.factory_snapshot().config.scopes[0].clone();
        scope.max_sessions = Some(1);
        engine.replace_scope(&scope.id.clone(), scope);

        let subject = task(&engine, Some("feature")).await;
        engine.start_run(&subject.id, Trigger::Manual).await;
        let held = engine.store.active_run(&subject.id).await.unwrap().unwrap();
        let holder = task(&engine, Some("chore")).await;
        engine.start_run(&holder.id, Trigger::Manual).await;
        assert!(engine.store.active_run(&holder.id).await.unwrap().unwrap().session.is_some());

        let queued = engine.decide_approval(
            &Caller::Owner, &held.id, AttestationVerdict::Pass, "approved while busy",
        ).await.unwrap();
        assert_eq!(queued.id, held.id);
        assert_eq!(queued.status, RunStatus::Blocked);
        assert!(queued.session.is_none());
        assert!(engine.require(&subject.id).await.unwrap().slot_wait.is_some());
        assert_eq!(engine.run_attestations(&held.id).await.unwrap().len(), 1);

        report_done(&engine, &holder.id).await;
        engine.recheck_capacity().await;
        let resumed = engine.require_run(&held.id).await.unwrap();
        assert!(resumed.session.is_some());
        assert!(engine.require(&subject.id).await.unwrap().slot_wait.is_none());
        assert_eq!(engine.store.runs(&subject.id, 10).await.unwrap().len(), 1);
        report_done(&engine, &subject.id).await;
        assert_eq!(settled(&engine, &held.id).await.status, RunStatus::Done);
    }

    async fn assert_approval_resume_failed(engine: &Arc<Engine>, task: &Task, held: &Run) {
        let error = engine
            .decide_approval(
                &Caller::Owner,
                &held.id,
                AttestationVerdict::Pass,
                "release approved",
            )
            .await
            .unwrap_err();
        assert!(!error.to_string().is_empty());
        let failed = engine.require_run(&held.id).await.unwrap();
        assert_eq!(failed.status, RunStatus::Failed);
        assert_eq!(failed.fail_kind, Some(factory_core::run::FailKind::DispatchFailed));
        assert!(failed.session.is_none());
        assert!(engine.store.active_run(&task.id).await.unwrap().is_none());
        assert!(engine.require(&task.id).await.unwrap().blocked_by_failure());
    }

    #[tokio::test]
    async fn a_harness_failure_after_approval_settles_the_held_run() {
        let (engine, work) = engine("      - { applies_to: [feature], step: approval, by: person }");
        let task = engine
            .create(NewTask {
                title: "use the probed harness".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("fake".into()),
                runtime: Some("quiet".into()),
                worktree: Some(false),
                category: Some("feature".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        engine.start_run(&task.id, Trigger::Manual).await;
        let held = engine.store.active_run(&task.id).await.unwrap().unwrap();
        std::fs::remove_file(work.join("fake-harness")).unwrap();

        assert_approval_resume_failed(&engine, &task, &held).await;
    }

    #[tokio::test]
    async fn a_worktree_failure_after_approval_settles_the_held_run() {
        let (engine, _) = engine("      - { applies_to: [feature], step: approval, by: person }");
        let task = engine
            .create(NewTask {
                title: "needs a worktree".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                runtime: Some("quiet".into()),
                worktree: Some(true),
                category: Some("feature".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        engine.start_run(&task.id, Trigger::Manual).await;
        let held = engine.store.active_run(&task.id).await.unwrap().unwrap();

        assert_approval_resume_failed(&engine, &task, &held).await;
    }

    #[tokio::test]
    async fn a_runtime_start_failure_after_approval_settles_the_held_run() {
        let (engine, _) = engine("      - { applies_to: [feature], step: approval, by: person }");
        let task = engine
            .create(NewTask {
                title: "uses a failing runtime".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                runtime: Some("failing".into()),
                worktree: Some(false),
                category: Some("feature".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        engine.start_run(&task.id, Trigger::Manual).await;
        let held = engine.store.active_run(&task.id).await.unwrap().unwrap();

        assert_approval_resume_failed(&engine, &task, &held).await;
    }

    #[tokio::test]
    async fn the_executor_cannot_approve_its_own_held_run() {
        let (engine, _) = engine("      - { applies_to: [feature], step: approval, by: person }");
        let task = task(&engine, Some("feature")).await;
        engine.start_run(&task.id, Trigger::Manual).await;
        let held = engine.store.active_run(&task.id).await.unwrap().unwrap();
        let caller = Caller::Agent {
            scope: "demo".into(),
            name: held.agent.clone(),
            role: factory_core::role::Role::foreman(),
            run_id: Some(held.id.clone()),
        };
        let error = engine
            .decide_approval(&caller, &held.id, AttestationVerdict::Pass, "self approve")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("cannot approve"), "{error}");
        assert!(engine.run_attestations(&held.id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_rejected_approval_stays_blocked_without_launching_the_agent() {
        let requires = "      - { applies_to: [feature], step: approval, by: person }\n      - { applies_to: [feature], step: tests, gate: \"true\" }\n      - { applies_to: [feature], step: review, by: independent }";
        let (engine, _) = engine(requires);
        let task = task(&engine, Some("feature")).await;
        engine.start_run(&task.id, Trigger::Manual).await;
        let held = engine.store.active_run(&task.id).await.unwrap().unwrap();
        engine
            .decide_approval(&Caller::Owner, &held.id, AttestationVerdict::Fail, "release evidence is missing")
            .await
            .unwrap();
        let rejected = settled(&engine, &held.id).await;
        assert_eq!(rejected.status, RunStatus::Blocked);
        assert!(rejected.session.is_none());
        let evidence = engine.run_attestations(&held.id).await.unwrap();
        assert_eq!(evidence.last().unwrap().verdict, AttestationVerdict::Fail);
        assert_eq!(evidence.last().unwrap().findings.as_deref(), Some("release evidence is missing"));
        assert_eq!(evidence.len(), 1, "rejection did not run the later gate");
        assert!(engine.store.list(&TaskFilter::default()).await.unwrap().iter().all(|task| {
            !task.labels.contains_key(REVIEW_RUN_LABEL)
        }), "rejection did not spawn the later review");
    }

    #[tokio::test]
    async fn reserved_review_labels_cannot_be_created_or_edited_by_a_caller() {
        let (engine, _) = engine(TESTS_FOR_FEATURES);
        let mut labelled = NewTask {
            title: "ordinary work".into(),
            scope: Some("demo".into()),
            agent: Some("shell".into()),
            runtime: Some("quiet".into()),
            worktree: Some(false),
            category: Some("feature".into()),
            ..Default::default()
        };
        labelled.labels.insert(REVIEW_RUN_LABEL.into(), "forged".into());
        let error = engine.create(labelled).await.unwrap_err();
        assert!(error.to_string().contains("reserved"), "{error}");

        let ordinary = task(&engine, Some("feature")).await;
        let error = engine.update(
            &ordinary.id,
            factory_core::task::TaskPatch {
                labels: Some(BTreeMap::from([(REVIEW_STEP_LABEL.into(), "review".into())])),
                ..Default::default()
            },
            None,
        ).await.unwrap_err();
        assert!(error.to_string().contains("reserved"), "{error}");
    }

    #[tokio::test]
    async fn gates_run_before_an_independent_review_and_plain_done_passes_it() {
        let requires = "      - { applies_to: [feature], step: tests, gate: \"true\" }\n      - { applies_to: [feature], step: review, by: independent }";
        let (engine, _) = engine(requires);
        let task = task(&engine, Some("feature")).await;
        engine.start_run(&task.id, Trigger::Manual).await;
        let subject = report_done(&engine, &task.id).await;
        let review = review_task(&engine, &subject.id).await;
        assert_eq!(review.agent, "checker");
        let review_run = engine.store.active_run(&review.id).await.unwrap().unwrap();
        engine
            .report(
                &review.id,
                TaskReport {
                    artifacts: Vec::new(),
                    status: Some(RunStatus::Done),
                    message: None,
                    result: Some("independently checked".into()),
                    send_to: None,
                    error: None,
                    token: review_run.token,
                },
            )
            .await
            .unwrap();
        assert_eq!(settled(&engine, &subject.id).await.status, RunStatus::Done);
        let evidence = engine.run_attestations(&subject.id).await.unwrap();
        assert_eq!(
            evidence.iter().map(|a| a.kind).collect::<Vec<_>>(),
            vec![StepKind::Gate, StepKind::Review]
        );
        assert_eq!(
            evidence
                .iter()
                .find(|a| a.kind == StepKind::Review)
                .unwrap()
                .actor,
            "checker"
        );
    }

    #[tokio::test]
    async fn a_rejected_review_carries_findings_and_offers_same_task_rework() {
        let (engine, _) =
            engine("      - { applies_to: [feature], step: review, by: independent }");
        let task = task(&engine, Some("feature")).await;
        engine.start_run(&task.id, Trigger::Manual).await;
        let subject = report_done(&engine, &task.id).await;
        let review = review_task(&engine, &subject.id).await;
        let review_run = engine.store.active_run(&review.id).await.unwrap().unwrap();
        engine
            .report(
                &review.id,
                TaskReport {
                    artifacts: Vec::new(),
                    status: Some(RunStatus::Done),
                    message: None,
                    result: Some("the public API lacks a denial test".into()),
                    send_to: Some("rework".into()),
                    error: None,
                    token: review_run.token,
                },
            )
            .await
            .unwrap();
        let blocked = settled(&engine, &subject.id).await;
        assert_eq!(blocked.status, RunStatus::Blocked);
        let evidence = engine.run_attestations(&subject.id).await.unwrap();
        let rejection = evidence
            .iter()
            .find(|a| a.kind == StepKind::Review)
            .unwrap();
        assert_eq!(rejection.verdict, AttestationVerdict::Fail);
        assert_eq!(
            rejection.findings.as_deref(),
            Some("the public API lacks a denial test")
        );
        engine.accept_rework(&Caller::Owner, &subject.id).await.unwrap();
        for _ in 0..200 {
            if engine.store.active_run(&task.id).await.unwrap().is_some() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("same-task rework did not start");
    }

    #[tokio::test]
    async fn a_terminal_review_without_a_verdict_blocks_and_the_same_task_can_be_retried() {
        let (engine, _) = engine("      - { applies_to: [feature], step: review, by: independent }");
        let task = task(&engine, Some("feature")).await;
        engine.start_run(&task.id, Trigger::Manual).await;
        let subject = report_done(&engine, &task.id).await;
        let review = review_task(&engine, &subject.id).await;
        let first = engine.store.active_run(&review.id).await.unwrap().unwrap();
        engine.fail_run(&first.id, factory_core::run::FailKind::AgentFailed, "review crashed").await;

        let blocked = settled(&engine, &subject.id).await;
        assert_eq!(blocked.status, RunStatus::Blocked);
        let entries = engine.store.run_entries(&subject.id, 100).await.unwrap();
        assert!(entries.iter().any(|entry| entry.kind == "review_unusable"
            && entry.message.contains(&review.id)), "{entries:#?}");

        engine.start_run(&review.id, Trigger::Manual).await;
        let retry = engine.store.active_run(&review.id).await.unwrap().unwrap();
        engine.report(
            &review.id,
            TaskReport {
                artifacts: Vec::new(),
                status: Some(RunStatus::Done),
                message: None,
                result: Some("retry reviewed the same digest".into()),
                send_to: None,
                error: None,
                token: retry.token,
            },
        ).await.unwrap();
        assert_eq!(settled(&engine, &subject.id).await.status, RunStatus::Done);
    }

    #[tokio::test]
    async fn changing_a_dirty_tree_while_review_waits_starts_a_new_digest_bound_round() {
        let requires = "      - { applies_to: [feature], step: tests, gate: \"true\" }\n      - { applies_to: [feature], step: review, by: independent }";
        let (engine, work) = engine(requires);
        let task = task(&engine, Some("feature")).await;
        engine.start_run(&task.id, Trigger::Manual).await;
        let subject = report_done(&engine, &task.id).await;
        let first_review = review_task(&engine, &subject.id).await;
        let first_run = engine.store.active_run(&first_review.id).await.unwrap().unwrap();

        std::fs::write(work.join("changed-during-review.txt"), "new state").unwrap();
        let error = engine.report(
            &first_review.id,
            TaskReport {
                artifacts: Vec::new(),
                status: Some(RunStatus::Done),
                message: None,
                result: Some("reviewed the old state".into()),
                send_to: None,
                error: None,
                token: first_run.token,
            },
        ).await.unwrap_err();
        assert!(error.to_string().contains("worktree changed"), "{error}");

        let second_review = review_task(&engine, &subject.id).await;
        assert_ne!(second_review.id, first_review.id);
        let second_run = engine.store.active_run(&second_review.id).await.unwrap().unwrap();
        engine.report(
            &second_review.id,
            TaskReport {
                artifacts: Vec::new(),
                status: Some(RunStatus::Done),
                message: None,
                result: Some("reviewed the new state".into()),
                send_to: None,
                error: None,
                token: second_run.token,
            },
        ).await.unwrap();
        assert_eq!(settled(&engine, &subject.id).await.status, RunStatus::Done);
        let evidence = engine.run_attestations(&subject.id).await.unwrap();
        assert_eq!(evidence.iter().filter(|item| item.kind == StepKind::Gate).count(), 2);
        assert_eq!(evidence.iter().filter(|item| item.kind == StepKind::Review).count(), 1);
        let final_digest = evidence.iter().find(|item| item.kind == StepKind::Review)
            .and_then(|item| item.worktree_digest.as_deref()).unwrap();
        assert_eq!(
            evidence.iter().rev().find(|item| item.kind == StepKind::Gate)
                .and_then(|item| item.worktree_digest.as_deref()),
            Some(final_digest),
        );
    }

    #[tokio::test]
    async fn done_waits_for_the_gate_blocks_on_its_failure_and_verifies_again_on_the_next_done() {
        let (engine, work) = engine(TESTS_FOR_FEATURES);
        let task = task(&engine, Some("feature")).await;
        engine.start_run(&task.id, Trigger::Manual).await;
        let run = engine.store.active_run(&task.id).await.unwrap().unwrap();
        assert_eq!(run.required_steps.len(), 1, "fixed at dispatch");
        assert_eq!(run.required_steps[0].required_by, vec!["house/tested"]);

        // Nothing built yet: the gate fails, and the run stops rather than fails.
        let verifying = report_done(&engine, &task.id).await;
        assert_eq!(verifying.status, RunStatus::Verifying, "done is not done yet");
        let blocked = settled(&engine, &run.id).await;
        assert_eq!(blocked.status, RunStatus::Blocked);
        assert_eq!(blocked.blocked_source, Some(BlockSource::Verification));
        assert!(blocked.session.is_some(), "the session stays, so the agent can be answered");
        let entries = engine.store.run_entries(&run.id, 100).await.unwrap();
        let reason = entries.iter().rev().find(|e| e.kind == "blocked").expect("a block reason").message.clone();
        assert!(reason.contains("tests exit 1"), "{reason}");
        assert!(reason.contains("house/tested"), "{reason}");

        // Fixed, and reported done again: verified, and done.
        std::fs::write(work.join("built.txt"), "ok").unwrap();
        report_done(&engine, &task.id).await;
        let done = settled(&engine, &run.id).await;
        assert_eq!(done.status, RunStatus::Done);
        assert_eq!(done.result.as_deref(), Some("built it"));

        let attestations = engine.run_attestations(&run.id).await.unwrap();
        let verdicts: Vec<_> = attestations.iter().map(|a| a.verdict).collect();
        assert_eq!(verdicts, vec![AttestationVerdict::Fail, AttestationVerdict::Pass], "append-only: both rounds kept");
        assert!(attestations.iter().all(|a| a.actor == GATE_ACTOR && a.category == "feature"));
        // finish_run publishes the run before updating its task mirror.
        // Await the mirror too, with the same bounded polling as settled().
        for _ in 0..400 {
            if engine.require(&task.id).await.unwrap().status == factory_core::task::TaskStatus::Done {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(engine.require(&task.id).await.unwrap().status, factory_core::task::TaskStatus::Done);
    }

    #[tokio::test]
    async fn the_executor_cannot_accept_its_own_rework_but_an_authorized_other_actor_can() {
        let (engine, _) = engine(TESTS_FOR_FEATURES);
        let task = task(&engine, Some("feature")).await;
        engine.start_run(&task.id, Trigger::Manual).await;
        let failed = report_done(&engine, &task.id).await;
        assert_eq!(settled(&engine, &failed.id).await.status, RunStatus::Blocked);
        let executor = Caller::Agent {
            scope: "demo".into(),
            name: failed.agent.clone(),
            role: factory_core::role::Role::new("custom-approver"),
            run_id: Some(failed.id.clone()),
        };
        let error = engine.accept_rework(&executor, &failed.id).await.unwrap_err();
        assert!(error.to_string().contains("cannot accept rework"), "{error}");
        assert_eq!(engine.require_run(&failed.id).await.unwrap().status, RunStatus::Blocked);
        let checker = Caller::Agent {
            scope: "demo".into(),
            name: "checker".into(),
            role: factory_core::role::Role::new("custom-approver"),
            run_id: None,
        };
        engine.accept_rework(&checker, &failed.id).await.unwrap();
        let entries = engine.store.run_entries(&failed.id, 100).await.unwrap();
        assert!(entries.iter().any(|entry| entry.kind == "rework_accepted"
            && entry.source == "checker"));
    }

    #[tokio::test]
    async fn a_verification_block_is_in_the_inbox_with_its_reason() {
        let (engine, _) = engine(TESTS_FOR_FEATURES);
        let task = task(&engine, Some("feature")).await;
        engine.start_run(&task.id, Trigger::Manual).await;
        let run = report_done(&engine, &task.id).await;
        settled(&engine, &run.id).await;
        let report = engine.operations_report(None, HealthWindow::Week, false).await.unwrap();
        let item = report
            .attention
            .iter()
            .find(|e| e.kind == ExceptionKind::Blocked && e.run_id.as_deref() == Some(run.id.as_str()))
            .expect("the blocked run is in the Inbox");
        assert!(item.reason.contains("verification did not pass"), "{}", item.reason);
    }

    #[tokio::test]
    async fn work_of_a_category_nothing_requires_anything_for_is_done_on_its_report() {
        let (engine, _) = engine(TESTS_FOR_FEATURES);
        let task = task(&engine, Some("docs")).await;
        engine.start_run(&task.id, Trigger::Manual).await;
        let run = report_done(&engine, &task.id).await;
        assert!(run.required_steps.is_empty());
        assert_eq!(run.status, RunStatus::Done);
    }

    #[tokio::test]
    async fn leaving_the_category_out_is_the_default_category_not_a_way_round_the_plan() {
        let (engine, work) = engine("      - { applies_to: [default], step: tests, gate: \"test -f built.txt\" }");
        std::fs::write(work.join("built.txt"), "ok").unwrap();
        let task = task(&engine, None).await;
        engine.start_run(&task.id, Trigger::Manual).await;
        let run = report_done(&engine, &task.id).await;
        assert_eq!(run.status, RunStatus::Verifying);
        assert_eq!(settled(&engine, &run.id).await.status, RunStatus::Done);
    }

    #[tokio::test]
    async fn a_required_gate_with_no_command_blocks_as_missing_evidence() {
        let (engine, _) = engine("      - { applies_to: [feature], step: sbom }");
        let task = task(&engine, Some("feature")).await;
        engine.start_run(&task.id, Trigger::Manual).await;
        let run = report_done(&engine, &task.id).await;
        let blocked = settled(&engine, &run.id).await;
        assert_eq!(blocked.status, RunStatus::Blocked);
        let entries = engine.store.run_entries(&run.id, 100).await.unwrap();
        let reason = &entries.iter().rev().find(|e| e.kind == "blocked").unwrap().message;
        assert!(reason.contains("no evidence for: sbom"), "{reason}");
        assert!(engine.run_attestations(&run.id).await.unwrap().is_empty(), "nothing ran, nothing is attested");
    }

    #[tokio::test]
    async fn while_it_verifies_the_agent_may_not_report_its_way_out() {
        let (engine, _) = engine("      - { applies_to: [feature], step: slow, gate: \"sleep 1\" }");
        let task = task(&engine, Some("feature")).await;
        engine.start_run(&task.id, Trigger::Manual).await;
        let run = report_done(&engine, &task.id).await;
        let error = engine
            .report(
                &task.id,
                TaskReport {
                    artifacts: Vec::new(),
                    status: Some(RunStatus::Done),
                    message: None,
                    result: None,
                    send_to: None,
                    error: None,
                    token: engine.require_run(&run.id).await.unwrap().token,
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("being verified"), "{error}");
        assert_eq!(settled(&engine, &run.id).await.status, RunStatus::Done);
    }

    fn node(id: &str) -> WorkflowNode {
        WorkflowNode {
            session: Default::default(),
            id: id.into(),
            position: CanvasPoint::default(),
            kind: WorkflowNodeKind::Task,
            task: NewTask {
                title: id.into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                runtime: Some("quiet".into()),
                worktree: Some(false),
                ..Default::default()
            },
            gate: None,
            exits: Vec::new(),
            expand: None,
        }
    }

    #[tokio::test]
    async fn workflow_approval_keeps_its_verdict_while_the_subject_runs_or_stays_rejected() {
        async fn started(engine: &Arc<Engine>) -> (factory_core::workflow::WorkflowRun, Task, Run) {
            let definition = engine.create_workflow(WorkflowDraft {
                name: "approval-state".into(),
                scope: "demo".into(),
                category: Some("feature".into()),
                nodes: vec![node("a")],
                ..Default::default()
            }).await.unwrap();
            let workflow = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
            loop {
                let state = engine.workflow_run(&workflow.id).await.unwrap();
                if let Some(task_id) = state.nodes.iter().find(|item| item.node_id == "a")
                    .and_then(|item| item.task_id.as_ref())
                {
                    let task = engine.require(task_id).await.unwrap();
                    if let Some(run) = engine.store.active_run(task_id).await.unwrap() {
                        return (workflow, task, run);
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }

        let requirement = "      - { applies_to: [feature], step: approval, by: person }";
        let (approved_engine, _) = engine(requirement);
        let (workflow, task, held) = started(&approved_engine).await;
        approved_engine.decide_approval(&Caller::Owner, &held.id, AttestationVerdict::Pass, "approved").await.unwrap();
        approved_engine.sync_workflow_for_task(&task.id).await;
        let state = approved_engine.workflow_run(&workflow.id).await.unwrap();
        let approval = state.definition.nodes.iter().find(|item| item.kind == WorkflowNodeKind::Approval).unwrap();
        assert_eq!(state.nodes.iter().find(|item| item.node_id == approval.id).unwrap().status, WorkflowNodeStatus::Done);

        let (rejected_engine, _) = engine(requirement);
        let (workflow, task, held) = started(&rejected_engine).await;
        rejected_engine.decide_approval(&Caller::Owner, &held.id, AttestationVerdict::Fail, "rejected").await.unwrap();
        rejected_engine.sync_workflow_for_task(&task.id).await;
        let state = rejected_engine.workflow_run(&workflow.id).await.unwrap();
        let approval = state.definition.nodes.iter().find(|item| item.kind == WorkflowNodeKind::Approval).unwrap();
        assert_eq!(state.nodes.iter().find(|item| item.node_id == approval.id).unwrap().status, WorkflowNodeStatus::Blocked);
    }

    /// implement (a worktree) -> review, planned as `feature`: a part
    /// workflow (`#235`).
    async fn feature_part_workflow(engine: &Arc<Engine>) -> factory_core::WorkflowDefinition {
        let mut implement = node("implement");
        implement.task.worktree = Some(true);
        engine
            .create_workflow(WorkflowDraft {
                name: "feature-part".into(),
                scope: "demo".into(),
                category: Some("feature".into()),
                part: Some(factory_core::workflow::PartSpec::default()),
                nodes: vec![implement, node("review")],
                edges: vec![WorkflowEdge { id: "ir".into(), from: "implement".into(), to: "review".into() }],
                ..Default::default()
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn lint_shows_a_part_workflow_as_every_part_gets_it() {
        let (engine, _) = engine(TESTS_FOR_FEATURES);
        let template = feature_part_workflow(&engine).await;
        let lint = engine.workflow_lint(Some(template.id.clone()), None, None, None).await.unwrap();
        let part = lint.part.clone().unwrap();
        assert_eq!((part.entry.as_str(), part.deliverable.as_str(), part.terminal.as_str()), ("implement", "implement", "review"));
        let mut gated: Vec<&str> = lint.injections.iter().map(|i| i.node_id.as_str()).collect();
        gated.sort();
        assert_eq!(gated, ["a-implement", "a-review", "b-implement", "b-review"], "every copied task node is injected");
        let injected = lint.injected.unwrap();
        assert!(injected.edges.iter().any(|e| e.from == "a-review.tests" && e.to == "b-implement"), "{:?}", injected.edges);
        injected.validate().unwrap();

        // The Policy tab's workflow enforcement reads the same lint.
        let report = engine.policy_report(Some("demo")).await.unwrap();
        let mut enforced: Vec<&str> = report
            .workflow_enforcement
            .iter()
            .filter(|row| row.workflow == template.id)
            .map(|row| row.node.as_str())
            .collect();
        enforced.sort();
        assert_eq!(enforced, ["a-implement", "a-review", "b-implement", "b-review"]);
    }

    #[tokio::test]
    async fn lint_and_the_policy_tab_hold_a_part_workflows_steps_to_before_rules() {
        let (engine, _) = engine("      - { applies_to: [feature], step: sbom, gate: \"true\", before: publish }");
        let mut publish = node("publish");
        publish.task.worktree = Some(true);
        let template = engine
            .create_workflow(WorkflowDraft {
                name: "release-part".into(),
                scope: "demo".into(),
                category: Some("feature".into()),
                part: Some(factory_core::workflow::PartSpec::default()),
                nodes: vec![publish],
                ..Default::default()
            })
            .await
            .unwrap();
        let lint = engine.workflow_lint(Some(template.id.clone()), None, None, None).await.unwrap();
        assert!(lint.violations.iter().any(|v| v.starts_with("a-publish ")), "{:?}", lint.violations);
        let report = engine.policy_report(Some("demo")).await.unwrap();
        assert!(
            report.workflow_findings.iter().any(|finding| finding.workflow == template.id && finding.detail.starts_with("a-publish ")),
            "{:?}",
            report.workflow_findings
        );
    }

    #[tokio::test]
    async fn a_plan_without_a_template_whose_dependency_is_gated_starts() {
        // Every category gets a tests gate, so the edge into `b` leaves
        // `a`'s gate rather than `a` -- which the expand node's "joined"
        // check refused before #238.
        let (engine, _) = engine("      - { applies_to: [\"*\"], step: tests, gate: \"true\" }");
        let parent = task(&engine, None).await;
        let parts = ["a", "b"].map(|id| factory_core::intake::SplitPart {
            id: id.into(),
            title: id.into(),
            instructions: format!("build {id}"),
            acceptance: Some("true".into()),
            owns: vec![id.into()],
            interface: Some(id.into()),
            estimate_seconds: Some(60),
            depends_on: if id == "b" { vec!["a".into()] } else { Vec::new() },
        });
        let routing = factory_core::intake::Routing { scope: "demo".into(), agent: Some("shell".into()), ..Default::default() };
        let run = engine.start_decomposition_workflow(&parent, &parts, &routing, &Caller::Owner).await.unwrap();
        let into_b: Vec<&str> = run.definition.edges.iter().filter(|e| e.to == "b").map(|e| e.from.as_str()).collect();
        assert_eq!(into_b, ["a.tests"]);
        let expand = run.definition.nodes.iter().find(|n| n.kind == WorkflowNodeKind::Expand).unwrap();
        assert_eq!(expand.expand.as_ref().unwrap().children, ["a", "b"], "the same children as ever");
        let _ = engine.cancel_workflow(&run.id).await;
    }

    #[tokio::test]
    async fn every_copied_task_node_of_a_decomposition_gets_its_locked_gate() {
        let (engine, _) = engine(TESTS_FOR_FEATURES);
        let template = feature_part_workflow(&engine).await;
        let parent = task(&engine, None).await;
        let parts = ["x", "y"].map(|id| factory_core::intake::SplitPart {
            id: id.into(),
            title: id.into(),
            instructions: format!("build {id}"),
            acceptance: Some("true".into()),
            owns: vec![id.into()],
            interface: Some(id.into()),
            estimate_seconds: Some(60),
            depends_on: if id == "y" { vec!["x".into()] } else { Vec::new() },
        });
        let routing = factory_core::intake::Routing {
            scope: "demo".into(),
            workflow: Some(template.id.clone()),
            ..Default::default()
        };
        let run = engine.start_decomposition_workflow(&parent, &parts, &routing, &Caller::Owner).await.unwrap();
        for work in ["x-implement", "x-review", "y-implement", "y-review"] {
            let gate = run.definition.nodes.iter().find(|n| n.id == format!("{work}.tests")).unwrap_or_else(|| panic!("no gate after {work}"));
            assert_eq!(gate.gate.as_ref().unwrap().subject.as_deref(), Some(work));
            assert!(gate.gate.as_ref().unwrap().locked);
        }
        let _ = engine.cancel_workflow(&run.id).await;
    }

    #[tokio::test]
    async fn a_workflow_run_gets_locked_gates_and_a_node_downstream_waits_for_verified_work() {
        let (engine, work) = engine(TESTS_FOR_FEATURES);
        std::fs::write(work.join("built.txt"), "ok").unwrap();
        let definition = engine
            .create_workflow(WorkflowDraft {
                name: "ship".into(),
                scope: "demo".into(),
                category: Some("feature".into()),
                nodes: vec![node("a"), node("b")],
                edges: vec![WorkflowEdge { id: "ab".into(), from: "a".into(), to: "b".into() }],
                ..Default::default()
            })
            .await
            .unwrap();
        let stored_nodes = definition.nodes.len();

        let lint = engine.workflow_lint(Some(definition.id.clone()), None, None, None).await.unwrap();
        assert_eq!(lint.injections.len(), 2, "one gate after each task node: {:#?}", lint.injections);

        let wf = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
        assert_eq!(wf.definition.nodes.len(), 4, "the snapshot carries the gates");
        assert_eq!(engine.workflow_definition(&definition.id).await.unwrap().nodes.len(), stored_nodes, "the stored workflow does not");

        let a_task = loop {
            let run = engine.workflow_run(&wf.id).await.unwrap();
            if let Some(id) = run.nodes.iter().find(|n| n.node_id == "a").and_then(|n| n.task_id.clone()) {
                if engine.store.active_run(&id).await.unwrap().is_some() {
                    break id;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        };
        assert_eq!(engine.require(&a_task).await.unwrap().category.as_deref(), Some("feature"));
        let run = report_done(&engine, &a_task).await;
        assert_eq!(run.required_steps.len(), 1);
        assert_eq!(run.required_steps[0].node_id.as_deref(), Some("a.tests"));
        let wf_now = engine.workflow_run(&wf.id).await.unwrap();
        let waiting_id = wf_now.nodes.iter().find(|n| n.node_id == "b").unwrap().task_id.as_ref().unwrap();
        assert!(engine.require(waiting_id).await.unwrap().after.is_some(), "b exists but is not released on a's word alone");
        assert!(engine.store.active_run(waiting_id).await.unwrap().is_none());
        assert!(engine.dependency_ready_tasks().await.unwrap().is_empty(), "the scheduler cannot bypass a running gate");

        settled(&engine, &run.id).await;
        engine.sync_workflow_for_task(&a_task).await;
        let wf_now = engine.workflow_run(&wf.id).await.unwrap();
        let gate = wf_now.nodes.iter().find(|n| n.node_id == "a.tests").unwrap();
        assert_eq!(gate.status, WorkflowNodeStatus::Done, "the gate mirrors its attestation");
        assert!(gate.task_id.is_none(), "a gate never spawns a task");
        assert!(wf_now.nodes.iter().find(|n| n.node_id == "b").unwrap().task_id.is_some(), "b starts on verified work");
    }

    #[tokio::test]
    async fn accepting_a_failed_workflow_gate_sends_the_subject_back_with_findings() {
        let (engine, _) = engine(TESTS_FOR_FEATURES);
        let definition = engine
            .create_workflow(WorkflowDraft {
                name: "gate-rework".into(),
                scope: "demo".into(),
                category: Some("feature".into()),
                nodes: vec![node("a")],
                ..Default::default()
            })
            .await
            .unwrap();
        let wf = engine
            .start_workflow(&definition.id, Default::default(), &Caller::Owner)
            .await
            .unwrap();
        let first_task = loop {
            let run = engine.workflow_run(&wf.id).await.unwrap();
            if let Some(id) = run.nodes.iter().find(|n| n.node_id == "a").and_then(|n| n.task_id.clone()) {
                if engine.store.active_run(&id).await.unwrap().is_some() {
                    break id;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        };
        let failed = report_done(&engine, &first_task).await;
        assert_eq!(settled(&engine, &failed.id).await.status, RunStatus::Blocked);

        engine.accept_rework(&Caller::Owner, &failed.id).await.unwrap();
        for _ in 0..400 {
            let run = engine.workflow_run(&wf.id).await.unwrap();
            let node = run.nodes.iter().find(|n| n.node_id == "a").unwrap();
            if node.task_id.as_deref() == Some(first_task.as_str()) {
                // Admission reserves the run row before patching round/feedback
                // and launching it. Observe the launched retry, not that
                // transient Dispatching row with default orchestration fields.
                // QuietRuntime does not report an acknowledgement, so session
                // attachment (not Running status) proves startup reached it.
                if let Some(retry) = engine.store.active_run(&first_task).await.unwrap()
                    .filter(|run| run.attempt == 2 && run.session.is_some()) {
                    assert_eq!(node.round, 1);
                    assert_eq!(retry.workflow_round, 1);
                    assert!(retry.feedback.as_ref().unwrap().feedback.as_deref().unwrap().contains("tests exit 1"));
                    return;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("failed gate did not start the bounded workflow rework round");
    }

    #[tokio::test]
    async fn exhausted_workflow_rework_leaves_the_subject_blocked_for_a_person() {
        let (engine, _) = engine(TESTS_FOR_FEATURES);
        let definition = engine
            .create_workflow(WorkflowDraft {
                name: "exhausted-gate-rework".into(),
                scope: "demo".into(),
                category: Some("feature".into()),
                nodes: vec![node("a")],
                ..Default::default()
            })
            .await
            .unwrap();
        let wf = engine
            .start_workflow(&definition.id, Default::default(), &Caller::Owner)
            .await
            .unwrap();
        let task_id = loop {
            let run = engine.workflow_run(&wf.id).await.unwrap();
            if let Some(id) = run
                .nodes
                .iter()
                .find(|n| n.node_id == "a")
                .and_then(|n| n.task_id.clone())
            {
                if engine.store.active_run(&id).await.unwrap().is_some() {
                    break id;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        };
        let failed = report_done(&engine, &task_id).await;
        assert_eq!(settled(&engine, &failed.id).await.status, RunStatus::Blocked);

        let edit = engine.workflow_edit.lock().await;
        let mut exhausted = engine.workflow_run(&wf.id).await.unwrap();
        let gate = exhausted
            .nodes
            .iter_mut()
            .find(|n| n.node_id == "a.tests")
            .unwrap();
        gate.round = 5;
        engine.workflows.put_run(&exhausted).await.unwrap();
        drop(edit);

        let error = engine.accept_rework(&Caller::Owner, &failed.id).await.unwrap_err();
        assert!(error.to_string().contains("exhausted"), "{error}");
        let still_blocked = engine.require_run(&failed.id).await.unwrap();
        assert_eq!(still_blocked.status, RunStatus::Blocked);
        assert_eq!(still_blocked.blocked_source, Some(BlockSource::Verification));
        assert!(still_blocked.error.is_none(), "the blocked subject was not consumed");
        let unchanged = engine.workflow_run(&wf.id).await.unwrap();
        let subject = unchanged.nodes.iter().find(|n| n.node_id == "a").unwrap();
        assert_eq!(subject.task_id.as_deref(), Some(task_id.as_str()));
        assert!(subject.superseded_task_ids.is_empty());
    }

    // -- attested_runs (#158) ------------------------------------------------

    fn provenance_git(work: &Path) {
        std::fs::write(work.join(".gitignore"), "ignored-artifact.bin\n").unwrap();
        std::fs::write(work.join("source.txt"), "source v1").unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["add", "."],
            vec![
                "-c",
                "user.name=Factory QA",
                "-c",
                "user.email=qa@example.invalid",
                "commit",
                "-qm",
                "source",
            ],
        ] {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(work)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    fn provenance_engine(requires: &str, spawn: bool) -> (Arc<Engine>, PathBuf) {
        let (mut engine, work) = engine_with_verifier(requires, false);
        let db = engine
            .factory_snapshot()
            .root
            .join(".factory/provenance-test.sqlite");
        Arc::get_mut(&mut engine).unwrap().policies =
            crate::policies::PolicyStore::open(&db).unwrap();
        if spawn {
            engine.spawn_verifier();
        }
        (engine, work)
    }

    async fn provenance_done(engine: &Arc<Engine>, task: &Task, paths: &[&str]) -> Result<Run> {
        let run = engine.store.active_run(&task.id).await?.unwrap();
        engine
            .report(
                &task.id,
                TaskReport {
                    artifacts: paths.iter().map(|s| s.to_string()).collect(),
                    status: Some(RunStatus::Done),
                    message: None,
                    result: Some("built release".into()),
                    send_to: None,
                    error: None,
                    token: run.token,
                },
            )
            .await
    }

    #[tokio::test]
    async fn provenance_captures_real_bytes_and_frozen_evidence_after_verification() {
        use sha2::Digest;
        let (engine, work) = provenance_engine(
            "      - { applies_to: [feature], step: tests, gate: \"true\" }",
            false,
        );
        provenance_git(&work);
        std::fs::write(work.join("release.bin"), b"release bytes\0\xff").unwrap();
        let task = task(&engine, Some("feature")).await;
        engine.start_run(&task.id, Trigger::Manual).await;
        let held = provenance_done(&engine, &task, &["release.bin"])
            .await
            .unwrap();
        assert_eq!(held.status, RunStatus::Verifying);
        assert!(engine.run_provenance(&held.id).await.unwrap().is_empty());
        assert!(engine
            .policies
            .provenance(&held.id)
            .await
            .unwrap()
            .is_empty());
        engine.verify_run(&held.id).await.unwrap();
        let done = engine.require_run(&held.id).await.unwrap();
        assert_eq!(done.status, RunStatus::Done);
        let records = crate::facts::Facts::<factory_kernel::L5>::new(&engine)
            .get::<factory_kernel::ArtifactProvenance>(&held.id)
            .await
            .unwrap();
        assert_eq!(records.len(), 1);
        let record = &records[0];
        let digest = format!("{:x}", sha2::Sha256::digest(b"release bytes\0\xff"));
        assert_eq!(record.artifact.sha256, digest);
        assert_eq!(
            std::fs::read(
                engine
                    .factory_snapshot()
                    .root
                    .join(&record.artifact.storage_path)
            )
            .unwrap(),
            b"release bytes\0\xff"
        );
        assert_eq!(
            record.statement.predicate.evidence.required_steps,
            held.required_steps
        );
        assert_eq!(record.statement.predicate.evidence.agent, held.agent);
        assert_eq!(record.statement.predicate.evidence.attestations.len(), 1);
        assert_eq!(
            record.statement.predicate.evidence.attestations[0]
                .worktree_digest
                .as_deref(),
            Some(record.artifact.source.worktree_digest.as_str())
        );
        assert_eq!(
            record.statement.predicate.run_details.metadata.finished_on,
            done.ended_at.unwrap()
        );
        assert!(!serde_json::to_string(record)
            .unwrap()
            .contains(held.token.as_deref().unwrap()));
        // Later task edits and file changes never rewrite historical evidence.
        std::fs::write(work.join("source.txt"), "source v2").unwrap();
        std::fs::write(work.join("release.bin"), "new output").unwrap();
        assert_eq!(engine.run_provenance(&held.id).await.unwrap(), records);
        engine.policies.append_provenance(record).await.unwrap();
        let mut changed = record.clone();
        changed.artifact.sha256 = "0".repeat(64);
        assert!(engine.policies.append_provenance(&changed).await.is_err());
        assert_eq!(engine.run_provenance(&held.id).await.unwrap(), records);
        let reopened = crate::policies::PolicyStore::open(
            &engine
                .factory_snapshot()
                .root
                .join(".factory/provenance-test.sqlite"),
        )
        .unwrap();
        assert_eq!(reopened.provenance(&held.id).await.unwrap(), records);
    }

    #[tokio::test]
    async fn provenance_changed_source_blocks_and_recapture_can_finish() {
        let (engine, work) = provenance_engine(
            "      - { applies_to: [feature], step: tests, gate: \"true\" }",
            false,
        );
        provenance_git(&work);
        std::fs::write(work.join("release.bin"), "v1").unwrap();
        let task = task(&engine, Some("feature")).await;
        engine.start_run(&task.id, Trigger::Manual).await;
        let held = provenance_done(&engine, &task, &["release.bin"])
            .await
            .unwrap();
        std::fs::write(work.join("source.txt"), "changed").unwrap();
        assert!(engine
            .verify_run(&held.id)
            .await
            .unwrap_err()
            .to_string()
            .contains("source worktree changed"));
        let blocked = engine.require_run(&held.id).await.unwrap();
        assert_eq!(blocked.status, RunStatus::Blocked);
        assert_eq!(blocked.blocked_source, Some(BlockSource::Verification));
        assert!(blocked.token.is_some());
        assert!(
            blocked.session.is_some(),
            "a publication hold keeps the session available for repair"
        );
        assert!(engine.run_provenance(&held.id).await.unwrap().is_empty());
        assert!(engine
            .store
            .run_entries(&held.id, 100)
            .await
            .unwrap()
            .iter()
            .any(|e| e.kind == "blocked" && e.message.contains("artifact provenance")));
        let retry = provenance_done(&engine, &task, &["release.bin"])
            .await
            .unwrap();
        assert_ne!(retry.artifacts[0].id, held.artifacts[0].id);
        engine.verify_run(&held.id).await.unwrap();
        assert_eq!(
            engine.require_run(&held.id).await.unwrap().status,
            RunStatus::Done
        );
        assert_eq!(
            engine.run_provenance(&held.id).await.unwrap()[0].id,
            retry.artifacts[0].id
        );
    }

    #[tokio::test]
    async fn provenance_detects_ignored_output_and_stored_copy_tampering() {
        for stored in [false, true] {
            let (engine, work) = provenance_engine(
                "      - { applies_to: [feature], step: tests, gate: \"true\" }",
                false,
            );
            provenance_git(&work);
            std::fs::write(work.join("ignored-artifact.bin"), "v1").unwrap();
            let task = task(&engine, Some("feature")).await;
            engine.start_run(&task.id, Trigger::Manual).await;
            let held = provenance_done(&engine, &task, &["ignored-artifact.bin"])
                .await
                .unwrap();
            let changed = if stored {
                engine
                    .factory_snapshot()
                    .root
                    .join(&held.artifacts[0].storage_path)
            } else {
                work.join("ignored-artifact.bin")
            };
            std::fs::write(changed, "tampered bytes").unwrap();
            assert!(engine
                .verify_run(&held.id)
                .await
                .unwrap_err()
                .to_string()
                .contains("artifact bytes changed"));
            assert_eq!(
                engine.require_run(&held.id).await.unwrap().status,
                RunStatus::Blocked
            );
            assert!(engine
                .policies
                .provenance(&held.id)
                .await
                .unwrap()
                .is_empty());
        }
    }

    #[tokio::test]
    async fn provenance_refuses_unsafe_paths_bad_reports_and_non_git_sources() {
        let (engine, work) = provenance_engine(TESTS_FOR_FEATURES, false);
        std::fs::write(work.join("release.bin"), "v1").unwrap();
        let task = task(&engine, Some("docs")).await;
        engine.start_run(&task.id, Trigger::Manual).await;
        assert!(provenance_done(&engine, &task, &["release.bin"])
            .await
            .unwrap_err()
            .to_string()
            .contains("readable Git"));
        provenance_git(&work);
        std::fs::write(work.parent().unwrap().join("outside.bin"), "private").unwrap();
        std::fs::create_dir_all(work.join(".factory")).unwrap();
        std::fs::write(work.join(".factory/private"), "private").unwrap();
        for paths in [
            vec!["../outside.bin"],
            vec![".git/HEAD"],
            vec![".factory/private"],
            vec!["missing"],
            vec!["release.bin", "release.bin"],
            vec!["release.bin"; 17],
        ] {
            assert!(
                provenance_done(&engine, &task, &paths).await.is_err(),
                "{paths:?}"
            );
        }
        let active = engine.store.active_run(&task.id).await.unwrap().unwrap();
        assert!(matches!(
            active.status,
            RunStatus::Dispatching | RunStatus::Running
        ));
        assert!(engine
            .report(
                &task.id,
                TaskReport {
                    artifacts: vec!["release.bin".into()],
                    status: Some(RunStatus::Running),
                    token: active.token.clone(),
                    message: None,
                    result: None,
                    send_to: None,
                    error: None
                }
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("done report"));
        assert!(engine
            .report(
                &task.id,
                TaskReport {
                    artifacts: vec!["release.bin".into()],
                    status: Some(RunStatus::Done),
                    token: Some("wrong".into()),
                    message: None,
                    result: None,
                    send_to: None,
                    error: None
                }
            )
            .await
            .is_err());
        let done = provenance_done(&engine, &task, &["release.bin"])
            .await
            .unwrap();
        assert_eq!(done.status, RunStatus::Done);
        assert_eq!(engine.run_provenance(&done.id).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn provenance_keeps_pre_dispatch_approval_and_digest_bound_independent_review() {
        let requires = "      - { applies_to: [feature], step: approval }\n      - { applies_to: [feature], step: tests, gate: \"true\" }\n      - { applies_to: [feature], step: review, by: independent }";
        let (engine, work) = provenance_engine(requires, true);
        provenance_git(&work);
        let task = task(&engine, Some("feature")).await;
        engine.start_run(&task.id, Trigger::Manual).await;
        let held = engine.store.active_run(&task.id).await.unwrap().unwrap();
        assert_eq!(held.status, RunStatus::Blocked);
        engine
            .decide_approval(
                &Caller::Owner,
                &held.id,
                AttestationVerdict::Pass,
                "authorized build",
            )
            .await
            .unwrap();
        for _ in 0..200 {
            if engine.require_run(&held.id).await.unwrap().status == RunStatus::Running {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        std::fs::write(work.join("release.bin"), "approved build output").unwrap();
        let subject = provenance_done(&engine, &task, &["release.bin"])
            .await
            .unwrap();
        let review = review_task(&engine, &subject.id).await;
        assert!(engine.run_provenance(&subject.id).await.unwrap().is_empty());
        report_done(&engine, &review.id).await;
        let done = settled(&engine, &subject.id).await;
        assert_eq!(done.status, RunStatus::Done);
        let record = &engine.run_provenance(&subject.id).await.unwrap()[0];
        let evidence = &record.statement.predicate.evidence;
        assert_eq!(evidence.required_steps, subject.required_steps);
        assert_eq!(evidence.attestations.len(), 3);
        let approval = evidence
            .attestations
            .iter()
            .find(|a| a.kind == StepKind::Approval)
            .unwrap();
        assert_ne!(
            approval.worktree_digest.as_deref(),
            Some(record.artifact.source.worktree_digest.as_str())
        );
        for item in evidence
            .attestations
            .iter()
            .filter(|a| a.kind != StepKind::Approval)
        {
            assert_eq!(
                item.worktree_digest.as_deref(),
                Some(record.artifact.source.worktree_digest.as_str())
            );
            assert_ne!(item.actor, subject.agent);
        }
    }

    #[tokio::test]
    async fn provenance_requires_current_independent_matching_functionary_evidence() {
        let (engine, work) = provenance_engine(
            "      - { applies_to: [feature], step: review, by: independent }",
            false,
        );
        provenance_git(&work);
        std::fs::write(work.join("release.bin"), "output").unwrap();
        let task = task(&engine, Some("feature")).await;
        engine.start_run(&task.id, Trigger::Manual).await;
        let held = provenance_done(&engine, &task, &["release.bin"])
            .await
            .unwrap();
        assert!(engine.validate_artifacts(&held).await.is_err());
        let step = &held.required_steps[0];
        let mut evidence: StepAttestation = serde_json::from_value(serde_json::json!({
            "id": "self", "run_id": held.id, "task_id": task.id,
            "scope": "demo", "category": "feature", "step": step.step,
            "kind": "review", "actor": held.agent, "verdict": "pass", "dir": work,
            "worktree_digest": held.artifacts[0].source.worktree_digest, "at": Utc::now()
        }))
        .unwrap();
        engine
            .policies
            .append_step_attestation(&evidence)
            .await
            .unwrap();
        assert!(engine.validate_artifacts(&held).await.is_err());
        evidence.id = "foreign".into();
        evidence.actor = "not-the-frozen-reviewer".into();
        engine
            .policies
            .append_step_attestation(&evidence)
            .await
            .unwrap();
        assert!(engine.validate_artifacts(&held).await.is_err());
        evidence.id = "stale".into();
        evidence.actor = step.actor.clone().unwrap();
        evidence.worktree_digest = Some("old-source".into());
        engine
            .policies
            .append_step_attestation(&evidence)
            .await
            .unwrap();
        assert!(engine.validate_artifacts(&held).await.is_err());
        evidence.id = "current".into();
        evidence.worktree_digest = Some(held.artifacts[0].source.worktree_digest.clone());
        evidence.at = Utc::now();
        engine
            .policies
            .append_step_attestation(&evidence)
            .await
            .unwrap();
        assert!(engine.validate_artifacts(&held).await.is_ok());
        evidence.id = "rejected".into();
        evidence.verdict = AttestationVerdict::Fail;
        evidence.at += chrono::Duration::seconds(1);
        engine
            .policies
            .append_step_attestation(&evidence)
            .await
            .unwrap();
        assert!(engine.validate_artifacts(&held).await.is_err());
        assert!(engine.run_provenance(&held.id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn provenance_partial_publication_stays_hidden_and_cancellation_cannot_be_overwritten() {
        let (engine, work) = provenance_engine(TESTS_FOR_FEATURES, false);
        provenance_git(&work);
        std::fs::write(work.join("release.bin"), "output").unwrap();
        let task = task(&engine, Some("docs")).await;
        engine.start_run(&task.id, Trigger::Manual).await;
        let mut run = engine.store.active_run(&task.id).await.unwrap().unwrap();
        run.artifacts = engine
            .capture_artifacts(&run, &task, &["release.bin".into()])
            .await
            .unwrap();
        engine
            .store
            .update_run(
                &run.id,
                &RunPatch {
                    artifacts: Some(run.artifacts.clone()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let record =
            factory_core::provenance::statement(&run, &run.artifacts[0], &[], "test", Utc::now());
        engine.policies.append_provenance(&record).await.unwrap();
        assert_eq!(engine.policies.provenance(&run.id).await.unwrap().len(), 1);
        assert!(engine.run_provenance(&run.id).await.unwrap().is_empty());
        engine
            .cancel_task_run(
                &task.id,
                Some(&run.id),
                factory_core::run::FailKind::CancelledByPerson,
            )
            .await
            .unwrap();
        assert!(engine
            .finish_run(
                &run.id,
                RunStatus::Done,
                RunPatch::default(),
                "late completion"
            )
            .await
            .is_err());
        assert_eq!(
            engine.require_run(&run.id).await.unwrap().status,
            RunStatus::Cancelled
        );
        assert!(engine.run_provenance(&run.id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn attested_runs_narrows_by_scope_category_and_window_and_excludes_bench_origin() {
        let (engine, work) = engine(TESTS_FOR_FEATURES);
        std::fs::write(work.join("built.txt"), "ok").unwrap();

        // A feature-category run that is held to `tests` and passes it.
        let feature_task = task(&engine, Some("feature")).await;
        engine.start_run(&feature_task.id, Trigger::Manual).await;
        let run = report_done(&engine, &feature_task.id).await;
        let feature_run = settled(&engine, &run.id).await;
        assert_eq!(feature_run.status, RunStatus::Done);

        // A docs-category run: this catalogue requires nothing of `docs`, so
        // it is never held to anything and finishes without verification.
        let docs_task = task(&engine, Some("docs")).await;
        engine.start_run(&docs_task.id, Trigger::Manual).await;
        let docs_run = report_done(&engine, &docs_task.id).await;
        assert_eq!(docs_run.status, RunStatus::Done);

        // A bench-origin task's run, built directly through the store --
        // `required_steps_for_task` never plans one (`#118`), so nothing in
        // the ordinary dispatch path can produce one to exercise here.
        let now = Utc::now();
        let bench_task = {
            let new = NewTask {
                title: "bench case".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                runtime: Some("quiet".into()),
                worktree: Some(false),
                category: Some("feature".into()),
                ..Default::default()
            };
            let mut t = task_from_new(new, "demo".into(), "shell".into(), "quiet".into());
            t.bench_origin = Some(BenchOrigin {
                bench_run_id: "br1".into(),
                case_id: "c1".into(),
                agent: "shell".into(),
                attempt: 1,
            }.into());
            engine.store.create(&t).await.unwrap()
        };
        let bench_run = engine
            .store
            .create_run(&NewRun {
                task_id: bench_task.id.clone(),
                trigger: Trigger::Manual,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: "quiet".into(),
                token: "tok".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        engine
            .store
            .update_run(
                &bench_run.id,
                &RunPatch {
                    status: Some(RunStatus::Done),
                    ended_at: Some(now),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let window = Window {
            from: now - chrono::Duration::days(1),
            to: now + chrono::Duration::minutes(1),
        };
        let all = engine.attested_runs(None, None, window).await.unwrap();
        let run_ids: BTreeSet<&str> = all.iter().map(|r| r.run_id.as_str()).collect();
        assert!(run_ids.contains(feature_run.id.as_str()));
        assert!(run_ids.contains(docs_run.id.as_str()));
        assert!(
            !run_ids.contains(bench_run.id.as_str()),
            "bench-origin runs are excluded"
        );

        // Category narrows to the one feature run, with its attestation.
        let feature_only = engine
            .attested_runs(None, Some(&BTreeSet::from(["feature".to_string()])), window)
            .await
            .unwrap();
        assert_eq!(feature_only.len(), 1);
        assert_eq!(feature_only[0].run_id, feature_run.id);
        assert_eq!(feature_only[0].category, "feature");
        assert_eq!(feature_only[0].attestations.len(), 1, "grouped per run");
        assert_eq!(
            feature_only[0].attestations[0].verdict,
            AttestationVerdict::Pass
        );
        assert_eq!(feature_only[0].attestations[0].step, "tests");

        // Out-of-window: a run ending exactly at `from` is outside `(from, to]`.
        let exact_from = Window {
            from: feature_run.ended_at.unwrap(),
            to: feature_run.ended_at.unwrap(),
        };
        assert!(engine
            .attested_runs(None, None, exact_from)
            .await
            .unwrap()
            .is_empty());
        let too_early = Window {
            from: now - chrono::Duration::days(30),
            to: now - chrono::Duration::days(20),
        };
        assert!(engine
            .attested_runs(None, None, too_early)
            .await
            .unwrap()
            .is_empty());

        // Scope filter: a scope name nothing here canonicalises to excludes
        // everything; the fixture's own scope keeps the two non-bench runs.
        let other_scope = engine
            .attested_runs(Some(&BTreeSet::from(["other".to_string()])), None, window)
            .await
            .unwrap();
        assert!(other_scope.is_empty());
        let demo_scope = engine
            .attested_runs(Some(&BTreeSet::from(["demo".to_string()])), None, window)
            .await
            .unwrap();
        assert_eq!(demo_scope.len(), 2);
    }
}
