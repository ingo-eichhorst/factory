//! The built-in task store. Whole tasks are kept as JSON with the few fields
//! the daemon queries on lifted out into columns, so the domain model can move
//! without a migration every time.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use factory_core::adapter::TaskStore;
use factory_core::error::{FactoryError, Result};
use factory_core::task::{Task, TaskEntry, TaskFilter, TaskPatch};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::{Arc, Mutex};

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

CREATE TABLE IF NOT EXISTS task_entries (
    seq     INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    at      TEXT NOT NULL,
    data    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS entries_task ON task_entries(task_id, seq);
"#;

pub struct SqliteStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| FactoryError::adapter("sqlite", e.to_string()))?;
        }
        let conn = Connection::open(path).map_err(|e| FactoryError::adapter("sqlite", e.to_string()))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .map_err(|e| FactoryError::adapter("sqlite", e.to_string()))?;
        conn.execute_batch(SCHEMA)
            .map_err(|e| FactoryError::adapter("sqlite", e.to_string()))?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()
            .map_err(|e| FactoryError::adapter("sqlite", e.to_string()))?;
        conn.execute_batch(SCHEMA)
            .map_err(|e| FactoryError::adapter("sqlite", e.to_string()))?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Every call hops to the blocking pool; rusqlite is synchronous and a
    /// locked mutex must never be held across an await.
    async fn with_conn<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let guard = conn.lock().map_err(|_| {
                FactoryError::adapter("sqlite", "connection mutex poisoned by an earlier panic")
            })?;
            f(&guard)
        })
        .await
        .map_err(|e| FactoryError::adapter("sqlite", format!("blocking task: {e}")))?
    }
}

fn row_to_task(json: String) -> Result<Task> {
    serde_json::from_str(&json)
        .map_err(|e| FactoryError::adapter("sqlite", format!("stored task is not readable: {e}")))
}

fn write_task(conn: &Connection, task: &Task) -> Result<()> {
    let data = serde_json::to_string(task)
        .map_err(|e| FactoryError::adapter("sqlite", e.to_string()))?;
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
    .map_err(|e| FactoryError::adapter("sqlite", e.to_string()))?;
    Ok(())
}

#[async_trait]
impl TaskStore for SqliteStore {
    fn name(&self) -> &str {
        "sqlite"
    }

    fn description(&self) -> String {
        "tasks in the instance's own sqlite database".into()
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
                .map_err(|e| FactoryError::adapter("sqlite", e.to_string()))?;
            row.map(row_to_task).transpose()
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
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| FactoryError::adapter("sqlite", e.to_string()))?;
            let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
            let rows = stmt
                .query_map(refs.as_slice(), |r| r.get::<_, String>(0))
                .map_err(|e| FactoryError::adapter("sqlite", e.to_string()))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row_to_task(
                    row.map_err(|e| FactoryError::adapter("sqlite", e.to_string()))?,
                )?);
            }
            Ok(out)
        })
        .await
    }

    async fn update(&self, id: &str, patch: &TaskPatch) -> Result<Task> {
        let id = id.to_string();
        let patch = patch.clone();
        self.with_conn(move |conn| {
            let json: Option<String> = conn
                .query_row("SELECT data FROM tasks WHERE id = ?1", params![id], |r| r.get(0))
                .optional()
                .map_err(|e| FactoryError::adapter("sqlite", e.to_string()))?;
            let mut task = row_to_task(json.ok_or_else(|| FactoryError::TaskNotFound(id.clone()))?)?;

            if let Some(v) = patch.title {
                task.title = v;
            }
            if let Some(v) = patch.instructions {
                task.instructions = v;
            }
            if let Some(v) = patch.status {
                task.status = v;
            }
            if let Some(v) = patch.schedule {
                task.schedule = Some(v);
            }
            if patch.clear_session {
                task.session = None;
            }
            if let Some(v) = patch.session {
                task.session = Some(v);
            }
            if let Some(v) = patch.result {
                task.result = Some(v);
            }
            if let Some(v) = patch.error {
                task.error = Some(v);
            }
            if let Some(v) = patch.token {
                task.token = Some(v);
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
                .map_err(|e| FactoryError::adapter("sqlite", e.to_string()))?;
            let n = conn
                .execute("DELETE FROM tasks WHERE id = ?1", params![id])
                .map_err(|e| FactoryError::adapter("sqlite", e.to_string()))?;
            Ok(n > 0)
        })
        .await
    }

    async fn append_entry(&self, id: &str, entry: &TaskEntry) -> Result<()> {
        let id = id.to_string();
        let entry = entry.clone();
        self.with_conn(move |conn| {
            let data = serde_json::to_string(&entry)
                .map_err(|e| FactoryError::adapter("sqlite", e.to_string()))?;
            conn.execute(
                "INSERT INTO task_entries (task_id, at, data) VALUES (?1, ?2, ?3)",
                params![id, entry.at.to_rfc3339(), data],
            )
            .map_err(|e| FactoryError::adapter("sqlite", e.to_string()))?;
            Ok(())
        })
        .await
    }

    async fn entries(&self, id: &str, limit: u32) -> Result<Vec<TaskEntry>> {
        let id = id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT data FROM (
                         SELECT seq, data FROM task_entries WHERE task_id = ?1
                         ORDER BY seq DESC LIMIT ?2
                     ) ORDER BY seq ASC",
                )
                .map_err(|e| FactoryError::adapter("sqlite", e.to_string()))?;
            let rows = stmt
                .query_map(params![id, limit], |r| r.get::<_, String>(0))
                .map_err(|e| FactoryError::adapter("sqlite", e.to_string()))?;
            let mut out = Vec::new();
            for row in rows {
                let json = row.map_err(|e| FactoryError::adapter("sqlite", e.to_string()))?;
                out.push(
                    serde_json::from_str(&json)
                        .map_err(|e| FactoryError::adapter("sqlite", e.to_string()))?,
                );
            }
            Ok(out)
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
                .map_err(|e| FactoryError::adapter("sqlite", e.to_string()))?;
            let rows = stmt
                .query_map(params![now], |r| r.get::<_, String>(0))
                .map_err(|e| FactoryError::adapter("sqlite", e.to_string()))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row_to_task(
                    row.map_err(|e| FactoryError::adapter("sqlite", e.to_string()))?,
                )?);
            }
            Ok(out)
        })
        .await
    }
}
