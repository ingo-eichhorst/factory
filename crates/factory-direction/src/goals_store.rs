//! Where check-ins live: one table, `goals_checkins`, append-only -- no
//! `UPDATE` and no `DELETE` anywhere below, the same rule `policies/store.rs`
//! holds for attestations. Recording a check-in inserts one row; nothing
//! ever revises or removes it, so a key result's confidence history is
//! exactly what was said, when.
//!
//! Follows `policies/store.rs` exactly: its own `CREATE TABLE IF NOT EXISTS`,
//! no `SCHEMA_VERSION` check, `open`/`in_memory`, one connection behind a
//! mutex, blocking `rusqlite` calls moved to `spawn_blocking` via `with_conn`.
//!
//! The catalogues themselves -- `.factory/goals/direction.yaml` and
//! `.factory/goals/<cycle-id>.yaml` -- are authored content read straight off
//! disk by `crate::goals::load`; nothing about them lives here. This
//! table holds only the one piece of new state the issue introduces: the
//! audit trail of who checked a manual key result in to what, and when.

use crate::goals::CheckIn;
use factory_kernel::{FactoryError, Result};
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::{Arc, Mutex};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS goals_checkins (
    id TEXT PRIMARY KEY,
    objective TEXT NOT NULL,
    kr TEXT NOT NULL,
    at TEXT NOT NULL,
    data TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS goals_checkins_kr ON goals_checkins(objective, kr, at);
"#;

fn error(error: impl std::fmt::Display) -> FactoryError {
    FactoryError::adapter("goals sqlite", error.to_string())
}

#[derive(Clone)]
pub struct GoalsStore {
    conn: Arc<Mutex<Connection>>,
}

impl GoalsStore {
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

    /// Insert one check-in row. Never fails on a duplicate value or
    /// confidence -- validation (KR exists and is manual, confidence
    /// `0..=10`, a finite value) happens before this is ever called
    /// (`goals_service::Service::checkin`), so this is a pure append.
    pub async fn append(&self, checkin: &CheckIn) -> Result<()> {
        let checkin = checkin.clone();
        self.with_conn(move |conn| {
            let data = serde_json::to_string(&checkin).map_err(error)?;
            conn.execute(
                "INSERT INTO goals_checkins (id, objective, kr, at, data) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![checkin.id, checkin.kr.objective, checkin.kr.kr, checkin.at.to_rfc3339(), data],
            )
            .map_err(error)?;
            Ok(())
        })
        .await
    }

    /// Every check-in ever recorded, most recent first. Small enough -- an
    /// instance's whole check-in history, not one key result's worth -- that
    /// filtering by key result is left to the caller, the same trade
    /// `PolicyStore::all` makes for attestations.
    pub async fn all(&self) -> Result<Vec<CheckIn>> {
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare("SELECT id, data FROM goals_checkins ORDER BY at DESC")
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
                .filter_map(|(id, json)| match serde_json::from_str::<CheckIn>(&json) {
                    Ok(c) => Some(c),
                    Err(err) => {
                        tracing::warn!(id, "skipping malformed goals_checkins row: {err}");
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
    use crate::goals::KrRef;
    use chrono::Utc;

    fn checkin(id: &str, objective: &str, kr: &str, value: f64) -> CheckIn {
        CheckIn {
            id: id.to_string(),
            kr: KrRef::new(objective, kr),
            value,
            confidence: 7,
            note: None,
            by: "owner".to_string(),
            at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn a_checkin_round_trips_and_all_lists_it() {
        let store = GoalsStore::in_memory().unwrap();
        let c = checkin("c1", "ship-compliant", "dpa-signed", 3.0);
        store.append(&c).await.unwrap();
        assert_eq!(store.all().await.unwrap(), vec![c]);
    }

    #[tokio::test]
    async fn all_is_most_recent_first_and_covers_every_key_result() {
        let store = GoalsStore::in_memory().unwrap();
        let a = checkin("c1", "ship-compliant", "dpa-signed", 1.0);
        store.append(&a).await.unwrap();
        // Distinguish ordering deterministically -- `Utc::now()` twice in
        // quick succession is not reliably different at second resolution.
        let mut b = checkin("c2", "ship-compliant", "dpa-signed", 2.0);
        b.at = a.at + chrono::Duration::seconds(1);
        store.append(&b).await.unwrap();
        let other = checkin("c3", "raise-quality", "other-kr", 9.0);
        store.append(&other).await.unwrap();

        let all = store.all().await.unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].id, "c2", "most recent first");

        let for_kr: Vec<&CheckIn> = all
            .iter()
            .filter(|c| c.kr == KrRef::new("ship-compliant", "dpa-signed"))
            .collect();
        assert_eq!(for_kr.len(), 2);
    }

    #[tokio::test]
    async fn a_malformed_row_is_skipped_not_fatal() {
        let store = GoalsStore::in_memory().unwrap();
        let good = checkin("c1", "ship-compliant", "dpa-signed", 1.0);
        store.append(&good).await.unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO goals_checkins (id, objective, kr, at, data) VALUES ('bad', 'a', 'b', '2024-01-01T00:00:00Z', 'not json')",
                [],
            )
            .unwrap();
        }
        let all = store.all().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "c1");
    }
}
