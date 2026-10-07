//! L5's one capability-limited handle on `Arc<Engine>` (#193 phase 6, S10), mirroring `L4Spawner`.
//!
//! `L5Service` borrows the engine, so it cannot `tokio::spawn`. Code that must is handed an `L5Spawner`: the only way
//! L5-owned code gets at an `Arc<Engine>` (`level_ownership_tests` forbids the type anywhere else in L5 files). The
//! spawned work reaches L4 only through the command edge (`Wiring<L5>::l4()`), never through L4 state.
//!
//! **Transitional.** `spawn_bench_attempt` is the dispatch of one bench attempt (create its task, start its run). It
//! goes when L4 owns a work queue for runs started by another level (S12); until then it does exactly what the inlined
//! `engine.clone()` + `tokio::spawn` did, at the same point.
use crate::engine::Engine;
use factory_core::bench::BenchOrigin;
use factory_core::run::Trigger;
use factory_core::task::NewTask;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct L5Spawner {
    engine: Arc<Engine>,
}

impl Engine {
    pub(crate) fn l5_spawner(self: &Arc<Self>) -> L5Spawner {
        L5Spawner { engine: self.clone() }
    }
}

impl L5Spawner {
    /// Create one bench attempt's task and start its run, on a task of its own. A task that could never be created
    /// settles the attempt as an error (`mark_bench_attempt_uncreated`).
    pub(crate) fn spawn_bench_attempt(&self, new: NewTask, origin: BenchOrigin, task_id: String, run_id: String) {
        let engine = self.engine.clone();
        tokio::spawn(async move {
            let l5 = engine.l5_service();
            match l5.wiring.l4().port().create_bench_task(new, origin, task_id.clone()).await {
                Ok(_) => l5.wiring.l4().port().start_run(&task_id, Trigger::Bench).await,
                Err(e) => {
                    tracing::warn!(task = task_id, "could not create bench task: {e}");
                    l5.mark_bench_attempt_uncreated(&run_id, &task_id, &e.to_string()).await;
                }
            }
        });
    }
}
