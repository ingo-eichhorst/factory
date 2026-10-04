//! Constructors only: physical L1 owners receive their own capabilities and
//! fresh plain declarations, never Engine or an upper-level callback.
use crate::engine::Engine;

pub(super) fn expiry_provider(
    engine: &Engine,
) -> factory_infrastructure::expiry_store::ObservationStore {
    engine.infrastructure_expiries.clone()
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
    let snapshot = engine.factory_snapshot();
    factory_infrastructure::backup_facts::Provider {
        store: &engine.backups,
        busy: &engine.backup_busy,
        root: snapshot.root.clone(),
        instance: snapshot.config.instance.name.clone(),
        config: snapshot.config.infrastructure.backup.clone(),
        booted_at: engine.booted_at,
    }
}
pub(crate) fn environment_provider(
    engine: &Engine,
) -> factory_infrastructure::environment_facts::Provider<'_> {
    let snapshot = engine.factory_snapshot();
    factory_infrastructure::environment_facts::Provider {
        store: &engine.environments,
        scopes: snapshot.scope_tree(),
        declarations: snapshot.config.environments(),
    }
}
pub(super) fn renewal_provider(
    engine: &Engine,
) -> factory_infrastructure::renewal_declarations::Provider<'_> {
    let snapshot = engine.factory_snapshot();
    factory_infrastructure::renewal_declarations::Provider {
        cache: &engine.renewal_declaration_cache,
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
