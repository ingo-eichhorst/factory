//! Outside-stack construction of the physical L5 provider. No gathering here.
use crate::engine::Engine;

pub(super) fn check_provider(
    engine: &Engine,
) -> factory_assurance::check_evaluation::Provider<'_, super::checks::Ports<'_>> {
    let snapshot = engine.factory_snapshot();
    factory_assurance::check_evaluation::Provider::new(
        factory_assurance::metrics_service::Service::new(super::checks::service(
            engine,
            snapshot.scope_tree(),
        )),
        snapshot.root.clone(),
        crate::metrics::reported_configuration(&snapshot),
    )
}

pub(super) fn metric_provider(
    engine: &Engine,
) -> factory_assurance::metric_values::Provider<'_, super::checks::Ports<'_>> {
    factory_assurance::metric_values::Provider::new(
        factory_assurance::metrics_service::Service::new(super::checks::service(
            engine,
            engine.factory_snapshot().scope_tree(),
        )),
    )
}

pub(super) fn signpost_provider(
    engine: &Engine,
) -> factory_assurance::signposts::Provider<'_, super::checks::Ports<'_>> {
    factory_assurance::signposts::Provider::new(
        factory_assurance::metrics_service::Service::new(super::checks::service(
            engine,
            engine.factory_snapshot().scope_tree(),
        )),
        &engine.signpost_cache,
    )
}

pub(super) fn plan_provider(engine: &Engine) -> factory_assurance::plan_service::Provider {
    let snapshot = engine.factory_snapshot();
    factory_assurance::plan_service::Provider::new(
        snapshot.root.clone(), crate::quality::quality_configuration(&snapshot))
}

pub(super) fn preview_provider(engine: &Engine) -> factory_assurance::workflow_preview::Provider<factory_agents::roster::Provider> {
    factory_assurance::workflow_preview::Provider::new(plan_provider(engine), super::l3::provider(engine),
        engine.factory_snapshot().config.daemon.default_agent.clone())
}

pub(super) fn provider(engine: &Engine) -> factory_assurance::facts::Provider<'_> {
    factory_assurance::facts::Provider::new(&engine.bench, engine.factory_snapshot().root)
}
