use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Where a task is in its life. The agent moves it through `Running` ->
/// `Done`/`Failed`/`Blocked` by calling back; nothing infers completion from a
/// terminal's appearance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Created, waiting for a trigger (manual, schedule, or another agent).
    Pending,
    /// The daemon has a runtime session and is handing the prompt over.
    Dispatching,
    /// The agent acknowledged the task and is working on it.
    Running,
    /// The agent needs a human before it can go on.
    Blocked,
    Done,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Dispatching => "dispatching",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl std::str::FromStr for TaskStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "pending" => Self::Pending,
            "dispatching" => Self::Dispatching,
            "running" => Self::Running,
            "blocked" => Self::Blocked,
            "done" => Self::Done,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            other => return Err(format!("unknown status: {other}")),
        })
    }
}

/// When a task fires on its own. `Cron` is a five- or six-field expression;
/// `Every` is a plain interval for the common "every N minutes" case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Schedule {
    Cron(String),
    Every { seconds: u64 },
}

/// The identity of a live agent session, as the runtime adapter that created it
/// understands it. The daemon treats `handle` as opaque.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRef {
    pub runtime: String,
    pub handle: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub meta: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub instructions: String,
    /// Name of the scope (working directory) the task runs in.
    pub scope: String,
    /// The agent this task runs as: the name of an agent the scope declares
    /// (`assistant`, `scratch`), or an adapter name (`pi`, `claude-code`) for
    /// one the scope does not name. The adapter behind it is resolved at
    /// dispatch, so renaming a harness in the config does not rewrite history.
    pub agent: String,
    /// Name of the agent-runtime adapter.
    pub runtime: String,
    pub status: TaskStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<Schedule>,
    /// How long one run is expected to occupy its agent. Advisory only: this
    /// never stops a run or changes its status (`timeout_seconds` does that).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_seconds: Option<u64>,
    /// The most recent run's outcome, mirrored so a list does not have to read
    /// every run. `Run` is where it actually lives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// How many times this task has been run.
    #[serde(default)]
    pub runs: u32,
    /// How long this task's agent has to acknowledge a run, and how long the
    /// whole run may take. `None` uses the instance's defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ack_timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    /// How long a run of this task may sit `Blocked` before the daemon stops
    /// waiting for a human to answer it. `None` uses the instance's default
    /// (`DaemonConfig::blocked_timeout_seconds`). Kept apart from
    /// `timeout_seconds` on purpose: that field is a cap on the run's total
    /// duration, and a blocked run waiting on a human wants a separate,
    /// longer clock than that -- one that gives a person a real chance to
    /// see it and answer before the daemon gives up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<DateTime<Utc>>,
    /// Whether a run of this task works in a git worktree of its own rather
    /// than the scope. `#[serde(default)]` reads a missing key as `false` --
    /// deliberately the opposite of `NewTask::worktree`'s absent-means-on,
    /// because the field is new and a database full of tasks that have been
    /// running against their scope for weeks must not all move to a worktree
    /// on the next restart just because nobody wrote this key down yet. Only
    /// tasks created after this shipped carry the field explicitly.
    #[serde(default)]
    pub worktree: bool,
    /// The workflow attempt that created this task, when there is one. This is
    /// stored with the task so adapters preserve the relationship without
    /// parsing labels or titles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_origin: Option<WorkflowOrigin>,
    /// The bench attempt that created this task, when there is one. Follows
    /// `workflow_origin`'s own shape and reason for existing: `#[serde(default)]`
    /// reads a task written before this field existed as `None`, and a task
    /// carries at most one of the two origins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bench_origin: Option<crate::bench::BenchOrigin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowOrigin {
    pub workflow_id: String,
    pub workflow_run_id: String,
    pub node_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NewTask {
    pub title: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<Schedule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ack_timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    /// Give this task its own git worktree, made fresh before each run.
    /// Absent means on, so every way of making a task -- the form, `factory
    /// task create`, the HTTP API, another agent -- gets the same default
    /// without having to say so. `Some(false)` is how a caller means it, not
    /// merely fails to mention it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<bool>,
}

/// A partial update. `None` means "leave alone" throughout, so a store can
/// apply one without reading the task first.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<Schedule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// `None` means "leave alone" here, so a run that succeeded needs a way to
    /// say the previous attempt's error no longer describes this task.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_result: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_error: bool,
    /// Set when a run is created, so `runs` counts without a second query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runs: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ack_timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_timeout_seconds: Option<u64>,
    /// `None` means "leave alone", so going back to the instance default,
    /// dropping an estimate, and dropping a schedule each need a field that
    /// can say so.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_ack_timeout: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_timeout: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_blocked_timeout: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_estimate: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_schedule: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// One line in a task's journal: what happened, when, and who said so.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskEntry {
    pub at: DateTime<Utc>,
    /// "daemon", "scheduler", "agent", or an adapter name.
    pub source: String,
    pub kind: String,
    pub message: String,
    /// The run this happened during. `None` for things that are true of the
    /// task itself, like its creation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl TaskEntry {
    pub fn new(source: impl Into<String>, kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            at: Utc::now(),
            source: source.into(),
            kind: kind.into(),
            message: message.into(),
            run_id: None,
            data: None,
        }
    }

    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }

    pub fn in_run(mut self, run_id: impl Into<String>) -> Self {
        self.run_id = Some(run_id.into());
        self
    }
}

/// What an agent sends back while it works. This is the contract named in every
/// dispatched prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskReport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<crate::run::RunStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Presented by the agent, checked against `Task::token`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_task_with_no_worktree_key_parses_as_absent_not_off() {
        // The wire says nothing either way; `unwrap_or(true)` at creation is
        // what turns this into "on". Confirming it deserializes to `None`
        // rather than `Some(false)` is what makes that later default honest.
        let new: NewTask = serde_json::from_str(r#"{"title":"do the thing"}"#).unwrap();
        assert_eq!(new.worktree, None);
    }

    #[test]
    fn a_stored_task_with_no_worktree_key_reads_as_off() {
        // A row written before this field existed. `#[serde(default)]` must
        // land on `false` here, not on the same "absent means on" `NewTask`
        // uses -- otherwise every task already in the database would move
        // into a worktree the next time the daemon starts.
        let json = r#"{
            "id": "t1", "title": "an old task", "instructions": "", "scope": "demo",
            "agent": "shell", "runtime": "herdr", "status": "pending",
            "created_at": "2024-01-01T00:00:00Z", "updated_at": "2024-01-01T00:00:00Z"
        }"#;
        let task: Task = serde_json::from_str(json).unwrap();
        assert!(!task.worktree);
        assert_eq!(task.estimate_seconds, None, "old tasks remain unestimated");
    }

    #[test]
    fn estimate_and_clear_estimate_have_distinct_patch_spellings() {
        let set: TaskPatch = serde_json::from_str(r#"{"estimate_seconds":900}"#).unwrap();
        assert_eq!(set.estimate_seconds, Some(900));
        assert!(!set.clear_estimate);

        let clear: TaskPatch = serde_json::from_str(r#"{"clear_estimate":true}"#).unwrap();
        assert_eq!(clear.estimate_seconds, None);
        assert!(clear.clear_estimate);
    }
}
