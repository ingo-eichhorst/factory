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
