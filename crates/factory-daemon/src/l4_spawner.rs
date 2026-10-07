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
//! - the workflow calls (`advance_workflow`, `sync_workflow_for_task`, `start_workflow_with_agents`,
//!   `start_decomposition_workflow`) now live in `L4Service` (S9b part 3) and take this handle for their spawns.
use crate::engine::{Due, Engine};
use factory_core::run::{Run, Trigger};
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

    /// Take a usage reading of `run` at `point` on a task of its own, off whatever request is waiting (a harness's
    /// turn-end hook, for instance).
    pub(crate) fn spawn_snapshot(&self, run: Run, point: factory_core::usage::SnapshotPoint) {
        let engine = self.engine.clone();
        tokio::spawn(async move { engine.l4_service().snapshot_usage(&run, point).await });
    }
}
