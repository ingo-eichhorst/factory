//! The durable task queue (design §2.4, §5; backlog §7).
//!
//! Skeleton only. The coordinator owns this file's module declarations and the
//! crate manifest so that concurrent work never collides in a centrally owned
//! file; every other line here belongs to the slice's own agents.
//!
//! This file carries the task vocabulary — [`TaskStatus`], [`BlockedReason`],
//! the transition table, [`blocked_reason_for`], the result-size limits, and
//! [`TaskError`] — mirroring how `factory_session` carries the session
//! vocabulary (`SessionState`, its transition table, `SessionError`).
//! [`create`] is where creation, read-only inspection, and cancellation
//! actually happen.

//! # Station 11 — the coordinator's binding decisions
//!
//! ADR 0021 carries the reasoning. These are the parts an agent working in
//! this crate must not re-decide.
//!
//! **1. A `tasks` row is a run.** There is no `task_runs` table and none is to
//! be added. A template sits above a run, in [`template`]; a run is what this
//! crate already manages. Anything that reads like "create a task run" is
//! [`create::create`] with `template_id` and `template_version` filled in.
//!
//! **2. `template_version` is frozen at creation.** Backlog §11: "that record
//! does not change when the template is later revised." No code path in this
//! crate may write `tasks.template_version` after the insert that created the
//! row. Revising a template raises `task_templates.version`; existing runs
//! keep the number they executed.
//!
//! **3. Every material transition writes exactly one [`events`] row, and
//! [`events`] is its one home.** "Material" means the set in the schema's own
//! CHECK: created, assigned, delivered, refused, running, progress, blocked,
//! done, failed, cancelled, verification, rework. No caller composes the event
//! itself and no transition function writes the row inline — each calls
//! [`events`]. The catching mutation for this is deleting an event write and
//! finding a test that dies in every module claiming the rule.
//!
//! `refused` is in that list because station 10 decided a refusal is not a
//! delivery: `Adapter::send` declines to submit into a busy session, nothing
//! reaches a terminal, and the task stays `queued`. Recording it as
//! `delivered` would put the confusion station 10 removed back into the one
//! place an operator looks to find out what happened.
//!
//! There is no `assigned` *status*. A task that has been assigned but not yet
//! delivered is still `queued`; assignment is `tasks.assigned_session_id` plus
//! an `assigned` event. `tasks_one_running_per_session` depends on `running`
//! being the one status that must name a session, so nothing is to be added to
//! that CHECK.
//!
//! **4. A verification verdict annotates and never transitions.** [`verify`]
//! writes one `verification` event and touches no other table and no column of
//! `tasks`.
//!
//! Its one guard is independence, and it compares **scopes, not sessions**.
//! Design §12.1: "a worker still cannot raise a verification run against
//! another session of *its own scope*." So a verdict whose author is a session
//! is refused when that session's scope is the run's `target_scope_id`; a
//! verdict with no author session is a human's and is allowed. Comparing
//! against `assigned_session_id` instead would both permit a sibling session
//! of the same scope and pass trivially on every task no session ever ran —
//! one a human closed, one cancelled while queued, one whose only delivery
//! attempt was refused. `target_scope_id` is NOT NULL and has no such hole.
//!
//! **5. Nothing in this crate creates a verification *run*.** ADR 0021
//! decision 5: version 1 has no agent-initiated inspection, so design §6 needs
//! no exception and none is written. A rule with no caller is how
//! `authorise_resume` sat unreachable for a whole station.
//!
//! **6. Events and decisions carry no secret and no copied private source.**
//! Design §11 and `agent-task-scheduler-design.md`'s security section both say
//! it outright. A payload is a short structured fact, never a transcript,
//! never a file body, never an environment variable.
//!
//! **7. [`schedule`] stores rules; it never fires them.** Creating a run from
//! a due schedule is the daemon's dispatcher (ADR 0021 decision 2). This
//! module is CRUD plus "when would this next fire," and the next-run answer is
//! computed, never stored.

pub mod assign;
pub mod complete;
pub mod create;
pub mod decisions;
pub mod deliver;
pub mod events;
pub mod schedule;
pub mod template;
pub mod verify;

/// Design §2.4's exact task-status vocabulary, matching `tasks.status`'s
/// CHECK constraint in `factory_store::schema` byte for byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Queued,
    Running,
    Blocked,
    Done,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub(crate) fn as_db_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// `tasks.status` is constrained by CHECK to exactly these six strings,
    /// so an unrecognised value read back from a row this crate itself just
    /// selected is a broken invariant, not an input to handle — hence the
    /// panic rather than a `Result`. Mirrors
    /// `factory_session::SessionState::from_db_str`'s reasoning exactly.
    pub(crate) fn from_db_str(s: &str) -> Self {
        match s {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "blocked" => Self::Blocked,
            "done" => Self::Done,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            other => unreachable!(
                "tasks.status is constrained by CHECK to the design §2.4 vocabulary; read {other:?}"
            ),
        }
    }

    /// Design §2.4's three terminal statuses: `done`, `failed`, `cancelled`.
    /// `blocked` is deliberately **not** terminal — it reads like an ending
    /// but is not one; a `blocked` task returns to `queued` and is reassigned
    /// through the ordinary path (see [`valid_targets`]'s `blocked → queued`
    /// entry).
    ///
    /// This is the single home of that rule in this crate, the way
    /// `factory_session::SessionState::holds_lease` is the single home of
    /// ADR 0012 decision 5: every place in this crate that needs to know
    /// whether a status is terminal calls this method rather than re-listing
    /// the three strings. `factory_session::on_task_terminal` also lists
    /// those three strings inline (it cannot call into this crate — this
    /// crate depends on `factory-session`, not the reverse) — see
    /// [`tests::is_terminal_agrees_with_factory_session_on_task_terminal`]
    /// below, which pins the two definitions together so a change to either
    /// one's list breaks a test rather than silently drifting from the
    /// other.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_db_str())
    }
}

/// Design §2.4's exact `blocked_reason` vocabulary, matching
/// `tasks.blocked_reason`'s CHECK constraint in `factory_store::schema` byte
/// for byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockedReason {
    Clarification,
    Permission,
    Interrupted,
    External,
}

impl BlockedReason {
    pub(crate) fn as_db_str(self) -> &'static str {
        match self {
            Self::Clarification => "clarification",
            Self::Permission => "permission",
            Self::Interrupted => "interrupted",
            Self::External => "external",
        }
    }

    /// `tasks.blocked_reason` is constrained by CHECK to exactly these four
    /// strings whenever it is non-NULL, so an unrecognised value read back
    /// from a row this crate itself just selected is a broken invariant, not
    /// an input to handle — the same reasoning as [`TaskStatus::from_db_str`]
    /// and `factory_session::SessionState::from_db_str`.
    pub(crate) fn from_db_str(s: &str) -> Self {
        match s {
            "clarification" => Self::Clarification,
            "permission" => Self::Permission,
            "interrupted" => Self::Interrupted,
            "external" => Self::External,
            other => unreachable!(
                "tasks.blocked_reason is constrained by CHECK to the design §2.4 vocabulary; read {other:?}"
            ),
        }
    }
}

impl std::fmt::Display for BlockedReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_db_str())
    }
}

/// The states `to` is reachable from `from`, in one call — design §2.4 and
/// backlog §7, in the same shape as
/// `factory_session::valid_targets`.
///
/// - `queued → running`: Factory assigned and delivered the task to a
///   session (design §5 steps 2–4).
/// - `queued → blocked`: an adapter observation is authoritative before the
///   agent ever starts working (see [`blocked_reason_for`]) — rare, but not
///   excluded by the design.
/// - `queued → failed`: delivery itself cannot be completed (design §5's
///   "record the delivery attempt before writing to the PTY" can still fail
///   after the attempt is journalled).
/// - `queued → cancelled`: [`create::cancel`]'s immediate path — nothing has
///   started, so there is nothing to be cooperative about.
/// - `running → done` / `running → failed`: the agent reports a terminal
///   result (design §5 step 5).
/// - `running → blocked`: the harness surfaces a question, a permission
///   prompt, or an interruption ([`blocked_reason_for`]).
/// - `running → cancelled`: [`complete::acknowledge_cancellation`]'s
///   cooperative path — the *only* function in this crate that writes this
///   edge. **This table permits the move unconditionally; that is only half
///   the rule.** What keeps it cooperative rather than a force-stop is that
///   function's own guard, which requires `cancel_requested_at IS NOT NULL`
///   (set by [`create::cancel`]'s `running` arm), checked inside the same
///   transaction as the status write — see its doc comment for the other
///   half of this same rule, documented at both sites on purpose. Design
///   §2.4: "Task cancellation is cooperative once a task is running." A
///   direct edge with no such guard would be the force-stop design §2.4
///   describes as "a separate user action," which this crate does not
///   implement (see [`create::cancel`]'s doc comment on why
///   `factory_session::interrupt` is not the tool for this either).
/// - `blocked → queued`: the blocking condition is resolved (an answer is
///   given, a permission is granted) and the task re-enters the ordinary
///   assignment path. Re-assignment is all it re-enters. [`deliver::deliver`]
///   refuses any task that already carries a `delivery_attempts` row, so a
///   requeued task is not automatically sent a second time — design §5 is
///   explicit that Factory "does not automatically resend a possibly
///   delivered prompt", and offers exactly two ways on: "a human may resume
///   or create a replacement task." Creating a replacement works today. The
///   explicit resume — a human authorising one further delivery, which is
///   not the automatic resend §5 forbids — is slice 9's action and does not
///   exist yet, so a requeued task waits for a human either way.
/// - `blocked → failed`: the blocking condition is never resolved and an
///   operator gives up on the task.
/// - `blocked → cancelled`: [`create::cancel`]'s immediate path — a blocked
///   task is not running anywhere, so, like `queued`, cancelling it needs no
///   cooperation from an agent.
/// - **No `blocked → running`.** A blocked task returns through `queued`
///   instead, rather than resuming silently against a session that may no
///   longer exist by the time the block is resolved. Going back through
///   `queued` is what forces the question "which session should this run in
///   now?" to be asked again instead of assumed.
/// - `done`, `failed`, `cancelled`: terminal (see [`TaskStatus::is_terminal`])
///   — nothing reaches anywhere from here.
pub(crate) fn valid_targets(from: TaskStatus) -> &'static [TaskStatus] {
    use TaskStatus::{Blocked, Cancelled, Done, Failed, Queued, Running};
    match from {
        Queued => &[Running, Blocked, Failed, Cancelled],
        Running => &[Done, Failed, Blocked, Cancelled],
        Blocked => &[Queued, Failed, Cancelled],
        Done | Failed | Cancelled => &[],
    }
}

pub(crate) fn is_valid_transition(from: TaskStatus, to: TaskStatus) -> bool {
    valid_targets(from).contains(&to)
}

/// Map an adapter's observation onto this crate's [`BlockedReason`]
/// vocabulary, or `None` when no automatic reason should be recorded.
///
/// A plain function from `&Observation` to `Option<BlockedReason>` (rather
/// than, say, a method on `Observation` itself, which would put a
/// `factory-task` concept inside `factory-adapter`) — `factory-adapter`
/// already owns the harness-facing vocabulary and must not know this crate's
/// column-level vocabulary exists; this crate owns the reverse direction
/// instead.
///
/// Two constraints, both from `factory_adapter`'s own documented contract
/// (read `crates/factory-adapter/src/lib.rs` lines 31-120 before changing
/// this):
///
/// - Only [`factory_adapter::Confidence::Authoritative`] may produce a
///   reason. [`factory_adapter::Confidence::Degraded`] is documented there as
///   "recorded, never acted on: at-most-once delivery must not be built on a
///   heuristic," and [`factory_adapter::Confidence::Unavailable`] is no
///   answer at all ("the caller falls back to manual confirmation"). Both
///   fall back to `None` here, same as an observation that is not `Blocked`
///   at all.
/// - Every [`factory_adapter::BlockedReason`] variant must map to a string
///   `tasks.blocked_reason`'s CHECK accepts — proved by
///   [`tests::every_adapter_blocked_reason_maps_to_a_check_accepted_string`]
///   below, which inserts the mapped value into a real database rather than
///   only asserting the match arms exist.
#[must_use]
pub fn blocked_reason_for(observation: &factory_adapter::Observation) -> Option<BlockedReason> {
    if observation.confidence != factory_adapter::Confidence::Authoritative {
        return None;
    }
    let factory_adapter::TaskSignal::Blocked(reason) = &observation.task_signal else {
        return None;
    };
    Some(match reason {
        factory_adapter::BlockedReason::Clarification => BlockedReason::Clarification,
        factory_adapter::BlockedReason::Permission => BlockedReason::Permission,
        factory_adapter::BlockedReason::Interrupted => BlockedReason::Interrupted,
        factory_adapter::BlockedReason::External => BlockedReason::External,
    })
}

/// Maximum size, in bytes, of `tasks.result_summary`.
///
/// `tasks` is a control record an operator reads in a listing (AGENTS.md:
/// "Design read-only inspection... users must be able to see the exact...
/// task... state that caused an action"), not a log sink. AGENTS.md also
/// forbids secrets in task records, and a harness transcript dumped wholesale
/// into this column is exactly how a secret gets in. 16 KiB is generous
/// enough for a genuine human-readable summary and small enough that pasting
/// a transcript by accident is visibly wrong, not merely large.
///
/// Enforcement is another agent's (a completion path, not this crate's
/// `create`): this constant and [`TaskError::ResultSummaryTooLarge`] are what
/// it is defined against. Overflow must be an error, never a silent
/// truncation — a truncated result destroys the evidence it existed to
/// carry, which is worse than refusing to store it at all.
pub const MAX_RESULT_SUMMARY_BYTES: usize = 16 * 1024;

/// Maximum size, in bytes, of `tasks.result_artifact_paths` (the JSON array
/// of paths described in `factory_store::schema`'s comment on that column).
///
/// Half of [`MAX_RESULT_SUMMARY_BYTES`]: a list of paths is structurally
/// smaller than free-text prose for the same reason a directory listing is
/// smaller than the files it names, and the same secret-shaped-overflow
/// concern applies — a path list is not where a credential belongs, but an
/// unbounded column is exactly where one accumulates unnoticed. Overflow is
/// an error, never a silent truncation, for the same reason given on
/// [`MAX_RESULT_SUMMARY_BYTES`].
pub const MAX_RESULT_ARTIFACT_PATHS_BYTES: usize = 8 * 1024;

/// Everything that can go wrong creating, inspecting, or cancelling a task —
/// in the style of `factory_session::SessionError`.
#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    /// A schedule disappeared between being read as due and being stamped.
    /// The foreign key on the run just inserted makes this unreachable in
    /// practice, so it is carried as a distinct variant rather than folded
    /// into [`TaskError::Store`]: if it ever fires, the cause is not a
    /// database error and should not read like one.
    #[error("the schedule this run came from could not be stamped: {0}")]
    ScheduleVanished(String),

    #[error("store error: {0}")]
    Store(#[from] factory_store::StoreError),

    #[error("no task with id {0}")]
    NotFound(uuid::Uuid),

    #[error(
        "task {id} already has status `{status}`, which is terminal\n  help: `done`, `failed`, and `cancelled` end a task; nothing — including cancellation — moves it again"
    )]
    AlreadyTerminal { id: uuid::Uuid, status: TaskStatus },

    #[error(
        "task {id} is `{status}`, not `blocked`\n  help: `authorise_resume` is one human action with one meaning — it re-queues a task that is waiting on a human and records that a further delivery was approved. A task that is not `blocked` has nothing for a human to authorise past"
    )]
    NotBlocked { id: uuid::Uuid, status: TaskStatus },

    #[error(
        "task {id} cannot move from `{from}` to `{to}`\n  help: `{from}` only reaches {allowed}"
    )]
    InvalidTransition {
        id: uuid::Uuid,
        from: TaskStatus,
        to: TaskStatus,
        allowed: String,
    },

    #[error(
        "task result summary is {len} bytes, over the {max}-byte limit\n  help: shorten the summary — `tasks` is a control record an operator reads in a listing, not a transcript sink, and overflow is refused rather than truncated so nothing is silently lost"
    )]
    ResultSummaryTooLarge { len: usize, max: usize },

    #[error(
        "task result artifact paths are {len} bytes, over the {max}-byte limit\n  help: shorten the artifact path list — overflow is refused rather than truncated so nothing is silently lost"
    )]
    ResultArtifactPathsTooLarge { len: usize, max: usize },

    /// §12.2's hook, `create::create_rework`'s own guard: a run cannot
    /// rework itself. `reworks_task_id` is nullable and self-referencing
    /// passes the schema's own `REFERENCES tasks (id)` foreign key (the new
    /// row exists by the time SQLite checks it), so nothing but this check
    /// stops it.
    #[error(
        "task {0} cannot rework itself\n  help: `reworks_task_id` must name a different, already-finished run"
    )]
    SelfRework(uuid::Uuid),

    /// §12.2's hook: a rework carries an inspection finding back onto the
    /// line, and a finding presumes the referenced run has already finished.
    /// `blocked` is deliberately not accepted here even though it is not
    /// terminal in the ordinary sense either — see `create::create_rework`'s
    /// own doc comment for why a run waiting on a human is not safe to
    /// rework.
    #[error(
        "task {id} cannot be reworked because it is `{status}`, not finished\n  help: a rework carries a finding back onto the line, and a finding presumes the referenced run is finished — wait for it to reach `done`, `failed`, or `cancelled` before reworking it"
    )]
    ReworkTargetNotTerminal { id: uuid::Uuid, status: TaskStatus },

    /// design §11 / §12.2: a rework's finding is what makes the record
    /// honest — without it, a `reworks_task_id` link says a run replaces
    /// another but never says why. The same three-valued-logic gap
    /// `decisions::record`'s own `NoRationale` guards against for
    /// `task_decisions.rationale`.
    #[error(
        "task {0} reworks another run but names no finding\n  help: a rework without the finding that caused it is an unexplained link, not an audit trail"
    )]
    ReworkFindingRequired(uuid::Uuid),
}

#[cfg(test)]
mod tests {
    use factory_adapter::{
        BlockedReason as AdapterBlockedReason, Confidence, Observation, PaneId, TaskSignal,
    };

    use super::*;

    fn observation(confidence: Confidence, task_signal: TaskSignal) -> Observation {
        Observation {
            pane: PaneId("pane-1".to_string()),
            harness_state: "whatever the harness said".to_string(),
            confidence,
            session_alive: true,
            task_signal,
            transcript_path: None,
            harness_session_id: None,
        }
    }

    /// The transition table, read back directly rather than restated: every
    /// row in design §2.4's table (queued/running/blocked all reach exactly
    /// their documented targets; done/failed/cancelled reach nothing).
    #[test]
    fn valid_targets_matches_the_design_table() {
        assert_eq!(
            valid_targets(TaskStatus::Queued),
            &[
                TaskStatus::Running,
                TaskStatus::Blocked,
                TaskStatus::Failed,
                TaskStatus::Cancelled
            ]
        );
        assert_eq!(
            valid_targets(TaskStatus::Running),
            &[
                TaskStatus::Done,
                TaskStatus::Failed,
                TaskStatus::Blocked,
                TaskStatus::Cancelled
            ],
            "running → cancelled is the cooperative-cancellation-acknowledgement \
             edge added for this slice; see complete::acknowledge_cancellation, \
             the only function that uses it, for the guard that keeps it \
             cooperative"
        );
        assert_eq!(
            valid_targets(TaskStatus::Blocked),
            &[
                TaskStatus::Queued,
                TaskStatus::Failed,
                TaskStatus::Cancelled
            ]
        );
        assert_eq!(valid_targets(TaskStatus::Done), &[]);
        assert_eq!(valid_targets(TaskStatus::Failed), &[]);
        assert_eq!(valid_targets(TaskStatus::Cancelled), &[]);
    }

    /// The one edge the design deliberately omits: `blocked → running` (a
    /// blocked task always returns through `queued` for re-assignment, never
    /// resuming straight against a session that may no longer exist by the
    /// time the block is resolved).
    ///
    /// `running → cancelled` used to be a second deliberately-missing edge
    /// here (hence this test's former name, `..._two_..._edges_are_absent`),
    /// until this slice added it for acknowledged cooperative cancellation.
    /// The table alone does not make that edge safe — see
    /// `running_to_cancelled_is_present_for_acknowledged_cancellation_only`
    /// below, and `complete::acknowledge_cancellation`'s doc comment, for the
    /// guard that keeps it cooperative rather than a force-stop.
    #[test]
    fn the_deliberately_missing_edge_is_absent() {
        assert!(!is_valid_transition(
            TaskStatus::Blocked,
            TaskStatus::Running
        ));
    }

    /// The companion to the test above: `running → cancelled` is now *in*
    /// the table, on purpose, for `complete::acknowledge_cancellation` — the
    /// only function in this crate that writes it. This table entry is one
    /// half of what keeps that edge cooperative rather than a force-stop;
    /// the other half is that function's own `cancel_requested_at IS NOT
    /// NULL` guard, checked inside its transaction, which this test cannot
    /// see from here (it is exercised in `tests/complete.rs`, including the
    /// task report's mutation 5, which drops that guard).
    #[test]
    fn running_to_cancelled_is_present_for_acknowledged_cancellation_only() {
        assert!(is_valid_transition(
            TaskStatus::Running,
            TaskStatus::Cancelled
        ));
    }

    #[test]
    fn only_done_failed_cancelled_are_terminal() {
        assert!(TaskStatus::Done.is_terminal());
        assert!(TaskStatus::Failed.is_terminal());
        assert!(TaskStatus::Cancelled.is_terminal());
        assert!(!TaskStatus::Queued.is_terminal());
        assert!(!TaskStatus::Running.is_terminal());
        assert!(
            !TaskStatus::Blocked.is_terminal(),
            "`blocked` reads like an ending but design §2.4 does not count it as one"
        );
    }

    /// `factory_session::on_task_terminal` inlines its own terminal-status
    /// check (`"done" | "failed" | "cancelled"`) rather than depending on
    /// this crate — this crate depends on `factory-session`, not the other
    /// way around, so it cannot call in. This test is the guard against the
    /// two lists drifting apart: for every status, `on_task_terminal`'s
    /// verdict (does it refuse with `TaskNotTerminal`, or proceed?) must
    /// agree with [`TaskStatus::is_terminal`]'s verdict on the same status.
    ///
    /// A fresh session per status is required, not one reused across the
    /// loop: `on_task_terminal(Temporary)` on a terminal task calls
    /// `factory_session::stop`, which is only legal from `running` or
    /// `disconnected` — reusing an already-stopped session would fail the
    /// next iteration with `InvalidTransition` instead of exercising the
    /// comparison this test exists to make.
    #[test]
    fn is_terminal_agrees_with_factory_session_on_task_terminal() {
        use factory_paths::CanonicalPath;

        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = factory_store::Store::open(dir.path()).expect("open");

        let scope_id =
            uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000100").expect("valid uuid");
        {
            let tx = store.transaction().expect("begin");
            tx.execute(
                "INSERT INTO scopes (id, name, declared_path, canonical_path) \
                 VALUES (?1, 'irrlicht', ?2, ?2)",
                (
                    scope_id.to_string(),
                    dir.path().to_string_lossy().into_owned(),
                ),
            )
            .expect("insert scope");
            tx.commit().expect("commit");
        }

        for (seed, status) in [
            (1u32, TaskStatus::Queued),
            (2, TaskStatus::Running),
            (3, TaskStatus::Blocked),
            (4, TaskStatus::Done),
            (5, TaskStatus::Failed),
            (6, TaskStatus::Cancelled),
        ] {
            let session_id = uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}"))
                .expect("valid uuid");
            // A fresh workspace per iteration, not one shared across the
            // loop: `on_task_terminal` only releases the previous
            // iteration's lease when its status was terminal, so a shared
            // workspace would fail every non-terminal iteration's
            // `begin_start` with `WorkspaceLeased` before the comparison
            // this test exists to make is ever reached.
            let workspace_dir = dir.path().join(format!("workspace-{seed}"));
            std::fs::create_dir(&workspace_dir).expect("create workspace");
            let workspace = CanonicalPath::resolve(&workspace_dir).expect("resolve workspace");
            factory_session::begin_start(&mut store, session_id, scope_id, "agent", 10, &workspace)
                .expect("start a fresh session for this iteration");
            factory_session::mark_running(&mut store, session_id).expect("readiness observed");

            let task_id = uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-1{seed:011x}"))
                .expect("valid uuid");
            let blocked_reason = (status == TaskStatus::Blocked).then_some("clarification");
            let assigned_session_id = (status == TaskStatus::Running).then_some(session_id);
            {
                let tx = store.transaction().expect("begin");
                tx.execute(
                    "INSERT INTO tasks \
                     (id, target_scope_id, assigned_session_id, prompt, status, blocked_reason) \
                     VALUES (?1, ?2, ?3, 'do it', ?4, ?5)",
                    (
                        task_id.to_string(),
                        scope_id.to_string(),
                        assigned_session_id.map(|s| s.to_string()),
                        status.as_db_str(),
                        blocked_reason,
                    ),
                )
                .expect("insert task");
                tx.commit().expect("commit");
            }

            let result = factory_session::on_task_terminal(
                &mut store,
                session_id,
                task_id,
                factory_config::Lifetime::Temporary,
            );

            assert_eq!(
                result.is_ok(),
                status.is_terminal(),
                "status {status} disagrees: on_task_terminal returned {result:?}, \
                 but TaskStatus::is_terminal() said {}",
                status.is_terminal()
            );
        }
    }

    #[test]
    fn authoritative_blocked_observation_maps_to_the_matching_reason() {
        let obs = observation(
            Confidence::Authoritative,
            TaskSignal::Blocked(AdapterBlockedReason::Permission),
        );
        assert_eq!(blocked_reason_for(&obs), Some(BlockedReason::Permission));
    }

    #[test]
    fn degraded_confidence_never_produces_a_reason_even_when_blocked() {
        let obs = observation(
            Confidence::Degraded,
            TaskSignal::Blocked(AdapterBlockedReason::Clarification),
        );
        assert_eq!(
            blocked_reason_for(&obs),
            None,
            "Degraded is recorded, never acted on"
        );
    }

    #[test]
    fn unavailable_confidence_never_produces_a_reason() {
        let obs = observation(Confidence::Unavailable, TaskSignal::NoChange);
        assert_eq!(blocked_reason_for(&obs), None);
    }

    #[test]
    fn a_non_blocked_signal_never_produces_a_reason_even_when_authoritative() {
        let obs = observation(Confidence::Authoritative, TaskSignal::Running);
        assert_eq!(blocked_reason_for(&obs), None);
    }

    /// Every `factory_adapter::BlockedReason` variant must map to a string
    /// `tasks.blocked_reason`'s CHECK accepts — proved against a real
    /// database, not merely by the match arms existing.
    #[test]
    fn every_adapter_blocked_reason_maps_to_a_check_accepted_string() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = factory_store::Store::open(dir.path()).expect("open");
        let scope_id = "00000000-0000-4000-8000-000000000200";
        {
            let tx = store.transaction().expect("begin");
            tx.execute(
                "INSERT INTO scopes (id, name, declared_path, canonical_path) \
                 VALUES (?1, 'irrlicht', '/instance', '/instance')",
                [scope_id],
            )
            .expect("insert scope");
            tx.commit().expect("commit");
        }

        for (n, variant) in [
            AdapterBlockedReason::Clarification,
            AdapterBlockedReason::Permission,
            AdapterBlockedReason::Interrupted,
            AdapterBlockedReason::External,
        ]
        .into_iter()
        .enumerate()
        {
            let obs = observation(Confidence::Authoritative, TaskSignal::Blocked(variant));
            let reason = blocked_reason_for(&obs).expect("Authoritative + Blocked always maps");

            let tx = store.transaction().expect("begin");
            tx.execute(
                "INSERT INTO tasks (id, target_scope_id, prompt, status, blocked_reason) \
                 VALUES (?1, ?2, 'do it', 'blocked', ?3)",
                (format!("task-{n}"), scope_id, reason.as_db_str()),
            )
            .unwrap_or_else(|e| {
                panic!(
                    "mapped string {:?} was rejected by the blocked_reason CHECK: {e}",
                    reason.as_db_str()
                )
            });
            tx.commit().expect("commit");
        }
    }
}
