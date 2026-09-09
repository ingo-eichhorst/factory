//! The delegation-completion notice (crate docs decision 6): when a task
//! whose `sender_scope_id` is `Some` — the field that actually distinguishes
//! "delegated" from merely "chained," see `lib.rs`'s payload table — reaches
//! a terminal state, the delegating scope's one `running` session is told,
//! through the same journal-then-write *ordering* [`factory_task::deliver::deliver`]
//! uses, carrying nothing but the task id and its outcome.
//!
//! This does not call [`factory_task::deliver::deliver`] itself: that
//! function refuses any task that is not `queued` with an unconsumed
//! authorisation, and the task this module is called for is terminal by
//! construction — `deliver`'s own precondition cannot hold. The *pattern* —
//! record the attempt, commit, only then write, then record the outcome — is
//! reused directly against `delivery_attempts`, because that is the only
//! durable delivery journal this codebase has, and decision 6 is explicit
//! that this must not grow a second one (no message table, no thread,
//! nothing readable back except the task record itself).
//!
//! The extra `delivery_attempts` row this leaves behind is inert to every
//! consumer that reads that table: `factory_task::deliver::deliver`'s own
//! at-most-once guard, `factory_recovery::restore::reconcile`'s "queued with
//! an attempt" row, and `factory_recovery::harness::task_under_review_of` all
//! key on `tasks.status = 'queued'`, and the task this row is journalled
//! against is terminal, never queued again.
//!
//! Every failure mode here is a silent skip, never a propagated error — a
//! notice is a best-effort courtesy, not part of what makes the completion
//! durable: "[i]f the delegating session is gone or busy, the notice is
//! skipped — the task record is still the durable truth, and `factory task
//! show` still answers" (task brief).

use factory_adapter::PaneId;

/// Tell the scope that delegated `task` that it reached a terminal state, if
/// there is anyone to tell right now. Never fails outward.
pub(crate) fn notify_delegator(
    h: &crate::handler::FactoryHandler,
    store: &mut factory_store::Store,
    task: &factory_task::create::Task,
) {
    let Some(sender_scope_id) = task.sender_scope_id else {
        return;
    };
    let Some(session_id) = running_session_in_scope(store, sender_scope_id) else {
        return;
    };
    let Ok((pane, _)) = crate::handler::pane::read(store, session_id) else {
        return;
    };
    let Some(pane) = pane else {
        return;
    };

    let text = format!(
        "[task {}] delegated task reached `{}`",
        task.id, task.status
    );

    let Ok(attempt_row_id) = journal_attempt(store, task.id, session_id) else {
        return;
    };

    // `Adapter::send` refusing `SessionBusy` (the session is mid-turn) is
    // exactly the "busy" half of "gone or busy" this module skips on — not
    // an error to surface, and not retried.
    let sent = h.adapter().send(&PaneId(pane), task.id, &text).is_ok();
    let _ = record_outcome(store, attempt_row_id, sent);
}

/// The first `running` session in `scope_id`, oldest first.
fn running_session_in_scope(
    store: &factory_store::Store,
    scope_id: uuid::Uuid,
) -> Option<uuid::Uuid> {
    let mut stmt = store
        .connection()
        .prepare("SELECT id FROM sessions WHERE scope_id = ?1 AND state = 'running' ORDER BY rowid")
        .ok()?;
    let mut rows = stmt
        .query_map([scope_id.to_string()], |row| {
            let id: String = row.get(0)?;
            Ok(id)
        })
        .ok()?;
    let first = rows.next()?.ok()?;
    uuid::Uuid::parse_str(&first).ok()
}

/// Transaction 1 of the journal-then-write pattern: record the attempt and
/// commit it *before* the adapter is ever called.
fn journal_attempt(
    store: &mut factory_store::Store,
    task_id: uuid::Uuid,
    session_id: uuid::Uuid,
) -> Result<i64, ()> {
    let tx = store.transaction().map_err(|_| ())?;
    tx.execute(
        "INSERT INTO delivery_attempts (task_id, session_id, outcome) VALUES (?1, ?2, NULL)",
        (task_id.to_string(), session_id.to_string()),
    )
    .map_err(|_| ())?;
    let id = tx.last_insert_rowid();
    tx.commit().map_err(|_| ())?;
    Ok(id)
}

/// Transaction 2: record what actually happened, on the same row, by its id.
fn record_outcome(
    store: &mut factory_store::Store,
    attempt_row_id: i64,
    sent: bool,
) -> Result<(), ()> {
    let tx = store.transaction().map_err(|_| ())?;
    tx.execute(
        "UPDATE delivery_attempts SET outcome = ?2 WHERE id = ?1",
        (attempt_row_id, if sent { "sent" } else { "failed" }),
    )
    .map_err(|_| ())?;
    tx.commit().map_err(|_| ())?;
    Ok(())
}
