//! The vocabulary every part of Factory shares: the domain, the event stream,
//! the wire protocol, and the five adapter traits. Nothing here knows about
//! sqlite, herdr, axum, or any other concrete choice.

pub mod adapter;
pub mod agent;
pub mod backup;
pub mod bench;
pub mod benchmark;
pub mod building;
pub mod config;
pub mod conformance;
pub mod control_plan;
pub mod dashboard;
pub mod dataset;
pub mod dependencies;
pub mod error;
pub mod event;
pub mod goals;
pub mod harness;
pub mod intake;
pub mod knowledge;
pub mod metrics;
pub mod occupancy;
pub mod operations;
pub mod policy;
pub mod policy_export;
pub mod protocol;
pub mod quality;
pub mod ready;
pub mod reporting_clock;
pub mod role;
pub mod run;
pub mod scenario;
pub mod task;
pub mod usage;
pub mod workflow;

pub use adapter::{Agent, AdapterKind, AgentRuntime, Interface, TaskStore};
pub use config::{Config, Factory, Scope};
pub use error::{FactoryError, Result};
pub use agent::{AgentSession, AgentState, Lifetime};
pub use role::{Grant, Reach, Role, RoleDef, RoleSpec, Roles};
pub use event::{Event, EventBus};
pub use run::{BlockSource, FailKind, NewRun, Run, RunPatch, RunStatus, Trigger};
pub use task::{
    CostEstimateRange, Estimate, NewTask, PendingRetry, RetryPolicy, Schedule, SessionRef, Task,
    TaskEntry, TaskFilter, TaskPatch, TaskReport, TaskStatus, TimeEstimateRange, WorkflowOrigin,
};
pub use workflow::{
    CanvasPoint, WorkflowDefinition, WorkflowDraft, WorkflowEdge, WorkflowNode, WorkflowNodeKind,
    WorkflowNodeRun, WorkflowNodeStatus, WorkflowRun, WorkflowRunStatus,
};

/// A short, unguessable string for a task's callback token. Not cryptographic
/// identity -- it only has to stop one running agent from closing another's
/// task by mistake.
pub fn new_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}
