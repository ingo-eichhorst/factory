//! Where bench runs live: `bench_runs` (one row per run, the snapshot and its
//! status) and `bench_attempts` (one row per case × agent × attempt).
//! Follows `workflows/store.rs` exactly -- its own `CREATE TABLE IF NOT
//! EXISTS`, no `SCHEMA_VERSION` check, so a mismatch in `store_sqlite.rs`'s
//! own version drops the four *task* tables and leaves these standing.
//!
//! `bench_runs.data` never carries the attempts themselves -- `get_run` and
//! `runs` always reassemble `.attempts` from `bench_attempts`, which is the
//! only table anything writes an attempt's progress to. That split is why
//! judging one attempt is a small write rather than rewriting a whole run's
//! worth of JSON on every gate that finishes.

use factory_core::bench::{BenchAttempt, BenchRun, BenchRunStatus};
use factory_core::error::{FactoryError, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::{Arc, Mutex};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS bench_runs (
    id TEXT PRIMARY KEY,
    dataset TEXT NOT NULL,
    status TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    data TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS bench_runs_dataset ON bench_runs(dataset, updated_at);
CREATE INDEX IF NOT EXISTS bench_runs_status ON bench_runs(status, updated_at);
CREATE TABLE IF NOT EXISTS bench_attempts (
    id TEXT PRIMARY KEY,
    bench_run_id TEXT NOT NULL,
    data TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS bench_attempts_run ON bench_attempts(bench_run_id);
"#;

fn error(error: impl std::fmt::Display) -> FactoryError {
    FactoryError::adapter("bench sqlite", error.to_string())
}

fn decode<T: serde::de::DeserializeOwned>(json: String) -> Result<T> {
    serde_json::from_str(&json).map_err(error)
}

/// Decode every row, skipping (and naming) whichever ones do not -- a
/// corrupted row is data, not an outage, the same rule `workflows/store.rs`
/// lives by.
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
pub struct BenchStore {
    conn: Arc<Mutex<Connection>>,
}

impl BenchStore {
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
            let mut guard = conn.lock().map_err(|_| error("connection mutex poisoned"))?;
            f(&mut guard)
        })
        .await
        .map_err(error)?
    }

    /// Write the run's own metadata -- everything but `attempts`, which
    /// lives in `bench_attempts` and is never trusted from this column.
    pub async fn put_run(&self, run: &BenchRun) -> Result<()> {
        let mut meta = run.clone();
        meta.attempts = Vec::new();
        self.with_conn(move |conn| {
            let data = serde_json::to_string(&meta).map_err(error)?;
            let status = match meta.status {
                BenchRunStatus::Running => "running",
                BenchRunStatus::Done => "done",
                BenchRunStatus::Cancelled => "cancelled",
            };
            conn.execute(
                "INSERT INTO bench_runs (id, dataset, status, updated_at, data) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET dataset=excluded.dataset, status=excluded.status, updated_at=excluded.updated_at, data=excluded.data",
                params![meta.id, meta.dataset, status, chrono::Utc::now().to_rfc3339(), data],
            ).map_err(error)?;
            Ok(())
        }).await
    }

    pub async fn put_attempt(&self, run_id: &str, attempt: &BenchAttempt) -> Result<()> {
        let run_id = run_id.to_string();
        let attempt = attempt.clone();
        self.with_conn(move |conn| {
            let data = serde_json::to_string(&attempt).map_err(error)?;
            conn.execute(
                "INSERT INTO bench_attempts (id, bench_run_id, data) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET data=excluded.data",
                params![attempt.id, run_id, data],
            )
            .map_err(error)?;
            Ok(())
        })
        .await
    }

    /// Every attempt of one run, ordered by case then agent then attempt
    /// number so the same run always reassembles in the same order.
    pub async fn attempts(&self, run_id: &str) -> Result<Vec<BenchAttempt>> {
        let run_id = run_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare("SELECT id, data FROM bench_attempts WHERE bench_run_id = ?1")
                .map_err(error)?;
            let rows = stmt
                .query_map([&run_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(error)?;
            let mut attempts: Vec<BenchAttempt> = decode_all(rows, "bench_attempts");
            attempts.sort_by(|a, b| {
                a.case_id
                    .cmp(&b.case_id)
                    .then_with(|| a.agent.cmp(&b.agent))
                    .then_with(|| a.attempt.cmp(&b.attempt))
            });
            Ok(attempts)
        })
        .await
    }

    /// The run with its attempts reassembled -- the whole, honest answer,
    /// never trusting whatever `attempts` happened to be embedded when the
    /// meta row was last written.
    pub async fn get_run(&self, id: &str) -> Result<Option<BenchRun>> {
        let id_owned = id.to_string();
        let meta: Option<BenchRun> = self
            .with_conn(move |conn| {
                let json: Option<String> = conn
                    .query_row(
                        "SELECT data FROM bench_runs WHERE id = ?1",
                        [&id_owned],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(error)?;
                json.map(decode).transpose()
            })
            .await?;
        let Some(mut run) = meta else { return Ok(None) };
        run.attempts = self.attempts(id).await?;
        Ok(Some(run))
    }

    /// Every run, most recently updated first, optionally narrowed to one
    /// dataset. Attempts are reassembled for each, the same as `get_run`.
    pub async fn runs(&self, dataset: Option<&str>, limit: u32) -> Result<Vec<BenchRun>> {
        let dataset = dataset.map(str::to_string);
        let metas: Vec<BenchRun> = self
            .with_conn(move |conn| {
                let rows: Vec<(String, String)> = match dataset.as_deref() {
                    Some(dataset) => {
                        let mut stmt = conn
                            .prepare(
                                "SELECT id, data FROM bench_runs WHERE dataset = ?1 ORDER BY updated_at DESC LIMIT ?2",
                            )
                            .map_err(error)?;
                        let mapped = stmt
                            .query_map(params![dataset, limit], |row| {
                                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                            })
                            .map_err(error)?
                            .collect::<std::result::Result<Vec<_>, _>>()
                            .map_err(error)?;
                        mapped
                    }
                    None => {
                        let mut stmt = conn
                            .prepare("SELECT id, data FROM bench_runs ORDER BY updated_at DESC LIMIT ?1")
                            .map_err(error)?;
                        let mapped = stmt
                            .query_map(params![limit], |row| {
                                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                            })
                            .map_err(error)?
                            .collect::<std::result::Result<Vec<_>, _>>()
                            .map_err(error)?;
                        mapped
                    }
                };
                Ok(decode_all::<BenchRun>(rows, "bench_runs"))
            })
            .await?;
        let mut out = Vec::with_capacity(metas.len());
        for mut meta in metas {
            meta.attempts = self.attempts(&meta.id).await?;
            out.push(meta);
        }
        Ok(out)
    }

    /// Every run still `running`, for restart recovery.
    pub async fn active_runs(&self) -> Result<Vec<BenchRun>> {
        let metas: Vec<BenchRun> = self
            .with_conn(move |conn| {
                let mut stmt = conn
                    .prepare("SELECT id, data FROM bench_runs WHERE status = 'running' ORDER BY updated_at")
                    .map_err(error)?;
                let rows = stmt
                    .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
                    .map_err(error)?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(error)?;
                Ok(decode_all::<BenchRun>(rows, "bench_runs"))
            })
            .await?;
        let mut out = Vec::with_capacity(metas.len());
        for mut meta in metas {
            meta.attempts = self.attempts(&meta.id).await?;
            out.push(meta);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_core::bench::BenchAttempt;
    use std::collections::BTreeMap;

    fn run(id: &str) -> BenchRun {
        BenchRun {
            id: id.to_string(),
            dataset: "demo".into(),
            dataset_revision: 1,
            cases: Vec::new(),
            case_bases: BTreeMap::new(),
            agents: vec!["builder".into()],
            attempts_per_case: 1,
            concurrency: 1,
            status: BenchRunStatus::Running,
            attempts: Vec::new(),
            started_at: chrono::Utc::now(),
            ended_at: None,
        }
    }

    #[tokio::test]
    async fn a_run_and_its_attempts_round_trip() {
        let store = BenchStore::in_memory().unwrap();
        let run = run("r1");
        store.put_run(&run).await.unwrap();
        let attempt = BenchAttempt::pending("a1".into(), "case-a".into(), "builder".into(), 1);
        store.put_attempt(&run.id, &attempt).await.unwrap();

        let loaded = store.get_run("r1").await.unwrap().unwrap();
        assert_eq!(loaded.attempts.len(), 1);
        assert_eq!(loaded.attempts[0].id, "a1");
    }

    #[tokio::test]
    async fn put_run_never_trusts_the_embedded_attempts_field() {
        let store = BenchStore::in_memory().unwrap();
        let mut run = run("r1");
        // A caller that (wrongly) hands `put_run` a run carrying attempts
        // must not have those embedded attempts silently become the source
        // of truth -- only `put_attempt` may write to `bench_attempts`.
        run.attempts = vec![BenchAttempt::pending("phantom".into(), "c".into(), "a".into(), 1)];
        store.put_run(&run).await.unwrap();
        let loaded = store.get_run("r1").await.unwrap().unwrap();
        assert!(loaded.attempts.is_empty(), "{:?}", loaded.attempts);
    }

    #[tokio::test]
    async fn active_runs_lists_only_running_ones() {
        let store = BenchStore::in_memory().unwrap();
        let mut done = run("done-run");
        done.status = BenchRunStatus::Done;
        store.put_run(&done).await.unwrap();
        store.put_run(&run("running-run")).await.unwrap();

        let active = store.active_runs().await.unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, "running-run");
    }

    #[tokio::test]
    async fn runs_narrows_by_dataset() {
        let store = BenchStore::in_memory().unwrap();
        let mut other = run("r-other");
        other.dataset = "other".into();
        store.put_run(&run("r-demo")).await.unwrap();
        store.put_run(&other).await.unwrap();

        let narrowed = store.runs(Some("demo"), 10).await.unwrap();
        assert_eq!(narrowed.len(), 1);
        assert_eq!(narrowed[0].id, "r-demo");
    }

    /// The acceptance criterion, proved directly: these tables carry their
    /// own `CREATE TABLE IF NOT EXISTS` and no version check, so a bump to
    /// `store_sqlite.rs`'s `SCHEMA_VERSION` -- which drops the four *task*
    /// tables it owns -- must never touch a bench run sitting in the same
    /// database file.
    #[tokio::test]
    async fn bench_tables_survive_a_task_store_schema_version_mismatch() {
        let dir = std::env::temp_dir().join(format!("factory-bench-schema-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("factory.sqlite");

        // Open as the task store first, so its own schema (and its
        // user_version) lands in this database file -- then the bench store
        // opens the very same file the way the daemon does.
        let _task_store = factory_plugins::SqliteStore::open(&db_path).unwrap();
        let bench_store = BenchStore::open(&db_path).unwrap();
        bench_store.put_run(&run("r1")).await.unwrap();
        drop(bench_store);
        drop(_task_store);

        // A "different version" task store: bump `user_version` behind its
        // back, forcing store_sqlite.rs's own mismatch-drop path on its next
        // open, without needing a second real schema version to exist.
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.pragma_update(None, "user_version", 999i64).unwrap();
        }
        let _task_store_again = factory_plugins::SqliteStore::open(&db_path).unwrap();

        let bench_store_again = BenchStore::open(&db_path).unwrap();
        let survived = bench_store_again.get_run("r1").await.unwrap();
        assert!(survived.is_some(), "the bench run must survive the task store's schema drop");

        std::fs::remove_dir_all(&dir).ok();
    }
}
