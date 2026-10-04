//! Improvement-owned bench gate facts.
use crate::engine::Engine;
use async_trait::async_trait;
use factory_core::error::{FactoryError, Result};
use factory_kernel::{GateCase, GateFact, KnowledgeTags, Provide};
use std::collections::{BTreeMap, BTreeSet};
pub(crate) struct Provider<'a> {
    pub(super) engine: &'a Engine,
}

/// Same-level L5 reads are ordinary calls, not an upward Facts<L6> identity.
/// The provider remains the sole implementation of the live read.
pub(crate) async fn assurance_gate_facts(
    engine: &Engine,
    names: &BTreeSet<String>,
) -> Result<BTreeMap<String, GateFact>> {
    Provide::<GateFact>::get(&Provider { engine }, names).await
}

pub(crate) async fn assurance_knowledge_tags(engine: &Engine) -> Result<KnowledgeTags> {
    Provide::<KnowledgeTags>::get(&Provider { engine }, &()).await
}

#[async_trait]
impl Provide<factory_kernel::BenchResolutionFact> for Provider<'_> {
    type Query = String;
    type Value = Option<factory_kernel::BenchResolutionFact>;
    type Error = FactoryError;
    async fn get(&self, dataset: &String) -> Result<Self::Value> {
        let runs = self.engine.bench.runs(Some(dataset), 200).await?;
        let Some(run) = runs.into_iter().find(|run| run.settled()) else { return Ok(None); };
        let results = factory_core::bench::aggregate(&run.attempts);
        let (passed, failed) = results.iter().fold((0, 0), |(passed, failed), result| {
            (passed + result.pass, failed + result.fail)
        });
        Ok(Some(factory_kernel::BenchResolutionFact {
            dataset: dataset.clone(), run_id: run.id, started_at: run.started_at,
            ended_at: run.ended_at, passed, failed,
        }))
    }
}

#[async_trait]
impl Provide<KnowledgeTags> for Provider<'_> {
    type Query = ();
    type Value = KnowledgeTags;
    type Error = FactoryError;
    async fn get(&self, _: &()) -> Result<Self::Value> {
        let root = self.engine.factory_snapshot().root;
        tokio::task::spawn_blocking(move || KnowledgeTags {
            tags: factory_core::knowledge::index(&root)
                .tags
                .into_iter()
                .map(|t| t.name)
                .collect(),
        })
        .await
        .map_err(|e| FactoryError::Other(anyhow::anyhow!("knowledge index: {e}")))
    }
}
impl factory_kernel::FactProvider for Provider<'_> {
    type Level = factory_kernel::L5;
}
impl Provider<'_> {
    async fn gate_fact_for(&self, dataset: &str) -> Result<Option<GateFact>> {
        // `BenchStore::runs` loads every returned run's attempts eagerly, so
        // this is bounded rather than "every run this dataset ever had" --
        // the same 200 `Request::BenchRuns` already asks for, which a
        // dataset gated often enough to bury its newest settled run past
        // could still, in principle, outrun; the same edge case that bound
        // already accepts.
        let runs = self.engine.bench.runs(Some(dataset), 200).await?;
        let Some(run) = runs.into_iter().find(|r| r.settled()) else {
            return Ok(None);
        };
        let cases = run
            .cases
            .iter()
            .map(|case| GateCase {
                id: case.id.clone(),
                gated: case.gate.is_some(),
                verdicts: run
                    .attempts
                    .iter()
                    .filter(|a| a.case_id == case.id)
                    .filter_map(|a| a.verdict)
                    .collect(),
            })
            .collect();
        Ok(Some(GateFact {
            run_id: run.id,
            ended_at: run.ended_at,
            cases,
        }))
    }
}
#[async_trait]
impl Provide<GateFact> for Provider<'_> {
    type Query = BTreeSet<String>;
    type Value = BTreeMap<String, GateFact>;
    type Error = FactoryError;
    async fn get(&self, names: &Self::Query) -> Result<Self::Value> {
        let mut gates = BTreeMap::new();
        for name in names {
            if let Some(fact) = self.gate_fact_for(name).await? {
                gates.insert(name.clone(), fact);
            }
        }
        Ok(gates)
    }
}
