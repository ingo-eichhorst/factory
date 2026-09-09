//! Reading and writing the two correlation columns migration 5 added to
//! `sessions` (`herdr_pane_id`, `harness_session_id`) from outside
//! `factory-session` — the same boundary
//! [`factory_recovery::reconnect::record_confirmed_identity`] already
//! crosses, for a parallel reason: nothing in `factory_session::begin_start`'s
//! own `INSERT` writes either column (that function predates migration 5, and
//! widening it is `factory-session`'s call, not this crate's to make), and
//! this crate — like `factory_recovery`'s reconnect module — is the one place
//! a live `Adapter::start` result reaches a place to record it at all. This
//! boundary call is reported in this task's own report, per the task brief's
//! instruction for exactly this situation.
//!
//! Persisting the pane immediately after a successful `Adapter::start`
//! ([`ops::agent::start`]), rather than keeping it only in an in-process map,
//! is what lets [`crate::observe`]'s loop — and a future daemon restart's
//! reconnection through `factory_recovery::reconnect` — find a session's pane
//! at all. An in-memory map is empty the moment the process that built it
//! restarts, exactly the "sessions to re-adopt" case ADR 0014's own open item
//! names.

use crate::envelope::ErrorBody;
use crate::errors;

fn store_err(e: impl std::fmt::Display) -> ErrorBody {
    errors::err("internal.store_error", e.to_string())
}

/// `(herdr_pane_id, harness_session_id)` for `session_id`. Both `None` if
/// neither has ever been recorded, or if `session_id` names no row at all —
/// this is a best-effort read, not an existence check.
pub(crate) fn read(
    store: &factory_store::Store,
    session_id: uuid::Uuid,
) -> Result<(Option<String>, Option<String>), ErrorBody> {
    let mut stmt = store
        .connection()
        .prepare("SELECT herdr_pane_id, harness_session_id FROM sessions WHERE id = ?1")
        .map_err(store_err)?;
    let mut rows = stmt
        .query_map([session_id.to_string()], |row| {
            let pane: Option<String> = row.get(0)?;
            let harness_session_id: Option<String> = row.get(1)?;
            Ok((pane, harness_session_id))
        })
        .map_err(store_err)?;
    match rows.next() {
        Some(row) => row.map_err(store_err),
        None => Ok((None, None)),
    }
}

/// Persist `pane` and, when the adapter reported one, `harness_session_id`
/// for `session_id`. Only ever fills a `NULL` — never overwrites a value
/// already recorded — mirroring `record_confirmed_identity`'s own `COALESCE`
/// choice and its reasoning: there is nothing here for a later, weaker
/// observation to clobber an established identity with.
pub(crate) fn record(
    store: &mut factory_store::Store,
    session_id: uuid::Uuid,
    pane: &str,
    harness_session_id: Option<&str>,
) -> Result<(), ErrorBody> {
    let tx = store.transaction().map_err(store_err)?;
    tx.execute(
        "UPDATE sessions SET herdr_pane_id = COALESCE(herdr_pane_id, ?2), \
         harness_session_id = COALESCE(harness_session_id, ?3) WHERE id = ?1",
        (session_id.to_string(), pane, harness_session_id),
    )
    .map_err(store_err)?;
    tx.commit().map_err(store_err)?;
    Ok(())
}
