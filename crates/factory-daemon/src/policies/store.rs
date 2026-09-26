//! Where attestations live: one table, `policy_attestations`, append-only --
//! no `UPDATE` and no `DELETE` anywhere below. Recording an attestation
//! inserts one row; withdrawing one inserts another that references it by
//! id, and never touches the row it names. `get` and `all` fold that second
//! row's `Withdrawal` onto the first's `policy::Attestation` on the way out,
//! so a caller never sees the two rows as anything but one attestation with
//! or without a `withdrawn`.
//!
//! Follows `workflows/store.rs` and `bench/store.rs` exactly: its own
//! `CREATE TABLE IF NOT EXISTS`, no `SCHEMA_VERSION` check, `open`/`in_memory`,
//! one connection behind a mutex, blocking `rusqlite` calls moved to
//! `spawn_blocking` via `with_conn`.
//!
//! The catalogues themselves -- `.factory/policies/<framework>.yaml` -- are
//! authored content read straight off disk by `policy::load_all`; nothing
//! about them lives here. This table holds only the one piece of new state
//! ADR 0004 introduces: the audit trail of who attested to what, and when
//! that was withdrawn.

use factory_core::control_plan::StepAttestation;
use factory_core::error::{FactoryError, Result};
use factory_core::policy::{Attestation, ControlRef, Withdrawal};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::{Arc, Mutex};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS policy_attestations (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    control TEXT NOT NULL,
    scope TEXT NOT NULL,
    withdraws TEXT,
    at TEXT NOT NULL,
    data TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS policy_attestations_scope ON policy_attestations(scope, at);
-- At most one withdrawal per attestation, enforced by the schema rather than
-- a read-then-write race: a second `append_withdrawal` for the same id fails
-- the insert instead of silently adding a row nothing folds in.
CREATE UNIQUE INDEX IF NOT EXISTS policy_attestations_one_withdrawal
    ON policy_attestations(withdraws) WHERE withdraws IS NOT NULL;

-- `#118`: the evidence each required step left for one run -- a gate's exit
-- code and output tail, who ran it, the commit it judged. Append-only like
-- the table above, and for the same reason: a second verification of the
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
pub struct PolicyStore {
    conn: Arc<Mutex<Connection>>,
}

impl PolicyStore {
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

    /// Insert one attestation row. `attestation.withdrawn` is ignored on the
    /// way in -- a fresh attestation is never recorded already withdrawn --
    /// and always `None` on the way back out of `get`/`all` until a matching
    /// `append_withdrawal` exists.
    pub async fn append_attestation(&self, attestation: &Attestation) -> Result<()> {
        let mut attestation = attestation.clone();
        attestation.withdrawn = None;
        self.with_conn(move |conn| {
            let data = serde_json::to_string(&attestation).map_err(error)?;
            conn.execute(
                "INSERT INTO policy_attestations (id, kind, control, scope, withdraws, at, data) \
                 VALUES (?1, 'attestation', ?2, ?3, NULL, ?4, ?5)",
                params![
                    attestation.id,
                    attestation.control.to_string(),
                    attestation.scope,
                    attestation.attested_at.to_rfc3339(),
                    data,
                ],
            )
            .map_err(error)?;
            Ok(())
        })
        .await
    }

    /// Insert a row that withdraws `attestation_id`, never touching the row
    /// it names. `control` and `scope` are the attestation's own -- the
    /// caller already has them from a prior `get`, and carrying them here
    /// keeps a scope- or control-filtered query from needing a join back to
    /// the attestation it withdraws. Fails outright (rather than silently
    /// doing nothing) when `attestation_id` was already withdrawn -- the
    /// unique index on `withdraws` is what enforces that, not a
    /// read-then-write check racing another caller's.
    pub async fn append_withdrawal(
        &self,
        attestation_id: &str,
        control: &ControlRef,
        scope: &str,
        withdrawal: &Withdrawal,
    ) -> Result<()> {
        let row_id = format!("withdrawal-{}", uuid::Uuid::new_v4());
        let attestation_id = attestation_id.to_string();
        let control = control.to_string();
        let scope = scope.to_string();
        let withdrawal = withdrawal.clone();
        self.with_conn(move |conn| {
            let data = serde_json::to_string(&withdrawal).map_err(error)?;
            conn.execute(
                "INSERT INTO policy_attestations (id, kind, control, scope, withdraws, at, data) \
                 VALUES (?1, 'withdrawal', ?2, ?3, ?4, ?5, ?6)",
                params![row_id, control, scope, attestation_id, withdrawal.at.to_rfc3339(), data],
            )
            .map_err(|e| match e {
                rusqlite::Error::SqliteFailure(err, _) if err.code == rusqlite::ErrorCode::ConstraintViolation => {
                    FactoryError::BadRequest(format!("attestation {attestation_id:?} is already withdrawn"))
                }
                other => error(other),
            })?;
            Ok(())
        })
        .await
    }

    /// One attestation, folded with its withdrawal when it has one. `None`
    /// when `id` names no attestation row at all.
    pub async fn get(&self, id: &str) -> Result<Option<Attestation>> {
        let id = id.to_string();
        self.with_conn(move |conn| {
            let json: Option<String> = conn
                .query_row(
                    "SELECT data FROM policy_attestations WHERE id=?1 AND kind='attestation'",
                    [&id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(error)?;
            let Some(json) = json else { return Ok(None) };
            let mut attestation: Attestation = decode(json)?;
            attestation.withdrawn = fetch_withdrawal(conn, &id)?;
            Ok(Some(attestation))
        })
        .await
    }

    /// Every attestation ever recorded, folded with its withdrawal, most
    /// recently attested first. Small enough -- an instance's whole audit
    /// trail, not one scope's worth of tasks -- that filtering by scope or
    /// control is left to the caller, which already has to walk the scope
    /// tree to know which scopes even apply.
    pub async fn all(&self) -> Result<Vec<Attestation>> {
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare("SELECT id, data FROM policy_attestations WHERE kind='attestation' ORDER BY at DESC")
                .map_err(error)?;
            let rows = stmt
                .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
                .map_err(error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(error)?;
            let mut attestations: Vec<Attestation> = decode_all(rows, "policy_attestations");

            let mut wstmt = conn
                .prepare("SELECT withdraws, data FROM policy_attestations WHERE kind='withdrawal'")
                .map_err(error)?;
            let wrows = wstmt
                .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
                .map_err(error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(error)?;
            let withdrawals: std::collections::BTreeMap<String, Withdrawal> = wrows
                .into_iter()
                .filter_map(|(withdraws, json)| match decode::<Withdrawal>(json) {
                    Ok(w) => Some((withdraws, w)),
                    Err(err) => {
                        tracing::warn!(withdraws, "skipping malformed withdrawal row: {err}");
                        None
                    }
                })
                .collect();

            for attestation in &mut attestations {
                attestation.withdrawn = withdrawals.get(&attestation.id).cloned();
            }
            Ok(attestations)
        })
        .await
    }
}

impl PolicyStore {
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
}

/// The one withdrawal row referencing `attestation_id`, if there is one --
/// the unique index guarantees there is never more than one.
fn fetch_withdrawal(conn: &Connection, attestation_id: &str) -> Result<Option<Withdrawal>> {
    let json: Option<String> = conn
        .query_row(
            "SELECT data FROM policy_attestations WHERE kind='withdrawal' AND withdraws=?1",
            [attestation_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(error)?;
    json.map(decode).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn attestation(id: &str, control: &str, scope: &str) -> Attestation {
        let now = Utc::now();
        Attestation {
            id: id.to_string(),
            control: control.parse().unwrap(),
            scope: scope.to_string(),
            evidence: "https://example.com/policy".to_string(),
            note: None,
            attested_by: "owner".to_string(),
            attested_at: now,
            expires_at: now + chrono::Duration::days(30),
            withdrawn: None,
        }
    }

    #[tokio::test]
    async fn step_attestations_append_and_come_back_per_run_oldest_first() {
        use factory_core::control_plan::{AttestationVerdict, StepKind};
        let store = PolicyStore::in_memory().unwrap();
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
        store.append_step_attestation(&make("b", "r1", AttestationVerdict::Pass, 2)).await.unwrap();
        store.append_step_attestation(&make("a", "r1", AttestationVerdict::Fail, 1)).await.unwrap();
        store.append_step_attestation(&make("c", "r2", AttestationVerdict::Pass, 1)).await.unwrap();
        let got = store.step_attestations("r1").await.unwrap();
        assert_eq!(got.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
        assert!(store.append_step_attestation(&make("a", "r1", AttestationVerdict::Pass, 3)).await.is_err(), "an id is written once");
    }

    #[tokio::test]
    async fn an_attestation_round_trips_and_all_lists_it() {
        let store = PolicyStore::in_memory().unwrap();
        let a = attestation("att-1", "cra/a", "demo");
        store.append_attestation(&a).await.unwrap();

        let fetched = store.get("att-1").await.unwrap().unwrap();
        assert_eq!(fetched, a);
        assert_eq!(store.all().await.unwrap(), vec![a]);
        assert!(store.get("no-such-id").await.unwrap().is_none());
    }

    /// The issue's own requirement: a withdrawal is its own appended row,
    /// and the original attestation row is never touched -- checked here by
    /// reading the raw row count and the original row's own bytes back off
    /// the connection directly, not just through the folded view `get`
    /// hands back.
    #[tokio::test]
    async fn a_withdrawal_is_a_new_row_and_leaves_the_original_row_intact() {
        let store = PolicyStore::in_memory().unwrap();
        let a = attestation("att-1", "cra/a", "demo");
        store.append_attestation(&a).await.unwrap();

        let original_data: String = {
            let conn = store.conn.lock().unwrap();
            conn.query_row(
                "SELECT data FROM policy_attestations WHERE id='att-1'",
                [],
                |row| row.get(0),
            )
            .unwrap()
        };

        let withdrawal = Withdrawal {
            at: Utc::now(),
            by: "owner".to_string(),
            reason: Some("superseded".to_string()),
        };
        store
            .append_withdrawal("att-1", &a.control, &a.scope, &withdrawal)
            .await
            .unwrap();

        let row_count: i64 = {
            let conn = store.conn.lock().unwrap();
            conn.query_row("SELECT COUNT(*) FROM policy_attestations", [], |row| row.get(0))
                .unwrap()
        };
        assert_eq!(row_count, 2, "one row for the attestation, one for the withdrawal");

        let still_original: String = {
            let conn = store.conn.lock().unwrap();
            conn.query_row(
                "SELECT data FROM policy_attestations WHERE id='att-1'",
                [],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(still_original, original_data, "the original row's bytes never changed");

        let folded = store.get("att-1").await.unwrap().unwrap();
        assert_eq!(folded.withdrawn, Some(withdrawal.clone()));

        let all = store.all().await.unwrap();
        assert_eq!(all.len(), 1, "the withdrawal folds onto the attestation, not a second entry");
        assert_eq!(all[0].withdrawn, Some(withdrawal));
    }

    #[tokio::test]
    async fn withdrawing_an_already_withdrawn_attestation_is_refused() {
        let store = PolicyStore::in_memory().unwrap();
        let a = attestation("att-1", "cra/a", "demo");
        store.append_attestation(&a).await.unwrap();
        let first = Withdrawal {
            at: Utc::now(),
            by: "owner".to_string(),
            reason: None,
        };
        store
            .append_withdrawal("att-1", &a.control, &a.scope, &first)
            .await
            .unwrap();

        let second = Withdrawal {
            at: Utc::now(),
            by: "owner".to_string(),
            reason: Some("again".to_string()),
        };
        let err = store
            .append_withdrawal("att-1", &a.control, &a.scope, &second)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("already withdrawn"), "{err}");

        // The first withdrawal still stands, untouched by the refused second.
        let folded = store.get("att-1").await.unwrap().unwrap();
        assert_eq!(folded.withdrawn, Some(first));
    }

    #[tokio::test]
    async fn a_malformed_attestation_row_is_skipped_in_all_and_errors_alone_when_fetched() {
        let store = PolicyStore::in_memory().unwrap();
        let good = attestation("att-1", "cra/a", "demo");
        store.append_attestation(&good).await.unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO policy_attestations (id, kind, control, scope, withdraws, at, data) \
                 VALUES ('bad-id', 'attestation', 'cra/b', 'demo', NULL, '2024-01-01T00:00:00Z', 'not json')",
                [],
            )
            .unwrap();
        }

        let all = store.all().await.unwrap();
        assert_eq!(all.len(), 1, "the malformed row is skipped, not fatal");
        assert_eq!(all[0].id, "att-1");
        assert!(store.get("bad-id").await.is_err());
        assert!(store.get("att-1").await.unwrap().is_some());
    }
}
