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

    /// Merge only the cleanup receipt into the newest row, never overwrite a
    /// concurrent node-state update with the sweep's earlier snapshot.
    pub async fn mark_workspace_cleanup(&self, id: &str) -> Result<Option<WorkflowRun>> {
        let id = id.to_string();
        self.with_conn(move |conn| {
            let tx = conn.transaction().map_err(error)?;
            let json: Option<String> = tx.query_row("SELECT data FROM workflow_runs WHERE id = ?1", [&id], |row| row.get(0))
                .optional().map_err(error)?;
            let Some(json) = json else { return Ok(None); };
            let mut run: WorkflowRun = decode(json)?;
            if !run.status.is_terminal() { return Ok(None); }
            let Some(integration) = run.integration.as_mut() else { return Ok(None); };
            if integration.cleanup_complete { return Ok(None); }
            integration.cleanup_complete = true;
            tx.execute("UPDATE workflow_runs SET data = ?2 WHERE id = ?1", params![id, serde_json::to_string(&run).map_err(error)?]).map_err(error)?;
            tx.commit().map_err(error)?;
            Ok(Some(run))
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

    /// Bounded history of one intent label, not the last N unrelated runs.
    pub async fn tagged_runs(&self, label: &str, scope: Option<&str>, limit: u32) -> Result<Vec<WorkflowRun>> {
        let path = format!("$.task.labels.\"{label}\"");
        let scope = scope.map(str::to_string);
        self.with_conn(move |conn| {
            let mut statement = conn.prepare(
                "SELECT id, data FROM workflow_runs WHERE (?1 IS NULL OR scope = ?1) \
                 AND EXISTS (SELECT 1 FROM json_each(workflow_runs.data, '$.definition.nodes') AS node \
                 WHERE json_extract(node.value, ?2) IS NOT NULL) ORDER BY updated_at DESC, id LIMIT ?3"
            ).map_err(error)?;
            let rows = statement.query_map(params![scope, path, limit.min(200)], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            }).map_err(error)?.collect::<std::result::Result<Vec<_>, _>>().map_err(error)?;
            Ok(decode_all(rows, "workflow_runs"))
        }).await
    }

    /// The most recently touched terminal runs, for one-off startup
    /// reconciliation of a node overlay a missed event or an older, buggier
    /// build left stale (see B2/R11) -- bounded rather than exhaustive: a
    /// `done` run from months ago is not worth reading on every restart, but
    /// one from a recent crash is.
    pub async fn recent_terminal_runs(&self, limit: u32) -> Result<Vec<WorkflowRun>> {
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, data FROM workflow_runs WHERE status!='running' ORDER BY updated_at DESC LIMIT ?1",
                )
                .map_err(error)?;
            let rows = stmt
                .query_map([limit], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
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
    use factory_core::task::NewTask;
    use factory_core::workflow::{
        CanvasPoint, SendBack, WorkflowActor, WorkflowDraft, WorkflowEdge, WorkflowExit,
        WorkflowNode, WorkflowNodeKind, WorkflowNodeStatus,
    };

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

    #[tokio::test]
    async fn a_legacy_stored_run_mid_rework_loads_and_can_take_its_next_round() {
        let store = WorkflowStore::in_memory().unwrap();
        let task = |id: &str, exits| WorkflowNode {
            session: Default::default(),
            id: id.into(),
            position: CanvasPoint::default(),
            kind: WorkflowNodeKind::Task,
            task: NewTask {
                title: id.into(),
                scope: Some("demo".into()),
                ..Default::default()
            },
            gate: None,
            exits,
            expand: None,
        };
        let definition = WorkflowDefinition::from_draft(WorkflowDraft {
            name: "legacy review loop".into(),
            scope: "demo".into(),
            nodes: vec![
                task("implement", Vec::new()),
                task(
                    "review",
                    vec![WorkflowExit {
                        to: "implement".into(),
                        check: None,
                        agent: Some("fixable review findings".into()),
                        max_rounds: Some(3),
                    }],
                ),
            ],
            edges: vec![WorkflowEdge {
                id: "implement-review".into(),
                from: "implement".into(),
                to: "review".into(),
            }],
            ..Default::default()
        });
        let mut run = WorkflowRun::new(definition, WorkflowActor::Owner);
        for node in &mut run.nodes {
            node.status = WorkflowNodeStatus::Done;
            node.task_id = Some(format!("{}-task-1", node.node_id));
        }
        assert_eq!(
            run.send_back("review", "implement"),
            SendBack::Sent {
                round: 1,
                max_rounds: 3
            }
        );

        // This is the exact durable pre-#149 shape: the immutable definition
        // inside a run still says `rework`, while the node-run state already
        // records one used round.
        let mut json = serde_json::to_value(&run).unwrap();
        let review = json["definition"]["nodes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|node| node["id"] == "review")
            .unwrap();
        review.as_object_mut().unwrap().remove("exits");
        review["rework"] = serde_json::json!({ "to": "implement", "max_rounds": 3 });
        let data = serde_json::to_string(&json).unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO workflow_runs (id, workflow_id, scope, status, updated_at, data) VALUES (?1, ?2, ?3, 'running', ?4, ?5)",
                params![run.id, run.workflow_id, run.scope, run.updated_at.to_rfc3339(), data],
            )
            .unwrap();
        }

        let mut loaded = store.get_run(&run.id).await.unwrap().unwrap();
        let exit = &loaded
            .definition
            .nodes
            .iter()
            .find(|node| node.id == "review")
            .unwrap()
            .exits[0];
        assert_eq!(exit.to, "implement");
        assert!(exit.agent.is_some());
        assert_eq!(exit.max_rounds, Some(3));
        assert_eq!(
            loaded
                .nodes
                .iter()
                .find(|node| node.node_id == "review")
                .unwrap()
                .round,
            1
        );

        for node in &mut loaded.nodes {
            node.status = WorkflowNodeStatus::Done;
            node.task_id = Some(format!("{}-task-2", node.node_id));
        }
        assert_eq!(
            loaded.send_back("review", "implement"),
            SendBack::Sent {
                round: 2,
                max_rounds: 3
            },
            "the restored round budget continues instead of restarting"
        );
    }
}
