//! Constructors only: physical L1 owners receive their own capabilities and
//! fresh plain declarations, never Engine or an upper-level callback.
use crate::engine::Engine;

pub(super) fn expiry_provider(
    engine: &Engine,
) -> factory_infrastructure::expiry_store::ObservationStore {
    engine.l1.infrastructure_expiries.clone()
}
pub(super) fn settings_provider(
    engine: &Engine,
) -> factory_infrastructure::settings_facts::SettingsProvider {
    let snapshot = engine.factory_snapshot();
    let daemon = &snapshot.config.daemon;
    factory_infrastructure::settings_facts::SettingsProvider {
        foreman_enabled: daemon.foreman.enabled,
        power_assertion: daemon.power_assertion,
        tick_seconds: daemon.tick_seconds,
        scopes: snapshot
            .config
            .scopes
            .iter()
            .map(|scope| (scope.name.clone(), scope.max_sessions))
            .collect(),
        interfaces: daemon.interfaces.clone(),
    }
}
pub(crate) fn backup_provider(
    engine: &Engine,
) -> factory_infrastructure::backup_facts::Provider<'_> {
    engine.l1_service().backup_provider()
}
pub(crate) fn environment_provider(
    engine: &Engine,
) -> factory_infrastructure::environment_facts::Provider<'_> {
    let snapshot = engine.factory_snapshot();
    factory_infrastructure::environment_facts::Provider {
        store: &engine.l1.environments,
        scopes: snapshot.scope_tree(),
        declarations: snapshot.config.environments(),
    }
}
pub(super) fn renewal_provider(
    engine: &Engine,
) -> factory_infrastructure::renewal_declarations::Provider<'_> {
    let snapshot = engine.factory_snapshot();
    factory_infrastructure::renewal_declarations::Provider {
        cache: &engine.l1.renewal_declaration_cache,
        instance_path: snapshot.factory_dir().join("config.yaml"),
        declarations: snapshot.config.renewals.clone(),
        scopes: snapshot
            .config
            .scope
            .iter()
            .chain(&snapshot.config.scopes)
            .map(
                |scope| factory_infrastructure::renewal_declarations::ScopeRenewals {
                    name: scope.name.clone(),
                    path: scope.path.clone(),
                    declarations: scope.renewals.clone(),
                },
            )
            .collect(),
    }
}

/// The one HTTP bind the daemon reports itself on: a loopback one when there are
/// several (it does not move when the host's network address does), else the
/// first. Derived from the L1 interface facts (`InterfaceConfig::http_bind`).
pub(crate) fn selected_http_bind(factory: &factory_core::config::Factory) -> Option<String> {
    let binds: Vec<String> = factory
        .config
        .daemon
        .interfaces
        .iter()
        .filter(|interface| interface.kind == "http")
        .map(|interface| interface.http_bind())
        .collect();
    binds
        .iter()
        .find(|bind| bind.starts_with("127.") || bind.starts_with("localhost:") || bind.starts_with("[::1]:"))
        .or_else(|| binds.first())
        .cloned()
}
