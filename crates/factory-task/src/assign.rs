//! Task-to-session assignment (design §2.4, §5; backlog §7).
//!
//! [`assign`] chooses which session a `queued` task goes to and records that
//! choice. It never starts a session, never touches a harness, and never
//! writes to a terminal — there is no PTY automation anywhere in this file
//! (backlog §7's scope line: "retain a manual operator handoff option until
//! PTY input is automated"). It also never sets a task's `status` to
//! `running`: that is delivery's and observation's job (station 7C), and this
//! function only ever records the choice in `tasks.assigned_session_id`,
//! leaving `status` at `queued`.
//!
//! A `starting` row with no process behind it would hold a workspace lease
//! forever with nobody to launch it (`factory_session::begin_start` is the
//! only thing that ever creates a `starting` row, and it requires a caller
//! that is actually about to launch a harness). So the third outcome below,
//! [`Assignment::StartSessionAt`], is only ever a *report*: "no idle session
//! exists, but you may start one here, within `max_sessions`." Nothing is
//! written to the database for that outcome — see [`assign`]'s own doc
//! comment.
//!
//! # "Idle" is one named predicate, with one definition
//!
//! [`is_idle`] is that predicate, and every branch below calls it rather than
//! re-deriving any part of it. A session is idle when **both** hold:
//!
//! - its `sessions.state` is exactly `running`. This is deliberately
//!   narrower than [`factory_session::SessionState::holds_lease`].
//!   `starting` holds a lease but has not been confirmed ready — nothing has
//!   observed that a harness is actually there to receive work.
//!   `disconnected` holds a lease *precisely because* Factory cannot see the
//!   process (ADR 0012 decision 5) — handing it a task would queue work
//!   behind a process Factory cannot currently confirm is even running.
//!   Neither state can be given work, so `is_idle` must reject both, not just
//!   accept the states that plainly cannot work (`stopped`, `failed`).
//!   Reaching for `holds_lease()` here is the obvious mistake — it is a
//!   *lease* predicate ("is this workspace occupied"), not a *readiness*
//!   predicate ("can this session accept a task right now"), and the two
//!   happen to agree on four of five states but disagree on the one that
//!   matters most (`starting`). [`tests::a_starting_session_holds_the_lease_but_is_not_idle`]
//!   in `tests/assign.rs` is the test that must fail if this predicate is
//!   ever widened to `holds_lease()` — see this task's mutation report.
//! - it has no task assigned to it (`tasks.assigned_session_id`) whose
//!   status is non-terminal, per [`TaskStatus::is_terminal`] — not a
//!   re-listing of the three terminal strings. `blocked` is deliberately
//!   **not** terminal (see `TaskStatus::is_terminal`'s own doc comment), so a
//!   session whose task is `blocked: clarification` is **not** idle: it is
//!   waiting on a human, and handing it a second task would put two pieces of
//!   work in one terminal, exactly what backlog §7's "each session has at
//!   most one running task" forbids.
//!
//! # Why this cannot simply be `tasks.status = 'running'`
//!
//! The database's own `tasks_one_running_per_session` unique index (see
//! `factory_store::schema` migration 3) only covers rows where
//! `status = 'running'`. A task this module has already assigned but that
//! has not yet been delivered stays `queued` with `assigned_session_id` set
//! — the index does not see it, and nothing in the schema stops a second
//! `assign` call from choosing the same session for a second task while the
//! first is still sitting in that gap. The in-transaction idle check below is
//! the *only* thing preventing that double-assignment; see
//! [`tests::a_second_untargeted_assign_does_not_double_book_the_only_idle_session`].
//!
//! # The transaction boundary
//!
//! [`assign`] opens exactly one `BEGIN IMMEDIATE` transaction
//! (`factory_store::Store::transaction`), reads the task row, runs the
//! matching branch's idle check(s) against that same transaction, and (for
//! the `Assigned` outcome) writes `assigned_session_id` before committing.
//! Nothing here checks idleness on one connection state and writes on
//! another: a check taken before the transaction opens is a race, because a
//! second, concurrent `assign` could observe the same "idle" snapshot before
//! either writer commits. `BEGIN IMMEDIATE` takes SQLite's write lock at the
//! start of the transaction (ADR 0012 decision 3, and the same argument
//! `factory_session::begin_start`'s own module docs make for its
//! scan-then-insert), so no concurrent mutator can be inside a conflicting
//! write transaction at the same time at all.
//!
//! # The lease-holding set has one home, and this module reaches it
//!
//! `max_sessions` is spent by [`factory_session::count_live_sessions`], and a
//! workspace's current occupant is found by filtering
//! [`factory_session::SessionState::holds_lease`] in Rust rather than with a
//! second `state IN (...)` in SQL. Both were briefly duplicated here, because
//! that function and `SessionState::from_db_str` were private to
//! `factory-session` and no file this slice owned could widen them. The
//! acceptance review published both instead, which is the right shape: a rule
//! with one home is only single-homed if every crate that needs it can reach
//! that home. Three copies of "which states hold a lease" is exactly the drift
//! `holds_lease`'s own doc comment exists to prevent.

use std::path::PathBuf;

use rusqlite::OptionalExtension;

use crate::events::EventType;
use crate::{TaskError, TaskStatus};

/// What [`assign`] decided, or reports needs to happen before it can decide.
///
/// A caller that gets [`Assignment::StartSessionAt`] owns starting the
/// session (via `factory_session::begin_start` and, separately, launching and
/// confirming a harness) and then calling `assign` again; nothing here does
/// either of those things — see the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Assignment {
    /// An idle session was found and now names this task in
    /// `assigned_session_id`. `status` is left at `queued`.
    Assigned(uuid::Uuid),
    /// No idle session exists for a requested workspace, but the agent is
    /// below `max_sessions`. Nothing was written; the caller may start a
    /// session at this (already-resolved, already-existing) path.
    StartSessionAt(PathBuf),
    /// Nothing to do right now. The task stays `queued`, untouched.
    Deferred(DeferReason),
}

/// Why [`assign`] could not assign the task right now. Busy is a normal
/// state, not a failure — every variant here is a routine outcome that
/// leaves the task `queued` for a later `assign` call to revisit, not a bug
/// report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeferReason {
    /// Untargeted task: no idle session of `agent_name` exists in the target
    /// scope right now.
    NoIdleSession { agent_name: String },
    /// A `target_session_id` was requested and no such session exists.
    /// design §2.4 / backlog §7: "[n]ever silently substitute another" — this
    /// is reported rather than falling back to the untargeted search.
    ///
    /// Kept for defence in depth ([`assign_requested_session`] re-reads the
    /// row rather than assuming it is there, the same stance
    /// `factory_session::on_task_terminal` takes toward `tasks.status`), but
    /// unreachable through this crate's own writing surface: `tasks
    /// .target_session_id TEXT REFERENCES sessions (id)` is enforced on
    /// every connection (`PRAGMA foreign_keys = ON`, ADR 0012 decision 3),
    /// so `create::create` itself rejects a `target_session_id` that names
    /// no real row before `assign` ever runs — proved directly in
    /// `tests/assign.rs::create_rejects_a_target_session_id_that_names_no_real_session`.
    /// Session rows are also never deleted once created (see
    /// `factory_session`'s module docs on why "the session record is
    /// removed" means `Failed`, never `DELETE FROM sessions`), so a
    /// previously valid `target_session_id` cannot go missing later either.
    RequestedSessionNotFound { session_id: uuid::Uuid },
    /// A specific session — either the one directly requested via
    /// `target_session_id`, or the one already occupying a requested
    /// `target_workspace_path` — exists but is not idle right now.
    /// `state` is the session's raw `sessions.state` value, which is enough
    /// for a caller to tell "busy" (`running`, working a non-terminal task)
    /// apart from "gone" (`starting`, never confirmed ready;
    /// `disconnected`, Factory cannot see it; `stopped`/`failed`, torn down)
    /// without this module inventing a second, coarser label for the same
    /// fact `sessions.state` already records.
    SessionNotIdle {
        session_id: uuid::Uuid,
        state: String,
    },
    /// A `target_workspace_path` was requested and a *different* scope's
    /// session holds it.
    ///
    /// Found during acceptance review by comparing this module's query
    /// against the constraint it claims to mirror.
    /// `sessions_one_live_lease_per_workspace` indexes `workspace_path`
    /// **alone** — a workspace is leased globally, not per scope — while the
    /// lookup here also filtered by `scope_id`, so a workspace held by
    /// another scope read as unoccupied and `assign` answered
    /// [`Assignment::StartSessionAt`]. The database would then have refused
    /// the start, so nothing could have corrupted a directory; the defect
    /// was that `assign` told a caller to do something the schema forbids.
    /// Reusing the holder is not an option either: it runs another scope's
    /// agent.
    WorkspaceLeasedByAnotherScope {
        session_id: uuid::Uuid,
        scope_id: uuid::Uuid,
    },
    /// A `target_workspace_path` was requested, and no directory exists
    /// there. Design §2.4: "the workspace itself must already exist."
    WorkspaceNotFound { path: PathBuf },
    /// A `target_workspace_path` was requested, no session currently
    /// occupies it, and `agent_name` already has `max_sessions` (or more)
    /// live sessions — starting a new one here would exceed the bound.
    AgentAtCapacity {
        agent_name: String,
        max_sessions: u32,
    },
}

/// Everything that can go wrong assigning a task to a session.
///
/// Deliberately its own type rather than an extension of
/// [`crate::TaskError`]: `TaskError` is declared in `lib.rs`, which this
/// task's brief places off limits ("If you need something exported from
/// `lib.rs` that is not public yet, stop and report it rather than editing
/// that file"), and none of its existing variants say "this task is not
/// `queued`" for a non-terminal, non-queued status (`running` or `blocked`) —
/// `TaskError::AlreadyTerminal` only ever fires for the three terminal
/// statuses. [`AssignError::Store`] and [`AssignError::TaskNotFound`] still
/// mirror `TaskError`'s own shape for the errors that are genuinely the same
/// kind of fact, for the same diagnostic consistency `TaskError` itself
/// keeps with `factory_session::SessionError`.
#[derive(Debug, thiserror::Error)]
pub enum AssignError {
    #[error("store error: {0}")]
    Store(#[from] factory_store::StoreError),

    #[error("path error: {0}")]
    Path(#[from] factory_paths::PathError),

    #[error("session error: {0}")]
    Session(#[from] factory_session::SessionError),

    #[error("no task with id {0}")]
    TaskNotFound(uuid::Uuid),

    #[error(
        "task {id} has status `{status}`, not `queued`\n  help: assign only ever runs against a queued task; delivery and observation own every later status transition"
    )]
    NotQueued { id: uuid::Uuid, status: TaskStatus },
}

/// The single home of "idle" (backlog §7) — see the module docs for the full
/// reasoning. Every branch of [`assign`] calls this rather than re-deriving
/// any part of it.
fn is_idle(tx: &rusqlite::Transaction<'_>, session_id: uuid::Uuid) -> Result<bool, AssignError> {
    let state: Option<String> = tx
        .query_row(
            "SELECT state FROM sessions WHERE id = ?1",
            [session_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(factory_store::StoreError::from)?;
    let Some(state) = state else {
        return Ok(false);
    };

    // Deliberately `state == Running`, not `factory_session::SessionState::from_db_str(&state).holds_lease()` (which
    // would also accept `starting` and `disconnected`) — see the module
    // docs' "Idle is one named predicate" section for why that narrower
    // comparison is the whole point. Compared through
    // `SessionState::Running`'s own `Display` impl, not the literal
    // `"running"`, so this stays anchored to the enum's public vocabulary
    // rather than a second hand-typed string.
    if state != factory_session::SessionState::Running.to_string() {
        return Ok(false);
    }

    let mut stmt = tx
        .prepare("SELECT status FROM tasks WHERE assigned_session_id = ?1")
        .map_err(factory_store::StoreError::from)?;
    let mut rows = stmt
        .query([session_id.to_string()])
        .map_err(factory_store::StoreError::from)?;
    while let Some(row) = rows.next().map_err(factory_store::StoreError::from)? {
        let status: String = row.get(0).map_err(factory_store::StoreError::from)?;
        if !TaskStatus::from_db_str(&status).is_terminal() {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The lease-holding session currently at `canonical_workspace_path`, if any,
/// with its scope and raw state string.
///
/// **Deliberately not filtered by scope.**
/// `sessions_one_live_lease_per_workspace` indexes `workspace_path` alone, so
/// a workspace is leased globally; asking "who holds this path *in my
/// scope*" answers a narrower question than the constraint enforces and
/// reports a path held by another scope as free. The caller decides what a
/// foreign holder means — see [`DeferReason::WorkspaceLeasedByAnotherScope`].
///
/// A `stopped` or `failed` row at this path is history, not an occupant, so
/// only lease-holding states count; nothing prevents starting a fresh session
/// where a torn-down one used to live.
fn find_lease_holder_at_workspace(
    tx: &rusqlite::Transaction<'_>,
    canonical_workspace_path: &str,
) -> Result<Option<(uuid::Uuid, uuid::Uuid, String)>, AssignError> {
    let mut stmt = tx
        .prepare(
            // No `state IN (...)` here. The lease-holding set has exactly
            // one home, `SessionState::holds_lease`, and a second copy in
            // SQL is the drift `count_live_sessions` documents itself as
            // avoiding. There are only ever a handful of sessions per
            // workspace, so filtering in Rust costs nothing.
            "SELECT id, scope_id, state FROM sessions \
             WHERE workspace_path = ?1 \
             ORDER BY rowid",
        )
        .map_err(factory_store::StoreError::from)?;
    let mut rows = stmt
        .query([canonical_workspace_path])
        .map_err(factory_store::StoreError::from)?;

    let parse = |column: &str, raw: &str| {
        uuid::Uuid::parse_str(raw)
            .unwrap_or_else(|e| panic!("sessions.{column} is a UUID; read {raw:?}: {e}"))
    };
    while let Some(row) = rows.next().map_err(factory_store::StoreError::from)? {
        let id: String = row.get(0).map_err(factory_store::StoreError::from)?;
        let holder_scope: String = row.get(1).map_err(factory_store::StoreError::from)?;
        let state: String = row.get(2).map_err(factory_store::StoreError::from)?;
        if !factory_session::SessionState::from_db_str(&state).holds_lease() {
            continue;
        }
        return Ok(Some((
            parse("id", &id),
            parse("scope_id", &holder_scope),
            state,
        )));
    }
    Ok(None)
}

/// The first idle session of `agent_name` in `scope_id`, in `rowid` order
/// (insertion order) — the same determinism choice
/// `factory_session::find_aliasing_conflict` documents for its own scan:
/// `ORDER BY rowid` makes the scan's row order an explicit, tested guarantee
/// rather than an unspecified property of `SELECT * FROM sessions` this
/// function happened to rely on. The lowest-`rowid` (earliest-registered)
/// idle session wins — simple, and it means an agent's sessions are worked
/// through in the order they were created rather than an order that could
/// silently change between two otherwise-identical calls.
fn find_idle_session(
    tx: &rusqlite::Transaction<'_>,
    scope_id: uuid::Uuid,
    agent_name: &str,
) -> Result<Option<uuid::Uuid>, AssignError> {
    let mut stmt = tx
        .prepare("SELECT id FROM sessions WHERE scope_id = ?1 AND agent_name = ?2 ORDER BY rowid")
        .map_err(factory_store::StoreError::from)?;
    let mut rows = stmt
        .query((scope_id.to_string(), agent_name))
        .map_err(factory_store::StoreError::from)?;

    while let Some(row) = rows.next().map_err(factory_store::StoreError::from)? {
        let id: String = row.get(0).map_err(factory_store::StoreError::from)?;
        let id = uuid::Uuid::parse_str(&id)
            .unwrap_or_else(|e| panic!("sessions.id is a UUID; read {id:?}: {e}"));
        if is_idle(tx, id)? {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

/// Untargeted case (design §2.4: "[a] task sent only to an agent is assigned
/// to any idle session"): choose any idle session of `agent_name` in
/// `scope_id`. If none is idle, the task stays `queued` — [`Assignment::Deferred`],
/// not an error; busy is a normal state.
fn assign_untargeted(
    tx: &rusqlite::Transaction<'_>,
    scope_id: uuid::Uuid,
    agent_name: &str,
) -> Result<Assignment, AssignError> {
    match find_idle_session(tx, scope_id, agent_name)? {
        Some(session_id) => Ok(Assignment::Assigned(session_id)),
        None => Ok(Assignment::Deferred(DeferReason::NoIdleSession {
            agent_name: agent_name.to_string(),
        })),
    }
}

/// Requested-session case (design §2.4: "[a] task may instead request a
/// specific session"): use exactly `session_id` if it is idle. Never falls
/// back to the untargeted search — the sender asked for this one.
fn assign_requested_session(
    tx: &rusqlite::Transaction<'_>,
    session_id: uuid::Uuid,
) -> Result<Assignment, AssignError> {
    let state: Option<String> = tx
        .query_row(
            "SELECT state FROM sessions WHERE id = ?1",
            [session_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(factory_store::StoreError::from)?;
    let Some(state) = state else {
        return Ok(Assignment::Deferred(
            DeferReason::RequestedSessionNotFound { session_id },
        ));
    };

    if is_idle(tx, session_id)? {
        Ok(Assignment::Assigned(session_id))
    } else {
        Ok(Assignment::Deferred(DeferReason::SessionNotIdle {
            session_id,
            state,
        }))
    }
}

/// Requested-workspace case (design §2.4: "For a requested workspace,
/// Factory reuses its session or starts one if the agent is below
/// `max_sessions`; the workspace itself must already exist").
fn assign_requested_workspace(
    tx: &rusqlite::Transaction<'_>,
    scope_id: uuid::Uuid,
    workspace_path: &str,
    agent_name: &str,
    max_sessions: u32,
) -> Result<Assignment, AssignError> {
    let canonical = match factory_paths::CanonicalPath::resolve(workspace_path) {
        Ok(canonical) => canonical,
        Err(factory_paths::PathError::NotFound { path, .. }) => {
            return Ok(Assignment::Deferred(DeferReason::WorkspaceNotFound {
                path,
            }));
        }
        Err(other) => return Err(AssignError::Path(other)),
    };
    let canonical_str = canonical.as_path().to_string_lossy().into_owned();

    if let Some((session_id, holder_scope_id, state)) =
        find_lease_holder_at_workspace(tx, &canonical_str)?
    {
        if holder_scope_id != scope_id {
            return Ok(Assignment::Deferred(
                DeferReason::WorkspaceLeasedByAnotherScope {
                    session_id,
                    scope_id: holder_scope_id,
                },
            ));
        }
        return if is_idle(tx, session_id)? {
            Ok(Assignment::Assigned(session_id))
        } else {
            Ok(Assignment::Deferred(DeferReason::SessionNotIdle {
                session_id,
                state,
            }))
        };
    }

    let live = factory_session::count_live_sessions(tx, scope_id, agent_name)?;
    // `<`, not `<=`: `live` counts sessions that already exist; a new one is
    // allowed only while `live` has not yet reached `max_sessions` — mirrors
    // `factory_session::begin_start`'s own `live >= max_sessions` rejection
    // (the same comparison, the opposite way round because this function
    // reports "may start" rather than performing the start).
    if live < max_sessions {
        Ok(Assignment::StartSessionAt(canonical.into_path_buf()))
    } else {
        Ok(Assignment::Deferred(DeferReason::AgentAtCapacity {
            agent_name: agent_name.to_string(),
            max_sessions,
        }))
    }
}

/// Choose a session for `task_id` and record the choice.
///
/// `agent_name` and `max_sessions` are the target scope's agent — the caller
/// already resolved which one, the same way a caller of
/// `factory_session::begin_start` already knows `agent_name` and
/// `max_sessions` before calling it. This mirrors that function's own
/// signature shape rather than reading `.factory/config.yaml` here, and for
/// the same underlying reason: `tasks` carries a `target_scope_id`, but (per
/// `factory_store::schema`, migrations 1 and 3, read in full before writing
/// this module) no agent-name column at all, so nothing durable exists here
/// for this function to resolve an agent from on its own for a scope with
/// more than one configured agent — see this task's report for this gap
/// flagged as a schema/design observation, not solved here.
///
/// Refuses a task that is not `queued`
/// ([`AssignError::NotQueued`]) and never sets `status` to `running` — this
/// function only ever writes `assigned_session_id`, leaving `status` at
/// `queued` for delivery to advance later.
///
/// # Precedence when a task carries both `target_session_id` and
/// `target_workspace_path`
///
/// Nothing in `factory_store::schema` forbids a row from carrying both (there
/// is no mutual-exclusion `CHECK`, and `create::create` accepts both
/// independently). `target_session_id` wins when both are present: it is the
/// more specific ask ("use exactly this one"), and design §2.4 states no
/// ordering between the two, so this function commits to the narrower request
/// rather than the broader one.
///
/// # Errors
///
/// [`AssignError::TaskNotFound`] if `task_id` does not exist.
/// [`AssignError::NotQueued`] if it exists but its `status` is not `queued`
/// (this is where an already-`running`, `blocked`, or terminal task is
/// refused — assign is not a re-assignment path).
pub fn assign(
    store: &mut factory_store::Store,
    task_id: uuid::Uuid,
    agent_name: &str,
    max_sessions: u32,
) -> Result<Assignment, AssignError> {
    // BEGIN IMMEDIATE takes the write lock here, before the idle check(s)
    // below read a single row — see the module docs' transaction-boundary
    // section.
    let tx = store.transaction().map_err(AssignError::Store)?;

    let row: Option<(String, Option<String>, Option<String>, String)> = tx
        .query_row(
            "SELECT target_scope_id, target_session_id, target_workspace_path, status \
             FROM tasks WHERE id = ?1",
            [task_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(factory_store::StoreError::from)?;
    let Some((target_scope_id, target_session_id, target_workspace_path, status)) = row else {
        return Err(AssignError::TaskNotFound(task_id));
    };

    let status = TaskStatus::from_db_str(&status);
    if status != TaskStatus::Queued {
        return Err(AssignError::NotQueued {
            id: task_id,
            status,
        });
    }

    let scope_id = uuid::Uuid::parse_str(&target_scope_id).unwrap_or_else(|e| {
        panic!("tasks.target_scope_id is a UUID; read {target_scope_id:?}: {e}")
    });

    let assignment = if let Some(session_id) = target_session_id {
        let session_id = uuid::Uuid::parse_str(&session_id).unwrap_or_else(|e| {
            panic!("tasks.target_session_id is a UUID; read {session_id:?}: {e}")
        });
        assign_requested_session(&tx, session_id)?
    } else if let Some(workspace_path) = target_workspace_path {
        assign_requested_workspace(&tx, scope_id, &workspace_path, agent_name, max_sessions)?
    } else {
        assign_untargeted(&tx, scope_id, agent_name)?
    };

    if let Assignment::Assigned(session_id) = assignment {
        tx.execute(
            "UPDATE tasks SET assigned_session_id = ?2, updated_at = CURRENT_TIMESTAMP \
             WHERE id = ?1",
            (task_id.to_string(), session_id.to_string()),
        )
        .map_err(factory_store::StoreError::from)?;

        // `crate`'s station-11 decision 3: the assignment event's one home is
        // here, in the same transaction as the write it records. The payload
        // names the session chosen — `tasks.assigned_session_id` holds only
        // the *current* choice and `authorise_resume` can clear it later, so
        // this is the audit log's only durable record of which session an
        // earlier assignment actually named. No author session: `assign`
        // never runs as a session, only as Factory's own coordination.
        let payload = serde_json::json!({ "session_id": session_id.to_string() }).to_string();
        crate::events::append(&tx, task_id, EventType::Assigned, None, Some(&payload))?;
    }

    tx.commit().map_err(factory_store::StoreError::from)?;
    Ok(assignment)
}

/// The task `session_id` is currently running, if any — backlog §8's
/// concurrency demonstrations read this to tell two sessions' work apart
/// without relying on terminal scrollback (AGENTS.md: "users must be able
/// to see the exact scope, task... state that caused an action").
///
/// `status = 'running'` is the exact predicate `tasks_one_running_per_session`
/// enforces (`factory_store::schema`), so at most one row can ever match; a
/// task that is merely `assigned_session_id`-set but still `queued` (the gap
/// between `assign` and `deliver`/`mark_running` this module's own docs
/// describe) does not count, and neither does a task this session ran to
/// completion. Bound through [`TaskStatus::Running`]'s own
/// [`TaskStatus::as_db_str`] rather than the literal `"running"`, for the
/// same reason [`is_idle`] compares session state through
/// `SessionState::Running`'s `Display` impl instead of a hand-typed string.
///
/// Read-only: `&Store`, through [`factory_store::Store::connection`] — same
/// reasoning as [`crate::create::delegation_chain_of`] and every other read
/// path in this crate.
pub fn running_task_of_session(
    store: &factory_store::Store,
    session_id: uuid::Uuid,
) -> Result<Option<uuid::Uuid>, TaskError> {
    let id: Option<String> = store
        .connection()
        .query_row(
            "SELECT id FROM tasks WHERE assigned_session_id = ?1 AND status = ?2",
            (session_id.to_string(), TaskStatus::Running.as_db_str()),
            |row| row.get(0),
        )
        .optional()
        .map_err(factory_store::StoreError::from)?;
    Ok(id.map(|id| {
        uuid::Uuid::parse_str(&id)
            .unwrap_or_else(|e| panic!("tasks.id is a UUID; read {id:?}: {e}"))
    }))
}
