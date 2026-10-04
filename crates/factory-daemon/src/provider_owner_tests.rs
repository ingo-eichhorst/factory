//! Cross-store integration lives outside the level ladder. All data is throwaway.
use factory_kernel::{
    ArtifactProvenance, ArtifactSnapshot, DeploymentMirrorFact, Facts, People, Provide,
};
use factory_kernel::{
    ConfirmedSecurityReport, EnvironmentRecoveryFact, RecoveryJournalFact, ReleaseBuildFact,
    ScheduledRunDatesFact, ScopeNode, ScopeTree, TaskFact, TaskInventoryFact, WorkflowFact,
};
use factory_plugins::SqliteStore;
use factory_process::{
    evidence_store::RunEvidenceStore,
    facts::ProvenanceProvider,
    run::{Run, RunStatus},
    workflow_store::WorkflowStore,
};
use factory_process::{
    facts::{
        NamedQuery, Provider as ProcessProvider, RecoveryQuery, ReleaseBuildQuery,
        TaskInventoryQuery,
    },
    store::{task_from_new, TaskStore},
    task::{NewTask, TaskPatch, TaskStatus},
    workflow::{WorkflowActor, WorkflowDefinition, WorkflowDraft, WorkflowNode, WorkflowRun},
};
use rusqlite::{params, Connection};
use serde_json::json;

fn scope_tree() -> ScopeTree {
    ScopeTree {
        scopes: [
            ("root", "."),
            ("engineering", "projects/old"),
            ("child", "projects/old/deep"),
            ("sibling", "projects/other"),
        ]
        .into_iter()
        .map(|(name, path)| ScopeNode {
            name: name.into(),
            path: path.into(),
        })
        .collect(),
    }
}

fn insert_run(conn: &Connection, run: &Run) {
    conn.execute("INSERT INTO runs(id, task_id, attempt, status, started_at, ended_at, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![run.id, run.task_id, run.attempt, serde_json::to_value(run.status).unwrap().as_str(), run.started_at.to_rfc3339(), run.ended_at.map(|at|at.to_rfc3339()), serde_json::to_string(run).unwrap()]).unwrap();
}

#[tokio::test]
async fn process_inventory_schedule_and_named_history_are_live_and_selective() {
    let temp = TempRoot::new();
    let path = temp.0.join("instance.sqlite");
    let store = SqliteStore::open(&path).unwrap();
    let workflows = WorkflowStore::open(&path).unwrap();
    let conn = Connection::open(&path).unwrap();
    let mut tasks = Vec::new();
    for (i, scope) in [
        "engineering",
        "engineering",
        "old",
        "child",
        "gone",
        "sibling",
    ]
    .iter()
    .enumerate()
    {
        let mut task = task_from_new(
            NewTask {
                title: "shared".into(),
                ..Default::default()
            },
            (*scope).into(),
            "shell".into(),
            "quiet".into(),
        );
        task.id = format!("task-{i}");
        task.schedule = Some(factory_kernel::Schedule::Cron("0 9 * * *".into()));
        task.next_run_at = Some("2026-10-05T09:00:00Z".parse().unwrap());
        if i == 1 {
            task.schedule_paused = true;
        }
        tasks.push(store.create(&task).await.unwrap());
    }
    for i in 0..25 {
        let run: Run=serde_json::from_value(json!({"id":format!("run-{i:02}"),"task_id":"task-0","attempt":i+1,"status":"done","trigger":"manual","agent":"shell","runtime":"quiet","started_at":format!("2026-09-25T12:00:{i:02}Z"),"ended_at":format!("2026-09-25T12:01:{i:02}Z")})).unwrap();
        insert_run(&conn, &run);
    }
    let provider = ProcessProvider::new(&store, &workflows, scope_tree(), temp.0.clone());
    let reader = Facts::<People>::new();
    let exact = reader
        .get::<TaskInventoryFact, _>(&provider, &TaskInventoryQuery::Exact("old".into()))
        .await
        .unwrap();
    assert_eq!(
        exact
            .iter()
            .map(|t| t.id.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        ["task-0", "task-1"].into_iter().collect()
    );
    let members = reader
        .get::<TaskInventoryFact, _>(
            &provider,
            &TaskInventoryQuery::Members(["old".into()].into_iter().collect()),
        )
        .await
        .unwrap();
    assert_eq!(
        members.len(),
        3,
        "membership canonicalizes stored aliases; exact store selection does not"
    );
    assert!(reader
        .get::<TaskInventoryFact, _>(&provider, &TaskInventoryQuery::Members(Default::default()))
        .await
        .unwrap()
        .is_empty());
    assert!(matches!(
        reader
            .get::<TaskInventoryFact, _>(&provider, &TaskInventoryQuery::Exact("missing".into()))
            .await
            .unwrap_err(),
        factory_kernel::FactoryError::NoSuchScope(_)
    ));
    let scheduled = reader
        .get::<ScheduledRunDatesFact, _>(&provider, &())
        .await
        .unwrap();
    assert_eq!(scheduled.runs.len(), 5);
    assert_eq!(
        scheduled
            .runs
            .iter()
            .find(|r| r.task == "task-2")
            .unwrap()
            .scope,
        "engineering"
    );
    assert_eq!(
        scheduled
            .runs
            .iter()
            .find(|r| r.task == "task-4")
            .unwrap()
            .scope,
        "gone"
    );
    let named = reader
        .get::<TaskFact, _>(
            &provider,
            &NamedQuery {
                scope: "old".into(),
                names: ["shared".into(), "task-0".into(), "absent".into()]
                    .into_iter()
                    .collect(),
            },
        )
        .await
        .unwrap();
    assert_eq!(named["shared"].len(), 2);
    assert!(named["shared"].iter().all(|t| t.runs.is_empty()));
    assert!(named["absent"].is_empty());
    assert_eq!(named["task-0"][0].runs.len(), 20);
    assert_eq!(named["task-0"][0].runs[0].id, "run-24");
    assert_eq!(named["task-0"][0].runs[19].id, "run-05");
    assert!(reader
        .get::<TaskFact, _>(
            &provider,
            &NamedQuery {
                scope: "missing".into(),
                names: Default::default()
            }
        )
        .await
        .unwrap()
        .is_empty());
    store
        .update(
            "task-0",
            &TaskPatch {
                status: Some(TaskStatus::Done),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let after = reader
        .get::<TaskInventoryFact, _>(&provider, &TaskInventoryQuery::All)
        .await
        .unwrap();
    assert!(!after.iter().find(|t| t.id == "task-0").unwrap().open);
    assert_eq!(
        reader
            .get::<ScheduledRunDatesFact, _>(&provider, &())
            .await
            .unwrap()
            .runs
            .len(),
        4
    );
    assert_eq!(tasks.len(), 6);
}

#[tokio::test]
async fn workflow_history_and_environment_recovery_keep_identity_bounds_and_empty_selection() {
    let temp = TempRoot::new();
    let store = SqliteStore::in_memory().unwrap();
    let workflows = WorkflowStore::in_memory().unwrap();
    let node: WorkflowNode=serde_json::from_value(json!({"id":"recover","task":{"title":"recover","labels":{"factory.recovery.environment":"demo"}}})).unwrap();
    let mut def = WorkflowDefinition::from_draft(WorkflowDraft {
        name: "shared".into(),
        scope: "engineering".into(),
        nodes: vec![node],
        ..Default::default()
    });
    // Use the canonical label, not the spelling of a UI or test fixture.
    def.nodes[0].task.labels = [
        (
            factory_kernel::RECOVERY_ENVIRONMENT_LABEL.into(),
            "demo".into(),
        ),
        (
            factory_kernel::RECOVERY_COMMIT_LABEL.into(),
            "commit".into(),
        ),
    ]
    .into_iter()
    .collect();
    workflows.put_definition(&def).await.unwrap();
    let mut duplicate = def.clone();
    duplicate.id = "duplicate".into();
    workflows.put_definition(&duplicate).await.unwrap();
    for i in 0..25 {
        let mut run = WorkflowRun::new(
            def.clone(),
            WorkflowActor::Agent {
                scope: "engineering".into(),
                name: "operator".into(),
            },
        );
        run.id = format!("wf-{i:02}");
        run.updated_at = "2026-09-25T12:00:00Z"
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap()
            + chrono::Duration::seconds(i);
        workflows.put_run(&run).await.unwrap();
    }
    let provider = ProcessProvider::new(&store, &workflows, scope_tree(), temp.0.clone());
    let reader = Facts::<People>::new();
    let named = reader
        .get::<WorkflowFact, _>(
            &provider,
            &NamedQuery {
                scope: "old".into(),
                names: ["shared".into(), def.id.clone()].into_iter().collect(),
            },
        )
        .await
        .unwrap();
    assert_eq!(named["shared"].len(), 2);
    assert!(named["shared"].iter().all(|f| f.runs.is_empty()));
    assert_eq!(named[&def.id][0].runs.len(), 20);
    assert_eq!(named[&def.id][0].runs[0].id, "wf-24");
    assert!(reader
        .get::<WorkflowFact, _>(
            &provider,
            &NamedQuery {
                scope: "missing".into(),
                names: Default::default()
            }
        )
        .await
        .unwrap()
        .is_empty());
    let empty = RecoveryQuery {
        scopes: Some(Default::default()),
        limit: 200,
    };
    assert!(reader
        .get::<EnvironmentRecoveryFact, _>(&provider, &empty)
        .await
        .unwrap()
        .is_empty());
    let facts = reader
        .get::<EnvironmentRecoveryFact, _>(
            &provider,
            &RecoveryQuery {
                scopes: None,
                limit: 0,
            },
        )
        .await
        .unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].workflow_run_id, "wf-24");
    assert_eq!(facts[0].requested_by, "engineering/operator");
    assert_eq!(facts[0].expected_commit.as_deref(), Some("commit"));
    assert!(facts[0].run.is_none());
    assert!(reader
        .get::<EnvironmentRecoveryFact, _>(
            &provider,
            &RecoveryQuery {
                scopes: Some(["sibling".into()].into_iter().collect()),
                limit: 200
            }
        )
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn journal_owner_imports_even_when_selection_is_empty_and_preserves_conflicts_and_history() {
    let temp = TempRoot::new();
    std::fs::create_dir_all(temp.0.join(".factory")).unwrap();
    std::fs::write(temp.0.join(".factory/config.yaml"), "{}").unwrap();
    let path = temp.0.join(".factory/factory.sqlite");
    let store = SqliteStore::open(&path).unwrap();
    let workflows = WorkflowStore::open(&path).unwrap();
    let action = factory_kernel::ScriptRecoveryAction {
        id: String::new(),
        scope: "old".into(),
        environment: "demo".into(),
        source: "operator".into(),
        actor: "owner".into(),
        reason: "receipt only".into(),
        command: "true".into(),
        started_at: chrono::Utc::now(),
        expected_commit: None,
        finish: None,
    };
    let valid = factory_process::recovery_journal::start(&temp.0, action.clone()).unwrap();
    let mut unknown = action;
    unknown.scope = "missing".into();
    let unknown = factory_process::recovery_journal::start(&temp.0, unknown).unwrap();
    let provider = ProcessProvider::new(&store, &workflows, scope_tree(), temp.0.clone());
    let reader = Facts::<People>::new();
    let first = reader
        .get::<RecoveryJournalFact, _>(
            &provider,
            &RecoveryQuery {
                scopes: Some(Default::default()),
                limit: 200,
            },
        )
        .await
        .unwrap();
    assert!(first.actions.is_empty());
    assert_eq!(
        first.findings,
        ["1 receipts had an unknown scope or conflicted with immutable history"]
    );
    assert_eq!(
        workflows.recovery_actions(None, 200).await.unwrap(),
        [valid.clone()]
    );
    let result = reader
        .get::<RecoveryJournalFact, _>(
            &provider,
            &RecoveryQuery {
                scopes: Some(["old".into()].into_iter().collect()),
                limit: 0,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        result.actions.len(),
        1,
        "store and provider both retain the existing 1..200 clamp"
    );
    assert!(factory_process::recovery_journal::inbox(&temp.0)
        .join(unknown.id)
        .exists());
    let restarted = WorkflowStore::open(&path).unwrap();
    assert_eq!(
        restarted.recovery_actions(Some("old"), 200).await.unwrap(),
        [valid]
    );
    assert!(
        store.list(&Default::default()).await.unwrap().is_empty(),
        "offline receipts never synthesize task status"
    );
}

#[tokio::test]
async fn release_owner_keeps_exact_scope_clean_commit_done_and_immutable_evidence() {
    let temp = TempRoot::new();
    let path = temp.0.join("instance.sqlite");
    let store = SqliteStore::open(&path).unwrap();
    let evidence = RunEvidenceStore::open(&path).unwrap();
    let conn = Connection::open(&path).unwrap();
    let mut task = task_from_new(
        NewTask {
            title: "release".into(),
            ..Default::default()
        },
        "demo".into(),
        "shell".into(),
        "quiet".into(),
    );
    task.id = "task".into();
    store.create(&task).await.unwrap();
    let a = artifact("release");
    let mut run: Run=serde_json::from_value(json!({"id":"run","task_id":"task","attempt":1,"status":"done","trigger":"manual","agent":"shell","runtime":"quiet","started_at":"2026-09-25T12:00:00Z","ended_at":"2026-09-25T12:01:00Z","artifacts":[a]})).unwrap();
    insert_run(&conn, &run);
    let statement =
        factory_core::provenance::statement(&run, &a, &[], "throwaway", run.ended_at.unwrap());
    evidence.append_provenance(&statement).await.unwrap();
    let provider = ProvenanceProvider::new(&store, &evidence);
    let reader = Facts::<People>::new();
    let query = ReleaseBuildQuery {
        scope: "demo".into(),
        commit: "a".repeat(40),
        run_id: "run".into(),
    };
    let fact = reader
        .get::<ReleaseBuildFact, _>(&provider, &query)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fact.artifacts, [statement]);
    assert!(fact.attestations.is_empty());
    for q in [
        ReleaseBuildQuery {
            scope: "other".into(),
            ..ReleaseBuildQuery {
                scope: query.scope.clone(),
                commit: query.commit.clone(),
                run_id: query.run_id.clone(),
            }
        },
        ReleaseBuildQuery {
            scope: "demo".into(),
            commit: "b".repeat(40),
            run_id: "run".into(),
        },
        ReleaseBuildQuery {
            scope: "demo".into(),
            commit: query.commit.clone(),
            run_id: "missing".into(),
        },
    ] {
        assert!(reader
            .get::<ReleaseBuildFact, _>(&provider, &q)
            .await
            .unwrap()
            .is_none());
    }
    run.status = RunStatus::Running;
    update_run(&conn, &run);
    assert!(reader
        .get::<ReleaseBuildFact, _>(&provider, &query)
        .await
        .unwrap()
        .is_none());
    run.status = RunStatus::Done;
    run.artifacts[0].source.dirty = true;
    update_run(&conn, &run);
    assert!(reader
        .get::<ReleaseBuildFact, _>(&provider, &query)
        .await
        .unwrap()
        .is_none());
    let mut dirty = factory_core::provenance::statement(
        &run,
        &run.artifacts[0],
        &[],
        "throwaway",
        run.ended_at.unwrap(),
    );
    dirty.id = "dirty".into();
    evidence.append_provenance(&dirty).await.unwrap();
    assert_eq!(
        reader
            .get::<ArtifactProvenance, _>(&provider, &"run".into())
            .await
            .unwrap(),
        [dirty]
    );
    assert!(
        reader
            .get::<ReleaseBuildFact, _>(&provider, &query)
            .await
            .unwrap()
            .is_none(),
        "matching Done provenance still cannot attest a dirty release"
    );
}

#[tokio::test]
async fn security_owner_uses_path_subtrees_but_never_guesses_a_stored_scope_alias() {
    let temp = TempRoot::new();
    let store = SqliteStore::in_memory().unwrap();
    let workflows = WorkflowStore::in_memory().unwrap();
    let mut tasks = Vec::new();
    for (i, scope) in ["engineering", "child", "old", "sibling"]
        .iter()
        .enumerate()
    {
        let mut task = task_from_new(
            NewTask {
                title: "security".into(),
                ..Default::default()
            },
            (*scope).into(),
            "shell".into(),
            "quiet".into(),
        );
        task.id = format!("security-{i}");
        task.intake=Some(serde_json::from_value(json!({"stage":"received","source":{"kind":"cli"},"requester":"owner","received_at":"2026-09-25T12:00:00Z","security":{"state":"confirmed","flagged_by":"owner","flagged_at":"2026-09-25T12:00:00Z","decided_by":"owner"}})).unwrap());
        tasks.push(store.create(&task).await.unwrap());
    }
    let provider = ProcessProvider::new(&store, &workflows, scope_tree(), temp.0.clone());
    let reader = Facts::<People>::new();
    let reports = reader
        .get::<ConfirmedSecurityReport, _>(&provider, &Some("old".into()))
        .await
        .unwrap();
    assert_eq!(
        reports.iter().map(|r| r.item.as_str()).collect::<Vec<_>>(),
        ["security-0", "security-1"],
        "subtree selection preserves historical raw stored-scope membership"
    );
    assert_eq!(
        reader
            .get::<ConfirmedSecurityReport, _>(&provider, &None)
            .await
            .unwrap()
            .len(),
        4
    );
    assert!(reader
        .get::<ConfirmedSecurityReport, _>(&provider, &Some("missing".into()))
        .await
        .is_err());
    let mut intake = tasks[1].intake.clone().unwrap();
    intake.security.as_mut().unwrap().state = factory_process::intake::SecurityState::Dismissed;
    store
        .update(
            &tasks[1].id,
            &TaskPatch {
                intake: Some(intake),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        reader
            .get::<ConfirmedSecurityReport, _>(&provider, &Some("engineering".into()))
            .await
            .unwrap()
            .len(),
        1,
        "not a cached status"
    );
}

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
