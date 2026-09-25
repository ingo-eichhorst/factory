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
    self, AttestationVerdict, ControlPlan, RequiredStep, StepAttestation, GATE_ACTOR,
};
use factory_core::error::{FactoryError, Result};
use factory_core::event::Event;
use factory_core::policy;
use factory_core::quality;
use factory_core::run::{BlockSource, Run, RunPatch, RunStatus};
use factory_core::task::{Task, TaskEntry};
use factory_core::workflow::{WorkflowDefinition, WorkflowLint, IMPLICIT_NODE};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

/// Ten minutes -- a gate's timeout when its requirement names none, the same
/// fallback the bench gate runner has always used.
pub(crate) const DEFAULT_GATE_TIMEOUT_SECS: u64 = 600;

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
    pub(crate) async fn required_steps_for_task(&self, task: &Task) -> Result<Vec<RequiredStep>> {
        if task.bench_origin.is_some() {
            return Ok(Vec::new());
        }
        if let Some(origin) = &task.workflow_origin {
            let run = self.workflow_run(&origin.workflow_run_id).await?;
            return Ok(run.definition.required_steps_for(&origin.node_id));
        }
        let definition = WorkflowDefinition::implicit(task);
        let plans = self.control_plans(&definition).await?;
        let (injected, _) = definition.inject(&plans);
        Ok(injected.required_steps_for(IMPLICIT_NODE))
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
        self.verifying.lock().unwrap().remove(run_id);
    }

    fn enqueue_verification(&self, run_id: &str) {
        {
            let mut verifying = self.verifying.lock().unwrap();
            if !verifying.insert(run_id.to_string()) {
                return; // already queued or being verified
            }
        }
        let _ = self.verify_tx.send(run_id.to_string());
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
        let (injected, injections) = definition.inject(&plans);
        let violations = injected.ordering_violations(&plans);
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
