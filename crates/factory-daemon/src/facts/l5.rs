//! Outside-stack construction of the physical L5 provider. No gathering here.
use crate::engine::Engine;

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

pub(super) fn provider(engine: &Engine) -> factory_assurance::facts::Provider<'_> {
    factory_assurance::facts::Provider::new(&engine.bench, engine.factory_snapshot().root)
}
