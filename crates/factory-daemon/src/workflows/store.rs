use factory_core::error::{FactoryError, Result};
use factory_core::workflow::{WorkflowDefinition, WorkflowRun, WorkflowRunStatus};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::{Arc, Mutex};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS workflow_definitions (
    id TEXT PRIMARY KEY,
    scope TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    data TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS workflows_scope ON workflow_definitions(scope, updated_at);
CREATE TABLE IF NOT EXISTS workflow_runs (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    status TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    data TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS workflow_runs_definition ON workflow_runs(workflow_id, updated_at);
CREATE INDEX IF NOT EXISTS workflow_runs_active ON workflow_runs(status, updated_at);
"#;

fn error(error: impl std::fmt::Display) -> FactoryError {
    FactoryError::adapter("workflow sqlite", error.to_string())
}

fn decode<T: serde::de::DeserializeOwned>(json: String) -> Result<T> {
    serde_json::from_str(&json).map_err(error)
}

/// Decode every row, skipping (and naming) whichever ones do not. One
/// corrupted row is data, not an outage: a list that failed outright over it
/// would take the whole board down with it, and recovery would never reach
/// the runs sitting next to it in the same table.
fn decode_all<T: serde::de::DeserializeOwned>(rows: Vec<(String, String)>, table: &str) -> Vec<T> {
    rows.into_iter()
        .filter_map(|(id, json)| match decode::<T>(json) {
            Ok(value) => Some(value),
            Err(err) => {
                tracing::warn!(id, table, "skipping malformed row: {err}");
                None
            }
        })
        .collect()
}

#[derive(Clone)]
pub struct WorkflowStore {
    conn: Arc<Mutex<Connection>>,
}

impl WorkflowStore {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).map_err(error)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .map_err(error)?;
        conn.execute_batch(SCHEMA).map_err(error)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(error)?;
        conn.execute_batch(SCHEMA).map_err(error)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    async fn with_conn<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = conn
                .lock()
                .map_err(|_| error("connection mutex poisoned"))?;
            f(&mut guard)
        })
        .await
        .map_err(error)?
    }

    pub async fn put_definition(&self, workflow: &WorkflowDefinition) -> Result<()> {
        let workflow = workflow.clone();
        self.with_conn(move |conn| {
            let data = serde_json::to_string(&workflow).map_err(error)?;
            conn.execute(
                "INSERT INTO workflow_definitions (id, scope, updated_at, data) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET scope=excluded.scope, updated_at=excluded.updated_at, data=excluded.data",
                params![workflow.id, workflow.scope, workflow.updated_at.to_rfc3339(), data],
            ).map_err(error)?;
            Ok(())
        }).await
    }

    pub async fn get_definition(&self, id: &str) -> Result<Option<WorkflowDefinition>> {
        let id = id.to_string();
        self.with_conn(move |conn| {
            let json = conn
                .query_row(
                    "SELECT data FROM workflow_definitions WHERE id=?1",
                    [&id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(error)?;
            json.map(decode).transpose()
        })
        .await
    }

    pub async fn definitions(&self, scope: Option<&str>) -> Result<Vec<WorkflowDefinition>> {
        let scope = scope.map(str::to_string);
        self.with_conn(move |conn| {
            let (sql, arg): (&str, Option<&str>) = match scope.as_deref() {
                Some(scope) => (
                    "SELECT id, data FROM workflow_definitions WHERE scope=?1 ORDER BY updated_at DESC",
                    Some(scope),
                ),
                None => (
                    "SELECT id, data FROM workflow_definitions ORDER BY updated_at DESC",
                    None,
                ),
            };
            let mut stmt = conn.prepare(sql).map_err(error)?;
            let row = |row: &rusqlite::Row| -> rusqlite::Result<(String, String)> {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            };
            let rows = match arg {
                Some(value) => stmt
                    .query_map([value], row)
                    .map_err(error)?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(error)?,
                None => stmt
                    .query_map([], row)
                    .map_err(error)?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(error)?,
            };
            Ok(decode_all(rows, "workflow_definitions"))
        })
        .await
    }

    pub async fn delete_definition(&self, id: &str) -> Result<bool> {
        let id = id.to_string();
        self.with_conn(move |conn| {
            Ok(conn
                .execute("DELETE FROM workflow_definitions WHERE id=?1", [&id])
                .map_err(error)?
                > 0)
        })
        .await
    }

    pub async fn put_run(&self, run: &WorkflowRun) -> Result<()> {
        let run = run.clone();
        self.with_conn(move |conn| {
            let data = serde_json::to_string(&run).map_err(error)?;
            let status = match run.status {
                WorkflowRunStatus::Running => "running",
                WorkflowRunStatus::Done => "done",
                WorkflowRunStatus::Failed => "failed",
                WorkflowRunStatus::Cancelled => "cancelled",
            };
            conn.execute(
                "INSERT INTO workflow_runs (id, workflow_id, scope, status, updated_at, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET status=excluded.status, updated_at=excluded.updated_at, data=excluded.data",
                params![run.id, run.workflow_id, run.scope, status, run.updated_at.to_rfc3339(), data],
            ).map_err(error)?;
            Ok(())
        }).await
    }

    pub async fn get_run(&self, id: &str) -> Result<Option<WorkflowRun>> {
        let id = id.to_string();
        self.with_conn(move |conn| {
            let json = conn
                .query_row("SELECT data FROM workflow_runs WHERE id=?1", [&id], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(error)?;
            json.map(decode).transpose()
        })
        .await
    }

    pub async fn runs(
        &self,
        workflow_id: Option<&str>,
        scope: Option<&str>,
        limit: u32,
    ) -> Result<Vec<WorkflowRun>> {
        let workflow_id = workflow_id.map(str::to_string);
        let scope = scope.map(str::to_string);
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, data FROM workflow_runs
                 WHERE (?1 IS NULL OR workflow_id=?1) AND (?2 IS NULL OR scope=?2)
                 ORDER BY updated_at DESC LIMIT ?3",
                )
                .map_err(error)?;
            let rows = stmt
                .query_map(params![workflow_id, scope, limit], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(error)?;
            Ok(decode_all(rows, "workflow_runs"))
        })
        .await
    }

    pub async fn active_runs(&self) -> Result<Vec<WorkflowRun>> {
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, data FROM workflow_runs WHERE status='running' ORDER BY updated_at",
                )
                .map_err(error)?;
            let rows = stmt
                .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
                .map_err(error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(error)?;
            Ok(decode_all(rows, "workflow_runs"))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_core::workflow::WorkflowDraft;

    #[tokio::test]
    async fn definitions_and_runs_round_trip() {
        let store = WorkflowStore::in_memory().unwrap();
        let mut draft = WorkflowDraft::default();
        draft.name = "deploy".into();
        draft.scope = "demo".into();
        let definition = WorkflowDefinition::from_draft(draft);
        store.put_definition(&definition).await.unwrap();
        assert_eq!(
            store
                .get_definition(&definition.id)
                .await
                .unwrap()
                .unwrap()
                .name,
            "deploy"
        );
        let run = WorkflowRun::new(definition, factory_core::workflow::WorkflowActor::Owner);
        store.put_run(&run).await.unwrap();
        assert_eq!(store.active_runs().await.unwrap()[0].id, run.id);
    }

    /// One undecodable row is data, not an outage: `definitions`/`active_runs`
    /// skip it and keep serving everything else; only fetching that exact id
    /// directly is an error, and only for it.
    #[tokio::test]
    async fn a_malformed_definition_row_is_skipped_in_lists_and_errors_alone_when_fetched() {
        let store = WorkflowStore::in_memory().unwrap();
        let mut draft = WorkflowDraft::default();
        draft.name = "deploy".into();
        draft.scope = "demo".into();
        let good = WorkflowDefinition::from_draft(draft);
        store.put_definition(&good).await.unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO workflow_definitions (id, scope, updated_at, data) VALUES ('bad-id', 'demo', '2024-01-01T00:00:00Z', 'not json')",
                [],
            )
            .unwrap();
        }

        let listed = store.definitions(None).await.unwrap();
        assert_eq!(listed.len(), 1, "the malformed row is skipped, not fatal");
        assert_eq!(listed[0].id, good.id);
        assert_eq!(store.definitions(Some("demo")).await.unwrap().len(), 1);
        assert!(store.get_definition("bad-id").await.is_err());
        assert!(store.get_definition(&good.id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn a_malformed_run_row_is_skipped_in_lists_and_errors_alone_when_fetched() {
        let store = WorkflowStore::in_memory().unwrap();
        let mut draft = WorkflowDraft::default();
        draft.name = "deploy".into();
        draft.scope = "demo".into();
        let definition = WorkflowDefinition::from_draft(draft);
        let good = WorkflowRun::new(definition, factory_core::workflow::WorkflowActor::Owner);
        store.put_run(&good).await.unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO workflow_runs (id, workflow_id, scope, status, updated_at, data) VALUES ('bad-run', 'wf', 'demo', 'running', '2024-01-01T00:00:00Z', 'not json')",
                [],
            )
            .unwrap();
        }

        assert_eq!(store.active_runs().await.unwrap().len(), 1);
        let listed = store.runs(None, None, 50).await.unwrap();
        assert_eq!(listed.len(), 1, "the malformed row is skipped, not fatal");
        assert_eq!(listed[0].id, good.id);
        assert!(store.get_run("bad-run").await.is_err());
        assert!(store.get_run(&good.id).await.unwrap().is_some());
    }
}
