//! Cross-store integration lives outside the level ladder. All data is throwaway.
use factory_kernel::{
    ArtifactProvenance, ArtifactSnapshot, DeploymentMirrorFact, Facts, People, Provide,
};
use factory_plugins::SqliteStore;
use factory_process::{
    evidence_store::RunEvidenceStore,
    facts::ProvenanceProvider,
    run::{Run, RunStatus},
    workflow_store::WorkflowStore,
};
use rusqlite::{params, Connection};
use serde_json::json;

struct TempRoot(std::path::PathBuf);
impl TempRoot {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("factory-owned-providers-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn artifact(id: &str) -> ArtifactSnapshot {
    serde_json::from_value(json!({
        "id": id, "name": "release", "scope": "demo", "category": "release",
        "sha256": "b".repeat(64), "size_bytes": 12,
        "storage_path": format!(".factory/artifacts/run/{id}/release"),
        "source_path": "/throwaway/release", "source": {
            "commit": "a".repeat(40), "worktree_digest": "c".repeat(64), "dirty": false
        }
    }))
    .unwrap()
}

fn rows(conn: &Connection, table: &str) -> Vec<(String, String)> {
    let mut query = conn
        .prepare(&format!("SELECT id, data FROM {table} ORDER BY id"))
        .unwrap();
    query
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn update_run(conn: &Connection, run: &Run) {
    conn.execute(
        "UPDATE runs SET status=?1, ended_at=?2, data=?3 WHERE id=?4",
        params![
            serde_json::to_value(run.status).unwrap().as_str(),
            run.ended_at.map(|at| at.to_rfc3339()),
            serde_json::to_string_pretty(run).unwrap(),
            run.id
        ],
    )
    .unwrap();
}

#[tokio::test]
async fn provenance_owner_keeps_authoritative_done_and_all_identity_filters_on_live_reads() {
    let temp = TempRoot::new();
    let path = temp.0.join("instance.sqlite");
    let store = SqliteStore::open(&path).unwrap();
    let evidence = RunEvidenceStore::open(&path).unwrap();
    let conn = Connection::open(&path).unwrap();
    let first = artifact("a-ok");
    let second = artifact("z-ok");
    let mut run: Run = serde_json::from_value(json!({
        "id": "run", "task_id": "task", "attempt": 1, "status": "done",
        "trigger": "manual", "agent": "builder", "runtime": "quiet",
        "started_at": "2026-09-25T12:00:00Z", "ended_at": "2026-09-25T12:01:00Z",
        "artifacts": [first, second]
    }))
    .unwrap();
    conn.execute("INSERT INTO runs(id, task_id, attempt, status, started_at, ended_at, data) VALUES ('run', 'task', 1, 'done', ?1, ?2, ?3)",
        params![run.started_at.to_rfc3339(), run.ended_at.unwrap().to_rfc3339(), serde_json::to_string_pretty(&run).unwrap()]).unwrap();
    let good_a =
        factory_core::provenance::statement(&run, &first, &[], "throwaway", run.ended_at.unwrap());
    let good_z =
        factory_core::provenance::statement(&run, &second, &[], "throwaway", run.ended_at.unwrap());
    evidence.append_provenance(&good_z).await.unwrap();
    evidence.append_provenance(&good_a).await.unwrap();
    for kind in ["wrong-run", "wrong-task", "wrong-finish", "wrong-artifact"] {
        let mut invalid = good_a.clone();
        invalid.id = kind.into();
        match kind {
            "wrong-run" => invalid.run_id = "other".into(),
            "wrong-task" => invalid.task_id = "other".into(),
            "wrong-finish" => {
                invalid.statement.predicate.run_details.metadata.finished_on +=
                    chrono::Duration::seconds(1)
            }
            "wrong-artifact" => invalid.artifact.sha256 = "d".repeat(64),
            _ => unreachable!(),
        }
        // A legacy malformed identity must be rejected even if its SQL index says "run".
        conn.execute(
            "INSERT INTO artifact_provenance VALUES (?1, 'run', ?2)",
            params![invalid.id, serde_json::to_string_pretty(&invalid).unwrap()],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO artifact_provenance VALUES ('malformed', 'run', 'not json')",
        [],
    )
    .unwrap();
    let before_runs = rows(&conn, "runs");
    let before_evidence = rows(&conn, "artifact_provenance");
    let provider = ProvenanceProvider::new(&store, &evidence);
    let reader = Facts::<People>::new();
    let expected = vec![good_a, good_z];
    assert_eq!(
        reader
            .get::<ArtifactProvenance, _>(&provider, &"run".into())
            .await
            .unwrap(),
        expected
    );
    assert_eq!(rows(&conn, "runs"), before_runs);
    assert_eq!(rows(&conn, "artifact_provenance"), before_evidence);
    let error = reader
        .get::<ArtifactProvenance, _>(&provider, &"missing".into())
        .await
        .unwrap_err();
    assert!(
        matches!(error, factory_kernel::FactoryError::TaskNotFound(ref id) if id == "run missing")
    );
    for status in [
        RunStatus::Dispatching,
        RunStatus::Running,
        RunStatus::Blocked,
        RunStatus::Verifying,
        RunStatus::Failed,
        RunStatus::Cancelled,
    ] {
        run.status = status;
        update_run(&conn, &run);
        assert!(
            Provide::<ArtifactProvenance>::get(&provider, &"run".into())
                .await
                .unwrap()
                .is_empty(),
            "{status:?}"
        );
    }
    run.status = RunStatus::Done;
    run.ended_at = None;
    update_run(&conn, &run);
    assert!(reader
        .get::<ArtifactProvenance, _>(&provider, &"run".into())
        .await
        .unwrap()
        .is_empty());
    run.ended_at = Some("2026-09-25T12:01:00Z".parse().unwrap());
    update_run(&conn, &run);
    assert_eq!(
        reader
            .get::<ArtifactProvenance, _>(&provider, &"run".into())
            .await
            .unwrap(),
        expected
    );
    conn.execute_batch("DROP TABLE artifact_provenance")
        .unwrap();
    assert!(
        matches!(reader.get::<ArtifactProvenance, _>(&provider, &"run".into()).await.unwrap_err(), factory_kernel::FactoryError::Adapter { ref adapter, .. } if adapter == "policy sqlite")
    );
    run.status = RunStatus::Running;
    update_run(&conn, &run);
    assert!(
        reader
            .get::<ArtifactProvenance, _>(&provider, &"run".into())
            .await
            .unwrap()
            .is_empty(),
        "unfinished runs must not query provenance"
    );
}

#[tokio::test]
async fn mirror_owner_retains_newest_insertion_order_bounded_decode_and_exact_selection() {
    let temp = TempRoot::new();
    let path = temp.0.join("instance.sqlite");
    let store = WorkflowStore::open(&path).unwrap();
    let conn = Connection::open(&path).unwrap();
    let reader = Facts::<People>::new();
    assert!(reader
        .get::<DeploymentMirrorFact, _>(&store, &"deploy".into())
        .await
        .unwrap()
        .is_empty());
    for i in 0..201 {
        let receipt: DeploymentMirrorFact = serde_json::from_value(json!({
            "id": format!("m-{i}"), "plan": {"deployment":"deploy", "scope":"demo", "repository":"owner/project",
                "environment":"prod", "commit":"abc", "state":"success", "verified":null, "transient":false, "production":true, "approval":"explicit"},
            "phase":"approved", "at": ("2026-09-25T12:00:00Z".parse::<chrono::DateTime<chrono::Utc>>().unwrap() - chrono::Duration::seconds(i)).to_rfc3339(),
            "approved_by":"owner", "remote_id":null, "status_id":null, "error":null
        })).unwrap();
        store.record_mirror(&receipt).await.unwrap();
    }
    conn.execute("INSERT INTO deployment_mirrors VALUES ('malformed', 'deploy', 'demo', '2026-09-25T12:00:00Z', 'not json')", []).unwrap();
    conn.execute("INSERT INTO deployment_mirrors VALUES ('other', 'different-deploy', 'demo', '2026-09-25T12:00:00Z', 'not json')", []).unwrap();
    let before = rows(&conn, "deployment_mirrors");
    let facts = reader
        .get::<DeploymentMirrorFact, _>(&store.clone(), &"deploy".into())
        .await
        .unwrap();
    assert_eq!(
        facts.len(),
        199,
        "limit applies before malformed-row skipping"
    );
    assert_eq!(facts.first().unwrap().id, "m-200");
    assert_eq!(facts.last().unwrap().id, "m-2");
    assert_eq!(facts, store.mirror_receipts("deploy", 200).await.unwrap());
    assert_eq!(rows(&conn, "deployment_mirrors"), before);
    assert!(reader
        .get::<DeploymentMirrorFact, _>(&store, &"missing".into())
        .await
        .unwrap()
        .is_empty());
    conn.execute_batch("DROP TABLE deployment_mirrors").unwrap();
    assert!(
        matches!(reader.get::<DeploymentMirrorFact, _>(&store, &"deploy".into()).await.unwrap_err(), factory_kernel::FactoryError::Adapter { ref adapter, .. } if adapter == "workflow sqlite")
    );
}
