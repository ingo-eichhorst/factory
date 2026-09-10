//! Structured decisions (`task_decisions`; design §11).
//!
//! Backlog §11 asks for decisions "strukturiert abrufbar" — retrievable as
//! decisions, not merely present somewhere in the log —
//! `factory_store::schema`'s own comment on the table gives the same reason
//! for a separate table rather than a `task_events` payload: "a decision is
//! looked up as a decision." This module is that lookup, and the recording
//! that feeds it.
//!
//! Unlike [`crate::events`], a decision never becomes part of the
//! append-only audit trail on its own — nothing here calls
//! [`crate::events::append`]. `task_decisions` is a second table, not a
//! second name for the first.

use crate::TaskError;

/// One `task_decisions` row, as read back by [`for_task`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub id: uuid::Uuid,
    pub task_id: uuid::Uuid,
    /// `None` means a human, not a session — the same convention
    /// `task_events.author_session_id` uses.
    pub author_session_id: Option<uuid::Uuid>,
    pub decision: String,
    pub rationale: String,
    pub alternatives: Option<String>,
    pub consequences: Option<String>,
    pub created_at: String,
}

/// Everything that can go wrong recording a decision.
#[derive(Debug, thiserror::Error)]
pub enum DecisionError {
    #[error(transparent)]
    Task(#[from] TaskError),

    #[error(
        "task {0} decision has no rationale\n  help: `task_decisions.rationale` is NOT NULL because design §11 asks for \"nachvollziehbare Entscheidungen\" (traceable decisions) — a decision without a rationale is a log line, not a decision"
    )]
    NoRationale(uuid::Uuid),
}

/// Record one decision against `task_id`, in its own transaction.
///
/// `id` is supplied by the caller, not generated here — the same choice
/// `create::create` makes and for the identical reason (`uuid` is pinned
/// workspace-wide without the `v4` feature, so this crate has no
/// `Uuid::new_v4()` to call even if minting the id were this function's job).
///
/// Refuses an empty `rationale` before opening a transaction
/// ([`DecisionError::NoRationale`]) — not just `rationale.is_empty()` at the
/// SQL boundary, because `task_decisions.rationale TEXT NOT NULL` accepts an
/// empty string just as readily as a real one, the same three-valued-logic
/// gap `complete::complete_with_result` (private to that module) already
/// guards against for `result_summary`.
///
/// A bad `task_id` is refused by `task_decisions.task_id REFERENCES tasks
/// (id)`, enforced on every connection (ADR 0012 decision 3) — this function
/// does not duplicate that check in Rust, the same stance
/// `create::create_from_template` takes toward `task_templates.id`.
#[allow(clippy::too_many_arguments)]
pub fn record(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    task_id: uuid::Uuid,
    author_session_id: Option<uuid::Uuid>,
    decision: &str,
    rationale: &str,
    alternatives: Option<&str>,
    consequences: Option<&str>,
) -> Result<(), DecisionError> {
    if rationale.is_empty() {
        return Err(DecisionError::NoRationale(task_id));
    }

    let tx = store.transaction().map_err(TaskError::from)?;
    tx.execute(
        "INSERT INTO task_decisions \
         (id, task_id, author_session_id, decision, rationale, alternatives, consequences) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        (
            id.to_string(),
            task_id.to_string(),
            author_session_id.map(|s| s.to_string()),
            decision,
            rationale,
            alternatives,
            consequences,
        ),
    )
    .map_err(factory_store::StoreError::from)
    .map_err(TaskError::from)?;
    tx.commit()
        .map_err(factory_store::StoreError::from)
        .map_err(TaskError::from)?;
    Ok(())
}

fn row_to_decision(row: &rusqlite::Row<'_>) -> rusqlite::Result<Decision> {
    let id: String = row.get(0)?;
    let task_id: String = row.get(1)?;
    let author_session_id: Option<String> = row.get(2)?;
    let decision: String = row.get(3)?;
    let rationale: String = row.get(4)?;
    let alternatives: Option<String> = row.get(5)?;
    let consequences: Option<String> = row.get(6)?;
    let created_at: String = row.get(7)?;

    Ok(Decision {
        id: uuid::Uuid::parse_str(&id)
            .unwrap_or_else(|e| panic!("task_decisions.id is a UUID; read {id:?}: {e}")),
        task_id: uuid::Uuid::parse_str(&task_id)
            .unwrap_or_else(|e| panic!("task_decisions.task_id is a UUID; read {task_id:?}: {e}")),
        author_session_id: author_session_id.as_deref().map(|s| {
            uuid::Uuid::parse_str(s).unwrap_or_else(|e| {
                panic!("task_decisions.author_session_id is a UUID; read {s:?}: {e}")
            })
        }),
        decision,
        rationale,
        alternatives,
        consequences,
        created_at,
    })
}

/// Every decision recorded for `task_id`, oldest first (`id` is a `TEXT
/// PRIMARY KEY`, the caller's own uuid, so `created_at, id` is the ordering
/// — the same tie-break `create::list` uses for `tasks`, for the identical
/// reason: `created_at` has second granularity).
///
/// Read-only: `&factory_store::Store`, never a transaction — the same
/// reasoning `create::list` and `events::for_task` give for their own reads.
pub fn for_task(
    store: &factory_store::Store,
    task_id: uuid::Uuid,
) -> Result<Vec<Decision>, TaskError> {
    let mut stmt = store
        .connection()
        .prepare(
            "SELECT id, task_id, author_session_id, decision, rationale, alternatives, \
             consequences, created_at \
             FROM task_decisions WHERE task_id = ?1 ORDER BY created_at, id",
        )
        .map_err(factory_store::StoreError::from)?;
    let rows = stmt
        .query_map([task_id.to_string()], row_to_decision)
        .map_err(factory_store::StoreError::from)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| factory_store::StoreError::from(e).into())
}
