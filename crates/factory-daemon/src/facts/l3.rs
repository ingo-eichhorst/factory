//! Runtime-owned roster facts; role resolution has one live source.
use crate::engine::Engine;
use async_trait::async_trait;
use factory_core::{
    config::{ForemanConfig, Scope},
    error::{FactoryError, Result},
    role::Roles,
};
use factory_kernel::{AgentFact, Provide};

pub(crate) struct Provider<'a> {
    pub(super) engine: &'a Engine,
}
impl factory_kernel::FactProvider for Provider<'_> {
    type Level = factory_kernel::L3;
}
#[async_trait]
impl Provide<AgentFact> for Provider<'_> {
    type Query = String;
    type Value = Vec<AgentFact>;
    type Error = FactoryError;
    async fn get(&self, scope: &String) -> Result<Self::Value> {
        let snapshot = self.engine.factory_snapshot();
        let scope = snapshot.scope(scope)?;
        let roles = self.engine.roles_for(&scope.name);
        Ok(agent_facts_for(
            scope,
            &snapshot.config.daemon.foreman,
            &roles,
        ))
    }
}
fn agent_facts_for(scope: &Scope, foreman: &ForemanConfig, roles: &Roles) -> Vec<AgentFact> {
    // A synthesized foreman is a real dispatchable agent and has no
    // sandbox; do not exempt it by reading only declared_agents.
    scope
        .agents_with(foreman)
        .into_iter()
        .map(|agent| {
            let grants = roles.get(&agent.role).map(|def| def.grants.clone());
            AgentFact {
                name: agent.name(),
                role: agent.role.as_str().to_string(),
                grants,
                has_sandbox: !agent.sandbox.is_none(),
            }
        })
        .collect()
}
