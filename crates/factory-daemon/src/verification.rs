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
use factory_core::control_plan::{
    self, AttestationVerdict, ControlPlan, RequiredStep, StepAttestation, StepKind, GATE_ACTOR,
};
use factory_core::error::{FactoryError, Result};
use factory_core::event::Event;
use factory_core::policy;
use factory_core::quality;
use factory_core::run::{BlockSource, Run, RunPatch, RunStatus};
use factory_core::task::{NewTask, Task, TaskEntry, TaskFilter, TaskStatus, WorkflowOrigin};
use factory_core::workflow::{
    WorkflowDefinition, WorkflowLint, WorkflowNodeKind, WorkflowNodeStatus, IMPLICIT_NODE,
};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

/// Ten minutes -- a gate's timeout when its requirement names none, the same
/// fallback the bench gate runner has always used.
pub(crate) const DEFAULT_GATE_TIMEOUT_SECS: u64 = 600;
pub(crate) const REVIEW_RUN_LABEL: &str = "factory.review_run";
pub(crate) const REVIEW_STEP_LABEL: &str = "factory.review_step";

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

/// The commit `dir` is at and whether its tree has uncommitted changes --
/// what an attestation says it judged. `(None, None)` outside a git tree.
async fn git_state(dir: &Path) -> (Option<String>, Option<bool>) {
    let head = tokio::process::Command::new("git").arg("-C").arg(dir).args(["rev-parse", "HEAD"]).output().await;
    let commit = match head {
        Ok(out) if out.status.success() => Some(String::from_utf8_lossy(&out.stdout).trim().to_string()),
        _ => return (None, None),
    };
    let status = tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["status", "--porcelain"])
        .output()
        .await;
    let dirty = match status {
        Ok(out) if out.status.success() => Some(!out.stdout.is_empty()),
        _ => None,
    };
    (commit, dirty)
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
        let (commit, dirty) = git_state(Path::new(&dir)).await;
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
            commit,
            dirty,
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
                )
                .await
            {
                Ok(run) => run,
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
        if run.status == RunStatus::Verifying
            || (run.status == RunStatus::Blocked
                && run.blocked_source == Some(BlockSource::Verification))
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
    pub(crate) async fn accept_rework(self: &Arc<Self>, run_id: &str) -> Result<Run> {
        let run = self.require_run(run_id).await?;
        if run.status != RunStatus::Blocked || run.blocked_source != Some(BlockSource::Verification)
        {
            return Err(FactoryError::BadRequest(
                "only a run blocked by verification has rework to accept".into(),
            ));
        }
        let task = self.require(&run.task_id).await?;
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
            // carry. Freeze its concrete finding into the next round's task
            // template; review findings travel through the review task.
            if failed.is_some_and(|a| a.kind == StepKind::Gate) {
                if let Some(node) = workflow.definition.nodes.iter_mut().find(|n| n.id == origin.node_id) {
                    node.task.instructions = format!(
                        "{}\n\nRework requested from run {}:\n{}",
                        node.task.instructions.trim_end(), run.id, finding
                    );
                }
            }
            match workflow.send_back(&from, &origin.node_id) {
                factory_core::workflow::SendBack::Sent { .. } => Some(workflow),
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
                "owner",
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
        if task.bench_origin.is_some() || task.labels.contains_key(REVIEW_RUN_LABEL) {
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

    fn enqueue_verification(&self, run_id: &str) {
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

    /// Runs still `verifying` when the daemon last stopped are verified
    /// again from the start -- their gates run anew; a half-finished round
    /// is never judged.
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
    ) -> Result<bool> {
        let attestations = self.policies.step_attestations(&subject.id).await?;
        let all_tasks = self.store.list(&TaskFilter::default()).await?;
        let (commit, dirty) = git_state(dir).await;
        let mut waiting = false;
        for step in subject
            .required_steps
            .iter()
            .filter(|s| s.kind == StepKind::Review)
        {
            if attestations
                .iter()
                .any(|a| a.step == step.step && a.kind == StepKind::Review)
            {
                continue;
            }
            let Some(actor) = step.actor.clone() else {
                continue; // judge names the missing independent functionary
            };
            if actor == subject.agent {
                continue; // never manufacture self-review
            }
            if let Some(existing) = all_tasks.iter().find(|candidate| {
                candidate.labels.get(REVIEW_RUN_LABEL) == Some(&subject.id)
                    && candidate.labels.get(REVIEW_STEP_LABEL) == Some(&step.step)
            }) {
                if existing.status == TaskStatus::Done {
                    if let Some(review_run) =
                        self.store.runs(&existing.id, 1).await?.into_iter().next()
                    {
                        self.record_review_attestation(existing, &review_run, subject, step)
                            .await?;
                        continue;
                    }
                }
                waiting = true;
                continue;
            }

            let mut labels = std::collections::BTreeMap::new();
            labels.insert(REVIEW_RUN_LABEL.into(), subject.id.clone());
            labels.insert(REVIEW_STEP_LABEL.into(), step.step.clone());
            let instructions = format!(
                "Independently review task {} (run {}).\n\nSubject result:\n{}\n\nWorktree: {}\nCommit: {}\nDirty: {}\n\nReport plain done to pass. Report done --send-to {} with --result containing concrete findings to reject and propose rework.",
                task.id,
                subject.id,
                subject.result.as_deref().unwrap_or("(no result supplied)"),
                dir.display(),
                commit.as_deref().unwrap_or("unknown"),
                dirty.map(|v| v.to_string()).unwrap_or_else(|| "unknown".into()),
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
            let review =
                if let (Some(origin), Some(node_id)) = (&task.workflow_origin, &step.node_id) {
                    let review = self
                        .create_workflow_task(
                            new,
                            WorkflowOrigin {
                                workflow_id: origin.workflow_id.clone(),
                                workflow_run_id: origin.workflow_run_id.clone(),
                                node_id: node_id.clone(),
                            },
                            review_id,
                        )
                        .await?;
                    let mut workflow = self.workflow_run(&origin.workflow_run_id).await?;
                    if let Some(node) = workflow.nodes.iter_mut().find(|n| n.node_id == *node_id) {
                        node.task_id = Some(review.id.clone());
                        node.status = WorkflowNodeStatus::Pending;
                    }
                    self.workflows.put_run(&workflow).await?;
                    self.bus
                        .publish(Event::WorkflowRunUpdated { run: workflow });
                    review
                } else {
                    self.create(new).await?
                };
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
        let (commit, dirty) = git_state(Path::new(&dir)).await;
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
            commit,
            dirty,
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
        let subject = self.require_run(subject_id).await?;
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
        self.record_review_attestation(review_task, review_run, &subject, step)
            .await?;
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
        let started = Utc::now();
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
        let (commit, dirty) = git_state(&dir).await;

        for step in run.required_steps.iter().filter(|s| s.kind.enforced()) {
            if step.kind != StepKind::Gate {
                continue;
            }
            let Some(command) = step.command.as_deref() else {
                continue; // nothing to run: `judge` reports it missing
            };
            let timeout = step.timeout_seconds.unwrap_or(DEFAULT_GATE_TIMEOUT_SECS);
            let (exit_code, output) = run_shell_capture(&dir, command, timeout).await;
            let verdict = if exit_code == Some(0) { AttestationVerdict::Pass } else { AttestationVerdict::Fail };
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
                commit: commit.clone(),
                dirty,
                node_id: step.node_id.clone(),
                at: Utc::now(),
            };
            self.policies.append_step_attestation(&attestation).await?;
            let code = exit_code.map(|c| format!("exit {c}")).unwrap_or_else(|| "did not finish".into());
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

        // Deterministic gates pass before a model review is ever spent. The
        // subject remains `verifying`; the review task's report wakes this
        // same coordinator, and labels make restart recovery idempotent.
        let gate_attestations = self.policies.step_attestations(&run.id).await?;
        let gates = control_plan::judge(
            &run.required_steps
                .iter()
                .filter(|s| s.kind == StepKind::Gate)
                .cloned()
                .collect::<Vec<_>>(),
            &gate_attestations,
            &run.agent,
            started,
        );
        if gates.passed && self.ensure_review_tasks(&run, &task, &dir).await? {
            self.release_verification(run_id);
            self.sync_workflow_for_task(&task.id).await;
            return Ok(());
        }

        let attestations = self.policies.step_attestations(&run.id).await?;
        let verdict = control_plan::judge(&run.required_steps, &attestations, &run.agent, started);
        // Free the run for its next `done` before settling it: once it reads
        // `blocked`, a report may re-enqueue it at any moment.
        self.release_verification(run_id);
        let run = self.require_run(run_id).await?;
        if run.status != RunStatus::Verifying {
            return Ok(());
        }
        if verdict.passed {
            self.entry(
                &task.id,
                TaskEntry::new("daemon", "verified", "every required step attested and passed").in_run(&run.id),
            )
            .await;
            self.close_session(&run).await;
            self.finish_run(
                &run.id,
                RunStatus::Done,
                RunPatch { status: Some(RunStatus::Done), ..Default::default() },
                "verified",
            )
            .await?;
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
            });
        };
        definition.validate().map_err(FactoryError::BadRequest)?;
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
            subject: definition.id.clone(),
            scope: definition.scope.clone(),
            plans: plans.into_values().collect(),
            injections,
            violations,
            injected: Some(injected),
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
    use factory_core::adapter::{AgentRuntime, StartRequest};
    use factory_core::config::{Config, DaemonConfig, Factory, Instance, PolicyDeclaration, Scope};
    use factory_core::operations::{ExceptionKind, HealthWindow};
    use factory_core::run::Trigger;
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
                    provider: None,
                }],
                runtime: Some("quiet".into()),
                git: None,
                task_store: None,
                roles: Default::default(),
                policies: Default::default(),
                quality: Default::default(),
                dependencies: Default::default(),
            }],
            infrastructure: Default::default(),
            plugins_dir: None,
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
        let (engine, _) = engine("      - { applies_to: [feature], step: approval, by: person }");
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
            vec![StepKind::Gate, StepKind::Review, StepKind::Gate]
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
        engine.accept_rework(&subject.id).await.unwrap();
        for _ in 0..200 {
            if engine.store.active_run(&task.id).await.unwrap().is_some() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("same-task rework did not start");
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
        assert_eq!(engine.require(&task.id).await.unwrap().status, factory_core::task::TaskStatus::Done);
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
        }
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
        assert!(
            wf_now.nodes.iter().find(|n| n.node_id == "b").unwrap().task_id.is_none(),
            "b does not start on a's word alone"
        );

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

        engine.accept_rework(&failed.id).await.unwrap();
        for _ in 0..400 {
            let run = engine.workflow_run(&wf.id).await.unwrap();
            let node = run.nodes.iter().find(|n| n.node_id == "a").unwrap();
            if let Some(id) = node.task_id.as_deref().filter(|id| *id != first_task) {
                let retry = engine.require(id).await.unwrap();
                assert_eq!(node.round, 1);
                assert!(retry.instructions.contains("tests exit 1"), "{}", retry.instructions);
                return;
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

        let mut exhausted = engine.workflow_run(&wf.id).await.unwrap();
        let gate = exhausted
            .nodes
            .iter_mut()
            .find(|n| n.node_id == "a.tests")
            .unwrap();
        gate.round = 5;
        engine.workflows.put_run(&exhausted).await.unwrap();

        let error = engine.accept_rework(&failed.id).await.unwrap_err();
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
}
