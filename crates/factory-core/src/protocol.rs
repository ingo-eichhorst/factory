//! One request/response envelope, spoken by every interface adapter. The CLI
//! sends it over a unix socket, the HTTP adapter maps REST onto it; adding a
//! third interface means translating to this, not inventing a new API.

use crate::event::Event;
use crate::task::{NewTask, Task, TaskEntry, TaskFilter, TaskPatch, TaskReport};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", content = "params", rename_all = "snake_case")]
pub enum Request {
    #[serde(rename = "status")]
    Status,
    #[serde(rename = "adapters")]
    Adapters,
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
    #[serde(rename = "task.output")]
    TaskOutput {
        id: String,
        #[serde(default)]
        lines: Option<u32>,
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
    Entries { entries: Vec<TaskEntry> },
    Text { text: String },
    Deleted { deleted: bool },
    Event { event: Event },
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
