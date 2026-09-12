//! One request/response envelope, spoken by every interface adapter. The CLI
//! sends it over a unix socket, the HTTP adapter maps REST onto it; adding a
//! third interface means translating to this, not inventing a new API.

use crate::adapter::Screen;
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
    /// Give a standing agent a role, or take the given one away and let the
    /// config decide again. The owner's to do, and nobody else's.
    #[serde(rename = "agent.role")]
    AgentRole {
        id: String,
        /// `None` clears an assignment rather than naming one.
        #[serde(default)]
        role: Option<String>,
    },
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
    /// One frame of a standing agent's screen, for a viewer that wants the
    /// terminal rather than the transcript.
    #[serde(rename = "agent.screen")]
    AgentScreen { id: String },
    /// The same for a task run's session.
    #[serde(rename = "run.screen")]
    RunScreen { id: String },
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
    /// How big each scope is on disk, for the site plan's hall sizes. Nothing
    /// else needs this, which is why it is its own request rather than a field
    /// every `agents` call would have to pay for.
    #[serde(rename = "site.footprint")]
    SiteFootprint,
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
    Scopes {
        scopes: Vec<ScopeView>,
        /// Every role this instance knows, so a picker can offer them.
        #[serde(default)]
        roles: Vec<RoleView>,
    },
    Entries { entries: Vec<TaskEntry> },
    Text { text: String },
    Deleted { deleted: bool },
    Event { event: Event },
    Occupancy { occupancy: Occupancy },
    Screen { screen: Screen },
    SiteFootprint { footprint: SiteFootprint },
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
    /// The role it is working under -- the config's, unless somebody gave it
    /// another.
    pub role: String,
    /// Set when a person gave it this role, so the roster can say that the
    /// config says something else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_role: Option<String>,
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

/// One role, as the roster and the pickers show it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleView {
    pub name: String,
    pub describe: String,
    /// What it may do, written the way the config writes it.
    pub grants: Vec<String>,
    /// `own` or `scope`.
    pub reach: String,
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

/// How big a scope is on disk, for the site plan's hall footprint. The
/// prototype this is ported from sized a hall as `2.6 + sqrt(k) * 0.85`
/// where `k` is the codebase's megabytes; this is where `k` comes from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeFootprint {
    pub name: String,
    /// `None` when the scope's directory could not be read -- gone, or a
    /// permission the daemon does not have. A hall with no footprint is drawn
    /// at a default size and says so, rather than claiming a number nobody
    /// measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// The scope's top-level entries -- a directory, or every loose file in
    /// the root as one entry -- largest first, for the site plan to treemap
    /// onto the hall's floor. Empty both when `size_bytes` is `None` (the
    /// scope could not be walked, so there is nothing to show) and when it is
    /// genuinely `Some(0)` (walked, and there was nothing there): the two
    /// stay tellable apart by `size_bytes` alone, the way they already were
    /// before this field existed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub areas: Vec<ScopeArea>,
    /// Whether the walk that produced `size_bytes` and `areas` stopped at
    /// `ENTRY_CAP` before it finished the tree. A truncated walk's numbers
    /// are a lower bound, not a measurement -- enough to size a hall next to
    /// its neighbours, not enough for the floor's proportions to be trusted,
    /// so the hall has to say "partial" rather than draw them as exact.
    pub truncated: bool,
}

/// One top-level entry of a scope's root, as the floor treemap draws it. A
/// directory keeps its own name; every file lying loose in the root -- a
/// `Cargo.toml`, a `README.md` -- is one entry rather than one per file, and
/// is named for what it is rather than invented as a directory that does not
/// exist.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeArea {
    pub name: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteFootprint {
    pub scopes: Vec<ScopeFootprint>,
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
