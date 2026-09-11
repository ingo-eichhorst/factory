//! Standing agents: the ones that are not a task.
//!
//! A task agent exists for the length of one run. A permanent agent just runs,
//! idle or not, because somebody wants it there -- an assistant to talk to, a
//! watcher, a session kept warm. It is declared in the instance config and the
//! daemon keeps it alive; it is never failed for being quiet, because being
//! quiet is what it is for.

use crate::task::SessionRef;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// How long an agent is meant to stick around.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifetime {
    /// Started with the daemon and restarted if its session dies.
    Permanent,
    /// The same thing, but only when somebody asks for it. Nothing starts or
    /// restarts it on its own.
    Temporary,
    /// Not a standing agent at all: offered for tasks in this scope.
    Task,
}

impl Default for Lifetime {
    fn default() -> Self {
        Self::Task
    }
}

impl Lifetime {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Permanent => "permanent",
            Self::Temporary => "temporary",
            Self::Task => "task",
        }
    }

    /// A standing agent is one that exists between tasks.
    pub fn is_standing(self) -> bool {
        matches!(self, Self::Permanent | Self::Temporary)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// A session is being opened.
    Starting,
    /// It is up.
    Ready,
    /// Its session went away without anybody asking.
    Gone,
    /// Somebody stopped it, and nothing will start it again on its own.
    Stopped,
}

impl AgentState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Gone => "gone",
            Self::Stopped => "stopped",
        }
    }

    pub fn is_live(self) -> bool {
        matches!(self, Self::Starting | Self::Ready)
    }
}

/// A standing agent as the daemon currently knows it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSession {
    /// `<scope>/<name>`. Deterministic, so reconciling against the config after
    /// a restart is a lookup rather than a search.
    pub id: String,
    pub name: String,
    pub scope: String,
    /// The agent adapter.
    pub agent: String,
    pub runtime: String,
    pub lifetime: Lifetime,
    pub state: AgentState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionRef>,
    /// The command a person types to get into this agent's terminal, when the
    /// runtime can name one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attach: Option<String>,
    /// False once the config stops declaring it: still here, no longer wanted.
    #[serde(default = "yes")]
    pub declared: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

fn yes() -> bool {
    true
}

impl AgentSession {
    pub fn id_for(scope: &str, name: &str) -> String {
        format!("{scope}/{name}")
    }

    pub fn new(scope: &str, name: &str, agent: &str, runtime: &str, lifetime: Lifetime) -> Self {
        let now = Utc::now();
        Self {
            id: Self::id_for(scope, name),
            name: name.to_string(),
            scope: scope.to_string(),
            agent: agent.to_string(),
            runtime: runtime.to_string(),
            lifetime,
            state: AgentState::Stopped,
            session: None,
            attach: None,
            declared: true,
            error: None,
            started_at: now,
            last_seen_at: now,
        }
    }
}
