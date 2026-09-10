//! Verification verdicts (design §12.1's hook; ADR 0021 decision 4, and
//! `crate`'s station-11 decision 4, which this module implements byte for
//! byte — read both before changing anything here).
//!
//! **A verdict annotates. It never transitions.** [`record_verdict`] writes
//! exactly one `verification` row into `task_events` and touches nothing
//! else: no column of `tasks`, no status change, no rework created, no yield
//! computed. Design §11: a verdict "never replaces or overwrites that
//! worker's own completion record." §12.1: "the gate itself remains a human
//! operation."
//!
//! Its one guard is independence, and it is a guard about **scopes, not
//! sessions**. §12.1: "a worker still cannot raise a verification run
//! against another session of *its own scope*. Allowing sibling scopes does
//! not change this: the inspecting session would have to live in a
//! different scope" — design §6's "a scope may not target itself" is the
//! rule this derives from. A verdict whose author is a session is refused
//! when that session's scope is the run's own `target_scope_id`; a verdict
//! with no author session is a human's, and is always allowed.
//!
//! **Deliberately not `tasks.assigned_session_id`.** A session-level guard
//! would permit a *sibling* session of the same scope to verify — exactly
//! what §12.1 rules out — and it has a hole `target_scope_id` does not:
//! `assigned_session_id` is NULL for a task no session ever ran (one a human
//! closed, one cancelled while queued, one whose only delivery attempt was
//! refused), so a guard comparing against it would pass trivially on every
//! one of those. `target_scope_id` is NOT NULL by schema.
//!
//! **This module creates no verification *run*.** ADR 0021 decision 5:
//! version 1 has no agent-initiated inspection, so design §6 needs no
//! exception and none is written here. Nothing in this crate calls
//! [`record_verdict`] — it exists for a caller outside this crate (a future
//! CLI command or operator tool) to reach, the same way `authorise_resume`
//! sat with no caller for a whole station before slice 9's guard finally got
//! one; ADR 0021 decision 5 names that history directly as the reason no
//! exception is written ahead of a concrete caller.

use rusqlite::OptionalExtension;

use crate::{TaskError, events::EventType};

/// Everything that can go wrong recording a verdict.
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error(transparent)]
    Task(#[from] TaskError),

    #[error("session error: {0}")]
    Session(#[from] factory_session::SessionError),

    #[error(
        "session {session_id} cannot verify task {task_id}\n  help: design §12.1's independence rule — a worker cannot raise a verification run against another session of its own scope, and this session's scope is the run's own target scope. Verify from a session in a different scope, or record this verdict with no author session (a human's verdict is always allowed)"
    )]
    NotIndependent {
        session_id: uuid::Uuid,
        task_id: uuid::Uuid,
    },
}

/// Record one verdict against `task_id`.
///
/// `verdict` is a short structured fact, not a report — decision 6 ("no
/// secret and no copied private source content, ever") applies here exactly
/// as it does to every other event payload in this crate: this is not the
/// place for a transcript, a diff, or a rationale that quotes private
/// source. The payload is `{"verdict": verdict}`, nothing else — there is no
/// acceptance-criteria vocabulary to check against in version 1 (§12.1's
/// gate "remains a human operation"), so this module does not constrain
/// `verdict`'s text beyond what `task_events.payload` already requires (JSON).
///
/// # The independence guard, and why the read happens before the transaction
///
/// `author_session_id`'s scope is read via [`factory_session::scope_of_session`]
/// **before** opening this function's own transaction — that function's own
/// doc comment gives the reason ("a caller holding `&Store` cannot also be
/// mid-transaction on the same store"), and reading it early introduces no
/// race: `sessions.scope_id` is written once, by `factory_session::begin_start`'s
/// own INSERT, and nothing in `factory-session` ever runs an `UPDATE
/// sessions SET scope_id` afterwards (confirmed by reading that crate's
/// `lib.rs` in full before writing this function) — a session's scope is a
/// write-once fact, so there is nothing for a transaction boundary to
/// protect it against.
///
/// The task's own `target_scope_id` is read **inside** the transaction that
/// also writes the event, so [`TaskError::NotFound`] and the guard itself
/// are decided against the same open transaction the write commits — the
/// ordinary shape every other function in this crate uses.
///
/// # Errors
///
/// [`VerifyError::Session`] if `author_session_id` is `Some` and names no
/// real session — reported before [`TaskError::NotFound`] would even be
/// checked, since the author's scope is resolved first; a caller passing a
/// nonexistent session and a nonexistent task sees the session error.
///
/// [`TaskError::NotFound`] if `task_id` names no task.
///
/// [`VerifyError::NotIndependent`] if `author_session_id` is `Some` and that
/// session's scope is the run's own `target_scope_id`.
pub fn record_verdict(
    store: &mut factory_store::Store,
    task_id: uuid::Uuid,
    author_session_id: Option<uuid::Uuid>,
    verdict: &str,
) -> Result<(), VerifyError> {
    let author_scope_id = author_session_id
        .map(|session_id| factory_session::scope_of_session(store, session_id))
        .transpose()?;

    let tx = store.transaction().map_err(TaskError::from)?;

    let target_scope_id: Option<String> = tx
        .query_row(
            "SELECT target_scope_id FROM tasks WHERE id = ?1",
            [task_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(factory_store::StoreError::from)
        .map_err(TaskError::from)?;
    let Some(target_scope_id) = target_scope_id else {
        return Err(TaskError::NotFound(task_id).into());
    };
    let target_scope_id = uuid::Uuid::parse_str(&target_scope_id).unwrap_or_else(|e| {
        panic!("tasks.target_scope_id is a UUID; read {target_scope_id:?}: {e}")
    });

    // The guard, decision 4: scope, not session. `author_session_id` is
    // carried through untouched so the error can name the session that was
    // refused, but the comparison is entirely on the two scope ids.
    if let (Some(session_id), Some(author_scope_id)) = (author_session_id, author_scope_id) {
        if author_scope_id == target_scope_id {
            return Err(VerifyError::NotIndependent {
                session_id,
                task_id,
            });
        }
    }

    let payload = serde_json::json!({ "verdict": verdict }).to_string();
    crate::events::append(
        &tx,
        task_id,
        EventType::Verification,
        author_session_id,
        Some(&payload),
    )
    .map_err(TaskError::from)?;

    tx.commit()
        .map_err(factory_store::StoreError::from)
        .map_err(TaskError::from)?;
    Ok(())
}
