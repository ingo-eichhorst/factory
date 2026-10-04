//! Owner-specific current expiry observations; not an authored catalogue.
//! Failed probes retain earlier expiry evidence without inventing freshness.

use factory_kernel::{ExpiryObservation, FactoryError, Result};
use rusqlite::{params, Connection};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

const TABLE: &str = "infrastructure_expiries";

fn error(error: impl std::fmt::Display) -> FactoryError {
    FactoryError::adapter("renewal metadata sqlite", error.to_string())
}

#[derive(Clone)]
pub struct ObservationStore {
    conn: Arc<Mutex<Connection>>,
}
impl ObservationStore {
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_connection(Connection::open(path).map_err(error)?)
    }
    pub fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory().map_err(error)?)
    }
    fn from_connection(conn: Connection) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(error)?;
        conn.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS {} (id TEXT PRIMARY KEY, data TEXT NOT NULL)",
            TABLE
        ))
        .map_err(error)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }
    pub async fn all(&self) -> Result<Vec<ExpiryObservation>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().map_err(error)?;
            let mut statement = conn
                .prepare(&format!("SELECT data FROM {} ORDER BY id", TABLE))
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
    pub async fn replace(
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
                    .prepare(&format!("SELECT id, data FROM {}", TABLE))
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
            tx.execute(&format!("DELETE FROM {}", TABLE), [])
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
                    &format!("INSERT INTO {} (id, data) VALUES (?1, ?2)", TABLE),
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

// OWNER CONTRACTS
#[cfg(test)]
mod owner_contracts {
    use super::*;
    use factory_kernel::{DateBasis, DateKind, DateSource};

    fn observation(id: &str) -> ExpiryObservation {
        let at = "2026-09-25T12:00:00Z"
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap();
        ExpiryObservation {
            id: id.into(),
            name: id.into(),
            kind: DateKind::Other,
            scope: Some("demo".into()),
            expires_at: Some(at + chrono::Duration::days(30)),
            no_expiry: false,
            basis: DateBasis::Observed,
            source: DateSource::Tls,
            detail: "expiry metadata".into(),
            observed_at: Some(at),
            attempted_at: at,
            issue: None,
            affects: Vec::new(),
            lead_seconds: 86400,
            renew: "review expiry".into(),
            owner: "owner".into(),
        }
    }

    #[tokio::test]
    async fn own_cache_preserves_failed_and_incomplete_evidence_without_freshening_it() {
        let store = ObservationStore::in_memory().unwrap();
        {
            let conn = store.conn.lock().unwrap();
            let mut query = conn
                .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
                .unwrap();
            let names = query
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(names, [TABLE]);
        }
        let first = observation("a");
        let retired = observation("b");
        store
            .replace(vec![retired.clone(), first.clone()], true)
            .await
            .unwrap();
        assert_eq!(store.all().await.unwrap(), [first.clone(), retired.clone()]);
        let mut failed = first.clone();
        failed.expires_at = None;
        failed.observed_at = None;
        failed.basis = DateBasis::Unknown;
        failed.detail.clear();
        failed.attempted_at += chrono::Duration::hours(1);
        failed.issue = Some("probe unavailable".into());
        store.replace(vec![failed.clone()], false).await.unwrap();
        let retained = store.all().await.unwrap();
        assert_eq!(retained.len(), 2);
        assert_eq!(retained[0].expires_at, first.expires_at);
        assert_eq!(retained[0].observed_at, first.observed_at);
        assert_eq!(retained[0].basis, first.basis);
        assert_eq!(retained[0].detail, first.detail);
        assert_eq!(retained[0].attempted_at, failed.attempted_at);
        assert_eq!(retained[0].issue, failed.issue);
        assert_eq!(retained[1].expires_at, retired.expires_at);
        assert_eq!(retained[1].observed_at, retired.observed_at);
        assert!(retained[1]
            .issue
            .as_ref()
            .unwrap()
            .contains("discovery unavailable"));
        store.replace(vec![failed], true).await.unwrap();
        assert_eq!(
            store.all().await.unwrap().len(),
            1,
            "complete discovery retires absent sources"
        );
        store.replace(Vec::new(), true).await.unwrap();
        assert!(store.all().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn rejected_snapshot_rolls_back_the_whole_cache_update() {
        let store = ObservationStore::in_memory().unwrap();
        let first = observation("a");
        store.replace(vec![first.clone()], true).await.unwrap();
        let duplicate = observation("b");
        assert!(store
            .replace(vec![duplicate.clone(), duplicate], true)
            .await
            .is_err());
        assert_eq!(store.all().await.unwrap(), [first]);
    }
}
