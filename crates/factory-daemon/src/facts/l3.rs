//! Constructor only: current plain L3 declarations, not resolved facts.
use crate::engine::Engine;

pub(crate) fn provider(engine: &Engine) -> factory_agents::roster::Provider {
    let snapshot = engine.factory_snapshot();
    factory_agents::roster::Provider {
        scopes: snapshot
            .config
            .scopes
            .iter()
            .map(|scope| factory_agents::roster::RosterScope {
                name: scope.name.clone(),
                path: scope.path.clone(),
                agent: scope.agent.clone(),
                agents: scope.agents.clone(),
                roles: scope.roles.clone(),
            })
            .collect(),
        root_roles: snapshot.config.roles.clone(),
        foreman: snapshot.config.daemon.foreman.clone(),
    }
}

pub(crate) fn live_provider(engine: &Engine) -> factory_agents::store::LiveProvider {
    factory_agents::store::LiveProvider {
        agents: engine.l3.agents.clone(),
    }
}

pub(crate) fn harness_provider(engine: &Engine) -> crate::harness_health::HarnessProvider {
    crate::harness_health::HarnessProvider {
        health: engine.l3.harness.clone(),
    }
}

pub(crate) fn role_provider(engine: &Engine) -> crate::l3_service::RoleProvider<'_> {
    crate::l3_service::RoleProvider(engine.l3_service())
}

pub(crate) fn version_provider(engine: &Engine) -> crate::harness_health::VersionProvider {
    crate::harness_health::VersionProvider {
        health: engine.l3.harness.clone(),
    }
}

pub(crate) fn liveness_provider(engine: &Engine) -> crate::l3_service::LivenessProvider {
    crate::l3_service::LivenessProvider(engine.l3.liveness.clone())
}

pub(crate) fn session_status_provider(engine: &Engine) -> factory_agents::runtime::StatusProvider<'_> {
    factory_agents::runtime::StatusProvider { runtimes: &engine.shared.registry }
}
