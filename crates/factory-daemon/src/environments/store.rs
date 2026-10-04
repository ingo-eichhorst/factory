//! What happened to the environments (`#185`): two tables in the instance
//! database.
//!
//! - `deploy_events` is append-only -- no `UPDATE` and no `DELETE` anywhere
//!   below, the rule `backup/store.rs` and `goals/store.rs` hold. A
//!   deployment starting, a deployment finishing and a release added by
//!   hand each insert one row, and a deployment is folded from its rows on
//!   read, so its history survives every release and every restart.
//! - `health_samples` is the one table here that is pruned: a check that
//!   runs every minute writes half a million rows a year, and nothing reads
//!   one older than the longest SLO window allows
//!   (`environments::SAMPLE_RETENTION_DAYS`). Samples are observations, not
//!   decisions, so dropping old ones loses no record of anything anyone did.
//!
//! Follows `backup/store.rs`: its own `CREATE TABLE IF NOT EXISTS`, one
//! connection behind a mutex, blocking calls moved to `spawn_blocking`.

use chrono::{DateTime, SecondsFormat, Utc};
use factory_core::environments::{DeployStatus, DeployVerification, Deployment, ReleaseFacts, Sample};
use factory_core::error::{FactoryError, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS deploy_events (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    deployment TEXT,
    at TEXT NOT NULL,
    data TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS deploy_events_deployment ON deploy_events(deployment);
CREATE TABLE IF NOT EXISTS health_samples (
    environment TEXT NOT NULL,
    check_name TEXT NOT NULL,
    at TEXT NOT NULL,
    ok INTEGER NOT NULL,
    latency_ms INTEGER NOT NULL,
    slow INTEGER NOT NULL DEFAULT 0,
    detail TEXT
);
CREATE INDEX IF NOT EXISTS health_samples_at ON health_samples(at);
CREATE INDEX IF NOT EXISTS health_samples_check ON health_samples(environment, check_name);
"#;

/// Upgrade old instances without reclassifying any historical sample.
/// Serialize schema checks with concurrent opens of the same database.
fn initialize(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(error)?;
    tx.execute_batch(SCHEMA).map_err(error)?;
    let has_slow: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('health_samples') WHERE name = 'slow')",
            [],
            |row| row.get(0),
        )
        .map_err(error)?;
    if !has_slow {
        tx.execute_batch("ALTER TABLE health_samples ADD COLUMN slow INTEGER NOT NULL DEFAULT 0")
            .map_err(error)?;
    }
    tx.commit().map_err(error)
}

fn error(error: impl std::fmt::Display) -> FactoryError {
    FactoryError::adapter("environments sqlite", error.to_string())
}

/// One fixed-width UTC form, so `at` compares as text in the order it
/// happened.
fn stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// How a deployment ended, as recorded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finished {
    pub status: DeployStatus,
    pub at: DateTime<Utc>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub verification: Option<DeployVerification>,
}

/// One row of `deploy_events`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Recorded {
    Started { deployment: Box<Deployment> },
    Finished { id: String, finished: Finished },
    ReleaseAdded { scope: String, release: ReleaseFacts, at: DateTime<Utc> },
}

impl Recorded {
    fn kind(&self) -> &'static str {
        match self {
            Self::Started { .. } => "started",
            Self::Finished { .. } => "finished",
            Self::ReleaseAdded { .. } => "release_added",
        }
    }

    fn deployment(&self) -> Option<&str> {
        match self {
            Self::Started { deployment } => Some(&deployment.id),
            Self::Finished { id, .. } => Some(id),
            Self::ReleaseAdded { .. } => None,
        }
    }

    fn at(&self) -> DateTime<Utc> {
        match self {
            Self::Started { deployment } => deployment.started_at,
            Self::Finished { finished, .. } => finished.at,
            Self::ReleaseAdded { at, .. } => *at,
        }
    }
}

#[derive(Clone)]
pub struct EnvironmentStore {
    conn: Arc<Mutex<Connection>>,
}

impl EnvironmentStore {
    pub fn open(path: &Path) -> Result<Self> {
        let mut conn = Connection::open(path).map_err(error)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .map_err(error)?;
        initialize(&mut conn)?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    pub fn in_memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory().map_err(error)?;
        initialize(&mut conn)?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
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

    async fn append(&self, recorded: Recorded) -> Result<()> {
        self.with_conn(move |conn| {
            let data = serde_json::to_string(&recorded).map_err(error)?;
            conn.execute(
                "INSERT INTO deploy_events (id, kind, deployment, at, data) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    uuid::Uuid::new_v4().to_string(),
                    recorded.kind(),
                    recorded.deployment(),
                    stamp(recorded.at()),
                    data
                ],
            )
            .map_err(error)?;
            Ok(())
        })
        .await
    }

    pub async fn started(&self, deployment: &Deployment) -> Result<()> {
        self.append(Recorded::Started { deployment: Box::new(deployment.clone()) }).await
    }

    pub async fn finished(&self, id: &str, finished: Finished) -> Result<()> {
        self.append(Recorded::Finished { id: id.to_string(), finished }).await
    }

    pub async fn release_added(&self, scope: &str, release: &ReleaseFacts, at: DateTime<Utc>) -> Result<()> {
        self.append(Recorded::ReleaseAdded { scope: scope.to_string(), release: release.clone(), at }).await
    }

    async fn rows(&self, deployment: Option<String>) -> Result<Vec<Recorded>> {
        self.with_conn(move |conn| {
            let (sql, args): (&str, Vec<String>) = match deployment {
                Some(id) => (
                    "SELECT id, data FROM deploy_events WHERE deployment = ?1 ORDER BY at, rowid",
                    vec![id],
                ),
                None => ("SELECT id, data FROM deploy_events ORDER BY at, rowid", vec![]),
            };
            let mut stmt = conn.prepare(sql).map_err(error)?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(args), |row| {
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
                        tracing::warn!(id, "skipping malformed deploy_events row: {err}");
                        None
                    }
                })
                .collect())
        })
        .await
    }

    /// Every deployment, folded from its rows, newest first.
    pub async fn deployments(&self) -> Result<Vec<Deployment>> {
        Ok(fold(self.rows(None).await?))
    }

    pub async fn deployment(&self, id: &str) -> Result<Option<Deployment>> {
        Ok(fold(self.rows(Some(id.to_string())).await?).into_iter().next())
    }

    /// Releases added without a deployment: `(scope, release, when)`.
    pub async fn releases_added(&self) -> Result<Vec<(String, ReleaseFacts, DateTime<Utc>)>> {
        Ok(self
            .rows(None)
            .await?
            .into_iter()
            .filter_map(|r| match r {
                Recorded::ReleaseAdded { scope, release, at } => Some((scope, release, at)),
                _ => None,
            })
            .collect())
    }

    pub async fn append_sample(&self, sample: Sample) -> Result<()> {
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO health_samples (environment, check_name, at, ok, latency_ms, detail, slow) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    sample.environment,
                    sample.check,
                    stamp(sample.at),
                    sample.ok as i64,
                    sample.latency_ms as i64,
                    sample.detail,
                    sample.slow as i64,
                ],
            )
            .map_err(error)?;
            Ok(())
        })
        .await
    }

    /// Every sample at or after `since`, oldest first.
    pub async fn samples_since(&self, since: DateTime<Utc>) -> Result<Vec<Sample>> {
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT environment, check_name, at, ok, latency_ms, detail, slow FROM health_samples \
                     WHERE at >= ?1 ORDER BY at",
                )
                .map_err(error)?;
            let rows = stmt
                .query_map(params![stamp(since)], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, bool>(6)?,
                    ))
                })
                .map_err(error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(error)?;
            Ok(rows
                .into_iter()
                .filter_map(|(environment, check, at, ok, latency, detail, slow)| {
                    let at = DateTime::parse_from_rfc3339(&at).ok()?.with_timezone(&Utc);
                    Some(Sample { environment, check, at, ok: ok != 0, latency_ms: latency.max(0) as u64, slow, detail })
                })
                .collect())
        })
        .await
    }

    /// Read only one check and page before its stable row cursor. The limit
    /// is applied in SQLite, not after loading an instance-wide history.
    pub async fn sample_page(
        &self, environment: String, check: String, from: DateTime<Utc>, to: DateTime<Utc>, before: Option<i64>, limit: u32,
    ) -> Result<factory_core::environments::SamplePage> {
        self.with_conn(move |conn| {
            use factory_core::environments::{SamplePage, SampleRecord};
            let limit = limit.clamp(1, 500) as usize;
            let mut statement = conn.prepare(
                "SELECT rowid, at, ok, latency_ms, detail, slow FROM health_samples \
                 WHERE environment = ?1 AND check_name = ?2 AND at >= ?3 AND at < ?4 \
                 AND (?5 IS NULL OR rowid < ?5) ORDER BY rowid DESC LIMIT ?6"
            ).map_err(error)?;
            let mut rows = statement.query_map(params![environment, check, stamp(from), stamp(to), before, (limit + 1) as u32], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, bool>(2)?, row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?, row.get::<_, bool>(5)?))
            }).map_err(error)?.collect::<std::result::Result<Vec<_>, _>>().map_err(error)?;
            let more = rows.len() > limit;
            rows.truncate(limit);
            let next_before = more.then(|| rows.last().expect("nonempty page").0);
            let samples = rows.into_iter().filter_map(|(id, at, ok, latency_ms, detail, slow)| {
                let at = DateTime::parse_from_rfc3339(&at).ok()?.with_timezone(&Utc);
                Some(SampleRecord { id, sample: Sample { environment: environment.clone(), check: check.clone(), at,
                    ok, latency_ms: latency_ms.max(0) as u64, detail, slow } })
            }).collect();
            Ok(SamplePage { environment, check, from, to, samples, next_before })
        }).await
    }

    /// Drop samples older than `before`; how many went.
    pub async fn prune_samples(&self, before: DateTime<Utc>) -> Result<usize> {
        self.with_conn(move |conn| {
            conn.execute("DELETE FROM health_samples WHERE at < ?1", params![stamp(before)])
                .map_err(error)
        })
        .await
    }
}

/// Deployments from their rows (oldest first), newest first. A `finished`
/// row for a deployment that has already finished is ignored: the first
/// ending recorded is the ending.
fn fold(rows: Vec<Recorded>) -> Vec<Deployment> {
    let mut by_id: BTreeMap<String, Deployment> = BTreeMap::new();
    for row in rows {
        match row {
            Recorded::Started { deployment } => {
                by_id.entry(deployment.id.clone()).or_insert(*deployment);
            }
            Recorded::Finished { id, finished } => {
                if let Some(d) = by_id.get_mut(&id) {
                    if d.status == DeployStatus::Running {
                        d.status = finished.status;
                        d.finished_at = Some(finished.at);
                        d.reason = finished.reason;
                        d.verification = finished.verification;
                    }
                }
            }
            Recorded::ReleaseAdded { .. } => {}
        }
    }
    let mut out: Vec<Deployment> = by_id.into_values().collect();
    out.sort_by(|a, b| b.started_at.cmp(&a.started_at).then_with(|| b.id.cmp(&a.id)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sample_pages_are_bounded_scoped_and_stable_at_equal_timestamps() {
        let store = EnvironmentStore::in_memory().unwrap();
        let now = Utc::now();
        for (environment, check, detail) in [("prod", "api", "first"), ("prod", "api", "second"), ("prod", "api", "third"), ("other", "api", "hidden"), ("prod", "disk", "different check")] {
            store.append_sample(Sample { environment: environment.into(), check: check.into(), at: now, ok: true, latency_ms: 1, detail: Some(detail.into()), slow: false }).await.unwrap();
        }
        let first = store.sample_page("prod".into(), "api".into(), now - chrono::Duration::seconds(1), now + chrono::Duration::seconds(1), None, 2).await.unwrap();
        assert_eq!(first.samples.len(), 2);
        assert_eq!(first.samples[0].sample.detail.as_deref(), Some("third"));
        assert_eq!(first.samples[1].sample.detail.as_deref(), Some("second"));
        assert!(first.next_before.is_some());
        store.append_sample(Sample { environment: "prod".into(), check: "api".into(), at: now, ok: false, latency_ms: 2, detail: Some("arrived after first page".into()), slow: false }).await.unwrap();
        let second = store.sample_page("prod".into(), "api".into(), first.from, first.to, first.next_before, 2).await.unwrap();
        assert_eq!(second.samples.len(), 1);
        assert_eq!(second.samples[0].sample.detail.as_deref(), Some("first"));
        assert!(second.next_before.is_none());
        assert!(!first.samples.iter().any(|s| s.id == second.samples[0].id));
    }
    use chrono::Duration;
    use factory_core::environments::{Actor, ActorKind};

    fn deployment(id: &str, at: DateTime<Utc>) -> Deployment {
        Deployment {
            id: id.into(),
            scope: "factory".into(),
            environment: "staging".into(),
            release: ReleaseFacts { commit: "abc".into(), ..Default::default() },
            actor: Actor { kind: ActorKind::Person, name: "owner".into(), run_id: None, task_id: None },
            via: None,
            manual: true,
            strict_verification: false,
            started_at: at,
            finished_at: None,
            status: DeployStatus::Running,
            reason: None,
            previous_commit: None,
            verification: None,
        }
    }

    #[tokio::test]
    async fn a_deployment_is_folded_from_its_rows_and_only_the_first_ending_counts() {
        let store = EnvironmentStore::in_memory().unwrap();
        let now = Utc::now();
        store.started(&deployment("a", now - Duration::minutes(5))).await.unwrap();
        store.started(&deployment("b", now)).await.unwrap();
        let end = |status| Finished { status, at: now, reason: Some("x".into()), verification: None };
        store.finished("a", end(DeployStatus::Failed)).await.unwrap();
        store.finished("a", end(DeployStatus::Succeeded)).await.unwrap();
        let all = store.deployments().await.unwrap();
        assert_eq!(all.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), vec!["b", "a"]);
        assert_eq!(all[1].status, DeployStatus::Failed);
        assert_eq!(all[0].status, DeployStatus::Running);
        assert_eq!(store.deployment("a").await.unwrap().unwrap().reason.as_deref(), Some("x"));
        assert!(store.deployment("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn samples_come_back_from_a_moment_on_and_old_ones_are_pruned() {
        let store = EnvironmentStore::in_memory().unwrap();
        let now = Utc::now();
        for (i, ok) in [(0, true), (10, false), (20, true)] {
            store
                .append_sample(Sample {
                    environment: "prod".into(),
                    check: "c".into(),
                    at: now - Duration::days(i),
                    ok,
                    latency_ms: 3,
                    slow: false,
                    detail: None,
                })
                .await
                .unwrap();
        }
        assert_eq!(store.samples_since(now - Duration::days(15)).await.unwrap().len(), 2);
        assert_eq!(store.prune_samples(now - Duration::days(15)).await.unwrap(), 1);
        let left = store.samples_since(now - Duration::days(365)).await.unwrap();
        assert_eq!(left.len(), 2);
        assert!(!left[0].ok, "oldest first");
    }

    #[tokio::test]
    async fn old_database_upgrade_is_idempotent_and_slow_history_survives_reopen() {
        let root = std::env::temp_dir().join(format!("factory-env-upgrade-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("store.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE health_samples (environment TEXT NOT NULL, check_name TEXT NOT NULL, at TEXT NOT NULL, ok INTEGER NOT NULL, latency_ms INTEGER NOT NULL, detail TEXT)").unwrap();
        let at = chrono::SubsecRound::trunc_subsecs(Utc::now(), 3);
        conn.execute("INSERT INTO health_samples VALUES ('prod', 'api', ?1, 1, 9000, '200')", [stamp(at)]).unwrap();
        drop(conn);
        let store = EnvironmentStore::open(&path).unwrap();
        let old = store.samples_since(at).await.unwrap().remove(0);
        assert!(old.ok && !old.slow);
        assert_eq!(old.latency_ms, 9000);
        let slow = Sample { at: at + Duration::seconds(1), slow: true, detail: Some("200; slow: 9000ms exceeds 750ms".into()), ..old.clone() };
        store.append_sample(slow.clone()).await.unwrap();
        drop(store);
        let reopened = EnvironmentStore::open(&path).unwrap();
        assert_eq!(reopened.samples_since(at).await.unwrap(), vec![old, slow]);
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }
}
