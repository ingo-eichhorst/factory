//! L4's one capability-limited handle on `Arc<Engine>` (#193 phase 6, S9b).
//!
//! `L4Service` borrows the engine, so it cannot `tokio::spawn` a run start or call the workflow methods that
//! themselves spawn. Code that must is handed an `L4Spawner`: the only way L4-owned code gets at an `Arc<Engine>`
//! (`level_ownership_tests` forbids the type anywhere else in L4 files). It exposes exactly what L4 needs and nothing
//! of the engine beyond that; each method does the same thing the inlined `engine.clone()` + `tokio::spawn` or direct
//! call did, so spawn timing is unchanged.
//!
//! **Transitional.** Each method is an Engine-resident call that the next slices replace:
//! - `spawn_start_run*`: a command from the scheduling owner (L4 starting its own run later) once the run-start path
//!   owns a work queue instead of detached tasks (S10/S12: the L5 service issues these as commands down to L4, and L4's
//!   own callers use a channel to a worker);
//! - `advance_workflow`, `sync_workflow_for_task`, `start_workflow_with_agents`, `start_decomposition_workflow`: they
//!   move into `L4Service` with the workflows (S9b part 3) and then need only this handle's spawn methods.
use crate::access::Caller;
use crate::engine::{Due, Engine};
use factory_core::error::Result;
use factory_core::intake::{Routing, SplitPart};
use factory_core::run::{Run, Trigger};
use factory_core::task::Task;
use factory_core::workflow::WorkflowRun;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct L4Spawner {
    engine: Arc<Engine>,
}

impl Engine {
    pub(crate) fn l4_spawner(self: &Arc<Self>) -> L4Spawner {
        L4Spawner { engine: self.clone() }
    }
}

impl L4Spawner {
    /// Start a run of `task_id` on a task of its own, due now.
    pub(crate) fn spawn_start_run(&self, task_id: String, trigger: Trigger) {
        let engine = self.engine.clone();
        tokio::spawn(async move {
            engine.l4_service().start_run(&task_id, trigger).await;
        });
    }

    /// Start a run of `task_id` on a task of its own, due at `due`.
    pub(crate) fn spawn_start_run_due(&self, task_id: String, trigger: Trigger, due: Due) {
        let engine = self.engine.clone();
        tokio::spawn(async move {
            engine.l4_service().start_run_due(&task_id, trigger, due).await;
        });
    }

    /// Start a `--continue` run of `task_id` resuming `previous`, on a task of its own.
    pub(crate) fn spawn_start_run_due_continue(&self, task_id: String, due: Due, previous: Run) {
        let engine = self.engine.clone();
        tokio::spawn(async move {
            engine.l4_service().start_run_due_continue(&task_id, due, previous).await;
        });
    }

    pub(crate) async fn advance_workflow(&self, id: &str) -> Result<()> {
        self.engine.advance_workflow(id).await
    }

    pub(crate) async fn sync_workflow_for_task(&self, task_id: &str) {
        self.engine.sync_workflow_for_task(task_id).await
    }

    pub(crate) async fn start_workflow_with_agents(
        &self,
        id: &str,
        inputs: BTreeMap<String, String>,
        agents: &BTreeMap<String, String>,
        caller: &Caller,
    ) -> Result<WorkflowRun> {
        self.engine.start_workflow_with_agents(id, inputs, agents, caller).await
    }

    pub(crate) async fn start_decomposition_workflow(
        &self,
        item: &Task,
        parts: &[SplitPart],
        routing: &Routing,
        caller: &Caller,
    ) -> Result<WorkflowRun> {
        self.engine.start_decomposition_workflow(item, parts, routing, caller).await
    }
}
