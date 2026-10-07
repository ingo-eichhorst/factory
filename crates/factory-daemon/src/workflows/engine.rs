use crate::access::Caller;
use crate::engine::Engine;
use crate::verification::run_shell_capture;
#[cfg(not(test))]
use crate::verification::DEFAULT_GATE_TIMEOUT_SECS;
use crate::worktree;
use chrono::Utc;
use factory_core::control_plan::AttestationVerdict;
use factory_core::error::{FactoryError, Result};
use factory_core::event::Event;
use factory_core::intake::{Routing, SplitPart};
#[cfg(test)]
use factory_core::run::FailKind;
use factory_core::run::{RunStatus, Trigger};
use factory_core::task::{Task, TaskEntry, TaskPatch, TaskStatus, WorkflowOrigin};
use factory_core::workflow::{
    CanvasPoint, ExpandCancelPolicy, ExpandJoin, ExpandSpec, IntegrationPart, ReworkRequest, SendBack,
    WorkflowDefinition, WorkflowDraft, WorkflowEdge, WorkflowIntegration, WorkflowNode, WorkflowNodeKind,
    WorkflowNodeStatus, WorkflowRun, WorkflowRunStatus,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(not(test))]
const EXIT_CHECK_TIMEOUT_SECS: u64 = DEFAULT_GATE_TIMEOUT_SECS;
#[cfg(test)]
const EXIT_CHECK_TIMEOUT_SECS: u64 = 1;

fn missing(kind: &str, id: &str) -> FactoryError {
    FactoryError::BadRequest(format!("no such {kind}: {id}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_core::adapter::agent::UpstreamOutput;
    use factory_core::adapter::{AgentRuntime, StartRequest};
    use factory_core::agent::{AgentSession, Lifetime};
    use factory_core::config::{Config, DaemonConfig, Factory, Instance, Scope};
    use factory_core::role::Role;
    use factory_core::run::RunStatus;
    use factory_core::task::{NewTask, SessionRef, TaskFilter, TaskReport};
    use factory_core::workflow::{
        CanvasPoint, WorkflowActor, WorkflowEdge, WorkflowExit, WorkflowNode, WorkflowNodeKind,
    };
    use factory_plugins::{Registry, SqliteStore};
    use std::path::PathBuf;

    #[derive(Default)]
    struct QuietRuntime {
        stopped: std::sync::Mutex<std::collections::HashSet<String>>,
    }
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
        async fn status(&self, session: &SessionRef) -> Result<factory_core::adapter::RuntimeStatus> {
            Ok(if self.stopped.lock().unwrap().contains(&session.handle) {
                factory_core::adapter::RuntimeStatus::Gone
            } else { factory_core::adapter::RuntimeStatus::Working })
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
        async fn stop(&self, session: &SessionRef) -> Result<()> {
            self.stopped.lock().unwrap().insert(session.handle.clone());
            Ok(())
        }
    }

    /// Like `QuietRuntime`, but remembers every prompt handed to `submit`,
    /// keyed by the session handle -- `start` sets that to the run id, which
    /// is the only way a test can pull back the exact text dispatch built for
    /// one particular run, upstream section (or, for `shell`, the
    /// `FACTORY_UPSTREAM_FILE` export) included.
    struct RecordingRuntime {
        prompts: std::sync::Mutex<Vec<(String, String)>>,
    }

    impl RecordingRuntime {
        fn new() -> Self {
            Self { prompts: std::sync::Mutex::new(Vec::new()) }
        }

        fn prompt_for(&self, session_handle: &str) -> Option<String> {
            self.prompts
                .lock()
                .unwrap()
                .iter()
                .find(|(handle, _)| handle == session_handle)
                .map(|(_, text)| text.clone())
        }
    }

    #[async_trait::async_trait]
    impl AgentRuntime for RecordingRuntime {
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
        async fn submit(&self, session: &SessionRef, text: &str) -> Result<()> {
            self.prompts.lock().unwrap().push((session.handle.clone(), text.to_string()));
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
            // `power_assertion` off -- these tests dispatch real runs
            // through the real `Engine`, and the default would fork a
            // real `caffeinate` on whatever machine runs the tests. The
            // harness probe off too: a `claude-code` node here is about the
            // prompt it is built, and must not depend on `claude` being
            // installed where the tests run (#131; `harness_health` has its
            // own tests, against fake harnesses).
            daemon: DaemonConfig {
                power_assertion: false,
                harness_health: factory_core::config::HarnessHealthConfig {
                    enabled: false,
                    ..Default::default()
                },
                ..DaemonConfig::default()
            },
            roles: Default::default(),
            dashboard: None,
            policies: Default::default(),
            quality: Default::default(),
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
                max_sessions: None,
                roles: Default::default(),
                dashboard: None,
                policies: Default::default(),
                quality: Default::default(),
                intake: Default::default(),
                dependencies: Default::default(),
                environments: Vec::new(),
                renewals: Vec::new(),
                metrics: None,
                backup: None,
            }],
            infrastructure: Default::default(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let mut registry = Registry::with_builtins();
        registry.add_runtime(Arc::new(QuietRuntime::default()), "test");
        Arc::new(Engine::new(
            Factory { root, config },
            registry,
            Arc::new(SqliteStore::in_memory().unwrap()),
            PathBuf::from("factory"),
            Vec::new(),
        ))
    }

    fn engine_in_git_scope(root: PathBuf, repo: PathBuf) -> Arc<Engine> {
        let config = git_scope_config(repo);
        let mut registry = Registry::with_builtins();
        registry.add_runtime(Arc::new(QuietRuntime::default()), "test");
        Arc::new(Engine::new(
            Factory { root, config },
            registry,
            Arc::new(SqliteStore::in_memory().unwrap()),
            PathBuf::from("factory"),
            Vec::new(),
        ))
    }

    /// One `demo` scope at `repo`, on the quiet runtime.
    fn git_scope_config(repo: PathBuf) -> Config {
        Config {
            version: 1,
            instance: Instance {
                id: "test".into(),
                name: "test".into(),
            },
            daemon: DaemonConfig {
                power_assertion: false,
                harness_health: factory_core::config::HarnessHealthConfig {
                    enabled: false,
                    ..Default::default()
                },
                ..DaemonConfig::default()
            },
            roles: Default::default(),
            dashboard: None,
            policies: Default::default(),
            quality: Default::default(),
            scope: None,
            scopes: vec![Scope {
                id: "demo-id".into(),
                name: "demo".into(),
                path: repo,
                agent: None,
                agents: Vec::new(),
                runtime: Some("quiet".into()),
                git: None,
                task_store: None,
                max_sessions: None,
                roles: Default::default(),
                dashboard: None,
                policies: Default::default(),
                quality: Default::default(),
                intake: Default::default(),
                dependencies: Default::default(),
                environments: Vec::new(),
                renewals: Vec::new(),
                metrics: None,
                backup: None,
            }],
            infrastructure: Default::default(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        }
    }

    async fn wait_for_worktree_run(engine: &Engine, task_id: &str) -> factory_core::Run {
        for _ in 0..200 {
            if let Some(run) = engine.l4.store.active_run(task_id).await.unwrap() {
                if run.worktree_path.is_some() && run.token.is_some() && run.session.is_some() {
                    return run;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("task {task_id} never acquired a worktree; task {:?}, runs {:?}", engine.require(task_id).await.unwrap(), engine.l4.store.runs(task_id, 10).await.unwrap());
    }

    async fn git_ok(dir: &Path, args: &[&str]) {
        let output = tokio::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[tokio::test]
    async fn a_workflow_revision_is_resolved_before_tasks_exist_and_cannot_move_with_its_ref() {
        let root = std::env::temp_dir().join(format!("factory-pinned-workflow-{}", uuid::Uuid::new_v4()));
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git_ok(&repo, &["init", "-q"]).await;
        git_ok(&repo, &["config", "user.name", "Pinned workflow QA"]).await;
        git_ok(&repo, &["config", "user.email", "pinned@example.invalid"]).await;
        git_ok(&repo, &["commit", "-q", "--allow-empty", "-m", "selected"]).await;
        git_ok(&repo, &["branch", "selected"]).await;
        let engine = engine_in_git_scope(root.clone(), repo.clone());
        let mut task = node("pinned");
        task.task.worktree = Some(true);
        let mut draft = WorkflowDraft { name: "pinned".into(), scope: "demo".into(), workspace_ref: Some("selected".into()), nodes: vec![task], ..Default::default() };
        let mut definition = WorkflowDefinition::from_draft(draft.clone());
        engine.freeze_workflow_workspace(&mut definition).await.unwrap();
        let selected = definition.workspace_ref.clone().unwrap();
        assert_eq!(selected.len(), 40);
        git_ok(&repo, &["commit", "-q", "--allow-empty", "-m", "later"]).await;
        git_ok(&repo, &["branch", "-f", "selected", "HEAD"]).await;
        engine.freeze_workflow_workspace(&mut definition).await.unwrap();
        assert_eq!(definition.workspace_ref.as_deref(), Some(selected.as_str()));
        let mut later = WorkflowDefinition::from_draft(draft.clone());
        engine.freeze_workflow_workspace(&mut later).await.unwrap();
        assert_ne!(later.workspace_ref, definition.workspace_ref);
        draft.nodes[0].task.worktree = Some(false);
        assert!(engine.freeze_workflow_workspace(&mut WorkflowDefinition::from_draft(draft.clone())).await.unwrap_err().to_string().contains("isolated"));
        draft.nodes[0].task.worktree = Some(true);
        draft.workspace_ref = Some("missing-ref".into());
        assert!(engine.freeze_workflow_workspace(&mut WorkflowDefinition::from_draft(draft)).await.is_err());
        assert!(engine.l4.store.list(&TaskFilter::default()).await.unwrap().is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    /// Like `engine()`, but the instance names its own roles and scope
    /// agents -- for the authorization tests, where the built-in presets
    /// grant more than the scenario wants to hold constant.
    fn engine_with_roles(yaml: &str) -> Arc<Engine> {
        let root =
            std::env::temp_dir().join(format!("factory-workflow-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let mut config: Config = serde_yaml_ng::from_str(yaml).unwrap();
        // None of these fixtures write a `daemon:` block, so this would
        // otherwise default on -- and some of these tests dispatch real
        // runs, which would fork a real `caffeinate` on whatever machine
        // runs them.
        config.daemon.power_assertion = false;
        config.validate().unwrap();
        let mut registry = Registry::with_builtins();
        registry.add_runtime(Arc::new(QuietRuntime::default()), "test");
        Arc::new(Engine::new(
            Factory { root, config },
            registry,
            Arc::new(SqliteStore::in_memory().unwrap()),
            PathBuf::from("factory"),
            Vec::new(),
        ))
    }

    /// Like `engine()`, but its "quiet" runtime is a `RecordingRuntime` --
    /// for tests that need to read back the exact prompt (or, for `shell`,
    /// the exact typed line) a dispatch built, not merely that dispatch
    /// happened.
    fn engine_with_recorder() -> (Arc<Engine>, Arc<RecordingRuntime>) {
        let root =
            std::env::temp_dir().join(format!("factory-workflow-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let config = Config {
            version: 1,
            instance: Instance {
                id: "test".into(),
                name: "test".into(),
            },
            // `power_assertion` off -- these tests dispatch real runs
            // through the real `Engine`, and the default would fork a
            // real `caffeinate` on whatever machine runs the tests. The
            // harness probe off too: see `engine()`.
            daemon: DaemonConfig {
                power_assertion: false,
                harness_health: factory_core::config::HarnessHealthConfig {
                    enabled: false,
                    ..Default::default()
                },
                ..DaemonConfig::default()
            },
            roles: Default::default(),
            dashboard: None,
            policies: Default::default(),
            quality: Default::default(),
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
                max_sessions: None,
                roles: Default::default(),
                dashboard: None,
                policies: Default::default(),
                quality: Default::default(),
                intake: Default::default(),
                dependencies: Default::default(),
                environments: Vec::new(),
                renewals: Vec::new(),
                metrics: None,
                backup: None,
            }],
            infrastructure: Default::default(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let recorder = Arc::new(RecordingRuntime::new());
        let mut registry = Registry::with_builtins();
        registry.add_runtime(recorder.clone(), "test");
        let engine = Arc::new(Engine::new(
            Factory { root, config },
            registry,
            Arc::new(SqliteStore::in_memory().unwrap()),
            PathBuf::from("factory"),
            Vec::new(),
        ));
        (engine, recorder)
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
            session: Default::default(),
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
            gate: None,
            exits: Vec::new(),
            expand: None,
        }
    }

    /// Like `node`, but names a bare adapter (`claude-code`) rather than
    /// `shell` -- `resolve_agent`'s fallback to any adapter that exists
    /// (no declaration required) is what makes this work with only the
    /// "quiet" runtime configured, and it is what lets a test read the
    /// *rendered* upstream section rather than the file `shell` points at.
    fn harness_node(id: &str) -> WorkflowNode {
        WorkflowNode {
            session: Default::default(),
            id: id.into(),
            position: CanvasPoint::default(),
            kind: WorkflowNodeKind::Task,
            task: NewTask {
                title: id.into(),
                instructions: "do the thing".into(),
                scope: Some("demo".into()),
                agent: Some("claude-code".into()),
                runtime: Some("quiet".into()),
                worktree: Some(false),
                ..Default::default()
            },
            gate: None,
            exits: Vec::new(),
            expand: None,
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
        engine.l4.store.list(&TaskFilter::default()).await.unwrap()
    }

    async fn wait_for_tasks(engine: &Arc<Engine>, count: usize) -> Vec<factory_core::Task> {
        // These older progression tests count admitted tasks, not the new
        // upfront waiting rows. Waiting-specific tests inspect the full store.
        for _ in 0..500 {
            let found: Vec<_> = tasks(engine).await.into_iter().filter(|task| task.after.is_none()
                && !task.closure.as_ref().is_some_and(|closure| closure.reason == factory_core::task::CloseReason::NotPlanned)).collect();
            if found.len() == count {
                return found;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("expected {count} tasks, got {}", tasks(engine).await.len());
    }

    async fn finish(engine: &Arc<Engine>, task_id: &str, status: RunStatus) {
        let run = loop {
            if let Some(run) = engine.l4.store.active_run(task_id).await.unwrap() {
                break run;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        engine
            .l4_service()
            .report(
                task_id,
                TaskReport {
                    artifacts: Vec::new(),
                    status: Some(status),
                    message: Some("reported by test".into()),
                    result: None,
                    send_to: None,
                    error: (status == RunStatus::Failed).then(|| "boom".into()),
                    token: run.token,
                },
            )
            .await
            .unwrap();
        engine.sync_workflow_for_task(task_id).await;
    }

    /// Like `finish`, but reports `Done` with a real result -- what the
    /// upstream-outputs tests need in hand for a downstream node to inherit.
    async fn finish_with_result(engine: &Arc<Engine>, task_id: &str, result: &str) {
        let run = loop {
            if let Some(run) = engine.l4.store.active_run(task_id).await.unwrap() {
                break run;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        engine
            .l4_service()
            .report(
                task_id,
                TaskReport {
                    artifacts: Vec::new(),
                    status: Some(RunStatus::Done),
                    message: Some("reported by test".into()),
                    result: Some(result.into()),
                    send_to: None,
                    error: None,
                    token: run.token,
                },
            )
            .await
            .unwrap();
        engine.sync_workflow_for_task(task_id).await;
    }

    /// Poll until `recorder` has captured a prompt for `task_id`'s active
    /// run -- dispatch happens on a spawned task, so there is no other signal
    /// to wait on.
    async fn wait_for_prompt(engine: &Arc<Engine>, recorder: &RecordingRuntime, task_id: &str) -> String {
        for _ in 0..200 {
            if let Some(run) = engine.l4.store.active_run(task_id).await.unwrap() {
                if let Some(prompt) = recorder.prompt_for(&run.id) {
                    return prompt;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("no prompt captured for task {task_id}");
    }

    #[tokio::test]
    async fn waiting_tasks_exist_upfront_and_are_released_once_without_scheduler_bypass() {
        let engine = engine();
        let definition = create(&engine, vec![node("a"), node("b"), node("c")], vec![edge("a", "b"), edge("b", "c")]).await;
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
        let all = tasks(&engine).await;
        assert_eq!(all.len(), 3, "all executable nodes exist before start returns");
        assert!(run.nodes.iter().all(|node| node.task_id.is_some() && node.task_created));
        let a = of_node(&all, "a")[0].clone();
        let b = of_node(&all, "b")[0].clone();
        let c = of_node(&all, "c")[0].clone();
        assert_eq!(b.after, Some(vec![a.id.clone()]));
        assert_eq!(c.after, Some(vec![b.id.clone()]));
        assert_eq!(b.status, TaskStatus::Pending);
        assert!(engine.l4_service().dependency_ready_tasks().await.unwrap().is_empty());
        engine.l4_service().start_run(&b.id, Trigger::Dependency).await;
        assert!(engine.l4.store.active_run(&b.id).await.unwrap().is_none());
        let response = engine.handle_request(factory_core::protocol::Request::TaskRun {
            id: b.id.clone(), reason: None, continue_run: false, override_wait: false,
        }).await;
        assert!(matches!(response, factory_core::protocol::Response::Error { .. }));
        engine.recover_workflows().await;
        assert_eq!(tasks(&engine).await.len(), 3);
        assert!(engine.require(&b.id).await.unwrap().after.is_some());
        finish(&engine, &a.id, RunStatus::Done).await;
        let released = wait_for_tasks(&engine, 2).await;
        assert_eq!(of_node(&released, "b")[0].id, b.id);
        assert!(engine.require(&b.id).await.unwrap().after.is_none());
        assert!(engine.require(&c.id).await.unwrap().after.is_some());
        finish(&engine, &b.id, RunStatus::Done).await;
        finish(&engine, &c.id, RunStatus::Done).await;
        engine.recover_workflows().await;
        assert_eq!(tasks(&engine).await.len(), 3);
        assert_eq!(engine.l4.store.runs(&b.id, 10).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn waiting_cancel_closes_unadmitted_tasks_as_not_planned() {
        let engine = engine();
        let definition = create(&engine, vec![node("a"), node("b")], vec![edge("a", "b")]).await;
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
        let all = tasks(&engine).await;
        let b = of_node(&all, "b")[0].clone();
        engine.cancel_workflow(&run.id).await.unwrap();
        let closed = engine.require(&b.id).await.unwrap();
        assert_eq!(closed.status, TaskStatus::Cancelled);
        assert_eq!(closed.closure.unwrap().reason, factory_core::task::CloseReason::NotPlanned);
        assert!(closed.after.is_none());
        assert!(engine.l4.store.runs(&b.id, 10).await.unwrap().is_empty());
        engine.recover_workflows().await;
        assert_eq!(tasks(&engine).await.len(), 2);
    }

    #[tokio::test]
    async fn waiting_deleted_task_is_never_regenerated_by_recovery() {
        let engine = engine();
        let definition = create(&engine, vec![node("a"), node("b")], vec![edge("a", "b")]).await;
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
        let id = node_run(&run, "b").task_id.clone().unwrap();
        engine.l4.store.delete(&id).await.unwrap();
        engine.recover_workflows().await;
        engine.recover_workflows().await;
        assert!(engine.l4.store.get(&id).await.unwrap().is_none());
        assert_eq!(engine.workflow_run(&run.id).await.unwrap().status, WorkflowRunStatus::Failed);
    }

    #[tokio::test]
    async fn waiting_override_requires_a_reason_and_records_the_early_release() {
        let engine = engine();
        let parent = engine.create(node("parent").task).await.unwrap();
        let mut new = node("child").task;
        new.after = Some(vec![parent.id.clone()]);
        let task = engine.create(new).await.unwrap();
        let request = |reason| factory_core::protocol::Request::TaskRun {
            id: task.id.clone(), reason, continue_run: false, override_wait: true,
        };
        assert!(matches!(engine.handle_request(request(None)).await, factory_core::protocol::Response::Error { .. }));
        assert!(engine.require(&task.id).await.unwrap().after.is_some());
        assert!(matches!(engine.handle_request(request(Some("manual investigation".into()))).await, factory_core::protocol::Response::Ok { .. }));
        assert!(engine.require(&task.id).await.unwrap().after.is_none());
        let entries = engine.l4.store.entries(&task.id, 50).await.unwrap();
        let entry = entries.iter().find(|entry| entry.kind == "waiting_override").unwrap();
        assert!(entry.message.contains("manual investigation"));
        assert_eq!(entry.source, "owner");
        assert_eq!(entry.data.as_ref().unwrap()["after"], serde_json::json!([parent.id]));
        finish(&engine, &task.id, RunStatus::Done).await;
    }

    #[tokio::test]
    async fn waiting_standalone_after_fires_once_and_validates_trigger_edits() {
        let engine = engine();
        let parent = engine.create(node("parent").task).await.unwrap();
        let mut new = node("child").task;
        new.after = Some(vec![parent.id.clone()]);
        let child = engine.create(new.clone()).await.unwrap();
        assert!(engine.l4_service().dependency_ready_tasks().await.unwrap().is_empty());
        let invalid = engine.update(&parent.id, TaskPatch { after: Some(vec![child.id.clone()]), ..Default::default() }, None).await;
        assert!(invalid.unwrap_err().to_string().contains("cycle"));
        new.schedule = Some(factory_core::task::Schedule::Every { seconds: 1 });
        assert!(engine.create(new).await.unwrap_err().to_string().contains("exclusive"));
        engine.l4_service().start_run(&parent.id, Trigger::Manual).await;
        finish(&engine, &parent.id, RunStatus::Done).await;
        let ready = engine.l4_service().dependency_ready_tasks().await.unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, child.id);
        engine.l4_service().start_run(&child.id, Trigger::Dependency).await;
        finish(&engine, &child.id, RunStatus::Done).await;
        assert!(engine.require(&child.id).await.unwrap().after.is_none());
        assert!(engine.l4_service().dependency_ready_tasks().await.unwrap().is_empty());
        assert_eq!(engine.l4.store.runs(&child.id, 10).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn waiting_workflow_override_stays_released_and_cancelled_admission_is_refused() {
        let engine = engine();
        let definition = create(&engine, vec![node("a"), node("b")], vec![edge("a", "b")]).await;
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
        let all = tasks(&engine).await;
        let b = of_node(&all, "b")[0].clone();
        engine.override_waiting(&b, &Caller::Owner, "inspect early").await.unwrap();
        engine.advance_workflow(&run.id).await.unwrap();
        assert!(engine.require(&b.id).await.unwrap().after.is_none());
        assert!(engine.l4.store.active_run(&b.id).await.unwrap().is_none(), "the graph does not automatically run an early override");
        engine.cancel_workflow(&run.id).await.unwrap();
        engine.l4_service().start_run(&b.id, Trigger::Workflow).await;
        assert!(engine.l4.store.runs(&b.id, 10).await.unwrap().is_empty(), "a queued release cannot launch after cancellation");
        assert_eq!(engine.require(&b.id).await.unwrap().closure.unwrap().reason, factory_core::task::CloseReason::NotPlanned);
    }

    #[tokio::test]
    async fn waiting_trigger_edits_cannot_create_a_cycle_concurrently() {
        let engine = engine();
        let a = engine.create(node("a").task).await.unwrap();
        let b = engine.create(node("b").task).await.unwrap();
        let (left, right) = tokio::join!(
            engine.update(&a.id, TaskPatch { after: Some(vec![b.id.clone()]), ..Default::default() }, None),
            engine.update(&b.id, TaskPatch { after: Some(vec![a.id.clone()]), ..Default::default() }, None)
        );
        assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
        assert!(left.err().or_else(|| right.err()).unwrap().to_string().contains("cycle"));
    }

    #[tokio::test]
    async fn waiting_terminal_workflow_still_allows_explicit_infrastructure_continue() {
        let engine = engine();
        let definition = create(&engine, vec![node("a"), node("b")], vec![edge("a", "b")]).await;
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
        let all = tasks(&engine).await;
        let a = of_node(&all, "a")[0].clone();
        let b = of_node(&all, "b")[0].clone();
        let first = wait_for_attempt(&engine, &a.id, 1).await;
        engine.l4_service().fail_run(&first.id, FailKind::AckTimeout, "provider vanished").await;
        engine.sync_workflow_for_task(&a.id).await;
        assert_eq!(engine.workflow_run(&run.id).await.unwrap().status, WorkflowRunStatus::Failed);
        let response = engine.handle_request(factory_core::protocol::Request::TaskRun {
            id: a.id.clone(), reason: None, continue_run: true, override_wait: false,
        }).await;
        assert!(matches!(response, factory_core::protocol::Response::Ok { .. }), "{response:?}");
        let second = wait_for_attempt(&engine, &a.id, 2).await;
        assert_eq!(second.continued_from.as_deref(), Some(first.id.as_str()));
        finish(&engine, &a.id, RunStatus::Done).await;
        assert_eq!(engine.require(&b.id).await.unwrap().runs, 0, "continuation never revives cancelled downstream work");
    }

    #[tokio::test]
    async fn dispatch_carries_direct_parent_outputs_for_a_fan_in_node() {
        let (engine, recorder) = engine_with_recorder();
        let definition = create(
            &engine,
            vec![node("a"), node("b"), node("c")],
            vec![edge("a", "c"), edge("b", "c")],
        )
        .await;
        engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
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

        finish_with_result(&engine, &a.id, "output from a").await;
        finish_with_result(&engine, &b.id, "output from b").await;

        let all = wait_for_tasks(&engine, 3).await;
        let c = all
            .iter()
            .find(|t| t.workflow_origin.as_ref().unwrap().node_id == "c")
            .unwrap()
            .clone();

        // `node()` makes every node a `shell` task, so the captured prompt is
        // just `. '<script path>'` -- the report wrapper (and the upstream
        // export, when there is one) lives in that file, not the prompt
        // text. Reading the script back, and then the upstream file it
        // names, is what actually exercises the daemon's own computation of
        // `upstream`, edge order and all, rather than a rendering of it.
        let prompt = wait_for_prompt(&engine, &recorder, &c.id).await;
        let script = std::fs::read_to_string(script_path_from_prompt(&prompt))
            .unwrap_or_else(|e| panic!("reading the shell agent's script ({prompt}): {e}"));
        let path = script
            .split("FACTORY_UPSTREAM_FILE='")
            .nth(1)
            .and_then(|rest| rest.split('\'').next())
            .unwrap_or_else(|| panic!("the script exports the upstream file when there is one: {script}"));
        let json = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("reading the upstream file the script points at ({path}): {e}"));
        let entries: Vec<UpstreamOutput> = serde_json::from_str(&json).unwrap();

        assert_eq!(entries.len(), 2, "both direct parents, no more: {entries:#?}");
        assert_eq!(entries[0].node_id, "a", "definition edge order is a, then b: {entries:#?}");
        assert_eq!(entries[0].task_id, a.id);
        assert_eq!(entries[0].title, "a");
        assert_eq!(entries[0].result.as_deref(), Some("output from a"));
        assert_eq!(entries[1].node_id, "b");
        assert_eq!(entries[1].task_id, b.id);
        assert_eq!(entries[1].result.as_deref(), Some("output from b"));
    }

    /// The other half of the same acceptance criterion: a harness agent's
    /// task gets a *prompt* that contains each parent's result, not just a
    /// file a shell command could go read. `harness_node` runs the same fan-in
    /// shape as the test above through `claude-code` instead of `shell`, so
    /// this reads `HarnessAgent::prompt`'s own rendering rather than a
    /// second look at `ShellAgent`'s.
    #[tokio::test]
    async fn dispatch_carries_direct_parent_outputs_into_a_harness_agents_prompt() {
        let (engine, recorder) = engine_with_recorder();
        let definition = create(
            &engine,
            vec![harness_node("a"), harness_node("b"), harness_node("c")],
            vec![edge("a", "c"), edge("b", "c")],
        )
        .await;
        engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
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

        finish_with_result(&engine, &a.id, "build succeeded").await;
        finish_with_result(&engine, &b.id, "lint succeeded").await;

        let all = wait_for_tasks(&engine, 3).await;
        let c = all
            .iter()
            .find(|t| t.workflow_origin.as_ref().unwrap().node_id == "c")
            .unwrap()
            .clone();

        let prompt = wait_for_prompt(&engine, &recorder, &c.id).await;
        assert!(
            prompt.contains("Output from the workflow steps this task follows"),
            "the labelled section is there: {prompt}"
        );
        assert!(prompt.contains("node a"), "{prompt}");
        assert!(prompt.contains("build succeeded"), "{prompt}");
        assert!(prompt.contains("node b"), "{prompt}");
        assert!(prompt.contains("lint succeeded"), "{prompt}");
    }

    #[tokio::test]
    async fn a_root_nodes_dispatch_carries_no_upstream_outputs() {
        let (engine, recorder) = engine_with_recorder();
        let definition = create(&engine, vec![node("a")], vec![]).await;
        engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
        let a = wait_for_tasks(&engine, 1).await.pop().unwrap();
        let prompt = wait_for_prompt(&engine, &recorder, &a.id).await;
        let script = std::fs::read_to_string(script_path_from_prompt(&prompt))
            .unwrap_or_else(|e| panic!("reading the shell agent's script ({prompt}): {e}"));
        assert!(
            !script.contains("FACTORY_UPSTREAM_FILE"),
            "a root node has no parents to report: {script}"
        );
    }

    /// `ShellAgent::prompt` (see `crates/factory-plugins/src/builtin/agents.rs`)
    /// returns exactly `. '<script path>'` -- the whole report wrapper lives
    /// in that file rather than the typed line itself. Pulled out once so
    /// every test reading a shell node's dispatched prompt agrees on how to
    /// get from it back to the script.
    fn script_path_from_prompt(prompt: &str) -> &str {
        prompt
            .strip_prefix(". '")
            .and_then(|rest| rest.strip_suffix('\''))
            .unwrap_or_else(|| panic!("expected `. '<script path>'`, got {prompt:?}"))
    }

    #[tokio::test]
    async fn linear_nodes_spawn_once_and_keep_provenance() {
        let engine = engine();
        let definition = create(&engine, vec![node("a"), node("b")], vec![edge("a", "b")]).await;
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
        let first = wait_for_tasks(&engine, 1).await.pop().unwrap();
        assert_eq!(
            first.workflow_origin.as_ref().unwrap().workflow_run_id,
            run.id
        );
        engine.advance_workflow(&run.id).await.unwrap();
        assert_eq!(
            tasks(&engine).await.len(),
            2,
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
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
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
        let all = tasks(&engine).await;
        assert_eq!(all.len(), 4);
        let d = of_node(&all, "d")[0];
        assert!(d.after.is_some(), "fan-in still waits for c");
        assert_eq!(d.runs, 0);
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
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
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
        let all = tasks(&engine).await;
        assert_eq!(all.len(), 2);
        assert_eq!(of_node(&all, "b")[0].closure.as_ref().unwrap().reason, factory_core::task::CloseReason::NotPlanned);
    }

    #[tokio::test]
    async fn blocked_pauses_and_cancelling_settles_active_and_unstarted_nodes() {
        let engine = engine();
        let definition = create(&engine, vec![node("a"), node("b")], vec![edge("a", "b")]).await;
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
        let root = wait_for_tasks(&engine, 1).await.pop().unwrap();
        finish(&engine, &root.id, RunStatus::Blocked).await;
        let paused = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(paused.status, WorkflowRunStatus::Running);
        assert_eq!(paused.nodes[0].status, WorkflowNodeStatus::Blocked);
        assert_eq!(
            tasks(&engine).await.len(),
            2,
            "blocked preserves the child's upfront row"
        );
        let all = tasks(&engine).await;
        assert!(of_node(&all, "b")[0].after.is_some(), "blocked never unlocks the child");

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
            WorkflowNodeStatus::Cancelled
        );
    }

    #[tokio::test]
    async fn recovery_is_idempotent_and_definition_edits_do_not_change_a_run_snapshot() {
        let engine = engine();
        let definition = create(&engine, vec![node("a")], vec![]).await;
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
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
            .start_workflow(&definition.id, Default::default(), &wearing("starter"))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("create tasks"),
            "a role without task.create is refused: {error}"
        );
        assert!(
            engine.l4.workflows.active_runs().await.unwrap().is_empty(),
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
            .start_workflow(&definition.id, Default::default(), &Caller::Owner)
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
        engine.l4.store.put_agent(&agent).await.unwrap();

        let definition = create(&engine, vec![node("a"), node("b")], vec![edge("a", "b")]).await;
        let caller = Caller::Agent {
            scope: "demo".into(),
            name: "w".into(),
            role: engine.l3_service().effective_role("demo", "w").await,
            run_id: None,
        };
        let run = engine.start_workflow(&definition.id, Default::default(), &caller).await.unwrap();
        let root = wait_for_tasks(&engine, 1).await.pop().unwrap();

        // The role changes while "a" is still in flight -- well after the
        // run started, well before "b" is ever considered.
        engine
            .l3_service().set_agent_role("demo/w", Some(Role::new("weak")))
            .await
            .unwrap();

        finish(&engine, &root.id, RunStatus::Done).await;

        let run = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(run.status, WorkflowRunStatus::Failed);
        assert_eq!(run.failure_node_id.as_deref(), Some("b"));
        let b = run.nodes.iter().find(|n| n.node_id == "b").unwrap();
        assert_eq!(b.status, WorkflowNodeStatus::Failed);
        let b_task = engine.require(b.task_id.as_ref().unwrap()).await.unwrap();
        assert_eq!(b_task.runs, 0, "revoked authority never dispatches the upfront task");
        assert_eq!(b_task.closure.as_ref().unwrap().reason, factory_core::task::CloseReason::NotPlanned);
        assert!(
            b.error.as_deref().unwrap_or("").contains("create tasks"),
            "{:?}",
            b.error
        );
        assert_eq!(
            tasks(&engine).await.len(),
            2,
            "the denied upfront task is retained as not planned"
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
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
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
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
        let root = wait_for_tasks(&engine, 1).await.pop().unwrap();

        engine.l4.store.delete(&root.id).await.unwrap();
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
        assert_eq!(tasks(&engine).await.len(), 1, "the upfront downstream row is closed, not deleted");
    }

    // -- B6: further execution coverage the issue asks for --------------------

    #[tokio::test]
    async fn every_spawned_task_carries_provenance_and_a_plain_task_carries_none() {
        let engine = engine();
        let definition = create(&engine, vec![node("a")], vec![]).await;
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
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
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
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
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
        let root = wait_for_tasks(&engine, 1).await.pop().unwrap();
        let active = loop {
            if let Some(r) = engine.l4.store.active_run(&root.id).await.unwrap() {
                break r;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        engine
            .l4_service()
            .report(
                &root.id,
                TaskReport {
                    artifacts: Vec::new(),
                    status: Some(RunStatus::Running),
                    message: Some("working".into()),
                    result: None,
                    send_to: None,
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
            WorkflowNodeStatus::Cancelled
        );

        let task = engine.l4.store.get(&root.id).await.unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Cancelled, "task history is kept, not deleted");
        assert!(
            !engine.l4.store.runs(&root.id, 10).await.unwrap().is_empty(),
            "run history is kept, not deleted"
        );
    }

    #[tokio::test]
    async fn a_node_naming_an_unknown_agent_fails_that_node_and_the_run() {
        let engine = engine();
        let mut bad = node("a");
        bad.task.agent = Some("does-not-exist".into());
        let definition = create(&engine, vec![bad], vec![]).await;
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
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
                // `power_assertion` off -- these tests dispatch real runs
                // through the real `Engine`, and the default would fork a
                // real `caffeinate` on whatever machine runs the tests.
                daemon: DaemonConfig { power_assertion: false, ..DaemonConfig::default() },
                roles: Default::default(),
                dashboard: None,
                policies: Default::default(),
                quality: Default::default(),
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
                    max_sessions: None,
                    roles: Default::default(),
                    dashboard: None,
                    policies: Default::default(),
                    quality: Default::default(),
                    intake: Default::default(),
                    dependencies: Default::default(),
                    environments: Vec::new(),
                    renewals: Vec::new(),
                    metrics: None,
                    backup: None,
                }],
                infrastructure: Default::default(),
                secrets: Vec::new(),
                plugins_dir: None,
                renewals: Vec::new(),
                renewals_notify: None,
            };
            let mut registry = Registry::with_builtins();
            registry.add_runtime(Arc::new(QuietRuntime::default()), "test");
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
            .start_workflow(&definition.id, Default::default(), &Caller::Owner)
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
            2,
            "recovery keeps both upfront task IDs"
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
        engine.l4.workflows.put_run(&run).await.unwrap();

        engine.recover_workflows().await;
        let created = wait_for_tasks(&engine, 1).await;
        assert_eq!(created[0].id, phantom_id, "recovery fills in exactly the persisted id");

        engine.recover_workflows().await;
        assert_eq!(tasks(&engine).await.len(), 1, "a second recovery does not duplicate it");
    }

    #[tokio::test]
    async fn recovery_also_mirrors_a_stale_node_inside_an_already_terminal_run() {
        let engine = engine();
        let definition = create(&engine, vec![node("a")], vec![]).await;
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
        let task = wait_for_tasks(&engine, 1).await.pop().unwrap();
        finish(&engine, &task.id, RunStatus::Done).await;
        let settled = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(settled.status, WorkflowRunStatus::Done);

        // R11: simulate the staleness this test exists for -- an older
        // build (or a missed event) left the run terminal but its node
        // overlay still "running", even though the task itself is done.
        let mut stale = settled;
        stale.nodes[0].status = WorkflowNodeStatus::Running;
        engine.l4.workflows.put_run(&stale).await.unwrap();

        engine.recover_workflows().await;

        let recovered = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(
            recovered.status,
            WorkflowRunStatus::Done,
            "a terminal run's own status is never rewritten by reconciliation"
        );
        assert_eq!(
            recovered.nodes[0].status,
            WorkflowNodeStatus::Done,
            "the stale node overlay is corrected to match its task"
        );
        assert_eq!(
            tasks(&engine).await.len(),
            1,
            "reconciling a terminal run never spawns anything"
        );
    }

    // --- #149: ordered exits, bounded backwards routes, and run inputs ---

    fn of_node<'a>(tasks: &'a [factory_core::Task], node: &str) -> Vec<&'a factory_core::Task> {
        tasks.iter().filter(|t| t.workflow_origin.as_ref().unwrap().node_id == node).collect()
    }

    fn node_run<'a>(run: &'a WorkflowRun, node: &str) -> &'a factory_core::workflow::WorkflowNodeRun {
        run.nodes.iter().find(|n| n.node_id == node).unwrap()
    }

    async fn wait_for_attempt(engine: &Engine, task_id: &str, attempt: u32) -> factory_core::Run {
        for _ in 0..400 {
            if let Some(run) = engine.l4.store.active_run(task_id).await.unwrap() {
                if run.attempt == attempt && engine.l4.store.entries(task_id, 50).await.unwrap().iter().any(|entry|
                    entry.kind == "dispatched" && entry.run_id.as_deref() == Some(run.id.as_str())) { return run; }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("attempt {attempt} did not start on task {task_id}");
    }

    /// implement -> review -> ship, review sending the work back to
    /// implement at most `max_rounds` times. `implement` is a harness node
    /// so the prompt it is re-dispatched with can be read back.
    async fn review_loop(engine: &Arc<Engine>, max_rounds: u32) -> WorkflowDefinition {
        let mut review = node("review");
        review.exits = vec![WorkflowExit {
            to: "implement".into(),
            check: None,
            agent: Some("concrete findings the implementer can fix alone".into()),
            max_rounds: Some(max_rounds),
        }];
        create(
            engine,
            vec![harness_node("implement"), review, node("ship")],
            vec![edge("implement", "review"), edge("review", "ship")],
        )
        .await
    }

    async fn review_fails(engine: &Arc<Engine>, task_id: &str, findings: &str) {
        let run = loop {
            if let Some(run) = engine.l4.store.active_run(task_id).await.unwrap() {
                break run;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        engine
            .l4_service()
            .report(
                task_id,
                TaskReport {
                    artifacts: Vec::new(),
                    status: Some(RunStatus::Done),
                    message: None,
                    result: Some(findings.into()),
                    send_to: Some("implement".into()),
                    error: None,
                    token: run.token,
                },
            )
            .await
            .unwrap();
        engine.sync_workflow_for_task(task_id).await;
    }

    #[tokio::test]
    async fn a_done_review_can_send_the_work_back_with_its_findings_until_it_passes() {
        let (engine, recorder) = engine_with_recorder();
        let definition = review_loop(&engine, 5).await;
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();

        let first = wait_for_tasks(&engine, 1).await.pop().unwrap();
        finish_with_result(&engine, &first.id, "PR https://example.test/pr/1").await;
        let review = of_node(&wait_for_tasks(&engine, 2).await, "review")[0].clone();
        let active = loop {
            if let Some(active) = engine.l4.store.active_run(&review.id).await.unwrap() {
                break active;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        let refused = engine.l4_service().report(&review.id, TaskReport {
            artifacts: Vec::new(),
            status: Some(RunStatus::Done), message: None, result: Some("findings".into()),
            send_to: Some("nowhere".into()), error: None, token: active.token,
        }).await.unwrap_err().to_string();
        assert!(refused.contains("implement (concrete findings the implementer can fix alone)"), "{refused}");
        review_fails(&engine, &review.id, "the parser test is missing").await;

        let waiting_review = engine.require(&review.id).await.unwrap();
        assert!(waiting_review.after.is_some(), "the next review round waits on rework");
        assert_eq!(waiting_review.status, TaskStatus::Pending);
        let routed_review = engine.l4.store.runs(&review.id, 1).await.unwrap().pop().unwrap();
        assert_eq!(
            routed_review.status,
            RunStatus::Done,
            "sending work back is a successful review verdict"
        );
        assert_eq!(routed_review.routed_to.as_deref(), Some("implement"));
        assert!(routed_review.fail_kind.is_none());
        let review_entries = engine.l4.store.entries(&review.id, 20).await.unwrap();
        assert!(review_entries.iter().any(|entry| {
            entry.kind == "routed_to"
                && entry
                    .data
                    .as_ref()
                    .is_some_and(|data| data["routed_to"] == "implement")
        }));

        let second_run = wait_for_attempt(&engine, &first.id, 2).await;
        let again = engine.require(&first.id).await.unwrap();
        assert_eq!(again.title, "implement");
        assert_eq!(second_run.workflow_round, 1);
        assert!(second_run.feedback.as_ref().unwrap().feedback.as_deref().unwrap().contains("the parser test is missing"));
        assert_eq!(tasks(&engine).await.len(), 3, "ship also exists upfront");
        let prompt = wait_for_prompt(&engine, &recorder, &again.id).await;
        assert!(prompt.contains("sent this work back -- rework round 1 of 5"), "{prompt}");
        assert!(prompt.contains("the parser test is missing"), "{prompt}");

        let midway = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(midway.status, WorkflowRunStatus::Running);
        assert!(node_run(&midway, "implement").superseded_task_ids.is_empty());
        assert!(node_run(&midway, "review").superseded_task_ids.is_empty());
        assert_eq!(node_run(&midway, "review").round, 1);
        assert_eq!(node_run(&midway, "ship").status, WorkflowNodeStatus::Unstarted);

        // The superseded review saying anything more changes nothing.
        engine.record_workflow_task_state(&review.id).await;
        assert_eq!(
            node_run(&engine.workflow_run(&run.id).await.unwrap(), "review").status,
            WorkflowNodeStatus::Unstarted
        );

        finish_with_result(&engine, &again.id, "PR https://example.test/pr/1, fixed").await;
        let second_review_run = wait_for_attempt(&engine, &review.id, 2).await;
        let second_review = engine.require(&review.id).await.unwrap();
        assert_eq!(second_review.title, "review");
        assert_eq!(second_review_run.workflow_round, 1);
        finish(&engine, &second_review.id, RunStatus::Done).await;
        let ship = of_node(&wait_for_tasks(&engine, 3).await, "ship")[0].clone();
        finish(&engine, &ship.id, RunStatus::Done).await;
        assert_eq!(engine.workflow_run(&run.id).await.unwrap().status, WorkflowRunStatus::Done);
    }

    #[tokio::test]
    async fn once_the_rounds_are_used_up_send_to_is_refused_and_the_run_waits_for_a_person() {
        let engine = engine();
        let mut review = node("review");
        review.exits = vec![WorkflowExit {
            to: "implement".into(),
            check: None,
            agent: Some("fixable findings".into()),
            max_rounds: Some(1),
        }];
        let definition = create(
            &engine,
            vec![node("implement"), review],
            vec![edge("implement", "review")],
        )
        .await;
        let run = engine
            .start_workflow(&definition.id, Default::default(), &Caller::Owner)
            .await
            .unwrap();

        let implement = wait_for_tasks(&engine, 1).await.pop().unwrap();
        finish(&engine, &implement.id, RunStatus::Done).await;
        let review = of_node(&wait_for_tasks(&engine, 2).await, "review")[0].clone();
        review_fails(&engine, &review.id, "still wrong").await;
        wait_for_attempt(&engine, &implement.id, 2).await;
        let again = engine.require(&implement.id).await.unwrap();
        finish(&engine, &again.id, RunStatus::Done).await;
        wait_for_attempt(&engine, &review.id, 2).await;
        let last = engine.require(&review.id).await.unwrap();
        let active = loop {
            if let Some(active) = engine.l4.store.active_run(&last.id).await.unwrap() {
                break active;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        let error = engine
            .l4_service()
            .report(
                &last.id,
                TaskReport {
                    artifacts: Vec::new(),
                    status: Some(RunStatus::Done),
                    message: None,
                    result: Some("still wrong, twice".into()),
                    send_to: Some("implement".into()),
                    error: None,
                    token: active.token,
                },
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("no rounds left"), "{error}");
        let run = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(run.status, WorkflowRunStatus::Running);
        assert_eq!(
            tasks(&engine).await.len(),
            2,
            "nothing spawned past the budget"
        );
    }

    #[tokio::test]
    async fn a_review_that_timed_out_is_not_a_verdict_and_sends_nothing_back() {
        let engine = engine();
        let mut review = node("review");
        review.exits = vec![WorkflowExit {
            to: "implement".into(),
            check: None,
            agent: Some("fixable findings".into()),
            max_rounds: Some(5),
        }];
        let definition = create(
            &engine,
            vec![node("implement"), review],
            vec![edge("implement", "review")],
        )
        .await;
        let run = engine
            .start_workflow(&definition.id, Default::default(), &Caller::Owner)
            .await
            .unwrap();

        let implement = wait_for_tasks(&engine, 1).await.pop().unwrap();
        finish(&engine, &implement.id, RunStatus::Done).await;
        let review = of_node(&wait_for_tasks(&engine, 2).await, "review")[0].clone();
        let active = loop {
            if let Some(active) = engine.l4.store.active_run(&review.id).await.unwrap() {
                break active;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        engine.l4_service().fail_run(&active.id, FailKind::RunTimeout, "ran out of time").await;
        engine.sync_workflow_for_task(&review.id).await;

        let run = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(run.status, WorkflowRunStatus::Failed);
        assert_eq!(node_run(&run, "review").round, 0);
        assert_eq!(tasks(&engine).await.len(), 2);
    }

    #[tokio::test]
    async fn an_agent_reported_failure_never_takes_an_agent_exit() {
        let engine = engine();
        let definition = review_loop(&engine, 5).await;
        let run = engine
            .start_workflow(&definition.id, Default::default(), &Caller::Owner)
            .await
            .unwrap();
        let implement = wait_for_tasks(&engine, 1).await.pop().unwrap();
        finish(&engine, &implement.id, RunStatus::Done).await;
        let review = of_node(&wait_for_tasks(&engine, 2).await, "review")[0].clone();
        finish(&engine, &review.id, RunStatus::Failed).await;
        let settled = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(settled.status, WorkflowRunStatus::Failed);
        assert_eq!(node_run(&settled, "review").round, 0);
        assert_eq!(
            tasks(&engine).await.len(),
            3,
            "failure spawned no new implementation task"
        );
    }

    #[tokio::test]
    async fn check_exits_hold_do_not_hold_and_error_with_output() {
        async fn setup(command: &str) -> (Arc<Engine>, WorkflowRun) {
            let engine = engine();
            let mut a = node("a");
            a.exits = vec![WorkflowExit {
                to: "c".into(),
                check: Some(command.into()),
                agent: None,
                max_rounds: None,
            }];
            let definition = create(
                &engine,
                vec![a, node("b"), node("c")],
                vec![edge("a", "b"), edge("b", "c"), edge("a", "c")],
            )
            .await;
            let run = engine
                .start_workflow(&definition.id, Default::default(), &Caller::Owner)
                .await
                .unwrap();
            (engine, run)
        }

        let (engine, run) = setup("true").await;
        let initial = tasks(&engine).await;
        let conditional = of_node(&initial, "b")[0];
        assert!(conditional.after_condition.as_deref().unwrap().contains("conditional"));
        let conditional_id = conditional.id.clone();
        let a = wait_for_tasks(&engine, 1).await.pop().unwrap();
        finish(&engine, &a.id, RunStatus::Done).await;
        let all = wait_for_tasks(&engine, 2).await;
        assert_eq!(
            of_node(&all, "b").len(),
            0,
            "the default branch was exclusive and skipped"
        );
        let c = of_node(&all, "c")[0].clone();
        let state = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(
            node_run(&state, "b").status,
            WorkflowNodeStatus::SkippedByRoute
        );
        assert_eq!(
            node_run(&state, "b").skip_reason.as_deref(),
            Some("skipped (a -> c)")
        );
        let closed = engine.require(&conditional_id).await.unwrap();
        assert_eq!(closed.closure.unwrap().reason, factory_core::task::CloseReason::NotPlanned);
        assert_eq!(closed.runs, 0);
        assert!(closed.after.is_none());
        finish(&engine, &c.id, RunStatus::Done).await;
        assert_eq!(
            engine.workflow_run(&run.id).await.unwrap().status,
            WorkflowRunStatus::Done
        );

        let (engine, _) = setup("false").await;
        let a = wait_for_tasks(&engine, 1).await.pop().unwrap();
        finish(&engine, &a.id, RunStatus::Done).await;
        let all = wait_for_tasks(&engine, 2).await;
        assert_eq!(
            of_node(&all, "b").len(),
            1,
            "exit 1 did not hold, so plain edges remained the default"
        );
        assert_eq!(of_node(&all, "c").len(), 0, "fan-in still waits for b");

        let (engine, run) = setup("printf 'broken check'; exit 2").await;
        let a = wait_for_tasks(&engine, 1).await.pop().unwrap();
        finish(&engine, &a.id, RunStatus::Done).await;
        let state = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(node_run(&state, "a").status, WorkflowNodeStatus::Blocked);
        assert!(node_run(&state, "a")
            .error
            .as_deref()
            .unwrap()
            .contains("broken check"));
        let entries = engine.l4.store.entries(&a.id, 20).await.unwrap();
        assert!(entries.iter().any(|entry| entry.kind == "exit_checked"
            && entry
                .data
                .as_ref()
                .is_some_and(|data| data.to_string().contains("broken check"))));

        let (engine, run) = setup("sleep 2").await;
        let a = wait_for_tasks(&engine, 1).await.pop().unwrap();
        finish(&engine, &a.id, RunStatus::Done).await;
        let state = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(node_run(&state, "a").status, WorkflowNodeStatus::Blocked);
        assert!(node_run(&state, "a").error.as_deref().unwrap().contains("did not finish within 1s"));
        let entries = engine.l4.store.entries(&a.id, 20).await.unwrap();
        assert!(entries.iter().any(|entry| entry.kind == "exit_checked"
            && entry.data.as_ref().is_some_and(|data| data.to_string().contains("did not finish within 1s"))));
    }

    #[tokio::test]
    async fn exits_are_first_match_wins_and_send_to_refusals_name_alternatives() {
        let standalone_engine = engine();
        let plain = standalone_engine.create(NewTask { title: "plain".into(), scope: Some("demo".into()), ..Default::default() }).await.unwrap();
        let standalone = standalone_engine.validate_workflow_send_to(&plain.id, Some(RunStatus::Done), Some("b"))
            .await.unwrap_err().to_string();
        assert!(standalone.contains("only available on a workflow node"), "{standalone}");

        let engine = engine();
        let mut a = node("a");
        a.exits = vec![
            WorkflowExit {
                to: "b".into(),
                check: Some("true".into()),
                agent: None,
                max_rounds: None,
            },
            WorkflowExit {
                to: "c".into(),
                check: Some("true".into()),
                agent: None,
                max_rounds: None,
            },
        ];
        let definition = create(
            &engine,
            vec![a, node("b"), node("c")],
            vec![edge("a", "b"), edge("a", "c")],
        )
        .await;
        let run = engine
            .start_workflow(&definition.id, Default::default(), &Caller::Owner)
            .await
            .unwrap();
        let task = wait_for_tasks(&engine, 1).await.pop().unwrap();
        let active = loop {
            if let Some(active) = engine.l4.store.active_run(&task.id).await.unwrap() {
                break active;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        let wrong_status = engine
            .l4_service()
            .report(
                &task.id,
                TaskReport {
                    artifacts: Vec::new(),
                    status: Some(RunStatus::Failed),
                    message: None,
                    result: None,
                    send_to: Some("b".into()),
                    error: Some("broke".into()),
                    token: active.token.clone(),
                },
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(
            wrong_status.contains("only with --status done"),
            "{wrong_status}"
        );
        let wrong_target = engine
            .l4_service()
            .report(
                &task.id,
                TaskReport {
                    artifacts: Vec::new(),
                    status: Some(RunStatus::Done),
                    message: None,
                    result: None,
                    send_to: Some("nowhere".into()),
                    error: None,
                    token: active.token,
                },
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(
            wrong_target.contains("no agent exits") && wrong_target.contains("plain done"),
            "{wrong_target}"
        );
        finish(&engine, &task.id, RunStatus::Done).await;
        let tasks = wait_for_tasks(&engine, 2).await;
        assert_eq!(of_node(&tasks, "b").len(), 1, "first true exit won");
        assert_eq!(
            of_node(&tasks, "c").len(),
            0,
            "the second true exit was not evaluated as another route"
        );
        assert_eq!(
            node_run(&engine.workflow_run(&run.id).await.unwrap(), "c").status,
            WorkflowNodeStatus::SkippedByRoute
        );
    }

    #[tokio::test]
    async fn a_run_is_started_with_its_inputs_written_into_every_node() {
        let engine = engine();
        let mut triage = node("triage");
        triage.task.title = "Triage #{{issue}}".into();
        triage.task.labels.insert("issue".into(), "{{issue}}".into());
        let definition = engine
            .create_workflow(WorkflowDraft {
                name: "ticket".into(),
                scope: "demo".into(),
                inputs: vec![factory_core::workflow::WorkflowInput { name: "issue".into(), description: String::new() }],
                nodes: vec![triage],
                ..Default::default()
            })
            .await
            .unwrap();

        let refused = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap_err();
        assert!(refused.to_string().contains("needs issue"), "{refused}");
        assert!(tasks(&engine).await.is_empty(), "a refused start spawns nothing");

        let inputs = BTreeMap::from([("issue".to_string(), "140".to_string())]);
        let run = engine.start_workflow(&definition.id, inputs.clone(), &Caller::Owner).await.unwrap();
        assert_eq!(run.inputs, inputs);
        let task = wait_for_tasks(&engine, 1).await.pop().unwrap();
        assert_eq!(task.title, "Triage #140");
        assert_eq!(task.labels["issue"], "140");
        assert_eq!(
            engine.workflow_definition(&definition.id).await.unwrap().nodes[0].task.title,
            "Triage #{{issue}}",
            "the stored definition keeps its placeholders"
        );
    }

    #[tokio::test]
    async fn decomposition_integrates_in_dependency_order_checks_the_union_and_cleans_every_worktree(
    ) {
        let root =
            std::env::temp_dir().join(format!("factory-integration-test-{}", uuid::Uuid::new_v4()));
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git_ok(&repo, &["init", "-q", "-b", "main"]).await;
        git_ok(&repo, &["config", "user.email", "factory@example.test"]).await;
        git_ok(&repo, &["config", "user.name", "Factory Test"]).await;
        std::fs::write(repo.join("README.md"), "base\n").unwrap();
        git_ok(&repo, &["add", "README.md"]).await;
        git_ok(&repo, &["commit", "-q", "-m", "base"]).await;

        let engine = engine_in_git_scope(root.clone(), repo.clone());
        let parent = engine
            .create(NewTask {
                title: "Build two slices".into(),
                instructions: "one integrated result".into(),
                scope: Some("demo".into()),
                worktree: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        let parts = vec![
            SplitPart {
                id: "foundation".into(),
                title: "Foundation".into(),
                instructions: "add foundation.txt".into(),
                acceptance: Some("test -f foundation.txt".into()),
                owns: vec!["foundation.txt".into()],
                interface: Some("foundation.txt exists".into()),
                estimate_seconds: Some(60),
                ..Default::default()
            },
            SplitPart {
                id: "surface".into(),
                title: "Surface".into(),
                instructions: "add surface.txt".into(),
                depends_on: vec!["foundation".into()],
                acceptance: Some(
                    "test -f surface.txt && test -f foundation.txt && test -f fixed.txt".into(),
                ),
                owns: vec!["surface.txt".into()],
                interface: Some("surface consumes foundation".into()),
                estimate_seconds: Some(60),
            },
        ];
        let routing = Routing {
            scope: "demo".into(),
            agent: Some("shell".into()),
            ..Default::default()
        };
        let workflow = engine
            .start_decomposition_workflow(&parent, &parts, &routing, &Caller::Owner)
            .await
            .unwrap();
        let integration_path =
            PathBuf::from(workflow.integration.as_ref().unwrap().worktree_path.clone());
        let children = engine
            .l4.store
            .list(&factory_core::TaskFilter {
                parent_task_id: Some(parent.id.clone()),
                ..Default::default()
            })
            .await
            .unwrap();
        let foundation = children
            .iter()
            .find(|task| task.decomposition_part.as_deref() == Some("foundation"))
            .unwrap();
        let surface = children
            .iter()
            .find(|task| task.decomposition_part.as_deref() == Some("surface"))
            .unwrap();
        assert_eq!(surface.depends_on, vec![foundation.id.clone()]);

        let foundation_run = wait_for_worktree_run(&engine, &foundation.id).await;
        let foundation_path = PathBuf::from(foundation_run.worktree_path.clone().unwrap());
        std::fs::write(foundation_path.join("foundation.txt"), "foundation\n").unwrap();
        git_ok(&foundation_path, &["add", "foundation.txt"]).await;
        git_ok(&foundation_path, &["commit", "-q", "-m", "foundation"]).await;
        engine
            .l4_service()
            .report(
                &foundation.id,
                TaskReport {
                    artifacts: Vec::new(),
                    status: Some(RunStatus::Done),
                    result: Some("foundation complete".into()),
                    token: foundation_run.token,
                    message: None,
                    send_to: None,
                    error: None,
                },
            )
            .await
            .unwrap();
        engine.advance_workflow(&workflow.id).await.unwrap();

        let surface_run = wait_for_worktree_run(&engine, &surface.id).await;
        let surface_path = PathBuf::from(surface_run.worktree_path.clone().unwrap());
        assert!(
            surface_path.join("foundation.txt").exists(),
            "a dependant branches from the integrated predecessor"
        );
        std::fs::write(surface_path.join("surface.txt"), "surface\n").unwrap();
        git_ok(&surface_path, &["add", "surface.txt"]).await;
        git_ok(&surface_path, &["commit", "-q", "-m", "surface"]).await;
        engine
            .l4_service()
            .report(
                &surface.id,
                TaskReport {
                    artifacts: Vec::new(),
                    status: Some(RunStatus::Done),
                    result: Some("surface complete".into()),
                    token: surface_run.token,
                    message: None,
                    send_to: None,
                    error: None,
                },
            )
            .await
            .unwrap();
        engine.advance_workflow(&workflow.id).await.unwrap();

        let rework_run = wait_for_worktree_run(&engine, &surface.id).await;
        assert_eq!(
            rework_run.attempt, 2,
            "the failed combined check returns only its owning child"
        );
        let rework_path = PathBuf::from(rework_run.worktree_path.clone().unwrap());
        assert!(
            rework_path.join("surface.txt").exists(),
            "rework starts from the combined branch that failed"
        );
        std::fs::write(rework_path.join("fixed.txt"), "fixed\n").unwrap();
        git_ok(&rework_path, &["add", "fixed.txt"]).await;
        git_ok(&rework_path, &["commit", "-q", "-m", "fix combined check"]).await;
        engine
            .l4_service()
            .report(
                &surface.id,
                TaskReport {
                    artifacts: Vec::new(),
                    status: Some(RunStatus::Done),
                    result: Some("combined check fixed".into()),
                    token: rework_run.token,
                    message: None,
                    send_to: None,
                    error: None,
                },
            )
            .await
            .unwrap();
        engine.advance_workflow(&workflow.id).await.unwrap();

        let finished = engine.workflow_run(&workflow.id).await.unwrap();
        assert_eq!(finished.status, WorkflowRunStatus::Done);
        let integration = finished.integration.unwrap();
        assert_eq!(integration.merged_nodes, vec!["foundation", "surface"]);
        assert!(integration.checks_passed);
        assert!(!integration.cleanup_complete, "a local-only integration has not backed up its commits");
        assert!(integration_path.exists());
        assert!(foundation_path.exists());
        assert!(surface_path.exists());
        assert_eq!(rework_path, surface_path, "rework keeps the same task workspace");
        let remote = root.join("backup.git");
        std::fs::create_dir(&remote).unwrap();
        git_ok(&remote, &["init", "-q", "--bare"]).await;
        git_ok(&repo, &["remote", "add", "origin", remote.to_str().unwrap()]).await;
        git_ok(&integration_path, &["push", "-q", "-u", "origin", &integration.branch]).await;
        engine.l4_service().sweep_workspaces().await;
        assert!(engine.workflow_run(&workflow.id).await.unwrap().integration.unwrap().cleanup_complete,
            "retained receipts: {:?}", engine.l4.workspaces.records().await.unwrap());
        assert!(!integration_path.exists());
        assert!(!foundation_path.exists());
        assert!(!surface_path.exists());
        assert!(engine
            .require(&parent.id)
            .await
            .unwrap()
            .result
            .unwrap()
            .contains("Integrated on"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn integration_pushes_once_and_opens_one_parent_pr_without_merging_it() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("factory-pr-test-{}", uuid::Uuid::new_v4()));
        let repo = root.join("repo");
        let remote = root.join("remote.git");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&remote).unwrap();
        git_ok(
            &remote,
            &["init", "--bare", "-q", "--initial-branch", "main"],
        )
        .await;
        git_ok(&repo, &["init", "-q", "-b", "main"]).await;
        git_ok(&repo, &["config", "user.email", "factory@example.test"]).await;
        git_ok(&repo, &["config", "user.name", "Factory Test"]).await;
        std::fs::write(repo.join("README.md"), "base\n").unwrap();
        git_ok(&repo, &["add", "README.md"]).await;
        git_ok(&repo, &["commit", "-q", "-m", "base"]).await;
        git_ok(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        )
        .await;
        git_ok(&repo, &["push", "-q", "-u", "origin", "main"]).await;

        let engine = engine_in_git_scope(root.clone(), repo.clone());
        let parent = engine
            .create(NewTask {
                title: "One reviewed change".into(),
                scope: Some("demo".into()),
                worktree: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        let definition = WorkflowDefinition::from_draft(WorkflowDraft {
            name: "integration".into(),
            scope: "demo".into(),
            nodes: vec![node("work")],
            ..Default::default()
        });
        let mut run = WorkflowRun::new(definition, factory_core::workflow::WorkflowActor::Owner);
        let integration_dir = root.join("integration");
        worktree::create(
            &repo,
            &integration_dir,
            "factory/issue-180",
            Some("origin/main"),
        )
        .await
        .unwrap();
        std::fs::write(integration_dir.join("done.txt"), "done\n").unwrap();
        git_ok(&integration_dir, &["add", "done.txt"]).await;
        git_ok(&integration_dir, &["commit", "-q", "-m", "integrated"]).await;
        run.integration = Some(WorkflowIntegration {
            parent_task_id: parent.id,
            base_ref: "origin/main".into(),
            branch: "factory/issue-180".into(),
            worktree_path: integration_dir.display().to_string(),
            repository: Some("acme/widgets".into()),
            issue_number: Some(180),
            checks_passed: true,
            ..Default::default()
        });

        let log = root.join("gh.log");
        let gh = root.join("gh");
        std::fs::write(
            &gh,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1 $2\" = \"pr view\" ]; then exit 1; fi\nprintf '%s\\n' 'https://github.com/acme/widgets/pull/99'\n",
                log.display()
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&gh).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&gh, permissions).unwrap();

        let url = engine
            .ensure_integration_pr_with_gh(&run, &gh)
            .await
            .unwrap();
        assert_eq!(
            url.as_deref(),
            Some("https://github.com/acme/widgets/pull/99")
        );
        let calls = std::fs::read_to_string(log).unwrap();
        assert_eq!(calls.matches("pr create").count(), 1, "{calls}");
        assert!(calls.contains("--base main"), "{calls}");
        assert!(calls.contains("--head factory/issue-180"), "{calls}");
        assert!(calls.contains("Closes #180"), "{calls}");
        let remote_branch = tokio::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["ls-remote", "--heads", "origin", "factory/issue-180"])
            .output()
            .await
            .unwrap();
        assert!(remote_branch.status.success());
        assert!(
            !remote_branch.stdout.is_empty(),
            "the integration branch was pushed"
        );
        let _ = worktree::remove(&repo, &integration_dir, "factory/issue-180").await;
        let _ = std::fs::remove_dir_all(root);
    }

    // --- #235: every part through a part workflow ---------------------------

    /// A repository with one commit on `main`, for an integrated plan.
    async fn epic_repo(root: &Path) -> PathBuf {
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git_ok(&repo, &["init", "-q", "-b", "main"]).await;
        git_ok(&repo, &["config", "user.email", "factory@example.test"]).await;
        git_ok(&repo, &["config", "user.name", "Factory Test"]).await;
        std::fs::write(repo.join("README.md"), "base\n").unwrap();
        git_ok(&repo, &["add", "README.md"]).await;
        git_ok(&repo, &["commit", "-q", "-m", "base"]).await;
        repo
    }

    /// `engine_in_git_scope`, kept on disk: dropping one and building
    /// another over the same root is a daemon restart.
    fn epic_engine_on_disk(root: &Path, repo: &Path) -> Arc<Engine> {
        let db = root.join("factory.db");
        let mut registry = Registry::with_builtins();
        registry.add_runtime(Arc::new(QuietRuntime::default()), "test");
        Arc::new(
            Engine::new(
                Factory { root: root.to_path_buf(), config: git_scope_config(repo.to_path_buf()) },
                registry,
                Arc::new(SqliteStore::open(&db).unwrap()),
                PathBuf::from("factory"),
                Vec::new(),
            )
            .with_workflow_store(crate::workflows::WorkflowStore::open(&db).unwrap()),
        )
    }

    /// implement (a worktree of its own) -> review (none), the review
    /// sending work back at most `max_rounds` times -- the shape of
    /// `workflows/epic-part.yaml`, on the `shell` agent.
    async fn part_workflow(engine: &Arc<Engine>, max_rounds: u32) -> WorkflowDefinition {
        let mut implement = node("implement");
        implement.task.worktree = Some(true);
        implement.task.title = "Implement {{part_title}}".into();
        implement.task.instructions = "{{part_instructions}}\nDone when: {{part_acceptance}}".into();
        let mut review = node("review");
        review.task.title = "Review {{part_title}}".into();
        review.exits = vec![WorkflowExit {
            to: "implement".into(),
            check: None,
            agent: Some("concrete findings the implementer can fix alone".into()),
            max_rounds: Some(max_rounds),
        }];
        engine
            .create_workflow(WorkflowDraft {
                name: "part-flow".into(),
                scope: "demo".into(),
                part: Some(factory_core::workflow::PartSpec { deliverable: Some("implement".into()), terminal: None }),
                nodes: vec![implement, review],
                edges: vec![edge("implement", "review")],
                ..Default::default()
            })
            .await
            .unwrap()
    }

    fn epic_part(id: &str, depends_on: &[&str], acceptance: &str) -> SplitPart {
        SplitPart {
            id: id.into(),
            title: format!("Part {}", id.to_uppercase()),
            instructions: format!("build {id}"),
            depends_on: depends_on.iter().map(|d| d.to_string()).collect(),
            acceptance: Some(acceptance.into()),
            owns: vec![format!("{id}.txt")],
            interface: Some(format!("{id}.txt exists")),
            estimate_seconds: Some(600),
        }
    }

    fn task_of<'a>(tasks: &'a [factory_core::Task], node: &str) -> &'a factory_core::Task {
        tasks
            .iter()
            .find(|task| task.workflow_origin.as_ref().is_some_and(|origin| origin.node_id == node))
            .unwrap_or_else(|| panic!("no task for node {node}"))
    }

    /// Report `done` for the task's active run, optionally sending the work
    /// back to `send_to`, then advance its workflow.
    async fn report_done(engine: &Arc<Engine>, task_id: &str, result: &str, send_to: Option<&str>) {
        let run = loop {
            if let Some(run) = engine.l4.store.active_run(task_id).await.unwrap() {
                break run;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        engine
            .l4_service()
            .report(
                task_id,
                TaskReport {
                    artifacts: Vec::new(),
                    status: Some(RunStatus::Done),
                    message: None,
                    result: Some(result.into()),
                    send_to: send_to.map(str::to_string),
                    error: None,
                    token: run.token,
                },
            )
            .await
            .unwrap();
        engine.sync_workflow_for_task(task_id).await;
    }

    /// Write `files` in the active run's worktree and commit them.
    async fn commit_in(engine: &Engine, task_id: &str, files: &[(&str, &str)]) -> PathBuf {
        let run = wait_for_worktree_run(engine, task_id).await;
        let dir = PathBuf::from(run.worktree_path.unwrap());
        for (name, text) in files {
            std::fs::write(dir.join(name), text).unwrap();
            git_ok(&dir, &["add", name]).await;
        }
        git_ok(&dir, &["commit", "-q", "-m", "work"]).await;
        dir
    }

    async fn merges_on(dir: &Path, branch: &str) -> usize {
        let output = tokio::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["rev-list", "--count", "--merges", "--first-parent", branch])
            .output()
            .await
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().parse().unwrap()
    }

    #[tokio::test]
    async fn a_part_workflow_is_copied_once_per_part_and_parts_join_terminal_to_entry() {
        let engine = engine();
        let template = part_workflow(&engine, 5).await;
        let parent = engine
            .create(NewTask {
                title: "Epic".into(),
                instructions: "the whole change".into(),
                scope: Some("demo".into()),
                worktree: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        let parts = vec![
            epic_part("a", &[], "test -f a.txt"),
            epic_part("b", &[], "test -f b.txt"),
            epic_part("c", &["a"], "test -f c.txt"),
        ];
        let routing = Routing {
            scope: "demo".into(),
            agent: Some("shell".into()),
            workflow: Some(template.id.clone()),
            agents: BTreeMap::from([("review".to_string(), "shell".to_string())]),
            ..Default::default()
        };
        let run = engine.start_decomposition_workflow(&parent, &parts, &routing, &Caller::Owner).await.unwrap();

        let definition = &run.definition;
        let task_nodes: Vec<&str> = definition
            .nodes
            .iter()
            .filter(|node| node.kind == WorkflowNodeKind::Task)
            .map(|node| node.id.as_str())
            .collect();
        assert_eq!(task_nodes, ["a-implement", "a-review", "b-implement", "b-review", "c-implement", "c-review"]);
        let expand = definition.nodes.iter().find(|node| node.kind == WorkflowNodeKind::Expand).unwrap();
        assert_eq!(expand.expand.as_ref().unwrap().children, task_nodes, "expand lists every copied node");
        let edges: Vec<(&str, &str)> = definition.edges.iter().map(|e| (e.from.as_str(), e.to.as_str())).collect();
        for expected in [
            ("expand", "a-implement"),
            ("expand", "b-implement"),
            ("a-implement", "a-review"),
            ("a-review", "c-implement"),
        ] {
            assert!(edges.contains(&expected), "{expected:?} in {edges:?}");
        }
        assert!(!edges.contains(&("expand", "c-implement")), "a dependant starts from its prerequisite, not expand");
        assert!(definition.description.contains("part workflow part-flow"), "{}", definition.description);
        assert!(run.integration.is_none(), "this scope is not a git repository");

        let children = tasks(&engine).await;
        assert_eq!(children.iter().filter(|task| task.parent_task_id.as_deref() == Some(parent.id.as_str())).count(), 6);
        let implement = task_of(&children, "a-implement");
        assert_eq!(implement.title, "Implement Part A");
        assert_eq!(implement.instructions, "build a\nDone when: test -f a.txt");
        assert_eq!(implement.parent_task_id.as_deref(), Some(parent.id.as_str()));
        assert_eq!(implement.decomposition_part.as_deref(), Some("a"));
        assert_eq!(implement.labels[factory_core::intake::PART_LABEL], "a");
        assert_eq!(implement.labels[factory_core::intake::PARENT_LABEL], parent.id);
        assert_eq!(implement.estimate_seconds, Some(600), "the part's estimate is on its deliverable");
        assert!(!implement.worktree, "no git repository, so no worktree for any copy");
        let review = task_of(&children, "a-review");
        assert_eq!(review.title, "Review Part A");
        assert_eq!(review.estimate_seconds, None);
        assert_eq!(review.decomposition_part.as_deref(), Some("a"));
        assert_eq!(review.depends_on, vec![implement.id.clone()]);
        let dependant = task_of(&children, "c-implement");
        assert_eq!(dependant.depends_on, vec![review.id.clone()], "c follows a's terminal");
        assert!(dependant.after.is_some(), "and waits for it");
    }

    #[tokio::test]
    async fn a_plan_without_a_part_workflow_is_generated_exactly_as_before() {
        let root = std::env::temp_dir().join(format!("factory-epic-plain-{}", uuid::Uuid::new_v4()));
        let repo = epic_repo(&root).await;
        let engine = engine_in_git_scope(root.clone(), repo);
        let parent = engine
            .create(NewTask {
                title: "Two slices".into(),
                instructions: "one result".into(),
                scope: Some("demo".into()),
                worktree: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        let parts = vec![epic_part("a", &[], "test -f a.txt"), epic_part("b", &["a"], "test -f b.txt")];
        let routing = Routing { scope: "demo".into(), agent: Some("shell".into()), ..Default::default() };
        let run = engine.start_decomposition_workflow(&parent, &parts, &routing, &Caller::Owner).await.unwrap();
        let definition = &run.definition;
        assert_eq!(definition.description, format!("Generated from approved intake task {}", parent.id));
        let ids: Vec<&str> = definition.nodes.iter().map(|node| node.id.as_str()).collect();
        assert_eq!(ids, ["expand", "a", "b"]);
        let edges: Vec<(&str, &str, &str)> =
            definition.edges.iter().map(|e| (e.id.as_str(), e.from.as_str(), e.to.as_str())).collect();
        assert_eq!(edges, [("expand-a", "expand", "a"), ("a-b", "a", "b")]);
        let expand = definition.nodes[0].expand.as_ref().unwrap();
        assert_eq!(expand.children, ["a", "b"]);
        let b = &definition.nodes[2];
        assert_eq!(b.task.title, "Part B");
        assert_eq!(b.task.estimate_seconds, Some(600));
        assert_eq!(b.task.worktree, Some(true));
        assert_eq!(
            b.task.instructions,
            format!(
                "build b\n\nDone when: test -f b.txt\n\nOwned surface: b.txt\n\nInterface / hand-off: b.txt exists\n\n---\nPart b of task {} (Two slices). The parent request, for context:\n\none result",
                parent.id
            )
        );
        let integration = run.integration.as_ref().unwrap();
        assert_eq!(integration.parts[1].node_id, "b");
        assert_eq!(integration.parts[1].terminal_node, None, "a single-node part plays every role");
        assert_eq!(integration.parts[1].terminal(), "b");
        let _ = engine.cancel_workflow(&run.id).await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_template_that_no_longer_keeps_the_contract_is_refused_at_expansion() {
        let engine = engine();
        // No `part:` block: the roles are derived, so the template can be
        // edited into something ambiguous and still be stored.
        let mut implement = node("implement");
        implement.task.worktree = Some(true);
        let template = create(&engine, vec![implement, node("review")], vec![edge("implement", "review")]).await;
        let mut draft = WorkflowDraft {
            name: template.name.clone(),
            scope: "demo".into(),
            nodes: template.nodes.clone(),
            edges: template.edges.clone(),
            ..Default::default()
        };
        draft.nodes[1].task.worktree = Some(true);
        engine.update_workflow(&template.id, draft).await.unwrap();

        let parent = engine
            .create(NewTask { title: "Epic".into(), scope: Some("demo".into()), worktree: Some(false), ..Default::default() })
            .await
            .unwrap();
        let parts = vec![epic_part("a", &[], "true"), epic_part("b", &[], "true")];
        let routing = Routing { scope: "demo".into(), workflow: Some(template.id.clone()), ..Default::default() };
        let refused = engine
            .start_decomposition_workflow(&parent, &parts, &routing, &Caller::Owner)
            .await
            .unwrap_err()
            .to_string();
        assert!(refused.contains("part workflow pipeline"), "{refused}");
        assert!(refused.contains("name the one whose branch is integrated with part.deliverable"), "{refused}");
        assert_eq!(tasks(&engine).await.len(), 1, "nothing but the parent exists");

        // Two parts whose namespaced ids collide are refused by name.
        let mut implement = node("x-y");
        implement.task.worktree = Some(true);
        let colliding = engine
            .create_workflow(WorkflowDraft {
                name: "colliding".into(),
                scope: "demo".into(),
                nodes: vec![node("y"), implement],
                edges: vec![edge("y", "x-y")],
                ..Default::default()
            })
            .await
            .unwrap();
        let parts = vec![epic_part("a", &[], "true"), epic_part("a-x", &[], "true")];
        let routing = Routing { scope: "demo".into(), workflow: Some(colliding.id), ..Default::default() };
        let refused = engine
            .start_decomposition_workflow(&parent, &parts, &routing, &Caller::Owner)
            .await
            .unwrap_err()
            .to_string();
        assert!(refused.contains("a-x-y") && refused.contains("rename a part"), "{refused}");
    }

    #[tokio::test]
    async fn each_part_is_reviewed_in_its_own_loop_and_again_after_integration_sends_it_back() {
        let root = std::env::temp_dir().join(format!("factory-epic-{}", uuid::Uuid::new_v4()));
        let repo = epic_repo(&root).await;
        let engine = epic_engine_on_disk(&root, &repo);
        let template = part_workflow(&engine, 5).await;
        let parent = engine
            .create(NewTask {
                title: "Epic in three parts".into(),
                instructions: "one integrated result".into(),
                scope: Some("demo".into()),
                worktree: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        let parts = vec![
            epic_part("a", &[], "test -f a.txt"),
            epic_part("b", &[], "test -f b.txt"),
            epic_part("c", &["a"], "test -f c.txt && test -f a.txt && test -f fixed.txt"),
        ];
        let routing = Routing {
            scope: "demo".into(),
            agent: Some("shell".into()),
            workflow: Some(template.id.clone()),
            ..Default::default()
        };
        let run = engine.start_decomposition_workflow(&parent, &parts, &routing, &Caller::Owner).await.unwrap();
        let integration = run.integration.clone().unwrap();
        let integration_dir = PathBuf::from(&integration.worktree_path);
        assert_eq!(
            integration.parts.iter().map(|part| (part.node_id.as_str(), part.terminal())).collect::<Vec<_>>(),
            [("a-implement", "a-review"), ("b-implement", "b-review"), ("c-implement", "c-review")]
        );
        let all = tasks(&engine).await;
        assert_eq!(all.iter().filter(|task| task.parent_task_id.as_deref() == Some(parent.id.as_str())).count(), 6);
        let id = |node: &str| task_of(&all, node).id.clone();

        // A and B start together; C waits for A.
        wait_for_worktree_run(&engine, &id("a-implement")).await;
        wait_for_worktree_run(&engine, &id("b-implement")).await;
        assert!(engine.l4.store.active_run(&id("c-implement")).await.unwrap().is_none());

        // A's review loops inside A only.
        commit_in(&engine, &id("a-implement"), &[("a.txt", "a\n"), ("README.md", "base\nalpha\n")]).await;
        report_done(&engine, &id("a-implement"), "branch with a", None).await;
        wait_for_attempt(&engine, &id("a-review"), 1).await;
        report_done(&engine, &id("a-review"), "1. a.txt needs a second line", Some("a-implement")).await;
        wait_for_attempt(&engine, &id("a-implement"), 2).await;
        let midway = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(node_run(&midway, "a-review").round, 1);
        assert_eq!(node_run(&midway, "b-review").round, 0, "the other part's budget is its own");
        assert!(midway.integration.as_ref().unwrap().merged_nodes.is_empty());
        assert!(engine.l4.store.active_run(&id("c-implement")).await.unwrap().is_none(), "C waits through A's loop");

        commit_in(&engine, &id("a-implement"), &[("a.txt", "a\nsecond\n")]).await;
        report_done(&engine, &id("a-implement"), "fixed", None).await;
        wait_for_attempt(&engine, &id("a-review"), 2).await;
        report_done(&engine, &id("a-review"), "passes", None).await;
        let merged = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(merged.integration.as_ref().unwrap().merged_nodes, ["a-implement"]);
        let c_run = wait_for_worktree_run(&engine, &id("c-implement")).await;
        assert!(
            PathBuf::from(c_run.worktree_path.unwrap()).join("a.txt").exists(),
            "C branches from the integration branch A was merged into"
        );

        // Restart in the middle of integration: the same branch, the same
        // ledger, nothing merged twice.
        drop(engine);
        let engine = epic_engine_on_disk(&root, &repo);
        engine.recover_workflows().await;
        engine.recover_workflows().await;
        let recovered = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(recovered.integration.as_ref().unwrap().merged_nodes, ["a-implement"]);
        assert_eq!(recovered.integration.as_ref().unwrap().worktree_path, integration.worktree_path);
        assert_eq!(merges_on(&integration_dir, &integration.branch).await, 1);
        assert_eq!(tasks(&engine).await.len(), 7, "the parent and its six tasks, no more");

        // B conflicts with A on the integration branch: it goes back to
        // B's implement, and B's review runs again before B is merged.
        let b_dir = commit_in(&engine, &id("b-implement"), &[("b.txt", "b\n"), ("README.md", "base\nbeta\n")]).await;
        report_done(&engine, &id("b-implement"), "branch with b", None).await;
        wait_for_attempt(&engine, &id("b-review"), 1).await;
        report_done(&engine, &id("b-review"), "passes", None).await;
        let rework = wait_for_worktree_run(&engine, &id("b-implement")).await;
        assert_eq!(rework.attempt, 2, "the merge conflict returns B's deliverable");
        assert!(rework.feedback.as_ref().unwrap().feedback.as_deref().unwrap().contains("merge the integration branch"));
        let sent_back = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(sent_back.integration.as_ref().unwrap().merged_nodes, ["a-implement"]);
        let b_review = node_run(&sent_back, "b-review");
        assert_eq!(b_review.status, WorkflowNodeStatus::Unstarted, "B's review is not left showing the old done");
        assert_eq!((b_review.round, b_review.integration_rounds, b_review.exit_rounds()), (1, 1, 0));
        assert_eq!(node_run(&sent_back, "b-implement").integration_rounds, 1);
        assert_eq!(engine.l4.store.runs(&id("b-review"), 10).await.unwrap().len(), 1);

        let merge = tokio::process::Command::new("git")
            .arg("-C")
            .arg(&b_dir)
            .args(["merge", "--no-edit", &integration.branch])
            .output()
            .await
            .unwrap();
        assert!(!merge.status.success(), "the conflict is real");
        commit_in(&engine, &id("b-implement"), &[("README.md", "base\nalpha\nbeta\n")]).await;
        report_done(&engine, &id("b-implement"), "merged the integration branch", None).await;
        let review_again = wait_for_attempt(&engine, &id("b-review"), 2).await;
        assert_eq!(review_again.workflow_round, 1);
        assert_eq!(
            engine.workflow_run(&run.id).await.unwrap().integration.unwrap().merged_nodes,
            ["a-implement"],
            "B waits for its review"
        );
        report_done(&engine, &id("b-review"), "passes again", None).await;
        assert_eq!(
            engine.workflow_run(&run.id).await.unwrap().integration.unwrap().merged_nodes,
            ["a-implement", "b-implement"]
        );

        // C runs its own chain. Its acceptance fails on the combined tree,
        // which sends C's deliverable back, and C's review runs again before
        // C is merged a second time.
        commit_in(&engine, &id("c-implement"), &[("c.txt", "c\n")]).await;
        report_done(&engine, &id("c-implement"), "branch with c", None).await;
        wait_for_attempt(&engine, &id("c-review"), 1).await;
        report_done(&engine, &id("c-review"), "passes", None).await;
        let rework = wait_for_worktree_run(&engine, &id("c-implement")).await;
        assert_eq!(rework.attempt, 2);
        assert!(rework.feedback.as_ref().unwrap().feedback.as_deref().unwrap().contains("combined integration check for part c failed"));
        let checking = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(checking.integration.as_ref().unwrap().merged_nodes, ["a-implement", "b-implement"]);
        assert!(!checking.integration.as_ref().unwrap().checks_passed);
        assert_eq!(node_run(&checking, "c-review").status, WorkflowNodeStatus::Unstarted);
        commit_in(&engine, &id("c-implement"), &[("fixed.txt", "fixed\n")]).await;
        report_done(&engine, &id("c-implement"), "combined check fixed", None).await;
        wait_for_attempt(&engine, &id("c-review"), 2).await;
        assert_eq!(engine.workflow_run(&run.id).await.unwrap().status, WorkflowRunStatus::Running);
        report_done(&engine, &id("c-review"), "passes again", None).await;
        let finished = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(finished.status, WorkflowRunStatus::Done, "{:?}", finished.error);
        let integration = finished.integration.unwrap();
        assert_eq!(integration.merged_nodes, ["a-implement", "b-implement", "c-implement"]);
        assert!(integration.checks_passed);
        assert_eq!(merges_on(&integration_dir, &integration.branch).await, 4, "C was merged again after its fix");
        assert!(engine.require(&parent.id).await.unwrap().result.unwrap().contains("Integrated on"));
        let integration_dirs = std::fs::read_dir(engine.factory_snapshot().worktrees_dir())
            .unwrap()
            .filter(|entry| entry.as_ref().unwrap().file_name().to_string_lossy().starts_with("integration-"))
            .count();
        assert_eq!(integration_dirs, 1, "one integration branch, across the restart");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn integration_rework_is_bounded_per_part_and_not_spent_by_its_review_loop() {
        let root = std::env::temp_dir().join(format!("factory-epic-exhaust-{}", uuid::Uuid::new_v4()));
        let repo = epic_repo(&root).await;
        let engine = engine_in_git_scope(root.clone(), repo);
        let template = part_workflow(&engine, 5).await;
        let parent = engine
            .create(NewTask { title: "Epic".into(), scope: Some("demo".into()), worktree: Some(false), ..Default::default() })
            .await
            .unwrap();
        let parts = vec![epic_part("a", &[], "true"), epic_part("b", &["a"], "true")];
        let routing = Routing {
            scope: "demo".into(),
            agent: Some("shell".into()),
            workflow: Some(template.id.clone()),
            ..Default::default()
        };
        let run = engine.start_decomposition_workflow(&parent, &parts, &routing, &Caller::Owner).await.unwrap();
        let all = tasks(&engine).await;
        let dependant = task_of(&all, "b-implement").id.clone();
        let implement = task_of(&all, "a-implement").id.clone();
        let review = task_of(&all, "a-review").id.clone();

        // An uncommitted file: the integrator refuses the worktree every
        // time A's review passes.
        let first = wait_for_worktree_run(&engine, &implement).await;
        std::fs::write(PathBuf::from(first.worktree_path.unwrap()).join("a.txt"), "never committed\n").unwrap();
        report_done(&engine, &implement, "done", None).await;
        // One round of A's own review loop first.
        wait_for_attempt(&engine, &review, 1).await;
        report_done(&engine, &review, "1. try again", Some("a-implement")).await;
        let mut implement_attempt = 2;
        let mut review_attempt = 2;
        for round in 1..=3 {
            wait_for_attempt(&engine, &implement, implement_attempt).await;
            report_done(&engine, &implement, "done", None).await;
            wait_for_attempt(&engine, &review, review_attempt).await;
            report_done(&engine, &review, "passes", None).await;
            let current = engine.workflow_run(&run.id).await.unwrap();
            assert_eq!(node_run(&current, "a-implement").integration_rounds, round, "integration round {round}");
            assert_eq!(current.status, WorkflowRunStatus::Running);
            implement_attempt += 1;
            review_attempt += 1;
        }
        wait_for_attempt(&engine, &implement, implement_attempt).await;
        report_done(&engine, &implement, "done", None).await;
        wait_for_attempt(&engine, &review, review_attempt).await;
        report_done(&engine, &review, "passes", None).await;

        let failed = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(failed.status, WorkflowRunStatus::Failed);
        assert_eq!(failed.failure_node_id.as_deref(), Some("a-implement"));
        assert!(failed.error.as_deref().unwrap().contains("integration rework exhausted after 3 rounds"), "{:?}", failed.error);
        assert_eq!(node_run(&failed, "a-review").exit_rounds(), 1, "the review spent one round of its own");
        assert!(
            engine.l4.store.runs(&dependant, 10).await.unwrap().is_empty(),
            "a's review was done four times, but a was never merged, so b never started"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_restart_in_the_middle_of_expansion_fills_in_the_same_task_once() {
        let root = std::env::temp_dir().join(format!("factory-epic-expand-{}", uuid::Uuid::new_v4()));
        let repo = epic_repo(&root).await;
        let engine = epic_engine_on_disk(&root, &repo);
        let template = part_workflow(&engine, 5).await;
        let parent = engine
            .create(NewTask { title: "Epic".into(), scope: Some("demo".into()), worktree: Some(false), ..Default::default() })
            .await
            .unwrap();
        let parts = vec![epic_part("a", &[], "true"), epic_part("b", &["a"], "true")];
        let routing = Routing {
            scope: "demo".into(),
            agent: Some("shell".into()),
            workflow: Some(template.id.clone()),
            ..Default::default()
        };
        let run = engine.start_decomposition_workflow(&parent, &parts, &routing, &Caller::Owner).await.unwrap();
        wait_for_worktree_run(&engine, &task_of(&tasks(&engine).await, "a-implement").id).await;

        // The crash window: b-review's id was chosen and persisted, but its
        // task was never created.
        let missing = {
            let _guard = engine.l4.workflow_edit.lock().await;
            let mut torn = engine.workflow_run(&run.id).await.unwrap();
            let node = torn.nodes.iter_mut().find(|node| node.node_id == "b-review").unwrap();
            node.task_created = false;
            let id = node.task_id.clone().unwrap();
            engine.l4.workflows.put_run(&torn).await.unwrap();
            assert!(engine.l4.store.delete(&id).await.unwrap());
            id
        };
        drop(engine);

        let engine = epic_engine_on_disk(&root, &repo);
        engine.recover_workflows().await;
        engine.recover_workflows().await;
        let all = tasks(&engine).await;
        assert_eq!(all.len(), 5, "the parent and four tasks: {:?}", all.iter().map(|t| &t.title).collect::<Vec<_>>());
        assert_eq!(task_of(&all, "b-review").id, missing, "recovery fills in exactly the persisted id");
        let recovered = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(recovered.integration, run.integration, "the same integration branch and worktree");
        let integration_dirs = std::fs::read_dir(engine.factory_snapshot().worktrees_dir())
            .unwrap()
            .filter(|entry| entry.as_ref().unwrap().file_name().to_string_lossy().starts_with("integration-"))
            .count();
        assert_eq!(integration_dirs, 1);
        let _ = engine.cancel_workflow(&run.id).await;
        let _ = std::fs::remove_dir_all(root);
    }

    // --- #235 follow-up: QA of #238 -----------------------------------------

    /// Report `done` for the task's active run without advancing its
    /// workflow, so several reports can land before one advance does.
    async fn report_only(engine: &Arc<Engine>, task_id: &str, result: &str, send_to: Option<&str>) {
        let run = loop {
            if let Some(run) = engine.l4.store.active_run(task_id).await.unwrap() {
                break run;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        engine
            .l4_service()
            .report(
                task_id,
                TaskReport {
                    artifacts: Vec::new(),
                    status: Some(RunStatus::Done),
                    message: None,
                    result: Some(result.into()),
                    send_to: send_to.map(str::to_string),
                    error: None,
                    token: run.token,
                },
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn each_parts_review_findings_reach_only_that_parts_implement() {
        let engine = engine();
        let template = part_workflow(&engine, 5).await;
        let parent = engine
            .create(NewTask { title: "Epic".into(), scope: Some("demo".into()), worktree: Some(false), ..Default::default() })
            .await
            .unwrap();
        let parts = vec![epic_part("a", &[], "true"), epic_part("b", &[], "true")];
        let routing = Routing {
            scope: "demo".into(),
            agent: Some("shell".into()),
            workflow: Some(template.id.clone()),
            ..Default::default()
        };
        let run = engine.start_decomposition_workflow(&parent, &parts, &routing, &Caller::Owner).await.unwrap();
        let all = tasks(&engine).await;
        let id = |node: &str| task_of(&all, node).id.clone();
        for part in ["a", "b"] {
            report_done(&engine, &id(&format!("{part}-implement")), "built", None).await;
            wait_for_attempt(&engine, &id(&format!("{part}-review")), 1).await;
        }

        // Both reviews send back before one advance sees either: one
        // advance evaluates both exits, at the same round number.
        report_only(&engine, &id("a-review"), "FINDING FOR A", Some("a-implement")).await;
        report_only(&engine, &id("b-review"), "FINDING FOR B", Some("b-implement")).await;
        engine.advance_workflow(&run.id).await.unwrap();
        for (part, mine, theirs) in [("a", "FINDING FOR A", "FINDING FOR B"), ("b", "FINDING FOR B", "FINDING FOR A")] {
            let rework = wait_for_attempt(&engine, &id(&format!("{part}-implement")), 2).await;
            let feedback = rework.feedback.as_ref().unwrap().feedback.clone().unwrap();
            assert!(feedback.contains(mine) && !feedback.contains(theirs), "{part}-implement was dispatched with {feedback:?}");
        }

        // One after the other: the second send-back leaves the first
        // part's stored request alone.
        for part in ["a", "b"] {
            report_done(&engine, &id(&format!("{part}-implement")), "fixed", None).await;
            wait_for_attempt(&engine, &id(&format!("{part}-review")), 2).await;
        }
        report_done(&engine, &id("b-review"), "SECOND FOR B", Some("b-implement")).await;
        wait_for_attempt(&engine, &id("b-implement"), 3).await;
        report_done(&engine, &id("a-review"), "SECOND FOR A", Some("a-implement")).await;
        let a_rework = wait_for_attempt(&engine, &id("a-implement"), 3).await;
        assert!(a_rework.feedback.unwrap().feedback.unwrap().contains("SECOND FOR A"));
        let after = engine.workflow_run(&run.id).await.unwrap();
        let b_request = node_run(&after, "b-implement").rework_request.clone().unwrap();
        assert_eq!(b_request.from_node, "b-review");
        assert_eq!(b_request.feedback.as_deref(), Some("SECOND FOR B"), "A's send-back did not overwrite B's request");
        let _ = engine.cancel_workflow(&run.id).await;
    }

    #[tokio::test]
    async fn a_part_workflow_cannot_be_started_as_itself() {
        let engine = engine();
        let template = part_workflow(&engine, 5).await;
        let refused = engine.start_workflow(&template.id, Default::default(), &Caller::Owner).await.unwrap_err().to_string();
        assert!(refused.contains("is a part workflow: it runs only inside an Intake decomposition"), "{refused}");
        assert!(tasks(&engine).await.is_empty(), "nothing titled \"Implement {{{{part_title}}}}\" exists");
        assert!(engine.l4.workflows.runs(Some(&template.id), None, 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_epic_part_review_stays_on_its_own_branch_so_factory_can_resume_and_release_it() {
        let draft: WorkflowDraft =
            serde_yaml_ng::from_str(include_str!("../../../../workflows/epic-part.yaml")).unwrap();
        let review = draft.nodes.iter().find(|node| node.id == "review").unwrap();
        assert!(review.task.instructions.contains("git reset --hard <their branch>"), "{}", review.task.instructions);
        assert!(!review.task.instructions.contains("switch --detach"));

        // What the template tells the review to do, against the owner and
        // resume checks a review's worktree has to pass.
        let root = std::env::temp_dir().join(format!("factory-epic-review-{}", uuid::Uuid::new_v4()));
        let repo = epic_repo(&root).await;
        let owner = crate::worktree::Owner::new(root.join("worktrees"));
        let spec = || factory_kernel::WorkspaceSpec {
            task_id: uuid::Uuid::new_v4().to_string(),
            workflow_run_id: None,
            lifetime: factory_kernel::WorkspaceLifetime::Task,
        };
        let (kept, _) = owner
            .provision(spec(), &repo, root.join("worktrees/review"), "factory/review-a".into(), Some("main"))
            .await
            .unwrap();
        let (detached, _) = owner
            .provision(spec(), &repo, root.join("worktrees/detached"), "factory/review-b".into(), Some("main"))
            .await
            .unwrap();
        // The implementer's branch, whose work the integration branch -- here
        // the scope's own HEAD -- already holds.
        git_ok(&repo, &["switch", "-q", "-c", "factory/part-a"]).await;
        std::fs::write(repo.join("a.txt"), "a\n").unwrap();
        git_ok(&repo, &["add", "a.txt"]).await;
        git_ok(&repo, &["commit", "-q", "-m", "part a"]).await;
        git_ok(&repo, &["switch", "-q", "main"]).await;
        git_ok(&repo, &["merge", "-q", "--ff-only", "factory/part-a"]).await;

        git_ok(&kept.path, &["reset", "-q", "--hard", "factory/part-a"]).await;
        assert!(kept.path.join("a.txt").exists(), "the review sees the part's work");
        assert!(crate::resume::on_branch(&kept.path, &kept.branch).await, "resume and reuse accept it");
        git_ok(&detached.path, &["switch", "-q", "--detach", "factory/part-a"]).await;
        assert!(!crate::resume::on_branch(&detached.path, &detached.branch).await);

        let outcomes = owner.release(&[kept.path.clone(), detached.path.clone()]).await.unwrap();
        let outcome = |path: &Path| outcomes.iter().find(|(record, _)| record.path == path).unwrap().1.clone();
        assert_eq!(outcome(&kept.path), Ok(()), "released like any finished worktree");
        assert!(!kept.path.exists());
        assert!(outcome(&detached.path).unwrap_err().contains("workspace branch changed"), "what the old template caused");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_rework_round_says_how_many_were_integrations() {
        assert_eq!(crate::engine::feedback_round_title(2, 0), "Workflow rework round 2");
        assert_eq!(
            crate::engine::feedback_round_title(2, 1),
            "Workflow rework round 2 (1 sent back by a workflow step, 1 by integration)"
        );
    }

    #[tokio::test]
    async fn a_dependant_hears_its_prerequisites_result_once() {
        let engine = engine();
        let parent = engine
            .create(NewTask { title: "Two slices".into(), scope: Some("demo".into()), worktree: Some(false), ..Default::default() })
            .await
            .unwrap();
        let parts = vec![epic_part("a", &[], "true"), epic_part("b", &["a"], "true")];
        let routing = Routing { scope: "demo".into(), agent: Some("shell".into()), ..Default::default() };
        let run = engine.start_decomposition_workflow(&parent, &parts, &routing, &Caller::Owner).await.unwrap();
        let all = tasks(&engine).await;
        let a = task_of(&all, "a").clone();
        let b = task_of(&all, "b").clone();
        assert_eq!(b.depends_on, vec![a.id.clone()], "a is b's dependency");
        assert!(run.definition.edges.iter().any(|edge| edge.from == "a" && edge.to == "b"), "and its workflow parent");
        report_done(&engine, &a.id, "A RESULT", None).await;
        let outputs = engine.l4_service().upstream_outputs(&engine.require(&b.id).await.unwrap()).await;
        assert_eq!(outputs.iter().filter(|output| output.task_id == a.id).count(), 1, "{outputs:?}");
        assert_eq!(outputs[0].result.as_deref(), Some("A RESULT"));
        let _ = engine.cancel_workflow(&run.id).await;
    }

    #[tokio::test]
    async fn expand_join_can_tolerate_a_declared_number_of_failed_children() {
        let engine = engine();
        let parent = engine
            .create(NewTask {
                title: "Tolerant fan-out".into(),
                scope: Some("demo".into()),
                worktree: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        let parts = ["left", "right"].map(|id| SplitPart {
            id: id.into(),
            title: id.into(),
            instructions: format!("complete {id}"),
            acceptance: Some("true".into()),
            owns: vec![format!("{id}.txt")],
            interface: Some(format!("{id} result")),
            estimate_seconds: Some(60),
            ..Default::default()
        });
        let routing = Routing { scope: "demo".into(), agent: Some("shell".into()), ..Default::default() };
        let mut run = engine
            .start_decomposition_workflow(&parent, &parts, &routing, &Caller::Owner)
            .await
            .unwrap();
        run.definition
            .nodes
            .iter_mut()
            .find(|node| node.kind == WorkflowNodeKind::Expand)
            .unwrap()
            .expand
            .as_mut()
            .unwrap()
            .join
            .tolerate = 1;
        engine.l4.workflows.put_run(&run).await.unwrap();

        let children = wait_for_tasks(&engine, 3).await;
        let left = children.iter().find(|task| task.decomposition_part.as_deref() == Some("left")).unwrap();
        let right = children.iter().find(|task| task.decomposition_part.as_deref() == Some("right")).unwrap();
        finish(&engine, &left.id, RunStatus::Failed).await;
        finish(&engine, &right.id, RunStatus::Done).await;

        let finished = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(finished.status, WorkflowRunStatus::Done);
        assert_eq!(finished.nodes.iter().filter(|node| node.status == WorkflowNodeStatus::Failed).count(), 1);
    }

    #[tokio::test]
    async fn expand_abandon_cancel_leaves_dispatched_children_running() {
        let engine = engine();
        let parent = engine
            .create(NewTask {
                title: "Abandoned fan-out".into(),
                scope: Some("demo".into()),
                worktree: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        let parts = vec![SplitPart {
            id: "child".into(),
            title: "child".into(),
            instructions: "keep running".into(),
            acceptance: Some("true".into()),
            owns: vec!["child.txt".into()],
            interface: Some("child result".into()),
            estimate_seconds: Some(60),
            ..Default::default()
        }];
        let mut parts = parts;
        let mut waiting = parts[0].clone();
        waiting.id = "later".into();
        waiting.title = "later".into();
        waiting.depends_on = vec!["child".into()];
        waiting.owns = vec!["later.txt".into()];
        parts.push(waiting);
        let routing = Routing { scope: "demo".into(), agent: Some("shell".into()), ..Default::default() };
        let mut run = engine
            .start_decomposition_workflow(&parent, &parts, &routing, &Caller::Owner)
            .await
            .unwrap();
        run.definition
            .nodes
            .iter_mut()
            .find(|node| node.kind == WorkflowNodeKind::Expand)
            .unwrap()
            .expand
            .as_mut()
            .unwrap()
            .cancel = ExpandCancelPolicy::Abandon;
        engine.l4.workflows.put_run(&run).await.unwrap();
        let child = wait_for_tasks(&engine, 2)
            .await
            .into_iter()
            .find(|task| task.decomposition_part.as_deref() == Some("child"))
            .unwrap();

        wait_for_attempt(&engine, &child.id, 1).await;
        let cancelled = engine.cancel_workflow(&run.id).await.unwrap();
        assert_eq!(cancelled.status, WorkflowRunStatus::Cancelled);
        assert!(engine.l4.store.active_run(&child.id).await.unwrap().is_some());
        assert_ne!(engine.require(&child.id).await.unwrap().status, TaskStatus::Cancelled);
        let waiting = tasks(&engine).await.into_iter().find(|task| task.decomposition_part.as_deref() == Some("later")).unwrap();
        assert_eq!(waiting.runs, 0);
        assert!(waiting.after.is_none());
        assert_eq!(waiting.closure.unwrap().reason, factory_core::task::CloseReason::NotPlanned);
    }

    #[tokio::test]
    async fn a_terminal_verifier_review_blocks_its_control_node_for_retry() {
        let engine = engine();
        let mut review = engine.create(NewTask {
            title: "review".into(),
            scope: Some("demo".into()),
            ..Default::default()
        }).await.unwrap();
        review.labels.insert(crate::verification::REVIEW_RUN_LABEL.into(), "subject".into());
        review.status = TaskStatus::Cancelled;
        assert_eq!(node_status(&review), WorkflowNodeStatus::Blocked);
    }
}

/// A node's status, read off its task. A task blocked by a failure is a
/// failed node (`#122`): the task waits for a person, but the workflow's
/// step did fail, and its on-failure edges and retries must see that.
fn node_status(task: &Task) -> WorkflowNodeStatus {
    if task.labels.contains_key(crate::verification::REVIEW_RUN_LABEL)
        && (task.has_failed() || task.status == TaskStatus::Cancelled)
    {
        // A verifier-owned review attempt may be retried. Its failure blocks
        // the control node and subject for a person; it does not consume the
        // whole workflow as a failed business step.
        return WorkflowNodeStatus::Blocked;
    }
    if task.has_failed() {
        return WorkflowNodeStatus::Failed;
    }
    match task.status {
        // A node's task is created straight onto the line and never goes
        // through intake; were one ever to, it is not started yet either.
        TaskStatus::Intake | TaskStatus::Pending => WorkflowNodeStatus::Pending,
        TaskStatus::Dispatching => WorkflowNodeStatus::Dispatching,
        TaskStatus::Running => WorkflowNodeStatus::Running,
        TaskStatus::Blocked => WorkflowNodeStatus::Blocked,
        TaskStatus::Verifying => WorkflowNodeStatus::Verifying,
        TaskStatus::Done => WorkflowNodeStatus::Done,
        TaskStatus::Failed => WorkflowNodeStatus::Failed,
        TaskStatus::Cancelled => WorkflowNodeStatus::Cancelled,
    }
}

impl Engine {
    /// Turn one approved intake decomposition into one durable workflow run.
    /// The definition is generated from the validated plan, while the run
    /// owns the fetched integration branch, merge ledger, combined checks
    /// and final PR hand-off.
    ///
    /// Each part is one task node, or -- when `routing.workflow` names a
    /// part workflow (`#235`) -- a copy of that workflow's whole graph,
    /// namespaced `<part>-<node>`. Either way `expand` leads into each
    /// root part's entry, a prerequisite's terminal into its dependant's
    /// entry, and control-plan injection runs afterwards, over every node.
    pub(crate) async fn start_decomposition_workflow(
        self: &Arc<Self>,
        item: &Task,
        parts: &[SplitPart],
        routing: &Routing,
        caller: &Caller,
    ) -> Result<WorkflowRun> {
        let factory = self.factory_snapshot();
        let scope = factory.scope(&routing.scope)?.clone();
        let scope_path = factory.scope_path(&scope.name)?;
        let github = item
            .intake
            .as_ref()
            .and_then(|record| Some((record.source.repository.clone()?, record.source.number?)));
        let (worktree_capable, worktree_reason) = worktree::capability(&scope_path).await;
        if github.is_some() && !worktree_capable {
            return Err(FactoryError::BadRequest(format!(
                "a GitHub decomposition needs an integration worktree, but this scope is not worktree-capable: {}",
                worktree_reason.unwrap_or_else(|| "unknown reason".into())
            )));
        }
        // Held to the contract again here, not only when the plan was
        // assessed: the template may have been edited since.
        let template = match &routing.workflow {
            Some(wanted) => {
                let found = self.find_workflow(&scope.name, wanted).await?;
                let shape = found
                    .validate()
                    .and_then(|_| found.part_shape())
                    .map_err(|error| FactoryError::BadRequest(format!("part workflow {}: {error}", found.name)))?;
                Some((found, shape))
            }
            None => None,
        };
        // Named before anything is generated, so a part workflow can say it
        // (`{{integration_branch}}`, #235); made further down.
        let integration_branch = worktree_capable.then(|| match github.as_ref() {
            Some((_, number)) => format!("factory/issue-{number}"),
            None => format!("factory/task-{}", &item.id[..8.min(item.id.len())]),
        });
        let mut labels = item.labels.clone();
        labels.insert(factory_core::intake::PARENT_LABEL.into(), item.id.clone());
        let category = item
            .intake
            .as_ref()
            .and_then(|record| record.triage.as_ref())
            .map(|triage| triage.assessment.category.clone());

        let mut nodes = vec![WorkflowNode {
            session: Default::default(),
            id: "expand".into(),
            position: CanvasPoint::default(),
            kind: WorkflowNodeKind::Expand,
            task: factory_core::NewTask {
                title: format!("Decompose: {}", item.title),
                scope: Some(scope.name.clone()),
                worktree: Some(false),
                ..Default::default()
            },
            gate: None,
            exits: Vec::new(),
            expand: Some(ExpandSpec {
                join: ExpandJoin { tolerate: 0 },
                cancel: ExpandCancelPolicy::Terminate,
                children: Vec::new(),
                max_rework_rounds: 3,
            }),
        }];
        let mut edges = Vec::new();
        let mut integration_parts = Vec::new();
        // Each part's (entry, terminal), which its dependency edges join.
        let mut ends: BTreeMap<String, (String, String)> = BTreeMap::new();
        // Which part's which template node each generated id came from, so
        // two that namespace to the same id are refused by name.
        let mut origins: BTreeMap<String, String> = BTreeMap::from([("expand".to_string(), "the expand node".to_string())]);
        // One row per part on the canvas, as tall as the template's graph.
        let (min_x, min_y, row) = template.as_ref().map_or((0.0, 0.0, 0.0), |(template, _)| {
            let xs = template.nodes.iter().map(|node| node.position.x);
            let ys: Vec<f64> = template.nodes.iter().map(|node| node.position.y).collect();
            let min_y = ys.iter().copied().fold(f64::INFINITY, f64::min);
            let max_y = ys.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            (xs.fold(f64::INFINITY, f64::min), min_y, max_y - min_y + 480.0)
        });
        for (index, part) in parts.iter().enumerate() {
            let acceptance = part.acceptance.as_deref().unwrap_or_default().trim();
            let ownership = part
                .owns
                .iter()
                .map(|owned| owned.trim())
                .collect::<Vec<_>>()
                .join(", ");
            let interface = part.interface.as_deref().unwrap_or_default().trim();
            let instructions = format!(
                "{}\n\nDone when: {}\n\nOwned surface: {}\n\nInterface / hand-off: {}\n\n---\nPart {} of task {} ({}). The parent request, for context:\n\n{}",
                part.instructions.trim(),
                acceptance,
                ownership,
                interface,
                part.id,
                item.id,
                item.title,
                item.instructions.trim(),
            );
            let mut part_labels = labels.clone();
            part_labels.insert(factory_core::intake::PART_LABEL.into(), part.id.clone());
            let Some((template, shape)) = &template else {
                nodes.push(WorkflowNode {
                    session: Default::default(),
                    id: part.id.clone(),
                    position: CanvasPoint {
                        x: 240.0 + index as f64 * 180.0,
                        y: 140.0,
                    },
                    kind: WorkflowNodeKind::Task,
                    task: factory_core::NewTask {
                        title: part.title.trim().to_string(),
                        instructions,
                        scope: Some(scope.name.clone()),
                        agent: routing.agent.clone(),
                        parent_task_id: Some(item.id.clone()),
                        decomposition_part: Some(part.id.clone()),
                        estimate_seconds: part.estimate_seconds,
                        labels: part_labels,
                        worktree: Some(worktree_capable),
                        category: category.clone(),
                        ..Default::default()
                    },
                    gate: None,
                    exits: Vec::new(),
                    expand: None,
                });
                integration_parts.push(IntegrationPart {
                    node_id: part.id.clone(),
                    part_id: part.id.clone(),
                    acceptance: acceptance.to_string(),
                    owns: part.owns.clone(),
                    terminal_node: None,
                });
                ends.insert(part.id.clone(), (part.id.clone(), part.id.clone()));
                continue;
            };

            let values: BTreeMap<String, String> = [
                ("part_id", part.id.as_str()),
                ("part_title", part.title.trim()),
                ("part_instructions", part.instructions.trim()),
                ("part_acceptance", acceptance),
                ("part_owns", ownership.as_str()),
                ("part_interface", interface),
                ("parent_title", item.title.as_str()),
                ("parent_instructions", item.instructions.trim()),
                (
                    "integration_branch",
                    integration_branch
                        .as_deref()
                        .unwrap_or("(none: this scope cannot make worktrees, so nothing is integrated)"),
                ),
            ]
            .into_iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
            let copy = template.expand_part(shape, &part.id, &values, &instructions);
            for (step, mut node) in template.nodes.iter().zip(copy.nodes) {
                if let Some(earlier) = origins.insert(node.id.clone(), format!("part {}'s node {}", part.id, step.id)) {
                    return Err(FactoryError::BadRequest(format!(
                        "part workflow {}: part {}'s node {} becomes {}, which is already {earlier}; rename a part so the ids stay apart",
                        template.name, part.id, step.id, node.id
                    )));
                }
                node.position = CanvasPoint {
                    x: 240.0 + node.position.x - min_x,
                    y: 140.0 + index as f64 * row + node.position.y - min_y,
                };
                node.task.scope = Some(scope.name.clone());
                if node.kind == WorkflowNodeKind::Task {
                    node.task.agent = routing
                        .agents
                        .get(&step.id)
                        .cloned()
                        .or(node.task.agent)
                        .or_else(|| routing.agent.clone());
                    node.task.parent_task_id = Some(item.id.clone());
                    node.task.decomposition_part = Some(part.id.clone());
                    let mut merged = labels.clone();
                    merged.extend(std::mem::take(&mut node.task.labels));
                    merged.insert(factory_core::intake::PARENT_LABEL.into(), item.id.clone());
                    merged.insert(factory_core::intake::PART_LABEL.into(), part.id.clone());
                    node.task.labels = merged;
                    if !worktree_capable {
                        node.task.worktree = Some(false);
                    }
                    // The part's estimate is for its deliverable; the other
                    // steps keep whatever the template estimates for them.
                    if node.id == copy.shape.deliverable && part.estimate_seconds.is_some() {
                        node.task.estimate = None;
                        node.task.estimate_seconds = part.estimate_seconds;
                    }
                    // Planned as the template says -- the node's own
                    // category, or the template's -- else as the item was
                    // triaged.
                    if node.task.category.is_none() {
                        node.task.category = template.category.clone().or_else(|| category.clone());
                    }
                }
                nodes.push(node);
            }
            edges.extend(copy.edges);
            integration_parts.push(IntegrationPart {
                node_id: copy.shape.deliverable.clone(),
                part_id: part.id.clone(),
                acceptance: acceptance.to_string(),
                owns: part.owns.clone(),
                terminal_node: Some(copy.shape.terminal.clone()),
            });
            ends.insert(part.id.clone(), (copy.shape.entry, copy.shape.terminal));
        }
        // A dependency may name a part listed after it, so the parts are
        // joined once every one of them exists.
        let templated = template.is_some();
        for part in parts {
            let entry = ends[&part.id].0.clone();
            if part.depends_on.is_empty() {
                edges.push(WorkflowEdge {
                    id: if templated { format!("expand->{entry}") } else { format!("expand-{}", part.id) },
                    from: "expand".into(),
                    to: entry,
                });
            } else {
                for dependency in &part.depends_on {
                    let terminal = ends
                        .get(dependency)
                        .map(|(_, terminal)| terminal.clone())
                        .unwrap_or_else(|| dependency.clone());
                    edges.push(WorkflowEdge {
                        id: if templated { format!("{terminal}->{entry}") } else { format!("{}-{}", dependency, part.id) },
                        from: terminal,
                        to: entry.clone(),
                    });
                }
            }
        }
        let children: Vec<String> = nodes
            .iter()
            .filter(|node| node.kind == WorkflowNodeKind::Task)
            .map(|node| node.id.clone())
            .collect();
        if let Some(expand) = nodes[0].expand.as_mut() {
            expand.children = children;
        }

        let description = match &template {
            Some((template, _)) => format!(
                "Generated from approved intake task {}; every part runs part workflow {} ({}, revision {})",
                item.id, template.name, template.id, template.revision
            ),
            None => format!("Generated from approved intake task {}", item.id),
        };
        let draft = WorkflowDraft {
            name: format!("Decomposition: {}", item.title),
            description,
            scope: scope.name.clone(),
            workspace_ref: None,
            category,
            inputs: Vec::new(),
            part: None,
            nodes,
            edges,
        };
        let definition = WorkflowDefinition::from_draft(draft);
        definition.validate().map_err(FactoryError::BadRequest)?;
        for node in definition
            .nodes
            .iter()
            .filter(|node| node.kind == WorkflowNodeKind::Task)
        {
            self.authorize_workflow_spawn(caller, &node.task).await?;
        }
        let plans = self.l4_service().control_plans(&definition).await?;
        let (definition, _) = definition.inject(&plans);
        definition.validate().map_err(FactoryError::BadRequest)?;

        let mut run = WorkflowRun::new(definition.clone(), caller.as_workflow_actor());
        if let Some(expand) = run.nodes.iter_mut().find(|node| node.node_id == "expand") {
            expand.status = WorkflowNodeStatus::Done;
            expand.exits_evaluated = true;
        }
        if worktree_capable {
            let branch = integration_branch.clone().expect("named for every worktree-capable scope");
            let base_ref = match github.as_ref() {
                Some(_) => {
                    worktree::fetch(&scope_path, "origin", "main")
                        .await
                        .map_err(|error| FactoryError::adapter("git", error))?;
                    "origin/main".to_string()
                }
                None => "HEAD".to_string(),
            };
            let integration_dir = factory
                .worktrees_dir()
                .join(format!("integration-{}", run.id));
            crate::assignments::AssignmentWorkspace {
                spec: factory_kernel::WorkspaceSpec {
                    task_id: item.id.clone(), workflow_run_id: Some(run.id.clone()),
                    lifetime: factory_kernel::WorkspaceLifetime::Task,
                },
                candidate: integration_dir.clone(), branch: branch.clone(),
                base: Some(base_ref.clone()), previous: None,
            }.provision(&self.l4.workspaces, &scope_path).await
                .map_err(|error| FactoryError::adapter("git", error))?;
            run.integration = Some(WorkflowIntegration {
                parent_task_id: item.id.clone(),
                base_ref,
                branch,
                worktree_path: integration_dir.display().to_string(),
                repository: github.as_ref().map(|(repository, _)| repository.clone()),
                issue_number: github.map(|(_, number)| number),
                parts: integration_parts,
                ..Default::default()
            });
        }

        // The expand node materialises every child task at once, including
        // dependants.  They therefore appear as scheduled work immediately;
        // only roots are dispatched below.  Persist the chosen ids before
        // creating rows so recovery can fill the exact same children after a
        // crash rather than fan out twice.
        for node in run.nodes.iter_mut().filter(|node| {
            definition.nodes.iter().any(|snapshot| {
                snapshot.id == node.node_id && snapshot.kind == WorkflowNodeKind::Task
            })
        }) {
            node.task_id = Some(uuid::Uuid::new_v4().to_string());
            node.task_created = false;
        }

        if let Err(error) = self.l4.workflows.put_definition(&definition).await {
            if let Some(integration) = &run.integration {
                let _ = crate::assignments::release(&self.l4.workspaces,
                    &[PathBuf::from(&integration.worktree_path)]).await;
            }
            return Err(error);
        }
        self.l4.workflows.put_run(&run).await?;
        self.shared.bus
            .publish(Event::WorkflowRunUpdated { run: run.clone() });
        {
            let _guard = self.l4.workflow_edit.lock().await;
            self.materialize_workflow_tasks(&mut run).await?;
            self.close_workflow_waits(&run).await?;
        }
        self.advance_workflow(&run.id).await?;
        self.workflow_run(&run.id).await
    }

    /// Validate the agent-selected half of ordered exits before accepting a
    /// report. Checks are the daemon's choice and need no flag; only an
    /// `agent:` exit may be named here.
    pub(crate) async fn validate_workflow_send_to(
        &self,
        task_id: &str,
        status: Option<RunStatus>,
        send_to: Option<&str>,
    ) -> Result<()> {
        let Some(to) = send_to else { return Ok(()) };
        if status != Some(RunStatus::Done) {
            return Err(FactoryError::BadRequest(
                "--send-to is accepted only with --status done; report blocked for a person or failed when the attempt broke"
                    .into(),
            ));
        }
        let task = self.require(task_id).await?;
        let Some(origin) = &task.workflow_origin else {
            return Err(FactoryError::BadRequest(
                "--send-to is only available on a workflow node with declared agent exits; report plain done instead"
                    .into(),
            ));
        };
        let run = self.workflow_run(&origin.workflow_run_id).await?;
        let definition_node = run
            .definition
            .nodes
            .iter()
            .find(|node| node.id == origin.node_id)
            .ok_or_else(|| {
                FactoryError::BadRequest("this task's workflow node no longer exists".into())
            })?;
        let choices: Vec<_> = definition_node
            .exits
            .iter()
            .filter(|exit| exit.agent.is_some())
            .collect();
        let allowed = choices
            .iter()
            .map(|exit| format!("{} ({})", exit.to, exit.agent.as_deref().unwrap_or("")))
            .collect::<Vec<_>>()
            .join(", ");
        let Some(exit) = choices.into_iter().find(|exit| exit.to == to) else {
            let alternative = if allowed.is_empty() {
                "this node has no agent exits; report plain done instead".to_string()
            } else {
                format!("allowed agent exits: {allowed}; otherwise report plain done")
            };
            return Err(FactoryError::BadRequest(format!(
                "node {:?} has no agent exit to {to:?}; {alternative}",
                origin.node_id
            )));
        };
        let used = run
            .nodes
            .iter()
            .find(|node| node.node_id == origin.node_id)
            .map_or(0, |node| node.exit_rounds());
        if exit.max_rounds.is_some_and(|max| used >= max) {
            return Err(FactoryError::BadRequest(format!(
                "agent exit to {to} has no rounds left: report `blocked` with the open findings"
            )));
        }
        Ok(())
    }

    pub(crate) async fn workflow_definition(&self, id: &str) -> Result<WorkflowDefinition> {
        self.l4.workflows
            .get_definition(id)
            .await?
            .ok_or_else(|| missing("workflow", id))
    }

    pub(crate) async fn workflow_run(&self, id: &str) -> Result<WorkflowRun> {
        self.l4_service().workflow_run(id).await
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
        self.l4.workflows.put_definition(&definition).await?;
        self.shared.bus.publish(Event::WorkflowCreated {
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
        self.l4.workflows.put_definition(&definition).await?;
        self.shared.bus.publish(Event::WorkflowUpdated {
            workflow: definition.clone(),
        });
        Ok(definition)
    }

    pub(crate) async fn delete_workflow(&self, id: &str) -> Result<bool> {
        let deleted = self.l4.workflows.delete_definition(id).await?;
        if deleted {
            self.shared.bus
                .publish(Event::WorkflowDeleted { id: id.to_string() });
        }
        Ok(deleted)
    }

    pub(crate) async fn start_workflow(
        self: &Arc<Self>,
        id: &str,
        inputs: BTreeMap<String, String>,
        caller: &Caller,
    ) -> Result<WorkflowRun> {
        self.start_workflow_with_agents(id, inputs, &BTreeMap::new(), caller).await
    }

    /// `start_workflow`, with some steps given to other agents than the
    /// definition names -- the route an intake item was released on. Only
    /// this run's snapshot changes; the definition stays as written.
    pub(crate) async fn start_workflow_with_agents(
        self: &Arc<Self>,
        id: &str,
        inputs: BTreeMap<String, String>,
        agents: &BTreeMap<String, String>,
        caller: &Caller,
    ) -> Result<WorkflowRun> {
        let definition = self.workflow_definition(id).await?;
        // `#235`: a part workflow is a template. Started as itself it would
        // run once, for no part, with its `{{part_...}}` left in braces.
        if definition.part.is_some() {
            return Err(FactoryError::BadRequest(format!(
                "workflow {} is a part workflow: it runs only inside an Intake decomposition, copied once per part; name it in an executable plan's routing.workflow",
                definition.name
            )));
        }
        definition.validate().map_err(FactoryError::BadRequest)?;
        // `#140`: the run's inputs are written into its snapshot before
        // anything else looks at it, so what is authorized, injected and
        // spawned below is exactly what will run.
        let mut definition = definition.with_inputs(&inputs).map_err(FactoryError::BadRequest)?;
        // Likewise each step's agent: set before the spawn check, so the
        // agent that is authorized is the one that will run.
        for (step, agent) in agents {
            let node = definition
                .nodes
                .iter_mut()
                .find(|n| &n.id == step && n.kind == WorkflowNodeKind::Task)
                .ok_or_else(|| {
                    FactoryError::BadRequest(format!("workflow {} has no task step {step:?}", definition.name))
                })?;
            node.task.agent = Some(agent.clone());
        }
        // Every node must be something this caller could `task.create` and
        // `task.run` by hand, checked before anything is persisted -- a
        // `workflow.run` grant is not a way to launder a caller into
        // authority over tasks it could not otherwise touch. See
        // `Engine::authorize_workflow_spawn`. A gate node spawns nothing.
        for node in definition.nodes.iter().filter(|n| n.kind == WorkflowNodeKind::Task) {
            self.authorize_workflow_spawn(caller, &node.task).await?;
        }
        // `#118`: the control plan's required steps merge into this run's
        // immutable snapshot as locked gate nodes. The stored definition is
        // untouched -- a later plan applies to later runs, never this one.
        let plans = self.l4_service().control_plans(&definition).await?;
        let (mut definition, _) = definition.inject(&plans);
        self.l4_service().bind_functionaries(&mut definition).await?;
        self.freeze_workflow_workspace(&mut definition).await?;
        definition.validate().map_err(FactoryError::BadRequest)?;
        let mut run = WorkflowRun::new(definition, caller.as_workflow_actor());
        run.inputs = inputs;
        self.l4.workflows.put_run(&run).await?;
        self.shared.bus
            .publish(Event::WorkflowRunUpdated { run: run.clone() });
        self.advance_workflow(&run.id).await?;
        self.workflow_run(&run.id).await
    }

    pub(crate) async fn cancel_workflow(&self, id: &str) -> Result<WorkflowRun> {
        let task_ids = {
            let _guard = self.l4.workflow_edit.lock().await;
            let mut run = self.workflow_run(id).await?;
            if run.status.is_terminal() {
                return Ok(run);
            }
            run.status = WorkflowRunStatus::Cancelled;
            run.updated_at = Utc::now();
            let abandon = run.definition.nodes.iter().any(|node| {
                node.expand
                    .as_ref()
                    .is_some_and(|expand| expand.cancel == ExpandCancelPolicy::Abandon)
            });
            let mut task_ids = Vec::new();
            for node in &mut run.nodes {
                if !node.status.is_terminal() {
                    if let Some(task_id) = &node.task_id {
                        if !abandon {
                        task_ids.push(task_id.clone());
                        }
                    } else {
                        node.status = WorkflowNodeStatus::Skipped;
                    }
                }
            }
            self.l4.workflows.put_run(&run).await?;
            self.shared.bus
                .publish(Event::WorkflowRunUpdated { run: run.clone() });
            task_ids
        };
        for task_id in task_ids {
            if self.l4.store.active_run(&task_id).await?.is_some() {
                let _ = self
                    .l4_service()
                    .cancel_task_run(&task_id, None, factory_core::run::FailKind::CancelledWithParent)
                    .await;
            }
        }
        let mut run = self.workflow_run(id).await?;

        self.close_workflow_waits(&run).await?;
        for node in &mut run.nodes {
            if let Some(task_id) = &node.task_id {
                if let Some(task) = self.l4.store.get(task_id).await? {
                    node.status = node_status(&task);
                    node.error = task.error;
                }
            }
        }
        run.updated_at = Utc::now();
        self.l4.workflows.put_run(&run).await?;
        self.shared.bus
            .publish(Event::WorkflowRunUpdated { run: run.clone() });
        Ok(run)
    }

    /// Resolve revision selection before persisting or dispatching any task.
    /// A multi-scope pinned workflow must name the same commit in every
    /// task scope, not unrelated branches which happen to have the same name.
    pub(crate) async fn freeze_workflow_workspace(&self, definition: &mut WorkflowDefinition) -> Result<()> {
        let Some(reference) = definition.workspace_ref.clone() else { return Ok(()); };
        if definition.nodes.iter().any(|node| node.kind == WorkflowNodeKind::Task && node.task.worktree == Some(false)) {
            return Err(FactoryError::BadRequest("workspace_ref requires isolated task workspaces".into()));
        }
        let factory = self.factory_snapshot();
        let scopes: std::collections::BTreeSet<&str> = definition.nodes.iter()
            .filter(|node| node.kind == WorkflowNodeKind::Task)
            .map(|node| node.task.scope.as_deref().unwrap_or(&definition.scope)).collect();
        let mut frozen = None;
        for scope in scopes {
            let dir = factory.scope_path(scope)?;
            let mut command = tokio::process::Command::new("git");
            command.current_dir(&dir).args(["rev-parse", "--verify", "--end-of-options", &format!("{reference}^{{commit}}")])
                .kill_on_drop(true);
            let output = tokio::time::timeout(std::time::Duration::from_secs(5), command.output()).await
                .map_err(|_| FactoryError::BadRequest(format!("resolving workspace_ref in {scope} timed out")))?
                .map_err(|error| FactoryError::BadRequest(format!("resolving workspace_ref in {scope}: {error}")))?;
            let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !output.status.success() || !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|c| c.is_ascii_hexdigit()) {
                return Err(FactoryError::BadRequest(format!("workspace_ref {reference:?} is not a commit in scope {scope}")));
            }
            if frozen.as_ref().is_some_and(|previous| previous != &commit) {
                return Err(FactoryError::BadRequest("workspace_ref resolves to different commits across task scopes".into()));
            }
            frozen = Some(commit);
        }
        definition.workspace_ref = frozen;
        Ok(())
    }

    fn integration_rework_limit(run: &WorkflowRun) -> u32 {
        run.definition
            .nodes
            .iter()
            .find_map(|node| node.expand.as_ref().map(|expand| expand.max_rework_rounds))
            .unwrap_or(3)
    }

    /// Put one already-completed child back on the line with concrete merge
    /// or combined-gate feedback.  The same task gets a new run, preserving
    /// its real parent/dependency identity; the caller resumes its harness
    /// session and exact worktree when the adapter can.
    ///
    /// `node_id` is the part's deliverable. When the part ran through a
    /// part workflow (`#235`), the rest of it down to the terminal goes back
    /// to `unstarted` for the next round too (`WorkflowRun::rework_part_body`),
    /// so what was fixed is reviewed again before the integrator merges it.
    async fn request_integration_rework(
        &self,
        run: &mut WorkflowRun,
        node_id: &str,
        feedback: String,
    ) -> Result<Option<(String, factory_core::Run)>> {
        let limit = Self::integration_rework_limit(run);
        let part = run
            .integration
            .as_ref()
            .and_then(|integration| integration.parts.iter().find(|part| part.node_id == node_id))
            .cloned();
        let templated = part.as_ref().is_some_and(|part| part.terminal_node.is_some());
        let Some(node) = run.nodes.iter_mut().find(|node| node.node_id == node_id) else {
            return Ok(None);
        };
        let Some(task_id) = node.task_id.clone() else {
            node.status = WorkflowNodeStatus::Failed;
            node.error = Some(feedback);
            return Ok(None);
        };
        // A single-node part counts its rounds as it always has. A part
        // workflow's deliverable also runs again for its own review loop,
        // and those rounds are not the integrator's to spend.
        let used = if templated { node.integration_rounds } else { node.round };
        if used >= limit {
            node.status = WorkflowNodeStatus::Failed;
            node.error = Some(format!(
                "integration rework exhausted after {limit} rounds: {feedback}"
            ));
            return Ok(None);
        }
        let previous = self.l4.store.runs(&task_id, 1).await?.into_iter().next();
        let Some(previous) = previous else {
            node.status = WorkflowNodeStatus::Failed;
            node.error = Some(format!(
                "cannot rework integration: task {task_id} has no completed run"
            ));
            return Ok(None);
        };
        node.round += 1;
        node.integration_rounds += 1;
        node.status = WorkflowNodeStatus::Pending;
        node.error = Some(feedback.clone());
        node.exits_evaluated = false;
        node.rework_request = Some(ReworkRequest {
            from_node: "integration".into(),
            from_task: task_id.clone(),
            round: used + 1,
            max_rounds: limit,
            feedback: Some(feedback.clone()),
        });
        let round = used + 1;
        if let Some(integration) = run.integration.as_mut() {
            integration.merged_nodes.retain(|merged| merged != node_id);
            integration.checks_passed = false;
        }
        if let Some(part) = &part {
            run.rework_part_body(part);
        }
        self.l4.store
            .update(
                &task_id,
                &TaskPatch {
                    status: Some(TaskStatus::Pending),
                    clear_result: true,
                    clear_error: true,
                    clear_failure: true,
                    clear_closure: true,
                    ..Default::default()
                },
            )
            .await?;
        self.entry(
            &task_id,
            TaskEntry::new(
                "daemon",
                "integration_rework",
                format!("integration sent this child back (round {round} of {limit}): {feedback}"),
            ),
        )
        .await;
        Ok(Some((task_id, previous)))
    }

    /// Merge every newly completed child in dependency order.  This method
    /// is called while `workflow_edit` is held, making the integration branch
    /// a literal single-writer resource.
    async fn integrate_ready_children(
        &self,
        run: &mut WorkflowRun,
    ) -> Result<Vec<(String, factory_core::Run)>> {
        let Some(integration) = run.integration.as_ref() else {
            return Ok(Vec::new());
        };
        let integration_dir = PathBuf::from(&integration.worktree_path);
        let mut merged = integration.merged_nodes.clone();
        let order = run
            .definition
            .validate()
            .map_err(FactoryError::BadRequest)?;
        let mut parts = integration.parts.clone();
        parts.sort_by_key(|part| order.iter().position(|id| id == &part.node_id).unwrap_or(usize::MAX));
        let mut rework = Vec::new();

        // Per part, not per node (#235): a part is merged once its terminal
        // node is done -- in this round, with its exits decided -- and every
        // part it depends on is already on the branch. Its deliverable's
        // newest run says which branch that is. A part without a template
        // is one node playing all three roles, as before.
        for part in parts {
            let node_id = part.node_id.clone();
            if merged.contains(&node_id) {
                continue;
            }
            let prerequisites_merged = run.prerequisite_parts(&part).iter().all(|prerequisite| {
                run.integration.as_ref().is_some_and(|integration| {
                    integration
                        .parts
                        .iter()
                        .any(|candidate| &candidate.part_id == prerequisite && merged.contains(&candidate.node_id))
                })
            });
            if !prerequisites_merged {
                continue;
            }
            let terminal_done = run.nodes.iter().find(|node| node.node_id == part.terminal()).is_some_and(|node| {
                node.status == WorkflowNodeStatus::Done && node.exits_evaluated
            });
            if !terminal_done {
                continue;
            }
            let Some(task_id) = run
                .nodes
                .iter()
                .find(|node| node.node_id == node_id)
                .and_then(|node| node.task_id.clone())
            else {
                continue;
            };
            let Some(attempt) = self.l4.store.runs(&task_id, 1).await?.into_iter().next() else {
                continue;
            };
            let (Some(path), Some(branch)) = (
                attempt.worktree_path.as_deref(),
                attempt.worktree_branch.as_deref(),
            ) else {
                if let Some(request) = self
                    .request_integration_rework(
                        run,
                        &node_id,
                        "the completed child has no recorded worktree and branch to integrate"
                            .into(),
                    )
                    .await?
                {
                    rework.push(request);
                }
                break;
            };
            match worktree::dirty(Path::new(path)).await {
                Ok(Some(status)) => {
                    if let Some(request) = self
                        .request_integration_rework(
                            run,
                            &node_id,
                            format!("commit every intended change before integration; the worktree is dirty:\n{status}"),
                        )
                        .await?
                    {
                        rework.push(request);
                    }
                    break;
                }
                Err(error) => {
                    if let Some(request) = self
                        .request_integration_rework(
                            run,
                            &node_id,
                            format!("could not inspect the child worktree: {error}"),
                        )
                        .await?
                    {
                        rework.push(request);
                    }
                    break;
                }
                Ok(None) => {}
            }
            if let Err(error) = worktree::merge(&integration_dir, branch).await {
                if let Some(request) = self
                    .request_integration_rework(
                        run,
                        &node_id,
                        format!(
                            "merge the integration branch into your branch, resolve and commit the conflict, then report done: {error}"
                        ),
                    )
                    .await?
                {
                    rework.push(request);
                }
                break;
            }
            merged.push(node_id.clone());
            if let Some(current) = run.integration.as_mut() {
                current.merged_nodes = merged.clone();
            }
            self.entry(
                &task_id,
                TaskEntry::new(
                    "daemon",
                    "integrated",
                    format!(
                        "merged {branch} into {}",
                        run.integration.as_ref().unwrap().branch
                    ),
                ),
            )
            .await;
        }
        Ok(rework)
    }

    /// Run every child's acceptance command against the union.  The command
    /// identifies the responsible child, so a failure returns to that child
    /// rather than turning the whole workflow into an unactionable red box.
    async fn run_combined_checks(
        &self,
        run: &mut WorkflowRun,
    ) -> Result<Vec<(String, factory_core::Run)>> {
        let Some(integration) = run.integration.as_ref() else {
            return Ok(Vec::new());
        };
        if integration.checks_passed || integration.merged_nodes.len() != integration.parts.len() {
            return Ok(Vec::new());
        }
        let dir = PathBuf::from(&integration.worktree_path);
        let checks = integration.parts.clone();
        for part in checks {
            let (code, output) =
                run_shell_capture(&dir, &part.acceptance, EXIT_CHECK_TIMEOUT_SECS).await;
            if code != Some(0) {
                let feedback = format!(
                    "the combined integration check for part {} failed ({}) while running `{}`:\n{}",
                    part.part_id,
                    code.map(|value| format!("exit {value}")).unwrap_or_else(|| "timeout or launch error".into()),
                    part.acceptance,
                    factory_core::bench::tail_4kib(&output),
                );
                return Ok(self
                    .request_integration_rework(run, &part.node_id, feedback)
                    .await?
                    .into_iter()
                    .collect());
            }
        }
        if let Some(integration) = run.integration.as_mut() {
            integration.checks_passed = true;
        }
        Ok(Vec::new())
    }

    async fn gh_output(gh: &Path, dir: &Path, args: &[String]) -> Result<std::process::Output> {
        let mut command = tokio::process::Command::new(gh);
        command.current_dir(dir).args(args).kill_on_drop(true);
        tokio::time::timeout(crate::github_intake::GH_TIMEOUT, command.output())
            .await
            .map_err(|_| FactoryError::adapter("github", "gh timed out"))?
            .map_err(|error| FactoryError::adapter("github", error.to_string()))
    }

    async fn ensure_integration_pr_with_gh(
        &self,
        run: &WorkflowRun,
        gh: &Path,
    ) -> Result<Option<String>> {
        let Some(integration) = run.integration.as_ref() else {
            return Ok(None);
        };
        let (Some(repository), Some(issue)) =
            (integration.repository.as_deref(), integration.issue_number)
        else {
            return Ok(None);
        };
        let dir = Path::new(&integration.worktree_path);
        worktree::push(dir, "origin", &integration.branch)
            .await
            .map_err(|error| FactoryError::adapter("git", error))?;

        let view_args = vec![
            "pr".into(),
            "view".into(),
            integration.branch.clone(),
            "--repo".into(),
            repository.to_string(),
            "--json".into(),
            "url".into(),
            "--jq".into(),
            ".url".into(),
        ];
        let existing = Self::gh_output(gh, dir, &view_args).await?;
        if existing.status.success() {
            let url = String::from_utf8_lossy(&existing.stdout).trim().to_string();
            if !url.is_empty() {
                return Ok(Some(url));
            }
        }

        let parent = self.require(&integration.parent_task_id).await?;
        let mut rows = Vec::new();
        for part in &integration.parts {
            let task = run
                .nodes
                .iter()
                .find(|node| node.node_id == part.node_id)
                .and_then(|node| node.task_id.as_deref());
            if let Some(task_id) = task {
                if let Some(task) = self.l4.store.get(task_id).await? {
                    rows.push(format!(
                        "- `{}` — {}: {}",
                        part.part_id,
                        task.title,
                        task.result.unwrap_or_else(|| "completed".into())
                    ));
                }
            }
        }
        let body = format!(
            "Closes #{issue}\n\nFactory decomposed this request into internal tasks, integrated them in dependency order, and ran every child acceptance command against the combined branch.\n\n{}",
            rows.join("\n")
        );
        let create_args = vec![
            "pr".into(),
            "create".into(),
            "--repo".into(),
            repository.to_string(),
            "--base".into(),
            "main".into(),
            "--head".into(),
            integration.branch.clone(),
            "--title".into(),
            format!("{} (#{issue})", parent.title),
            "--body".into(),
            body,
        ];
        let created = Self::gh_output(gh, dir, &create_args).await?;
        if !created.status.success() {
            let stderr = String::from_utf8_lossy(&created.stderr).trim().to_string();
            return Err(FactoryError::adapter(
                "github",
                if stderr.is_empty() {
                    format!("gh pr create exited with {}", created.status)
                } else {
                    format!("gh pr create exited with {}: {stderr}", created.status)
                },
            ));
        }
        let url = String::from_utf8_lossy(&created.stdout).trim().to_string();
        if url.is_empty() {
            return Err(FactoryError::adapter(
                "github",
                "gh pr create returned no pull request URL",
            ));
        }
        Ok(Some(url))
    }

    async fn handoff_integration(&self, run: &mut WorkflowRun) -> Result<()> {
        let Some(integration) = run.integration.as_ref() else {
            return Ok(());
        };
        if !integration.checks_passed {
            return Err(FactoryError::BadRequest(
                "the combined integration checks have not passed".into(),
            ));
        }
        let pr_url = if let Some(url) = integration.pr_url.clone() {
            Some(url)
        } else {
            self.ensure_integration_pr_with_gh(run, Path::new("gh"))
                .await?
        };
        if let Some(current) = run.integration.as_mut() {
            current.pr_url = pr_url.clone();
        }
        // Persist the externally visible hand-off before deleting local
        // workspaces.  Recovery can now find the same PR if cleanup is
        // interrupted, without opening a duplicate.
        run.updated_at = Utc::now();
        self.l4.workflows.put_run(run).await?;
        // Release happens only after the workflow outcome is durable and all
        // siblings have stopped. A refused cleanup never fails a valid handoff.
        let result = match pr_url {
            Some(url) => format!("Implemented in {url}"),
            None => format!("Integrated on {}", run.integration.as_ref().unwrap().branch),
        };
        self.l4.store
            .update(
                &run.integration.as_ref().unwrap().parent_task_id,
                &TaskPatch {
                    result: Some(result.clone()),
                    ..Default::default()
                },
            )
            .await?;
        self.entry(
            &run.integration.as_ref().unwrap().parent_task_id,
            TaskEntry::new("daemon", "integration_handed_off", result),
        )
        .await;
        Ok(())
    }

    /// Reconcile persisted node decisions with authoritative task state, then
    /// make every newly eligible decision. The mutex serializes reports,
    /// cancellation and restart recovery so one node can never be chosen
    /// twice, and so `recover_workflows`'s repair of a node never races this.
    pub(crate) async fn advance_workflow(self: &Arc<Self>, id: &str) -> Result<()> {
        let _guard = self.l4.workflow_edit.lock().await;
        let mut run = self.workflow_run(id).await?;
        self.materialize_workflow_tasks(&mut run).await?;

        // Mirror authoritative task state into every node that has one, even
        // once the run itself is terminal. A sibling still running when its
        // neighbour failed does not freeze mid-flight forever just because
        // the workflow gave up on the run as a whole -- see issue #45's node
        // overlay requirement and the README.
        for node in &mut run.nodes {
            let Some(task_id) = node.task_id.clone() else {
                continue;
            };
            match self.l4.store.get(&task_id).await {
                Ok(Some(task)) => {
                    if (task.after.is_some() && task.status == TaskStatus::Pending) || (node.status.is_terminal()
                        && task.closure.as_ref().is_some_and(|closure| closure.reason == factory_core::task::CloseReason::NotPlanned)) {
                        continue;
                    }
                    let attempts = self.l4.store.runs(&task_id, u32::MAX).await?;
                    node.attempts = attempts.iter().rev().map(|attempt| factory_core::workflow::WorkflowAttempt {
                        run_id: attempt.id.clone(), attempt: attempt.attempt,
                        round: attempt.workflow_round, status: attempt.status,
                    }).collect();
                    // The task still mirrors the last round until the next
                    // dispatch. Never let that old done overwrite send_back.
                    if node.round > 0 {
                        let latest = self.l4.store.runs(&task_id, 1).await?.into_iter().next();
                        if latest.as_ref().is_none_or(|attempt| attempt.workflow_round < node.round) {
                            continue;
                        }
                    }
                    // A check-exit error blocks the workflow node after its
                    // task has already completed. Preserve that orchestration
                    // block instead of mirroring the task's `done` over it.
                    if !(node.status == WorkflowNodeStatus::Blocked
                        && node.exits_evaluated
                        && task.status == TaskStatus::Done)
                    {
                        node.status = node_status(&task);
                        node.error = task.error;
                    }
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

        self.mirror_gate_nodes(&mut run).await;

        if run.status.is_terminal() {
            // A terminal run never spawns again, and nothing above may
            // rewrite its own status or failure node -- only the per-node
            // mirror does, and that alone is worth persisting and publishing.
            self.close_workflow_waits(&run).await?;
            run.updated_at = Utc::now();
            self.l4.workflows.put_run(&run).await?;
            self.shared.bus.publish(Event::WorkflowRunUpdated { run });
            self.l4_service().sweep_workspaces().await;
            return Ok(());
        }

        // A completed task and its route are separate facts (#149). Evaluate
        // each done node's ordered exits exactly once. A backwards match
        // resets its bounded loop; a forward match skips the default branch;
        // no match leaves the ordinary outgoing edges as the default.
        for done in run
            .nodes
            .iter()
            .filter(|n| n.status == WorkflowNodeStatus::Done && !n.exits_evaluated)
            .map(|n| n.node_id.clone())
            .collect::<Vec<_>>()
        {
            self.evaluate_node_exits(&mut run, &done).await?;
        }
        self.materialize_workflow_tasks(&mut run).await?;
        self.close_workflow_waits(&run).await?;

        let mut to_continue = self.integrate_ready_children(&mut run).await?;
        if to_continue.is_empty() {
            to_continue = self.run_combined_checks(&mut run).await?;
        }
        if !to_continue.is_empty() {
            run.updated_at = Utc::now();
            self.l4.workflows.put_run(&run).await?;
            self.shared.bus
                .publish(Event::WorkflowRunUpdated { run: run.clone() });
            drop(_guard);
            for (task_id, previous) in to_continue {
                let engine = self.clone();
                tokio::spawn(async move {
                    engine
                        .l4_service()
                        .start_run_due_continue(&task_id, crate::engine::Due::now(), previous)
                        .await;
                });
            }
            return Ok(());
        }

        let expand_policy = run
            .definition
            .nodes
            .iter()
            .find_map(|node| node.expand.as_ref().cloned());
        let tolerated_failures = expand_policy
            .as_ref()
            .map_or(0, |expand| expand.join.tolerate as usize);
        let failed_children = expand_policy.as_ref().map_or(0, |expand| {
            expand
                .children
                .iter()
                .filter(|child| {
                    run.nodes
                        .iter()
                        .find(|node| &node.node_id == *child)
                        .is_some_and(|node| node.status == WorkflowNodeStatus::Failed)
                })
                .count()
        });
        if let Some(failed) = run
            .nodes
            .iter()
            .find(|n| n.status == WorkflowNodeStatus::Failed)
            .filter(|_| expand_policy.is_none() || failed_children > tolerated_failures)
        {
            run.status = WorkflowRunStatus::Failed;
            run.failure_node_id = Some(failed.node_id.clone());
            run.error = Some(
                failed
                    .error
                    .clone()
                    .unwrap_or_else(|| "a task node failed".into()),
            );
        } else if let Some(cancelled) = run
            .nodes
            .iter()
            .find(|n| n.status == WorkflowNodeStatus::Cancelled)
        {
            run.status = WorkflowRunStatus::Cancelled;
            run.failure_node_id = Some(cancelled.node_id.clone());
        } else if run.integration.is_none()
            && failed_children > 0
            && failed_children <= tolerated_failures
            && run.nodes.iter().all(|node| node.status.is_terminal())
        {
            run.status = WorkflowRunStatus::Done;
        } else if run.nodes.iter().all(|node| {
            matches!(
                node.status,
                WorkflowNodeStatus::Done | WorkflowNodeStatus::SkippedByRoute
            )
        }) {
            if run.integration.is_some() {
                match self.handoff_integration(&mut run).await {
                    Ok(()) => run.status = WorkflowRunStatus::Done,
                    Err(error) => {
                        run.status = WorkflowRunStatus::Failed;
                        run.failure_node_id = Some("integration".into());
                        run.error = Some(format!("integration hand-off failed: {error}"));
                    }
                }
            } else {
                run.status = WorkflowRunStatus::Done;
            }
        }
        if run.status.is_terminal() {
            for node in &mut run.nodes {
                if node.status == WorkflowNodeStatus::Unstarted {
                    node.status = WorkflowNodeStatus::Skipped;
                }
            }
            run.updated_at = Utc::now();
            self.close_workflow_waits(&run).await?;
            self.l4.workflows.put_run(&run).await?;
            self.shared.bus.publish(Event::WorkflowRunUpdated { run });
            self.l4_service().sweep_workspaces().await;
            return Ok(());
        }

        self.close_workflow_waits(&run).await?;
        let eligible: Vec<String> = run
            .nodes
            .iter()
            .filter(|node| {
                node.status == WorkflowNodeStatus::Unstarted
                    || (node.status == WorkflowNodeStatus::Pending && node.task_id.is_some())
            })
            .filter(|node| {
                run.definition
                    .nodes
                    .iter()
                    .any(|n| n.id == node.node_id && n.kind == WorkflowNodeKind::Task)
            })
            // An integrated decomposition starts a part's work only once
            // every part it depends on is on the integration branch. Per
            // part (#235): a part workflow's own earlier steps are never
            // merged before its later ones run.
            .filter(|node| {
                run.integration.as_ref().is_none_or(|integration| match run.part_of(&node.node_id) {
                    Some(part) => run
                        .prerequisite_parts(part)
                        .iter()
                        .all(|prerequisite| run.part_merged(prerequisite)),
                    None => run
                        .definition
                        .prerequisite_edges(&node.node_id)
                        .into_iter()
                        .filter(|edge| {
                            run.definition.nodes.iter().any(|candidate| {
                                candidate.id == edge.from
                                    && candidate.kind == WorkflowNodeKind::Task
                            })
                        })
                        .all(|edge| integration.merged_nodes.contains(&edge.from)),
                })
            })
            .filter(|node| {
                run.definition
                    .prerequisite_edges(&node.node_id)
                    .into_iter()
                    .all(|edge| {
                        let done = |id: &str| {
                            run.nodes
                                .iter()
                                .find(|n| n.node_id == id)
                                .is_some_and(|n| n.status == WorkflowNodeStatus::Done)
                        };
                        let finished = |id: &str| {
                            run.nodes.iter().find(|n| n.node_id == id).is_some_and(|n| {
                                matches!(
                                    n.status,
                                    WorkflowNodeStatus::Done | WorkflowNodeStatus::SkippedByRoute
                                )
                            })
                        };
                        // A gate parent counts only once the work it judged
                        // is itself done -- a step that passed in a round
                        // that then blocked on another step is not a
                        // verified run to build on.
                        match run.definition.nodes.iter().find(|n| n.id == edge.from) {
                            Some(n) if n.kind == WorkflowNodeKind::Gate => {
                                finished(&edge.from)
                                    && (run
                                        .nodes
                                        .iter()
                                        .find(|node| node.node_id == edge.from)
                                        .is_some_and(|node| {
                                            node.status == WorkflowNodeStatus::SkippedByRoute
                                        })
                                        || run
                                            .definition
                                            .gate_subject(&n.id)
                                            .is_some_and(|subject| done(&subject)))
                            }
                            _ => finished(&edge.from),
                        }
                    })
            })
            .filter(|node| {
                let parents = run.definition.prerequisite_edges(&node.node_id);
                parents.is_empty()
                    || parents.iter().any(|edge| {
                        run.nodes
                            .iter()
                            .find(|n| n.node_id == edge.from)
                            .is_some_and(|n| n.status == WorkflowNodeStatus::Done)
                            || run
                                .definition
                                .nodes
                                .iter()
                                .find(|n| n.id == edge.from)
                                .is_some_and(|n| n.kind == WorkflowNodeKind::Approval)
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

            // An expand node materialises all decomposition tasks up front.
            // For one of those, eligibility means dispatching the existing
            // zero-attempt row, not creating a second child.
            if let Some(existing_id) = run
                .nodes
                .iter()
                .find(|node| node.node_id == node_id)
                .and_then(|node| node.task_id.clone())
            {
                if self
                    .l4.store
                    .get(&existing_id)
                    .await?
                    .is_some_and(|task| task.status == TaskStatus::Pending && task.runs == 0)
                {
                    let caller = self.caller_for_actor(&run.started_by).await;
                    let template = &run.definition.nodes.iter().find(|node| node.id == node_id).expect("snapshot node").task;
                    if let Err(denial) = self.authorize_workflow_spawn(&caller, template).await {
                        let node = run.nodes.iter_mut().find(|node| node.node_id == node_id).expect("snapshot node");
                        node.status = WorkflowNodeStatus::Failed;
                        node.error = Some(denial.to_string());
                        run.status = WorkflowRunStatus::Failed;
                        run.failure_node_id = Some(node_id);
                        run.error = Some(denial.to_string());
                        continue;
                    }
                    self.l4.store.update(&existing_id, &TaskPatch { clear_after: true, ..Default::default() }).await?;
                    run.nodes.iter_mut().find(|node| node.node_id == node_id).expect("snapshot node").status = WorkflowNodeStatus::Pending;
                    self.publish_task(&existing_id).await;
                    to_start.push(existing_id);
                } else if run.nodes.iter().any(|node| node.node_id == node_id && node.round > 0) {
                    let caller = self.caller_for_actor(&run.started_by).await;
                    let template = &run.definition.nodes.iter().find(|node| node.id == node_id)
                        .expect("snapshot node").task;
                    if let Err(denial) = self.authorize_workflow_spawn(&caller, template).await {
                        let node = run.nodes.iter_mut().find(|node| node.node_id == node_id)
                            .expect("snapshot node");
                        node.status = WorkflowNodeStatus::Failed;
                        node.error = Some(denial.to_string());
                        run.status = WorkflowRunStatus::Failed;
                        run.failure_node_id = Some(node_id);
                        run.error = Some(denial.to_string());
                        continue;
                    }
                    self.l4.store.update(&existing_id, &TaskPatch {
                        status: Some(TaskStatus::Pending),
                        clear_after: true,
                        clear_result: true, clear_routed_to: true, clear_error: true,
                        clear_failure: true, clear_closure: true,
                        ..Default::default()
                    }).await?;
                    let node = run.nodes.iter_mut().find(|node| node.node_id == node_id)
                        .expect("snapshot node");
                    node.status = WorkflowNodeStatus::Pending;
                    self.entry(&existing_id, TaskEntry::new("daemon", "workflow_feedback",
                        format!("rework round {} queued on the same task", node.round))).await;
                    self.publish_task(&existing_id).await;
                    to_start.push(existing_id);
                }
                continue;
            }

            let snapshot_node = run
                .definition
                .nodes
                .iter()
                .find(|node| node.id == node_id)
                .expect("run nodes come from the snapshot");
            let mut template = snapshot_node.task.clone();
            // The spawned task carries the category it was planned as, so
            // its own record says what its run was held to.
            template.category = Some(run.definition.node_category(snapshot_node));

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
            self.l4.workflows.put_run(&run).await?;
            self.shared.bus
                .publish(Event::WorkflowRunUpdated { run: run.clone() });

            let origin = WorkflowOrigin {
                workflow_id: run.workflow_id.clone(),
                workflow_run_id: run.id.clone(),
                node_id: node_id.clone(),
                workspace: run.task_workspace(),
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
        self.close_workflow_waits(&run).await?;
        run.updated_at = Utc::now();
        self.l4.workflows.put_run(&run).await?;
        self.shared.bus
            .publish(Event::WorkflowRunUpdated { run: run.clone() });
        drop(_guard);
        for task_id in to_start {
            let engine = self.clone();
            tokio::spawn(async move {
                engine.l4_service().start_run(&task_id, Trigger::Workflow).await;
            });
        }
        Ok(())
    }

    async fn evaluate_node_exits(&self, run: &mut WorkflowRun, from: &str) -> Result<()> {
        let exits = run
            .definition
            .nodes
            .iter()
            .find(|node| node.id == from)
            .map(|node| node.exits.clone())
            .unwrap_or_default();
        let task_id = run
            .nodes
            .iter()
            .find(|node| node.node_id == from)
            .and_then(|node| node.task_id.clone());
        let task = match task_id.as_deref() {
            Some(id) => self.l4.store.get(id).await?,
            None => None,
        };
        let mut selected = None;

        for (index, exit) in exits.iter().enumerate() {
            let holds = if let Some(command) = &exit.check {
                let dir = if let Some(task) = &task {
                    let latest = self.l4.store.runs(&task.id, 1).await?.into_iter().next();
                    latest
                        .and_then(|attempt| attempt.worktree_path.map(std::path::PathBuf::from))
                        .unwrap_or(self.factory_snapshot().scope_path(&task.scope)?)
                } else {
                    self.factory_snapshot().scope_path(&run.scope)?
                };
                let (code, output) = run_shell_capture(&dir, command, EXIT_CHECK_TIMEOUT_SECS).await;
                if let Some(task_id) = &task_id {
                    let code_words = code
                        .map(|n| format!("exit {n}"))
                        .unwrap_or_else(|| "did not finish".into());
                    self.entry(
                        task_id,
                        TaskEntry::new(
                            "daemon",
                            "exit_checked",
                            format!("exit {} to {}: {code_words}", index + 1, exit.to),
                        )
                        .with_data(serde_json::json!({
                            "exit": index + 1,
                            "to": exit.to,
                            "command": command,
                            "exit_code": code,
                            "output": factory_core::bench::tail_4kib(&output),
                        })),
                    )
                    .await;
                }
                match code {
                    Some(0) => true,
                    Some(1) => false,
                    other => {
                        let detail = other
                            .map(|code| format!("exit {code}"))
                            .unwrap_or_else(|| "timeout or launch error".into());
                        if let Some(node) = run.nodes.iter_mut().find(|node| node.node_id == from) {
                            node.status = WorkflowNodeStatus::Blocked;
                            node.exits_evaluated = true;
                            node.error = Some(format!(
                                "exit {} check could not decide ({detail}): {}",
                                index + 1,
                                factory_core::bench::tail_4kib(&output)
                            ));
                        }
                        return Ok(());
                    }
                }
            } else {
                task.as_ref().and_then(|task| task.routed_to.as_deref()) == Some(exit.to.as_str())
            };
            if holds {
                selected = Some(exit.clone());
                break;
            }
        }

        if let Some(exit) = selected {
            let backwards = run.definition.ancestors(from).contains(&exit.to);
            if backwards {
                match run.send_back(from, &exit.to) {
                    SendBack::Sent { round, max_rounds } => {
                        let feedback = task.as_ref().map(|task| [task.error.as_deref(), task.result.as_deref()]
                            .into_iter().flatten().collect::<Vec<_>>().join("\n\n"));
                        // Only the node this exit sent the work to. Another
                        // loop in the same run -- another part's review
                        // (#235), say -- can hold a request of the same
                        // round number, and its findings are its own.
                        if let Some(request) = run
                            .nodes
                            .iter_mut()
                            .find(|node| node.node_id == exit.to)
                            .and_then(|node| node.rework_request.as_mut())
                            .filter(|request| request.from_node == from && request.round == round)
                        {
                            request.feedback = feedback;
                        }
                        if let Some(task_id) = &task_id {
                            self.entry(
                                task_id,
                                TaskEntry::new(
                                    "daemon",
                                    "routed_to",
                                    format!("sent the work back to {}: round {round} of {max_rounds}", exit.to),
                                )
                                .with_data(serde_json::json!({ "routed_to": exit.to, "round": round, "max_rounds": max_rounds })),
                            )
                            .await;
                        }
                    }
                    SendBack::Exhausted { .. } => {
                        if let Some(node) = run.nodes.iter_mut().find(|node| node.node_id == from) {
                            node.status = WorkflowNodeStatus::Blocked;
                            node.exits_evaluated = true;
                            node.error = Some(format!(
                                "agent exit to {} has no rounds left: report blocked with the open findings",
                                exit.to
                            ));
                        }
                    }
                    SendBack::NoExit => {}
                }
            } else {
                run.route_forward(from, &exit.to);
                if let Some(node) = run.nodes.iter_mut().find(|node| node.node_id == from) {
                    node.exits_evaluated = true;
                }
            }
            if let Some(task_id) = &task_id {
                let _ = self
                    .l4.store
                    .update(
                        task_id,
                        &factory_core::task::TaskPatch {
                            routed_to: Some(exit.to),
                            ..Default::default()
                        },
                    )
                    .await;
            }
        } else if let Some(node) = run.nodes.iter_mut().find(|node| node.node_id == from) {
            node.exits_evaluated = true;
            node.routed_to = None;
            if let Some(task_id) = &task_id {
                let _ = self
                    .l4.store
                    .update(
                        task_id,
                        &factory_core::task::TaskPatch {
                            clear_routed_to: true,
                            ..Default::default()
                        },
                    )
                    .await;
            }
        }
        Ok(())
    }

    /// A gate node's status is what its subject's newest run found
    /// (`#118`): the verifier writes attestations, and this reads them back
    /// onto the node. A gate never spawns anything of its own.
    ///
    /// * the subject's run is `done` -- verified, so every gate passed;
    /// * it is `verifying` -- the gate is running, or waiting its turn;
    /// * it is blocked by verification -- this round's newest attestation
    ///   for the step says which: passed, or failed (`blocked`: the line is
    ///   stopped, not given up on), or none (never reached: `unstarted`);
    /// * it failed or was cancelled -- the gate never will run: `skipped`;
    /// * anything else -- not reached yet.
    async fn mirror_gate_nodes(&self, run: &mut WorkflowRun) {
        let gates: Vec<(String, String, String, WorkflowNodeKind)> = run
            .definition
            .nodes
            .iter()
            .filter(|n| {
                matches!(
                    n.kind,
                    WorkflowNodeKind::Gate | WorkflowNodeKind::Review | WorkflowNodeKind::Approval
                )
            })
            .filter_map(|n| {
                let subject = run.definition.gate_subject(&n.id)?;
                Some((n.id.clone(), subject, n.gate.as_ref()?.step.clone(), n.kind))
            })
            .collect();
        for (gate_id, subject, step, kind) in gates {
            if kind == WorkflowNodeKind::Review
                && run
                    .nodes
                    .iter()
                    .find(|n| n.node_id == gate_id)
                    .and_then(|n| n.task_id.as_ref())
                    .is_some()
            {
                continue;
            }
            let Some(task_id) = run.nodes.iter().find(|n| n.node_id == subject).and_then(|n| n.task_id.clone()) else {
                continue;
            };
            let Ok(Some(subject_run)) = self.l4.store.runs(&task_id, 1).await.map(|r| r.into_iter().next()) else {
                continue;
            };
            if run.nodes.iter().find(|node| node.node_id == subject)
                .is_some_and(|node| node.round > subject_run.workflow_round) {
                continue;
            }
            let evidence = self
                .l4.run_evidence
                .step_attestations(&subject_run.id)
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|a| a.step == step)
                .max_by_key(|a| a.at);
            let approval_status = if kind == WorkflowNodeKind::Approval {
                match &evidence {
                    Some(a) if a.verdict == AttestationVerdict::Pass => Some((WorkflowNodeStatus::Done, None)),
                    Some(a) => Some((WorkflowNodeStatus::Blocked, a.findings.clone())),
                    None if subject_run.status == RunStatus::Blocked => {
                        Some((WorkflowNodeStatus::Blocked, Some("waiting for approval".into())))
                    }
                    None => None,
                }
            } else {
                None
            };
            let (status, error) = if let Some(status) = approval_status {
                status
            } else {
                match subject_run.status {
                    RunStatus::Done => (WorkflowNodeStatus::Done, None),
                    RunStatus::Verifying => (WorkflowNodeStatus::Verifying, None),
                    RunStatus::Failed | RunStatus::Cancelled => {
                        (WorkflowNodeStatus::Skipped, None)
                    }
                    RunStatus::Blocked
                        if subject_run.blocked_source
                            == Some(factory_core::run::BlockSource::Verification) =>
                    {
                        let since = subject_run.blocked_since.unwrap_or(subject_run.started_at);
                        let newest = self
                            .l4.run_evidence
                            .step_attestations(&subject_run.id)
                            .await
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|a| a.step == step && a.at <= since)
                            .max_by_key(|a| a.at);
                        match newest {
                            Some(a) if a.verdict == AttestationVerdict::Pass => {
                                (WorkflowNodeStatus::Done, None)
                            }
                            Some(a) => (
                                WorkflowNodeStatus::Blocked,
                                Some(format!(
                                    "{step} failed ({})",
                                    a.exit_code
                                        .map(|c| format!("exit {c}"))
                                        .unwrap_or_else(|| "did not finish".into())
                                )),
                            ),
                            None => (WorkflowNodeStatus::Unstarted, None),
                        }
                    }
                    _ => (WorkflowNodeStatus::Unstarted, None),
                }
            };
            if let Some(node) = run.nodes.iter_mut().find(|n| n.node_id == gate_id) {
                if !matches!(
                    node.status,
                    WorkflowNodeStatus::Skipped | WorkflowNodeStatus::SkippedByRoute
                ) || status != WorkflowNodeStatus::Unstarted
                {
                    node.status = status;
                    node.error = error;
                }
            }
        }
    }

    pub(crate) async fn sync_workflow_for_task(self: &Arc<Self>, task_id: &str) {
        let Ok(Some(task)) = self.l4.store.get(task_id).await else {
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
        self.l4_service().sweep_workspaces().await;
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
        let Ok(Some(task)) = self.l4.store.get(task_id).await else {
            return;
        };
        let Some(origin) = task.workflow_origin.clone() else {
            return;
        };
        let _guard = self.l4.workflow_edit.lock().await;
        let Ok(mut run) = self.workflow_run(&origin.workflow_run_id).await else {
            return;
        };
        // A task an earlier rework round superseded speaks for nobody: the
        // node has moved on to a newer one (`#140`).
        if run.nodes.iter().all(|node| node.node_id != origin.node_id || node.task_id.as_deref() != Some(task_id)) {
            return;
        }
        let Some(node) = run
            .nodes
            .iter_mut()
            .find(|node| node.node_id == origin.node_id)
        else {
            return;
        };
        if node.round > 0 {
            let Ok(Some(latest)) = self.l4.store.runs(task_id, 1).await.map(|runs| runs.into_iter().next()) else {
                return;
            };
            if latest.workflow_round < node.round {
                return;
            }
        }
        if (task.after.is_some() && task.status == TaskStatus::Pending) || (node.status.is_terminal()
            && task.closure.as_ref().is_some_and(|closure| closure.reason == factory_core::task::CloseReason::NotPlanned)) { return; }
        node.status = node_status(&task);
        node.error = task.error;
        if let Ok(attempts) = self.l4.store.runs(task_id, u32::MAX).await {
            node.attempts = attempts.iter().rev().map(|attempt| factory_core::workflow::WorkflowAttempt {
                run_id: attempt.id.clone(), attempt: attempt.attempt,
                round: attempt.workflow_round, status: attempt.status,
            }).collect();
        }
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
        if self.close_workflow_waits(&run).await.is_err() {
            tracing::warn!(workflow_run = run.id, "could not close unadmitted workflow tasks");
        }
        if self.l4.workflows.put_run(&run).await.is_ok() {
            self.shared.bus.publish(Event::WorkflowRunUpdated { run });
        }
    }

    /// Restart recovery is a reconciliation, not a replay: persisted task ids
    /// win. Only interrupted creations (no receipt) are repaired, never
    /// deletions. Released pending tasks with no attempt are dispatched once.
    pub(crate) async fn recover_workflows(self: &Arc<Self>) {
        let runs = match self.l4.workflows.active_runs().await {
            Ok(runs) => runs,
            Err(error) => {
                tracing::warn!("could not load workflow runs: {error}");
                return;
            }
        };
        for run in runs {
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
                    if let Ok(Some(task)) = self.l4.store.get(&task_id).await {
                        if task.status == TaskStatus::Pending
                            && task.after.is_none()
                            && (task.runs == 0 || node.round > 0)
                            && self
                                .l4.store
                                .active_run(&task_id)
                                .await
                                .ok()
                                .flatten()
                                .is_none()
                        {
                            let engine = self.clone();
                            tokio::spawn(async move {
                                engine.l4_service().start_run(&task_id, Trigger::Workflow).await;
                            });
                        }
                    }
                }
            }
        }

        // R11: a terminal run can still carry a node overlay that never
        // caught up with its task -- a report the daemon missed, or a row
        // an older, buggier build wrote. `advance_workflow` already mirrors
        // unconditionally and refuses to spawn anything or rewrite a
        // terminal run's own status once it sees the run is settled (B2),
        // so reusing it here is exactly "mirror only, do nothing else" with
        // no separate mechanism to keep in sync with that one.
        match self.l4.workflows.recent_terminal_runs(200).await {
            Ok(terminal_runs) => {
                for run in terminal_runs {
                    if let Err(error) = self.close_workflow_waits(&run).await {
                        tracing::warn!(workflow_run = run.id, "could not close recovered workflow waits: {error}");
                    }
                    let stale = run
                        .nodes
                        .iter()
                        .any(|node| node.task_id.is_some() && !node.status.is_terminal());
                    if stale {
                        if let Err(error) = self.advance_workflow(&run.id).await {
                            tracing::warn!(
                                workflow_run = run.id,
                                "could not reconcile a terminal workflow run: {error}"
                            );
                        }
                    }
                }
            }
            Err(error) => {
                tracing::warn!("could not load recent workflow runs for reconciliation: {error}");
            }
        }
    }
}
