//! Live Process evidence, using only L4's task and immutable evidence stores.
use crate::{evidence_store::RunEvidenceStore, run::RunStatus, store::TaskStore};
use factory_kernel::{ArtifactProvenance, FactoryError, Result};

pub struct ProvenanceProvider<'a> {
    store: &'a dyn TaskStore,
    evidence: &'a RunEvidenceStore,
}

impl<'a> ProvenanceProvider<'a> {
    pub fn new(store: &'a dyn TaskStore, evidence: &'a RunEvidenceStore) -> Self {
        Self { store, evidence }
    }
}

impl factory_kernel::FactProvider for ProvenanceProvider<'_> {
    type Level = factory_kernel::L4;
}

#[async_trait::async_trait]
impl factory_kernel::Provide<ArtifactProvenance> for ProvenanceProvider<'_> {
    type Query = String;
    type Value = Vec<ArtifactProvenance>;
    type Error = FactoryError;
    async fn get(&self, id: &String) -> Result<Self::Value> {
        let run = self
            .store
            .get_run(id)
            .await?
            .ok_or_else(|| FactoryError::TaskNotFound(format!("run {id}")))?;
        if run.status != RunStatus::Done {
            return Ok(Vec::new());
        }
        let records = self.evidence.provenance(id).await?;
        Ok(records
            .into_iter()
            .filter(|p| {
                p.run_id == run.id
                    && p.task_id == run.task_id
                    && Some(p.statement.predicate.run_details.metadata.finished_on) == run.ended_at
                    && run.artifacts.iter().any(|a| a == &p.artifact)
            })
            .collect())
    }
}
