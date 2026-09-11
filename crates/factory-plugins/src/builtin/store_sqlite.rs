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
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Bumped whenever the shape below changes. A database at any other version is
/// discarded.
const SCHEMA_VERSION: i64 = 4;

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
"#;

/// What a version mismatch throws away. `agent_status` is not in here on
/// purpose: it is an observation log, and the runs it annotates being rebuilt
/// is no reason to forget what the agents were doing.
const DROP_ALL: &str = r#"
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
            if patch.clear_result {
                task.result = None;
            }
            if let Some(v) = patch.result {
                task.result = Some(v);
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

            let run = Run {
                id: uuid::Uuid::new_v4().to_string(),
                task_id: new.task_id.clone(),
                attempt,
                status: RunStatus::Dispatching,
                trigger: new.trigger,
                agent: new.agent.clone(),
                adapter: new.adapter.clone(),
                runtime: new.runtime.clone(),
                session: None,
                token: Some(new.token.clone()),
                result: None,
                error: None,
                started_at: Utc::now(),
                ended_at: None,
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
            if let Some(v) = patch.error {
                run.error = Some(v);
            }
            if patch.clear_token {
                run.token = None;
            }
            if let Some(v) = patch.ended_at {
                run.ended_at = Some(v);
            }
            // A run that reached a terminal state is over, whether or not the
            // caller remembered to say when.
            if run.status.is_terminal() && run.ended_at.is_none() {
                run.ended_at = Some(Utc::now());
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
                     WHERE next_run_at IS NOT NULL AND next_run_at <= ?1 AND status = 'pending'
                     ORDER BY next_run_at ASC",
                )
                .map_err(adapter_err)?;
            collect(&mut stmt, params![now])
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_core::run::Trigger;
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
            result: None,
            error: None,
            runs: 0,
            ack_timeout_seconds: None,
            timeout_seconds: None,
            labels: Default::default(),
            created_at: now,
            updated_at: now,
            last_run_at: None,
            next_run_at: None,
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
}
