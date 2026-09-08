//! Task creation, read-only inspection, and cooperative cancellation
//! (design §2.4, §5; backlog §7).
//!
//! Nothing here talks to a terminal, a harness, or an adapter. Assignment
//! (choosing a session) and delivery (writing to a PTY) are the other two
//! modules this slice defines — `assign` and `deliver` — which stay
//! placeholders in this task; this module only ever writes `queued`, reads
//! rows back, or moves a non-terminal task straight to `cancelled` /
//! records a cancellation request. There is no PTY automation anywhere in
//! this file.

use rusqlite::OptionalExtension;

use crate::{BlockedReason, TaskError, TaskStatus, is_valid_transition, valid_targets};

/// One `tasks` row, as read back by [`list`] and [`show`].
///
/// A plain data record, not a handle: nothing here talks back to the
/// database. Every `TEXT`-typed identifier column is parsed into its typed
/// form ([`uuid::Uuid`] for ids, [`TaskStatus`] / [`BlockedReason`] for the
/// vocabulary columns) since this crate itself wrote them and a value that
/// does not parse is a broken invariant, not an input to handle — the same
/// stance [`TaskStatus::from_db_str`] takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: uuid::Uuid,
    /// `None` means the sender was a human, not another scope.
    pub sender_scope_id: Option<uuid::Uuid>,
    pub target_scope_id: uuid::Uuid,
    /// The session the sender *requested*, if any. See [`Task::assigned_session_id`]
    /// for the column this is deliberately distinct from.
    pub target_session_id: Option<uuid::Uuid>,
    pub target_workspace_path: Option<String>,
    /// The session Factory *chose* — set once the task is assigned, distinct
    /// from [`Task::target_session_id`] (design §2.4 / `factory_store::schema`
    /// migration 3's doc comment: "the session Factory *chose*" vs. "what the
    /// sender *requested*"). Non-NULL whenever `status` is
    /// [`TaskStatus::Running`] (enforced by the database, not this struct).
    pub assigned_session_id: Option<uuid::Uuid>,
    pub prompt: String,
    pub status: TaskStatus,
    pub blocked_reason: Option<BlockedReason>,
    /// Set once an operator asks a *running* task to stop; the task itself
    /// stays `running` until the agent reports a terminal status. See
    /// [`cancel`].
    pub cancel_requested_at: Option<String>,
    pub result_summary: Option<String>,
    /// A JSON array of artifact paths, stored verbatim (this crate does not
    /// parse it — see `factory_store::schema`'s comment on the column).
    pub result_artifact_paths: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

const TASK_COLUMNS: &str = "id, sender_scope_id, target_scope_id, target_session_id, \
     target_workspace_path, assigned_session_id, prompt, status, blocked_reason, \
     cancel_requested_at, result_summary, result_artifact_paths, created_at, updated_at";

/// `tasks.id`, `.sender_scope_id`, `.target_scope_id`, `.target_session_id`,
/// and `.assigned_session_id` are all UUIDs this crate — or `factory-session`
/// / a future registry — wrote; a value that fails to parse is a broken
/// invariant in a row this code just selected, so this panics rather than
/// threading a parse error through every caller of [`list`] and [`show`].
fn parse_uuid(column: &str, value: &str) -> uuid::Uuid {
    uuid::Uuid::parse_str(value)
        .unwrap_or_else(|e| panic!("tasks.{column} is a UUID; read {value:?}: {e}"))
}

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let id: String = row.get(0)?;
    let sender_scope_id: Option<String> = row.get(1)?;
    let target_scope_id: String = row.get(2)?;
    let target_session_id: Option<String> = row.get(3)?;
    let target_workspace_path: Option<String> = row.get(4)?;
    let assigned_session_id: Option<String> = row.get(5)?;
    let prompt: String = row.get(6)?;
    let status: String = row.get(7)?;
    let blocked_reason: Option<String> = row.get(8)?;
    let cancel_requested_at: Option<String> = row.get(9)?;
    let result_summary: Option<String> = row.get(10)?;
    let result_artifact_paths: Option<String> = row.get(11)?;
    let created_at: String = row.get(12)?;
    let updated_at: String = row.get(13)?;

    Ok(Task {
        id: parse_uuid("id", &id),
        sender_scope_id: sender_scope_id
            .as_deref()
            .map(|s| parse_uuid("sender_scope_id", s)),
        target_scope_id: parse_uuid("target_scope_id", &target_scope_id),
        target_session_id: target_session_id
            .as_deref()
            .map(|s| parse_uuid("target_session_id", s)),
        target_workspace_path,
        assigned_session_id: assigned_session_id
            .as_deref()
            .map(|s| parse_uuid("assigned_session_id", s)),
        prompt,
        status: TaskStatus::from_db_str(&status),
        blocked_reason: blocked_reason.as_deref().map(BlockedReason::from_db_str),
        cancel_requested_at,
        result_summary,
        result_artifact_paths,
        created_at,
        updated_at,
    })
}

/// Commit `queued` in its own transaction and return the id it was written
/// under.
///
/// Design §5 step 1: the row is durable *before* anything is entered into a
/// terminal. This function is the entirety of that step — it does not choose
/// a session and does not touch a harness or an adapter.
///
/// `delegation_chain` is written in the *same* transaction as the task row.
/// A task whose chain were committed separately could be read back, after a
/// crash between the two commits, as a task that had travelled through no
/// scope at all — and backlog §8 asks for exactly the opposite: "the chain is
/// recorded durably with the task, so a delegation loop is reconstructable
/// after a restart rather than only detectable while running." This function
/// stores the chain it is given and judges none of it; deciding *what* the
/// chain is, and whether the target may be appended to it at all, belongs to
/// `factory_delegation`, which calls this.
///
/// `id` is supplied by the caller rather than generated here, mirroring
/// `factory_session::begin_start`'s own `id: uuid::Uuid` parameter: the
/// `uuid` crate is pinned workspace-wide without the `v4` feature (and the
/// workspace manifest is centrally owned, so this crate cannot add it), so
/// there is no `Uuid::new_v4()` available to call inside this crate even if
/// it were this function's job to mint the id. The caller — the same seam
/// that already decides session ids — decides task ids too. Returning `id`
/// back is a convenience for a caller that wants to log or chain on it
/// immediately, not evidence that this function chose it.
#[allow(clippy::too_many_arguments)]
pub fn create(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    sender_scope_id: Option<uuid::Uuid>,
    target_scope_id: uuid::Uuid,
    target_session_id: Option<uuid::Uuid>,
    target_workspace_path: Option<&str>,
    prompt: &str,
    delegation_chain: &[uuid::Uuid],
) -> Result<uuid::Uuid, TaskError> {
    let tx = store.transaction()?;
    tx.execute(
        "INSERT INTO tasks \
         (id, sender_scope_id, target_scope_id, target_session_id, target_workspace_path, prompt, status) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'queued')",
        (
            id.to_string(),
            sender_scope_id.map(|s| s.to_string()),
            target_scope_id.to_string(),
            target_session_id.map(|s| s.to_string()),
            target_workspace_path,
            prompt,
        ),
    )
    .map_err(factory_store::StoreError::from)?;
    for (position, scope_id) in delegation_chain.iter().enumerate() {
        tx.execute(
            "INSERT INTO task_delegation_chain (task_id, position, scope_id) \
             VALUES (?1, ?2, ?3)",
            (id.to_string(), position as i64, scope_id.to_string()),
        )
        .map_err(factory_store::StoreError::from)?;
    }
    tx.commit().map_err(factory_store::StoreError::from)?;
    Ok(id)
}

/// The ordered delegation chain recorded with `task_id`, position 0 first —
/// design §2.4's "delegation chain (every scope the task has passed
/// through)" and backlog §8's "[e]very task carries the ordered delegation
/// chain of scopes it has passed through." 0-based, with the target itself
/// last, mirroring exactly what [`create`] was given and wrote — this
/// function judges none of it, the same stance [`create`]'s own doc comment
/// takes.
///
/// Read-only: takes `&Store`, going through [`factory_store::Store::connection`]
/// — never [`factory_store::Store::transaction`], per that method's own doc
/// comment on why a read path must not take the write lock and contend with
/// real writers.
///
/// `ORDER BY position` is load-bearing, not decorative, and for a reason
/// stronger than "insertion order might not be preserved": this table also
/// carries a `UNIQUE (task_id, scope_id)` index, and — confirmed with
/// `EXPLAIN QUERY PLAN` against a throwaway database before this function
/// was written — a query that only selects `scope_id` and has no `ORDER BY`
/// is satisfied by SQLite's query planner from *that* index as a covering
/// scan, which returns rows in `scope_id`'s lexicographic order, not
/// `position` order and not insertion order. Dropping this clause is
/// therefore invisible against a chain whose scopes happen to sort the same
/// way they were positioned, which is why
/// [`tests::delegation_chain_of_round_trips_a_three_scope_chain_in_position_order`]
/// in `tests/create.rs` deliberately chooses scope ids whose lexicographic
/// order differs from their position order.
pub fn delegation_chain_of(
    store: &factory_store::Store,
    task_id: uuid::Uuid,
) -> Result<Vec<uuid::Uuid>, TaskError> {
    let mut stmt = store
        .connection()
        .prepare("SELECT scope_id FROM task_delegation_chain WHERE task_id = ?1 ORDER BY position")
        .map_err(factory_store::StoreError::from)?;
    let scope_ids: Vec<String> = stmt
        .query_map([task_id.to_string()], |row| row.get(0))
        .map_err(factory_store::StoreError::from)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(factory_store::StoreError::from)?;
    Ok(scope_ids
        .into_iter()
        .map(|scope_id| {
            uuid::Uuid::parse_str(&scope_id).unwrap_or_else(|e| {
                panic!("task_delegation_chain.scope_id is a UUID; read {scope_id:?}: {e}")
            })
        })
        .collect())
}

/// Every task, oldest first — read-only inspection before automation
/// (AGENTS.md: "Design read-only inspection... before automation").
///
/// Takes `&Store`, not `&mut Store`: per `Store::connection`'s own doc
/// comment, a read path must never go through `Store::transaction` (which
/// takes a write lock and would needlessly contend with real writers under
/// WAL).
pub fn list(store: &factory_store::Store) -> Result<Vec<Task>, TaskError> {
    let mut stmt = store
        .connection()
        .prepare(&format!(
            "SELECT {TASK_COLUMNS} FROM tasks ORDER BY created_at, id"
        ))
        .map_err(factory_store::StoreError::from)?;
    let rows = stmt
        .query_map([], row_to_task)
        .map_err(factory_store::StoreError::from)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| factory_store::StoreError::from(e).into())
}

/// One task by id — the other half of read-only inspection.
pub fn show(store: &factory_store::Store, id: uuid::Uuid) -> Result<Task, TaskError> {
    store
        .connection()
        .query_row(
            &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"),
            [id.to_string()],
            row_to_task,
        )
        .optional()
        .map_err(factory_store::StoreError::from)?
        .ok_or(TaskError::NotFound(id))
}

/// Describe `valid_targets(from)` the way `factory_session::transition_in_tx`
/// formats its own `SessionError::InvalidTransition::allowed` field, for the
/// identical `TaskError::InvalidTransition` message shape.
fn describe_targets(from: TaskStatus) -> String {
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

/// The whole cancellation rule (design §2.4), in one place:
///
/// - a `queued` or `blocked` task moves straight to `cancelled` — nothing is
///   running anywhere, so there is no one to cooperate with;
/// - a `running` task instead gets `cancel_requested_at` set and **stays**
///   `running`; this function does not touch the session, its state, or its
///   lease. `factory_session::interrupt` is the wrong neighbour to reach for
///   here despite sharing the word "interrupt": that function models a
///   session that has *died* — it fails the session and releases its
///   workspace lease. This models the opposite situation: the session is
///   presumed alive and working, and design §2.4's "cooperative" cancellation
///   means the running agent itself is expected to notice the request and
///   report a terminal status; forcing the session down is "a separate user
///   action" the design explicitly declines to fold into cancellation;
/// - a task already in a terminal status is refused with
///   [`TaskError::AlreadyTerminal`] — cancelling something that already
///   finished is a caller bug worth surfacing, not a silent no-op.
///
/// Returns the task's resulting status: [`TaskStatus::Cancelled`] for the
/// immediate path, or [`TaskStatus::Running`] (unchanged) for the cooperative
/// one, so a caller can tell the two outcomes apart without a second read.
///
/// Setting `cancel_requested_at` is idempotent — a second cancel request
/// against an already-requested, still-running task leaves the original
/// timestamp in place (`COALESCE`) rather than overwriting it, so the
/// recorded time is always the *first* request, matching AGENTS.md's
/// "[m]ake mutations transactional and idempotent where retries... are
/// possible."
pub fn cancel(store: &mut factory_store::Store, id: uuid::Uuid) -> Result<TaskStatus, TaskError> {
    let tx = store.transaction()?;

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
    let current = TaskStatus::from_db_str(&status);

    if current.is_terminal() {
        return Err(TaskError::AlreadyTerminal {
            id,
            status: current,
        });
    }

    let outcome = match current {
        TaskStatus::Queued | TaskStatus::Blocked => {
            if !is_valid_transition(current, TaskStatus::Cancelled) {
                return Err(TaskError::InvalidTransition {
                    id,
                    from: current,
                    to: TaskStatus::Cancelled,
                    allowed: describe_targets(current),
                });
            }
            tx.execute(
                "UPDATE tasks SET status = 'cancelled', blocked_reason = NULL, \
                 updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
                [id.to_string()],
            )
            .map_err(factory_store::StoreError::from)?;
            TaskStatus::Cancelled
        }
        TaskStatus::Running => {
            tx.execute(
                "UPDATE tasks SET \
                 cancel_requested_at = COALESCE(cancel_requested_at, CURRENT_TIMESTAMP), \
                 updated_at = CURRENT_TIMESTAMP \
                 WHERE id = ?1",
                [id.to_string()],
            )
            .map_err(factory_store::StoreError::from)?;
            TaskStatus::Running
        }
        TaskStatus::Done | TaskStatus::Failed | TaskStatus::Cancelled => {
            unreachable!("TaskStatus::is_terminal() already refused these above")
        }
    };

    tx.commit().map_err(factory_store::StoreError::from)?;
    Ok(outcome)
}
