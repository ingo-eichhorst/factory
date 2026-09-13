use crate::access::Caller;
use crate::engine::Engine;
use chrono::Utc;
use factory_core::error::{FactoryError, Result};
use factory_core::event::Event;
use factory_core::run::Trigger;
use factory_core::task::{TaskStatus, WorkflowOrigin};
use factory_core::workflow::{
    WorkflowDefinition, WorkflowDraft, WorkflowNodeStatus, WorkflowRun, WorkflowRunStatus,
};
use std::sync::Arc;

fn missing(kind: &str, id: &str) -> FactoryError {
    FactoryError::BadRequest(format!("no such {kind}: {id}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_core::adapter::{AgentRuntime, StartRequest};
    use factory_core::agent::{AgentSession, Lifetime};
    use factory_core::config::{Config, DaemonConfig, Factory, Instance, Scope};
    use factory_core::role::Role;
    use factory_core::run::RunStatus;
    use factory_core::task::{NewTask, SessionRef, TaskFilter, TaskReport};
    use factory_core::workflow::{
        CanvasPoint, WorkflowActor, WorkflowEdge, WorkflowNode, WorkflowNodeKind,
    };
    use factory_plugins::{Registry, SqliteStore};
    use std::path::PathBuf;

    struct QuietRuntime;
    #[async_trait::async_trait]
    impl AgentRuntime for QuietRuntime {
        fn name(&self) -> &str {
            "quiet"
        }
        async fn start(&self, req: &StartRequest) -> Result<SessionRef> {
            Ok(SessionRef {
                runtime: "quiet".into(),
                handle: req.id.clone(),
                meta: Default::default(),
            })
        }
        async fn submit(&self, _: &SessionRef, _: &str) -> Result<()> {
            Ok(())
        }
        async fn status(&self, _: &SessionRef) -> Result<factory_core::adapter::RuntimeStatus> {
            Ok(factory_core::adapter::RuntimeStatus::Working)
        }
        async fn send_text(&self, _: &SessionRef, _: &str) -> Result<()> {
            Ok(())
        }
        async fn send_keys(&self, _: &SessionRef, _: &[String]) -> Result<()> {
            Ok(())
        }
        async fn read(&self, _: &SessionRef, _: u32) -> Result<String> {
            Ok(String::new())
        }
        async fn stop(&self, _: &SessionRef) -> Result<()> {
            Ok(())
        }
    }

    fn engine() -> Arc<Engine> {
        let root =
            std::env::temp_dir().join(format!("factory-workflow-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let config = Config {
            version: 1,
            instance: Instance {
                id: "test".into(),
                name: "test".into(),
            },
            daemon: DaemonConfig::default(),
            roles: Default::default(),
            scope: None,
            scopes: vec![Scope {
                id: "demo-id".into(),
                name: "demo".into(),
                path: PathBuf::new(),
                agent: None,
                agents: Vec::new(),
                runtime: Some("quiet".into()),
                git: None,
                task_store: None,
            }],
            plugins_dir: None,
        };
        let mut registry = Registry::with_builtins();
        registry.add_runtime(Arc::new(QuietRuntime), "test");
        Arc::new(Engine::new(
            Factory { root, config },
            registry,
            Arc::new(SqliteStore::in_memory().unwrap()),
            PathBuf::from("factory"),
            Vec::new(),
        ))
    }

    /// Like `engine()`, but the instance names its own roles and scope
    /// agents -- for the authorization tests, where the built-in presets
    /// grant more than the scenario wants to hold constant.
    fn engine_with_roles(yaml: &str) -> Arc<Engine> {
        let root =
            std::env::temp_dir().join(format!("factory-workflow-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let config: Config = serde_yaml_ng::from_str(yaml).unwrap();
        config.validate().unwrap();
        let mut registry = Registry::with_builtins();
        registry.add_runtime(Arc::new(QuietRuntime), "test");
        Arc::new(Engine::new(
            Factory { root, config },
            registry,
            Arc::new(SqliteStore::in_memory().unwrap()),
            PathBuf::from("factory"),
            Vec::new(),
        ))
    }

    /// A caller wearing `role` in the "demo" scope, constructed directly the
    /// way `access.rs`'s own tests do -- `authorize` and
    /// `authorize_workflow_spawn` take whatever `Caller` they are handed, so
    /// this is the same shortcut past `caller_for`/`effective_role` that a
    /// real request would have already taken before reaching either.
    fn wearing(role: &str) -> Caller {
        Caller::Agent {
            scope: "demo".into(),
            name: "w".into(),
            role: Role::new(role),
            run_id: None,
        }
    }

    fn node(id: &str) -> WorkflowNode {
        WorkflowNode {
            id: id.into(),
            position: CanvasPoint::default(),
            kind: WorkflowNodeKind::Task,
            task: NewTask {
                title: id.into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                runtime: Some("quiet".into()),
                worktree: Some(false),
                ..Default::default()
            },
        }
    }

    fn edge(from: &str, to: &str) -> WorkflowEdge {
        WorkflowEdge {
            id: format!("{from}-{to}"),
            from: from.into(),
            to: to.into(),
        }
    }

    async fn create(
        engine: &Arc<Engine>,
        nodes: Vec<WorkflowNode>,
        edges: Vec<WorkflowEdge>,
    ) -> WorkflowDefinition {
        engine
            .create_workflow(WorkflowDraft {
                name: "pipeline".into(),
                scope: "demo".into(),
                nodes,
                edges,
                ..Default::default()
            })
            .await
            .unwrap()
    }

    async fn tasks(engine: &Arc<Engine>) -> Vec<factory_core::Task> {
        engine.store.list(&TaskFilter::default()).await.unwrap()
    }

    async fn wait_for_tasks(engine: &Arc<Engine>, count: usize) -> Vec<factory_core::Task> {
        for _ in 0..100 {
            let found = tasks(engine).await;
            if found.len() == count {
                return found;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("expected {count} tasks, got {}", tasks(engine).await.len());
    }

    async fn finish(engine: &Arc<Engine>, task_id: &str, status: RunStatus) {
        let run = loop {
            if let Some(run) = engine.store.active_run(task_id).await.unwrap() {
                break run;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        engine
            .report(
                task_id,
                TaskReport {
                    status: Some(status),
                    message: Some("reported by test".into()),
                    result: None,
                    error: (status == RunStatus::Failed).then(|| "boom".into()),
                    token: run.token,
                },
            )
            .await
            .unwrap();
        engine.sync_workflow_for_task(task_id).await;
    }

    #[tokio::test]
    async fn linear_nodes_spawn_once_and_keep_provenance() {
        let engine = engine();
        let definition = create(&engine, vec![node("a"), node("b")], vec![edge("a", "b")]).await;
        let run = engine.start_workflow(&definition.id, &Caller::Owner).await.unwrap();
        let first = wait_for_tasks(&engine, 1).await.pop().unwrap();
        assert_eq!(
            first.workflow_origin.as_ref().unwrap().workflow_run_id,
            run.id
        );
        engine.advance_workflow(&run.id).await.unwrap();
        assert_eq!(
            tasks(&engine).await.len(),
            1,
            "reconciliation does not duplicate the root"
        );
        finish(&engine, &first.id, RunStatus::Done).await;
        let all = wait_for_tasks(&engine, 2).await;
        assert!(all
            .iter()
            .any(|task| task.workflow_origin.as_ref().unwrap().node_id == "b"));
    }

    #[tokio::test]
    async fn fan_out_starts_together_and_fan_in_waits_for_every_parent() {
        let engine = engine();
        let definition = create(
            &engine,
            vec![node("a"), node("b"), node("c"), node("d")],
            vec![
                edge("a", "b"),
                edge("a", "c"),
                edge("b", "d"),
                edge("c", "d"),
            ],
        )
        .await;
        let run = engine.start_workflow(&definition.id, &Caller::Owner).await.unwrap();
        let root = wait_for_tasks(&engine, 1).await.pop().unwrap();
        finish(&engine, &root.id, RunStatus::Done).await;
        let branch = wait_for_tasks(&engine, 3).await;
        let b = branch
            .iter()
            .find(|task| task.workflow_origin.as_ref().unwrap().node_id == "b")
            .unwrap();
        let c = branch
            .iter()
            .find(|task| task.workflow_origin.as_ref().unwrap().node_id == "c")
            .unwrap();
        finish(&engine, &b.id, RunStatus::Done).await;
        assert_eq!(tasks(&engine).await.len(), 3, "fan-in still waits for c");
        finish(&engine, &c.id, RunStatus::Done).await;
        wait_for_tasks(&engine, 4).await;
        let run = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(
            run.nodes
                .iter()
                .filter(|node| node.task_id.is_some())
                .count(),
            4
        );
    }

    #[tokio::test]
    async fn failure_stops_downstream_nodes_truthfully() {
        let engine = engine();
        let definition = create(&engine, vec![node("a"), node("b")], vec![edge("a", "b")]).await;
        let run = engine.start_workflow(&definition.id, &Caller::Owner).await.unwrap();
        let root = wait_for_tasks(&engine, 1).await.pop().unwrap();
        finish(&engine, &root.id, RunStatus::Failed).await;
        let run = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(run.status, WorkflowRunStatus::Failed);
        assert_eq!(run.failure_node_id.as_deref(), Some("a"));
        assert_eq!(
            run.nodes
                .iter()
                .find(|node| node.node_id == "b")
                .unwrap()
                .status,
            WorkflowNodeStatus::Skipped
        );
        assert_eq!(tasks(&engine).await.len(), 1);
    }

    #[tokio::test]
    async fn blocked_pauses_and_cancelling_settles_active_and_unstarted_nodes() {
        let engine = engine();
        let definition = create(&engine, vec![node("a"), node("b")], vec![edge("a", "b")]).await;
        let run = engine.start_workflow(&definition.id, &Caller::Owner).await.unwrap();
        let root = wait_for_tasks(&engine, 1).await.pop().unwrap();
        finish(&engine, &root.id, RunStatus::Blocked).await;
        let paused = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(paused.status, WorkflowRunStatus::Running);
        assert_eq!(paused.nodes[0].status, WorkflowNodeStatus::Blocked);
        assert_eq!(
            tasks(&engine).await.len(),
            1,
            "blocked never unlocks the child"
        );

        let cancelled = engine.cancel_workflow(&run.id).await.unwrap();
        assert_eq!(cancelled.status, WorkflowRunStatus::Cancelled);
        assert_eq!(
            cancelled
                .nodes
                .iter()
                .find(|node| node.node_id == "a")
                .unwrap()
                .status,
            WorkflowNodeStatus::Cancelled
        );
        assert_eq!(
            cancelled
                .nodes
                .iter()
                .find(|node| node.node_id == "b")
                .unwrap()
                .status,
            WorkflowNodeStatus::Skipped
        );
    }

    #[tokio::test]
    async fn recovery_is_idempotent_and_definition_edits_do_not_change_a_run_snapshot() {
        let engine = engine();
        let definition = create(&engine, vec![node("a")], vec![]).await;
        let run = engine.start_workflow(&definition.id, &Caller::Owner).await.unwrap();
        wait_for_tasks(&engine, 1).await;
        engine.recover_workflows().await;
        engine.recover_workflows().await;
        assert_eq!(
            tasks(&engine).await.len(),
            1,
            "recovery keeps the persisted node-to-task decision"
        );

        let mut changed = WorkflowDraft {
            name: "renamed".into(),
            scope: "demo".into(),
            nodes: vec![node("a")],
            edges: vec![],
            ..Default::default()
        };
        changed.nodes[0].task.title = "changed later".into();
        let updated = engine
            .update_workflow(&definition.id, changed)
            .await
            .unwrap();
        assert_eq!(updated.revision, 2);
        let original = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(original.revision, 1);
        assert_eq!(original.definition.nodes[0].task.title, "a");
    }

    // -- B1: spawn authorization --------------------------------------------

    #[tokio::test]
    async fn starting_a_workflow_needs_the_same_authority_a_manual_spawn_would() {
        let engine = engine_with_roles(
            "instance:\n  id: test\n  name: test\nscopes:\n  - id: demo-id\n    name: demo\n    runtime: quiet\nroles:\n  starter:\n    grants: [workflow.create, workflow.edit, workflow.run]\n    reach: scope\n",
        );
        let definition = create(&engine, vec![node("a")], vec![]).await;
        let error = engine
            .start_workflow(&definition.id, &wearing("starter"))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("create tasks"),
            "a role without task.create is refused: {error}"
        );
        assert!(
            engine.workflows.active_runs().await.unwrap().is_empty(),
            "a denial at start persists no run at all"
        );
        assert!(tasks(&engine).await.is_empty());
    }

    #[tokio::test]
    async fn the_owner_may_start_a_workflow_under_any_role_configuration() {
        let engine = engine_with_roles(
            "instance:\n  id: test\n  name: test\nscopes:\n  - id: demo-id\n    name: demo\n    runtime: quiet\nroles:\n  starter:\n    grants: [workflow.create, workflow.edit, workflow.run]\n    reach: scope\n",
        );
        let definition = create(&engine, vec![node("a")], vec![]).await;
        engine
            .start_workflow(&definition.id, &Caller::Owner)
            .await
            .unwrap();
        wait_for_tasks(&engine, 1).await;
    }

    #[tokio::test]
    async fn a_role_that_loses_task_create_before_a_downstream_spawn_fails_just_that_node() {
        let engine = engine_with_roles(
            "instance:\n  id: test\n  name: test\nscopes:\n  - id: demo-id\n    name: demo\n    runtime: quiet\n    agents:\n      - name: w\n        harness: shell\n        role: starter\nroles:\n  starter:\n    grants: [workflow.create, workflow.edit, workflow.run, task.create, task.run]\n    reach: scope\n  weak:\n    grants: [workflow.create, workflow.edit, workflow.run]\n    reach: scope\n",
        );
        // A store record is needed before `set_agent_role` can find one to
        // change; the record's own `role` field is cosmetic here --
        // `role_with` reads only `assigned_role`, so the config's declared
        // role still wins until something assigns one.
        let agent = AgentSession::new("demo", "w", "shell", "quiet", Lifetime::Task, Role::worker());
        engine.store.put_agent(&agent).await.unwrap();

        let definition = create(&engine, vec![node("a"), node("b")], vec![edge("a", "b")]).await;
        let caller = Caller::Agent {
            scope: "demo".into(),
            name: "w".into(),
            role: engine.effective_role("demo", "w").await,
            run_id: None,
        };
        let run = engine.start_workflow(&definition.id, &caller).await.unwrap();
        let root = wait_for_tasks(&engine, 1).await.pop().unwrap();

        // The role changes while "a" is still in flight -- well after the
        // run started, well before "b" is ever considered.
        engine
            .set_agent_role("demo/w", Some(Role::new("weak")))
            .await
            .unwrap();

        finish(&engine, &root.id, RunStatus::Done).await;

        let run = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(run.status, WorkflowRunStatus::Failed);
        assert_eq!(run.failure_node_id.as_deref(), Some("b"));
        let b = run.nodes.iter().find(|n| n.node_id == "b").unwrap();
        assert_eq!(b.status, WorkflowNodeStatus::Failed);
        assert!(b.task_id.is_none(), "a denial never mints a task id");
        assert!(
            b.error.as_deref().unwrap_or("").contains("create tasks"),
            "{:?}",
            b.error
        );
        assert_eq!(
            tasks(&engine).await.len(),
            1,
            "no task is created for the denied node"
        );
    }

    // -- B2: overlays keep moving after the run is terminal ------------------

    #[tokio::test]
    async fn a_sibling_still_running_when_the_run_fails_still_settles_once_it_finishes() {
        let engine = engine();
        let definition = create(
            &engine,
            vec![node("a"), node("b"), node("c")],
            vec![edge("a", "c")],
        )
        .await;
        let run = engine.start_workflow(&definition.id, &Caller::Owner).await.unwrap();
        let roots = wait_for_tasks(&engine, 2).await;
        let a = roots
            .iter()
            .find(|t| t.workflow_origin.as_ref().unwrap().node_id == "a")
            .unwrap()
            .clone();
        let b = roots
            .iter()
            .find(|t| t.workflow_origin.as_ref().unwrap().node_id == "b")
            .unwrap()
            .clone();

        finish(&engine, &a.id, RunStatus::Failed).await;
        let mid = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(mid.status, WorkflowRunStatus::Failed);
        assert_eq!(mid.failure_node_id.as_deref(), Some("a"));
        assert_eq!(
            mid.nodes.iter().find(|n| n.node_id == "c").unwrap().status,
            WorkflowNodeStatus::Skipped
        );
        assert_ne!(
            mid.nodes.iter().find(|n| n.node_id == "b").unwrap().status,
            WorkflowNodeStatus::Skipped,
            "b was already spawned before the run failed; the sweep must not touch it"
        );

        finish(&engine, &b.id, RunStatus::Done).await;
        let done = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(
            done.status,
            WorkflowRunStatus::Failed,
            "the run keeps its truthful outcome"
        );
        assert_eq!(done.failure_node_id.as_deref(), Some("a"));
        assert_eq!(
            done.nodes.iter().find(|n| n.node_id == "b").unwrap().status,
            WorkflowNodeStatus::Done,
            "b's own overlay still moves with its task even though the run is settled"
        );
    }

    // -- B3: a task deleted out from under an active node ---------------------

    #[tokio::test]
    async fn a_task_deleted_out_from_under_a_node_fails_it_and_settles_the_run() {
        let engine = engine();
        let definition = create(&engine, vec![node("a"), node("b")], vec![edge("a", "b")]).await;
        let run = engine.start_workflow(&definition.id, &Caller::Owner).await.unwrap();
        let root = wait_for_tasks(&engine, 1).await.pop().unwrap();

        engine.store.delete(&root.id).await.unwrap();
        engine.advance_workflow(&run.id).await.unwrap();

        let run = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(run.status, WorkflowRunStatus::Failed);
        assert_eq!(run.failure_node_id.as_deref(), Some("a"));
        let a = run.nodes.iter().find(|n| n.node_id == "a").unwrap();
        assert_eq!(a.status, WorkflowNodeStatus::Failed);
        assert!(
            a.error.as_deref().unwrap_or("").contains("no longer exists"),
            "{:?}",
            a.error
        );
        assert_eq!(
            run.nodes.iter().find(|n| n.node_id == "b").unwrap().status,
            WorkflowNodeStatus::Skipped
        );
        assert_eq!(tasks(&engine).await.len(), 0);
    }

    // -- B6: further execution coverage the issue asks for --------------------

    #[tokio::test]
    async fn every_spawned_task_carries_provenance_and_a_plain_task_carries_none() {
        let engine = engine();
        let definition = create(&engine, vec![node("a")], vec![]).await;
        let run = engine.start_workflow(&definition.id, &Caller::Owner).await.unwrap();
        let spawned = wait_for_tasks(&engine, 1).await.pop().unwrap();
        let origin = spawned.workflow_origin.as_ref().unwrap();
        assert_eq!(origin.workflow_id, definition.id);
        assert_eq!(origin.workflow_run_id, run.id);
        assert_eq!(origin.node_id, "a");

        let plain = engine
            .create(NewTask {
                title: "ordinary".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                runtime: Some("quiet".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(plain.workflow_origin.is_none());
    }

    #[tokio::test]
    async fn replaying_advance_after_done_never_creates_a_child_twice() {
        let engine = engine();
        let definition = create(&engine, vec![node("a"), node("b")], vec![edge("a", "b")]).await;
        let run = engine.start_workflow(&definition.id, &Caller::Owner).await.unwrap();
        let root = wait_for_tasks(&engine, 1).await.pop().unwrap();
        finish(&engine, &root.id, RunStatus::Done).await;
        wait_for_tasks(&engine, 2).await;

        for _ in 0..5 {
            engine.advance_workflow(&run.id).await.unwrap();
            engine.sync_workflow_for_task(&root.id).await;
        }
        assert_eq!(
            tasks(&engine).await.len(),
            2,
            "replayed advances never duplicate a child"
        );
    }

    #[tokio::test]
    async fn cancelling_an_actively_running_task_uses_the_cancellation_path_and_keeps_history() {
        let engine = engine();
        let definition = create(&engine, vec![node("a"), node("b")], vec![edge("a", "b")]).await;
        let run = engine.start_workflow(&definition.id, &Caller::Owner).await.unwrap();
        let root = wait_for_tasks(&engine, 1).await.pop().unwrap();
        let active = loop {
            if let Some(r) = engine.store.active_run(&root.id).await.unwrap() {
                break r;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        engine
            .report(
                &root.id,
                TaskReport {
                    status: Some(RunStatus::Running),
                    message: Some("working".into()),
                    result: None,
                    error: None,
                    token: active.token.clone(),
                },
            )
            .await
            .unwrap();
        engine.sync_workflow_for_task(&root.id).await;

        let cancelled = engine.cancel_workflow(&run.id).await.unwrap();
        assert_eq!(cancelled.status, WorkflowRunStatus::Cancelled);
        assert_eq!(
            cancelled.nodes.iter().find(|n| n.node_id == "a").unwrap().status,
            WorkflowNodeStatus::Cancelled
        );
        assert_eq!(
            cancelled.nodes.iter().find(|n| n.node_id == "b").unwrap().status,
            WorkflowNodeStatus::Skipped
        );

        let task = engine.store.get(&root.id).await.unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Cancelled, "task history is kept, not deleted");
        assert!(
            !engine.store.runs(&root.id, 10).await.unwrap().is_empty(),
            "run history is kept, not deleted"
        );
    }

    #[tokio::test]
    async fn a_node_naming_an_unknown_agent_fails_that_node_and_the_run() {
        let engine = engine();
        let mut bad = node("a");
        bad.task.agent = Some("does-not-exist".into());
        let definition = create(&engine, vec![bad], vec![]).await;
        let run = engine.start_workflow(&definition.id, &Caller::Owner).await.unwrap();
        assert_eq!(run.status, WorkflowRunStatus::Failed);
        assert_eq!(run.failure_node_id.as_deref(), Some("a"));
        let a = run.nodes.iter().find(|n| n.node_id == "a").unwrap();
        assert!(a.task_id.is_none());
        assert!(a.error.as_deref().unwrap_or("").contains("no agent named"));
        assert_eq!(tasks(&engine).await.len(), 0);
    }

    #[tokio::test]
    async fn restart_recovery_against_real_files_never_duplicates_a_task() {
        let root =
            std::env::temp_dir().join(format!("factory-workflow-restart-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let db_path = root.join("factory.db");

        fn build(root: &std::path::Path, db_path: &std::path::Path) -> Arc<Engine> {
            let config = Config {
                version: 1,
                instance: Instance {
                    id: "test".into(),
                    name: "test".into(),
                },
                daemon: DaemonConfig::default(),
                roles: Default::default(),
                scope: None,
                scopes: vec![Scope {
                    id: "demo-id".into(),
                    name: "demo".into(),
                    path: PathBuf::new(),
                    agent: None,
                    agents: Vec::new(),
                    runtime: Some("quiet".into()),
                    git: None,
                    task_store: None,
                }],
                plugins_dir: None,
            };
            let mut registry = Registry::with_builtins();
            registry.add_runtime(Arc::new(QuietRuntime), "test");
            Arc::new(
                Engine::new(
                    Factory {
                        root: root.to_path_buf(),
                        config,
                    },
                    registry,
                    Arc::new(SqliteStore::open(db_path).unwrap()),
                    PathBuf::from("factory"),
                    Vec::new(),
                )
                .with_workflow_store(crate::workflows::WorkflowStore::open(db_path).unwrap()),
            )
        }

        let engine1 = build(&root, &db_path);
        let definition = create(&engine1, vec![node("a"), node("b")], vec![edge("a", "b")]).await;
        let run = engine1
            .start_workflow(&definition.id, &Caller::Owner)
            .await
            .unwrap();
        let root_task = wait_for_tasks(&engine1, 1).await.pop().unwrap();
        // Let the spawned dispatch -- and the `record_workflow_task_state`
        // mirror `start_run` performs right behind it -- actually land
        // before pulling the rug out, so the "crash" is a clean restart
        // rather than a torn write. Dropping `engine1` here does not cancel
        // that spawned task (it holds its own clone of the engine, same as
        // any dispatch would); waiting for the node's own status to move
        // off `Pending` is waiting for that write to have happened, since
        // nothing else touches it in between.
        loop {
            let node = engine1.workflow_run(&run.id).await.unwrap();
            if node.nodes.iter().any(|n| n.node_id == "a" && n.status != WorkflowNodeStatus::Pending) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        drop(engine1);

        let engine2 = build(&root, &db_path);
        engine2.recover_workflows().await;
        engine2.recover_workflows().await;
        assert_eq!(
            tasks(&engine2).await.len(),
            1,
            "recovery never recreates the root"
        );

        finish(&engine2, &root_task.id, RunStatus::Done).await;
        let all = wait_for_tasks(&engine2, 2).await;
        let b = all
            .iter()
            .find(|t| t.workflow_origin.as_ref().unwrap().node_id == "b")
            .unwrap()
            .clone();
        finish(&engine2, &b.id, RunStatus::Done).await;

        let finished = engine2.workflow_run(&run.id).await.unwrap();
        assert_eq!(finished.status, WorkflowRunStatus::Done);
    }

    #[tokio::test]
    async fn a_decision_persisted_with_no_task_yet_is_recreated_with_that_exact_id_once() {
        let engine = engine();
        let definition = create(&engine, vec![node("a")], vec![]).await;

        // Simulate the crash window directly: the node's decision (a task id)
        // is persisted, but the task itself was never created.
        let phantom_id = uuid::Uuid::new_v4().to_string();
        let mut run = WorkflowRun::new(definition.clone(), WorkflowActor::Owner);
        run.nodes[0].task_id = Some(phantom_id.clone());
        run.nodes[0].status = WorkflowNodeStatus::Pending;
        engine.workflows.put_run(&run).await.unwrap();

        engine.recover_workflows().await;
        let created = wait_for_tasks(&engine, 1).await;
        assert_eq!(created[0].id, phantom_id, "recovery fills in exactly the persisted id");

        engine.recover_workflows().await;
        assert_eq!(tasks(&engine).await.len(), 1, "a second recovery does not duplicate it");
    }
}

fn node_status(status: TaskStatus) -> WorkflowNodeStatus {
    match status {
        TaskStatus::Pending => WorkflowNodeStatus::Pending,
        TaskStatus::Dispatching => WorkflowNodeStatus::Dispatching,
        TaskStatus::Running => WorkflowNodeStatus::Running,
        TaskStatus::Blocked => WorkflowNodeStatus::Blocked,
        TaskStatus::Done => WorkflowNodeStatus::Done,
        TaskStatus::Failed => WorkflowNodeStatus::Failed,
        TaskStatus::Cancelled => WorkflowNodeStatus::Cancelled,
    }
}

impl Engine {
    pub(crate) async fn workflow_definition(&self, id: &str) -> Result<WorkflowDefinition> {
        self.workflows
            .get_definition(id)
            .await?
            .ok_or_else(|| missing("workflow", id))
    }

    pub(crate) async fn workflow_run(&self, id: &str) -> Result<WorkflowRun> {
        self.workflows
            .get_run(id)
            .await?
            .ok_or_else(|| missing("workflow run", id))
    }

    pub(crate) async fn create_workflow(&self, draft: WorkflowDraft) -> Result<WorkflowDefinition> {
        let factory = self.factory_snapshot();
        let scope = factory.scope(&draft.scope)?.name.clone();
        let mut draft = draft;
        draft.scope = scope.clone();
        for node in &mut draft.nodes {
            if node.task.scope.is_none() {
                node.task.scope = Some(scope.clone());
            }
        }
        let definition = WorkflowDefinition::from_draft(draft);
        definition.validate().map_err(FactoryError::BadRequest)?;
        self.workflows.put_definition(&definition).await?;
        self.bus.publish(Event::WorkflowCreated {
            workflow: definition.clone(),
        });
        Ok(definition)
    }

    pub(crate) async fn update_workflow(
        &self,
        id: &str,
        draft: WorkflowDraft,
    ) -> Result<WorkflowDefinition> {
        let mut definition = self.workflow_definition(id).await?;
        if draft.scope != definition.scope {
            return Err(FactoryError::BadRequest(
                "a workflow cannot move to another scope".into(),
            ));
        }
        let mut draft = draft;
        for node in &mut draft.nodes {
            if node.task.scope.is_none() {
                node.task.scope = Some(definition.scope.clone());
            }
        }
        definition.apply(draft);
        definition.validate().map_err(FactoryError::BadRequest)?;
        self.workflows.put_definition(&definition).await?;
        self.bus.publish(Event::WorkflowUpdated {
            workflow: definition.clone(),
        });
        Ok(definition)
    }

    pub(crate) async fn delete_workflow(&self, id: &str) -> Result<bool> {
        let deleted = self.workflows.delete_definition(id).await?;
        if deleted {
            self.bus
                .publish(Event::WorkflowDeleted { id: id.to_string() });
        }
        Ok(deleted)
    }

    pub(crate) async fn start_workflow(
        self: &Arc<Self>,
        id: &str,
        caller: &Caller,
    ) -> Result<WorkflowRun> {
        let definition = self.workflow_definition(id).await?;
        definition.validate().map_err(FactoryError::BadRequest)?;
        // Every node must be something this caller could `task.create` and
        // `task.run` by hand, checked before anything is persisted -- a
        // `workflow.run` grant is not a way to launder a caller into
        // authority over tasks it could not otherwise touch. See
        // `Engine::authorize_workflow_spawn`.
        for node in &definition.nodes {
            self.authorize_workflow_spawn(caller, &node.task).await?;
        }
        let run = WorkflowRun::new(definition, caller.as_workflow_actor());
        self.workflows.put_run(&run).await?;
        self.bus
            .publish(Event::WorkflowRunUpdated { run: run.clone() });
        self.advance_workflow(&run.id).await?;
        self.workflow_run(&run.id).await
    }

    pub(crate) async fn cancel_workflow(self: &Arc<Self>, id: &str) -> Result<WorkflowRun> {
        let task_ids = {
            let _guard = self.workflow_edit.lock().await;
            let mut run = self.workflow_run(id).await?;
            if run.status.is_terminal() {
                return Ok(run);
            }
            run.status = WorkflowRunStatus::Cancelled;
            run.updated_at = Utc::now();
            let mut task_ids = Vec::new();
            for node in &mut run.nodes {
                if !node.status.is_terminal() {
                    if let Some(task_id) = &node.task_id {
                        task_ids.push(task_id.clone());
                    } else {
                        node.status = WorkflowNodeStatus::Skipped;
                    }
                }
            }
            self.workflows.put_run(&run).await?;
            self.bus
                .publish(Event::WorkflowRunUpdated { run: run.clone() });
            task_ids
        };
        for task_id in task_ids {
            if self.store.active_run(&task_id).await?.is_some() {
                let _ = self.cancel_task_run(&task_id).await;
            }
        }
        let mut run = self.workflow_run(id).await?;
        for node in &mut run.nodes {
            if let Some(task_id) = &node.task_id {
                if let Some(task) = self.store.get(task_id).await? {
                    node.status = node_status(task.status);
                    node.error = task.error;
                }
            }
        }
        run.updated_at = Utc::now();
        self.workflows.put_run(&run).await?;
        self.bus
            .publish(Event::WorkflowRunUpdated { run: run.clone() });
        Ok(run)
    }

    /// Reconcile persisted node decisions with authoritative task state, then
    /// make every newly eligible decision. The mutex serializes reports,
    /// cancellation and restart recovery so one node can never be chosen
    /// twice, and so `recover_workflows`'s repair of a node never races this.
    pub(crate) async fn advance_workflow(self: &Arc<Self>, id: &str) -> Result<()> {
        let _guard = self.workflow_edit.lock().await;
        let mut run = self.workflow_run(id).await?;

        // Mirror authoritative task state into every node that has one, even
        // once the run itself is terminal. A sibling still running when its
        // neighbour failed does not freeze mid-flight forever just because
        // the workflow gave up on the run as a whole -- see issue #45's node
        // overlay requirement and the README.
        for node in &mut run.nodes {
            let Some(task_id) = node.task_id.clone() else {
                continue;
            };
            match self.store.get(&task_id).await {
                Ok(Some(task)) => {
                    node.status = node_status(task.status);
                    node.error = task.error;
                }
                Ok(None) if !node.status.is_terminal() => {
                    // The decision was made (or a task once existed) and now
                    // there is nothing at that id. Outside the crash window
                    // `recover_workflows` repairs -- which never overlaps
                    // this, since both hold `workflow_edit` across the
                    // persist-then-create pair and recovery's own repair
                    // runs before its `advance_workflow` call -- that is a
                    // fact about the task (deleted out from under the node),
                    // not a race to paper over.
                    node.status = WorkflowNodeStatus::Failed;
                    node.error = Some(format!(
                        "the task spawned for this node ({task_id}) no longer exists"
                    ));
                }
                Ok(None) => {} // already terminal; a vanished task changes nothing more
                Err(error) => {
                    // A store hiccup is not evidence the task is gone; do not
                    // let a transient read failure fail the node.
                    tracing::warn!(
                        workflow_run = id,
                        node = node.node_id,
                        task = task_id,
                        "could not read spawned task: {error}"
                    );
                }
            }
        }

        if run.status.is_terminal() {
            // A terminal run never spawns again, and nothing above may
            // rewrite its own status or failure node -- only the per-node
            // mirror does, and that alone is worth persisting and publishing.
            run.updated_at = Utc::now();
            self.workflows.put_run(&run).await?;
            self.bus.publish(Event::WorkflowRunUpdated { run });
            return Ok(());
        }

        if let Some(failed) = run
            .nodes
            .iter()
            .find(|n| n.status == WorkflowNodeStatus::Failed)
        {
            run.status = WorkflowRunStatus::Failed;
            run.failure_node_id = Some(failed.node_id.clone());
            run.error = failed
                .error
                .clone()
                .or_else(|| Some("a task node failed".into()));
        } else if let Some(cancelled) = run
            .nodes
            .iter()
            .find(|n| n.status == WorkflowNodeStatus::Cancelled)
        {
            run.status = WorkflowRunStatus::Cancelled;
            run.failure_node_id = Some(cancelled.node_id.clone());
        } else if run
            .nodes
            .iter()
            .all(|node| node.status == WorkflowNodeStatus::Done)
        {
            run.status = WorkflowRunStatus::Done;
        }
        if run.status.is_terminal() {
            for node in &mut run.nodes {
                if node.status == WorkflowNodeStatus::Unstarted {
                    node.status = WorkflowNodeStatus::Skipped;
                }
            }
            run.updated_at = Utc::now();
            self.workflows.put_run(&run).await?;
            self.bus.publish(Event::WorkflowRunUpdated { run });
            return Ok(());
        }

        let eligible: Vec<String> = run
            .nodes
            .iter()
            .filter(|node| node.status == WorkflowNodeStatus::Unstarted)
            .filter(|node| {
                run.definition
                    .edges
                    .iter()
                    .filter(|edge| edge.to == node.node_id)
                    .all(|edge| {
                        run.nodes
                            .iter()
                            .find(|parent| parent.node_id == edge.from)
                            .map(|parent| parent.status == WorkflowNodeStatus::Done)
                            .unwrap_or(false)
                    })
            })
            .map(|node| node.node_id.clone())
            .collect();

        let mut to_start = Vec::new();
        for node_id in eligible {
            // A denial or creation failure earlier in this very pass already
            // ended the run; nothing else in this fan-out batch gets to
            // start. The rest keep reading `unstarted` until the sweep below.
            if run.status.is_terminal() {
                break;
            }

            let template = run
                .definition
                .nodes
                .iter()
                .find(|node| node.id == node_id)
                .expect("run nodes come from the snapshot")
                .task
                .clone();

            // Re-resolve who this run runs for, every time: a role can
            // change between the click that started it and a node it spawns
            // long afterward, and the owner's authority never needs this at
            // all. See `WorkflowActor` and `authorize_workflow_spawn`.
            let caller = self.caller_for_actor(&run.started_by).await;
            if let Err(denial) = self.authorize_workflow_spawn(&caller, &template).await {
                let node = run
                    .nodes
                    .iter_mut()
                    .find(|node| node.node_id == node_id)
                    .expect("snapshot node");
                node.status = WorkflowNodeStatus::Failed;
                node.error = Some(denial.to_string());
                run.status = WorkflowRunStatus::Failed;
                run.failure_node_id = Some(node_id);
                run.error = Some(denial.to_string());
                continue;
            }

            let task_id = uuid::Uuid::new_v4().to_string();
            let node_run = run
                .nodes
                .iter_mut()
                .find(|node| node.node_id == node_id)
                .expect("snapshot node");
            node_run.task_id = Some(task_id.clone());
            node_run.status = WorkflowNodeStatus::Pending;
            run.updated_at = Utc::now();
            // Persist the decision before creating or publishing the task. A
            // restart can fill in this exact id; it must never choose another.
            self.workflows.put_run(&run).await?;
            self.bus
                .publish(Event::WorkflowRunUpdated { run: run.clone() });

            let origin = WorkflowOrigin {
                workflow_id: run.workflow_id.clone(),
                workflow_run_id: run.id.clone(),
                node_id: node_id.clone(),
            };
            match self
                .create_workflow_task(template, origin, task_id.clone())
                .await
            {
                Ok(_) => to_start.push(task_id),
                Err(error) => {
                    let node = run
                        .nodes
                        .iter_mut()
                        .find(|node| node.node_id == node_id)
                        .unwrap();
                    // No task exists at this id and none ever will: leaving
                    // it set would make `recover_workflows` try to create it
                    // again after every future restart.
                    node.task_id = None;
                    node.status = WorkflowNodeStatus::Failed;
                    node.error = Some(error.to_string());
                    run.status = WorkflowRunStatus::Failed;
                    run.failure_node_id = Some(node_id);
                    run.error = Some(error.to_string());
                }
            }
        }
        // A denial or creation failure inside the loop above is a fresh
        // transition to terminal within this same pass; give it the same
        // skip-sweep the early-return branches give theirs, so a fan-out
        // sibling that never got its turn this pass reads `skipped` rather
        // than lingering `unstarted`.
        if run.status.is_terminal() {
            for node in &mut run.nodes {
                if node.status == WorkflowNodeStatus::Unstarted {
                    node.status = WorkflowNodeStatus::Skipped;
                }
            }
        }
        run.updated_at = Utc::now();
        self.workflows.put_run(&run).await?;
        self.bus
            .publish(Event::WorkflowRunUpdated { run: run.clone() });
        drop(_guard);
        for task_id in to_start {
            let engine = self.clone();
            tokio::spawn(async move {
                engine.start_run(&task_id, Trigger::Workflow).await;
            });
        }
        Ok(())
    }

    pub(crate) async fn sync_workflow_for_task(self: &Arc<Self>, task_id: &str) {
        let Ok(Some(task)) = self.store.get(task_id).await else {
            return;
        };
        let Some(origin) = task.workflow_origin else {
            return;
        };
        if let Err(error) = self.advance_workflow(&origin.workflow_run_id).await {
            tracing::warn!(
                workflow_run = origin.workflow_run_id,
                task = task_id,
                "could not advance workflow: {error}"
            );
        }
    }

    /// Mirror dispatch progress without recursively advancing the graph. The
    /// report/cancel paths perform advancement; this hook exists so a launch
    /// failure still settles the workflow even though no agent can report it.
    ///
    /// Runs even once the run is terminal, for the same reason
    /// `advance_workflow`'s mirror does: the node's own status still moves
    /// with its task. Only the settle step below -- promoting a freshly
    /// failed node into the run's own failure -- is skipped once the run
    /// already has an outcome, so a terminal run's status and failure node
    /// are never rewritten here either.
    pub(crate) async fn record_workflow_task_state(&self, task_id: &str) {
        let Ok(Some(task)) = self.store.get(task_id).await else {
            return;
        };
        let Some(origin) = task.workflow_origin else {
            return;
        };
        let _guard = self.workflow_edit.lock().await;
        let Ok(mut run) = self.workflow_run(&origin.workflow_run_id).await else {
            return;
        };
        let Some(node) = run
            .nodes
            .iter_mut()
            .find(|node| node.node_id == origin.node_id)
        else {
            return;
        };
        node.status = node_status(task.status);
        node.error = task.error;
        if !run.status.is_terminal() && node.status == WorkflowNodeStatus::Failed {
            run.status = WorkflowRunStatus::Failed;
            run.failure_node_id = Some(node.node_id.clone());
            run.error = node
                .error
                .clone()
                .or_else(|| Some("task dispatch failed".into()));
            for other in &mut run.nodes {
                if other.status == WorkflowNodeStatus::Unstarted {
                    other.status = WorkflowNodeStatus::Skipped;
                }
            }
        }
        run.updated_at = Utc::now();
        if self.workflows.put_run(&run).await.is_ok() {
            self.bus.publish(Event::WorkflowRunUpdated { run });
        }
    }

    /// Restart recovery is a reconciliation, not a replay: persisted task ids
    /// win. Missing tasks are recreated with those ids and pending tasks with
    /// no attempt are dispatched once.
    pub(crate) async fn recover_workflows(self: &Arc<Self>) {
        let runs = match self.workflows.active_runs().await {
            Ok(runs) => runs,
            Err(error) => {
                tracing::warn!("could not load workflow runs: {error}");
                return;
            }
        };
        for mut run in runs {
            // Repair a decision persisted just before task creation. This
            // walks the run outside `workflow_edit` -- recovery runs once,
            // sequentially, before anything else can be dispatching a report
            // for a run it has not reached yet -- but always before this
            // same run's own `advance_workflow` call below takes that lock,
            // so nothing here can race the mirror it performs.
            let mut repaired = false;
            for node in run.nodes.clone() {
                let Some(task_id) = &node.task_id else {
                    continue;
                };
                match self.store.get(task_id).await {
                    Ok(Some(_)) => continue, // already exists; nothing to repair
                    Ok(None) => {}
                    Err(error) => {
                        // Not evidence the task is gone -- a transient read
                        // failure must never spawn a duplicate (see B4/B3).
                        tracing::warn!(
                            workflow_run = run.id,
                            node = node.node_id,
                            task = task_id,
                            "could not check for a recovered workflow task: {error}"
                        );
                        continue;
                    }
                }
                let Some(template) = run
                    .definition
                    .nodes
                    .iter()
                    .find(|item| item.id == node.node_id)
                    .map(|item| item.task.clone())
                else {
                    continue;
                };
                // Re-authorize exactly as a live spawn would: the actor
                // recorded on the run may have lost the grant it started
                // with while the daemon was down.
                let caller = self.caller_for_actor(&run.started_by).await;
                if let Err(denial) = self.authorize_workflow_spawn(&caller, &template).await {
                    tracing::warn!(
                        workflow_run = run.id,
                        node = node.node_id,
                        "denying recovered workflow spawn: {denial}"
                    );
                    if let Some(current) =
                        run.nodes.iter_mut().find(|item| item.node_id == node.node_id)
                    {
                        // No task will ever exist at this id; leaving it set
                        // would have this same repair retry forever.
                        current.task_id = None;
                        current.status = WorkflowNodeStatus::Failed;
                        current.error = Some(denial.to_string());
                    }
                    repaired = true;
                    continue;
                }
                let origin = WorkflowOrigin {
                    workflow_id: run.workflow_id.clone(),
                    workflow_run_id: run.id.clone(),
                    node_id: node.node_id.clone(),
                };
                if let Err(error) = self
                    .create_workflow_task(template, origin, task_id.clone())
                    .await
                {
                    tracing::warn!(
                        workflow_run = run.id,
                        node = node.node_id,
                        "could not recover workflow task: {error}"
                    );
                    if let Some(current) =
                        run.nodes.iter_mut().find(|item| item.node_id == node.node_id)
                    {
                        current.task_id = None;
                        current.status = WorkflowNodeStatus::Failed;
                        current.error = Some(error.to_string());
                    }
                    repaired = true;
                }
            }
            if repaired {
                run.updated_at = Utc::now();
                if let Err(error) = self.workflows.put_run(&run).await {
                    tracing::warn!(
                        workflow_run = run.id,
                        "could not persist a recovered workflow denial: {error}"
                    );
                } else {
                    self.bus
                        .publish(Event::WorkflowRunUpdated { run: run.clone() });
                }
            }
            if let Err(error) = self.advance_workflow(&run.id).await {
                tracing::warn!(workflow_run = run.id, "could not recover workflow: {error}");
            }
            // A task that exists but never acquired a run is the other side
            // of the same crash window.
            if let Ok(current) = self.workflow_run(&run.id).await {
                for node in current.nodes {
                    let Some(task_id) = node.task_id else {
                        continue;
                    };
                    if let Ok(Some(task)) = self.store.get(&task_id).await {
                        if task.status == TaskStatus::Pending
                            && task.runs == 0
                            && self
                                .store
                                .active_run(&task_id)
                                .await
                                .ok()
                                .flatten()
                                .is_none()
                        {
                            let engine = self.clone();
                            tokio::spawn(async move {
                                engine.start_run(&task_id, Trigger::Workflow).await;
                            });
                        }
                    }
                }
            }
        }
    }
}
