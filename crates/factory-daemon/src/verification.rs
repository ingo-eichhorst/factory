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
use factory_core::conformance::AttestedRun;
use factory_core::control_plan::{
    self, AttestationVerdict, ControlPlan, RequiredStep, StepAttestation, GATE_ACTOR,
};
use factory_core::error::{FactoryError, Result};
use factory_core::event::Event;
use factory_core::operations::Window;
use factory_core::policy;
use factory_core::quality;
use factory_core::run::{BlockSource, Run, RunPatch, RunStatus};
use factory_core::task::{Task, TaskEntry, TaskFilter};
use factory_core::workflow::{WorkflowDefinition, WorkflowLint, IMPLICIT_NODE};
use std::collections::{BTreeMap, BTreeSet};
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
    /// This becomes `Provide<…>` in `#193` phase 3; until then it wraps
    /// `PolicyStore` exactly where the table already lives.
    pub(crate) async fn attested_runs(
        &self,
        scopes: Option<&BTreeSet<String>>,
        categories: Option<&BTreeSet<String>>,
        window: Window,
    ) -> Result<Vec<AttestedRun>> {
        let mut runs = self.store.runs_between(window.from, window.to).await?;
        runs.retain(|r| r.ended_at.is_some_and(|ended| window.contains(ended)));
        if runs.is_empty() {
            return Ok(Vec::new());
        }
        let snapshot = self.factory_snapshot();
        let tasks = self.store.list(&TaskFilter::default()).await?;
        let tasks_by_id: BTreeMap<&str, &Task> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();

        struct Resolved {
            run_id: String,
            task_id: String,
            scope: String,
            category: String,
            agent: String,
            status: RunStatus,
            ended_at: chrono::DateTime<Utc>,
            fail_kind: Option<factory_core::run::FailKind>,
            required_steps: Vec<RequiredStep>,
        }
        let mut resolved = Vec::new();
        for run in runs {
            let Some(task) = tasks_by_id.get(run.task_id.as_str()) else {
                continue;
            };
            if task.bench_origin.is_some() {
                continue;
            }
            let scope = snapshot.canonical_scope_name(&task.scope);
            if let Some(scopes) = scopes {
                if !scopes.contains(&scope) {
                    continue;
                }
            }
            let category = control_plan::effective_category(task.category.as_deref()).to_string();
            if let Some(categories) = categories {
                if !categories.contains(&category) {
                    continue;
                }
            }
            resolved.push(Resolved {
                run_id: run.id.clone(),
                task_id: task.id.clone(),
                scope,
                category,
                agent: run.agent.clone(),
                status: run.status,
                ended_at: run.ended_at.expect("retained above"),
                fail_kind: run.fail_kind,
                required_steps: run.required_steps.clone(),
            });
        }
        if resolved.is_empty() {
            return Ok(Vec::new());
        }
        let run_ids: Vec<String> = resolved.iter().map(|r| r.run_id.clone()).collect();
        let mut attestations_by_run = self.policies.step_attestations_for(&run_ids).await?;
        Ok(resolved
            .into_iter()
            .map(|r| AttestedRun {
                attestations: attestations_by_run.remove(&r.run_id).unwrap_or_default(),
                run_id: r.run_id,
                task_id: r.task_id,
                scope: r.scope,
                category: r.category,
                agent: r.agent,
                status: r.status,
                ended_at: r.ended_at,
                fail_kind: r.fail_kind,
                required_steps: r.required_steps,
            })
            .collect())
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
    use factory_plugins::{Registry, SqliteStore};
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

    /// An instance committed to one framework, `house`, whose single control
    /// requires `requires` -- YAML for one `requires:` list -- and whose
    /// only scope, `demo`, is a plain directory the gates run in.
    fn engine(requires: &str) -> (Arc<Engine>, PathBuf) {
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
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig { power_assertion: false, ..DaemonConfig::default() },
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
                agents: Vec::new(),
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
            }],
            infrastructure: Default::default(),
            plugins_dir: None,
        };
        let mut registry = Registry::with_builtins();
        registry.add_runtime(Arc::new(QuietRuntime), "test");
        let engine = Arc::new(Engine::new(
            Factory { root, config },
            registry,
            Arc::new(SqliteStore::in_memory().unwrap()),
            PathBuf::from("factory"),
            Vec::new(),
        ));
        engine.spawn_verifier();
        (engine, work)
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
            session: Default::default(),
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

    // -- attested_runs (#158) ------------------------------------------------

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
            });
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
