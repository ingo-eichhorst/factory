//! L4 append-only evidence for run verification and captured artifacts.
//! Owns only the existing run_attestations and artifact_provenance tables.
//! Policy receipt storage belongs to L6; no status table or fact log is added.
//! The daemon opens both owners on the existing instance database.

use crate::control_plan::StepAttestation;
use factory_kernel::{FactoryError, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::{Arc, Mutex};

const SCHEMA: &str = r#"
-- `#118`: the evidence each required step left for one run -- a gate's exit
-- code and output tail, who ran it, the commit it judged. Append-only:
-- a second verification of the
-- same run appends, it never rewrites what the first one found.
CREATE TABLE IF NOT EXISTS run_attestations (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    step TEXT NOT NULL,
    at TEXT NOT NULL,
    data TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS run_attestations_run ON run_attestations(run_id, at);

CREATE TABLE IF NOT EXISTS artifact_provenance (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    data TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS artifact_provenance_run ON artifact_provenance(run_id);
"#;

fn error(error: impl std::fmt::Display) -> FactoryError {
    FactoryError::adapter("policy sqlite", error.to_string())
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
/// Process evidence cannot be used to write upper-level policy receipts.
/// ```compile_fail
/// async fn wrong(store: factory_process::evidence_store::RunEvidenceStore) {
///     store.all().await.unwrap();
/// }
/// ```
pub struct RunEvidenceStore {
    conn: Arc<Mutex<Connection>>,
}

impl RunEvidenceStore {
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

    pub async fn append_provenance(
        &self,
        record: &factory_kernel::ArtifactProvenance,
    ) -> Result<()> {
        let record = record.clone();
        self.with_conn(move |conn| {
            let data = serde_json::to_string(&record).map_err(error)?;
            // An id identifies one captured immutable artifact. Never overwrite it.
            let existing: Option<String> = conn
                .query_row(
                    "SELECT data FROM artifact_provenance WHERE id = ?1",
                    params![record.id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(error)?;
            if let Some(existing) = existing {
                if existing == data {
                    return Ok(());
                }
                return Err(FactoryError::BadRequest(
                    "artifact provenance is append-only; this id already has different evidence"
                        .into(),
                ));
            }
            conn.execute(
                "INSERT INTO artifact_provenance(id, run_id, data) VALUES (?1, ?2, ?3)",
                params![record.id, record.run_id, data],
            )
            .map_err(error)?;
            Ok(())
        })
        .await
    }

    pub async fn provenance(
        &self,
        run_id: &str,
    ) -> Result<Vec<factory_kernel::ArtifactProvenance>> {
        let run_id = run_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare("SELECT id, data FROM artifact_provenance WHERE run_id = ?1 ORDER BY id")
                .map_err(error)?;
            let rows = stmt
                .query_map(params![run_id], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(error)?
                .collect::<std::result::Result<Vec<(String, String)>, _>>()
                .map_err(error)?;
            Ok(decode_all(rows, "artifact_provenance"))
        })
        .await
    }
}

impl RunEvidenceStore {
    /// Record one step's evidence for one run (`#118`). Insert only.
    pub async fn append_step_attestation(&self, attestation: &StepAttestation) -> Result<()> {
        let attestation = attestation.clone();
        self.with_conn(move |conn| {
            let data = serde_json::to_string(&attestation).map_err(error)?;
            conn.execute(
                "INSERT INTO run_attestations (id, run_id, task_id, step, at, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    attestation.id,
                    attestation.run_id,
                    attestation.task_id,
                    attestation.step,
                    attestation.at.to_rfc3339(),
                    data
                ],
            )
            .map_err(error)?;
            Ok(())
        })
        .await
    }

    /// Every attestation a run has collected, oldest first.
    pub async fn step_attestations(&self, run_id: &str) -> Result<Vec<StepAttestation>> {
        let run_id = run_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare("SELECT id, data FROM run_attestations WHERE run_id=?1 ORDER BY at ASC, rowid ASC")
                .map_err(error)?;
            let rows = stmt
                .query_map([&run_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
                .map_err(error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(error)?;
            Ok(decode_all(rows, "run_attestations"))
        })
        .await
    }

    /// `#158`: every attestation for each of `run_ids`, grouped by run,
    /// oldest first within a run -- `Engine::attested_runs`'s own batch
    /// read, so judging a whole window of runs costs one round trip (well,
    /// one per 500 ids -- sqlite's own limit on bound parameters) rather
    /// than one `step_attestations` call per run. A run id absent from the
    /// result never had one; the caller reads that as an empty `Vec`, the
    /// same as `step_attestations` would for it alone.
    pub async fn step_attestations_for(
        &self,
        run_ids: &[String],
    ) -> Result<std::collections::BTreeMap<String, Vec<StepAttestation>>> {
        let run_ids = run_ids.to_vec();
        self.with_conn(move |conn| {
            let mut out: std::collections::BTreeMap<String, Vec<StepAttestation>> = std::collections::BTreeMap::new();
            for chunk in run_ids.chunks(500) {
                if chunk.is_empty() {
                    continue;
                }
                let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
                let sql = format!(
                    "SELECT id, run_id, data FROM run_attestations WHERE run_id IN ({placeholders}) \
                     ORDER BY at ASC, rowid ASC"
                );
                let mut stmt = conn.prepare(&sql).map_err(error)?;
                let params: Vec<&dyn rusqlite::ToSql> = chunk.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
                let rows = stmt
                    .query_map(params.as_slice(), |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
                    })
                    .map_err(error)?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(error)?;
                for (id, run_id, json) in rows {
                    match decode::<StepAttestation>(json) {
                        Ok(attestation) => out.entry(run_id).or_default().push(attestation),
                        Err(err) => tracing::warn!(id, table = "run_attestations", "skipping malformed row: {err}"),
                    }
                }
            }
            Ok(out)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn only_process_tables_are_initialized() {
        let store = RunEvidenceStore::in_memory().unwrap();
        let conn = store.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap();
        let tables = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(tables, ["artifact_provenance", "run_attestations"]);
    }

    #[tokio::test]
    async fn step_attestations_append_and_come_back_per_run_oldest_first() {
        use crate::control_plan::{AttestationVerdict, StepKind};
        let store = RunEvidenceStore::in_memory().unwrap();
        let at = Utc::now();
        let make = |id: &str, run: &str, verdict, secs| StepAttestation {
            id: id.into(),
            run_id: run.into(),
            task_id: "t".into(),
            scope: "demo".into(),
            category: "feature".into(),
            step: "tests".into(),
            kind: StepKind::Gate,
            actor: "factory-daemon".into(),
            verdict,
            findings: None,
            round: 0,
            required_by: vec![],
            command: Some("true".into()),
            exit_code: Some(0),
            output: None,
            dir: "/tmp".into(),
            commit: None,
            dirty: None,
            worktree_digest: None,
            node_id: None,
            at: at + chrono::Duration::seconds(secs),
        };
        store
            .append_step_attestation(&make("b", "r1", AttestationVerdict::Pass, 2))
            .await
            .unwrap();
        store
            .append_step_attestation(&make("a", "r1", AttestationVerdict::Fail, 1))
            .await
            .unwrap();
        store
            .append_step_attestation(&make("c", "r2", AttestationVerdict::Pass, 1))
            .await
            .unwrap();
        let got = store.step_attestations("r1").await.unwrap();
        assert_eq!(
            got.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert!(
            store
                .append_step_attestation(&make("a", "r1", AttestationVerdict::Pass, 3))
                .await
                .is_err(),
            "an id is written once"
        );
    }

    #[tokio::test]
    async fn step_attestations_for_groups_by_run_oldest_first_and_leaves_out_runs_with_none() {
        use crate::control_plan::{AttestationVerdict, StepKind};
        let store = RunEvidenceStore::in_memory().unwrap();
        let at = Utc::now();
        let make = |id: &str, run: &str, secs| StepAttestation {
            id: id.into(),
            run_id: run.into(),
            task_id: "t".into(),
            scope: "demo".into(),
            category: "feature".into(),
            step: "tests".into(),
            kind: StepKind::Gate,
            actor: "factory-daemon".into(),
            verdict: AttestationVerdict::Pass,
            required_by: vec![],
            findings: None,
            round: 0,
            worktree_digest: None,
            command: Some("true".into()),
            exit_code: Some(0),
            output: None,
            dir: "/tmp".into(),
            commit: None,
            dirty: None,
            node_id: None,
            at: at + chrono::Duration::seconds(secs),
        };
        store
            .append_step_attestation(&make("b", "r1", 2))
            .await
            .unwrap();
        store
            .append_step_attestation(&make("a", "r1", 1))
            .await
            .unwrap();
        store
            .append_step_attestation(&make("c", "r2", 1))
            .await
            .unwrap();

        let got = store
            .step_attestations_for(&["r1".to_string(), "r2".to_string(), "r3".to_string()])
            .await
            .unwrap();
        assert_eq!(
            got.get("r1")
                .unwrap()
                .iter()
                .map(|a| a.id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert_eq!(
            got.get("r2")
                .unwrap()
                .iter()
                .map(|a| a.id.as_str())
                .collect::<Vec<_>>(),
            vec!["c"]
        );
        assert!(
            !got.contains_key("r3"),
            "a run with no attestations is simply absent"
        );

        assert!(store.step_attestations_for(&[]).await.unwrap().is_empty());
    }
}
