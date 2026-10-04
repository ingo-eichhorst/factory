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

fn measurement_at(seconds: i64) -> chrono::DateTime<chrono::Utc> {
    "2026-09-25T12:00:00Z"
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap()
        + chrono::Duration::seconds(seconds)
}

fn measurement_run(id: &str, task: &str, start: i64, end: i64) -> Run {
    serde_json::from_value(json!({"id":id,"task_id":task,"attempt":1,"status":"done",
        "trigger":"manual","agent":"shell","runtime":"quiet",
        "started_at":measurement_at(start),"ended_at":measurement_at(end)}))
    .unwrap()
}

async fn measurement_task(
    store: &SqliteStore,
    id: &str,
    scope: &str,
) -> factory_process::task::Task {
    let mut task = task_from_new(
        NewTask {
            title: id.into(),
            category: Some("feature".into()),
            ..Default::default()
        },
        scope.into(),
        "shell".into(),
        "quiet".into(),
    );
    task.id = id.into();
    store.create(&task).await.unwrap()
}

#[tokio::test]
async fn measurement_spend_is_live_scope_canonical_and_keeps_unattributed_runs() {
    use factory_kernel::{CostGroupBy, CostReport, SpendBasis};
    use factory_process::measurements::MeasurementProvider;
    use factory_process::usage::{RunUsage, SnapshotPoint, SpendQuery, TokenCounts, UsageState};
    let temp = TempRoot::new();
    let path = temp.0.join("instance.sqlite");
    let store = SqliteStore::open(&path).unwrap();
    let workflows = WorkflowStore::open(&path).unwrap();
    let evidence = RunEvidenceStore::open(&path).unwrap();
    let conn = Connection::open(&path).unwrap();
    for (id, scope) in [("parent", "old"), ("child", "child"), ("gone", "gone")] {
        measurement_task(&store, id, scope).await;
    }
    for (id, task, cost) in [
        ("one", "parent", Some(2.0)),
        ("two", "child", Some(2.0)),
        ("three", "gone", None),
        ("four", "deleted", Some(3.0)),
    ] {
        let mut run = measurement_run(id, task, 0, 60);
        run.usage = cost.map(|cost| RunUsage {
            state: UsageState::Known,
            reason: None,
            cost_usd: Some(cost),
            tokens: TokenCounts {
                input: Some(10),
                output: Some(0),
                cache_read: Some(0),
                cache_write: Some(0),
            },
            as_of_point: Some(SnapshotPoint::RunEnd),
            ..RunUsage::unknown("", 2)
        });
        insert_run(&conn, &run);
    }
    let provider = MeasurementProvider::new(&store, &workflows, &evidence, scope_tree());
    let reader = Facts::<People>::new();
    let q = SpendQuery {
        scope: Some("old".into()),
        from: Some(measurement_at(-1)),
        to: Some(measurement_at(3600)),
        group_by: CostGroupBy::Scope,
        basis: SpendBasis::Finished,
    };
    let scoped = reader.get::<CostReport, _>(&provider, &q).await.unwrap();
    assert_eq!(scoped.scope.as_deref(), Some("engineering"));
    assert_eq!(scoped.total.runs, 2);
    assert_eq!(scoped.total.cost_usd, 4.0);
    assert_eq!(
        scoped.unattributed_runs, 2,
        "removed scopes and deleted tasks are not free"
    );
    assert_eq!(scoped.daily[0].unattributed_runs, 2);
    assert!(scoped
        .finished
        .unwrap()
        .unit_cost
        .reason
        .unwrap()
        .contains("unattributed"));
    let all = reader
        .get::<CostReport, _>(
            &provider,
            &SpendQuery {
                scope: None,
                group_by: CostGroupBy::Workflow,
                ..q.clone()
            },
        )
        .await
        .unwrap();
    assert_eq!(all.total.runs, 4);
    assert_eq!(all.total.runs_unknown, 1);
    assert_eq!(all.total.cost_usd, 7.0);
    assert!(all
        .rows
        .iter()
        .any(|row| row.key == "(deleted task)" && row.label.is_none()));
    let mut run: Run = serde_json::from_str(
        &conn
            .query_row("SELECT data FROM runs WHERE id='one'", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
    )
    .unwrap();
    run.usage.as_mut().unwrap().cost_usd = Some(6.0);
    update_run(&conn, &run);
    assert_eq!(
        reader
            .get::<CostReport, _>(&provider, &q)
            .await
            .unwrap()
            .total
            .cost_usd,
        8.0
    );
    assert!(matches!(
        reader
            .get::<CostReport, _>(
                &provider,
                &SpendQuery {
                    scope: Some("missing".into()),
                    ..q.clone()
                }
            )
            .await
            .unwrap_err(),
        factory_kernel::FactoryError::NoSuchScope(_)
    ));
    assert!(reader
        .get::<CostReport, _>(&provider, &SpendQuery { from: q.to, ..q })
        .await
        .is_err());
}

#[tokio::test]
async fn measurement_production_keeps_exact_people_selection_distinct_from_subtrees() {
    use factory_kernel::{ProductionBin, ProductionFact};
    use factory_process::measurements::{MeasurementProvider, ProductionQuery};
    let temp = TempRoot::new();
    let path = temp.0.join("instance.sqlite");
    let store = SqliteStore::open(&path).unwrap();
    let workflows = WorkflowStore::open(&path).unwrap();
    let evidence = RunEvidenceStore::open(&path).unwrap();
    let conn = Connection::open(&path).unwrap();
    measurement_task(&store, "parent", "old").await;
    measurement_task(&store, "child", "child").await;
    let mut failed = measurement_run("failed", "parent", 0, 10);
    failed.status = RunStatus::Failed;
    insert_run(&conn, &failed);
    let mut retry = measurement_run("retry", "parent", 11, 20);
    retry.attempt = 2;
    retry.trigger = factory_process::run::Trigger::Retry;
    insert_run(&conn, &retry);
    insert_run(&conn, &measurement_run("child", "child", 21, 30));
    insert_run(&conn, &measurement_run("ghost", "deleted", 31, 40));
    let provider = MeasurementProvider::new(&store, &workflows, &evidence, scope_tree());
    let reader = Facts::<People>::new();
    let mut q = ProductionQuery {
        scope: Some("engineering".into()),
        now: measurement_at(60),
        minutes: Some(5),
        bin: ProductionBin::Day,
        subtree: false,
    };
    let exact = reader
        .get::<ProductionFact, _>(&provider, &q)
        .await
        .unwrap();
    assert_eq!(exact.buckets.iter().map(|b| b.finished).sum::<u32>(), 2);
    assert_eq!(exact.buckets.iter().map(|b| b.scrapped).sum::<u32>(), 1);
    assert_eq!(exact.buckets.iter().map(|b| b.reworked).sum::<u32>(), 1);
    q.subtree = true;
    assert_eq!(
        reader
            .get::<ProductionFact, _>(&provider, &q)
            .await
            .unwrap()
            .buckets
            .iter()
            .map(|b| b.finished)
            .sum::<u32>(),
        3
    );
    q.scope = None;
    let all = reader
        .get::<ProductionFact, _>(&provider, &q)
        .await
        .unwrap();
    assert_eq!(
        all.buckets.iter().map(|b| b.finished).sum::<u32>(),
        4,
        "unscoped includes deleted tasks"
    );
    assert_eq!(all.buckets.iter().map(|b| b.first_pass).sum::<u32>(), 2);
    q.scope = Some("missing".into());
    q.subtree = false;
    assert_eq!(
        reader
            .get::<ProductionFact, _>(&provider, &q)
            .await
            .unwrap()
            .buckets
            .iter()
            .map(|b| b.finished)
            .sum::<u32>(),
        0
    );
    q.subtree = true;
    assert!(matches!(
        reader
            .get::<ProductionFact, _>(&provider, &q)
            .await
            .unwrap_err(),
        factory_kernel::FactoryError::NoSuchScope(_)
    ));
}

#[tokio::test]
async fn measurement_conformance_uses_exact_canonical_selection_and_frozen_steps() {
    use factory_kernel::{AttestedRun, StepAttestation};
    use factory_process::measurements::{AttestedQuery, MeasurementProvider};
    let temp = TempRoot::new();
    let path = temp.0.join("instance.sqlite");
    let store = SqliteStore::open(&path).unwrap();
    let workflows = WorkflowStore::open(&path).unwrap();
    let evidence = RunEvidenceStore::open(&path).unwrap();
    let conn = Connection::open(&path).unwrap();
    measurement_task(&store, "parent", "old").await;
    measurement_task(&store, "child", "child").await;
    let mut bench = task_from_new(
        NewTask {
            title: "bench".into(),
            ..Default::default()
        },
        "engineering".into(),
        "shell".into(),
        "quiet".into(),
    );
    bench.id = "bench".into();
    bench.bench_origin = Some(serde_json::from_value(json!({"opaque":"bench"})).unwrap());
    store.create(&bench).await.unwrap();
    let mut run = measurement_run("parent", "parent", 0, 10);
    run.required_steps = vec![serde_json::from_value(
        json!({"step":"tests","kind":"gate","command":"old command","required_by":["house/rule"]}),
    )
    .unwrap()];
    insert_run(&conn, &run);
    insert_run(&conn, &measurement_run("boundary", "parent", -10, 0));
    insert_run(&conn, &measurement_run("child", "child", 11, 20));
    insert_run(&conn, &measurement_run("bench", "bench", 11, 20));
    let provider = MeasurementProvider::new(&store, &workflows, &evidence, scope_tree());
    let reader = Facts::<People>::new();
    let q = AttestedQuery {
        scopes: Some(["engineering".into()].into_iter().collect()),
        categories: Some(["feature".into()].into_iter().collect()),
        window: factory_process::window::Window {
            from: measurement_at(0),
            to: measurement_at(30),
        },
    };
    let before = reader.get::<AttestedRun, _>(&provider, &q).await.unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].required_steps, run.required_steps);
    assert!(before[0].attestations.is_empty());
    let receipt:StepAttestation=serde_json::from_value(json!({"id":"receipt","run_id":"parent","task_id":"parent","scope":"engineering","category":"feature","step":"tests","kind":"gate","actor":"factory-daemon","verdict":"pass","dir":"/throwaway","at":measurement_at(10)})).unwrap();
    evidence.append_step_attestation(&receipt).await.unwrap();
    let after = reader.get::<AttestedRun, _>(&provider, &q).await.unwrap();
    assert_eq!(after[0].attestations, [receipt]);
    assert_eq!(
        after[0].required_steps[0].command.as_deref(),
        Some("old command")
    );
}

#[tokio::test]
async fn measurement_hours_share_chart_unions_and_ignore_liveness_guesses() {
    use factory_kernel::ProcessMetricFact;
    use factory_process::measurements::{MeasurementProvider, ProcessMetricsQuery};
    let temp = TempRoot::new();
    let path = temp.0.join("instance.sqlite");
    let store = SqliteStore::open(&path).unwrap();
    let workflows = WorkflowStore::open(&path).unwrap();
    let evidence = RunEvidenceStore::open(&path).unwrap();
    let conn = Connection::open(&path).unwrap();
    measurement_task(&store, "parent", "old").await;
    measurement_task(&store, "child", "child").await;
    for (id, task, start, end, agent) in [
        ("a", "parent", 0, 600, "shell"),
        ("b", "parent", 300, 900, "shell"),
        ("c", "parent", 100, 700, "other"),
        ("d", "child", 0, 300, "shell"),
    ] {
        let mut run = measurement_run(id, task, start, end);
        run.agent = agent.into();
        insert_run(&conn, &run);
    }
    for (run, kind, at) in [
        ("a", "blocked", 100),
        ("a", "unblocked", 200),
        ("b", "blocked", 150),
        ("b", "done", 450),
        ("d", "blocked", 10),
        ("d", "done", 50),
    ] {
        let task = if run == "d" { "child" } else { "parent" };
        let mut entry = factory_process::task::TaskEntry::new("worker", kind, kind).in_run(run);
        entry.at = measurement_at(at);
        store.append_entry(task, &entry).await.unwrap();
    }
    store
        .append_status(&factory_process::occupancy::StatusChange {
            subject: "engineering/shell".into(),
            scope: "engineering".into(),
            agent: "shell".into(),
            status: factory_core::adapter::RuntimeStatus::Blocked,
            at: measurement_at(0),
        })
        .await
        .unwrap();
    let provider = MeasurementProvider::new(&store, &workflows, &evidence, scope_tree());
    let reader = Facts::<People>::new();
    let q = ProcessMetricsQuery {
        scope: Some("old".into()),
        now: measurement_at(3600),
        window_days: Some(1),
        names: ["agent_hours".into(), "blocked_hours".into()]
            .into_iter()
            .collect(),
    };
    let facts = reader
        .get::<ProcessMetricFact, _>(&provider, &q)
        .await
        .unwrap();
    assert_eq!(facts["agent_hours"].value, Some(1800.0 / 3600.0));
    assert_eq!(facts["blocked_hours"].value, Some(390.0 / 3600.0));
    assert_eq!(facts["blocked_hours"].as_of, q.now);
    let unknown = ProcessMetricsQuery {
        names: ["compliance.cra".into()].into_iter().collect(),
        ..q
    };
    assert!(reader
        .get::<ProcessMetricFact, _>(&provider, &unknown)
        .await
        .is_err());
}

#[tokio::test]
async fn measurement_metrics_keep_unknowns_and_instance_wide_goal_labels() {
    use factory_kernel::ProcessMetricFact;
    use factory_process::measurements::{MeasurementProvider, ProcessMetricsQuery};
    let store = SqliteStore::in_memory().unwrap();
    let workflows = WorkflowStore::in_memory().unwrap();
    let evidence = RunEvidenceStore::in_memory().unwrap();
    let task = measurement_task(&store, "goal", "sibling").await;
    store
        .update(
            &task.id,
            &TaskPatch {
                status: Some(TaskStatus::Done),
                labels: Some([("goal".into(), "obj/kr".into())].into_iter().collect()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let provider = MeasurementProvider::new(&store, &workflows, &evidence, scope_tree());
    let reader = Facts::<People>::new();
    let mut q = ProcessMetricsQuery {
        scope: Some("child".into()),
        now: measurement_at(3600),
        window_days: None,
        names: [
            "goal_tasks_done.obj.kr".into(),
            "fail_rate".into(),
            "ready_rate".into(),
        ]
        .into_iter()
        .collect(),
    };
    let facts = reader
        .get::<ProcessMetricFact, _>(&provider, &q)
        .await
        .unwrap();
    assert_eq!(facts["goal_tasks_done.obj.kr"].value, Some(1.0));
    assert!(facts["fail_rate"].value.is_none() && facts["fail_rate"].reason.is_some());
    assert!(facts["ready_rate"].value.is_none());
    q.window_days = Some(i64::MAX);
    assert!(matches!(
        reader
            .get::<ProcessMetricFact, _>(&provider, &q)
            .await
            .unwrap_err(),
        factory_kernel::FactoryError::BadRequest(_)
    ));
}

#[tokio::test]
async fn measurement_store_read_failure_is_an_error_not_a_zero_fact() {
    use factory_process::measurements::MeasurementProvider;
    let temp = TempRoot::new();
    let path = temp.0.join("instance.sqlite");
    let store = SqliteStore::open(&path).unwrap();
    let workflows = WorkflowStore::open(&path).unwrap();
    let evidence = RunEvidenceStore::open(&path).unwrap();
    let conn = Connection::open(&path).unwrap();
    conn.execute("DROP TABLE runs", []).unwrap();
    let provider = MeasurementProvider::new(&store, &workflows, &evidence, scope_tree());
    let query = factory_process::usage::SpendQuery {
        scope: None,
        from: Some(measurement_at(0)),
        to: Some(measurement_at(60)),
        group_by: factory_kernel::CostGroupBy::Scope,
        basis: factory_kernel::SpendBasis::Started,
    };
    assert!(Facts::<People>::new()
        .get::<factory_kernel::CostReport, _>(&provider, &query)
        .await
        .is_err());
}

#[tokio::test]
async fn measurement_hours_require_authoritative_journal_not_chart_liveness() {
    use factory_process::measurements::{MeasurementProvider, ProcessMetricsQuery};
    let temp = TempRoot::new();
    let path = temp.0.join("instance.sqlite");
    let store = SqliteStore::open(&path).unwrap();
    let workflows = WorkflowStore::open(&path).unwrap();
    let evidence = RunEvidenceStore::open(&path).unwrap();
    let conn = Connection::open(&path).unwrap();
    measurement_task(&store, "task", "engineering").await;
    insert_run(&conn, &measurement_run("run", "task", 0, 60));
    let provider = MeasurementProvider::new(&store, &workflows, &evidence, scope_tree());
    let query = ProcessMetricsQuery {
        scope: None,
        now: measurement_at(3600),
        window_days: Some(1),
        names: ["agent_hours".into(), "blocked_hours".into()]
            .into_iter()
            .collect(),
    };
    conn.execute("DROP TABLE agent_status", []).unwrap();
    let facts = Facts::<People>::new()
        .get::<factory_kernel::ProcessMetricFact, _>(&provider, &query)
        .await
        .unwrap();
    assert_eq!(facts["agent_hours"].value, Some(60.0 / 3600.0));
    assert_eq!(facts["blocked_hours"].value, Some(0.0));
    conn.execute("DROP TABLE task_entries", []).unwrap();
    assert!(
        Facts::<People>::new()
            .get::<factory_kernel::ProcessMetricFact, _>(&provider, &query)
            .await
            .is_err(),
        "a failed authoritative journal read must not fabricate zero blocked hours"
    );
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
