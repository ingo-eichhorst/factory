//! Outside-stack construction of the physical L5 provider. No gathering here.
use crate::engine::Engine;

pub(super) fn provider(engine: &Engine) -> factory_assurance::facts::Provider<'_> {
    factory_assurance::facts::Provider::new(&engine.bench, engine.factory_snapshot().root)
}
