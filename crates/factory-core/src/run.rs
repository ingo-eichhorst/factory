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
    /// The agent reported `done`, and the steps its control plan requires
    /// (`Run::required_steps`) are being run and attested before that
    /// counts (`#118`). Not terminal: it ends `Done` when every required
    /// attestation exists and passed, or goes to `Blocked` with the reason.
    Verifying,
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
            Self::Verifying => "verifying",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// The task's own status while this is its most recent run. A failed
    /// run leaves its task `Blocked`, never failed: the run really did fail
    /// and stays that way, but the task is an open item until a person
    /// disposes of it (`#122`) -- `Task::failure` says it is that kind of
    /// block. A scheduled task's engine may still hold it `Pending` while a
    /// retry is queued (`Engine::mirror_to_task`).
    pub fn as_task_status(self) -> crate::task::TaskStatus {
        use crate::task::TaskStatus as T;
        match self {
            Self::Dispatching => T::Dispatching,
            Self::Running => T::Running,
            Self::Blocked => T::Blocked,
            Self::Verifying => T::Verifying,
            Self::Done => T::Done,
            Self::Failed => T::Blocked,
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
            "verifying" => Self::Verifying,
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
    /// A scheduled task's automatic retry of a run that just failed -- see
    /// `Engine::queue_or_end_retry`. Deliberately its own variant rather than
    /// reusing `Schedule`: "it failed" reads differently for the week's
    /// regular firing than for the daemon quietly trying again a few minutes
    /// later, and a journal or a dashboard that could not tell the two apart
    /// would be exactly the "silently skipped" problem this exists to fix.
    Retry,
    Agent,
    Workflow,
    /// Started as one attempt of a bench run -- see `bench::BenchOrigin`,
    /// which the task itself carries.
    Bench,
}

impl Trigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Schedule => "schedule",
            Self::Retry => "retry",
            Self::Agent => "agent",
            Self::Workflow => "workflow",
            Self::Bench => "bench",
        }
    }
}

/// Who put a run into `Blocked`. `RunStatus::Blocked` alone does not say
/// this, and it has to be said somewhere: the daemon may honestly take back
/// only what it put there itself. An agent that called
/// `factory task report --status blocked` gets to be the only one who calls
/// it back, even if the runtime later thinks the pane looks busy again --
/// see `AGENTS.md` on statuses coming from the agent, never from a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockSource {
    /// The agent said so itself, over the reporting contract.
    Agent,
    /// A runtime's lifecycle hook told Factory, with no report from the agent
    /// involved at all.
    Runtime,
    /// The daemon's own `done` gate (`#118`): the agent said it was done and
    /// a required step's attestation was missing or failed. The agent may
    /// take it back by reporting again -- `done` re-runs the verification.
    Verification,
}

impl BlockSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Runtime => "runtime",
            Self::Verification => "verification",
        }
    }
}

/// Why a run ended `Failed` or `Cancelled`, as a fact recorded where it
/// happened rather than read back out of `error`'s prose -- counting
/// timeouts by matching strings is exactly what this exists to avoid. The
/// free-text `error` stays alongside it for a person to read.
///
/// `None` on a run that ended any other way, and on every run that ended
/// before this field existed: those are *unclassified*, never quietly
/// counted as one of the kinds below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailKind {
    /// Still `Dispatching` past its ack timeout: the agent never said a word.
    AckTimeout,
    /// Ran past its total-duration cap (`timeout_seconds`).
    RunTimeout,
    /// Sat `Blocked` past `blocked_timeout_seconds` with nobody answering.
    BlockedTimeout,
    /// The session went away and the agent never reported back.
    SessionGone,
    /// Never reached an agent at all: the scope, the worktree, the runtime
    /// or the agent refused before the task was handed over.
    DispatchFailed,
    /// The agent itself reported `failed`.
    AgentFailed,
    /// The harness said the agent's turn ended with no report before it --
    /// a `Stop` hook that stood, or `pi`'s own `idle` lifecycle hook.
    TurnEnded,
    /// The harness's `StopFailure` hook: the turn was cut short by an API
    /// error.
    StopFailure,
    /// Cancelled on a request that came in as the owner -- the UI, the CLI,
    /// or anything else that presented no token. That last clause is the
    /// honest limit of the name: an agent that omits its token *is* the
    /// owner (`AGENTS.md`), and nothing here can tell the two apart.
    CancelledByPerson,
    /// Cancelled by an agent that identified itself, or reported as
    /// `cancelled` by the run's own agent.
    CancelledByAgent,
    /// Cancelled because the workflow or bench run it belongs to was.
    CancelledWithParent,
}

impl FailKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AckTimeout => "ack_timeout",
            Self::RunTimeout => "run_timeout",
            Self::BlockedTimeout => "blocked_timeout",
            Self::SessionGone => "session_gone",
            Self::DispatchFailed => "dispatch_failed",
            Self::AgentFailed => "agent_failed",
            Self::TurnEnded => "turn_ended",
            Self::StopFailure => "stop_failure",
            Self::CancelledByPerson => "cancelled_by_person",
            Self::CancelledByAgent => "cancelled_by_agent",
            Self::CancelledWithParent => "cancelled_with_parent",
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
    /// Where this attempt worked, when its task asked for a worktree of its
    /// own: the branch it is on and the path it was checked out at. `None`
    /// for a run that worked in the scope directly, and for any run this
    /// daemon made before the field existed. Set once, when the worktree is
    /// made, and never cleared -- the daemon does not clean these up, so this
    /// is the only record of where the work went once the run ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_branch: Option<String>,
    pub runtime: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionRef>,
    /// The secret the agent presents when reporting on this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// The task estimate as it stood when this attempt was dispatched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_estimate: Option<crate::task::Estimate>,
    /// The configured provider account selected for this attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_account: Option<String>,
    /// The first-turn re-estimate, recorded once and never rewritten.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub re_estimate: Option<crate::usage::ReEstimate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// The workflow route selected by this run's terminal `done` report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routed_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    /// The moment this attempt could first have been dispatched -- the
    /// start of its queue wait, so `started_at - queued_at` is how long the
    /// daemon took to get it going once nothing stood in the way:
    ///
    /// * a schedule's firing: the latest of its slot (`scheduled_for`), the
    ///   moment this daemon came up if it was down at the slot, and the end
    ///   of the task's previous run if that was still going at the slot
    ///   (only a pending task fires). Time lost to those two is schedule
    ///   *lateness*, measured separately as `started_at - scheduled_for`,
    ///   and never counted as queue wait;
    /// * a queued retry: the same, from when its backoff ran out;
    /// * a manual `task.run`: when the request arrived;
    /// * a workflow node, an agent's request, a bench attempt: when the
    ///   daemon made it eligible and asked for it to start.
    ///
    /// A host asleep with the daemon still running leaves no record, so
    /// that stretch still counts as queue wait. `None` on every run made
    /// before the field existed -- an unknown wait, never a zero one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued_at: Option<DateTime<Utc>>,
    /// The schedule slot that fired this run, for a `Trigger::Schedule` run
    /// and nothing else. Captured before `advance_schedule` moves
    /// `next_run_at` on, which is the only moment the slot still exists;
    /// `started_at - scheduled_for` is the firing's *lateness* -- every
    /// second between the slot and the dispatch, whatever held it up --
    /// which includes the queue wait (`started_at - queued_at`) and
    /// whatever came before it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_for: Option<DateTime<Utc>>,
    /// Why the run ended `Failed` or `Cancelled` -- see [`FailKind`]. Set
    /// once, by whatever ended it. A `Failed` run's kind is mirrored onto
    /// its task as `Task::failure` (`#122`), so a board can say why a task
    /// is blocked; the engine clears it when a newer run starts, like any
    /// other mirrored field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fail_kind: Option<FailKind>,
    /// When the current block began. `None` whenever `status` is not
    /// `Blocked` -- this is the block's own clock, not a history of every
    /// block the run has ever had, so it is cleared the moment the block
    /// ends and `blocked_timeout_seconds` is measured from it rather than
    /// from `started_at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_since: Option<DateTime<Utc>>,
    /// Who set the current block. Governs who may honestly take it back off
    /// -- see `BlockSource`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_source: Option<BlockSource>,
    /// Not a status, and never treated as one: a timestamp the runtime's own
    /// screen-guess put here, so the UI can show "the runtime thinks this may
    /// be waiting, since T" without Factory ever having asserted it. Cleared
    /// the moment the guess stops being `blocked`, confirmed or not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_suspected_since: Option<DateTime<Utc>>,
    /// When the harness's `Stop` hook last said this run's turn ended with no
    /// report before it -- a fact the harness reported, held rather than acted
    /// on at once, because another `Stop` hook in the same session may have
    /// kept the turn going (see `occupancy::settle_turn_end`). Cleared by any
    /// report from the agent and when the run ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_ended_at: Option<DateTime<Utc>>,
    /// What the run fails with if that turn end stands. Set and cleared
    /// together with `turn_ended_at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_end_reason: Option<String>,
    /// The steps this run must pass before `done` counts, fixed at dispatch
    /// from its task's control plan (`#118`) -- a catalogue edited while the
    /// run works does not change what it is held to. Empty for a run with
    /// nothing required, which reports `done` straight to `Done` as always.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_steps: Vec<crate::control_plan::RequiredStep>,
    /// What this run used, as of its newest usage snapshot -- derived from
    /// the append-only snapshots the store keeps (`TaskStore::usage_snapshots`)
    /// and rewritten whole each time one is added, so a reader of a run
    /// never has to know they exist. `None` on a run that has no snapshot
    /// at all, including every run from before #117; a run whose runtime
    /// could not answer has `Some` with `state: unknown` and the reason.
    /// Never mirrored onto the task: a task's usage is a sum over its runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<crate::usage::RunUsage>,
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
    /// See `Run::queued_at`. `None` means "now": the store stamps the run's
    /// own start, which is honest for anything dispatched the moment it was
    /// asked for. Defaulted on the wire, so an out-of-process store that
    /// predates it still parses what it is sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued_at: Option<DateTime<Utc>>,
    /// See `Run::scheduled_for`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_for: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<RunStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_estimate: Option<crate::task::Estimate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub re_estimate: Option<crate::usage::ReEstimate>,
    /// `None` means "leave alone" everywhere else here, so letting go of a
    /// session needs a field of its own.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_session: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routed_to: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_routed_to: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// A finished run has no more use for its token.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_token: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_since: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_source: Option<BlockSource>,
    /// `blocked_since` and `blocked_source` are never meaningfully set apart,
    /// so one flag clears both -- the `clear_session`/`clear_token` pattern
    /// above, applied to the pair together.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_blocked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_suspected_since: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_block_suspicion: bool,
    /// Record a `Stop` hook's turn end: when, and the reason it will fail
    /// with. Always set as a pair.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_ended_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_end_reason: Option<String>,
    /// Clears both of the above.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_turn_ended: bool,
    /// Set once, right after `git worktree add` succeeds. Nothing ever clears
    /// these -- there is no "leave the worktree" patch, because there is
    /// nothing else for the run to have used once it had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_branch: Option<String>,
    /// Set together with a terminal `Failed`/`Cancelled` status. Nothing
    /// clears it: a run ends once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fail_kind: Option<FailKind>,
    /// Set once, at dispatch -- see `Run::required_steps`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_steps: Option<Vec<crate::control_plan::RequiredStep>>,
    /// Replace the run's derived usage -- see `Run::usage`. Only ever set
    /// with a value freshly computed from every snapshot, so there is no
    /// "clear" to go with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<crate::usage::RunUsage>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stored_run_from_before_queue_and_fail_facts_reads_them_as_unknown() {
        // A row written before `queued_at`, `scheduled_for` and `fail_kind`
        // existed. They must come back `None` -- unknown -- so nothing
        // downstream can mistake an old run for one that waited zero
        // seconds or failed for no reason at all.
        let json = r#"{
            "id": "r1", "task_id": "t1", "attempt": 1, "status": "failed",
            "trigger": "schedule", "agent": "shell", "runtime": "herdr",
            "error": "the agent never acknowledged the task within 300s",
            "started_at": "2024-01-01T00:00:00Z", "ended_at": "2024-01-01T00:05:00Z"
        }"#;
        let run: Run = serde_json::from_str(json).unwrap();
        assert_eq!(run.queued_at, None);
        assert_eq!(run.scheduled_for, None);
        assert_eq!(run.fail_kind, None);
    }

    #[test]
    fn a_fail_kind_is_snake_case_on_the_wire_and_matches_as_str() {
        for kind in [
            FailKind::AckTimeout,
            FailKind::RunTimeout,
            FailKind::BlockedTimeout,
            FailKind::SessionGone,
            FailKind::DispatchFailed,
            FailKind::AgentFailed,
            FailKind::TurnEnded,
            FailKind::StopFailure,
            FailKind::CancelledByPerson,
            FailKind::CancelledByAgent,
            FailKind::CancelledWithParent,
        ] {
            let json = serde_json::to_value(kind).unwrap();
            assert_eq!(json, serde_json::json!(kind.as_str()));
            assert_eq!(serde_json::from_value::<FailKind>(json).unwrap(), kind);
        }
    }

    #[test]
    fn a_new_run_from_an_older_caller_parses_with_no_queue_facts() {
        let new: NewRun = serde_json::from_str(
            r#"{"task_id":"t1","trigger":"manual","agent":"shell","adapter":"shell","runtime":"herdr","token":"x"}"#,
        )
        .unwrap();
        assert_eq!(new.queued_at, None);
        assert_eq!(new.scheduled_for, None);
    }
}
