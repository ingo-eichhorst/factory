//! Task endings, and the two statuses that are not endings (design §2.4,
//! §5; backlog §7).
//!
//! - `done` / `failed` ([`done`] / [`fail`]) close a task with a result:
//!   design §5 step 5, "store the final state and result when observed."
//! - `blocked` ([`blocked`] / [`blocked_from_observation`]) is **not**
//!   terminal (see [`crate::TaskStatus::is_terminal`]) — it is a task
//!   waiting for a human, not an ending.
//! - Acknowledged cancellation ([`acknowledge_cancellation`]) is the other
//!   non-obvious ending: `create::cancel` on a `running` task only *records*
//!   `cancel_requested_at` and leaves the task `running` (design §2.4:
//!   cancellation is cooperative), so something has to let an agent's
//!   acknowledgement actually finish the job. That is this module's, not
//!   `create`'s.
//!
//! Every status write below goes through `lib.rs`'s transition table
//! (`is_valid_transition`) — never straight SQL — per this crate's own house
//! rule.

use rusqlite::OptionalExtension;

use crate::events::EventType;
use crate::{
    BlockedReason, MAX_RESULT_ARTIFACT_PATHS_BYTES, MAX_RESULT_SUMMARY_BYTES, TaskError,
    TaskStatus, is_valid_transition, valid_targets,
};

/// Describe `valid_targets(from)` for an `InvalidTransition` message, the
/// same shape `create::describe_targets` and
/// `factory_session::transition_in_tx` both already use. `create.rs` is not
/// this slice's file to add a `pub(crate)` export to, so this copy lives
/// here instead and `deliver::mark_running` reaches it as
/// `crate::complete::describe_targets` — one definition inside this slice's
/// own two files, not three.
pub(crate) fn describe_targets(from: TaskStatus) -> String {
    let allowed = valid_targets(from);
    if allowed.is_empty() {
        "nothing — this is a terminal state".to_string()
    } else {
        allowed
            .iter()
            .map(TaskStatus::to_string)
            .collect::<Vec<_>>()
            .join(" or ")
    }
}

/// Fetch and parse `tasks.status` for `id`, inside an already-open
/// transaction. [`TaskError::NotFound`] when no such row exists.
fn fetch_status(tx: &rusqlite::Transaction<'_>, id: uuid::Uuid) -> Result<TaskStatus, TaskError> {
    let status: Option<String> = tx
        .query_row(
            "SELECT status FROM tasks WHERE id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(factory_store::StoreError::from)?;
    let Some(status) = status else {
        return Err(TaskError::NotFound(id));
    };
    Ok(TaskStatus::from_db_str(&status))
}

/// Everything [`done`], [`fail`], [`blocked`], and
/// [`acknowledge_cancellation`] can return beyond the ordinary transition
/// errors already in [`TaskError`].
#[derive(Debug, thiserror::Error)]
pub enum CompleteError {
    #[error(transparent)]
    Task(#[from] TaskError),

    #[error(
        "task {0} has neither a result summary nor artifact paths\n  help: backlog §7: completion stores `done`/`failed` \"plus a result summary or artifact paths\" — pass at least one"
    )]
    NoResult(uuid::Uuid),

    #[error(
        "task {id} is `{status}`, not `running`\n  help: acknowledged cancellation only finishes a cooperative running-task cancellation; a queued or blocked task is already moved straight to `cancelled` by `create::cancel`"
    )]
    NotRunning { id: uuid::Uuid, status: TaskStatus },

    #[error(
        "task {0} has no recorded cancellation request\n  help: `running → cancelled` requires `cancel_requested_at` to be set first, by `create::cancel` on a running task; this function will not force-stop a session that was never asked to stop"
    )]
    CancellationNotRequested(uuid::Uuid),
}

/// Validate a completion result against `lib.rs`'s two size limits, counting
/// **bytes** — `str::len()` and the length of the serialized JSON array, not
/// `chars().count()` — because [`MAX_RESULT_SUMMARY_BYTES`] and
/// [`MAX_RESULT_ARTIFACT_PATHS_BYTES`] are documented there as byte limits.
/// A multi-byte character must not slip past a char-counting check that
/// silently allows more storage than the limit says; see
/// `a_multibyte_summary_is_counted_in_bytes_not_chars` in
/// `tests/complete.rs` for the direct evidence, and this task's report
/// mutation 4 for what happens when this counts `chars()` instead.
///
/// Overflow is refused, never truncated — both limits' own doc comments in
/// `lib.rs` say the same thing, and a silent truncation would destroy the
/// evidence a result existed to carry.
fn validate_result(
    result_summary: Option<&str>,
    result_artifact_paths: Option<&[String]>,
) -> Result<(Option<String>, Option<String>), TaskError> {
    if let Some(summary) = result_summary {
        let len = summary.len();
        if len > MAX_RESULT_SUMMARY_BYTES {
            return Err(TaskError::ResultSummaryTooLarge {
                len,
                max: MAX_RESULT_SUMMARY_BYTES,
            });
        }
    }

    let artifact_json = match result_artifact_paths {
        Some(paths) => {
            let json = serde_json::to_string(paths)
                .expect("Vec<String> / &[String] always serializes to JSON");
            let len = json.len();
            if len > MAX_RESULT_ARTIFACT_PATHS_BYTES {
                return Err(TaskError::ResultArtifactPathsTooLarge {
                    len,
                    max: MAX_RESULT_ARTIFACT_PATHS_BYTES,
                });
            }
            Some(json)
        }
        None => None,
    };

    Ok((result_summary.map(str::to_string), artifact_json))
}

/// The body of [`done`] and [`fail`]: validate the result, check the
/// transition, write it — all state at once, with `result_summary` and
/// `result_artifact_paths` set, `blocked_reason` cleared, and `status` moved
/// to `to`.
fn complete_with_result(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    to: TaskStatus,
    result_summary: Option<&str>,
    result_artifact_paths: Option<&[String]>,
) -> Result<(), CompleteError> {
    debug_assert!(matches!(to, TaskStatus::Done | TaskStatus::Failed));

    // Not just `is_none()`: `Some("")` and `Some(&[])` are, in substance,
    // still no result — backlog §7's "plus a result summary or artifact
    // paths" is a claim about content, not about which argument was passed.
    // Treating an explicitly-empty value as satisfying the requirement would
    // let a caller spell "no result" in a way this check does not catch.
    let has_summary = result_summary.is_some_and(|s| !s.is_empty());
    let has_paths = result_artifact_paths.is_some_and(|p| !p.is_empty());
    if !has_summary && !has_paths {
        return Err(CompleteError::NoResult(id));
    }

    // Validated before the transaction opens, same reasoning as
    // `factory_session::begin_start`'s `FileId::of` call before its own
    // `store.transaction()`: no reason to hold the write lock while doing
    // work that does not touch the database.
    let (summary, artifact_json) = validate_result(result_summary, result_artifact_paths)?;

    let tx = store.transaction().map_err(TaskError::from)?;

    let current = fetch_status(&tx, id)?;
    if current.is_terminal() {
        return Err(TaskError::AlreadyTerminal {
            id,
            status: current,
        }
        .into());
    }
    if !is_valid_transition(current, to) {
        return Err(TaskError::InvalidTransition {
            id,
            from: current,
            to,
            allowed: describe_targets(current),
        }
        .into());
    }

    // The event records *that* a result was reported, not the result. The
    // text itself already lives one column away in `tasks.result_summary`,
    // reachable from the event by `task_id`, so copying it here buys a reader
    // nothing and costs a second permanent copy: `task_events` is append-only
    // and is never corrected or deleted, so a summary written here outlives
    // any later edit of the task's own. Up to 16 KiB of agent-authored free
    // text, duplicated forever, is the wrong default for a log whose payloads
    // are meant to be short structured facts (crate decision 6).
    //
    // `has_summary` is still what decides whether there is a payload at all,
    // reusing the check [`CompleteError::NoResult`] already makes rather than
    // re-deriving it from `summary`. `result_artifact_paths` is absent for
    // the same reason as the summary, and more obviously so.
    let event_payload =
        has_summary.then(|| serde_json::json!({ "result_summary_present": true }).to_string());

    tx.execute(
        "UPDATE tasks SET status = ?2, blocked_reason = NULL, result_summary = ?3, \
         result_artifact_paths = ?4, updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
        (id.to_string(), to.as_db_str(), summary, artifact_json),
    )
    .map_err(factory_store::StoreError::from)
    .map_err(TaskError::from)?;

    let event_type = match to {
        TaskStatus::Done => EventType::Done,
        TaskStatus::Failed => EventType::Failed,
        _ => unreachable!("this function's own debug_assert restricts `to` to Done or Failed"),
    };
    crate::events::append(&tx, id, event_type, None, event_payload.as_deref())
        .map_err(TaskError::from)?;

    tx.commit()
        .map_err(factory_store::StoreError::from)
        .map_err(TaskError::from)?;
    Ok(())
}

/// Design §5 step 5: `queued → done`/`running → done` with a result summary
/// or artifact paths (or both — at least one is required, see
/// [`CompleteError::NoResult`]).
///
/// `result_artifact_paths` is stored as a JSON array (`serde_json`), never
/// parsed back by this crate — `create::Task::result_artifact_paths`'s own
/// doc comment already says the same about the column.
pub fn done(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    result_summary: Option<&str>,
    result_artifact_paths: Option<&[String]>,
) -> Result<(), CompleteError> {
    complete_with_result(
        store,
        id,
        TaskStatus::Done,
        result_summary,
        result_artifact_paths,
    )
}

/// The `failed` twin of [`done`] — same result rules, same size limits, same
/// at-least-one-of requirement.
pub fn fail(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    result_summary: Option<&str>,
    result_artifact_paths: Option<&[String]>,
) -> Result<(), CompleteError> {
    complete_with_result(
        store,
        id,
        TaskStatus::Failed,
        result_summary,
        result_artifact_paths,
    )
}

/// Move a non-terminal task to `blocked` with `reason`. Design §2.4:
/// `blocked` is not terminal — it is "a task waiting for a human," per this
/// module's own doc comment, not an ending; a later resolution returns it to
/// `queued` for ordinary re-assignment (`lib.rs`'s `blocked → queued` edge,
/// which this module does not implement — that belongs wherever an
/// operator's answer or permission grant is recorded).
pub fn blocked(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    reason: BlockedReason,
) -> Result<(), CompleteError> {
    let tx = store.transaction().map_err(TaskError::from)?;

    let current = fetch_status(&tx, id)?;
    if current.is_terminal() {
        return Err(TaskError::AlreadyTerminal {
            id,
            status: current,
        }
        .into());
    }
    if !is_valid_transition(current, TaskStatus::Blocked) {
        return Err(TaskError::InvalidTransition {
            id,
            from: current,
            to: TaskStatus::Blocked,
            allowed: describe_targets(current),
        }
        .into());
    }

    tx.execute(
        "UPDATE tasks SET status = 'blocked', blocked_reason = ?2, updated_at = CURRENT_TIMESTAMP \
         WHERE id = ?1",
        (id.to_string(), reason.as_db_str()),
    )
    .map_err(factory_store::StoreError::from)
    .map_err(TaskError::from)?;

    // `crate`'s station-11 decision 3, same transaction as the status write.
    // The brief's own words: "a blocked event carries its reason."
    let payload = serde_json::json!({ "reason": reason.as_db_str() }).to_string();
    crate::events::append(&tx, id, EventType::Blocked, None, Some(&payload))
        .map_err(TaskError::from)?;

    tx.commit()
        .map_err(factory_store::StoreError::from)
        .map_err(TaskError::from)?;
    Ok(())
}

/// Observation-driven blocking: the automatic half of [`blocked`].
///
/// Calls `lib.rs`'s [`crate::blocked_reason_for`] — never re-implements its
/// rule — which returns `None` for anything except
/// `factory_adapter::Confidence::Authoritative` paired with
/// `factory_adapter::TaskSignal::Blocked`. `Degraded` is documented in
/// `factory_adapter` as "recorded, never acted on: at-most-once delivery
/// must not be built on a heuristic," and `Unavailable` is no answer at all
/// ("the caller falls back to manual confirmation") — both, like a
/// non-`Blocked` signal, leave the task **untouched** here, for manual
/// handling, exactly as `factory_adapter`'s own contract requires. Returns
/// `Ok(None)` in that case rather than an error: declining to act
/// automatically is the correct, non-exceptional outcome for a `Degraded` or
/// `Unavailable` observation, not a failure.
pub fn blocked_from_observation(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    observation: &factory_adapter::Observation,
) -> Result<Option<BlockedReason>, CompleteError> {
    let Some(reason) = crate::blocked_reason_for(observation) else {
        return Ok(None);
    };
    blocked(store, id, reason)?;
    Ok(Some(reason))
}

/// Let a `running` task's cooperative cancellation actually finish.
///
/// `create::cancel` on a `running` task only records `cancel_requested_at`
/// and leaves `status = 'running'` (design §2.4: cancellation is
/// cooperative once a task is running). This function is what an agent's
/// acknowledgement of that request calls once it has stopped — the *only*
/// function in this crate that writes the `running → cancelled` edge
/// `lib.rs`'s `valid_targets` now permits (this slice's one authorized
/// change to that file).
///
/// **Documented at both sites, on purpose, per this slice's brief:**
/// `lib.rs`'s table permits `running → cancelled` unconditionally — that is
/// only half the rule. The other half is here: this function requires
/// `cancel_requested_at IS NOT NULL`, checked inside the same transaction as
/// the status write, before it will use that edge
/// ([`CompleteError::CancellationNotRequested`] when it is NULL). A running
/// task with no recorded request cannot reach `cancelled` through this
/// function — the table alone would allow the move; this guard is what
/// keeps it cooperative rather than the force-stop design §2.4 reserves as
/// "a separate user action."
///
/// Restricted to `current == Running`
/// ([`CompleteError::NotRunning`] otherwise) rather than deferring entirely
/// to `is_valid_transition`: a `queued` or `blocked` task can *also* reach
/// `cancelled` per the table, but through `create::cancel`'s immediate path,
/// not this one — conflating the two would let a caller "acknowledge" a
/// cancellation that was never cooperative to begin with.
pub fn acknowledge_cancellation(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
) -> Result<(), CompleteError> {
    let tx = store.transaction().map_err(TaskError::from)?;

    let row: Option<(String, Option<String>)> = tx
        .query_row(
            "SELECT status, cancel_requested_at FROM tasks WHERE id = ?1",
            [id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(factory_store::StoreError::from)
        .map_err(TaskError::from)?;
    let Some((status, cancel_requested_at)) = row else {
        return Err(TaskError::NotFound(id).into());
    };
    let current = TaskStatus::from_db_str(&status);

    if current.is_terminal() {
        return Err(TaskError::AlreadyTerminal {
            id,
            status: current,
        }
        .into());
    }
    if current != TaskStatus::Running {
        return Err(CompleteError::NotRunning {
            id,
            status: current,
        });
    }

    // Defence in depth, per this crate's house rule ("everything writes
    // through the transition table... do not write a status with raw SQL
    // that bypasses `is_valid_transition`"): `current == Running` already
    // guarantees today's table allows this move (see
    // `running_to_cancelled_is_present_for_acknowledged_cancellation_only`
    // in `lib.rs`), but the call stays so a future regression in the table
    // is caught here too, not only by that unit test.
    if !is_valid_transition(current, TaskStatus::Cancelled) {
        return Err(TaskError::InvalidTransition {
            id,
            from: current,
            to: TaskStatus::Cancelled,
            allowed: describe_targets(current),
        }
        .into());
    }

    // The guard: the table permits the move; this is what keeps it
    // cooperative. See this function's doc comment for the other half of
    // this same rule, documented in `lib.rs`.
    if cancel_requested_at.is_none() {
        return Err(CompleteError::CancellationNotRequested(id));
    }

    tx.execute(
        "UPDATE tasks SET status = 'cancelled', updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
        [id.to_string()],
    )
    .map_err(factory_store::StoreError::from)
    .map_err(TaskError::from)?;

    // `crate`'s station-11 decision 3, same transaction as the status write.
    // No payload: nothing beyond "this task is now cancelled" is known here
    // that the row itself does not already carry.
    crate::events::append(&tx, id, EventType::Cancelled, None, None).map_err(TaskError::from)?;

    tx.commit()
        .map_err(factory_store::StoreError::from)
        .map_err(TaskError::from)?;
    Ok(())
}
