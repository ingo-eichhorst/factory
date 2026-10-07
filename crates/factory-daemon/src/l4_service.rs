//! L4 Process's service (#193 phase 6, slice S9c): the first part of it.
//!
//! It owns access to `L4State` (the task store, the workflow ledger, run evidence, the
//! workspace owner and the liveness cache) and serves the Process level's own reports and
//! housekeeping that do not need the rest of the run lifecycle: usage and estimate snapshots,
//! the production report, run artifacts, workspace recovery/release, and the liveness record.
//! The task/run core (`dispatch`, `report`, `finish_run`, workflows, intake, verification)
//! moves in S9a/S9b; until then `Engine` keeps one-line forwarders for the small helpers
//! below (`entry`, `require`, `require_run`, `publish_task`) that those paths use everywhere.
//!
//! It reaches the rest of the daemon only through `Wiring<'a, L4>`: the configuration snapshot,
//! the adapter registry, the bus, and the facts L4 may read (L1 to L3).
//!
//! Page or service (D5), module by module (S9c):
//! - `costs.rs`, `production.rs`, `artifacts.rs`, `workspace_lifecycle.rs`: service.
//! - `worktree.rs`, `worktree/owner.rs`: plain git and ledger helpers, no `Engine` (the owner is
//!   `L4State::workspaces`).
//! - `occupancy.rs`: the liveness record (`record_status`, `record_gone`, `close_liveness`) is
//!   service; the chart (`occupancy`, which composes the roster) and the run turn-end handling stay
//!   `impl Engine` until S9a.
//! - `operations_report.rs` (the Operations tab) and `site.rs` (the site page): **pages**. They compose
//!   L4's tasks and runs with L2/L3 state (sandbox attention, the roster, the repository walk) and own
//!   no state. `operations.rs` keeps the task commands (skip, close, reopen, answer), which are L4's.
use crate::facts::Wiring;
use crate::state::L4State;
use chrono::Utc;
use factory_core::adapter::runtime::RuntimeStatus;
use factory_core::error::{FactoryError, Result};
use factory_core::occupancy::StatusChange;
use factory_core::event::Event;
use factory_core::run::Run;
use crate::access::Caller;
use factory_core::task::{NewTask, Task, TaskEntry, WorkflowOrigin};
use factory_core::protocol::Request;
use factory_core::workflow::WorkflowActor;
use factory_kernel::L4;

pub(crate) struct L4Service<'a> {
    pub(crate) state: &'a L4State,
    pub(crate) wiring: Wiring<'a, L4>,
    /// Transitional (S9a part 2): `Engine` methods the moved run-start code still calls. See `run_start.rs`.
    pub(crate) core: &'a crate::engine::Engine,
    /// What L4 needs from above, supplied when the service is built (`factory_process::supplied`).
    pub(crate) above: &'a dyn factory_process::supplied::SuppliedFromAbove,
}

impl crate::engine::Engine {
    pub(crate) fn l4_service(&self) -> L4Service<'_> {
        L4Service {
            state: &self.l4,
            wiring: Wiring::new(self),
            core: self,
            above: self,
        }
    }
}

impl L4Service<'_> {
    pub(crate) async fn require(&self, id: &str) -> Result<Task> {
        self.state
            .store
            .get(id)
            .await?
            .ok_or_else(|| FactoryError::TaskNotFound(id.to_string()))
    }

    pub(crate) async fn workflow_run(&self, id: &str) -> Result<factory_core::workflow::WorkflowRun> {
        self.state
            .workflows
            .get_run(id)
            .await?
            .ok_or_else(|| FactoryError::BadRequest(format!("no such workflow run: {id}")))
    }

    pub(crate) async fn workflow_definition(&self, id: &str) -> Result<factory_core::workflow::WorkflowDefinition> {
        self.state
            .workflows
            .get_definition(id)
            .await?
            .ok_or_else(|| FactoryError::BadRequest(format!("no such workflow: {id}")))
    }

    pub(crate) async fn require_run(&self, id: &str) -> Result<Run> {
        self.state
            .store
            .get_run(id)
            .await?
            .ok_or_else(|| FactoryError::TaskNotFound(format!("run {id}")))
    }

    pub(crate) async fn publish_task(&self, id: &str) {
        if let Ok(Some(task)) = self.state.store.get(id).await {
            self.wiring.bus().publish(Event::TaskUpdated { task });
        }
    }

    /// A line in a task's journal, published as it is written. A store that will not
    /// write is a line in the log, never an error: the journal is a record, not a gate.
    pub(crate) async fn entry(&self, task_id: &str, entry: TaskEntry) {
        if let Err(e) = self.state.store.append_entry(task_id, &entry).await {
            tracing::warn!(task = task_id, "could not record journal entry: {e}");
        }
        self.wiring.bus().publish(Event::TaskEntry {
            id: task_id.to_string(),
            entry,
        });
    }

    /// Every attestation a run has collected, oldest first.
    pub(crate) async fn run_attestations(&self, run_id: &str) -> Result<Vec<factory_core::control_plan::StepAttestation>> {
        self.require_run(run_id).await?;
        self.state.run_evidence.step_attestations(run_id).await
    }
}

/// The liveness record the occupancy chart is drawn from (`seen_status`, `append_status`): L4's.
impl crate::l4_service::L4Service<'_> {
    /// Write down that a session's liveness changed -- and only then.
    ///
    /// This is the whole of Factory's liveness history. The runtime has none:
    /// herdr will say what an agent is doing now and has no idea what it was
    /// doing an hour ago, so a status that is not appended here when it is
    /// seen is gone for good. What that costs is honest to state: the runtime
    /// is polled, so a flip and a flip back between two ticks leaves no trace.
    ///
    /// Skipping a sample when nothing changed is right for this record and
    /// wrong for anything that wants to know an agent is still doing what it
    /// was doing -- `agents.rs`'s pushed-event handler feeds this for the
    /// chart and publishes on the bus separately, on purpose, rather than
    /// folding that into here.
    pub(crate) async fn record_status(
        &self,
        subject: &str,
        scope: &str,
        agent: &str,
        status: RuntimeStatus,
    ) {
        {
            let mut seen = match self.state.seen_status.lock() {
                Ok(seen) => seen,
                // A poisoned lock is not a reason to take the daemon down over
                // a chart. Skip the sample and carry on.
                Err(_) => return,
            };
            if seen.get(subject) == Some(&status) {
                return;
            }
            seen.insert(subject.to_string(), status);
        }
        let change = StatusChange {
            subject: subject.to_string(),
            scope: scope.to_string(),
            agent: agent.to_string(),
            status,
            at: Utc::now(),
        };
        if let Err(e) = self.state.store.append_status(&change).await {
            tracing::debug!(subject, "could not record liveness: {e}");
        }
    }
    /// A session that is not there any more. Closes the open span rather than
    /// letting the chart draw it forward to now.
    pub(crate) async fn record_gone(&self, subject: &str, scope: &str, agent: &str) {
        self.record_status(subject, scope, agent, RuntimeStatus::Gone)
            .await;
    }
    /// Close every open liveness span on the way out. Nothing observes an
    /// agent while the daemon is down, and a span left open would be drawn
    /// straight through the outage as though someone had been watching.
    pub async fn close_liveness(&self) {
        for agent in self.state.store.agents().await.unwrap_or_default() {
            if agent.session.is_some() {
                self.record_gone(&agent.id, &agent.scope, &agent.name).await;
            }
        }
        for run in self.state.store.active_runs().await.unwrap_or_default() {
            let Ok(Some(task)) = self.state.store.get(&run.task_id).await else {
                continue;
            };
            self.record_gone(&format!("run:{}", run.id), &task.scope, &run.agent)
                .await;
        }
    }
}

/// Task creation: every way a task is born goes through the L4 creation command (`commands::process`).
impl L4Service<'_> {
    pub async fn create(&self, new: NewTask) -> Result<Task> {
        self.create_task(new, None, None, None, None, false).await
    }

    /// A task born inside the intake gate (`#119`): `TaskStatus::Intake`
    /// from its first write, so there is no moment it could be dispatched.
    pub(crate) async fn create_intake_task(
        &self,
        new: NewTask,
        intake: factory_core::intake::Intake,
    ) -> Result<Task> {
        if new.schedule.is_some() {
            return Err(FactoryError::BadRequest(
                "an intake item has no schedule; release it first, then schedule the task".into(),
            ));
        }
        self.create_task(new, None, None, None, Some(intake), false).await
    }

    pub(crate) async fn create_workflow_task(
        &self,
        new: NewTask,
        origin: WorkflowOrigin,
        id: String,
    ) -> Result<Task> {
        self.create_task(new, Some(origin), None, Some(id), None, false).await
    }

    pub(crate) async fn create_bench_task(
        &self,
        new: NewTask,
        origin: factory_core::bench::BenchOrigin,
        id: String,
    ) -> Result<Task> {
        self.create_task(new, None, Some(origin), Some(id), None, false).await
    }

    pub(crate) async fn create_review_task(
        &self,
        new: NewTask,
        origin: Option<WorkflowOrigin>,
        id: String,
    ) -> Result<Task> {
        self.create_task(new, origin, None, Some(id), None, true).await
    }

    async fn create_task(
        &self,
        new: NewTask,
        workflow_origin: Option<WorkflowOrigin>,
        bench_origin: Option<factory_core::bench::BenchOrigin>,
        id: Option<String>,
        intake: Option<factory_core::intake::Intake>,
        internal_review: bool,
    ) -> Result<Task> {
        let observer = crate::commands::CreationObserver(self.wiring.bus().clone());
        let snapshot = self.wiring.snapshot();
        let process = crate::commands::process_with(
            self.state.store.as_ref(),
            self.wiring.registry(),
            &snapshot,
            &observer,
        );
        let receipt = process
            .create_extended(new, workflow_origin, bench_origin.map(Into::into), id, intake, internal_review)
            .await?;
        self.require(&receipt.id).await
    }

    /// Re-derive who a persisted `WorkflowActor` is right now, honouring a
    /// role change since the run started -- a workflow run remembers who
    /// asked, not a frozen copy of what they were allowed to do that moment.
    pub(crate) async fn caller_for_actor(&self, actor: &WorkflowActor) -> Caller {
        match actor {
            WorkflowActor::Owner => Caller::Owner,
            WorkflowActor::Agent { scope, name } => Caller::Agent {
                scope: scope.clone(),
                name: name.clone(),
                role: self.effective_role(scope, name).await,
                run_id: None,
            },
        }
    }

    /// The authority a hand-typed `task.create` immediately followed by
    /// `task.run` would need from `caller` -- exactly what a workflow node's
    /// spawn must never exceed. The task does not exist yet when a node
    /// becomes eligible, so `TaskRun` is checked against an id nothing has
    /// created: `authorize` already treats an unknown id as "let the engine
    /// report `no such task`" rather than as anybody's, which is task.run's
    /// grant-and-scope shape with no task-specific reach left to weigh in.
    pub(crate) async fn authorize_workflow_spawn(
        &self,
        caller: &Caller,
        template: &NewTask,
    ) -> Result<()> {
        self.core.authorize(caller, &Request::TaskCreate(template.clone()))
            .await?;
        self.core.authorize(
            caller,
            &Request::TaskRun {
                override_wait: false,
                id: uuid::Uuid::new_v4().to_string(),
                reason: None,
                continue_run: false,
            },
        )
        .await
    }
}

/// What L4 reads of L3: facts, and the configuration both share.
impl L4Service<'_> {
    /// The role an agent runs under right now (L3's `EffectiveRoleFact`).
    pub(crate) async fn effective_role(&self, scope: &str, name: &str) -> factory_core::role::Role {
        self.wiring
            .facts()
            .get::<factory_kernel::EffectiveRoleFact>(&(scope.to_string(), name.to_string()))
            .await
            .map(|fact| factory_core::role::Role::new(fact.0))
            .unwrap_or_default()
    }

    /// The version L3 last observed for an adapter's harness binary (L3's `HarnessVersionFact`).
    pub(crate) async fn harness_version_of(
        &self,
        adapter: &str,
        probe: &factory_agents::harness::HealthProbe,
    ) -> Option<String> {
        self.wiring
            .facts()
            .get::<factory_kernel::HarnessVersionFact>(&(adapter.to_string(), probe.clone()))
            .await
            .ok()
            .and_then(|fact| fact.0)
    }

    /// The health probe of the harness an agent resolves to, if its adapter declares one: a pure function of the
    /// configuration and the adapter registry.
    pub(crate) fn probe_for(&self, scope: &str, agent: &str) -> Option<factory_agents::harness::HealthProbe> {
        let (_, adapter, _) = self.wiring.resolve_agent(scope, agent).ok()?;
        self.wiring.registry().agent(&adapter).ok()?.health_probe()
    }
}

#[cfg(test)]
mod read_tests {
    /// L4 reads the effective role and the harness version from L3 as facts, and they are what L3 itself answers.
    #[tokio::test]
    async fn l4_reads_the_effective_role_and_harness_version_as_l3_facts() {
        let (engine, _root) = crate::environments::tests::engine_with("  - name: prod\n");
        let l4 = engine.l4_service();
        for (scope, agent) in [("company", "nobody"), ("no-such-scope", "x")] {
            assert_eq!(
                l4.effective_role(scope, agent).await,
                engine.l3_service().effective_role(scope, agent).await,
                "{scope}/{agent}"
            );
        }
        let probe = factory_agents::harness::HealthProbe::version("nonexistent-binary-for-the-test");
        assert_eq!(l4.harness_version_of("claude-code", &probe).await, None, "nothing observed yet");
    }
}
