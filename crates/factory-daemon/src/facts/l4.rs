//! Process-owned providers. Ambiguous names never acquire run history.
use crate::engine::Engine;

pub(super) fn blueprint_provider(engine: &Engine) -> factory_process::workflow_blueprints::Provider<'_> {
    factory_process::workflow_blueprints::Provider {tasks: engine.l4.store.as_ref(), workflows: &engine.l4.workflows}
}

pub(super) fn mirror_provider(engine: &Engine) -> factory_process::workflow_store::WorkflowStore {
    engine.l4.workflows.clone()
}

pub(super) fn provenance_provider(
    engine: &Engine,
) -> factory_process::facts::ProvenanceProvider<'_> {
    factory_process::facts::ProvenanceProvider::new(engine.l4.store.as_ref(), &engine.l4.run_evidence)
}

pub(super) fn provider(engine: &Engine) -> factory_process::facts::Provider<'_> {
    let snapshot = engine.factory_snapshot();
    factory_process::facts::Provider::new(
        engine.l4.store.as_ref(),
        &engine.l4.workflows,
        snapshot.scope_tree(),
        snapshot.root,
    )
}

pub(crate) async fn import_recovery_journal(engine: &Engine) -> Vec<String> {
    provider(engine).import_recovery_journal().await
}

/// Capture current scope identities for each read; no hub enters the owner.
pub(super) fn measurement_provider(
    engine: &Engine,
) -> factory_process::measurements::MeasurementProvider<'_> {
    factory_process::measurements::MeasurementProvider::new(
        engine.l4.store.as_ref(),
        &engine.l4.workflows,
        &engine.l4.run_evidence,
        engine.factory_snapshot().scope_tree(),
    )
}
