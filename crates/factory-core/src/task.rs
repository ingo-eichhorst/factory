use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeEstimateRange {
    pub low: u64,
    pub expected: u64,
    pub high: u64,
}

impl TimeEstimateRange {
    pub fn point(seconds: u64) -> Self { Self { low: seconds, expected: seconds, high: seconds } }
    pub fn validate(self) -> std::result::Result<(), String> {
        if self.low == 0 { return Err("a task estimate must be at least one second".into()); }
        if self.low > self.expected || self.expected > self.high {
            return Err("estimate time must be ordered low <= expected <= high".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CostEstimateRange {
    pub low: f64,
    pub expected: f64,
    pub high: f64,
}

impl CostEstimateRange {
    pub fn validate(self) -> std::result::Result<(), String> {
        if !self.low.is_finite() || !self.expected.is_finite() || !self.high.is_finite() {
            return Err("estimate cost values must be finite".into());
        }
        if self.low < 0.0 || self.low > self.expected || self.expected > self.high {
            return Err("estimate cost must be ordered 0 <= low <= expected <= high".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Estimate {
    pub time: TimeEstimateRange,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<CostEstimateRange>,
}

impl Estimate {
    pub fn point(seconds: u64) -> Self { Self { time: TimeEstimateRange::point(seconds), cost: None } }
    pub fn validate(&self) -> std::result::Result<(), String> {
        self.time.validate()?;
        if let Some(cost) = self.cost { cost.validate()?; }
        Ok(())
    }
}

/// Where a task is in its life. The agent moves it through `Running` ->
/// `Done`/`Failed`/`Blocked` by calling back; nothing infers completion from a
/// terminal's appearance.
///
/// A *run* that failed stays `Failed`; the *task* behind it does not
/// (`#122`). A failure is something a person has to look at, so the task
/// goes to `Blocked` with `Task::failure` saying why -- see
/// `Task::blocked_by_failure`. Closed means `Done` or `Cancelled`, and a
/// failure never gets there on its own: closing one is a person's act, with
/// a reason (`Task::closure`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Handed in through intake (`#119`) and not yet released: held back
    /// from dispatch, with no run, until triage decides. Never due, and
    /// `task run` refuses it -- see `crate::intake`.
    Intake,
    /// Created and not started. Nothing dispatches it on its own -- there is
    /// no queue and no capacity to wait on (`#124`): an unscheduled task stays
    /// here until someone calls `task run`, and a scheduled one until its
    /// slot (or a queued retry) comes due.
    Pending,
    /// The daemon has a runtime session and is handing the prompt over.
    Dispatching,
    /// The agent acknowledged the task and is working on it.
    Running,
    /// The agent needs a human before it can go on.
    Blocked,
    /// The agent said it is done; the steps its control plan requires are
    /// being checked before that counts (`#118`). Not terminal.
    Verifying,
    Done,
    /// Legacy, read-only: what a task whose run failed was stored as before
    /// `#122`. Nothing writes it any more, and the daemon moves every row
    /// still carrying it to `Blocked` when it starts
    /// (`Engine::migrate_failed_tasks`). Kept only so those rows still
    /// parse until then.
    Failed,
    Cancelled,
}

impl TaskStatus {
    /// Closed: `Done`, or `Cancelled` -- closed as not planned or as a
    /// duplicate. Not `Failed`: a failed task is open until someone
    /// disposes of it (`#122`), which is what makes a failed remediation
    /// task count as the open one a duplicate would pile up behind.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Intake => "intake",
            Self::Pending => "pending",
            Self::Dispatching => "dispatching",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Verifying => "verifying",
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
            "intake" => Self::Intake,
            "pending" => Self::Pending,
            "dispatching" => Self::Dispatching,
            "running" => Self::Running,
            "blocked" => Self::Blocked,
            "verifying" => Self::Verifying,
            "done" => Self::Done,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            other => return Err(format!("unknown status: {other}")),
        })
    }
}

/// When a task fires on its own. `Cron` is a five- or six-field expression,
/// read in a timezone of its own if it names one; `Every` is a plain
/// interval for the common "every N minutes" case, which no timezone
/// changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Schedule {
    Cron(CronSchedule),
    Every { seconds: u64 },
}

/// A cron expression and the timezone its fields are read in. Without a
/// timezone the fields are UTC, which is what every schedule meant before
/// this existed -- a schedule written then keeps firing exactly when it did.
/// With one (an IANA name, `Europe/Berlin`), `0 9 * * 1` is nine o'clock on
/// that wall clock, summer and winter alike.
///
/// On the wire, and in every row already stored, a schedule with no
/// timezone is the bare expression string it always was:
/// `{"cron": "0 7 * * 1"}`. Only one that names a timezone takes the object
/// form, `{"cron": {"expr": "0 9 * * 1", "timezone": "Europe/Berlin"}}`, so
/// nothing written before this -- a database row, a script, the UI -- has
/// to change to keep working.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "CronRepr", into = "CronRepr")]
pub struct CronSchedule {
    pub expr: String,
    /// An IANA timezone name. `None` is UTC. Checked when the schedule is
    /// set -- see `schedule::next_after` in the daemon -- not when it fires.
    pub timezone: Option<String>,
}

impl CronSchedule {
    /// How a schedule reads to a person: `0 9 * * 1`, or
    /// `0 9 * * 1 (Europe/Berlin)`.
    pub fn describe(&self) -> String {
        match &self.timezone {
            Some(tz) => format!("{} ({tz})", self.expr),
            None => self.expr.clone(),
        }
    }
}

/// A UTC schedule from a bare expression -- what `Schedule::Cron("…".into())`
/// has always meant.
impl From<&str> for CronSchedule {
    fn from(expr: &str) -> Self {
        Self { expr: expr.to_string(), timezone: None }
    }
}

impl From<String> for CronSchedule {
    fn from(expr: String) -> Self {
        Self { expr, timezone: None }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum CronRepr {
    Bare(String),
    Zoned {
        expr: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timezone: Option<String>,
    },
}

impl From<CronRepr> for CronSchedule {
    fn from(repr: CronRepr) -> Self {
        match repr {
            CronRepr::Bare(expr) => Self { expr, timezone: None },
            CronRepr::Zoned { expr, timezone } => Self {
                expr,
                // An empty name is no name, not a timezone called "".
                timezone: timezone.filter(|t| !t.trim().is_empty()),
            },
        }
    }
}

impl From<CronSchedule> for CronRepr {
    fn from(cron: CronSchedule) -> Self {
        match cron.timezone {
            None => CronRepr::Bare(cron.expr),
            timezone => CronRepr::Zoned { expr: cron.expr, timezone },
        }
    }
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

/// How a task was closed (`#122`), after GitHub's `state_reason` and Jira's
/// resolution: the status says *that* it ended, this says *how*. A closed
/// set, so a board can say each one plainly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseReason {
    /// Finished: the task is `Done`.
    Completed,
    /// Won't do. It did not complete and will not be picked up again,
    /// whatever the reason. The task is `Cancelled`.
    NotPlanned,
    /// Another task covers it -- `TaskClosure::duplicate_of` may say which.
    /// The task is `Cancelled`.
    Duplicate,
}

impl CloseReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::NotPlanned => "not_planned",
            Self::Duplicate => "duplicate",
        }
    }

    /// How a person reads it on a card.
    pub fn label(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::NotPlanned => "won't do",
            Self::Duplicate => "duplicate",
        }
    }

    /// The status a task closed this way sits in.
    pub fn status(self) -> TaskStatus {
        match self {
            Self::Completed => TaskStatus::Done,
            Self::NotPlanned | Self::Duplicate => TaskStatus::Cancelled,
        }
    }
}

impl std::str::FromStr for CloseReason {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.trim() {
            "completed" => Self::Completed,
            "not_planned" | "not-planned" => Self::NotPlanned,
            "duplicate" => Self::Duplicate,
            other => {
                return Err(format!(
                    "unknown close reason {other:?}: the reasons are completed, not_planned and duplicate"
                ))
            }
        })
    }
}

/// A task closed on purpose (`#122`): by whom, when, how, and what they
/// said. Written only by an explicit close (`Engine::close_task`), and
/// cleared by a reopen and by any run the task starts after it -- the
/// mirror rule, since a close record left behind would call a task that
/// has since run again closed.
///
/// A task that reached `Done` or `Cancelled` through a run has none: its
/// reason is read off the status (`Task::close_reason`), which is also how
/// every row written before this existed reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskClosure {
    pub reason: CloseReason,
    /// The task this one duplicates, when `reason` is `duplicate` and the
    /// closer said which.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicate_of: Option<String>,
    /// A short note in the closer's words. Journaled too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Who closed it, as the journal names them: `the owner`, or an agent.
    pub by: String,
    pub at: DateTime<Utc>,
}

/// The failure a task is `Blocked` on (`#122`): its newest run ended
/// `Failed` and nothing is going to try again on its own. Mirrored from
/// that run so a board can say why without reading every run -- the
/// prose stays in `Task::error`, as before.
///
/// Set whenever the newest run failed, including while a scheduled task's
/// retry is queued (the task is `Pending` then, and the card can say it is
/// retrying after this); cleared the moment a newer run starts. Never a
/// `Blocked` *run*: that is what keeps a failed task out of the blocked
/// timeout, which only scans active runs -- a new blocked run made to show
/// a failure would time out into another failure, forever.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskFailure {
    /// Why, as a fact -- `None` only for a run that ended before
    /// `FailKind` existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<crate::run::FailKind>,
    /// The run that failed. `None` when dispatch refused before any run
    /// existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    pub at: DateTime<Utc>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate: Option<Estimate>,
    /// The most recent run's outcome, mirrored so a list does not have to read
    /// every run. `Run` is where it actually lives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// The workflow node this task selected with `done --send-to`, if any.
    /// Mirrored from its newest run like `result` and cleared on a new run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routed_to: Option<String>,
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
    /// Whether a run of this task is handed the knowledge pages that match
    /// its title and instructions, searched once at dispatch. Off unless a
    /// caller turns it on, so a task written before the field existed reads
    /// the same as one that never asked. The pages a run was given belong to
    /// that run -- its journal records them -- and are never mirrored here.
    #[serde(default)]
    pub knowledge_hints: bool,
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
    /// A person stopped this task's schedule without losing it -- the
    /// andon cord. `due()` passes over a paused task, so neither its
    /// regular slots nor a queued retry fire; the schedule itself, and
    /// `next_run_at` as it stood, stay put so the task still says what it
    /// would have done. Resuming recomputes `next_run_at` from the moment
    /// of resuming (`Engine::update`): the slots that passed while paused
    /// were paused, not missed, and never fire as a burst of catch-up runs.
    /// Absent on every task written before this existed, which reads as
    /// running -- exactly what those tasks were doing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub schedule_paused: bool,
    /// What kind of work this is -- `feature`, `bugfix`, `release`, `docs`
    /// (`#118`). Keys the control plan its runs are held to. `None` is
    /// planned as `control_plan::DEFAULT_CATEGORY`, never as "no plan": a
    /// task cannot get round the plan by leaving this out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// The intake record (`#119`), on a task that was handed in through the
    /// gate rather than created straight onto the line. It stays after
    /// release -- the triage verdict and who decided belong to the task's
    /// history -- and is absent on every other task, including every one
    /// written before intake existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intake: Option<crate::intake::Intake>,
    /// Why the newest run failed, while it is the newest -- see
    /// `TaskFailure`. Absent on every row written before `#122`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<TaskFailure>,
    /// How and by whom the task was closed, when a person or an agent
    /// closed it on purpose -- see `TaskClosure`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closure: Option<TaskClosure>,
}

impl Task {
    pub fn effective_estimate(&self) -> Option<Estimate> {
        self.estimate.clone().or_else(|| self.estimate_seconds.map(Estimate::point))
    }

    /// `Blocked` because its newest run failed, rather than because an
    /// agent is waiting on a question (`#122`). No run is active: there is
    /// nobody to answer, only a failure to look at, run again, or close.
    pub fn blocked_by_failure(&self) -> bool {
        self.status == TaskStatus::Blocked && self.failure.is_some()
    }

    /// Nothing is running and nothing will until someone acts: closed, or
    /// blocked by a failure. The question a caller waiting on the task's
    /// *outcome* asks -- a workflow node, a bench attempt, a triage run --
    /// as opposed to `TaskStatus::is_terminal`, which asks whether the task
    /// is closed. A legacy `Failed` row counts, for the moment before the
    /// startup migration reaches it.
    pub fn is_settled(&self) -> bool {
        self.status.is_terminal() || self.blocked_by_failure() || self.status == TaskStatus::Failed
    }

    /// Whether its schedule may fire it: pending, or blocked by a failure,
    /// which a later success clears (`#122`). Never a closed task, and
    /// never one blocked on a question -- that one has a run.
    pub fn fires(&self) -> bool {
        self.status == TaskStatus::Pending || self.blocked_by_failure()
    }

    /// Did it end in failure? Blocked by one, or a legacy `Failed` row.
    pub fn has_failed(&self) -> bool {
        self.blocked_by_failure() || self.status == TaskStatus::Failed
    }

    /// How a closed task was closed: its close record when someone closed
    /// it on purpose, otherwise what its status says -- `Done` completed,
    /// `Cancelled` (a run cancelled, or intake's wontfix) not planned.
    /// `None` for a task that is not closed.
    pub fn close_reason(&self) -> Option<CloseReason> {
        if !self.status.is_terminal() {
            return None;
        }
        Some(match &self.closure {
            Some(c) => c.reason,
            None if self.status == TaskStatus::Done => CloseReason::Completed,
            None => CloseReason::NotPlanned,
        })
    }
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
    pub estimate: Option<Estimate>,
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
    /// Hand each run the knowledge pages that match this task -- see
    /// `Task::knowledge_hints`. Absent means off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub knowledge_hints: bool,
    /// This task's own retry policy, overriding the daemon's default. Absent
    /// means "use the default" -- see `RetryPolicy`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryPolicy>,
    /// See `Task::category`. Absent is the default category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
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
    pub estimate: Option<Estimate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routed_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// `None` means "leave alone" here, so a run that succeeded needs a way to
    /// say the previous attempt's error no longer describes this task.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_result: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_routed_to: bool,
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
    /// Turn knowledge hints on or off. A plain setting, so `Some(false)` is
    /// how to switch them off and no `clear_` twin is needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge_hints: Option<bool>,
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
    /// Pause or resume the schedule -- see `Task::schedule_paused`. A plain
    /// setting like `knowledge_hints`, so `Some(false)` resumes and no
    /// `clear_` twin is needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_paused: Option<bool>,
    /// Set the category; `clear_category` goes back to the default one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_category: bool,
    /// The whole intake record, replaced -- only the engine's intake
    /// transitions write it. Never cleared: a released item keeps it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intake: Option<crate::intake::Intake>,
    /// The mirrored failure (`#122`) -- set and cleared by the engine as
    /// the newest run changes, the `Option`-plus-`clear` shape `error` has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<TaskFailure>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_failure: bool,
    /// The close record (`#122`) -- written by a close, cleared by a reopen
    /// and by any new run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closure: Option<TaskClosure>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_closure: bool,
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
    /// Select one of this workflow node's declared `agent:` exits. Valid
    /// only together with `status: done`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_to: Option<String>,
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
    fn a_stored_cron_schedule_from_before_timezones_still_reads_as_utc() {
        // Verbatim the shape of the live weekly audit's row.
        let old: Schedule = serde_json::from_str(r#"{"cron":"0 7 * * 1"}"#).unwrap();
        assert_eq!(old, Schedule::Cron("0 7 * * 1".into()));
        let Schedule::Cron(cron) = old else { panic!("not cron") };
        assert_eq!(cron.timezone, None);
    }

    #[test]
    fn a_utc_cron_schedule_is_still_written_as_the_bare_string() {
        // So an older daemon, script or UI reading it back sees nothing new.
        let json = serde_json::to_string(&Schedule::Cron("0 7 * * 1".into())).unwrap();
        assert_eq!(json, r#"{"cron":"0 7 * * 1"}"#);
    }

    #[test]
    fn a_zoned_cron_schedule_round_trips_as_an_object() {
        let zoned = Schedule::Cron(CronSchedule {
            expr: "0 9 * * 1".into(),
            timezone: Some("Europe/Berlin".into()),
        });
        let json = serde_json::to_string(&zoned).unwrap();
        assert_eq!(json, r#"{"cron":{"expr":"0 9 * * 1","timezone":"Europe/Berlin"}}"#);
        assert_eq!(serde_json::from_str::<Schedule>(&json).unwrap(), zoned);
    }

    #[test]
    fn an_object_with_no_or_an_empty_timezone_is_utc() {
        for json in [r#"{"cron":{"expr":"0 9 * * 1"}}"#, r#"{"cron":{"expr":"0 9 * * 1","timezone":""}}"#] {
            let parsed: Schedule = serde_json::from_str(json).unwrap();
            assert_eq!(parsed, Schedule::Cron("0 9 * * 1".into()), "{json}");
        }
    }

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
        assert!(!task.schedule_paused, "an old row predates pausing, so its schedule is running");
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

    #[test]
    fn estimate_ranges_validate_order_and_old_point_estimates_remain_effective() {
        let estimate = Estimate {
            time: TimeEstimateRange { low: 600, expected: 900, high: 1800 },
            cost: Some(CostEstimateRange { low: 1.0, expected: 2.0, high: 4.0 }),
        };
        assert!(estimate.validate().is_ok());
        assert!(Estimate { time: TimeEstimateRange { low: 901, expected: 900, high: 1800 }, cost: None }.validate().is_err());
        assert!(Estimate { time: TimeEstimateRange::point(900), cost: Some(CostEstimateRange { low: 2.0, expected: 1.0, high: 3.0 }) }.validate().is_err());
        let task: Task = serde_json::from_str(r#"{
            "id":"t", "title":"old", "instructions":"", "scope":"demo",
            "agent":"builder", "runtime":"herdr", "status":"pending", "estimate_seconds":900,
            "created_at":"2026-01-01T00:00:00Z", "updated_at":"2026-01-01T00:00:00Z"
        }"#).unwrap();
        assert_eq!(task.effective_estimate(), Some(Estimate::point(900)));
    }
}
