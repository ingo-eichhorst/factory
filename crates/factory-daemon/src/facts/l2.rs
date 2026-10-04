//! Constructors only: fresh plain inputs and own L2 capabilities, never callbacks.
use crate::engine::Engine;
pub(crate) fn credentials_provider(engine: &Engine) -> factory_environment::credentials::Provider {
    let snapshot = engine.factory_snapshot();
    factory_environment::credentials::Provider {
        root: snapshot.root.clone(),
        scopes: snapshot.scope_tree(),
        home: std::env::var_os("HOME").map(std::path::PathBuf::from),
    }
}
pub(crate) fn dependencies_provider(
    engine: &Engine,
) -> factory_environment::dependency_inventory::Provider {
    let snapshot = engine.factory_snapshot();
    factory_environment::dependency_inventory::Provider {
        root: snapshot.root.clone(),
        scopes: snapshot.scope_tree(),
        declarations: snapshot
            .config
            .scopes
            .iter()
            .map(|scope| factory_environment::dependency_inventory::Scope {
                name: scope.name.clone(),
                dependencies: scope.dependencies.clone(),
            })
            .collect(),
        credentials: credentials_provider(engine),
    }
}
pub(super) fn evidence_provider(
    engine: &Engine,
) -> factory_environment::service_observations::Provider {
    let snapshot = engine.factory_snapshot();
    factory_environment::service_observations::Provider {
        root: snapshot.root.clone(),
        instance: snapshot.config.instance.id.clone(),
        scopes: snapshot.scope_tree(),
    }
}
pub(crate) fn provider_declarations(
    snapshot: &factory_core::config::Factory,
) -> Vec<factory_environment::credential_expiry::ScopedProviders> {
    snapshot
        .scope_names()
        .into_iter()
        .filter_map(|name| {
            let scope = snapshot.scope(&name).ok()?;
            Some(factory_environment::credential_expiry::ScopedProviders {
                name: scope.name.clone(),
                agents: scope
                    .declared_agents()
                    .into_iter()
                    .filter_map(|agent| {
                        let config = agent.openshell.clone()?;
                        Some(factory_environment::credential_expiry::AgentProviders {
                            name: agent.name(),
                            config,
                        })
                    })
                    .collect(),
            })
        })
        .collect()
}
pub(super) fn expiry_provider(
    engine: &Engine,
) -> factory_environment::credential_expiry::Provider<'_> {
    let snapshot = engine.factory_snapshot();
    factory_environment::credential_expiry::Provider {
        store: &engine.credential_expiries,
        instance: snapshot.config.instance.id.clone(),
        declarations: snapshot.config.secrets.clone(),
        providers: provider_declarations(&snapshot),
    }
}
