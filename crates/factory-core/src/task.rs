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

/// How a scheduled task's run responds to failure. `Task::retry` overrides
/// the daemon's own default (`DaemonConfig::default_retry`) when set --
/// `None` there means "use the default", which is a different thing from
/// `RetryPolicy::None` spelled out on a task: the explicit form is how a
/// task says a re-run would be actively harmful (a payment run, anything
/// non-idempotent) and none should ever be attempted automatically, no
/// matter what the instance's default is.
///
/// Deliberately a flat backoff rather than a curve that grows with each
/// attempt: a scheduled task's own firing is already the natural ceiling --
/// retries exist to ride out a transient failure before the next real
/// slot, not to explore how long the daemon is willing to keep trying --
/// and a flat number is one fewer thing to explain in `factory task show`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryPolicy {
    /// A failed run displaces nothing: the task sits `pending`, showing that
    /// run's error, until its next regular firing.
    None,
    Backoff {
        /// How many retries this failure streak may queue, on top of the
        /// attempt that just failed.
        max_attempts: u32,
        /// How long to wait before each retry.
        backoff_seconds: u64,
    },
}

/// A retry queued after a scheduled task's run failed, waiting for its
/// backoff to elapse. Exists only for the life of a failure streak: the
/// moment a retry succeeds, is cancelled, or the policy's `max_attempts` is
/// used up, it is cleared. This is exactly the mirror `AGENTS.md` warns
/// about -- a copy kept on the task so the scheduler does not have to read
/// a run to know whether the next due firing is a retry or the schedule's
/// own -- and a stale copy left behind after a successful retry would
/// misreport a task that just succeeded as still mid-retry, or let a later,
/// unrelated failure resume counting from someone else's streak instead of
/// starting its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingRetry {
    /// How many retries this streak has already queued -- 1 right after the
    /// first one is scheduled, compared against the policy's `max_attempts`
    /// to know whether another is allowed.
    pub attempts: u32,
    /// The regular firing this streak is standing in front of. Captured once,
    /// when the streak began, from `next_run_at` as `advance_schedule` had
    /// already left it -- the *next* regular slot, since it runs before
    /// dispatch -- and restored to `next_run_at` the moment the streak ends,
    /// so a retry can never permanently displace the schedule.
    pub resume_at: DateTime<Utc>,
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
    /// This task's own retry policy for a scheduled run that fails. `None`
    /// means "use the daemon's default" -- see `RetryPolicy`'s own comment
    /// for why that is not the same as `Some(RetryPolicy::None)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryPolicy>,
    /// A retry queued and waiting on its backoff, if this failure streak has
    /// one. `None` the rest of the time, including right after a regular
    /// firing -- see `PendingRetry`'s own comment on why it must never
    /// outlive the streak it describes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_retry: Option<PendingRetry>,
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
    /// This task's own retry policy, overriding the daemon's default. Absent
    /// means "use the default" -- see `RetryPolicy`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryPolicy>,
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
    /// This task's own retry policy. `Some(RetryPolicy::None)` is a real
    /// value -- "never retry" -- distinct from leaving this patch field
    /// `None`, which means "leave alone"; going back to the daemon's default
    /// needs `clear_retry` instead, the same three-way shape `schedule` and
    /// `clear_schedule` already use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryPolicy>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_retry: bool,
    /// Set by the engine when a scheduled run fails and its policy allows
    /// another attempt, or cleared once the streak ends -- see
    /// `PendingRetry`. Not something a caller outside the engine has reason
    /// to set directly, but it is an ordinary mirrored field like `result`
    /// and `error`, so it gets the same `Option`-plus-`clear` shape they do
    /// rather than a special case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_retry: Option<PendingRetry>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_pending_retry: bool,
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

/// Which lifecycle event a harness fired when an agent's turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnEndEvent {
    /// The turn finished normally -- Claude Code's `Stop`.
    Stop,
    /// The turn was cut short by an API error -- Claude Code's
    /// `StopFailure`. This is how a turn killed mid-response by a host
    /// sleep ends (issue #62).
    StopFailure,
}

/// What a harness's own lifecycle hook says when an agent's turn ends: not
/// the agent's report, and never mistaken for one -- the whole point is that
/// the agent may not have sent one. It is the harness speaking, which is the
/// exception `AGENTS.md` allows; the daemon decides what, if anything, it
/// means for the run (see `occupancy::hook_turn_ended_action`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnEnded {
    pub event: TurnEndEvent,
    /// Work the harness will wake the agent up for without anyone typing:
    /// background tasks still running and session-scoped crons. A turn that
    /// ends with any of this pending has paused, not finished.
    #[serde(default)]
    pub pending_background: u32,
    /// `StopFailure`'s error type (`rate_limit`, `server_error`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// `StopFailure`'s human-readable message from the API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_details: Option<String>,
    /// The tail of the agent's last message, so whoever looks at the failed
    /// run can see where it stopped without opening the transcript.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message: Option<String>,
    /// The run's token, checked exactly as `TaskReport::token` is.
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
        assert_eq!(task.retry, None, "an old row names no policy of its own -- the daemon default applies");
        assert_eq!(
            task.pending_retry, None,
            "an old row predates retries entirely, so it is certainly not mid-streak"
        );
    }

    /// `RetryPolicy::None` has to round-trip as the bare string `retry: none`
    /// -- the exact spelling the issue this exists for asks for, and what
    /// `factory-cli`'s own `parse_retry` accepts.
    #[test]
    fn retry_none_is_the_bare_string_none_on_the_wire() {
        let task_json = serde_json::json!({ "retry": "none" });
        let policy: RetryPolicy = serde_json::from_value(task_json["retry"].clone()).unwrap();
        assert_eq!(policy, RetryPolicy::None);
        assert_eq!(serde_json::to_value(RetryPolicy::None).unwrap(), serde_json::json!("none"));
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
