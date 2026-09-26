//! The built-in task store. Whole rows are kept as JSON with the few fields the
//! daemon queries on lifted out into columns, so the domain model can move
//! without a migration every time.
//!
//! There is no migration machinery. The schema carries a version, and a
//! database written by a different one is dropped and rebuilt -- which is the
//! honest thing for a prototype, as long as it says so out loud.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use factory_core::adapter::{RuntimeStatus, TaskStore};
use factory_core::error::{FactoryError, Result};
use factory_core::agent::AgentSession;
use factory_core::occupancy::StatusChange;
use factory_core::run::{NewRun, Run, RunPatch, RunStatus};
use factory_core::task::{Task, TaskEntry, TaskFilter, TaskPatch};
use factory_core::usage::UsageSnapshot;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Bumped whenever the shape below changes. A database at any other version is
/// discarded.
/// The task-store schema this build can open without discarding its task
/// tables. Backup restore checks this before it commits a restored root: a
/// newer or older snapshot must never be installed only for startup to erase
/// the operating history it was meant to recover.
pub const SCHEMA_VERSION: i64 = 4;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS tasks (
    id          TEXT PRIMARY KEY,
    status      TEXT NOT NULL,
    scope       TEXT NOT NULL,
    next_run_at TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    data        TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS tasks_status ON tasks(status);
CREATE INDEX IF NOT EXISTS tasks_due ON tasks(next_run_at);

CREATE TABLE IF NOT EXISTS runs (
    id         TEXT PRIMARY KEY,
    task_id    TEXT NOT NULL,
    attempt    INTEGER NOT NULL,
    status     TEXT NOT NULL,
    started_at TEXT NOT NULL,
    ended_at   TEXT,
    data       TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS runs_task ON runs(task_id, attempt);
CREATE INDEX IF NOT EXISTS runs_open ON runs(ended_at);

CREATE TABLE IF NOT EXISTS task_entries (
    seq     INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    run_id  TEXT,
    at      TEXT NOT NULL,
    data    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS entries_task ON task_entries(task_id, seq);
CREATE INDEX IF NOT EXISTS entries_run ON task_entries(run_id, seq);
-- `entries_of_kinds` reads a window of the journal by time. An index is
-- not a change of shape a row could notice, and this whole script runs on
-- every open with IF NOT EXISTS, so a database already at this version
-- gains it on its next start -- no version bump, and so no drop.
CREATE INDEX IF NOT EXISTS entries_at ON task_entries(at);

CREATE TABLE IF NOT EXISTS agent_sessions (
    id    TEXT PRIMARY KEY,
    scope TEXT NOT NULL,
    name  TEXT NOT NULL,
    state TEXT NOT NULL,
    data  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS agents_scope ON agent_sessions(scope, name);

-- Liveness, append-only. Deliberately outside the version handshake below:
-- this table is a log of what was observed, not a projection of the domain,
-- so a schema change to tasks or runs has no business erasing it. Nothing
-- else will ever have this history -- herdr answers "now" and keeps no past.
CREATE TABLE IF NOT EXISTS agent_status (
    seq     INTEGER PRIMARY KEY AUTOINCREMENT,
    subject TEXT NOT NULL,
    scope   TEXT NOT NULL,
    agent   TEXT NOT NULL,
    status  TEXT NOT NULL,
    at      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS agent_status_at ON agent_status(at);
CREATE INDEX IF NOT EXISTS agent_status_subject ON agent_status(subject, seq);

-- Usage snapshots (#117), append-only: one row per reading of a run's
-- session -- at dispatch, at each turn end, as the run ends -- answered or
-- not. `Run::usage` is derived from these. A new table rather than a change
-- to an old one's shape, so it joins a database already at this version on
-- its next start without a version bump (the same reasoning as
-- `entries_at` above); it annotates runs, so a version mismatch that
-- rebuilds them drops it with them (`DROP_ALL`).
CREATE TABLE IF NOT EXISTS run_usage (
    seq     INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id  TEXT NOT NULL,
    task_id TEXT NOT NULL,
    point   TEXT NOT NULL,
    at      TEXT NOT NULL,
    data    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS run_usage_run ON run_usage(run_id, seq);
"#;

/// What a version mismatch throws away. `agent_status` is not in here on
/// purpose: it is an observation log, and the runs it annotates being rebuilt
/// is no reason to forget what the agents were doing.
const DROP_ALL: &str = r#"
DROP TABLE IF EXISTS run_usage;
DROP TABLE IF EXISTS agent_sessions;
DROP TABLE IF EXISTS task_entries;
DROP TABLE IF EXISTS runs;
DROP TABLE IF EXISTS tasks;
"#;

pub struct SqliteStore {
    conn: Arc<Mutex<Connection>>,
    name: String,
}

// The `adapter` field names the kind of adapter, not this instance: an error
// saying "sqlite" is what tells a reader which engine failed, whatever name a
// second sqlite database happens to be registered under.
fn adapter_err(e: impl std::fmt::Display) -> FactoryError {
    FactoryError::adapter("sqlite", e.to_string())
}

impl SqliteStore {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_named(path, "sqlite")
    }

    pub fn in_memory() -> Result<Self> {
        Self::in_memory_named("sqlite")
    }

    /// A second sqlite database, registered under a name of its own so a
    /// scope can be routed to it while another scope keeps the default one.
    pub fn open_named(path: &Path, name: &str) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(adapter_err)?;
        }
        let conn = Connection::open(path).map_err(adapter_err)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .map_err(adapter_err)?;
        Self::prepare(&conn, Some(path))?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            name: name.to_string(),
        })
    }

    pub fn in_memory_named(name: &str) -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(adapter_err)?;
        Self::prepare(&conn, None)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            name: name.to_string(),
        })
    }

    fn prepare(conn: &Connection, path: Option<&Path>) -> Result<()> {
        let found: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(adapter_err)?;

        let empty: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'tasks'",
                [],
                |r| r.get(0),
            )
            .map_err(adapter_err)?;

        if empty == 0 {
            // Nothing there yet; nothing to lose.
        } else if found != SCHEMA_VERSION {
            tracing::warn!(
                found,
                expected = SCHEMA_VERSION,
                database = %path.map(|p| p.display().to_string()).unwrap_or_else(|| ":memory:".into()),
                "this database was written by a different schema; dropping it and starting over"
            );
            conn.execute_batch(DROP_ALL).map_err(adapter_err)?;
        }

        conn.execute_batch(SCHEMA).map_err(adapter_err)?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(adapter_err)?;
        Ok(())
    }

    /// Every call hops to the blocking pool; rusqlite is synchronous and a
    /// locked mutex must never be held across an await.
    async fn with_conn<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = conn.lock().map_err(|_| {
                FactoryError::adapter("sqlite", "connection mutex poisoned by an earlier panic")
            })?;
            f(&mut guard)
        })
        .await
        .map_err(|e| FactoryError::adapter("sqlite", format!("blocking task: {e}")))?
    }
}

fn decode<T: serde::de::DeserializeOwned>(json: String) -> Result<T> {
    serde_json::from_str(&json)
        .map_err(|e| FactoryError::adapter("sqlite", format!("stored row is not readable: {e}")))
}

fn write_task(conn: &Connection, task: &Task) -> Result<()> {
    let data = serde_json::to_string(task).map_err(adapter_err)?;
    conn.execute(
        "INSERT INTO tasks (id, status, scope, next_run_at, created_at, updated_at, data)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
             status = excluded.status,
             scope = excluded.scope,
             next_run_at = excluded.next_run_at,
             updated_at = excluded.updated_at,
             data = excluded.data",
        params![
            task.id,
            task.status.as_str(),
            task.scope,
            task.next_run_at.map(|t| t.to_rfc3339()),
            task.created_at.to_rfc3339(),
            task.updated_at.to_rfc3339(),
            data,
        ],
    )
    .map_err(adapter_err)?;
    Ok(())
}

fn write_run(conn: &Connection, run: &Run) -> Result<()> {
    let data = serde_json::to_string(run).map_err(adapter_err)?;
    conn.execute(
        "INSERT INTO runs (id, task_id, attempt, status, started_at, ended_at, data)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
             status = excluded.status,
             ended_at = excluded.ended_at,
             data = excluded.data",
        params![
            run.id,
            run.task_id,
            run.attempt,
            run.status.as_str(),
            run.started_at.to_rfc3339(),
            run.ended_at.map(|t| t.to_rfc3339()),
            data,
        ],
    )
    .map_err(adapter_err)?;
    Ok(())
}

/// `None` when this database is not the one holding the task -- true for any
/// scope whose tasks live in another engine, and not an error on its own.
fn read_task_opt(conn: &Connection, id: &str) -> Result<Option<Task>> {
    let json: Option<String> = conn
        .query_row("SELECT data FROM tasks WHERE id = ?1", params![id], |r| r.get(0))
        .optional()
        .map_err(adapter_err)?;
    json.map(decode).transpose()
}

fn read_task(conn: &Connection, id: &str) -> Result<Task> {
    read_task_opt(conn, id)?.ok_or_else(|| FactoryError::TaskNotFound(id.to_string()))
}

/// A timestamp written by this store and read back. A row we cannot parse is
/// a bug in us, not bad input, so it is an adapter error rather than a skip.
fn parse_time(raw: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| adapter_err(format!("unreadable timestamp {raw:?}: {e}")))
}

fn collect<T: serde::de::DeserializeOwned>(
    stmt: &mut rusqlite::Statement<'_>,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Vec<T>> {
    let rows = stmt
        .query_map(args, |r| r.get::<_, String>(0))
        .map_err(adapter_err)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(decode(row.map_err(adapter_err)?)?);
    }
    Ok(out)
}

#[async_trait]
impl TaskStore for SqliteStore {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        if self.name == "sqlite" {
            "tasks, runs and journal in the instance's own sqlite database".into()
        } else {
            format!(
                "tasks, runs and journal in the instance's own sqlite database, registered as \"{}\"",
                self.name
            )
        }
    }

    async fn create(&self, task: &Task) -> Result<Task> {
        let task = task.clone();
        self.with_conn(move |conn| {
            write_task(conn, &task)?;
            Ok(task)
        })
        .await
    }

    async fn get(&self, id: &str) -> Result<Option<Task>> {
        let id = id.to_string();
        self.with_conn(move |conn| {
            let row: Option<String> = conn
                .query_row("SELECT data FROM tasks WHERE id = ?1", params![id], |r| r.get(0))
                .optional()
                .map_err(adapter_err)?;
            row.map(decode).transpose()
        })
        .await
    }

    async fn list(&self, filter: &TaskFilter) -> Result<Vec<Task>> {
        let filter = filter.clone();
        self.with_conn(move |conn| {
            let mut sql = String::from("SELECT data FROM tasks WHERE 1=1");
            let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            if let Some(status) = filter.status {
                sql.push_str(" AND status = ?");
                args.push(Box::new(status.as_str().to_string()));
            }
            if let Some(scope) = &filter.scope {
                sql.push_str(" AND scope = ?");
                args.push(Box::new(scope.clone()));
            }
            sql.push_str(" ORDER BY created_at DESC");
            if let Some(limit) = filter.limit {
                sql.push_str(&format!(" LIMIT {limit}"));
            }
            let mut stmt = conn.prepare(&sql).map_err(adapter_err)?;
            let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
            collect(&mut stmt, refs.as_slice())
        })
        .await
    }

    async fn update(&self, id: &str, patch: &TaskPatch) -> Result<Task> {
        let id = id.to_string();
        let patch = patch.clone();
        self.with_conn(move |conn| {
            let mut task = read_task(conn, &id)?;

            if let Some(v) = patch.title {
                task.title = v;
            }
            if let Some(v) = patch.instructions {
                task.instructions = v;
            }
            if let Some(v) = patch.status {
                task.status = v;
            }
            if patch.clear_schedule {
                task.schedule = None;
                task.next_run_at = None;
            }
            if let Some(v) = patch.schedule {
                task.schedule = Some(v);
            }
            if patch.clear_estimate {
                task.estimate_seconds = None;
            }
            if let Some(v) = patch.estimate_seconds {
                task.estimate_seconds = Some(v);
            }
            if let Some(v) = patch.scope {
                task.scope = v;
            }
            if let Some(v) = patch.agent {
                task.agent = v;
            }
            if let Some(v) = patch.runtime {
                task.runtime = v;
            }
            if patch.clear_ack_timeout {
                task.ack_timeout_seconds = None;
            }
            if let Some(v) = patch.ack_timeout_seconds {
                task.ack_timeout_seconds = Some(v);
            }
            if patch.clear_timeout {
                task.timeout_seconds = None;
            }
            if let Some(v) = patch.timeout_seconds {
                task.timeout_seconds = Some(v);
            }
            if patch.clear_blocked_timeout {
                task.blocked_timeout_seconds = None;
            }
            if let Some(v) = patch.blocked_timeout_seconds {
                task.blocked_timeout_seconds = Some(v);
            }
            if patch.clear_result {
                task.result = None;
            }
            if let Some(v) = patch.result {
                task.result = Some(v);
            }
            if patch.clear_routed_to {
                task.routed_to = None;
            }
            if let Some(v) = patch.routed_to {
                task.routed_to = Some(v);
            }
            if patch.clear_error {
                task.error = None;
            }
            if let Some(v) = patch.error {
                task.error = Some(v);
            }
            if let Some(v) = patch.runs {
                task.runs = v;
            }
            if let Some(v) = patch.last_run_at {
                task.last_run_at = Some(v);
            }
            if let Some(v) = patch.next_run_at {
                task.next_run_at = Some(v);
            }
            if let Some(v) = patch.labels {
                task.labels = v;
            }
            if patch.clear_retry {
                task.retry = None;
            }
            if let Some(v) = patch.retry {
                task.retry = Some(v);
            }
            if let Some(v) = patch.knowledge_hints {
                task.knowledge_hints = v;
            }
            if patch.clear_pending_retry {
                task.pending_retry = None;
            }
            if let Some(v) = patch.pending_retry {
                task.pending_retry = Some(v);
            }
            if let Some(v) = patch.schedule_paused {
                task.schedule_paused = v;
            }
            if patch.clear_category {
                task.category = None;
            }
            if let Some(v) = patch.category {
                task.category = Some(v);
            }
            if let Some(v) = patch.intake {
                task.intake = Some(v);
            }
            if patch.clear_failure {
                task.failure = None;
            }
            if let Some(v) = patch.failure {
                task.failure = Some(v);
            }
            if patch.clear_closure {
                task.closure = None;
            }
            if let Some(v) = patch.closure {
                task.closure = Some(v);
            }
            task.updated_at = Utc::now();

            write_task(conn, &task)?;
            Ok(task)
        })
        .await
    }

    async fn delete(&self, id: &str) -> Result<bool> {
        let id = id.to_string();
        self.with_conn(move |conn| {
            conn.execute("DELETE FROM task_entries WHERE task_id = ?1", params![id])
                .map_err(adapter_err)?;
            conn.execute("DELETE FROM runs WHERE task_id = ?1", params![id])
                .map_err(adapter_err)?;
            let n = conn
                .execute("DELETE FROM tasks WHERE id = ?1", params![id])
                .map_err(adapter_err)?;
            Ok(n > 0)
        })
        .await
    }

    async fn create_run(&self, new: &NewRun) -> Result<Run> {
        let new = new.clone();
        self.with_conn(move |conn| {
            // One transaction, so the attempt number a run gets cannot be the
            // one another dispatch is about to take.
            let tx = conn.transaction().map_err(adapter_err)?;

            let attempt: u32 = tx
                .query_row(
                    "SELECT COALESCE(MAX(attempt), 0) + 1 FROM runs WHERE task_id = ?1",
                    params![new.task_id],
                    |r| r.get(0),
                )
                .map_err(adapter_err)?;

            let started_at = Utc::now();
            let run = Run {
                id: uuid::Uuid::new_v4().to_string(),
                task_id: new.task_id.clone(),
                attempt,
                status: RunStatus::Dispatching,
                trigger: new.trigger,
                agent: new.agent.clone(),
                adapter: new.adapter.clone(),
                // Not known until the daemon has decided whether this run
                // gets one and, if so, made it -- which happens after the
                // run row exists, since the worktree is named after it.
                worktree_path: None,
                worktree_branch: None,
                runtime: new.runtime.clone(),
                session: None,
                token: Some(new.token.clone()),
                result: None,
                routed_to: None,
                error: None,
                started_at,
                ended_at: None,
                // Never later than the start: a slot or a request is
                // always in the past by the time the row exists, and a
                // caller that names none was dispatched on the spot.
                queued_at: Some(new.queued_at.map_or(started_at, |q| q.min(started_at))),
                scheduled_for: new.scheduled_for,
                fail_kind: None,
                blocked_since: None,
                blocked_source: None,
                block_suspected_since: None,
                turn_ended_at: None,
                turn_end_reason: None,
                required_steps: Vec::new(),
                usage: None,
            };
            write_run(&tx, &run)?;

            // The task's mirror of where it stands, updated in the same breath
            // -- but only when this database is the one holding the task. A
            // scope can keep its tasks in another engine while every run still
            // lands in the local ledger, and then mirroring the task is that
            // other store's job, not ours.
            if let Some(mut task) = read_task_opt(&tx, &new.task_id)? {
                task.runs = attempt;
                task.status = RunStatus::Dispatching.as_task_status();
                task.last_run_at = Some(run.started_at);
                // A new attempt is newer than whatever failed or closed the
                // task before it (`#122`): neither mirror may outlive it.
                task.failure = None;
                task.closure = None;
                task.routed_to = None;
                task.updated_at = Utc::now();
                write_task(&tx, &task)?;
            }

            tx.commit().map_err(adapter_err)?;
            Ok(run)
        })
        .await
    }

    async fn get_run(&self, id: &str) -> Result<Option<Run>> {
        let id = id.to_string();
        self.with_conn(move |conn| {
            let row: Option<String> = conn
                .query_row("SELECT data FROM runs WHERE id = ?1", params![id], |r| r.get(0))
                .optional()
                .map_err(adapter_err)?;
            row.map(decode).transpose()
        })
        .await
    }

    async fn update_run(&self, id: &str, patch: &RunPatch) -> Result<Run> {
        let id = id.to_string();
        let patch = patch.clone();
        self.with_conn(move |conn| {
            let json: Option<String> = conn
                .query_row("SELECT data FROM runs WHERE id = ?1", params![id], |r| r.get(0))
                .optional()
                .map_err(adapter_err)?;
            let mut run: Run = decode(
                json.ok_or_else(|| FactoryError::TaskNotFound(format!("run {id}")))?,
            )?;

            if let Some(v) = patch.status {
                run.status = v;
            }
            if patch.clear_session {
                run.session = None;
            }
            if let Some(v) = patch.session {
                run.session = Some(v);
            }
            if let Some(v) = patch.result {
                run.result = Some(v);
            }
            if patch.clear_routed_to {
                run.routed_to = None;
            }
            if let Some(v) = patch.routed_to {
                run.routed_to = Some(v);
            }
            if let Some(v) = patch.error {
                run.error = Some(v);
            }
            if patch.clear_token {
                run.token = None;
            }
            if let Some(v) = patch.ended_at {
                run.ended_at = Some(v);
            }
            if patch.clear_blocked {
                run.blocked_since = None;
                run.blocked_source = None;
            }
            if let Some(v) = patch.blocked_since {
                run.blocked_since = Some(v);
            }
            if let Some(v) = patch.blocked_source {
                run.blocked_source = Some(v);
            }
            if patch.clear_block_suspicion {
                run.block_suspected_since = None;
            }
            if let Some(v) = patch.block_suspected_since {
                run.block_suspected_since = Some(v);
            }
            if patch.clear_turn_ended {
                run.turn_ended_at = None;
                run.turn_end_reason = None;
            }
            if let Some(v) = patch.turn_ended_at {
                run.turn_ended_at = Some(v);
            }
            if let Some(v) = patch.turn_end_reason {
                run.turn_end_reason = Some(v);
            }
            // A run that reached a terminal state is over, whether or not the
            // caller remembered to say when.
            if run.status.is_terminal() && run.ended_at.is_none() {
                run.ended_at = Some(Utc::now());
            }
            if let Some(v) = patch.worktree_path {
                run.worktree_path = Some(v);
            }
            if let Some(v) = patch.worktree_branch {
                run.worktree_branch = Some(v);
            }
            if let Some(v) = patch.fail_kind {
                run.fail_kind = Some(v);
            }
            if let Some(v) = patch.required_steps {
                run.required_steps = v;
            }
            if let Some(v) = patch.usage {
                run.usage = Some(v);
            }

            write_run(conn, &run)?;
            Ok(run)
        })
        .await
    }

    async fn runs(&self, task_id: &str, limit: u32) -> Result<Vec<Run>> {
        let task_id = task_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT data FROM runs WHERE task_id = ?1 ORDER BY attempt DESC LIMIT ?2",
                )
                .map_err(adapter_err)?;
            collect(&mut stmt, params![task_id, limit])
        })
        .await
    }

    async fn active_run(&self, task_id: &str) -> Result<Option<Run>> {
        let task_id = task_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT data FROM runs
                     WHERE task_id = ?1 AND status NOT IN ('done', 'failed', 'cancelled')
                     ORDER BY attempt DESC LIMIT 1",
                )
                .map_err(adapter_err)?;
            Ok(collect::<Run>(&mut stmt, params![task_id])?.into_iter().next())
        })
        .await
    }

    async fn active_runs(&self) -> Result<Vec<Run>> {
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT data FROM runs
                     WHERE status NOT IN ('done', 'failed', 'cancelled')
                     ORDER BY started_at ASC",
                )
                .map_err(adapter_err)?;
            collect(&mut stmt, params![])
        })
        .await
    }

    async fn put_agent(&self, agent: &AgentSession) -> Result<()> {
        let agent = agent.clone();
        self.with_conn(move |conn| {
            let data = serde_json::to_string(&agent).map_err(adapter_err)?;
            conn.execute(
                "INSERT INTO agent_sessions (id, scope, name, state, data)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET
                     state = excluded.state,
                     data = excluded.data",
                params![agent.id, agent.scope, agent.name, agent.state.as_str(), data],
            )
            .map_err(adapter_err)?;
            Ok(())
        })
        .await
    }

    async fn get_agent(&self, id: &str) -> Result<Option<AgentSession>> {
        let id = id.to_string();
        self.with_conn(move |conn| {
            let row: Option<String> = conn
                .query_row(
                    "SELECT data FROM agent_sessions WHERE id = ?1",
                    params![id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(adapter_err)?;
            row.map(decode).transpose()
        })
        .await
    }

    async fn agents(&self) -> Result<Vec<AgentSession>> {
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare("SELECT data FROM agent_sessions ORDER BY scope, name")
                .map_err(adapter_err)?;
            collect(&mut stmt, params![])
        })
        .await
    }

    async fn delete_agent(&self, id: &str) -> Result<bool> {
        let id = id.to_string();
        self.with_conn(move |conn| {
            let n = conn
                .execute("DELETE FROM agent_sessions WHERE id = ?1", params![id])
                .map_err(adapter_err)?;
            Ok(n > 0)
        })
        .await
    }

    async fn append_entry(&self, task_id: &str, entry: &TaskEntry) -> Result<()> {
        let task_id = task_id.to_string();
        let entry = entry.clone();
        self.with_conn(move |conn| {
            let data = serde_json::to_string(&entry).map_err(adapter_err)?;
            conn.execute(
                "INSERT INTO task_entries (task_id, run_id, at, data) VALUES (?1, ?2, ?3, ?4)",
                params![task_id, entry.run_id, entry.at.to_rfc3339(), data],
            )
            .map_err(adapter_err)?;
            Ok(())
        })
        .await
    }

    async fn entries(&self, task_id: &str, limit: u32) -> Result<Vec<TaskEntry>> {
        let task_id = task_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT data FROM (
                         SELECT seq, data FROM task_entries WHERE task_id = ?1
                         ORDER BY seq DESC LIMIT ?2
                     ) ORDER BY seq ASC",
                )
                .map_err(adapter_err)?;
            collect(&mut stmt, params![task_id, limit])
        })
        .await
    }

    async fn task_own_entries(&self, task_id: &str, limit: u32) -> Result<Vec<TaskEntry>> {
        let task_id = task_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT data FROM (
                         SELECT seq, data FROM task_entries WHERE task_id = ?1 AND run_id IS NULL
                         ORDER BY seq DESC LIMIT ?2
                     ) ORDER BY seq ASC",
                )
                .map_err(adapter_err)?;
            collect(&mut stmt, params![task_id, limit])
        })
        .await
    }

    async fn run_entries(&self, run_id: &str, limit: u32) -> Result<Vec<TaskEntry>> {
        let run_id = run_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT data FROM (
                         SELECT seq, data FROM task_entries WHERE run_id = ?1
                         ORDER BY seq DESC LIMIT ?2
                     ) ORDER BY seq ASC",
                )
                .map_err(adapter_err)?;
            collect(&mut stmt, params![run_id, limit])
        })
        .await
    }

    async fn entries_of_kinds(
        &self,
        kinds: &[&str],
        since: DateTime<Utc>,
    ) -> Result<Vec<(String, TaskEntry)>> {
        if kinds.is_empty() {
            return Ok(Vec::new());
        }
        let kinds: Vec<String> = kinds.iter().map(|k| k.to_string()).collect();
        let since = since.to_rfc3339();
        self.with_conn(move |conn| {
            // The kind lives inside the JSON row, so it is matched there --
            // after the `at` index has narrowed the scan to the window.
            let marks: Vec<String> = (0..kinds.len()).map(|i| format!("?{}", i + 2)).collect();
            let sql = format!(
                "SELECT task_id, data FROM task_entries
                 WHERE at > ?1 AND json_extract(data, '$.kind') IN ({})
                 ORDER BY seq ASC",
                marks.join(", ")
            );
            let mut stmt = conn.prepare(&sql).map_err(adapter_err)?;
            let mut args: Vec<&dyn rusqlite::ToSql> = vec![&since];
            for kind in &kinds {
                args.push(kind);
            }
            let rows = stmt
                .query_map(args.as_slice(), |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(adapter_err)?;
            let mut out = Vec::new();
            for row in rows {
                let (task_id, data) = row.map_err(adapter_err)?;
                out.push((task_id, decode(data)?));
            }
            Ok(out)
        })
        .await
    }

    async fn runs_between(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Vec<Run>> {
        let (from, to) = (from.to_rfc3339(), to.to_rfc3339());
        self.with_conn(move |conn| {
            // A run overlaps the window if it started before the window ended
            // and has not finished before the window began. An open run has no
            // `ended_at`, and is still going by definition.
            let mut stmt = conn
                .prepare(
                    "SELECT data FROM runs
                     WHERE started_at <= ?2 AND (ended_at IS NULL OR ended_at >= ?1)
                     ORDER BY started_at ASC",
                )
                .map_err(adapter_err)?;
            collect(&mut stmt, params![from, to])
        })
        .await
    }

    async fn append_usage(&self, snapshot: &UsageSnapshot) -> Result<()> {
        let data = serde_json::to_string(snapshot).map_err(adapter_err)?;
        let (run_id, task_id) = (snapshot.run_id.clone(), snapshot.task_id.clone());
        let (point, at) = (snapshot.point.as_str(), snapshot.at.to_rfc3339());
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO run_usage (run_id, task_id, point, at, data) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![run_id, task_id, point, at, data],
            )
            .map_err(adapter_err)?;
            Ok(())
        })
        .await
    }

    async fn usage_snapshots(&self, run_id: &str) -> Result<Vec<UsageSnapshot>> {
        let run_id = run_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare("SELECT data FROM run_usage WHERE run_id = ?1 ORDER BY seq ASC")
                .map_err(adapter_err)?;
            collect(&mut stmt, params![run_id])
        })
        .await
    }

    async fn append_status(&self, change: &StatusChange) -> Result<()> {
        let (subject, scope, agent) = (
            change.subject.clone(),
            change.scope.clone(),
            change.agent.clone(),
        );
        let (status, at) = (change.status.as_str(), change.at.to_rfc3339());
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO agent_status (subject, scope, agent, status, at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![subject, scope, agent, status, at],
            )
            .map_err(adapter_err)?;
            Ok(())
        })
        .await
    }

    async fn status_changes(&self, since: DateTime<Utc>) -> Result<Vec<StatusChange>> {
        let since = since.to_rfc3339();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT subject, scope, agent, status, at FROM agent_status
                     WHERE at >= ?1 ORDER BY seq ASC",
                )
                .map_err(adapter_err)?;
            let rows = stmt
                .query_map(params![since], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                })
                .map_err(adapter_err)?;
            let mut out = Vec::new();
            for row in rows {
                let (subject, scope, agent, status, at) = row.map_err(adapter_err)?;
                out.push(StatusChange {
                    subject,
                    scope,
                    agent,
                    status: RuntimeStatus::parse(&status),
                    at: parse_time(&at)?,
                });
            }
            Ok(out)
        })
        .await
    }

    async fn status_origin(&self) -> Result<Option<DateTime<Utc>>> {
        self.with_conn(move |conn| {
            let raw: Option<String> = conn
                .query_row("SELECT at FROM agent_status ORDER BY seq ASC LIMIT 1", [], |r| r.get(0))
                .optional()
                .map_err(adapter_err)?;
            raw.map(|at| parse_time(&at)).transpose()
        })
        .await
    }

    async fn due(&self, now: DateTime<Utc>) -> Result<Vec<Task>> {
        let now = now.to_rfc3339();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT data FROM tasks
                     WHERE next_run_at IS NOT NULL AND next_run_at <= ?1 AND status IN ('pending', 'blocked')
                     ORDER BY next_run_at ASC",
                )
                .map_err(adapter_err)?;
            // A scheduled task blocked by a failure keeps firing (`#122`):
            // its schedule is still the intent, and the next run that
            // succeeds clears the block. One blocked on a question has an
            // active run and is not due -- `failure` is only ever set on a
            // task whose newest run has ended. It lives only in the JSON
            // row, like `schedule_paused` below, so it is sorted out after
            // decoding.
            // `schedule_paused` lives only in the JSON row -- no column, so
            // no schema change, and a schema change here drops the whole
            // database (see the module comment). Due tasks are a handful
            // per tick, so passing over the paused ones after decoding
            // costs nothing.
            let mut due: Vec<Task> = collect(&mut stmt, params![now])?;
            due.retain(|t| !t.schedule_paused && t.fires());
            Ok(due)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_core::run::{BlockSource, Trigger};
    use factory_core::task::TaskStatus;

    fn sample_task(id: &str) -> Task {
        let now = Utc::now();
        Task {
            id: id.to_string(),
            title: "a title".into(),
            instructions: "do the thing".into(),
            scope: "demo".into(),
            agent: "assistant".into(),
            runtime: "shell".into(),
            status: TaskStatus::Pending,
            schedule: None,
            estimate_seconds: None,
            result: None,
            routed_to: None,
            error: None,
            runs: 0,
            ack_timeout_seconds: None,
            timeout_seconds: None,
            blocked_timeout_seconds: None,
            labels: Default::default(),
            created_at: now,
            updated_at: now,
            last_run_at: None,
            next_run_at: None,
            worktree: false,
            knowledge_hints: false,
            workflow_origin: None,
            bench_origin: None,
            retry: None,
            pending_retry: None,
            schedule_paused: false,
            category: None,
            intake: None,
            failure: None,
            closure: None,
        }
    }

    fn sample_new_run(task_id: &str) -> NewRun {
        NewRun {
            task_id: task_id.to_string(),
            trigger: Trigger::Manual,
            agent: "assistant".into(),
            adapter: "shell".into(),
            runtime: "shell".into(),
            token: "tok".into(),
            queued_at: None,
            scheduled_for: None,
        }
    }

    // The regression guard: a task this database actually holds must still
    // get its mirror updated exactly as before, whatever else `create_run`
    // now tolerates.
    #[tokio::test]
    async fn a_run_for_a_task_the_store_holds_still_bumps_its_mirror() {
        let store = SqliteStore::in_memory().unwrap();
        let task = sample_task("t1");
        store.create(&task).await.unwrap();

        let run = store.create_run(&sample_new_run("t1")).await.unwrap();
        assert_eq!(run.attempt, 1);

        let mirrored = store.get("t1").await.unwrap().unwrap();
        assert_eq!(mirrored.runs, 1);
        assert_eq!(mirrored.status, TaskStatus::Dispatching);
        assert_eq!(mirrored.last_run_at, Some(run.started_at));
    }

    // A scope's tasks can live in another engine entirely; the run still
    // belongs in this local ledger, and there is no row here to mirror it onto.
    #[tokio::test]
    async fn a_run_can_be_created_for_a_task_this_database_does_not_hold() {
        let store = SqliteStore::in_memory().unwrap();

        let run = store.create_run(&sample_new_run("elsewhere-1")).await.unwrap();
        assert_eq!(run.attempt, 1);
        assert_eq!(run.task_id, "elsewhere-1");

        // Nothing was ever written to the tasks table for it.
        assert!(store.get("elsewhere-1").await.unwrap().is_none());
        assert!(store.list(&TaskFilter::default()).await.unwrap().is_empty());
    }

    // The attempt counter is read from the runs table, not the task row, so it
    // still climbs correctly when there is no task row to read.
    #[tokio::test]
    async fn a_second_run_for_a_task_the_store_does_not_hold_gets_attempt_two() {
        let store = SqliteStore::in_memory().unwrap();

        let first = store.create_run(&sample_new_run("elsewhere-2")).await.unwrap();
        let second = store.create_run(&sample_new_run("elsewhere-2")).await.unwrap();

        assert_eq!(first.attempt, 1);
        assert_eq!(second.attempt, 2);
    }

    // Two sqlite databases can be registered side by side under different
    // names -- proof that the routing a multi-store instance depends on
    // actually works, not just that the type signature allows it.
    #[tokio::test]
    async fn two_in_memory_named_stores_report_different_names_and_do_not_see_each_others_tasks() {
        let a = SqliteStore::in_memory_named("a").unwrap();
        let b = SqliteStore::in_memory_named("b").unwrap();
        assert_eq!(a.name(), "a");
        assert_eq!(b.name(), "b");

        a.create(&sample_task("only-in-a")).await.unwrap();

        assert!(a.get("only-in-a").await.unwrap().is_some());
        assert!(b.get("only-in-a").await.unwrap().is_none());
    }

    /// `Run` is kept as JSON in a `data` column with nothing lifted out for
    /// the three new fields, so nothing here needed a schema change -- this
    /// pins down that the round trip actually holds, not just that it ought
    /// to.
    #[tokio::test]
    async fn a_runs_block_state_round_trips_through_sqlite() {
        let store = SqliteStore::in_memory().unwrap();
        store.create(&sample_task("t1")).await.unwrap();
        let run = store
            .create_run(&NewRun {
                task_id: "t1".into(),
                trigger: Trigger::Manual,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: "herdr".into(),
                token: "tok".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        assert!(run.blocked_since.is_none(), "a fresh run starts with no block at all");

        let since = Utc::now();
        store
            .update_run(
                &run.id,
                &RunPatch {
                    status: Some(RunStatus::Blocked),
                    blocked_since: Some(since),
                    blocked_source: Some(BlockSource::Runtime),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let reloaded = store.get_run(&run.id).await.unwrap().unwrap();
        assert_eq!(reloaded.status, RunStatus::Blocked);
        assert_eq!(reloaded.blocked_source, Some(BlockSource::Runtime));
        assert_eq!(reloaded.blocked_since, Some(since));

        let cleared = store
            .update_run(
                &run.id,
                &RunPatch {
                    clear_blocked: true,
                    block_suspected_since: Some(since),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(cleared.blocked_since.is_none(), "clear_blocked drops the timestamp");
        assert!(cleared.blocked_source.is_none(), "clear_blocked drops the source too");
        assert_eq!(cleared.block_suspected_since, Some(since), "a suspicion is a separate field");

        let unsuspected = store
            .update_run(
                &run.id,
                &RunPatch {
                    clear_block_suspicion: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(unsuspected.block_suspected_since.is_none());
    }

    #[tokio::test]
    async fn a_tasks_blocked_timeout_override_round_trips_and_clears() {
        let store = SqliteStore::in_memory().unwrap();
        store.create(&sample_task("t1")).await.unwrap();

        let with_override = store
            .update(
                "t1",
                &TaskPatch {
                    blocked_timeout_seconds: Some(7200),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(with_override.blocked_timeout_seconds, Some(7200));

        let reloaded = store.get("t1").await.unwrap().unwrap();
        assert_eq!(reloaded.blocked_timeout_seconds, Some(7200));

        let cleared = store
            .update(
                "t1",
                &TaskPatch {
                    clear_blocked_timeout: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(cleared.blocked_timeout_seconds.is_none());
    }

    #[tokio::test]
    async fn a_tasks_estimate_round_trips_and_clears() {
        let store = SqliteStore::in_memory().unwrap();
        store.create(&sample_task("t1")).await.unwrap();

        let estimated = store
            .update(
                "t1",
                &TaskPatch {
                    estimate_seconds: Some(900),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(estimated.estimate_seconds, Some(900));
        assert_eq!(
            store.get("t1").await.unwrap().unwrap().estimate_seconds,
            Some(900)
        );

        let cleared = store
            .update(
                "t1",
                &TaskPatch {
                    clear_estimate: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(cleared.estimate_seconds, None);
        assert_eq!(
            store.get("t1").await.unwrap().unwrap().estimate_seconds,
            None
        );
    }

    // The three operations facts ride in the JSON row, with no column of
    // their own -- this pins the round trip, and that a caller naming no
    // queue time gets the run's own start rather than nothing.
    #[tokio::test]
    async fn a_runs_queue_and_fail_facts_round_trip_through_sqlite() {
        use factory_core::run::FailKind;
        let store = SqliteStore::in_memory().unwrap();
        store.create(&sample_task("t1")).await.unwrap();

        let slot = Utc::now() - chrono::Duration::minutes(3);
        let run = store
            .create_run(&NewRun {
                trigger: Trigger::Schedule,
                queued_at: Some(slot),
                scheduled_for: Some(slot),
                ..sample_new_run("t1")
            })
            .await
            .unwrap();
        assert_eq!(run.queued_at, Some(slot));
        assert_eq!(run.scheduled_for, Some(slot));
        assert_eq!(run.fail_kind, None);

        store
            .update_run(
                &run.id,
                &RunPatch {
                    status: Some(RunStatus::Failed),
                    fail_kind: Some(FailKind::AckTimeout),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let reloaded = store.get_run(&run.id).await.unwrap().unwrap();
        assert_eq!(reloaded.queued_at, Some(slot));
        assert_eq!(reloaded.scheduled_for, Some(slot));
        assert_eq!(reloaded.fail_kind, Some(FailKind::AckTimeout));

        let manual = store.create_run(&sample_new_run("t1")).await.unwrap();
        assert_eq!(manual.queued_at, Some(manual.started_at), "no queue time named means dispatched on the spot");
        assert_eq!(manual.scheduled_for, None);

        let future = store
            .create_run(&NewRun {
                queued_at: Some(Utc::now() + chrono::Duration::hours(1)),
                ..sample_new_run("t1")
            })
            .await
            .unwrap();
        assert_eq!(future.queued_at, Some(future.started_at), "a queue wait is never negative");
    }

    #[tokio::test]
    async fn a_paused_schedule_is_not_due_and_round_trips() {
        let store = SqliteStore::in_memory().unwrap();
        let past = Utc::now() - chrono::Duration::minutes(1);
        for id in ["running", "paused"] {
            let mut task = sample_task(id);
            task.schedule = Some(factory_core::task::Schedule::Every { seconds: 60 });
            task.next_run_at = Some(past);
            store.create(&task).await.unwrap();
        }

        let paused = store
            .update("paused", &TaskPatch { schedule_paused: Some(true), ..Default::default() })
            .await
            .unwrap();
        assert!(paused.schedule_paused);
        assert!(paused.schedule.is_some(), "pausing keeps the schedule");
        assert_eq!(paused.next_run_at, Some(past), "and the slot it would have fired");
        assert!(store.get("paused").await.unwrap().unwrap().schedule_paused);

        let due: Vec<String> = store.due(Utc::now()).await.unwrap().into_iter().map(|t| t.id).collect();
        assert_eq!(due, vec!["running".to_string()]);

        let resumed = store
            .update("paused", &TaskPatch { schedule_paused: Some(false), ..Default::default() })
            .await
            .unwrap();
        assert!(!resumed.schedule_paused);
        assert_eq!(store.due(Utc::now()).await.unwrap().len(), 2);
    }

    /// `#122`: a scheduled task blocked by a failure keeps firing; one
    /// blocked on a question (it has a run) and a closed one do not. A new
    /// run clears the failure and any close record with it.
    #[tokio::test]
    async fn a_task_blocked_by_a_failure_is_due_and_a_new_run_clears_the_mirror() {
        let store = SqliteStore::in_memory().unwrap();
        let past = Utc::now() - chrono::Duration::minutes(1);
        for (id, status) in [
            ("failed", TaskStatus::Blocked),
            ("asking", TaskStatus::Blocked),
            ("closed", TaskStatus::Cancelled),
        ] {
            let mut task = sample_task(id);
            task.schedule = Some(factory_core::task::Schedule::Every { seconds: 60 });
            task.next_run_at = Some(past);
            task.status = status;
            store.create(&task).await.unwrap();
        }
        let failure = factory_core::task::TaskFailure {
            kind: Some(factory_core::run::FailKind::AgentFailed),
            run_id: Some("r0".into()),
            attempt: Some(1),
            at: Utc::now(),
        };
        let marked = store
            .update("failed", &TaskPatch { failure: Some(failure), ..Default::default() })
            .await
            .unwrap();
        assert!(marked.blocked_by_failure(), "the failure round-trips");
        let due: Vec<String> = store.due(Utc::now()).await.unwrap().into_iter().map(|t| t.id).collect();
        assert_eq!(due, vec!["failed".to_string()]);

        store
            .create_run(&NewRun {
                task_id: "failed".into(),
                trigger: Trigger::Schedule,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: "herdr".into(),
                token: "t".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        let task = store.get("failed").await.unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Dispatching);
        assert!(task.failure.is_none(), "a new attempt is newer than the failure");
    }

    #[tokio::test]
    async fn usage_snapshots_are_kept_in_order_and_an_existing_database_gains_the_table() {
        use factory_core::usage::{SnapshotPoint, UsageSnapshot};
        let dir = std::env::temp_dir().join(format!("factory-sqlite-usage-{}", uuid::Uuid::new_v4()));
        let path = dir.join("f.sqlite");
        {
            let store = SqliteStore::open(&path).unwrap();
            store.create(&sample_task("t1")).await.unwrap();
        }
        // A database written before `run_usage` existed, at the same version.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("DROP TABLE run_usage;").unwrap();
        }
        let store = SqliteStore::open(&path).unwrap();
        assert!(store.get("t1").await.unwrap().is_some(), "gaining a table drops nothing");

        let snap = |point, reason: &str| UsageSnapshot {
            run_id: "r1".into(),
            task_id: "t1".into(),
            point,
            at: Utc::now(),
            runtime: "herdr".into(),
            usage: None,
            unknown: Some(reason.into()),
        };
        store.append_usage(&snap(SnapshotPoint::Dispatch, "a")).await.unwrap();
        store.append_usage(&snap(SnapshotPoint::RunEnd, "b")).await.unwrap();
        store.append_usage(&UsageSnapshot { run_id: "r2".into(), ..snap(SnapshotPoint::Dispatch, "c") }).await.unwrap();
        let got = store.usage_snapshots("r1").await.unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].unknown.as_deref(), Some("a"));
        assert_eq!(got[1].point, SnapshotPoint::RunEnd);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn entries_of_kinds_reads_a_window_of_named_kinds_across_tasks() {
        let store = SqliteStore::in_memory().unwrap();
        let now = Utc::now();
        let at = |mins: i64, kind: &str| {
            let mut e = TaskEntry::new("daemon", kind, format!("{kind} {mins}m ago"));
            e.at = now - chrono::Duration::minutes(mins);
            e
        };
        store.append_entry("a", &at(120, "schedule_skipped")).await.unwrap();
        store.append_entry("a", &at(30, "schedule_skipped")).await.unwrap();
        store.append_entry("a", &at(20, "transcript")).await.unwrap();
        store.append_entry("b", &at(10, "answer")).await.unwrap();

        let found = store
            .entries_of_kinds(&["schedule_skipped", "answer"], now - chrono::Duration::hours(1))
            .await
            .unwrap();
        let seen: Vec<(String, String)> = found.into_iter().map(|(t, e)| (t, e.kind)).collect();
        assert_eq!(
            seen,
            vec![("a".to_string(), "schedule_skipped".to_string()), ("b".to_string(), "answer".to_string())],
            "only the named kinds, only inside the window, oldest first"
        );
        assert!(store.entries_of_kinds(&[], now - chrono::Duration::days(1)).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_tasks_own_entries_are_not_crowded_out_by_its_runs() {
        let store = SqliteStore::in_memory().unwrap();
        store.append_entry("t", &TaskEntry::new("owner", "schedule_paused", "paused")).await.unwrap();
        for i in 0..300 {
            store.append_entry("t", &TaskEntry::new("agent", "progress", format!("line {i}")).in_run("r")).await.unwrap();
        }
        store.append_entry("t", &TaskEntry::new("owner", "schedule_resumed", "resumed")).await.unwrap();
        assert!(!store.entries("t", 200).await.unwrap().iter().any(|e| e.kind == "schedule_paused"), "the run's lines push it out");
        let own: Vec<String> = store.task_own_entries("t", 200).await.unwrap().into_iter().map(|e| e.kind).collect();
        assert_eq!(own, vec!["schedule_paused", "schedule_resumed"], "oldest first, no run's lines");
        assert_eq!(store.task_own_entries("t", 1).await.unwrap()[0].kind, "schedule_resumed", "the newest when limited");
    }
}
