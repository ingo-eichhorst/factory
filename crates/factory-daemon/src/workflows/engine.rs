use crate::access::Caller;
use crate::engine::Engine;
use crate::verification::run_shell_capture;
#[cfg(not(test))]
use crate::verification::DEFAULT_GATE_TIMEOUT_SECS;
use chrono::Utc;
use factory_core::control_plan::AttestationVerdict;
use factory_core::error::{FactoryError, Result};
use factory_core::event::Event;
#[cfg(test)]
use factory_core::run::FailKind;
use factory_core::run::{RunStatus, Trigger};
use factory_core::task::{Task, TaskEntry, TaskPatch, TaskStatus, WorkflowOrigin};
use factory_core::workflow::{
    SendBack, WorkflowDefinition, WorkflowDraft, WorkflowNodeKind, WorkflowNodeStatus, WorkflowRun, WorkflowRunStatus,
};
use std::collections::BTreeMap;
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
    use factory_core::task::{NewTask, SessionRef, TaskEntry, TaskFilter, TaskReport};
    use factory_core::workflow::{
        CanvasPoint, WorkflowActor, WorkflowEdge, WorkflowExit, WorkflowNode, WorkflowNodeKind,
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

    /// `#178`: a runtime that says a session is `Gone` once it has been
    /// stopped -- what herdr says of a closed pane, and what
    /// `resolve_continue` needs to hear before it will resume a conversation
    /// from disk (rule 1). Every other runtime here answers `Working`
    /// forever, so no test on them can ever reach a resume. Records each
    /// launch (args and cwd) and prompt by run id.
    struct ResumableRuntime {
        starts: std::sync::Mutex<Vec<StartRequest>>,
        prompts: std::sync::Mutex<Vec<(String, String)>>,
        stopped: std::sync::Mutex<std::collections::BTreeSet<String>>,
    }

    impl ResumableRuntime {
        fn new() -> Self {
            Self { starts: Default::default(), prompts: Default::default(), stopped: Default::default() }
        }
        fn start_for(&self, run_id: &str) -> Option<StartRequest> {
            self.starts.lock().unwrap().iter().find(|s| s.id == run_id).cloned()
        }
        fn prompt_for(&self, run_id: &str) -> Option<String> {
            self.prompts.lock().unwrap().iter().find(|(h, _)| h == run_id).map(|(_, p)| p.clone())
        }
    }

    #[async_trait::async_trait]
    impl AgentRuntime for ResumableRuntime {
        fn name(&self) -> &str {
            "quiet"
        }
        async fn start(&self, req: &StartRequest) -> Result<SessionRef> {
            self.starts.lock().unwrap().push(req.clone());
            Ok(SessionRef { runtime: "quiet".into(), handle: req.id.clone(), meta: Default::default() })
        }
        async fn submit(&self, session: &SessionRef, text: &str) -> Result<()> {
            self.prompts.lock().unwrap().push((session.handle.clone(), text.to_string()));
            Ok(())
        }
        async fn status(&self, session: &SessionRef) -> Result<factory_core::adapter::RuntimeStatus> {
            Ok(if self.stopped.lock().unwrap().contains(&session.handle) {
                factory_core::adapter::RuntimeStatus::Gone
            } else {
                factory_core::adapter::RuntimeStatus::Working
            })
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

    /// `engine()` on `runtime`, with `daemon` adjusted by `tweak`.
    fn engine_on(runtime: Arc<dyn AgentRuntime>, tweak: impl FnOnce(&mut DaemonConfig)) -> Arc<Engine> {
        let base = engine();
        let mut config = base.factory_snapshot().config.clone();
        tweak(&mut config.daemon);
        let root = base.factory_snapshot().root.clone();
        let mut registry = Registry::with_builtins();
        registry.add_runtime(runtime, "test");
        Arc::new(Engine::new(
            Factory { root, config },
            registry,
            Arc::new(SqliteStore::in_memory().unwrap()),
            PathBuf::from("factory"),
            Vec::new(),
        ))
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
            }],
            infrastructure: Default::default(),
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
        let mut config: Config = serde_yaml_ng::from_str(yaml).unwrap();
        // None of these fixtures write a `daemon:` block, so this would
        // otherwise default on -- and some of these tests dispatch real
        // runs, which would fork a real `caffeinate` on whatever machine
        // runs them.
        config.daemon.power_assertion = false;
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
            }],
            infrastructure: Default::default(),
            plugins_dir: None,
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
            session: Default::default(),
        }
    }

    /// Like `node`, but names a bare adapter (`claude-code`) rather than
    /// `shell` -- `resolve_agent`'s fallback to any adapter that exists
    /// (no declaration required) is what makes this work with only the
    /// "quiet" runtime configured, and it is what lets a test read the
    /// *rendered* upstream section rather than the file `shell` points at.
    fn harness_node(id: &str) -> WorkflowNode {
        WorkflowNode {
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
            session: Default::default(),
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

    /// The tasks that have been handed a run. Every node's task exists from
    /// the moment its workflow run starts (`#178`), so "which nodes have
    /// started" is which tasks have runs, not which tasks exist.
    async fn started(engine: &Arc<Engine>) -> Vec<factory_core::Task> {
        tasks(engine).await.into_iter().filter(|t| t.runs > 0).collect()
    }

    async fn wait_for_tasks(engine: &Arc<Engine>, count: usize) -> Vec<factory_core::Task> {
        for _ in 0..200 {
            let found = started(engine).await;
            if found.len() == count {
                return found;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("expected {count} started tasks, got {}", started(engine).await.len());
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
            if let Some(run) = engine.store.active_run(task_id).await.unwrap() {
                break run;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        engine
            .report(
                task_id,
                TaskReport {
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
            if let Some(run) = engine.store.active_run(task_id).await.unwrap() {
                if let Some(prompt) = recorder.prompt_for(&run.id) {
                    return prompt;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("no prompt captured for task {task_id}");
    }

    /// Poll until `task_id` has its `attempt`th run -- a rework round is a
    /// new run of the same task (`#178`), so this, not a task count, is what
    /// says a round has started.
    async fn wait_for_run(engine: &Arc<Engine>, task_id: &str, attempt: u32) -> factory_core::run::Run {
        for _ in 0..400 {
            let runs = engine.store.runs(task_id, 50).await.unwrap();
            if let Some(run) = runs.into_iter().find(|r| r.attempt == attempt) {
                return run;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("task {task_id} never got run {attempt}");
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
            started(&engine).await.len(),
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
        assert_eq!(started(&engine).await.len(), 3, "fan-in still waits for c");
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
        assert_eq!(started(&engine).await.len(), 1);
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
            started(&engine).await.len(),
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
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
        wait_for_tasks(&engine, 1).await;
        engine.recover_workflows().await;
        engine.recover_workflows().await;
        assert_eq!(
            started(&engine).await.len(),
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
            engine.workflows.active_runs().await.unwrap().is_empty(),
            "a denial at start persists no run at all"
        );
        assert!(tasks(&engine).await.is_empty(), "nor any task");
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
        engine.store.put_agent(&agent).await.unwrap();

        let definition = create(&engine, vec![node("a"), node("b")], vec![edge("a", "b")]).await;
        let caller = Caller::Agent {
            scope: "demo".into(),
            name: "w".into(),
            role: engine.effective_role("demo", "w").await,
            run_id: None,
        };
        let run = engine.start_workflow(&definition.id, Default::default(), &caller).await.unwrap();
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
        assert!(
            b.error.as_deref().unwrap_or("").contains("create tasks"),
            "{:?}",
            b.error
        );
        // `#178`: b's task was made up front, when the role still allowed
        // it; the denial at its turn means it never runs, and it does not
        // linger either -- it is closed as not planned with the run.
        let b_task = engine.store.get(b.task_id.as_deref().unwrap()).await.unwrap().unwrap();
        assert_eq!(b_task.runs, 0, "the denied node's task never ran");
        assert_eq!(b_task.close_reason(), Some(factory_core::task::CloseReason::NotPlanned));
        assert_eq!(started(&engine).await.len(), 1, "nothing ran for the denied node");
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
        assert_eq!(started(&engine).await.len(), 0);
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
            started(&engine).await.len(),
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
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
        assert_eq!(run.status, WorkflowRunStatus::Failed);
        assert_eq!(run.failure_node_id.as_deref(), Some("a"));
        let a = run.nodes.iter().find(|n| n.node_id == "a").unwrap();
        assert!(a.task_id.is_none());
        assert!(a.error.as_deref().unwrap_or("").contains("no agent named"));
        assert_eq!(started(&engine).await.len(), 0);
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
                }],
                infrastructure: Default::default(),
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
            started(&engine2).await.len(),
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
        run.nodes[0].launched_round = Some(0);
        run.nodes[0].status = WorkflowNodeStatus::Pending;
        engine.workflows.put_run(&run).await.unwrap();

        engine.recover_workflows().await;
        let created = wait_for_tasks(&engine, 1).await;
        assert_eq!(created[0].id, phantom_id, "recovery fills in exactly the persisted id");

        engine.recover_workflows().await;
        assert_eq!(started(&engine).await.len(), 1, "a second recovery does not duplicate it");
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
        engine.workflows.put_run(&stale).await.unwrap();

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
            started(&engine).await.len(),
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
            if let Some(run) = engine.store.active_run(task_id).await.unwrap() {
                break run;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        engine
            .report(
                task_id,
                TaskReport {
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
            if let Some(active) = engine.store.active_run(&review.id).await.unwrap() {
                break active;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        let refused = engine.report(&review.id, TaskReport {
            status: Some(RunStatus::Done), message: None, result: Some("findings".into()),
            send_to: Some("nowhere".into()), error: None, token: active.token,
        }).await.unwrap_err().to_string();
        assert!(refused.contains("implement (concrete findings the implementer can fix alone)"), "{refused}");
        review_fails(&engine, &review.id, "the parser test is missing").await;

        let verdict = engine.store.runs(&review.id, 1).await.unwrap().remove(0);
        assert_eq!(verdict.status, RunStatus::Done, "sending work back is a successful review verdict");
        assert_eq!(verdict.routed_to.as_deref(), Some("implement"));
        let routed_review = engine.store.get(&review.id).await.unwrap().unwrap();
        assert!(routed_review.failure.is_none());
        // `#178`: its own next round waits on the implementation's.
        assert_eq!(routed_review.status, TaskStatus::Pending);
        let after = routed_review.after.clone().expect("the review waits on implement's round");
        assert_eq!(after.tasks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), vec![first.id.as_str()]);
        let review_entries = engine.store.entries(&review.id, 20).await.unwrap();
        assert!(review_entries.iter().any(|entry| {
            entry.kind == "routed_to"
                && entry
                    .data
                    .as_ref()
                    .is_some_and(|data| data["routed_to"] == "implement")
        }));

        // `#178`: the round is a second run of the same implement task --
        // no `(rework 1)` task beside it, the title untouched, the round and
        // the findings on the run itself.
        let second = wait_for_run(&engine, &first.id, 2).await;
        assert_eq!(started(&engine).await.len(), 2, "a round never creates a task");
        let again = engine.store.get(&first.id).await.unwrap().unwrap();
        assert_eq!(again.title, "implement");
        assert_eq!(second.round, 1);
        assert_eq!(second.trigger, Trigger::Workflow);
        let feedback = second.feedback.clone().expect("the round carries its findings");
        assert_eq!(feedback.from_node, "review");
        assert_eq!(feedback.from_task, review.id);
        assert_eq!((feedback.round, feedback.max_rounds), (1, 5));
        assert!(feedback.text.contains("the parser test is missing"), "{feedback:?}");
        let prompt = wait_for_prompt(&engine, &recorder, &first.id).await;
        assert!(prompt.contains("sent this work back -- rework round 1 of 5"), "{prompt}");
        assert!(prompt.contains("the parser test is missing"), "{prompt}");
        let entries = engine.store.entries(&first.id, 50).await.unwrap();
        assert!(
            entries.iter().any(|e| e.kind == "round" && e.message.contains("rework round 1 of 5")),
            "the journal says why a done task runs again: {entries:#?}"
        );

        let midway = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(midway.status, WorkflowRunStatus::Running);
        assert!(node_run(&midway, "implement").superseded_task_ids.is_empty());
        assert_eq!(node_run(&midway, "implement").task_id.as_deref(), Some(first.id.as_str()));
        assert_eq!(node_run(&midway, "review").task_id.as_deref(), Some(review.id.as_str()));
        assert_eq!(node_run(&midway, "review").round, 1);
        assert_eq!(node_run(&midway, "review").status, WorkflowNodeStatus::Waiting);
        assert_eq!(node_run(&midway, "ship").status, WorkflowNodeStatus::Waiting);

        // The review's own last-round task state says nothing about round 1.
        engine.record_workflow_task_state(&review.id).await;
        assert_eq!(
            node_run(&engine.workflow_run(&run.id).await.unwrap(), "review").status,
            WorkflowNodeStatus::Waiting
        );

        finish_with_result(&engine, &first.id, "PR https://example.test/pr/1, fixed").await;
        let second_review = wait_for_run(&engine, &review.id, 2).await;
        assert_eq!(second_review.round, 1);
        assert!(second_review.feedback.is_none(), "only the node sent back to gets the findings");
        assert_eq!(engine.store.get(&review.id).await.unwrap().unwrap().title, "review");
        finish(&engine, &review.id, RunStatus::Done).await;
        let ship = of_node(&wait_for_tasks(&engine, 3).await, "ship")[0].clone();
        finish(&engine, &ship.id, RunStatus::Done).await;
        assert_eq!(engine.workflow_run(&run.id).await.unwrap().status, WorkflowRunStatus::Done);
        let rounds: Vec<u32> = engine.store.runs(&first.id, 10).await.unwrap().iter().rev().map(|r| r.round).collect();
        assert_eq!(rounds, vec![0, 1], "the task's run list shows each round");
    }

    /// `#178`'s migration rule: a run already in flight from before rounds
    /// were runs finishes the way it started -- its round is a new
    /// `(rework N)` task, and the task it supersedes stays as it was.
    #[tokio::test]
    async fn a_legacy_run_in_flight_still_gives_a_round_a_task_of_its_own() {
        let engine = engine();
        let definition = review_loop(&engine, 5).await;
        // As a run from before this build was stored: no shape flag, and
        // each node's task made only when its turn came.
        let mut run = WorkflowRun::new(definition.clone(), WorkflowActor::Owner);
        run.rounds_as_runs = false;
        engine.workflows.put_run(&run).await.unwrap();
        engine.advance_workflow(&run.id).await.unwrap();
        assert_eq!(tasks(&engine).await.len(), 1, "a legacy run makes a task only when its turn comes");

        let first = wait_for_tasks(&engine, 1).await.pop().unwrap();
        finish(&engine, &first.id, RunStatus::Done).await;
        let review = of_node(&wait_for_tasks(&engine, 2).await, "review")[0].clone();
        review_fails(&engine, &review.id, "missing test").await;
        let all = wait_for_tasks(&engine, 3).await;
        let again = of_node(&all, "implement").into_iter().find(|t| t.id != first.id).unwrap().clone();
        assert_eq!(again.title, "implement (rework 1)");
        assert_eq!(engine.store.runs(&first.id, 10).await.unwrap().len(), 1, "the superseded task never runs again");
        let midway = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(node_run(&midway, "implement").superseded_task_ids, vec![first.id.clone()]);
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
        wait_for_run(&engine, &implement.id, 2).await;
        finish(&engine, &implement.id, RunStatus::Done).await;
        wait_for_run(&engine, &review.id, 2).await;
        let last = review.clone();
        let active = loop {
            if let Some(active) = engine.store.active_run(&last.id).await.unwrap() {
                break active;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        let error = engine
            .report(
                &last.id,
                TaskReport {
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
            started(&engine).await.len(),
            2,
            "nothing spawned past the budget"
        );
        assert_eq!(engine.store.runs(&implement.id, 10).await.unwrap().len(), 2, "and no third implement round");
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
            if let Some(active) = engine.store.active_run(&review.id).await.unwrap() {
                break active;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        engine.fail_run(&active.id, FailKind::RunTimeout, "ran out of time").await;
        engine.sync_workflow_for_task(&review.id).await;

        let run = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(run.status, WorkflowRunStatus::Failed);
        assert_eq!(node_run(&run, "review").round, 0);
        assert_eq!(started(&engine).await.len(), 2);
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
            started(&engine).await.len(),
            2,
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
        let entries = engine.store.entries(&a.id, 20).await.unwrap();
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
        let entries = engine.store.entries(&a.id, 20).await.unwrap();
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
            if let Some(active) = engine.store.active_run(&task.id).await.unwrap() {
                break active;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        let wrong_status = engine
            .report(
                &task.id,
                TaskReport {
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
            .report(
                &task.id,
                TaskReport {
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

    // --- #178: a rework round resumes its previous run's session ---

    /// Run `review_loop` on `engine` up to the moment the review sends the
    /// work back, with the first implement run having recorded `session`
    /// as its harness session id. Returns (implement task, its first run,
    /// review task).
    async fn send_back_once(
        engine: &Arc<Engine>,
        definition: &WorkflowDefinition,
        session: Option<&str>,
    ) -> (factory_core::Task, factory_core::run::Run, factory_core::Task) {
        engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
        let implement = wait_for_tasks(engine, 1).await.pop().unwrap();
        let first = wait_for_run(engine, &implement.id, 1).await;
        if let Some(session) = session {
            engine
                .store
                .update_run(
                    &first.id,
                    &factory_core::run::RunPatch { turn_ended_session_id: Some(session.into()), ..Default::default() },
                )
                .await
                .unwrap();
        }
        finish_with_result(engine, &implement.id, "PR https://example.test/pr/1").await;
        let review = of_node(&wait_for_tasks(engine, 2).await, "review")[0].clone();
        review_fails(engine, &review.id, "the parser test is missing").await;
        (implement, first, review)
    }

    #[tokio::test]
    async fn a_rework_round_resumes_the_previous_runs_conversation_and_reports_on_its_own_token() {
        let runtime = Arc::new(ResumableRuntime::new());
        let engine = engine_on(runtime.clone(), |_| {});
        let definition = review_loop(&engine, 5).await;
        let (implement, first, _) = send_back_once(&engine, &definition, Some("sess-1")).await;

        let second = wait_for_run(&engine, &implement.id, 2).await;
        let prompt = loop {
            if let Some(p) = runtime.prompt_for(&second.id) {
                break p;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        let second = engine.store.get_run(&second.id).await.unwrap().unwrap();
        assert_eq!(second.resumed_session.as_deref(), Some("sess-1"));
        assert_eq!(second.continued_from.as_deref(), Some(first.id.as_str()));
        assert_eq!(second.round, 1);
        let start = runtime.start_for(&second.id).unwrap();
        assert_eq!(&start.launch.args[..2], ["--resume", "sess-1"], "claude is launched into its own session");
        assert_eq!(start.cwd, runtime.start_for(&first.id).unwrap().cwd, "in the same working directory");
        assert!(prompt.contains("rework round 1"), "{prompt}");
        assert!(prompt.contains("the parser test is missing"), "the feedback is the resumed turn's prompt: {prompt}");
        assert!(prompt.contains("void now"), "earlier report commands are void: {prompt}");
        assert!(!prompt.contains("do the thing"), "the task is not replayed into its own conversation: {prompt}");

        // The previous run's token is dead, with a reason; the new one works.
        let stale = engine
            .report(&implement.id, TaskReport {
                status: Some(RunStatus::Done), message: None, result: None, send_to: None, error: None,
                token: first.token.clone(),
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(stale.contains("a newer run of task"), "{stale}");
        finish_with_result(&engine, &implement.id, "fixed").await;
        assert_eq!(engine.store.get_run(&second.id).await.unwrap().unwrap().status, RunStatus::Done);
    }

    async fn fallback_reason(engine: &Arc<Engine>, task_id: &str) -> String {
        for _ in 0..400 {
            let entries = engine.store.entries(task_id, 100).await.unwrap();
            if let Some(entry) = entries.iter().find(|e| e.kind == "continue_fallback") {
                return entry.message.clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("no continue_fallback entry on {task_id}");
    }

    #[tokio::test]
    async fn a_rework_round_falls_back_to_fresh_and_says_why() {
        // No session id was ever recorded for the previous run.
        let engine = engine_on(Arc::new(ResumableRuntime::new()), |_| {});
        let definition = review_loop(&engine, 5).await;
        let (implement, _, _) = send_back_once(&engine, &definition, None).await;
        assert!(fallback_reason(&engine, &implement.id).await.contains("no session id was recorded"));
        let second = wait_for_run(&engine, &implement.id, 2).await;
        assert!(second.resumed_session.is_none());
        assert_eq!(second.round, 1, "a fresh round is still the round");

        // The node opts out of resuming.
        let engine = engine_on(Arc::new(ResumableRuntime::new()), |_| {});
        let mut definition = review_loop(&engine, 5).await;
        let mut draft = WorkflowDraft {
            name: definition.name.clone(),
            scope: "demo".into(),
            nodes: definition.nodes.clone(),
            edges: definition.edges.clone(),
            ..Default::default()
        };
        draft.nodes.iter_mut().find(|n| n.id == "implement").unwrap().session = factory_core::workflow::NodeSession::Fresh;
        definition = engine.update_workflow(&definition.id, draft).await.unwrap();
        let (implement, _, _) = send_back_once(&engine, &definition, Some("sess-1")).await;
        assert!(fallback_reason(&engine, &implement.id).await.contains("session: fresh"));

        // Past the round cap.
        let engine = engine_on(Arc::new(ResumableRuntime::new()), |d| d.resume_round_cap = 0);
        let definition = review_loop(&engine, 5).await;
        let (implement, _, _) = send_back_once(&engine, &definition, Some("sess-1")).await;
        assert!(fallback_reason(&engine, &implement.id).await.contains("resume cap of 0"));

        // The shell agent never resumes: the review's own round says so.
        let engine = engine_on(Arc::new(ResumableRuntime::new()), |_| {});
        let definition = review_loop(&engine, 5).await;
        let (implement, _, review) = send_back_once(&engine, &definition, Some("sess-1")).await;
        wait_for_run(&engine, &implement.id, 2).await;
        finish(&engine, &implement.id, RunStatus::Done).await;
        wait_for_run(&engine, &review.id, 2).await;
        let why = fallback_reason(&engine, &review.id).await;
        assert!(why.contains("the shell adapter declares no resume"), "{why}");
    }

    #[tokio::test]
    async fn a_rework_round_never_resumes_a_session_that_is_not_confirmed_gone() {
        // `QuietRuntime` answers `Working` for every session, stopped or not.
        let engine = engine();
        let definition = review_loop(&engine, 5).await;
        let (implement, _, _) = send_back_once(&engine, &definition, Some("sess-1")).await;
        assert!(fallback_reason(&engine, &implement.id).await.contains("not confirmed gone"));
        assert!(wait_for_run(&engine, &implement.id, 2).await.resumed_session.is_none());
    }

    // --- #178: nodes waiting on their upstream exist as tasks from the start ---

    fn task_of(all: &[factory_core::Task], node: &str) -> factory_core::Task {
        all.iter().find(|t| t.workflow_origin.as_ref().unwrap().node_id == node).unwrap().clone()
    }

    #[tokio::test]
    async fn every_node_has_its_task_from_the_start_and_a_waiting_one_starts_when_its_upstream_finishes() {
        let engine = engine();
        let definition = create(&engine, vec![node("a"), node("b")], vec![edge("a", "b")]).await;
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
        let all = tasks(&engine).await;
        assert_eq!(all.len(), 2, "b's task exists before a has finished");
        let (a, b) = (task_of(&all, "a"), task_of(&all, "b"));
        assert_eq!(b.status, TaskStatus::Pending);
        assert_eq!(b.runs, 0);
        assert!(b.schedule.is_none());
        let after = b.after.clone().expect("b waits on a");
        assert_eq!(after.tasks, vec![factory_core::task::AfterTask { id: a.id.clone(), title: "a".into() }]);
        assert!(after.conditional.is_none(), "nothing routes around b");
        assert_eq!(node_run(&engine.workflow_run(&run.id).await.unwrap(), "b").status, WorkflowNodeStatus::Waiting);
        let entries = engine.store.entries(&b.id, 20).await.unwrap();
        assert!(entries.iter().any(|e| e.kind == "waiting" && e.message == "waiting on a"), "{entries:#?}");

        finish(&engine, &a.id, RunStatus::Done).await;
        let b_run = wait_for_run(&engine, &b.id, 1).await;
        assert_eq!(b_run.trigger, Trigger::Workflow);
        let b = engine.store.get(&b.id).await.unwrap().unwrap();
        assert!(b.after.is_none(), "the wait is over");
        let entries = engine.store.entries(&b.id, 20).await.unwrap();
        assert!(entries.iter().any(|e| e.kind == "released" && e.message.contains("a finished")), "{entries:#?}");
        assert_eq!(tasks(&engine).await.len(), 2, "b was never created twice");
    }

    #[tokio::test]
    async fn a_restart_never_starts_a_task_that_is_still_waiting() {
        let engine = engine();
        let definition = create(&engine, vec![node("a"), node("b")], vec![edge("a", "b")]).await;
        engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
        let b = task_of(&tasks(&engine).await, "b");
        engine.recover_workflows().await;
        engine.recover_workflows().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let b = engine.store.get(&b.id).await.unwrap().unwrap();
        assert_eq!(b.runs, 0, "recovery dispatches a launched node, never a waiting one");
        assert!(b.after.is_some());
        assert_eq!(tasks(&engine).await.len(), 2);
    }

    #[tokio::test]
    async fn a_branch_the_route_never_takes_reads_as_conditional_and_is_closed_as_not_planned() {
        let engine = engine();
        let mut a = node("a");
        a.exits = vec![WorkflowExit { to: "c".into(), check: Some("true".into()), agent: None, max_rounds: None }];
        let definition = create(
            &engine,
            vec![a, node("b"), node("c")],
            vec![edge("a", "b"), edge("b", "c"), edge("a", "c")],
        )
        .await;
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
        let all = tasks(&engine).await;
        let b = task_of(&all, "b");
        let conditional = b.after.as_ref().and_then(|a| a.conditional.clone()).expect("b may never run");
        assert!(conditional.contains("runs only if a routes"), "{conditional}");

        finish(&engine, &task_of(&all, "a").id, RunStatus::Done).await;
        let c = task_of(&all, "c");
        wait_for_run(&engine, &c.id, 1).await;
        finish(&engine, &c.id, RunStatus::Done).await;
        let settled = engine.workflow_run(&run.id).await.unwrap();
        assert_eq!(settled.status, WorkflowRunStatus::Done);
        assert_eq!(node_run(&settled, "b").status, WorkflowNodeStatus::SkippedByRoute);
        let b = engine.store.get(&b.id).await.unwrap().unwrap();
        assert_eq!(b.runs, 0);
        assert_eq!(b.close_reason(), Some(factory_core::task::CloseReason::NotPlanned));
        assert!(b.closure.as_ref().unwrap().note.as_deref().unwrap_or("").contains("another branch"), "{:?}", b.closure);
    }

    #[tokio::test]
    async fn cancelling_a_workflow_closes_its_waiting_tasks_with_it() {
        let engine = engine();
        let definition = create(&engine, vec![node("a"), node("b")], vec![edge("a", "b")]).await;
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
        let b = task_of(&tasks(&engine).await, "b");
        let cancelled = engine.cancel_workflow(&run.id).await.unwrap();
        assert_eq!(node_run(&cancelled, "b").status, WorkflowNodeStatus::Skipped);
        let b = engine.store.get(&b.id).await.unwrap().unwrap();
        assert_eq!(b.close_reason(), Some(factory_core::task::CloseReason::NotPlanned));
        assert_eq!(b.runs, 0);
    }

    #[tokio::test]
    async fn running_a_waiting_task_by_hand_is_refused_unless_asked_and_then_counts_as_the_step() {
        use factory_core::protocol::{Request, Response};
        let engine = engine();
        let definition = create(&engine, vec![node("a"), node("b")], vec![edge("a", "b")]).await;
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
        let all = tasks(&engine).await;
        let (a, b) = (task_of(&all, "a"), task_of(&all, "b"));

        let refused = engine
            .handle_request(Request::TaskRun { id: b.id.clone(), reason: None, continue_run: false, start_waiting: false })
            .await;
        match refused {
            Response::Error { message, .. } => {
                assert!(message.contains("waiting on a") && message.contains("--ignore-wait"), "{message}")
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert_eq!(engine.store.get(&b.id).await.unwrap().unwrap().runs, 0);

        let started = engine
            .handle_request(Request::TaskRun {
                id: b.id.clone(),
                reason: Some("unblock the demo".into()),
                continue_run: false,
                start_waiting: true,
            })
            .await;
        assert!(matches!(started, Response::Ok { .. }), "{started:?}");
        wait_for_run(&engine, &b.id, 1).await;
        let entries = engine.store.entries(&b.id, 20).await.unwrap();
        assert!(entries.iter().any(|e| e.kind == "wait_overridden" && e.message.contains("unblock the demo")), "{entries:#?}");
        assert!(node_run(&engine.workflow_run(&run.id).await.unwrap(), "b").launched(true));

        finish(&engine, &b.id, RunStatus::Done).await;
        finish(&engine, &a.id, RunStatus::Done).await;
        assert_eq!(engine.store.runs(&b.id, 10).await.unwrap().len(), 1, "a's finishing never starts b a second time");
        assert_eq!(engine.workflow_run(&run.id).await.unwrap().status, WorkflowRunStatus::Done);
    }

    // --- #178: worktrees go once nothing will resume in them ---

    /// `engine()` whose "demo" scope is a real git repository with one
    /// commit, so a node with `worktree: true` gets a real worktree.
    fn engine_in_git() -> Arc<Engine> {
        let base = engine();
        let mut config = base.factory_snapshot().config.clone();
        let repo = std::env::temp_dir().join(format!("factory-workflow-git-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&repo).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["-c", "user.email=f@e", "-c", "user.name=f", "-c", "commit.gpgsign=false", "commit", "-q", "--allow-empty", "-m", "base"],
        ] {
            assert!(std::process::Command::new("git").arg("-C").arg(&repo).args(&args).status().unwrap().success());
        }
        config.scopes[0].path = repo;
        let root = base.factory_snapshot().root.clone();
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

    fn worktree_node(id: &str) -> WorkflowNode {
        let mut n = node(id);
        n.task.worktree = Some(true);
        n
    }

    async fn worktree_of(engine: &Arc<Engine>, task_id: &str) -> PathBuf {
        let run = wait_for_run(engine, task_id, 1).await;
        for _ in 0..400 {
            if let Some(path) = engine.store.get_run(&run.id).await.unwrap().unwrap().worktree_path {
                return PathBuf::from(path);
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("no worktree for {task_id}");
    }

    fn kinds(entries: &[TaskEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.kind.as_str()).collect()
    }

    #[tokio::test]
    async fn a_workflows_worktrees_stay_while_it_runs_and_go_when_it_is_finished() {
        let engine = engine_in_git();
        let definition = create(&engine, vec![worktree_node("a"), worktree_node("b")], vec![edge("a", "b")]).await;
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
        let all = tasks(&engine).await;
        let (a, b) = (task_of(&all, "a"), task_of(&all, "b"));
        let a_dir = worktree_of(&engine, &a.id).await;
        finish(&engine, &a.id, RunStatus::Done).await;
        let b_dir = worktree_of(&engine, &b.id).await;
        assert!(a_dir.exists(), "a later node or a round may still resume in a's worktree");
        finish(&engine, &b.id, RunStatus::Done).await;
        assert_eq!(engine.workflow_run(&run.id).await.unwrap().status, WorkflowRunStatus::Done);
        assert!(!a_dir.exists() && !b_dir.exists(), "a finished workflow takes all of its worktrees");
        let entries = engine.store.entries(&a.id, 50).await.unwrap();
        let released = entries.iter().find(|e| e.kind == "worktree_released").unwrap_or_else(|| panic!("{:?}", kinds(&entries)));
        assert!(released.message.contains("is finished") && released.message.contains("deleted"), "{}", released.message);
    }

    #[tokio::test]
    async fn a_worktree_holding_uncommitted_work_is_kept_and_journaled_never_thrown_away() {
        let engine = engine_in_git();
        let definition = create(&engine, vec![worktree_node("a")], vec![]).await;
        engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
        let a = task_of(&tasks(&engine).await, "a");
        let dir = worktree_of(&engine, &a.id).await;
        std::fs::write(dir.join("half-done.txt"), "work in progress\n").unwrap();
        finish(&engine, &a.id, RunStatus::Done).await;
        assert!(dir.join("half-done.txt").exists(), "nothing is thrown away");
        let entries = engine.store.entries(&a.id, 50).await.unwrap();
        let kept = entries.iter().find(|e| e.kind == "worktree_kept").unwrap_or_else(|| panic!("{:?}", kinds(&entries)));
        assert!(kept.message.contains("1 uncommitted change"), "{}", kept.message);

        // Once it is safe, the start-up sweep takes it -- and journals a
        // kept worktree only once, however often it looks.
        engine.sweep_worktrees().await;
        let again = engine.store.entries(&a.id, 50).await.unwrap();
        assert_eq!(again.iter().filter(|e| e.kind == "worktree_kept").count(), 1);
        std::fs::remove_file(dir.join("half-done.txt")).unwrap();
        engine.sweep_worktrees().await;
        assert!(!dir.exists(), "the sweep released it once it was clean");
    }

    #[tokio::test]
    async fn a_failed_nodes_worktree_waits_for_a_person_and_goes_when_its_task_is_closed() {
        use factory_core::protocol::{Request, Response};
        let engine = engine_in_git();
        let definition = create(&engine, vec![worktree_node("a"), worktree_node("b")], vec![edge("a", "b")]).await;
        let run = engine.start_workflow(&definition.id, Default::default(), &Caller::Owner).await.unwrap();
        let a = task_of(&tasks(&engine).await, "a");
        let dir = worktree_of(&engine, &a.id).await;
        finish(&engine, &a.id, RunStatus::Failed).await;
        assert_eq!(engine.workflow_run(&run.id).await.unwrap().status, WorkflowRunStatus::Failed);
        assert!(dir.exists(), "a person may still --continue the failed node in it");

        let closed = engine
            .handle_request(Request::TaskClose {
                id: a.id.clone(),
                reason: factory_core::task::CloseReason::NotPlanned,
                duplicate_of: None,
                note: Some("giving up".into()),
            })
            .await;
        assert!(matches!(closed, Response::Ok { .. }), "{closed:?}");
        assert!(!dir.exists(), "closing the last unsettled task finishes the workflow, and its worktrees go");
    }

    #[tokio::test]
    async fn a_one_off_task_outside_a_workflow_lets_its_worktree_go_when_it_is_done() {
        let engine = engine_in_git();
        let task = engine
            .create(NewTask {
                title: "one-off".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                runtime: Some("quiet".into()),
                worktree: Some(true),
                ..Default::default()
            })
            .await
            .unwrap();
        engine.start_run(&task.id, Trigger::Manual).await;
        let dir = worktree_of(&engine, &task.id).await;
        finish(&engine, &task.id, RunStatus::Done).await;
        assert!(!dir.exists());
        let entries = engine.store.entries(&task.id, 50).await.unwrap();
        assert!(entries.iter().any(|e| e.kind == "worktree_released" && e.message.contains("the task is finished")), "{:?}", kinds(&entries));
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
}

/// A node's status, read off its task. A task blocked by a failure is a
/// failed node (`#122`): the task waits for a person, but the workflow's
/// step did fail, and its on-failure edges and retries must see that.
fn node_status(task: &Task) -> WorkflowNodeStatus {
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
            .map_or(0, |node| node.round);
        if exit.max_rounds.is_some_and(|max| used >= max) {
            return Err(FactoryError::BadRequest(format!(
                "agent exit to {to} has no rounds left: report `blocked` with the open findings"
            )));
        }
        Ok(())
    }

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
        let plans = self.control_plans(&definition).await?;
        let (definition, _) = definition.inject(&plans);
        definition.validate().map_err(FactoryError::BadRequest)?;
        let mut run = WorkflowRun::new(definition, caller.as_workflow_actor());
        run.inputs = inputs;
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
            let rounds_as_runs = run.rounds_as_runs;
            for node in &mut run.nodes {
                if !node.status.is_terminal() {
                    match &node.task_id {
                        Some(task_id) if node.launched(rounds_as_runs) => task_ids.push(task_id.clone()),
                        _ => node.status = WorkflowNodeStatus::Skipped,
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
                let _ = self
                    .cancel_task_run(&task_id, None, factory_core::run::FailKind::CancelledWithParent)
                    .await;
            }
        }
        let mut run = self.workflow_run(id).await?;
        let rounds_as_runs = run.rounds_as_runs;
        for node in &mut run.nodes {
            if !node.launched(rounds_as_runs) {
                continue;
            }
            if let Some(task_id) = &node.task_id {
                if let Some(task) = self.store.get(task_id).await? {
                    node.status = node_status(&task);
                    node.error = task.error;
                }
            }
        }
        run.updated_at = Utc::now();
        self.workflows.put_run(&run).await?;
        self.bus
            .publish(Event::WorkflowRunUpdated { run: run.clone() });
        // Its waiting tasks close with it (`#178`), and once nothing of it is
        // still running or waiting on a person, its worktrees go too.
        self.settle_unreached_tasks(&run).await;
        self.release_finished_workflow(&run).await;
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
        let rounds_as_runs = run.rounds_as_runs;
        for node in &mut run.nodes {
            let Some(task_id) = node.task_id.clone() else {
                continue;
            };
            // `#178`: a node sent back for another round keeps its task, and
            // that task still reads as the last round's outcome until its
            // next run is asked for. Until then the node's status is the
            // engine's own, not its task's.
            if !node.launched(rounds_as_runs) {
                continue;
            }
            match self.store.get(&task_id).await {
                Ok(Some(task)) => {
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
            run.updated_at = Utc::now();
            self.workflows.put_run(&run).await?;
            self.bus.publish(Event::WorkflowRunUpdated { run: run.clone() });
            // Idempotent, and a restart's repair of one that never got done.
            // Outside the lock: releasing a worktree is git work that can take
            // seconds, and every workflow report waits on this lock.
            drop(_guard);
            self.settle_unreached_tasks(&run).await;
            self.release_finished_workflow(&run).await;
            return Ok(());
        }

        // `#178`: every task node of a current run has its task from the
        // moment the run starts, waiting on the steps before it -- so the
        // board shows what is queued behind the work in progress.
        self.materialize_node_tasks(&mut run).await?;

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

        if let Some(failed) = run
            .nodes
            .iter()
            .find(|n| n.status == WorkflowNodeStatus::Failed)
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
        } else if run.nodes.iter().all(|node| {
            matches!(
                node.status,
                WorkflowNodeStatus::Done | WorkflowNodeStatus::SkippedByRoute
            )
        }) {
            run.status = WorkflowRunStatus::Done;
        }
        if run.status.is_terminal() {
            run.skip_unreached();
            run.updated_at = Utc::now();
            self.workflows.put_run(&run).await?;
            self.bus.publish(Event::WorkflowRunUpdated { run: run.clone() });
            drop(_guard);
            self.settle_unreached_tasks(&run).await;
            self.release_finished_workflow(&run).await;
            return Ok(());
        }

        let rounds_as_runs = run.rounds_as_runs;
        let eligible: Vec<String> = run
            .nodes
            .iter()
            .filter(|node| {
                matches!(node.status, WorkflowNodeStatus::Unstarted | WorkflowNodeStatus::Waiting)
                    && !(rounds_as_runs && node.launched(true))
            })
            .filter(|node| {
                run.definition
                    .nodes
                    .iter()
                    .any(|n| n.id == node.node_id && n.kind == WorkflowNodeKind::Task)
            })
            .filter(|node| {
                run.definition
                    .edges
                    .iter()
                    .filter(|edge| edge.to == node.node_id)
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
                        finished(&edge.from)
                            && match run.definition.nodes.iter().find(|n| n.id == edge.from) {
                                Some(n) if n.kind == WorkflowNodeKind::Gate => {
                                    run.nodes
                                        .iter()
                                        .find(|node| node.node_id == edge.from)
                                        .is_some_and(|node| {
                                            node.status == WorkflowNodeStatus::SkippedByRoute
                                        })
                                        || run
                                            .definition
                                            .gate_subject(&n.id)
                                            .is_some_and(|subject| done(&subject))
                                }
                                _ => true,
                            }
                    })
            })
            .filter(|node| {
                let parents: Vec<_> = run
                    .definition
                    .edges
                    .iter()
                    .filter(|edge| edge.to == node.node_id)
                    .collect();
                parents.is_empty()
                    || parents.iter().any(|edge| {
                        run.nodes
                            .iter()
                            .find(|n| n.node_id == edge.from)
                            .is_some_and(|n| n.status == WorkflowNodeStatus::Done)
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
            let (round, existing) = run
                .nodes
                .iter()
                .find(|n| n.node_id == node_id)
                .map_or((0, None), |n| (n.round, n.task_id.clone()));
            // A legacy run (`rounds_as_runs` off) still gives each round a
            // task of its own, titled for it; a current one never does.
            if round > 0 && !run.rounds_as_runs {
                template.title = format!("{} (rework {round})", template.title);
            }

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

            // `#178`: the node already has its task -- made up front and
            // waiting, or sent back for another round -- so this is that
            // task's next run. The task is put back to `pending`, its wait
            // over, with the journal saying why, before the node is
            // persisted as launched: from then on its mirror reads the run
            // that is coming, never a last round's `done`. A crash in
            // between leaves the node unlaunched, and the next advance
            // simply does this again.
            if let (Some(task_id), true) = (existing, run.rounds_as_runs) {
                if let Err(error) = self.release_node_task(&run, &node_id, &task_id, round).await {
                    let node = run.nodes.iter_mut().find(|node| node.node_id == node_id).expect("snapshot node");
                    node.status = WorkflowNodeStatus::Failed;
                    node.error = Some(error.to_string());
                    run.status = WorkflowRunStatus::Failed;
                    run.failure_node_id = Some(node_id);
                    run.error = Some(error.to_string());
                    continue;
                }
                let node = run.nodes.iter_mut().find(|node| node.node_id == node_id).expect("snapshot node");
                node.launched_round = Some(round);
                node.status = WorkflowNodeStatus::Pending;
                node.error = None;
                run.updated_at = Utc::now();
                self.workflows.put_run(&run).await?;
                self.bus.publish(Event::WorkflowRunUpdated { run: run.clone() });
                to_start.push(task_id);
                continue;
            }

            let task_id = uuid::Uuid::new_v4().to_string();
            let node_run = run
                .nodes
                .iter_mut()
                .find(|node| node.node_id == node_id)
                .expect("snapshot node");
            node_run.task_id = Some(task_id.clone());
            node_run.launched_round = Some(round);
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
                .create_workflow_task(template, origin, task_id.clone(), None)
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
            run.skip_unreached();
        } else if run.rounds_as_runs {
            // `#178`: a node with its task that is still not its turn --
            // waiting since the run started, or sent back for a round
            // behind another step's -- has that task read so: pending,
            // waiting on the steps before it.
            for node_id in run
                .nodes
                .iter()
                .filter(|n| !n.launched(true) && n.task_id.is_some())
                .filter(|n| matches!(n.status, WorkflowNodeStatus::Unstarted | WorkflowNodeStatus::Waiting))
                .map(|n| n.node_id.clone())
                .collect::<Vec<_>>()
            {
                self.hold_node_task(&run, &node_id).await;
                if let Some(node) = run.nodes.iter_mut().find(|n| n.node_id == node_id) {
                    node.status = WorkflowNodeStatus::Waiting;
                }
            }
        }
        run.updated_at = Utc::now();
        self.workflows.put_run(&run).await?;
        self.bus
            .publish(Event::WorkflowRunUpdated { run: run.clone() });
        drop(_guard);
        if run.status.is_terminal() {
            self.settle_unreached_tasks(&run).await;
            self.release_finished_workflow(&run).await;
        }
        for task_id in to_start {
            let engine = self.clone();
            tokio::spawn(async move {
                engine.launch_node_task(&task_id).await;
            });
        }
        Ok(())
    }

    /// Hand a launched node's task its run (`#178`). A task that has run
    /// before is on a rework round, and its newest run -- the round before --
    /// is what this one continues: same worktree and same conversation by
    /// default, `resolve_continue` falling back to fresh (journaled) when it
    /// cannot. A task that never ran just starts.
    async fn launch_node_task(self: &Arc<Self>, task_id: &str) {
        let previous = match self.store.runs(task_id, 1).await {
            Ok(runs) => runs.into_iter().next().filter(|r| r.status.is_terminal()),
            Err(_) => None,
        };
        match previous {
            Some(prev) => {
                self.start_run_due_continue(task_id, Trigger::Workflow, crate::engine::Due::now(), prev).await
            }
            None => self.start_run(task_id, Trigger::Workflow).await,
        }
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
            Some(id) => self.store.get(id).await?,
            None => None,
        };
        let mut selected = None;

        for (index, exit) in exits.iter().enumerate() {
            let holds = if let Some(command) = &exit.check {
                let dir = if let Some(task) = &task {
                    let latest = self.store.runs(&task.id, 1).await?.into_iter().next();
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
                    .store
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
                    .store
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
        let gates: Vec<(String, String, String)> = run
            .definition
            .nodes
            .iter()
            .filter(|n| n.kind == WorkflowNodeKind::Gate)
            .filter_map(|n| {
                let subject = run.definition.gate_subject(&n.id)?;
                Some((n.id.clone(), subject, n.gate.as_ref()?.step.clone()))
            })
            .collect();
        for (gate_id, subject, step) in gates {
            let Some(subject_node) = run.nodes.iter().find(|n| n.node_id == subject) else {
                continue;
            };
            let Some(task_id) = subject_node.task_id.clone() else {
                continue;
            };
            let subject_round = subject_node.round;
            // `#178`: a subject sent back for another round has only its
            // last round's run to show until the next one exists -- a gate
            // never reads that as this round's verdict.
            let launched = subject_node.launched(run.rounds_as_runs);
            let Ok(Some(subject_run)) = self.store.runs(&task_id, 1).await.map(|r| r.into_iter().next()) else {
                continue;
            };
            if run.rounds_as_runs && (!launched || subject_run.round < subject_round) {
                continue;
            }
            let (status, error) = match subject_run.status {
                RunStatus::Done => (WorkflowNodeStatus::Done, None),
                RunStatus::Verifying => (WorkflowNodeStatus::Verifying, None),
                RunStatus::Failed | RunStatus::Cancelled => (WorkflowNodeStatus::Skipped, None),
                RunStatus::Blocked
                    if subject_run.blocked_source == Some(factory_core::run::BlockSource::Verification) =>
                {
                    let since = subject_run.blocked_since.unwrap_or(subject_run.started_at);
                    let newest = self
                        .policies
                        .step_attestations(&subject_run.id)
                        .await
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|a| a.step == step && a.at <= since)
                        .max_by_key(|a| a.at);
                    match newest {
                        Some(a) if a.verdict == AttestationVerdict::Pass => (WorkflowNodeStatus::Done, None),
                        Some(a) => (
                            WorkflowNodeStatus::Blocked,
                            Some(format!(
                                "{step} failed ({})",
                                a.exit_code.map(|c| format!("exit {c}")).unwrap_or_else(|| "did not finish".into())
                            )),
                        ),
                        None => (WorkflowNodeStatus::Unstarted, None),
                    }
                }
                _ => (WorkflowNodeStatus::Unstarted, None),
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

    /// A node's task template as this run spawns it: the snapshot's task,
    /// carrying the category it was planned as so its own record says what
    /// its run was held to.
    fn node_template(run: &WorkflowRun, node_id: &str) -> Option<factory_core::task::NewTask> {
        let node = run.definition.nodes.iter().find(|n| n.id == node_id)?;
        let mut template = node.task.clone();
        template.category = Some(run.definition.node_category(node));
        Some(template)
    }

    /// `#178`: give every task node of a current run that has none yet its
    /// task, up front -- a node with steps before it waits on them
    /// (`Task::after`), a root starts in this same pass. Each is authorized
    /// exactly as a spawn always was, and every id is persisted before any
    /// task is created, so a restart fills in exactly those ids. A denial or
    /// a creation failure fails that node and the run, the same as it did
    /// when the node was reached.
    async fn materialize_node_tasks(&self, run: &mut WorkflowRun) -> Result<()> {
        if !run.rounds_as_runs || run.status.is_terminal() {
            return Ok(());
        }
        let missing: Vec<String> = run
            .nodes
            .iter()
            .filter(|n| n.task_id.is_none() && n.status == WorkflowNodeStatus::Unstarted)
            .filter(|n| run.definition.nodes.iter().any(|d| d.id == n.node_id && d.kind == WorkflowNodeKind::Task))
            .map(|n| n.node_id.clone())
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        let caller = self.caller_for_actor(&run.started_by).await;
        let mut decided = Vec::new();
        for node_id in missing {
            let Some(template) = Self::node_template(run, &node_id) else { continue };
            let node = run.nodes.iter_mut().find(|n| n.node_id == node_id).expect("snapshot node");
            match self.authorize_workflow_spawn(&caller, &template).await {
                Ok(()) => {
                    node.task_id = Some(uuid::Uuid::new_v4().to_string());
                    decided.push(node_id);
                }
                Err(denial) => {
                    node.status = WorkflowNodeStatus::Failed;
                    node.error = Some(denial.to_string());
                    run.status = WorkflowRunStatus::Failed;
                    run.failure_node_id = Some(node_id);
                    run.error = Some(denial.to_string());
                    break;
                }
            }
        }
        // Persist the decisions before creating or publishing any task. A
        // restart can fill in these exact ids; it must never choose others.
        run.updated_at = Utc::now();
        self.workflows.put_run(run).await?;
        self.bus.publish(Event::WorkflowRunUpdated { run: run.clone() });
        for node_id in decided {
            if run.status.is_terminal() {
                break;
            }
            let Some(template) = Self::node_template(run, &node_id) else { continue };
            let after = run.after_for(&node_id);
            let task_id = run.nodes.iter().find(|n| n.node_id == node_id).and_then(|n| n.task_id.clone()).expect("decided");
            let origin = WorkflowOrigin {
                workflow_id: run.workflow_id.clone(),
                workflow_run_id: run.id.clone(),
                node_id: node_id.clone(),
            };
            let waiting = after.is_some();
            let result = self.create_workflow_task(template, origin, task_id.clone(), after.clone()).await;
            let node = run.nodes.iter_mut().find(|n| n.node_id == node_id).expect("snapshot node");
            match result {
                Ok(_) => {
                    if waiting {
                        node.status = WorkflowNodeStatus::Waiting;
                        if let Some(after) = &after {
                            self.entry(
                                &task_id,
                                TaskEntry::new("daemon", "waiting", format!("waiting on {}", after.titles()))
                                    .with_data(serde_json::json!({ "after": after })),
                            )
                            .await;
                        }
                    }
                }
                Err(error) => {
                    // No task exists at this id and none ever will: leaving
                    // it set would make recovery try to create it again.
                    node.task_id = None;
                    node.status = WorkflowNodeStatus::Failed;
                    node.error = Some(error.to_string());
                    run.status = WorkflowRunStatus::Failed;
                    run.failure_node_id = Some(node_id);
                    run.error = Some(error.to_string());
                }
            }
        }
        Ok(())
    }

    /// `#178`: a node's task, its turn come -- the steps it waited on have
    /// finished, or its work was sent back for another round. Pending, the
    /// wait over, and the journal says which. The run itself is asked for
    /// by the caller once the node's launch is persisted.
    async fn release_node_task(&self, run: &WorkflowRun, node_id: &str, task_id: &str, round: u32) -> Result<()> {
        let request = run.nodes.iter().find(|n| n.node_id == node_id).and_then(|n| n.rework_request.clone());
        let task = self.require(task_id).await?;
        let was = task.status;
        self.store
            .update(task_id, &TaskPatch { status: Some(TaskStatus::Pending), clear_after: true, ..Default::default() })
            .await?;
        let words = match (&request, round) {
            (_, 0) => task.after.as_ref().map(|after| format!("{} finished: its turn", after.titles())),
            (Some(r), _) => Some(format!(
                "rework round {round} of {}: {} sent the work back, so this task runs again (it was {})",
                r.max_rounds,
                r.from_node,
                was.as_str()
            )),
            (None, _) => Some(format!(
                "rework round {round}: work was sent back through this step, so this task runs again (it was {})",
                was.as_str()
            )),
        };
        if let Some(words) = words {
            let kind = if round > 0 { "round" } else { "released" };
            self.entry(
                task_id,
                TaskEntry::new("daemon", kind, words).with_data(serde_json::json!({
                    "round": round,
                    "max_rounds": request.as_ref().map(|r| r.max_rounds),
                    "sent_back_by": request.as_ref().map(|r| r.from_node.clone()),
                    "workflow_run": run.id,
                    "was": was.as_str(),
                })),
            )
            .await;
        }
        self.publish_task(task_id).await;
        Ok(())
    }

    /// `#178`: a node's task that is not its turn reads as waiting on the
    /// steps before it -- written only when it does not already, so an
    /// advance that changes nothing writes nothing. A task sent back behind
    /// another step (a review waiting for the implementation's next round)
    /// is how a `done` task comes to wait again; the journal says so.
    async fn hold_node_task(&self, run: &WorkflowRun, node_id: &str) {
        let Some(task_id) = run.nodes.iter().find(|n| n.node_id == node_id).and_then(|n| n.task_id.clone()) else {
            return;
        };
        let Some(after) = run.after_for(node_id) else { return };
        let Ok(Some(task)) = self.store.get(&task_id).await else { return };
        if task.status == TaskStatus::Pending && task.after.as_ref() == Some(&after) {
            return;
        }
        if task.status.is_terminal() && task.closure.is_some() {
            return; // closed on purpose; not the engine's to put back
        }
        if self.store.active_run(&task_id).await.ok().flatten().is_some() {
            return;
        }
        if self
            .store
            .update(&task_id, &TaskPatch { status: Some(TaskStatus::Pending), after: Some(after.clone()), ..Default::default() })
            .await
            .is_err()
        {
            return;
        }
        let round = run.nodes.iter().find(|n| n.node_id == node_id).map_or(0, |n| n.round);
        let words = if task.status == TaskStatus::Pending {
            format!("waiting on {}", after.titles())
        } else {
            format!("rework round {round}: runs again once {} finishes (it was {})", after.titles(), task.status.as_str())
        };
        self.entry(&task_id, TaskEntry::new("daemon", "waiting", words).with_data(serde_json::json!({ "after": after, "round": round })))
            .await;
        self.publish_task(&task_id).await;
    }

    /// `#178`: the run is over, and some of its nodes' tasks never got their
    /// turn -- a branch the route did not take, or steps the run ended
    /// before. None of them lingers as pending. One that never ran is
    /// closed as not planned, saying why; one that ran in an earlier round
    /// keeps its last run's outcome. Idempotent: a closed task, or one
    /// whose node was launched, is left alone.
    async fn settle_unreached_tasks(&self, run: &WorkflowRun) {
        if !run.rounds_as_runs {
            return;
        }
        for node in run.nodes.iter().filter(|n| !n.launched(true)) {
            let Some(task_id) = &node.task_id else { continue };
            let Ok(Some(task)) = self.store.get(task_id).await else { continue };
            if task.close_reason().is_some() || task.status != TaskStatus::Pending {
                continue;
            }
            if self.store.active_run(task_id).await.ok().flatten().is_some() {
                continue;
            }
            let why = match (&node.status, &node.skip_reason) {
                (WorkflowNodeStatus::SkippedByRoute, Some(reason)) => format!("the route took another branch: {reason}"),
                (WorkflowNodeStatus::SkippedByRoute, None) => "the route took another branch".to_string(),
                _ => format!("the workflow run ended ({}) before this step's turn", serde_json::to_value(run.status).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()),
            };
            if task.runs == 0 {
                let asked = crate::operations::Asked { by: "the workflow".into(), source: "daemon", reason: Some(why) };
                if let Err(error) = self.close_task(task_id, factory_core::task::CloseReason::NotPlanned, None, &asked).await {
                    tracing::warn!(workflow_run = run.id, task = task_id, "could not close an unreached workflow task: {error}");
                }
            } else {
                let last = self.store.runs(task_id, 1).await.ok().and_then(|r| r.into_iter().next());
                let Some(last) = last else { continue };
                let _ = self
                    .store
                    .update(task_id, &TaskPatch { status: Some(last.status.as_task_status()), clear_after: true, ..Default::default() })
                    .await;
                self.entry(
                    task_id,
                    TaskEntry::new("daemon", "round_skipped", format!("{why}; the task keeps attempt {}'s outcome", last.attempt)),
                )
                .await;
                self.publish_task(task_id).await;
            }
        }
    }

    /// `#178`: a person (or an agent) starting a waiting node's task by
    /// hand, ahead of what it waits on. The node is marked launched for its
    /// round and the task's wait cleared, under the same lock every advance
    /// takes, so the workflow follows that run from here and never starts
    /// the step a second time when its upstream does finish.
    pub(crate) async fn claim_waiting_node(&self, task: &Task) -> Result<()> {
        let _guard = self.workflow_edit.lock().await;
        if let Some(origin) = &task.workflow_origin {
            let mut run = self.workflow_run(&origin.workflow_run_id).await?;
            if run.status.is_terminal() {
                return Err(FactoryError::BadRequest(
                    "this task's workflow run has already ended; it will not run as part of it".into(),
                ));
            }
            if let Some(node) = run
                .nodes
                .iter_mut()
                .find(|n| n.node_id == origin.node_id && n.task_id.as_deref() == Some(task.id.as_str()))
            {
                node.launched_round = Some(node.round);
                node.status = WorkflowNodeStatus::Pending;
                run.updated_at = Utc::now();
                self.workflows.put_run(&run).await?;
                self.bus.publish(Event::WorkflowRunUpdated { run });
            }
        }
        self.store.update(&task.id, &TaskPatch { clear_after: true, ..Default::default() }).await?;
        Ok(())
    }

    /// `#178`: a workflow run's worktrees go once it is *completely*
    /// finished -- the run itself terminal, and every task it ever made
    /// settled for good: done, or closed. A node blocked on a failure is not:
    /// its task waits for a person, who may yet `--continue` it in exactly
    /// the worktree this would take away, so the run's worktrees wait with
    /// it until that task is closed (closing it comes back here). Every task
    /// of the run is released together, rounds and legacy rework tasks
    /// included. Idempotent, and cheap on a run already released.
    pub(crate) async fn release_finished_workflow(&self, run: &WorkflowRun) {
        if !run.status.is_terminal() {
            return;
        }
        let task_ids: Vec<&String> = run
            .nodes
            .iter()
            .flat_map(|n| n.task_id.iter().chain(n.superseded_task_ids.iter()))
            .collect();
        let mut settled = Vec::new();
        for id in task_ids {
            let Ok(Some(task)) = self.store.get(id).await else { continue };
            if !task.status.is_terminal() || self.store.active_run(id).await.ok().flatten().is_some() {
                return;
            }
            settled.push(task);
        }
        let why = format!("workflow run {} is finished", run.id);
        for task in settled {
            self.release_task_worktrees(&task, &why).await;
        }
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
        let Some(origin) = task.workflow_origin.clone() else {
            return;
        };
        let _guard = self.workflow_edit.lock().await;
        let Ok(mut run) = self.workflow_run(&origin.workflow_run_id).await else {
            return;
        };
        // A task an earlier rework round superseded speaks for nobody: the
        // node has moved on to a newer one (`#140`). Nor does one whose node
        // was sent back for a round its task has not been handed yet
        // (`#178`): what it says is the last round's.
        let rounds_as_runs = run.rounds_as_runs;
        if run.nodes.iter().all(|node| {
            node.node_id != origin.node_id
                || node.task_id.as_deref() != Some(task_id)
                || !node.launched(rounds_as_runs)
        }) {
            return;
        }
        let Some(node) = run
            .nodes
            .iter_mut()
            .find(|node| node.node_id == origin.node_id)
        else {
            return;
        };
        node.status = node_status(&task);
        node.error = task.error;
        if !run.status.is_terminal() && node.status == WorkflowNodeStatus::Failed {
            run.status = WorkflowRunStatus::Failed;
            run.failure_node_id = Some(node.node_id.clone());
            run.error = node
                .error
                .clone()
                .or_else(|| Some("task dispatch failed".into()));
            run.skip_unreached();
        }
        run.updated_at = Utc::now();
        if self.workflows.put_run(&run).await.is_ok() {
            self.bus.publish(Event::WorkflowRunUpdated { run: run.clone() });
        }
        if run.status.is_terminal() {
            drop(_guard);
            self.settle_unreached_tasks(&run).await;
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
                // A node made up front and not yet launched is recreated
                // waiting, exactly as it was decided (`#178`).
                let after = (run.rounds_as_runs && !node.launched(true))
                    .then(|| run.after_for(&node.node_id))
                    .flatten();
                if let Err(error) = self
                    .create_workflow_task(template, origin, task_id.clone(), after)
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
                    if !node.launched(current.rounds_as_runs) {
                        continue;
                    }
                    let Some(task_id) = node.task_id.clone() else {
                        continue;
                    };
                    if let Ok(Some(task)) = self.store.get(&task_id).await {
                        // `#178`: a round is a run of a task that already has
                        // some, so "never ran" means "has no run for the
                        // round its node was launched for".
                        let newest_round = match self.store.runs(&task_id, 1).await {
                            Ok(runs) => runs.first().map(|r| r.round),
                            Err(_) => continue,
                        };
                        let round_unrun = if current.rounds_as_runs {
                            newest_round.is_none_or(|r| r < node.round)
                        } else {
                            task.runs == 0
                        };
                        if task.status == TaskStatus::Pending
                            && round_unrun
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
                                engine.launch_node_task(&task_id).await;
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
        match self.workflows.recent_terminal_runs(200).await {
            Ok(terminal_runs) => {
                for run in terminal_runs {
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
