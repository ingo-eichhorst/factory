//! Outside-stack construction of the physical L5 provider. No gathering here.
use crate::engine::Engine;
use factory_core::error::Result;
use factory_kernel::{GateFact, KnowledgeTags, Provide};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn provider(engine: &Engine) -> factory_assurance::facts::Provider<'_> {
    factory_assurance::facts::Provider::new(&engine.bench, engine.factory_snapshot().root)
}

/// Same-level reads call the actual L5 provider directly, not Facts<L6>.
pub(crate) async fn assurance_gate_facts(
    engine: &Engine,
    names: &BTreeSet<String>,
) -> Result<BTreeMap<String, GateFact>> {
    Provide::<GateFact>::get(&provider(engine), names).await
}

pub(crate) async fn assurance_knowledge_tags(engine: &Engine) -> Result<KnowledgeTags> {
    Provide::<KnowledgeTags>::get(&provider(engine), &()).await
}
