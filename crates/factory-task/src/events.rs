//! The append-only audit log (design §11's `task_events`; ADR 0021 decision
//! 3, and `crate`'s station-11 decision 3, which this module implements).
//!
//! "`task_events` wird nie geändert oder gelöscht. Korrigiert wird durch ein
//! neues Ereignis" (`factory_store::schema`'s own comment on the table). This
//! module is the one place that INSERTs into it. Every material transition
//! elsewhere in this crate — the set in the schema's own CHECK — calls
//! [`append`] rather than composing the row itself; see each of `create.rs`,
//! `assign.rs`, `deliver.rs`, `complete.rs` for the call sites and this
//! task's report for the exhaustive list.
//!
//! # What the signature guarantees, and what it does not
//!
//! [`append`] takes `&rusqlite::Transaction`, never `&mut factory_store::Store`
//! — it cannot open or commit a transaction of its own. The event it writes
//! only ever becomes durable through *some caller's* `tx.commit()`, never
//! through one of its own. That rules out the specific mistake of `append`
//! silently persisting an event on a path where the transition itself later
//! fails or rolls back.
//!
//! It does **not**, by itself, prove that every caller passes the *same*
//! transaction it used for the transition rather than a second, later one —
//! `deliver` genuinely opens two transactions in sequence (see its own doc
//! comment), and nothing stops a future wired function from doing the same
//! and calling `append` against the wrong one. Each call site's own comment
//! is what pins it to the right transaction; the tests that show a failing
//! transition leaves no event behind (`tests/complete.rs`'s repeated-`done`
//! case and its siblings in the other test files) are a behavioural
//! backstop for that, not a proof of it.
use crate::TaskError;

/// The twelve strings `task_events.event_type`'s CHECK allows
/// (`factory_store::schema`, `V6_SCHEMA`), typed so no caller threads a
/// hand-written string past this module's boundary — the same reasoning as
/// [`crate::TaskStatus`] and [`crate::BlockedReason`].
///
/// Not every variant has a caller in this station. `Progress` and `Rework`
/// are in the schema's CHECK for a later station (§12.2's rework, §12.4's
/// progress reporting) and nothing here writes them — ADR 0021 decision 5's
/// own warning against a rule with no caller applies just as much to an
/// event type with no writer, so neither is wired ahead of the work that
/// would need it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    Created,
    Assigned,
    Delivered,
    Refused,
    Running,
    Progress,
    Blocked,
    Done,
    Failed,
    Cancelled,
    Resumed,
    Verification,
    Rework,
}

impl EventType {
    pub(crate) fn as_db_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Assigned => "assigned",
            Self::Delivered => "delivered",
            Self::Refused => "refused",
            Self::Running => "running",
            Self::Progress => "progress",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Resumed => "resumed",
            Self::Verification => "verification",
            Self::Rework => "rework",
        }
    }

    /// `task_events.event_type` is constrained by CHECK to exactly these
    /// thirteen strings, so an unrecognised value read back from a row this
    /// crate itself just selected is a broken invariant, not an input to
    /// handle — the same stance [`crate::TaskStatus::from_db_str`] takes.
    pub(crate) fn from_db_str(s: &str) -> Self {
        match s {
            "created" => Self::Created,
            "assigned" => Self::Assigned,
            "delivered" => Self::Delivered,
            "refused" => Self::Refused,
            "running" => Self::Running,
            "progress" => Self::Progress,
            "blocked" => Self::Blocked,
            "done" => Self::Done,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "resumed" => Self::Resumed,
            "verification" => Self::Verification,
            "rework" => Self::Rework,
            other => unreachable!(
                "task_events.event_type is constrained by CHECK to the thirteen station-11 strings; read {other:?}"
            ),
        }
    }
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_db_str())
    }
}

/// One `task_events` row, as read back by [`for_task`].
///
/// `payload` is stored and returned as raw JSON text, never parsed back by
/// this crate — the same stance `create::Task::result_artifact_paths` takes
/// on its own JSON column; each writer below knows the shape of the fact it
/// wrote and each reader is expected to know what it is asking for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// The append order — `task_events.id`, an `INTEGER PRIMARY KEY`. This is
    /// the ordering [`for_task`] returns rows in, not `created_at`: two
    /// events in the same second are ordinary (`factory_store::schema`'s own
    /// comment on the table).
    pub id: i64,
    pub task_id: uuid::Uuid,
    pub event_type: EventType,
    /// `None` means a human or the daemon itself, not a session
    /// (`factory_store::schema`'s comment on the column).
    pub author_session_id: Option<uuid::Uuid>,
    pub payload: Option<String>,
    pub created_at: String,
}

/// Append one event row inside `tx` — the caller's own already-open
/// transaction, never one this function opens. See the module docs' "why
/// atomicity is a type, not a test" for why the signature is shaped this way
/// rather than taking `&mut factory_store::Store`.
///
/// `payload` is never validated here — decision 6 ("no secret and no copied
/// private source content, ever") is a rule for what a *caller* puts in a
/// payload, not something this generic append function could check; each
/// call site is where that judgment is made and documented.
///
/// Returns `factory_store::StoreError` rather than [`TaskError`] — the
/// crate's five call sites (`create`, `assign`, `deliver`, `complete`,
/// `verify`) each have their own error type, and every one of them already
/// knows how to turn a bare `StoreError` into its own type the same way it
/// does for every other `tx.execute(...)` call in this crate: either
/// directly (`AssignError::Store`, `DeliverError::Store`) or through
/// `TaskError::Store` (`CompleteError::Task`, `DecisionError::Task`,
/// `VerifyError::Task`, and `create`'s own functions, which return
/// `TaskError` directly). Returning `TaskError` here instead would force a
/// second, redundant unwrap-and-rewrap at every one of those call sites.
pub(crate) fn append(
    tx: &rusqlite::Transaction<'_>,
    task_id: uuid::Uuid,
    event_type: EventType,
    author_session_id: Option<uuid::Uuid>,
    payload: Option<&str>,
) -> Result<(), factory_store::StoreError> {
    tx.execute(
        "INSERT INTO task_events (task_id, event_type, author_session_id, payload) \
         VALUES (?1, ?2, ?3, ?4)",
        (
            task_id.to_string(),
            event_type.as_db_str(),
            author_session_id.map(|s| s.to_string()),
            payload,
        ),
    )
    .map_err(factory_store::StoreError::from)?;
    Ok(())
}

fn row_to_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<Event> {
    let id: i64 = row.get(0)?;
    let task_id: String = row.get(1)?;
    let event_type: String = row.get(2)?;
    let author_session_id: Option<String> = row.get(3)?;
    let payload: Option<String> = row.get(4)?;
    let created_at: String = row.get(5)?;

    Ok(Event {
        id,
        task_id: uuid::Uuid::parse_str(&task_id)
            .unwrap_or_else(|e| panic!("task_events.task_id is a UUID; read {task_id:?}: {e}")),
        event_type: EventType::from_db_str(&event_type),
        author_session_id: author_session_id.as_deref().map(|s| {
            uuid::Uuid::parse_str(s).unwrap_or_else(|e| {
                panic!("task_events.author_session_id is a UUID; read {s:?}: {e}")
            })
        }),
        payload,
        created_at,
    })
}

/// Every event recorded for `task_id`, in append order — [`Event::id`]
/// ascending, per the module docs.
///
/// Read-only: `&factory_store::Store`, through
/// [`factory_store::Store::connection`], never [`factory_store::Store::transaction`]
/// — a read path must not take the write lock and contend with real writers,
/// the same reasoning `create::list` and `create::delegation_chain_of` give
/// for their own reads.
pub fn for_task(
    store: &factory_store::Store,
    task_id: uuid::Uuid,
) -> Result<Vec<Event>, TaskError> {
    let mut stmt = store
        .connection()
        .prepare(
            "SELECT id, task_id, event_type, author_session_id, payload, created_at \
             FROM task_events WHERE task_id = ?1 ORDER BY id",
        )
        .map_err(factory_store::StoreError::from)?;
    let rows = stmt
        .query_map([task_id.to_string()], row_to_event)
        .map_err(factory_store::StoreError::from)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| factory_store::StoreError::from(e).into())
}
