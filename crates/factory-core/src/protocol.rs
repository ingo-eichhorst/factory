//! One request/response envelope, spoken by every interface adapter. The CLI
//! sends it over a unix socket, the HTTP adapter maps REST onto it; adding a
//! third interface means translating to this, not inventing a new API.

use crate::agent::AgentSession;
use crate::event::Event;
use crate::occupancy::Occupancy;
use crate::run::Run;
use crate::task::{NewTask, Task, TaskEntry, TaskFilter, TaskPatch, TaskReport};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", content = "params", rename_all = "snake_case")]
pub enum Request {
    #[serde(rename = "status")]
    Status,
    #[serde(rename = "adapters")]
    Adapters,
    /// The scopes, the agents each one declares, and what they are doing.
    #[serde(rename = "agents")]
    Agents,
    /// Bring a declared standing agent up.
    #[serde(rename = "agent.start")]
    AgentStart { scope: String, name: String },
    /// Take one down and leave it down.
    #[serde(rename = "agent.stop")]
    AgentStop { id: String },
    /// Type at a standing agent's session.
    #[serde(rename = "agent.input")]
    AgentInput {
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        keys: Vec<String>,
        id: String,
    },
    #[serde(rename = "agent.output")]
    AgentOutput {
        id: String,
        #[serde(default)]
        lines: Option<u32>,
    },
    /// Type at a task run's session, for the same reason.
    #[serde(rename = "run.input")]
    RunInput {
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        keys: Vec<String>,
        id: String,
    },
    #[serde(rename = "task.create")]
    TaskCreate(NewTask),
    #[serde(rename = "task.get")]
    TaskGet { id: String },
    #[serde(rename = "task.list")]
    TaskList(TaskFilter),
    #[serde(rename = "task.update")]
    TaskUpdate { id: String, patch: TaskPatch },
    #[serde(rename = "task.delete")]
    TaskDelete { id: String },
    #[serde(rename = "task.run")]
    TaskRun { id: String },
    #[serde(rename = "task.cancel")]
    TaskCancel { id: String },
    #[serde(rename = "task.report")]
    TaskReport { id: String, report: TaskReport },
    #[serde(rename = "task.entries")]
    TaskEntries {
        id: String,
        #[serde(default)]
        limit: Option<u32>,
    },
    /// Terminal output for the task's most recent run.
    #[serde(rename = "task.output")]
    TaskOutput {
        id: String,
        #[serde(default)]
        lines: Option<u32>,
    },
    #[serde(rename = "run.list")]
    RunList {
        task_id: String,
        #[serde(default)]
        limit: Option<u32>,
    },
    #[serde(rename = "run.get")]
    RunGet { id: String },
    #[serde(rename = "run.entries")]
    RunEntries {
        id: String,
        #[serde(default)]
        limit: Option<u32>,
    },
    /// Terminal output for one run: live from its session while it is running,
    /// and the transcript kept at the end once it is not.
    #[serde(rename = "run.output")]
    RunOutput {
        id: String,
        #[serde(default)]
        lines: Option<u32>,
    },
    /// What every bay was doing over a window: the runs that held it, what is
    /// scheduled to hold it next, and what the runtime saw in between.
    #[serde(rename = "occupancy")]
    Occupancy {
        /// How far back to look. Defaults to the last twelve hours.
        #[serde(default)]
        minutes: Option<u32>,
    },
    /// Turn this connection into an event stream. Only the socket interface
    /// answers this; HTTP uses its WebSocket instead.
    #[serde(rename = "subscribe")]
    Subscribe,
}

/// Struct variants throughout: an internally tagged enum can only carry a map,
/// and a `Tasks(Vec<Task>)` newtype would fail at serialization time rather
/// than at compile time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Payload {
    Ok,
    Status { status: StatusInfo },
    Adapters { adapters: Vec<AdapterEntry> },
    Task { task: Task },
    Tasks { tasks: Vec<Task> },
    Run { run: Run },
    Runs { runs: Vec<Run> },
    Agent { agent: AgentSession },
    Scopes { scopes: Vec<ScopeView> },
    Entries { entries: Vec<TaskEntry> },
    Text { text: String },
    Deleted { deleted: bool },
    Event { event: Event },
    Occupancy { occupancy: Occupancy },
}

/// A request plus who is making it.
///
/// The token is how an agent says which agent it is. Absent means the owner --
/// the person at the socket. That is not a security boundary: every agent runs
/// as the owner and can read the socket, so one that leaves the token out is
/// indistinguishable from a person. It keeps agents inside their role by
/// accident, not against intent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    #[serde(flatten)]
    pub request: Request,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl From<Request> for Envelope {
    fn from(request: Request) -> Self {
        Self {
            request,
            token: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ok { data: Payload },
    Error { code: String, message: String },
}

impl Response {
    pub fn ok(data: Payload) -> Self {
        Self::Ok { data }
    }

    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Error {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusInfo {
    pub instance: String,
    pub instance_id: String,
    pub root: String,
    pub version: String,
    pub uptime_seconds: u64,
    pub tasks_total: usize,
    pub tasks_active: usize,
    pub subscribers: usize,
    pub interfaces: Vec<String>,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterEntry {
    pub kind: String,
    pub name: String,
    pub description: String,
    /// `builtin` or `plugin:<manifest path>`.
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterList {
    pub adapters: Vec<AdapterEntry>,
}

impl From<AdapterList> for Payload {
    fn from(list: AdapterList) -> Self {
        Payload::Adapters { adapters: list.adapters }
    }
}

/// One thing an agent is doing right now.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentActivity {
    pub run_id: String,
    pub task_id: String,
    pub task_title: String,
    pub scope: String,
    pub attempt: u32,
    pub status: String,
    pub runtime: String,
    pub trigger: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// The runtime's handle for the session, so a person can find the window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

/// One agent in one scope, as the agents page shows it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentView {
    /// The standing agent's session id, when it is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// What it is called in this scope.
    pub name: String,
    /// The adapter behind it.
    pub adapter: String,
    pub description: String,
    /// `builtin` or `plugin:<manifest path>`.
    pub source: String,
    /// `permanent`, `temporary`, or `task`.
    pub lifetime: String,
    /// `worker` or `foreman`.
    pub role: String,
    pub autostart: bool,
    /// For a standing agent: `starting`, `ready`, `gone`, `stopped`.
    /// For a task agent: `task`.
    pub state: String,
    /// True when tasks in this scope use it unless they say otherwise.
    pub is_default: bool,
    /// False once the config stops declaring it.
    pub declared: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attach: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Task runs this agent is working on in this scope right now.
    pub active: Vec<AgentActivity>,
}

/// A scope and everything that runs in it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeView {
    pub name: String,
    pub path: String,
    /// The adapter a task here runs on unless it says otherwise.
    pub default_agent: String,
    pub runtime: String,
    pub agents: Vec<AgentView>,
    /// Every agent adapter registered, so a task can be started with any of
    /// them regardless of what the scope declares.
    pub available: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // `flatten` over an adjacently tagged enum routes through a content buffer,
    // which is a rough edge in serde. Prove it round-trips before anything is
    // built on top of it.
    #[test]
    fn an_envelope_carries_a_request_and_a_token() {
        let json = r#"{"op":"task.list","params":{},"token":"abc"}"#;
        let env: Envelope = serde_json::from_str(json).expect("envelope parses");
        assert_eq!(env.token.as_deref(), Some("abc"));
        assert!(matches!(env.request, Request::TaskList(_)));

        let back = serde_json::to_string(&env).unwrap();
        let again: Envelope = serde_json::from_str(&back).unwrap();
        assert_eq!(again.token.as_deref(), Some("abc"));
        assert!(matches!(again.request, Request::TaskList(_)));
    }

    #[test]
    fn a_request_without_a_token_still_parses() {
        let env: Envelope = serde_json::from_str(r#"{"op":"status"}"#).expect("no token is fine");
        assert!(env.token.is_none());
        assert!(matches!(env.request, Request::Status));
    }

    #[test]
    fn a_request_with_params_survives_the_flatten() {
        let json = r#"{"op":"task.get","params":{"id":"t1"},"token":"t"}"#;
        let env: Envelope = serde_json::from_str(json).unwrap();
        match env.request {
            Request::TaskGet { id } => assert_eq!(id, "t1"),
            other => panic!("wrong request: {other:?}"),
        }
    }
}
