//! Process-owned providers. Ambiguous names never acquire run history.
use crate::engine::Engine;
use factory_core::error::Result;

pub(super) fn blueprint_provider(engine: &Engine) -> factory_process::workflow_blueprints::Provider<'_> {
    factory_process::workflow_blueprints::Provider {tasks: engine.store.as_ref(), workflows: &engine.workflows}
}

pub(super) fn mirror_provider(engine: &Engine) -> factory_process::workflow_store::WorkflowStore {
    engine.workflows.clone()
}

pub(super) fn provenance_provider(
    engine: &Engine,
) -> factory_process::facts::ProvenanceProvider<'_> {
    factory_process::facts::ProvenanceProvider::new(engine.store.as_ref(), &engine.run_evidence)
}

pub(super) fn provider(engine: &Engine) -> factory_process::facts::Provider<'_> {
    let snapshot = engine.factory_snapshot();
    factory_process::facts::Provider::new(
        engine.store.as_ref(),
        &engine.workflows,
        snapshot.scope_tree(),
        snapshot.root,
    )
}

pub(crate) async fn import_recovery_journal(engine: &Engine) -> Vec<String> {
    provider(engine).import_recovery_journal().await
}

pub(crate) async fn process_security_reports(
    engine: &Engine,
    scope: Option<&str>,
) -> Result<Vec<factory_kernel::ConfirmedSecurityReport>> {
    super::Facts::<factory_kernel::People>::new(engine)
        .get::<factory_kernel::ConfirmedSecurityReport>(&scope.map(str::to_owned))
        .await
}

/// Capture current scope identities for each read; no hub enters the owner.
pub(super) fn measurement_provider(
    engine: &Engine,
) -> factory_process::measurements::MeasurementProvider<'_> {
    factory_process::measurements::MeasurementProvider::new(
        engine.store.as_ref(),
        &engine.workflows,
        &engine.run_evidence,
        engine.factory_snapshot().scope_tree(),
    )
}
