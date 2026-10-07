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
