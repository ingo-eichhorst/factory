//! What happened to backups: one table, `backup_events`, append-only -- no
//! `UPDATE` and no `DELETE` anywhere below, the same rule `goals/store.rs`
//! and `policies/store.rs` hold. A backup taken, a backup that failed and a
//! verification each insert one row; nothing ever revises one.
//!
//! Follows `goals/store.rs` exactly: its own `CREATE TABLE IF NOT EXISTS`, no
//! `SCHEMA_VERSION` check, `open`/`in_memory`, one connection behind a mutex,
//! blocking `rusqlite` calls moved to `spawn_blocking` via `with_conn`.
//!
//! The archives themselves are the destination's; this is only the record of
//! what this daemon did with them. The listing on the page is the
//! destination's own directory, joined with these rows by archive name --
//! an archive with no row here (a database restored from an older backup,
//! a file copied in by hand) is still listed, just without a file count.

use crate::backup::{BackupFailure, Snapshot, Verification};
use factory_kernel::error::{FactoryError, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS backup_events (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    snapshot TEXT,
    at TEXT NOT NULL,
    data TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS backup_events_at ON backup_events(at);
"#;

fn error(error: impl std::fmt::Display) -> FactoryError {
    FactoryError::adapter("backup sqlite", error.to_string())
}

/// One row, as recorded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Recorded {
    Completed { snapshot: Snapshot },
    Failed { failure: BackupFailure },
    Verified { verification: Verification },
}

impl Recorded {
    fn kind(&self) -> &'static str {
        match self {
            Self::Completed { .. } => "completed",
            Self::Failed { .. } => "failed",
            Self::Verified { .. } => "verified",
        }
    }

    fn snapshot(&self) -> Option<&str> {
        match self {
            Self::Completed { snapshot } => Some(&snapshot.name),
            Self::Failed { .. } => None,
            Self::Verified { verification } => Some(&verification.snapshot),
        }
    }

    pub fn at(&self) -> chrono::DateTime<chrono::Utc> {
        match self {
            Self::Completed { snapshot } => snapshot.at,
            Self::Failed { failure } => failure.at,
            Self::Verified { verification } => verification.at,
        }
    }
}

#[derive(Clone)]
pub struct BackupStore {
    conn: Arc<Mutex<Connection>>,
}

impl BackupStore {
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

    pub async fn append(&self, recorded: Recorded) -> Result<()> {
        self.with_conn(move |conn| {
            let data = serde_json::to_string(&recorded).map_err(error)?;
            conn.execute(
                "INSERT INTO backup_events (id, kind, snapshot, at, data) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    uuid::Uuid::new_v4().to_string(),
                    recorded.kind(),
                    recorded.snapshot(),
                    recorded.at().to_rfc3339(),
                    data
                ],
            )
            .map_err(error)?;
            Ok(())
        })
        .await
    }

    /// Every row, newest first. An instance takes one backup a night; a
    /// decade of them is a few thousand small rows, so the page's joins are
    /// done by the caller rather than in SQL, the same trade
    /// `GoalsStore::all` makes.
    pub async fn all(&self) -> Result<Vec<Recorded>> {
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare("SELECT id, data FROM backup_events ORDER BY at DESC")
                .map_err(error)?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(error)?;
            Ok(rows
                .into_iter()
                .filter_map(|(id, json)| match serde_json::from_str::<Recorded>(&json) {
                    Ok(r) => Some(r),
                    Err(err) => {
                        tracing::warn!(id, "skipping malformed backup_events row: {err}");
                        None
                    }
                })
                .collect())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::BackupTrigger;
    use chrono::{Duration, Utc};

    #[tokio::test]
    async fn rows_come_back_newest_first_whatever_their_kind() {
        let store = BackupStore::in_memory().unwrap();
        let now = Utc::now();
        store
            .append(Recorded::Failed {
                failure: BackupFailure {
                    at: now - Duration::hours(2),
                    trigger: BackupTrigger::Schedule,
                    reason: "disk".into(),
                },
            })
            .await
            .unwrap();
        store
            .append(Recorded::Verified {
                verification: Verification {
                    snapshot: "s".into(),
                    at: now,
                    by: "owner".into(),
                    ok: true,
                    checks: vec![],
                    duration_ms: 1,
                },
            })
            .await
            .unwrap();
        let all = store.all().await.unwrap();
        assert_eq!(all.len(), 2);
        assert!(matches!(all[0], Recorded::Verified { .. }));
        assert!(matches!(all[1], Recorded::Failed { .. }));
    }
}

// OWNER CONTRACTS
#[cfg(test)]
mod owner_contracts {
    use super::*;
    #[test]
    fn initializes_only_backup_events() {
        let store = BackupStore::in_memory().unwrap();
        let conn = store.conn.lock().unwrap();
        let mut query = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap();
        let names = query
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(names, ["backup_events"]);
    }
}
