//! The vocabulary every part of Factory shares: the domain, the event stream,
//! the wire protocol, and the four adapter traits. Nothing here knows about
//! sqlite, herdr, axum, or any other concrete choice.

pub mod adapter;
pub mod config;
pub mod error;
pub mod event;
pub mod protocol;
pub mod task;

pub use adapter::{Agent, AdapterKind, AgentRuntime, Interface, TaskStore};
pub use config::{Config, Factory, Scope};
pub use error::{FactoryError, Result};
pub use event::{Event, EventBus};
pub use task::{
    NewTask, Schedule, SessionRef, Task, TaskEntry, TaskFilter, TaskPatch, TaskReport, TaskStatus,
};

/// A short, unguessable string for a task's callback token. Not cryptographic
/// identity -- it only has to stop one running agent from closing another's
/// task by mistake.
pub fn new_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}
