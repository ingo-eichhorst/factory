//! Whole-instance upgrade evidence is outside the physical ladder.
use factory_direction::renewal_store::AlertStore;
use factory_infrastructure::{backup_store::BackupStore, environment_store::EnvironmentStore};
use factory_kernel::ExpiryObservation;
use factory_process::workflow_store::WorkflowStore;
use rusqlite::{params, types::Value, Connection};
use serde_json::json;

const LEGACY_SCHEMA: &str = include_str!("../tests/fixtures/level_stores_legacy.sql");
const AT: &str = "2026-09-25T12:00:00Z";

fn schema(conn: &Connection) -> Vec<(String, String)> {
    let mut query = conn
        .prepare("SELECT name, sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name")
        .unwrap();
    query
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap()
}

fn dump(conn: &Connection) -> Vec<(String, Vec<Vec<Value>>)> {
    let mut query = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap();
    let names = query
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    names
        .into_iter()
        .map(|name| {
            let mut query = conn
                .prepare(&format!("SELECT * FROM \"{name}\" ORDER BY rowid"))
                .unwrap();
            let columns = query.column_count();
            let rows = query
                .query_map([], |row| {
                    (0..columns)
                        .map(|i| row.get::<_, Value>(i))
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap();
            (name, rows)
        })
        .collect()
}

fn observation(detail: &str) -> ExpiryObservation {
    serde_json::from_value(json!({
        "id": "same-id", "name": "metadata", "kind": "other", "scope": "demo",
        "expires_at": "2026-10-25T12:00:00Z", "no_expiry": false, "basis": "observed",
        "source": "tls", "detail": detail, "observed_at": AT, "attempted_at": AT,
        "issue": null, "affects": [], "lead_seconds": 86400, "renew": "review metadata", "owner": "owner"
    })).unwrap()
}

#[tokio::test]
async fn all_physical_stores_preserve_populated_legacy_tables_and_unknown_receipts() {
    use factory_process::workflow::{
        WorkflowActor, WorkflowDefinition, WorkflowDraft, WorkflowRun,
    };
    let path = std::env::temp_dir().join(format!(
        "factory-level-store-upgrade-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(LEGACY_SCHEMA).unwrap();
    conn.execute_batch("CREATE TABLE unrelated_attachment (id TEXT PRIMARY KEY, data BLOB NOT NULL); INSERT INTO unrelated_attachment VALUES ('foreign', X'00ff0102')").unwrap();
    let backup = factory_infrastructure::backup_store::Recorded::Failed {
        failure: factory_infrastructure::backup::BackupFailure {
            at: AT.parse().unwrap(),
            trigger: factory_infrastructure::backup::BackupTrigger::Schedule,
            reason: "unavailable destination".into(),
        },
    };
    conn.execute(
        "INSERT INTO backup_events VALUES ('backup', 'failed', NULL, ?1, ?2)",
        params![AT, serde_json::to_string_pretty(&backup).unwrap()],
    )
    .unwrap();
    let definition = WorkflowDefinition::from_draft(WorkflowDraft {
        name: "legacy flow".into(),
        scope: "demo".into(),
        ..Default::default()
    });
    let run = WorkflowRun::new(definition.clone(), WorkflowActor::Owner);
    conn.execute(
        "INSERT INTO workflow_definitions VALUES (?1, 'demo', ?2, ?3)",
        params![
            definition.id,
            AT,
            serde_json::to_string_pretty(&definition).unwrap()
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO workflow_runs VALUES (?1, ?2, 'demo', 'running', ?3, ?4)",
        params![
            run.id,
            definition.id,
            AT,
            serde_json::to_string_pretty(&run).unwrap()
        ],
    )
    .unwrap();
    let action = factory_kernel::ScriptRecoveryAction {
        id: uuid::Uuid::new_v4().to_string(),
        scope: "demo".into(),
        environment: "prod".into(),
        source: "ensure.sh".into(),
        actor: "operator".into(),
        reason: "unavailable daemon".into(),
        command: "restart installed".into(),
        started_at: AT.parse().unwrap(),
        expected_commit: None,
        finish: None,
    };
    conn.execute(
        "INSERT INTO recovery_journal VALUES (?1, ?2, 'demo', 0, ?3, ?4)",
        params![
            format!("{}:0", action.id),
            action.id,
            AT,
            serde_json::to_string(&action).unwrap()
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO deployment_mirrors VALUES ('malformed', 'deploy', 'demo', ?1, 'not json')",
        [AT],
    )
    .unwrap();
    let deployment = factory_infrastructure::environments::Deployment {
        id: "deploy".into(),
        scope: "demo".into(),
        environment: "prod".into(),
        release: factory_infrastructure::environments::ReleaseFacts {
            commit: "abc".into(),
            ..Default::default()
        },
        actor: factory_infrastructure::environments::Actor {
            kind: factory_infrastructure::environments::ActorKind::Person,
            name: "owner".into(),
            run_id: None,
            task_id: None,
        },
        via: None,
        manual: true,
        strict_verification: false,
        started_at: AT.parse().unwrap(),
        finished_at: None,
        status: factory_infrastructure::environments::DeployStatus::Running,
        reason: None,
        previous_commit: None,
        verification: None,
    };
    conn.execute(
        "INSERT INTO deploy_events VALUES ('event', 'started', 'deploy', ?1, ?2)",
        params![
            AT,
            serde_json::to_string_pretty(&json!({"kind": "started", "deployment": deployment}))
                .unwrap()
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO health_samples VALUES ('prod', 'api', ?1, 1, 9000, 1, 'slow response')",
        [AT],
    )
    .unwrap();
    let infrastructure = observation("certificate metadata");
    let credential = observation("credential metadata");
    conn.execute(
        "INSERT INTO infrastructure_expiries VALUES ('same-id', ?1)",
        [serde_json::to_string_pretty(&infrastructure).unwrap()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO credential_expiries VALUES ('same-id', ?1)",
        [serde_json::to_string_pretty(&credential).unwrap()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO renewal_push_receipts VALUES ('unknown', ?1, 'attempted')",
        [AT],
    )
    .unwrap();
    let original_schema = schema(&conn);
    let original_rows = dump(&conn);

    let backup_owner = BackupStore::open(&path).unwrap();
    let environment_owner = EnvironmentStore::open(&path).unwrap();
    let process_owner = WorkflowStore::open(&path).unwrap();
    let infrastructure_owner =
        factory_infrastructure::expiry_store::ObservationStore::open(&path).unwrap();
    let credential_owner =
        factory_environment::expiry_store::ObservationStore::open(&path).unwrap();
    let alert_owner = AlertStore::open(&path).unwrap();
    // Old daemon paths are canonical owner types, not delegating wrappers.
    let old_backup: &crate::backup::BackupStore = &backup_owner;
    let old_environment: &crate::environments::EnvironmentStore = &environment_owner;
    let old_process: &crate::workflows::WorkflowStore = &process_owner;
    let old_infrastructure: &crate::renewals::store::InfrastructureExpiryStore =
        &infrastructure_owner;
    let old_credentials: &crate::renewals::store::CredentialExpiryStore = &credential_owner;
    let old_alerts: &crate::renewals::store::AlertStore = &alert_owner;
    assert_eq!(old_backup.all().await.unwrap(), [backup]);
    assert_eq!(
        serde_json::to_value(
            old_process
                .get_definition(&definition.id)
                .await
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(&definition).unwrap()
    );
    assert_eq!(
        serde_json::to_value(old_process.get_run(&run.id).await.unwrap().unwrap()).unwrap(),
        serde_json::to_value(&run).unwrap()
    );
    assert_eq!(
        old_process
            .recovery_actions(Some("demo"), 10)
            .await
            .unwrap(),
        [action.clone()]
    );
    old_process.import_recovery(&action).await.unwrap();
    assert!(old_process
        .mirror_receipts("deploy", 10)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        old_environment.deployment("deploy").await.unwrap(),
        Some(deployment)
    );
    let samples = old_environment
        .samples_since(AT.parse().unwrap())
        .await
        .unwrap();
    assert_eq!(samples.len(), 1);
    assert!(samples[0].ok && samples[0].slow);
    assert_eq!(samples[0].latency_ms, 9000);
    assert_eq!(
        old_infrastructure.all().await.unwrap(),
        [infrastructure.clone()]
    );
    assert_eq!(old_credentials.all().await.unwrap(), [credential.clone()]);
    assert!(!old_alerts
        .claim("unknown".into(), chrono::Utc::now())
        .await
        .unwrap());
    assert_eq!(schema(&conn), original_schema);
    assert_eq!(
        dump(&conn),
        original_rows,
        "upgrade never rewrites legacy records or unrelated binary data"
    );
    drop((
        backup_owner,
        environment_owner,
        process_owner,
        infrastructure_owner,
        credential_owner,
        alert_owner,
    ));
    let credential_owner =
        factory_environment::expiry_store::ObservationStore::open(&path).unwrap();
    let infrastructure_owner =
        factory_infrastructure::expiry_store::ObservationStore::open(&path).unwrap();
    assert_eq!(credential_owner.all().await.unwrap(), [credential]);
    assert_eq!(infrastructure_owner.all().await.unwrap(), [infrastructure]);
    assert_eq!(schema(&conn), original_schema);
    assert_eq!(dump(&conn), original_rows);
    drop((credential_owner, infrastructure_owner, conn));
    std::fs::remove_file(path).unwrap();
}
