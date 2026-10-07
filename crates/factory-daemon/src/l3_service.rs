//! L3 Agent's service (#193 phase 6, slice S7).
//!
//! It owns access to `L3State` (the standing-agent rows, `AgentStore`, and the harness
//! health cache) and serves the agent level: starting, stopping and reconciling standing
//! agents, their roles and the role board, typing at and reading an agent's session, and
//! the answer to "what may this agent do" (`effective_role`), which every authorisation
//! reads. The bodies live in `agents.rs` and `roles.rs`.
//!
//! It reaches the rest of the daemon only through `Wiring<'a, L3>`: the configuration
//! snapshot, the adapter registry, the bus, and the L2 and L1 facts it may read. The one
//! upward-in-the-wrong-direction call left is `Wiring::record_status`, the liveness record
//! the occupancy chart keeps in L4's store: L3 reports what it observed there until L4
//! reads it from L3 (S9).
//!
//! Page or service (D5), module by module:
//! - `agents.rs`, `roles.rs`: service.
//! - `runtime_events.rs`: **composition at the router.** A pushed runtime event is about a
//!   session that belongs to a standing agent (L3) or a run (L4), and the occupancy record
//!   is L4's, so mapping it asks both levels. It also holds `run_input`, which types at a
//!   run's session (an L4 row).
//! - `admission.rs` (new, L4): `capacity_for` and the capacity arithmetic. It was in
//!   `agents.rs` but counts runs and tasks; it reads L3's `StandingAgentLiveFact`.
//! - Scope views and runtime connections (`engine.rs`, `scope_views`/`runtime_connections`):
//!   **pages**. They compose the roster with L4's active runs, capacity and the registry.
//! - `harness_health.rs`: still `impl Engine` in this PR; D6 follows.
use crate::facts::Wiring;
use crate::state::L3State;
use factory_agents::agent::AgentSession;
use factory_core::role::Role;
use factory_kernel::L3;

pub(crate) struct L3Service<'a> {
    pub(crate) state: &'a L3State,
    pub(crate) wiring: Wiring<'a, L3>,
}

impl crate::engine::Engine {
    pub(crate) fn l3_service(&self) -> L3Service<'_> {
        L3Service {
            state: &self.l3,
            wiring: Wiring::new(self),
        }
    }
}

impl L3Service<'_> {
    /// The role the config gives an agent. An agent nobody named -- a one-off
    /// `--agent claude-code` -- is a worker.
    pub fn role_of(&self, scope: &str, name: &str) -> Role {
        let factory = self.wiring.snapshot();
        factory
            .scope(scope)
            .ok()
            .map(|s| s.agents_with(&factory.config.daemon.foreman))
            .unwrap_or_default()
            .into_iter()
            .find(|a| a.name() == name)
            .map(|a| a.role)
            .unwrap_or_default()
    }

    /// The role an agent is actually working under: the one somebody gave it
    /// if there is one, and the config's otherwise. Every answer to "what may
    /// this agent do" comes through here, so a standing agent and a run of the
    /// same agent cannot be told two different things.
    pub async fn effective_role(&self, scope: &str, name: &str) -> Role {
        let declared = self.role_of(scope, name);
        match self.state.agents.get_agent(&AgentSession::id_for(scope, name)).await {
            Ok(Some(agent)) => agent.role_with(&declared),
            _ => declared,
        }
    }
}

/// L3's provider for `EffectiveRoleFact`: the role an agent runs under right now.
pub(crate) struct RoleProvider<'a>(pub(crate) L3Service<'a>);
impl factory_kernel::FactProvider for RoleProvider<'_> {
    type Level = factory_kernel::L3;
}
#[async_trait::async_trait]
impl factory_kernel::Provide<factory_kernel::EffectiveRoleFact> for RoleProvider<'_> {
    type Query = (String, String);
    type Value = factory_kernel::EffectiveRoleFact;
    type Error = factory_core::error::FactoryError;
    async fn get(&self, (scope, name): &(String, String)) -> factory_core::error::Result<Self::Value> {
        Ok(factory_kernel::EffectiveRoleFact(self.0.effective_role(scope, name).await.as_str().to_string()))
    }
}
