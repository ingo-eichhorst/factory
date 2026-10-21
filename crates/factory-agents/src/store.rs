//! L3's standing-agent rows (#193 phase 6, D3).
//!
//! A standing agent's session is written whole: there are few of them, they change
//! rarely, and a partial update has no meaning for something whose whole state is
//! "is it up". L3 holds this trait, not the Process level's whole task store, so
//! agent code can read and write its own rows and nothing of L4's.
//!
//! The rows live in the same table they always did (`agent_sessions`, behind the
//! `TaskStore` adapter's four agent methods); nothing is copied or migrated. The
//! adapter seam is unchanged: whoever supplies the task store supplies these rows
//! through the daemon's `AgentRows` handle.
use crate::agent::AgentSession;
use factory_kernel::Result;

#[async_trait::async_trait]
pub trait AgentStore: Send + Sync {
    /// Written whole, keyed by `AgentSession::id`.
    async fn put_agent(&self, agent: &AgentSession) -> Result<()>;
    async fn get_agent(&self, id: &str) -> Result<Option<AgentSession>>;
    async fn agents(&self) -> Result<Vec<AgentSession>>;
    async fn delete_agent(&self, id: &str) -> Result<bool>;
}

/// L3's provider for `StandingAgentLiveFact`: how many of one standing agent's
/// instances are live, read fresh from its own rows (never cached, never inferred
/// from a screen). Every row of the declaration counts, a surplus instance still
/// draining after a scale-down included: it holds a session until it stops.
pub struct LiveProvider {
    pub agents: std::sync::Arc<dyn AgentStore>,
}
impl factory_kernel::FactProvider for LiveProvider {
    type Level = factory_kernel::L3;
}
#[async_trait::async_trait]
impl factory_kernel::Provide<factory_kernel::StandingAgentLiveFact> for LiveProvider {
    /// `(scope, agent name)`, the canonical scope as the caller knows it.
    type Query = (String, String);
    type Value = factory_kernel::StandingAgentLiveFact;
    type Error = factory_kernel::FactoryError;
    async fn get(&self, query: &(String, String)) -> Result<Self::Value> {
        let rows = self.agents.agents().await.unwrap_or_default();
        let live = rows
            .iter()
            .filter(|agent| agent.scope == query.0 && agent.name == query.1 && agent.state.is_live())
            .count();
        Ok(factory_kernel::StandingAgentLiveFact { live: u32::try_from(live).unwrap_or(u32::MAX) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentState, Lifetime};
    use crate::role::Role;
    use factory_kernel::{Facts, StandingAgentLiveFact, L4};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Rows(Mutex<Vec<AgentSession>>);
    #[async_trait::async_trait]
    impl AgentStore for Rows {
        async fn put_agent(&self, agent: &AgentSession) -> Result<()> {
            let mut rows = self.0.lock().unwrap();
            rows.retain(|row| row.id != agent.id);
            rows.push(agent.clone());
            Ok(())
        }
        async fn get_agent(&self, id: &str) -> Result<Option<AgentSession>> {
            Ok(self.0.lock().unwrap().iter().find(|row| row.id == id).cloned())
        }
        async fn agents(&self) -> Result<Vec<AgentSession>> {
            Ok(self.0.lock().unwrap().clone())
        }
        async fn delete_agent(&self, id: &str) -> Result<bool> {
            let mut rows = self.0.lock().unwrap();
            let before = rows.len();
            rows.retain(|row| row.id != id);
            Ok(rows.len() != before)
        }
    }

    async fn live(provider: &LiveProvider, scope: &str, name: &str) -> u32 {
        Facts::<L4>::new()
            .get::<StandingAgentLiveFact, _>(provider, &(scope.to_string(), name.to_string()))
            .await
            .unwrap()
            .live
    }

    #[tokio::test]
    async fn a_standing_agent_is_live_only_while_its_row_says_so_and_an_l4_reader_may_ask() {
        let rows = Arc::new(Rows::default());
        let provider = LiveProvider { agents: rows.clone() };
        assert_eq!(live(&provider, "demo", "watcher").await, 0, "no row reads not live");
        let mut agent = AgentSession::new("demo", "watcher", "shell", "herdr", Lifetime::Permanent, Role::worker());
        agent.state = AgentState::Ready;
        rows.put_agent(&agent).await.unwrap();
        assert_eq!(live(&provider, "demo", "watcher").await, 1);
        assert_eq!(live(&provider, "demo", "other").await, 0);
        agent.state = AgentState::Stopped;
        rows.put_agent(&agent).await.unwrap();
        assert_eq!(live(&provider, "demo", "watcher").await, 0, "a stopped agent is not live");
    }

    #[tokio::test]
    async fn every_live_instance_of_the_declaration_counts_and_nothing_else_does() {
        let rows = Arc::new(Rows::default());
        let provider = LiveProvider { agents: rows.clone() };
        let base = AgentSession::new("demo", "watcher", "shell", "herdr", Lifetime::Permanent, Role::worker());
        for (instance, state) in [(1, AgentState::Ready), (2, AgentState::Starting), (3, AgentState::Gone)] {
            let mut row = base.clone().as_instance(instance);
            row.state = state;
            rows.put_agent(&row).await.unwrap();
        }
        // Another scope's agent of the same name is somebody else's slot.
        let mut elsewhere = AgentSession::new("other", "watcher", "shell", "herdr", Lifetime::Permanent, Role::worker());
        elsewhere.state = AgentState::Ready;
        rows.put_agent(&elsewhere).await.unwrap();
        assert_eq!(live(&provider, "demo", "watcher").await, 2, "#1 and #2 are live, #3 is gone");
    }
}
