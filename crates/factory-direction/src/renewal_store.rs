//! L6 renewal push attempt receipts. Claims precede delivery, so an interrupted
//! attempt stays unknown, never a fabricated delivery or restart duplicate.

use factory_kernel::{FactoryError, Result};
use rusqlite::{params, Connection};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

fn error(error: impl std::fmt::Display) -> FactoryError {
    FactoryError::adapter("renewal metadata sqlite", error.to_string())
}

#[derive(Clone)]
pub struct AlertStore {
    conn: Arc<Mutex<Connection>>,
}
impl AlertStore {
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_connection(Connection::open(path).map_err(error)?)
    }
    pub fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory().map_err(error)?)
    }
    fn from_connection(conn: Connection) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(error)?;
        conn.execute_batch("CREATE TABLE IF NOT EXISTS renewal_push_receipts (identity TEXT PRIMARY KEY, attempted_at TEXT NOT NULL, outcome TEXT NOT NULL)").map_err(error)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }
    /// Claim before the external effect. A crash/timeout is explicitly an
    /// unknown attempt, never an invented delivery or a restart duplicate.
    pub async fn claim(&self, identity: String, at: chrono::DateTime<chrono::Utc>) -> Result<bool> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().map_err(error)?;
            Ok(conn
                .execute(
                    "INSERT OR IGNORE INTO renewal_push_receipts VALUES (?1, ?2, 'attempted')",
                    params![identity, at.to_rfc3339()],
                )
                .map_err(error)?
                == 1)
        })
        .await
        .map_err(error)?
    }
    pub async fn finish(&self, identity: String, delivered: bool) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            conn.lock()
                .map_err(error)?
                .execute(
                    "UPDATE renewal_push_receipts SET outcome=?2 WHERE identity=?1",
                    params![identity, if delivered { "delivered" } else { "failed" }],
                )
                .map_err(error)?;
            Ok(())
        })
        .await
        .map_err(error)?
    }
}

// OWNER CONTRACTS
#[cfg(test)]
mod owner_contracts {
    use super::*;
    #[tokio::test]
    async fn interrupted_claim_stays_attempted_across_restart_and_cannot_be_reclaimed() {
        let path = std::env::temp_dir().join(format!(
            "factory-renewal-owner-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let at = chrono::Utc::now();
        let store = AlertStore::open(&path).unwrap();
        assert!(store.claim("unknown".into(), at).await.unwrap());
        assert!(store.claim("failed".into(), at).await.unwrap());
        store.finish("failed".into(), false).await.unwrap();
        assert!(store.claim("delivered".into(), at).await.unwrap());
        store.finish("delivered".into(), true).await.unwrap();
        drop(store);
        let reopened = AlertStore::open(&path).unwrap();
        for id in ["unknown", "failed", "delivered"] {
            assert!(!reopened.claim(id.into(), at).await.unwrap());
        }
        {
            let conn = reopened.conn.lock().unwrap();
            let mut query = conn
                .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
                .unwrap();
            let names = query
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(names, ["renewal_push_receipts"]);
            let mut query = conn
                .prepare("SELECT identity, outcome FROM renewal_push_receipts ORDER BY identity")
                .unwrap();
            let rows = query
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(
                rows,
                [
                    ("delivered".into(), "delivered".into()),
                    ("failed".into(), "failed".into()),
                    ("unknown".into(), "attempted".into())
                ]
            );
        }
        drop(reopened);
        std::fs::remove_file(path).unwrap();
    }
}
