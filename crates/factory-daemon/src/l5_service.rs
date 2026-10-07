//! L5 Improvement's service (#193 phase 6, slice S10): the first part of it.
//!
//! It owns access to `L5State` (the bench store and its edit lock, the filed suggestions, dataset locks, the quality
//! guide cache and the signpost cache) and serves what L5 serves. S10 part 1 moves datasets and suggestions; bench
//! and quality follow.
//!
//! It reaches the rest of the daemon only through `Wiring<'a, L5>`: the configuration snapshot, the adapter
//! registry, the bus, the facts L5 may read (L1 to L4 records as `TaskSnapshotFact` and `RunSnapshotFact`), and one
//! command edge down, `Commands<L5, L4Port>` (`wiring.l4()`), for everything L5 makes L4 do.
//!
//! Page or service (D5), module by module (S10 part 1):
//! - `datasets.rs`, `suggestions.rs`: service.
//! - `metrics.rs`, `signposts.rs`: **pages**. They compose evidence from every level (the check service reads L1 to
//!   L4 facts) and hand L6's authored policy inputs down to L5's evidence services; they own no L5 state.
use crate::engine::Engine;
use crate::facts::Wiring;
use crate::state::L5State;
use factory_core::error::{FactoryError, Result};
use factory_core::run::Run;
use factory_core::task::Task;
use factory_kernel::{RunSnapshotFact, RunSnapshotQuery, TaskSnapshotFact, L5};

pub(crate) struct L5Service<'a> {
    pub(crate) state: &'a L5State,
    pub(crate) wiring: Wiring<'a, L5>,
}

impl Engine {
    pub(crate) fn l5_service(&self) -> L5Service<'_> {
        L5Service { state: &self.l5, wiring: Wiring::new(self) }
    }
}

/// L4's records, read as facts. A task or run that does not exist is `None`, not an error.
impl L5Service<'_> {
    pub(crate) async fn task_record(&self, id: &str) -> Result<Option<Task>> {
        match self.wiring.facts().get::<TaskSnapshotFact>(&id.to_string()).await {
            Ok(fact) => Ok(Some(serde_json::from_value(fact.0).map_err(|e| FactoryError::Other(e.into()))?)),
            Err(FactoryError::TaskNotFound(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(crate) async fn run_record(&self, query: RunSnapshotQuery) -> Result<Option<Run>> {
        let fact = self.wiring.facts().get::<RunSnapshotFact>(&query).await?;
        fact.0.map(|value| serde_json::from_value(value).map_err(|e| FactoryError::Other(e.into()))).transpose()
    }

    /// The task's newest run, whatever its status.
    pub(crate) async fn latest_run(&self, task_id: &str) -> Result<Option<Run>> {
        self.run_record(RunSnapshotQuery::Latest(task_id.to_string())).await
    }

    /// The task's run in progress, if any.
    pub(crate) async fn active_run(&self, task_id: &str) -> Result<Option<Run>> {
        self.run_record(RunSnapshotQuery::Active(task_id.to_string())).await
    }

    /// The task, or `TaskNotFound`.
    pub(crate) async fn require_task(&self, id: &str) -> Result<Task> {
        self.task_record(id).await?.ok_or_else(|| FactoryError::TaskNotFound(id.to_string()))
    }
}
