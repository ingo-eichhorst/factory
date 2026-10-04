//! Owner-specific expiry caches and L6 push receipts in the instance database.
//! No authored content or credential is read here. Failed probes retain the
//! last known date; a cache never turns a failed probe into a fresh success.
use factory_core::error::{FactoryError, Result};
use factory_kernel::{ExpiryObservation, L1, L2};
use rusqlite::{params, Connection};
use std::{
    marker::PhantomData,
    path::Path,
    sync::{Arc, Mutex},
};

fn error(error: impl std::fmt::Display) -> FactoryError {
    FactoryError::adapter("renewal metadata sqlite", error.to_string())
}

pub(crate) trait ExpiryOwner: Send + Sync + 'static {
    const TABLE: &'static str;
}
impl ExpiryOwner for L1 {
    const TABLE: &'static str = "infrastructure_expiries";
}
impl ExpiryOwner for L2 {
    const TABLE: &'static str = "credential_expiries";
}

pub(crate) struct ObservationStore<L: ExpiryOwner> {
    conn: Arc<Mutex<Connection>>,
    owner: PhantomData<L>,
}
impl<L: ExpiryOwner> Clone for ObservationStore<L> {
    fn clone(&self) -> Self {
        Self {
            conn: self.conn.clone(),
            owner: PhantomData,
        }
    }
}
impl<L: ExpiryOwner> ObservationStore<L> {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        Self::from_connection(Connection::open(path).map_err(error)?)
    }
    pub(crate) fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory().map_err(error)?)
    }
    fn from_connection(conn: Connection) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(error)?;
        conn.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS {} (id TEXT PRIMARY KEY, data TEXT NOT NULL)",
            L::TABLE
        ))
        .map_err(error)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            owner: PhantomData,
        })
    }
    pub(crate) async fn all(&self) -> Result<Vec<ExpiryObservation>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().map_err(error)?;
            let mut statement = conn
                .prepare(&format!("SELECT data FROM {} ORDER BY id", L::TABLE))
                .map_err(error)?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(error)?;
            rows.map(|row| serde_json::from_str(&row.map_err(error)?).map_err(error))
                .collect()
        })
        .await
        .map_err(error)?
    }
    /// Atomic current-source snapshot. Retired sources leave this regenerable
    /// cache; an active failed source keeps its earlier immutable expiry.
    pub(crate) async fn replace(
        &self,
        mut observations: Vec<ExpiryObservation>,
        discovery_complete: bool,
    ) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = conn.lock().map_err(error)?;
            let tx = conn.transaction().map_err(error)?;
            let mut previous = std::collections::BTreeMap::new();
            {
                let mut statement = tx
                    .prepare(&format!("SELECT id, data FROM {}", L::TABLE))
                    .map_err(error)?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(error)?;
                for row in rows {
                    let (id, data) = row.map_err(error)?;
                    previous.insert(
                        id,
                        serde_json::from_str::<ExpiryObservation>(&data).map_err(error)?,
                    );
                }
            }
            if !discovery_complete {
                let ids: std::collections::BTreeSet<_> =
                    observations.iter().map(|item| item.id.clone()).collect();
                for old in previous.values().filter(|old| !ids.contains(&old.id)) {
                    let mut stale = old.clone();
                    stale.attempted_at = chrono::Utc::now();
                    stale.issue =
                        Some("metadata discovery unavailable; retaining last-known expiry".into());
                    observations.push(stale);
                }
            }
            tx.execute(&format!("DELETE FROM {}", L::TABLE), [])
                .map_err(error)?;
            for mut observation in observations {
                if observation.issue.is_some()
                    || (observation.expires_at.is_none() && !observation.no_expiry)
                {
                    if let Some(old) = previous.get(&observation.id) {
                        if observation.issue.is_none()
                            && (old.expires_at.is_some() || old.no_expiry)
                        {
                            observation.issue = Some(
                                "source no longer reports an expiry; retaining last-known expiry"
                                    .into(),
                            );
                        }
                        observation.expires_at = old.expires_at;
                        observation.no_expiry = old.no_expiry;
                        observation.observed_at = old.observed_at;
                        observation.basis = old.basis;
                        observation.detail = old.detail.clone();
                    }
                }
                let json = serde_json::to_string(&observation).map_err(error)?;
                tx.execute(
                    &format!("INSERT INTO {} (id, data) VALUES (?1, ?2)", L::TABLE),
                    params![observation.id, json],
                )
                .map_err(error)?;
            }
            tx.commit().map_err(error)
        })
        .await
        .map_err(error)?
    }
}

#[derive(Clone)]
pub(crate) struct AlertStore {
    conn: Arc<Mutex<Connection>>,
}
impl AlertStore {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        Self::from_connection(Connection::open(path).map_err(error)?)
    }
    pub(crate) fn in_memory() -> Result<Self> {
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
    pub(crate) async fn claim(
        &self,
        identity: String,
        at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool> {
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
    pub(crate) async fn finish(&self, identity: String, delivered: bool) -> Result<()> {
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
