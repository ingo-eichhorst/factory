//! Cross-owner database upgrade tests belong outside the physical ladder.
use factory_direction::policy::{Attestation, Withdrawal};
use factory_direction::policy_store::PolicyStore;
use factory_kernel::{ArtifactProvenance, StepAttestation};
use factory_process::evidence_store::RunEvidenceStore;
use rusqlite::{params, Connection};
use serde_json::json;
use std::path::PathBuf;

// Exact pre-split DDL, independently retained as an upgrade fixture.
const LEGACY_SCHEMA: &str = r#"
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
CREATE UNIQUE INDEX IF NOT EXISTS policy_attestations_one_withdrawal
    ON policy_attestations(withdraws) WHERE withdraws IS NOT NULL;
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

const AT: &str = "2026-09-25T12:00:00Z";

struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "factory-evidence-owner-{}.sqlite",
            uuid::Uuid::new_v4()
        )))
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn receipt(id: &str) -> Attestation {
    serde_json::from_value(json!({
        "id": id, "control": "cra/tests", "scope": "demo",
        "evidence": "https://example.invalid/evidence", "attested_by": "owner",
        "attested_at": AT, "expires_at": "2026-10-25T12:00:00Z"
    }))
    .unwrap()
}

fn step(id: &str, run: &str) -> StepAttestation {
    // Optional fields deliberately absent, as in older stored run evidence.
    serde_json::from_value(json!({
        "id": id, "run_id": run, "task_id": "task", "scope": "demo",
        "category": "feature", "step": "tests", "kind": "gate",
        "actor": "factory-daemon", "verdict": "pass", "dir": "/tmp/demo", "at": AT
    }))
    .unwrap()
}

fn provenance() -> ArtifactProvenance {
    let source = json!({"commit": "commit", "worktree_digest": "digest", "dirty": false});
    serde_json::from_value(json!({
        "id": "artifact-1", "run_id": "run", "task_id": "task", "scope": "demo",
        "artifact": {"id": "artifact-1", "name": "release.bin", "scope": "demo",
            "category": "feature", "sha256": "012345", "size_bytes": 6,
            "storage_path": ".factory/artifacts/artifact-1", "source_path": "release.bin",
            "source": source},
        "statement": {"_type": "https://in-toto.io/Statement/v1", "subject": [],
            "predicateType": "https://slsa.dev/provenance/v1", "predicate": {
                "buildDefinition": {"buildType": "factory", "externalParameters": {
                    "taskId": "task", "scope": "demo", "category": "feature"}, "resolvedDependencies": []},
                "runDetails": {"builder": {"id": "factory"}, "metadata": {
                    "invocationId": "run", "startedOn": AT, "finishedOn": AT}},
                "https://github.com/ingo-eichhorst/factory/run-evidence/v1": {
                    "agent": "shell", "adapter": "shell", "runtime": "herdr", "source": source,
                    "required_steps": [], "attestations": []}
            }}
    })).unwrap()
}

fn schema(conn: &Connection) -> Vec<(String, String)> {
    let mut stmt = conn
        .prepare("SELECT name, sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap()
}

fn rows(conn: &Connection) -> Vec<(String, String, String)> {
    let mut stmt = conn.prepare("SELECT 'policy', id, data FROM policy_attestations UNION ALL SELECT 'run', id, data FROM run_attestations UNION ALL SELECT 'artifact', id, data FROM artifact_provenance ORDER BY 1, 2").unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap()
}

#[tokio::test]
async fn legacy_combined_database_reopens_without_rewriting_schema_or_evidence() {
    let db = Database::new();
    let conn = Connection::open(&db.0).unwrap();
    conn.execute_batch(LEGACY_SCHEMA).unwrap();
    let receipt = receipt("receipt");
    let withdrawal = Withdrawal {
        at: AT.parse().unwrap(),
        by: "owner".into(),
        reason: Some("superseded".into()),
    };
    conn.execute("INSERT INTO policy_attestations VALUES ('receipt', 'attestation', 'cra/tests', 'demo', NULL, ?1, ?2)",
        params![AT, serde_json::to_string_pretty(&receipt).unwrap()]).unwrap();
    conn.execute("INSERT INTO policy_attestations VALUES ('withdrawal', 'withdrawal', 'cra/tests', 'demo', 'receipt', ?1, ?2)",
        params![AT, serde_json::to_string_pretty(&withdrawal).unwrap()]).unwrap();
    let first = step("z-first", "run");
    let second = step("a-second", "run");
    for step in [&first, &second] {
        conn.execute(
            "INSERT INTO run_attestations VALUES (?1, 'run', 'task', 'tests', ?2, ?3)",
            params![step.id, AT, serde_json::to_string_pretty(step).unwrap()],
        )
        .unwrap();
    }
    let artifact = provenance();
    conn.execute(
        "INSERT INTO artifact_provenance VALUES ('artifact-1', 'run', ?1)",
        [serde_json::to_string_pretty(&artifact).unwrap()],
    )
    .unwrap();
    let original_schema = schema(&conn);
    let original_rows = rows(&conn);

    let policy = PolicyStore::open(&db.0).unwrap();
    let evidence = RunEvidenceStore::open(&db.0).unwrap();
    // Existing daemon compatibility path is the physical L6 type, not a wrapper.
    let old_path: &crate::policies::PolicyStore = &policy;
    let mut expected_receipt = receipt.clone();
    expected_receipt.withdrawn = Some(withdrawal.clone());
    assert_eq!(
        old_path.get("receipt").await.unwrap(),
        Some(expected_receipt.clone())
    );
    assert_eq!(policy.all().await.unwrap(), vec![expected_receipt]);
    assert_eq!(
        evidence.step_attestations("run").await.unwrap(),
        vec![first.clone(), second]
    );
    assert_eq!(
        evidence.provenance("run").await.unwrap(),
        vec![artifact.clone()]
    );
    assert!(policy.append_attestation(&receipt).await.is_err());
    assert!(policy
        .append_withdrawal("receipt", &receipt.control, &receipt.scope, &withdrawal)
        .await
        .unwrap_err()
        .to_string()
        .contains("already withdrawn"));
    assert!(evidence.append_step_attestation(&first).await.is_err());
    let mut changed = artifact.clone();
    changed.artifact.sha256 = "changed".into();
    assert!(evidence.append_provenance(&changed).await.is_err());
    assert_eq!(schema(&conn), original_schema);
    assert_eq!(
        rows(&conn),
        original_rows,
        "opening owners and refused writes never rewrite legacy JSON"
    );
    drop(policy);
    drop(evidence);

    // Reverse startup order also works on the same old database.
    let evidence = RunEvidenceStore::open(&db.0).unwrap();
    let policy = PolicyStore::open(&db.0).unwrap();
    assert_eq!(evidence.provenance("run").await.unwrap(), vec![artifact]);
    assert_eq!(evidence.step_attestations("run").await.unwrap().len(), 2);
    assert_eq!(policy.all().await.unwrap().len(), 1);
    assert_eq!(schema(&conn), original_schema);
    assert_eq!(rows(&conn), original_rows);
}

#[tokio::test]
async fn split_owners_append_concurrently_and_persist_on_one_database() {
    let db = Database::new();
    let policy = PolicyStore::open(&db.0).unwrap();
    let evidence = RunEvidenceStore::open(&db.0).unwrap();
    let mut writers = Vec::new();
    for i in 0..32 {
        let policy = policy.clone();
        let evidence = evidence.clone();
        writers.push(tokio::spawn(async move {
            policy
                .append_attestation(&receipt(&format!("policy-{i}")))
                .await
                .unwrap();
            evidence
                .append_step_attestation(&step(&format!("step-{i}"), "run"))
                .await
                .unwrap();
        }));
    }
    for writer in writers {
        writer.await.unwrap();
    }
    evidence.append_provenance(&provenance()).await.unwrap();
    // Idempotence is retained for the exact serialized immutable record.
    evidence.append_provenance(&provenance()).await.unwrap();
    drop(policy);
    drop(evidence);
    let evidence = RunEvidenceStore::open(&db.0).unwrap();
    let policy = PolicyStore::open(&db.0).unwrap();
    assert_eq!(policy.all().await.unwrap().len(), 32);
    assert_eq!(evidence.step_attestations("run").await.unwrap().len(), 32);
    assert_eq!(
        evidence.provenance("run").await.unwrap(),
        vec![provenance()]
    );
    assert_eq!(
        schema(&Connection::open(&db.0).unwrap()).len(),
        7,
        "three existing tables and four explicit indexes only"
    );
}

#[tokio::test]
async fn process_batch_chunks_keep_tie_order_and_skip_malformed_rows() {
    let db = Database::new();
    let evidence = RunEvidenceStore::open(&db.0).unwrap();
    let run_ids: Vec<String> = (0..501).map(|i| format!("run-{i}")).collect();
    let conn = Connection::open(&db.0).unwrap();
    let mut expected = std::collections::BTreeMap::new();
    for run in &run_ids {
        let steps = vec![
            step(&format!("z-{run}"), run),
            step(&format!("a-{run}"), run),
        ];
        for step in &steps {
            conn.execute(
                "INSERT INTO run_attestations VALUES (?1, ?2, 'task', 'tests', ?3, ?4)",
                params![step.id, run, AT, serde_json::to_string(step).unwrap()],
            )
            .unwrap();
        }
        expected.insert(run.clone(), steps);
    }
    for run in ["run-0", "run-500", "only-bad"] {
        conn.execute(
            "INSERT INTO run_attestations VALUES (?1, ?2, 'task', 'tests', ?3, 'not json')",
            params![format!("bad-{run}"), run, AT],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO artifact_provenance VALUES ('bad', 'run', 'not json')",
        [],
    )
    .unwrap();
    assert_eq!(
        evidence.step_attestations_for(&run_ids).await.unwrap(),
        expected
    );
    assert_eq!(
        evidence.step_attestations("run-0").await.unwrap(),
        expected["run-0"]
    );
    assert!(evidence
        .step_attestations_for(&["only-bad".into(), "absent".into()])
        .await
        .unwrap()
        .is_empty());
    assert!(evidence
        .step_attestations_for(&[])
        .await
        .unwrap()
        .is_empty());
    assert!(evidence.provenance("run").await.unwrap().is_empty());
}
