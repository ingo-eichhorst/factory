//! The provenance log for the two durable-writing commands (backlog §12,
//! ADR 0022): `factory knowledge write` and `factory memory add`. Every
//! successful write of either kind gets exactly one row in `durable_writes`
//! (`schema::V7_SCHEMA`).
//!
//! # What `append`'s signature guarantees, and what it does not
//!
//! [`append`] takes `&rusqlite::Transaction`, never `&mut Store` — it cannot
//! open or commit a transaction of its own. The row it writes only ever
//! becomes durable through *some caller's* `tx.commit()`. ADR 0022 decision 3
//! depends on exactly this: the daemon writes the note to a temporary file,
//! opens one transaction, calls [`append`], renames the temporary file into
//! place, and only then commits — an order chosen so that a crash leaves
//! either nothing or a file with no provenance row, never a row naming a file
//! that was never written. A version of `append` that could commit on its own
//! would let the row become durable before the rename happens, which is
//! exactly the ordering decision 3 forbids. This module follows the same
//! stance `factory_task::events`'s `append` takes toward `task_events`, for
//! the same reason.
//!
//! It does **not**, by itself, prove that a caller passes the transaction it
//! is about to commit rather than some other one — that is each call site's
//! own responsibility to get right, the same limit `factory_task::events`'s
//! module doc names for itself.

use crate::{Store, StoreError};

/// The two strings `durable_writes.kind`'s CHECK allows (`schema::V7_SCHEMA`),
/// typed so no caller threads a hand-written string past this module's
/// boundary — the same reasoning `factory_task::events::EventType` gives for
/// itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteKind {
    Knowledge,
    Memory,
}

impl WriteKind {
    fn as_db_str(self) -> &'static str {
        match self {
            Self::Knowledge => "knowledge",
            Self::Memory => "memory",
        }
    }

    /// `durable_writes.kind` is constrained by CHECK to exactly these two
    /// strings, so an unrecognised value read back from a row this crate
    /// just selected is a broken invariant, not an input to handle — the
    /// same stance `factory_task::events::EventType::from_db_str` takes.
    fn from_db_str(s: &str) -> Self {
        match s {
            "knowledge" => Self::Knowledge,
            "memory" => Self::Memory,
            other => unreachable!(
                "durable_writes.kind is constrained by CHECK to 'knowledge' or 'memory'; read {other:?}"
            ),
        }
    }
}

impl std::fmt::Display for WriteKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_db_str())
    }
}

/// One `durable_writes` row, as read back by [`list_for_task`] and
/// [`list_for_scope`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableWrite {
    /// The append order — `durable_writes.id`, an `INTEGER PRIMARY KEY`. This
    /// is the ordering both list functions return rows in, not `created_at`:
    /// two writes in the same second are ordinary, the same reasoning
    /// `factory_task::events::Event::id`'s own doc comment gives.
    pub id: i64,
    pub kind: WriteKind,
    pub name: String,
    pub path: String,
    /// `None` for a `knowledge` write; `Some` for a `memory` write. See
    /// `schema::V7_SCHEMA`'s doc comment for why this is a foreign key and
    /// why it is paired with `kind` rather than made `NOT NULL` outright.
    pub scope_id: Option<String>,
    /// `None` means a human at a terminal, not a task — the reason this
    /// table exists rather than a row in `task_events`.
    pub task_id: Option<String>,
    /// `None` means a human or the daemon itself, not a session.
    pub author_session_id: Option<String>,
    pub created_at: String,
}

fn row_to_durable_write(row: &rusqlite::Row<'_>) -> rusqlite::Result<DurableWrite> {
    let id: i64 = row.get(0)?;
    let kind: String = row.get(1)?;
    let name: String = row.get(2)?;
    let path: String = row.get(3)?;
    let scope_id: Option<String> = row.get(4)?;
    let task_id: Option<String> = row.get(5)?;
    let author_session_id: Option<String> = row.get(6)?;
    let created_at: String = row.get(7)?;

    Ok(DurableWrite {
        id,
        kind: WriteKind::from_db_str(&kind),
        name,
        path,
        scope_id,
        task_id,
        author_session_id,
        created_at,
    })
}

/// Append one `durable_writes` row inside `tx` — the caller's own
/// already-open transaction, never one this function opens. See the module
/// docs for why the signature is shaped this way rather than taking
/// `&mut Store`.
///
/// `scope_id` and `kind` are not cross-checked here in Rust: the CHECK on
/// `durable_writes.scope_id` (`schema::V7_SCHEMA`) already refuses a `memory`
/// write with no scope and a `knowledge` write with one, enforced on every
/// connection — the same "one home for a rule" stance
/// `factory_task::events::append` takes toward `task_events.task_id`'s
/// foreign key.
pub fn append(
    tx: &rusqlite::Transaction<'_>,
    kind: WriteKind,
    name: &str,
    path: &str,
    scope_id: Option<&str>,
    task_id: Option<&str>,
    author_session_id: Option<&str>,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO durable_writes (kind, name, path, scope_id, task_id, author_session_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        (
            kind.as_db_str(),
            name,
            path,
            scope_id,
            task_id,
            author_session_id,
        ),
    )?;
    Ok(())
}

/// Every durable write recorded against `task_id`, in append order —
/// [`DurableWrite::id`] ascending.
///
/// Read-only: `&Store`, through [`Store::connection`], never
/// [`Store::transaction`] — a read path must not take the write lock and
/// contend with real writers, the same reasoning
/// `factory_task::events::for_task` gives for itself.
pub fn list_for_task(store: &Store, task_id: &str) -> Result<Vec<DurableWrite>, StoreError> {
    let mut stmt = store.connection().prepare(
        "SELECT id, kind, name, path, scope_id, task_id, author_session_id, created_at \
         FROM durable_writes WHERE task_id = ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map([task_id], row_to_durable_write)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::from)
}

/// Every durable write recorded against `scope_id`, in append order. In
/// practice this only ever returns `memory` rows — a `knowledge` write's
/// `scope_id` is always NULL by the CHECK on `schema::V7_SCHEMA` — but it
/// does not assume that here; it simply asks the index the same question
/// `memory list` needs answered.
///
/// Read-only, for the same reason [`list_for_task`] is.
pub fn list_for_scope(store: &Store, scope_id: &str) -> Result<Vec<DurableWrite>, StoreError> {
    let mut stmt = store.connection().prepare(
        "SELECT id, kind, name, path, scope_id, task_id, author_session_id, created_at \
         FROM durable_writes WHERE scope_id = ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map([scope_id], row_to_durable_write)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::from)
}
