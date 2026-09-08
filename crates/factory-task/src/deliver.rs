//! Task delivery: design §5 steps 3 and 4, backlog §7 (durable task queue and
//! single-session delivery).
//!
//! **There is no PTY automation in station 7.** The backlog's scope line
//! keeps a manual operator handoff until terminal input is automated:
//! delivery goes through the [`PromptWriter`] trait, and the implementation
//! this slice ships ([`OperatorPromptWriter`]) renders the prompt for an
//! operator to paste by hand. Nothing here calls `herdr send-keys` or
//! anything that types into a terminal.
//!
//! The order below is the entire point of this module (design §5, read
//! twice before changing anything here):
//!
//! 1. refuse unless the task is `queued` and carries a non-NULL
//!    `assigned_session_id` ([`DeliverError::NotQueued`] /
//!    [`DeliverError::NotAssigned`]);
//! 2. refuse if the task's recorded `delivery_attempts` rows already number
//!    **at least** `tasks.authorised_deliveries` ([`DeliverError::AlreadyAttempted`])
//!    — delivery is at-most-once *per authorisation*. Every task is created
//!    with exactly one authorisation (`factory_store::schema` migration 4
//!    backfills and defaults `authorised_deliveries` to `1`), so for a task
//!    nothing has ever resumed this is byte-for-byte the slice 7 rule it
//!    replaces ("refuse any task that already carries a `delivery_attempts`
//!    row"): one attempt already meets a limit of one. [`authorise_resume`]
//!    is the only way that limit ever moves, and it moves it by exactly one
//!    human action at a time — see its own doc comment for why a *failed*
//!    attempt still consumes the authorisation it was given, and why that is
//!    not a bug;
//! 3. **insert the `delivery_attempts` row with `outcome` NULL and commit
//!    it, in its own transaction, before calling the writer.** A crash
//!    between that commit and the writer call must leave a row saying an
//!    attempt was made and its result is unknown — that row is the only
//!    evidence a prompt *may* have gone out, and ADR 0019 decision 2 treats a
//!    `queued` task carrying a delivery attempt as possibly-delivered after a
//!    restore precisely because this ordering guarantees the row exists
//!    whenever the write might have happened;
//! 4. call the writer, then record the outcome on that same row — success or
//!    failure — in a second, separate transaction. A writer that returns an
//!    error still leaves the row, with an outcome saying so.
//!
//! `deliver` does **not** set the task's status to `running` — design §5
//! step 4 splits "send the task once" from "mark it `running` when
//! observed," and [`mark_running`] is that second half, kept separate so a
//! caller can wait for confirmation between the two. It is the only place in
//! this crate that writes `tasks.status = 'running'`.
//!
//! # A deviation this module found and did not silently fix
//!
//! `lib.rs`'s `valid_targets` doc comment says a re-queued `blocked → queued`
//! task is "re-assigned and re-delivered by the ordinary path with a fresh
//! delivery-attempt row." That is not what this module does, and per this
//! slice's brief it must not: the at-most-once guard below
//! ([`DeliverError::AlreadyAttempted`]) refuses a second `deliver` call for a
//! task id that already has *any* `delivery_attempts` row, regardless of how
//! the task got back to `queued`. Design §5's own recovery text agrees with
//! this module, not with that comment: "Factory does not automatically
//! resend a possibly delivered prompt... A human may resume or **create a
//! replacement task**" — the remedy is a new task id, never a second
//! attempt on the same one. `lib.rs`'s doc comment is stale; this slice's
//! authorization to edit `lib.rs` covers only the cancellation edge, so the
//! comment is reported here rather than changed.

use rusqlite::OptionalExtension;

use crate::{TaskStatus, is_valid_transition};

/// What every prompt-delivery mechanism provides.
///
/// A trait, not a concrete function, because station 7 ships exactly one
/// implementation ([`OperatorPromptWriter`]) while leaving room for a future
/// slice to add PTY automation without changing [`deliver`]'s signature or
/// its transaction ordering.
pub trait PromptWriter {
    /// Deliver `prompt` — which already contains the task UUID, see
    /// `render_prompt` — to `session_id`.
    fn write_prompt(
        &mut self,
        session_id: uuid::Uuid,
        prompt: &str,
    ) -> Result<(), PromptWriteError>;
}

/// A [`PromptWriter`] failed. Carries only a human-readable message, never
/// the prompt or any harness output — this crate does not persist a writer
/// error's text anywhere (see [`DeliveryOutcome`]'s doc comment), and a
/// message-only type keeps that true by construction rather than by
/// discipline at every call site.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct PromptWriteError {
    message: String,
}

impl PromptWriteError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl From<std::io::Error> for PromptWriteError {
    fn from(source: std::io::Error) -> Self {
        Self::new(source.to_string())
    }
}

/// The manual operator handoff: renders the prompt to `out` (typically
/// standard output) for a human to read and paste into the session's
/// terminal by hand. This is the whole of station 7's delivery mechanism —
/// "There is no PTY automation in station 7" (backlog §7's scope line) — so
/// "writing the prompt" means making it visible to a person, nothing more.
pub struct OperatorPromptWriter<W: std::io::Write> {
    out: W,
}

impl<W: std::io::Write> OperatorPromptWriter<W> {
    pub fn new(out: W) -> Self {
        Self { out }
    }
}

impl<W: std::io::Write> PromptWriter for OperatorPromptWriter<W> {
    fn write_prompt(
        &mut self,
        session_id: uuid::Uuid,
        prompt: &str,
    ) -> Result<(), PromptWriteError> {
        writeln!(
            self.out,
            "--- paste into session {session_id} ---\n{prompt}\n--- end of prompt ---"
        )
        .map_err(PromptWriteError::from)
    }
}

/// Backlog §7: "every prompt contains the task UUID," so that work later
/// observed in a terminal can be correlated with its database record
/// (design §5's opening sentence states the same requirement). This is the
/// only place that requirement is met — [`deliver`] hands the writer exactly
/// this string and nothing upstream re-adds the id.
fn render_prompt(task_id: uuid::Uuid, prompt: &str) -> String {
    format!("[task {task_id}]\n{prompt}")
}

/// The two outcomes a delivery attempt can record once the writer has run.
///
/// `NULL` — the value [`deliver`]'s first transaction leaves the row with —
/// is deliberately not a third variant here. It is not an outcome; it is the
/// ADR 0019 evidence state itself: "an attempt was made and its result is
/// unknown," the exact condition a crash between that commit and the writer
/// call leaves behind. Modelling the column as `Option<DeliveryOutcome>` at
/// the SQL boundary keeps "unknown" visibly distinct from "known, and this
/// is what happened" instead of folding it into the enum as a third case.
///
/// Carries no message text — see [`PromptWriteError`]'s doc comment on why a
/// writer's error text is never persisted here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryOutcome {
    Sent,
    Failed,
}

impl DeliveryOutcome {
    fn as_db_str(self) -> &'static str {
        match self {
            Self::Sent => "sent",
            Self::Failed => "failed",
        }
    }
}

/// Everything that can go wrong delivering a task, or observing it running.
#[derive(Debug, thiserror::Error)]
pub enum DeliverError {
    #[error("store error: {0}")]
    Store(#[from] factory_store::StoreError),

    #[error("no task with id {0}")]
    NotFound(uuid::Uuid),

    #[error(
        "task {id} is `{status}`, not `queued`\n  help: delivery happens exactly once, from the `queued` state assignment leaves a task in; a task already running, blocked, or terminal has either already been delivered or was never assigned"
    )]
    NotQueued { id: uuid::Uuid, status: TaskStatus },

    #[error(
        "task {0} has no assigned session\n  help: assignment (choosing an idle session) must run before delivery; this task was never assigned one"
    )]
    NotAssigned(uuid::Uuid),

    #[error(
        "task {0} has no remaining authorised deliveries\n  help: delivery is at-most-once per authorisation (design §5); a second attempt after an ambiguous outcome is exactly what the design forbids — a human may authorise a further delivery with `authorise_resume`, or create a replacement task"
    )]
    AlreadyAttempted(uuid::Uuid),

    #[error("writing the prompt for task {id} failed: {source}")]
    WriteFailed {
        id: uuid::Uuid,
        source: PromptWriteError,
    },

    #[error(
        "task {0} has not been delivered\n  help: `mark_running` observes that a delivered prompt was received; this task has no `delivery_attempts` row at all, so nothing was ever sent"
    )]
    NotDelivered(uuid::Uuid),

    #[error(
        "task {id} cannot move from `{from}` to `{to}`\n  help: `{from}` only reaches {allowed}"
    )]
    InvalidTransition {
        id: uuid::Uuid,
        from: TaskStatus,
        to: TaskStatus,
        allowed: String,
    },
}

/// Perform design §5 steps 3 and 4, in the order documented at the top of
/// this file. See the module docs for the full ordering argument; this
/// function's body is deliberately shaped as two separate transactions
/// around one non-transactional call, and that shape is the entire point.
///
/// Never writes `tasks.status`. In particular, a writer error does **not**
/// move the task to `failed`: `lib.rs`'s `queued → failed` edge exists for a
/// different situation (delivery cannot be attempted at all — see its doc
/// comment), and using it here would mean inferring the prompt failed to
/// reach the agent merely because the writer reported an error, which
/// AGENTS.md forbids ("never infer that an external action failed merely
/// because its response was lost"). Leaving the task `queued` with a
/// `failed`-outcome `delivery_attempts` row is exactly the ambiguous state
/// ADR 0019 decision 2's restore reconciliation is built to recognise.
pub fn deliver<W: PromptWriter>(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    writer: &mut W,
) -> Result<(), DeliverError> {
    // --- Transaction 1: guard, journal the attempt, commit. -----------
    let (session_id, prompt, attempt_row_id) = {
        let tx = store.transaction()?;

        let row: Option<(String, String, Option<String>, i64)> = tx
            .query_row(
                "SELECT status, prompt, assigned_session_id, authorised_deliveries \
                 FROM tasks WHERE id = ?1",
                [id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(factory_store::StoreError::from)?;
        let Some((status, prompt, assigned_session_id, authorised_deliveries)) = row else {
            return Err(DeliverError::NotFound(id));
        };
        let status = TaskStatus::from_db_str(&status);
        if status != TaskStatus::Queued {
            return Err(DeliverError::NotQueued { id, status });
        }
        let Some(assigned_session_id) = assigned_session_id else {
            return Err(DeliverError::NotAssigned(id));
        };
        let session_id = uuid::Uuid::parse_str(&assigned_session_id).unwrap_or_else(|e| {
            panic!("tasks.assigned_session_id is a UUID; read {assigned_session_id:?}: {e}")
        });

        // At-most-once *per authorisation*: refuse once the number of
        // recorded attempts reaches (not merely exceeds) what has been
        // authorised. Every task starts with `authorised_deliveries = 1`
        // (`factory_store::schema` migration 4), so for a never-resumed task
        // this is exactly slice 7's "any row at all" — one existing attempt
        // already meets a limit of one. See this module's doc comment and
        // [`authorise_resume`] for how, and why, that limit ever moves.
        let attempted_count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM delivery_attempts WHERE task_id = ?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .map_err(factory_store::StoreError::from)?;
        if attempted_count >= authorised_deliveries {
            return Err(DeliverError::AlreadyAttempted(id));
        }

        tx.execute(
            "INSERT INTO delivery_attempts (task_id, session_id, outcome) VALUES (?1, ?2, NULL)",
            (id.to_string(), session_id.to_string()),
        )
        .map_err(factory_store::StoreError::from)?;
        let attempt_row_id = tx.last_insert_rowid();

        tx.commit().map_err(factory_store::StoreError::from)?;
        (session_id, prompt, attempt_row_id)
    };

    // --- Not a transaction: the writer runs with no lock held. --------
    let rendered = render_prompt(id, &prompt);
    let write_result = writer.write_prompt(session_id, &rendered);
    let outcome = match &write_result {
        Ok(()) => DeliveryOutcome::Sent,
        Err(_) => DeliveryOutcome::Failed,
    };

    // --- Transaction 2: record the outcome on the same row, by its id. ---
    {
        let tx = store.transaction()?;
        tx.execute(
            "UPDATE delivery_attempts SET outcome = ?2 WHERE id = ?1",
            (attempt_row_id, outcome.as_db_str()),
        )
        .map_err(factory_store::StoreError::from)?;
        tx.commit().map_err(factory_store::StoreError::from)?;
    }

    write_result.map_err(|source| DeliverError::WriteFailed { id, source })
}

/// Design §5 step 4's second half: "mark it `running` when observed." The
/// only place in this crate that writes `tasks.status = 'running'`.
///
/// Mirrors `factory_session::mark_running(store, id)`'s shape: the caller —
/// whatever later confirms the harness picked up the prompt — decides when
/// to call this; this function does not itself talk to an adapter or accept
/// an `Observation`. It does insist the task was actually delivered
/// ([`DeliverError::NotDelivered`] when no `delivery_attempts` row exists at
/// all) — deliberately checking only that a row exists, not that its
/// `outcome` is `sent`: a `NULL`-outcome row plus an external confirmation
/// that the session is running is exactly the crash-between-commit-and-write
/// case this module's ordering exists to leave recoverable evidence for.
pub fn mark_running(store: &mut factory_store::Store, id: uuid::Uuid) -> Result<(), DeliverError> {
    let tx = store.transaction()?;

    let row: Option<(String, Option<String>)> = tx
        .query_row(
            "SELECT status, assigned_session_id FROM tasks WHERE id = ?1",
            [id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(factory_store::StoreError::from)?;
    let Some((status, assigned_session_id)) = row else {
        return Err(DeliverError::NotFound(id));
    };
    let current = TaskStatus::from_db_str(&status);

    if assigned_session_id.is_none() {
        return Err(DeliverError::NotAssigned(id));
    }

    let delivered: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM delivery_attempts WHERE task_id = ?1)",
            [id.to_string()],
            |row| row.get(0),
        )
        .map_err(factory_store::StoreError::from)?;
    if !delivered {
        return Err(DeliverError::NotDelivered(id));
    }

    // Every write in this crate goes through the transition table — never
    // straight to SQL — even here, where `current` is expected to already be
    // `Queued` by the time a caller has something to observe: this is the
    // defence against that expectation being wrong.
    if !is_valid_transition(current, TaskStatus::Running) {
        return Err(DeliverError::InvalidTransition {
            id,
            from: current,
            to: TaskStatus::Running,
            allowed: crate::complete::describe_targets(current),
        });
    }

    tx.execute(
        "UPDATE tasks SET status = 'running', updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
        [id.to_string()],
    )
    .map_err(factory_store::StoreError::from)?;
    tx.commit().map_err(factory_store::StoreError::from)?;
    Ok(())
}

/// The operator action design §5 names as the other way on from an ambiguous
/// delivery — "a human may resume or create a replacement task" — and
/// backlog §9's "Resume has no mechanism yet, found during slice 7." This is
/// that mechanism: the first function in this crate to write the
/// `blocked → queued` edge [`crate::valid_targets`] has always permitted but
/// nothing has used ("the explicit resume... does not exist yet, so a
/// requeued task waits for a human either way," per that table's own doc
/// comment — this function is what ends the wait).
///
/// One human action, one meaning, in one transaction:
///
/// - requires `status = 'blocked'` ([`TaskError::NotBlocked`] otherwise —
///   refused for a task that is `queued` or `running` (nothing to authorise
///   past yet) and for every terminal status alike);
/// - increments `tasks.authorised_deliveries` by exactly one — recording the
///   authorisation *with the task*, per `factory_store::schema` migration
///   4's doc comment, rather than accepting it as a call argument to
///   [`deliver`] that a restart could never see;
/// - moves the task to `queued` and clears `assigned_session_id`, exactly
///   the pairing [`crate::valid_targets`]'s doc comment already describes
///   for this edge — "re-assignment is all it re-enters." Clearing the
///   assignment is what lets the ordinary untargeted-assignment path choose
///   fresh, instead of handing back to a session that may no longer be the
///   right one, or a live one at all, by the time the block is resolved;
/// - clears `blocked_reason`, because `tasks.blocked_reason`'s own CHECK
///   (`factory_store::schema`) requires it to be NULL for every non-`blocked`
///   status — the same clearing [`crate::create::cancel`] already performs
///   for its own `blocked → cancelled` arm, for the identical reason.
///
/// # Why one transaction, not two calls
///
/// The increment and the re-queue are one write on purpose, not two. Splitting
/// them — "record the authorisation," then separately "re-queue the task" —
/// would let a restart land between the two and find a task that is still
/// `blocked` but already carries the incremented count, or one that is
/// `queued` with a fresh budget but no visible reason why. Either state is
/// indistinguishable from an ordinary half-written mutation, which is exactly
/// the ambiguity `authorised_deliveries` exists to remove. A human authorising
/// one further delivery is not the automatic resend design §5 forbids — but a
/// half-resumed task discovered on restart would be precisely as ambiguous as
/// the possibly-delivered prompts this whole mechanism exists to stop
/// guessing about.
///
/// # This does not deliver anything, and does not decide delivery's outcome
///
/// `authorise_resume` only raises the delivery budget and re-queues the task;
/// it does not call [`deliver`] and never touches a [`PromptWriter`]. The
/// ordinary assignment and delivery path picks the re-queued task up exactly
/// as it would any other `queued` task, and [`deliver`]'s own guard (this
/// module's top doc comment, point 2) is what actually spends the
/// authorisation granted here.
///
/// Worth stating plainly, because it is easy to design away by accident: a
/// **failed** delivery still consumes the authorisation it was given. The
/// `delivery_attempts` row is journalled *before* the terminal write (design
/// §5 step 3), specifically so that a crash or a writer error leaves evidence
/// an attempt was made even when its outcome is unknown — which is exactly
/// the case where Factory cannot tell whether the prompt arrived. Refusing to
/// count a failed attempt against the budget would treat "the writer returned
/// an error" as proof no external effect occurred, which is the same
/// inference AGENTS.md forbids ("never infer that an external action failed
/// merely because its response was lost") and which
/// `deliver`'s own doc comment already refuses to draw for `tasks.status`.
/// The conservative reading — one authorisation, one attempt, regardless of
/// outcome — is the only one that keeps `authorised_deliveries` meaning what
/// its name says: a human authorised **an attempt**, not a **successful**
/// one. A future change that only decrements the budget on a confirmed
/// failure would silently reopen the at-most-once guarantee this migration
/// was built to keep intact.
pub fn authorise_resume(
    store: &mut factory_store::Store,
    task_id: uuid::Uuid,
) -> Result<(), crate::TaskError> {
    let tx = store.transaction()?;

    let status: Option<String> = tx
        .query_row(
            "SELECT status FROM tasks WHERE id = ?1",
            [task_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(factory_store::StoreError::from)?;
    let Some(status) = status else {
        return Err(crate::TaskError::NotFound(task_id));
    };
    let current = TaskStatus::from_db_str(&status);

    if current != TaskStatus::Blocked {
        return Err(crate::TaskError::NotBlocked {
            id: task_id,
            status: current,
        });
    }

    // Defence in depth, per this crate's house rule ("everything writes
    // through the transition table... do not write a status with raw SQL
    // that bypasses `is_valid_transition`"): `current == Blocked` already
    // guarantees today's table allows `blocked → queued` (`crate::valid_targets`),
    // but the call stays so a future regression in the table is caught here
    // too, not only by `lib.rs`'s own unit tests.
    if !is_valid_transition(current, TaskStatus::Queued) {
        return Err(crate::TaskError::InvalidTransition {
            id: task_id,
            from: current,
            to: TaskStatus::Queued,
            allowed: crate::complete::describe_targets(current),
        });
    }

    tx.execute(
        "UPDATE tasks SET \
         status = 'queued', \
         blocked_reason = NULL, \
         assigned_session_id = NULL, \
         authorised_deliveries = authorised_deliveries + 1, \
         updated_at = CURRENT_TIMESTAMP \
         WHERE id = ?1",
        [task_id.to_string()],
    )
    .map_err(factory_store::StoreError::from)?;
    tx.commit().map_err(factory_store::StoreError::from)?;
    Ok(())
}
