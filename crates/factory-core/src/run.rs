//! A run is one attempt at a task.
//!
//! The task is the standing intent -- what to do, where, with which agent. The
//! run is what actually happened on one occasion: which session it opened, what
//! it reported, when it ended. A task that failed and is started again has two
//! runs, and both are kept.

use crate::task::SessionRef;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Where a run is. Unlike a task, a run is never `pending`: it exists because
/// something started it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// The session is opening and the agent is being handed the task.
    Dispatching,
    /// The agent said it is working.
    Running,
    /// The agent needs a human.
    Blocked,
    Done,
    Failed,
    Cancelled,
}

impl RunStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dispatching => "dispatching",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// The task's own status while this is its most recent run.
    pub fn as_task_status(self) -> crate::task::TaskStatus {
        use crate::task::TaskStatus as T;
        match self {
            Self::Dispatching => T::Dispatching,
            Self::Running => T::Running,
            Self::Blocked => T::Blocked,
            Self::Done => T::Done,
            Self::Failed => T::Failed,
            Self::Cancelled => T::Cancelled,
        }
    }
}

impl std::str::FromStr for RunStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "dispatching" => Self::Dispatching,
            "running" => Self::Running,
            "blocked" => Self::Blocked,
            "done" => Self::Done,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            other => return Err(format!("unknown run status: {other}")),
        })
    }
}

/// What started this run. Worth keeping: "it failed" reads differently for a
/// run somebody asked for and one a schedule fired at 3am.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    Manual,
    Schedule,
    Agent,
}

impl Trigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Schedule => "schedule",
            Self::Agent => "agent",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub task_id: String,
    /// 1 for the first run of a task, 2 for the retry after it failed, and so
    /// on. Assigned by the store, so two dispatches landing at once cannot
    /// claim the same number.
    pub attempt: u32,
    pub status: RunStatus,
    pub trigger: Trigger,
    /// What the task asked for -- an agent the scope declares, or an adapter
    /// name. Copied at dispatch: a task whose agent is changed later must not
    /// rewrite what an old run actually did.
    pub agent: String,
    /// The adapter that actually ran, once the name above was resolved.
    #[serde(default)]
    pub adapter: String,
    pub runtime: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionRef>,
    /// The secret the agent presents when reporting on this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
}

impl Run {
    /// The view that leaves the daemon. The token is the one field a run's own
    /// observers must not see -- it is what stops one agent closing another's.
    pub fn redacted(&self) -> Run {
        let mut r = self.clone();
        r.token = None;
        r
    }
}

/// What the engine hands the store. Everything the store owns -- the id, the
/// attempt number, the clock -- is filled in there.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewRun {
    pub task_id: String,
    pub trigger: Trigger,
    pub agent: String,
    pub adapter: String,
    pub runtime: String,
    pub token: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<RunStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionRef>,
    /// `None` means "leave alone" everywhere else here, so letting go of a
    /// session needs a field of its own.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_session: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// A finished run has no more use for its token.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_token: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
}
