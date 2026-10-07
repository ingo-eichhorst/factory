//! L3's handle on the standing-agent rows: the four agent methods of the task
//! store, and nothing else of it (#193 phase 6, D3). The rows stay in the same
//! `agent_sessions` table behind the `TaskStore` adapter; this only narrows what
//! the Agent level can reach.
use factory_agents::agent::AgentSession;
use factory_agents::store::AgentStore;
use factory_core::adapter::TaskStore;
use factory_core::error::Result;
use std::sync::Arc;

pub(crate) struct AgentRows(pub(crate) Arc<dyn TaskStore>);

#[async_trait::async_trait]
impl AgentStore for AgentRows {
    async fn put_agent(&self, agent: &AgentSession) -> Result<()> {
        self.0.put_agent(agent).await
    }
    async fn get_agent(&self, id: &str) -> Result<Option<AgentSession>> {
        self.0.get_agent(id).await
    }
    async fn agents(&self) -> Result<Vec<AgentSession>> {
        self.0.agents().await
    }
    async fn delete_agent(&self, id: &str) -> Result<bool> {
        self.0.delete_agent(id).await
    }
}

#[cfg(test)]
mod tests {
    //! The handle is a window onto the rows the task store already holds: the same
    //! table, so an existing database needs no migration and a restart sees what
    //! either side wrote.
    use super::*;
    use factory_agents::agent::Lifetime;
    use factory_core::role::Role;
    use factory_plugins::SqliteStore;

    fn agent(scope: &str, name: &str) -> AgentSession {
        let mut agent = AgentSession::new(scope, name, "shell", "herdr", Lifetime::Permanent, Role::worker());
        agent.token = Some(format!("token-{name}"));
        agent.assigned_role = Some(Role::new("limited"));
        agent.error = Some("kept".into());
        agent
    }

    #[tokio::test]
    async fn rows_written_before_this_change_are_read_back_and_written_to_the_same_table() {
        let dir = std::env::temp_dir().join(format!("factory-agent-rows-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("factory.sqlite");
        // What an existing instance has: rows written through the task store.
        let before = {
            let store = SqliteStore::open(&db).unwrap();
            for (scope, name) in [("projects/demo", "watcher"), ("root", "foreman"), ("projects/demo", "reviewer")] {
                store.put_agent(&agent(scope, name)).await.unwrap();
            }
            store.agents().await.unwrap()
        };
        assert_eq!(before.len(), 3);
        // A restart on the new build: the handle sees every row, unchanged.
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open(&db).unwrap());
        let rows = AgentRows(store.clone());
        let seen = rows.agents().await.unwrap();
        assert_eq!(serde_json::to_value(&seen).unwrap(), serde_json::to_value(&before).unwrap());
        for want in &before {
            let got = rows.get_agent(&want.id).await.unwrap().expect("the row");
            assert_eq!(serde_json::to_value(&got).unwrap(), serde_json::to_value(want).unwrap());
        }
        // A write through the handle is the row the task store reads (and the reverse).
        let mut changed = before[0].clone();
        changed.error = Some("changed through L3".into());
        rows.put_agent(&changed).await.unwrap();
        assert_eq!(store.get_agent(&changed.id).await.unwrap().unwrap().error.as_deref(), Some("changed through L3"));
        store.put_agent(&agent("projects/demo", "late")).await.unwrap();
        assert!(rows.get_agent("projects/demo/late").await.unwrap().is_some());
        assert!(rows.delete_agent("projects/demo/late").await.unwrap());
        assert!(store.get_agent("projects/demo/late").await.unwrap().is_none());
        assert!(!rows.delete_agent("projects/demo/late").await.unwrap(), "deleting twice says so");
        // Another restart: the survivors are exactly the rows left.
        drop(rows);
        drop(store);
        let reopened = SqliteStore::open(&db).unwrap();
        assert_eq!(reopened.agents().await.unwrap().len(), 3);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
