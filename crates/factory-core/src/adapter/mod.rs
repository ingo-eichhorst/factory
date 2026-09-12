//! The four seams. Each is a trait the daemon depends on and knows nothing
//! behind: a built-in adapter and an out-of-process plugin are the same thing
//! to the caller.

pub mod agent;
pub mod interface;
pub mod runtime;
pub mod store;

pub use agent::{Agent, AgentContext, LaunchKind, LaunchSpec, TaskBinding};
pub use interface::{Interface, InterfaceContext};
pub use runtime::{
    AgentRuntime, RuntimeConnectionDiagnostic, RuntimeConnectionState, RuntimeEvent,
    RuntimeEventKind, RuntimeEventStream, RuntimePeer, RuntimeStatus, Screen, StartRequest,
    StatusReport, StatusSource,
};
pub use store::TaskStore;

/// Which seam an adapter plugs into. A plugin manifest names one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterKind {
    Agent,
    Runtime,
    Task,
    Interface,
}

impl AdapterKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Runtime => "runtime",
            Self::Task => "task",
            Self::Interface => "interface",
        }
    }
}

impl std::fmt::Display for AdapterKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
