//! Standing agents: starting them, keeping them, and letting a person type at
//! them.
//!
//! A task agent lives for one run and is failed if it goes quiet. A standing
//! agent is quiet by design -- it is only ever checked for whether its session
//! is still there.

use chrono::Utc;
use factory_core::adapter::agent::AgentContext;
use factory_core::adapter::runtime::{
    RuntimeEvent, RuntimeEventKind, RuntimeEventStream, RuntimeStatus, Screen, StartRequest,
};
use factory_core::agent::{AgentSession, AgentState, Lifetime};
use factory_core::role::Role;
use factory_core::config::ScopeAgent;
use factory_core::error::{FactoryError, Result};
use factory_core::event::Event;
use factory_core::task::SessionRef;
use std::sync::Arc;

use crate::engine::Engine;

/// herdr and anything like it want a name without spaces in it.
fn runtime_name(scope: &str, name: &str) -> String {
    let clean = |s: &str| {
        s.chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
            .collect::<String>()
    };
    format!("factory-{}-{}", clean(scope), clean(name))
}

impl Engine {
    fn declared(&self, scope: &str, name: &str) -> Result<ScopeAgent> {
        self.factory
            .scope(scope)?
            .agents_with(&self.factory.config.daemon.foreman)
            .into_iter()
            .find(|a| a.name() == name)
            .ok_or_else(|| {
                FactoryError::BadRequest(format!("scope {scope:?} declares no agent named {name:?}"))
            })
    }

    fn runtime_for(&self, scope: &str) -> String {
        self.factory
            .scope(scope)
            .ok()
            .and_then(|s| s.runtime.clone())
            .unwrap_or_else(|| self.factory.config.daemon.default_runtime.clone())
    }

    /// Bring a declared standing agent up. Idempotent: an agent already live is
    /// returned as it is rather than started twice.
    pub async fn start_agent(&self, scope: &str, name: &str) -> Result<AgentSession> {
        let decl = self.declared(scope, name)?;
        if !decl.lifetime.is_standing() {
            return Err(FactoryError::BadRequest(format!(
                "{name:?} in {scope:?} is a task agent; start a task with it instead"
            )));
        }

        let id = AgentSession::id_for(scope, name);
        let existing = self.store.get_agent(&id).await?;
        if let Some(existing) = &existing {
            if existing.state.is_live() && self.agent_alive(existing).await {
                return Ok(existing.clone());
            }
        }

        let runtime_name_ = self.runtime_for(scope);
        let adapter = self.registry.agent(&decl.harness)?;
        let runtime = self.registry.runtime(&runtime_name_)?;
        let cwd = self.factory.scope_path(scope)?;
        if !cwd.is_dir() {
            return Err(FactoryError::BadRequest(format!(
                "scope {scope:?} points at {}, which is not a directory",
                cwd.display()
            )));
        }

        let mut agent = AgentSession::new(
            scope,
            name,
            &decl.harness,
            &runtime_name_,
            decl.lifetime,
            decl.role.clone(),
        );
        // A role somebody gave this agent outlives the session it was given in.
        agent.assigned_role = existing.and_then(|a| a.assigned_role);
        agent.role = agent.role_with(&decl.role);
        // A fresh token each time it comes up: an old session's token must not
        // still speak for the agent that replaced it.
        let identity = factory_core::new_token();
        agent.token = Some(identity.clone());
        agent.state = AgentState::Starting;
        agent.started_at = Utc::now();
        agent.error = None;
        self.store.put_agent(&agent).await?;
        self.bus.publish(Event::AgentUpdated {
            agent: agent.clone(),
        });

        // No task: a standing agent is started to be there, and is told nothing.
        let ctx = AgentContext {
            scope: scope.to_string(),
            cwd: cwd.clone(),
            factory_bin: self.factory_bin.clone(),
            socket: self.factory.socket_path(),
            task: None,
            identity_token: Some(identity),
        };

        let launch = match adapter.launch_spec(&ctx).await {
            Ok(l) => l,
            Err(e) => return self.mark_agent_failed(agent, e).await,
        };

        let session = match runtime
            .start(&StartRequest {
                id: agent.id.clone(),
                name: runtime_name(scope, name),
                label: format!("factory: {scope}/{name}"),
                cwd,
                launch,
            })
            .await
        {
            Ok(s) => s,
            Err(e) => return self.mark_agent_failed(agent, e).await,
        };

        agent.attach = runtime.attach_command(&session);
        agent.session = Some(session);
        agent.state = AgentState::Ready;
        agent.last_seen_at = Utc::now();
        self.store.put_agent(&agent).await?;
        self.bus.publish(Event::AgentUpdated {
            agent: agent.clone(),
        });
        tracing::info!(agent = %agent.id, "standing agent is up");
        Ok(agent)
    }

    async fn mark_agent_failed(
        &self,
        mut agent: AgentSession,
        e: FactoryError,
    ) -> Result<AgentSession> {
        agent.state = AgentState::Gone;
        agent.error = Some(e.to_string());
        agent.session = None;
        let _ = self.store.put_agent(&agent).await;
        self.bus.publish(Event::AgentUpdated { agent });
        Err(e)
    }

    /// Stop a standing agent and leave it stopped. Nothing restarts it until
    /// somebody asks, not even a permanent one -- otherwise the button would
    /// do nothing anybody could see.
    pub async fn stop_agent(&self, id: &str) -> Result<AgentSession> {
        let mut agent = self.require_agent(id).await?;
        if let Some(session) = &agent.session {
            if let Ok(runtime) = self.registry.runtime(&session.runtime) {
                let _ = runtime.stop(session).await;
            }
        }
        agent.session = None;
        agent.attach = None;
        agent.token = None;
        agent.state = AgentState::Stopped;
        agent.last_seen_at = Utc::now();
        self.store.put_agent(&agent).await?;
        self.bus.publish(Event::AgentUpdated {
            agent: agent.clone(),
        });
        self.record_gone(&agent.id, &agent.scope, &agent.name).await;
        tracing::info!(agent = %agent.id, "standing agent stopped");
        Ok(agent)
    }

    /// Give a standing agent a role, or take the given one away and let the
    /// config decide again.
    ///
    /// The session it is already in keeps running: a role is checked when the
    /// agent asks for something, not when it starts, so the new one holds from
    /// its next request. Only a person does this -- an agent that could hand
    /// itself a role would not be bounded by the one it has.
    ///
    /// A standing agent, because that is what has a row to remember it in. A
    /// task agent is whatever its scope declares for the length of one run, so
    /// its role is a question for the config.
    pub async fn set_agent_role(&self, id: &str, role: Option<Role>) -> Result<AgentSession> {
        let mut agent = self.require_agent(id).await?;
        if let Some(role) = &role {
            if !self.roles.contains(role) {
                return Err(FactoryError::BadRequest(format!(
                    "no role named {:?}. This instance has: {}",
                    role.as_str(),
                    self.roles.names().join(", ")
                )));
            }
        }
        let declared = self.role_of(&agent.scope, &agent.name);
        agent.assigned_role = role;
        agent.role = agent.role_with(&declared);
        agent.last_seen_at = Utc::now();
        self.store.put_agent(&agent).await?;
        self.bus.publish(Event::AgentUpdated {
            agent: agent.clone(),
        });
        tracing::info!(agent = %agent.id, role = %agent.role, "role set");
        Ok(agent)
    }

    /// Type at an agent. This is how a person answers the prompt an agent is
    /// sitting on -- a trust dialog, a permission question -- without leaving
    /// the page.
    pub async fn agent_input(&self, id: &str, text: Option<&str>, keys: &[String]) -> Result<()> {
        let agent = self.require_agent(id).await?;
        let session = agent.session.as_ref().ok_or_else(|| {
            FactoryError::BadRequest(format!("{id} has no session to type into"))
        })?;
        let runtime = self.registry.runtime(&session.runtime)?;
        if let Some(text) = text.filter(|t| !t.is_empty()) {
            runtime.send_text(session, text).await?;
        }
        if !keys.is_empty() {
            runtime.send_keys(session, keys).await?;
        }
        Ok(())
    }

    /// The same for a task run's session, so the modal's terminal can be typed
    /// into as well.
    pub async fn run_input(&self, run_id: &str, text: Option<&str>, keys: &[String]) -> Result<()> {
        let run = self
            .store
            .get_run(run_id)
            .await?
            .ok_or_else(|| FactoryError::TaskNotFound(format!("run {run_id}")))?;
        let session = run.session.as_ref().ok_or_else(|| {
            FactoryError::BadRequest(format!("run {run_id} has no session to type into"))
        })?;
        let runtime = self.registry.runtime(&session.runtime)?;
        if let Some(text) = text.filter(|t| !t.is_empty()) {
            runtime.send_text(session, text).await?;
        }
        if !keys.is_empty() {
            runtime.send_keys(session, keys).await?;
        }
        Ok(())
    }

    pub async fn agent_output(&self, id: &str, lines: u32) -> Result<String> {
        let agent = self.require_agent(id).await?;
        let Some(session) = &agent.session else {
            return Ok(String::new());
        };
        let runtime = self.registry.runtime(&session.runtime)?;
        Ok(runtime.read(session, lines).await.unwrap_or_default())
    }

    /// One frame of a standing agent's screen. A runtime that cannot render one
    /// says so, and the caller shows the transcript instead.
    pub async fn agent_screen(&self, id: &str) -> Result<Option<Screen>> {
        let agent = self.require_agent(id).await?;
        let Some(session) = &agent.session else {
            return Ok(None);
        };
        let runtime = self.registry.runtime(&session.runtime)?;
        runtime.screen(session).await
    }

    pub async fn require_agent(&self, id: &str) -> Result<AgentSession> {
        self.store
            .get_agent(id)
            .await?
            .ok_or_else(|| FactoryError::TaskNotFound(format!("agent {id}")))
    }

    /// Is the session still there -- and, on the way past, write down what it
    /// was doing. The supervisor asks the runtime once per tick anyway, and
    /// that answer is the only record of liveness anyone will ever have.
    async fn agent_alive(&self, agent: &AgentSession) -> bool {
        let Some(session) = &agent.session else {
            return false;
        };
        let status = match self.registry.runtime(&session.runtime) {
            Ok(rt) => rt.status(session).await.unwrap_or(RuntimeStatus::Gone),
            Err(_) => RuntimeStatus::Gone,
        };
        self.record_status(&agent.id, &agent.scope, &agent.name, status)
            .await;
        status != RuntimeStatus::Gone
    }

    /// Line up what the config declares with what is actually still running,
    /// after a restart or a config change. Three different things can be true
    /// and each needs its own answer.
    pub async fn reconcile_agents(self: &Arc<Self>) {
        let stored = self.store.agents().await.unwrap_or_default();
        let mut seen = std::collections::BTreeSet::new();

        for scope in &self.factory.config.scopes {
            for decl in scope.standing_agents_with(&self.factory.config.daemon.foreman) {
                let id = AgentSession::id_for(&scope.name, &decl.name());
                seen.insert(id.clone());

                match stored.iter().find(|a| a.id == id) {
                    // Still there from before: adopt it rather than opening a
                    // second session beside the one already running.
                    Some(existing) if existing.state.is_live() && self.agent_alive(existing).await => {
                        let mut a = existing.clone();
                        a.declared = true;
                        a.lifetime = decl.lifetime;
                        a.role = a.role_with(&decl.role);
                        a.last_seen_at = Utc::now();
                        let _ = self.store.put_agent(&a).await;
                        tracing::info!(agent = %id, "adopted a standing agent that outlived the daemon");
                    }
                    // Known but not running. Start it if it is meant to start
                    // itself; a stopped one stays stopped.
                    Some(existing) => {
                        let mut a = existing.clone();
                        a.declared = true;
                        a.lifetime = decl.lifetime;
                        a.role = a.role_with(&decl.role);
                        a.session = None;
                        if a.state != AgentState::Stopped {
                            a.state = AgentState::Gone;
                        }
                        let _ = self.store.put_agent(&a).await;
                        if decl.autostart() && a.state != AgentState::Stopped {
                            self.autostart(&scope.name, &decl.name()).await;
                        }
                    }
                    // Never seen. Record it so the page can show it, and start
                    // it if it wants starting.
                    None => {
                        let a = AgentSession::new(
                            &scope.name,
                            &decl.name(),
                            &decl.harness,
                            &self.runtime_for(&scope.name),
                            decl.lifetime,
                            decl.role.clone(),
                        );
                        let _ = self.store.put_agent(&a).await;
                        if decl.autostart() {
                            self.autostart(&scope.name, &decl.name()).await;
                        }
                    }
                }
            }
        }

        // A stored agent the config no longer declares. Its session is real and
        // nobody is tracking it any more, so close it rather than leave a pane
        // for somebody to find next week.
        for agent in stored {
            if seen.contains(&agent.id) {
                continue;
            }
            tracing::warn!(agent = %agent.id, "no longer declared; closing its session");
            if let Some(session) = &agent.session {
                if let Ok(rt) = self.registry.runtime(&session.runtime) {
                    let _ = rt.stop(session).await;
                }
            }
            let _ = self.store.delete_agent(&agent.id).await;
            self.bus.publish(Event::AgentRemoved { id: agent.id });
        }
    }

    async fn autostart(self: &Arc<Self>, scope: &str, name: &str) {
        if let Err(e) = self.start_agent(scope, name).await {
            tracing::warn!(scope, name, "could not start standing agent: {e}");
        }
    }

    /// The standing-agent watchdog. Deliberately not the run watchdog: a
    /// permanent agent that has said nothing for an hour is doing its job.
    pub async fn supervise_agents(self: &Arc<Self>) {
        let agents = self.store.agents().await.unwrap_or_default();
        for agent in agents {
            if !agent.declared || agent.state == AgentState::Stopped {
                continue;
            }
            if agent.state.is_live() {
                if self.agent_alive(&agent).await {
                    let mut a = agent;
                    a.last_seen_at = Utc::now();
                    a.state = AgentState::Ready;
                    let _ = self.store.put_agent(&a).await;
                    continue;
                }
                let mut a = agent.clone();
                a.state = AgentState::Gone;
                a.session = None;
                a.attach = None;
                let _ = self.store.put_agent(&a).await;
                self.bus.publish(Event::AgentUpdated { agent: a });
                tracing::warn!(agent = %agent.id, "standing agent's session is gone");
            }

            // Gone and permanent means bring it back.
            if agent.lifetime == Lifetime::Permanent {
                let declared = self
                    .declared(&agent.scope, &agent.name)
                    .ok()
                    .filter(|d| d.autostart());
                if declared.is_some() {
                    tracing::info!(agent = %agent.id, "restarting permanent agent");
                    self.autostart(&agent.scope, &agent.name).await;
                }
            }
        }
    }

    /// Ask every registered runtime whether it can push, and listen if so.
    /// One task per runtime that says yes; a runtime that answers `None`
    /// changes nothing here -- the poll in `scheduler.rs` already covers it,
    /// exactly as it did before this existed.
    pub async fn watch_runtimes(self: &Arc<Self>) {
        for runtime in self.registry.runtimes() {
            let name = runtime.name().to_string();
            match runtime.watch().await {
                Ok(Some(stream)) => {
                    tracing::info!(runtime = %name, "listening for pushed status changes");
                    let engine = self.clone();
                    tokio::spawn(async move { engine.pump_runtime_events(stream).await });
                }
                Ok(None) => {}
                Err(e) => tracing::warn!(runtime = %name, "could not start watching: {e}"),
            }
        }
    }

    async fn pump_runtime_events(self: Arc<Self>, mut stream: RuntimeEventStream) {
        while let Some(event) = stream.recv().await {
            self.on_runtime_event(event).await;
        }
        // The channel closed: the runtime stopped pushing. Nothing to do --
        // the poll never depended on this in the first place.
    }

    /// A push from a runtime, mapped onto whichever standing agent or run's
    /// session it was about, and set loose on the bus. This must never move
    /// a task or a run -- only the agent's own `factory task report` may do
    /// that -- so all this does is feed the occupancy record and publish an
    /// event; nothing here touches `self.store.update` or a run's status.
    async fn on_runtime_event(&self, event: RuntimeEvent) {
        let Some((subject, scope, agent)) = self.subject_for_session(&event.session).await else {
            // A push about a session Factory has no record of -- already
            // stopped, or never one of ours. Nothing to attach it to.
            return;
        };
        let status = match event.kind {
            RuntimeEventKind::StatusChanged(status) => status,
            RuntimeEventKind::SessionGone => RuntimeStatus::Gone,
            // Not read yet: output is a second pass, coalesced, once there is
            // something to judge the volume against.
            RuntimeEventKind::OutputMoved => return,
        };

        // The occupancy chart's own record. This skips a sample when nothing
        // changed, which is right for a chart and wrong for what follows.
        self.record_status(&subject, &scope, &agent, status).await;

        // Published separately, on the event itself: an agent that is
        // already `working` and keeps working is exactly the case this
        // exists for, and `record_status` above deliberately produces
        // nothing for it.
        self.bus.publish(Event::AgentActivity {
            subject,
            scope,
            agent,
            status,
            at: Utc::now(),
        });
    }

    /// Which standing agent or run currently holds this session, if either
    /// does. `(subject, scope, agent name)` -- a standing agent's own id, or
    /// `run:<id>` for a task's session.
    async fn subject_for_session(&self, session: &SessionRef) -> Option<(String, String, String)> {
        if let Ok(agents) = self.store.agents().await {
            if let Some(agent) = agents.into_iter().find(|a| a.session.as_ref() == Some(session)) {
                return Some((agent.id, agent.scope, agent.name));
            }
        }
        if let Ok(runs) = self.store.active_runs().await {
            if let Some(run) = runs.into_iter().find(|r| r.session.as_ref() == Some(session)) {
                if let Ok(Some(task)) = self.store.get(&run.task_id).await {
                    return Some((format!("run:{}", run.id), task.scope, run.agent));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    //! A live herdr subscription cannot be asserted here, so these exercise
    //! `on_runtime_event` and `watch_runtimes` against a stub runtime that
    //! pushes events on demand -- the daemon-side half of this issue. The
    //! CLI-driven wait loop in `runtime_herdr.rs` itself is what the ticket
    //! asks be checked by hand against a real herdr instance instead.

    use super::*;
    use async_trait::async_trait;
    use factory_core::adapter::runtime::AgentRuntime;
    use factory_core::config::{Config, DaemonConfig, Factory, Instance};
    use factory_core::run::{NewRun, RunPatch, RunStatus, Trigger};
    use factory_core::task::{Task, TaskStatus};
    use factory_plugins::registry::Registry;
    use factory_plugins::SqliteStore;
    use std::path::PathBuf;
    use std::time::Duration;
    use tokio::sync::mpsc;

    fn engine() -> Arc<Engine> {
        engine_with(Registry::with_builtins())
    }

    fn engine_with(registry: Registry) -> Arc<Engine> {
        let config = Config {
            version: 1,
            instance: Instance {
                id: "i".into(),
                name: "test".into(),
            },
            daemon: DaemonConfig::default(),
            scopes: vec![serde_yaml_ng::from_str("name: demo\npath: .\n").unwrap()],
            roles: Default::default(),
            plugins_dir: None,
        };
        let factory = Factory {
            root: PathBuf::from("/tmp/factory-agents-test"),
            config,
        };
        Arc::new(Engine::new(
            factory,
            registry,
            Arc::new(SqliteStore::in_memory().unwrap()),
            PathBuf::from("factory"),
            vec![],
        ))
    }

    fn session(handle: &str) -> SessionRef {
        SessionRef {
            runtime: "herdr".into(),
            handle: handle.into(),
            meta: Default::default(),
        }
    }

    async fn standing_agent(engine: &Engine, scope: &str, name: &str, sess: SessionRef) -> AgentSession {
        let mut agent = AgentSession::new(scope, name, "pi", "herdr", Lifetime::Permanent, Role::worker());
        agent.state = AgentState::Ready;
        agent.session = Some(sess);
        engine.store.put_agent(&agent).await.unwrap();
        agent
    }

    /// A task with one active run, its session set to `sess`.
    async fn task_and_run(engine: &Engine, scope: &str, agent_name: &str, sess: SessionRef) -> (Task, String) {
        let now = Utc::now();
        let task = Task {
            id: "t1".into(),
            title: "t".into(),
            instructions: String::new(),
            scope: scope.into(),
            agent: agent_name.into(),
            runtime: "herdr".into(),
            status: TaskStatus::Running,
            schedule: None,
            result: None,
            error: None,
            runs: 1,
            ack_timeout_seconds: None,
            timeout_seconds: None,
            labels: Default::default(),
            created_at: now,
            updated_at: now,
            last_run_at: None,
            next_run_at: None,
        };
        let task = engine.store.create(&task).await.unwrap();
        let run = engine
            .store
            .create_run(&NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Manual,
                agent: agent_name.into(),
                adapter: "pi".into(),
                runtime: "herdr".into(),
                token: "tok".into(),
            })
            .await
            .unwrap();
        let run = engine
            .store
            .update_run(
                &run.id,
                &RunPatch {
                    status: Some(RunStatus::Running),
                    session: Some(sess),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        (task, run.id)
    }

    #[tokio::test]
    async fn a_push_maps_onto_the_standing_agent_holding_the_session() {
        let engine = engine();
        let sess = session("pane-1");
        let agent = standing_agent(&engine, "demo", "watcher", sess.clone()).await;

        let mut bus = engine.bus.subscribe();
        engine
            .on_runtime_event(RuntimeEvent {
                session: sess,
                kind: RuntimeEventKind::StatusChanged(RuntimeStatus::Working),
            })
            .await;

        let ev = bus.try_recv().expect("a bus event");
        // The whole point: activity with no task behind it.
        assert_eq!(ev.task_id(), None);
        match ev {
            Event::AgentActivity {
                subject,
                scope,
                agent: agent_name,
                status,
                ..
            } => {
                assert_eq!(subject, agent.id);
                assert_eq!(scope, "demo");
                assert_eq!(agent_name, "watcher");
                assert_eq!(status, RuntimeStatus::Working);
            }
            other => panic!("expected AgentActivity, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_push_maps_onto_the_run_holding_the_session() {
        let engine = engine();
        let sess = session("pane-2");
        let (task, run_id) = task_and_run(&engine, "demo", "pi", sess.clone()).await;

        let mut bus = engine.bus.subscribe();
        engine
            .on_runtime_event(RuntimeEvent {
                session: sess,
                kind: RuntimeEventKind::StatusChanged(RuntimeStatus::Idle),
            })
            .await;

        let ev = bus.try_recv().expect("a bus event");
        assert_eq!(ev.task_id(), None);
        match ev {
            Event::AgentActivity {
                subject,
                scope,
                agent,
                status,
                ..
            } => {
                assert_eq!(subject, format!("run:{run_id}"));
                assert_eq!(scope, task.scope);
                assert_eq!(agent, "pi");
                assert_eq!(status, RuntimeStatus::Idle);
            }
            other => panic!("expected AgentActivity, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_repeated_status_still_publishes_even_though_the_chart_record_does_not() {
        let engine = engine();
        let sess = session("pane-3");
        standing_agent(&engine, "demo", "watcher", sess.clone()).await;

        let mut bus = engine.bus.subscribe();
        for _ in 0..2 {
            engine
                .on_runtime_event(RuntimeEvent {
                    session: sess.clone(),
                    kind: RuntimeEventKind::StatusChanged(RuntimeStatus::Working),
                })
                .await;
        }

        // Published on both pushes -- an agent that is already `working` and
        // keeps working is exactly the case this exists for.
        assert!(bus.try_recv().is_ok(), "first push should publish");
        assert!(bus.try_recv().is_ok(), "second push should publish too");
        assert!(bus.try_recv().is_err(), "only two pushes happened");

        // But `record_status`, feeding the occupancy chart, keeps only the
        // one real change.
        let changes = engine
            .store
            .status_changes(Utc::now() - chrono::Duration::hours(1))
            .await
            .unwrap();
        let for_subject = changes.iter().filter(|c| c.subject == "demo/watcher").count();
        assert_eq!(for_subject, 1, "record_status should have skipped the repeat");
    }

    #[tokio::test]
    async fn a_pushed_event_never_moves_a_run_or_a_task_status() {
        let engine = engine();
        let sess = session("pane-4");
        let (task, run_id) = task_and_run(&engine, "demo", "pi", sess.clone()).await;
        // What the store actually holds once the run exists, not the literal
        // handed to `create()` -- `create_run` mirrors its own status onto
        // the task, which is the baseline a pushed event must leave alone.
        let before = engine.store.get(&task.id).await.unwrap().unwrap();

        engine
            .on_runtime_event(RuntimeEvent {
                session: sess,
                kind: RuntimeEventKind::SessionGone,
            })
            .await;

        let run_after = engine.store.get_run(&run_id).await.unwrap().unwrap();
        assert_eq!(
            run_after.status,
            RunStatus::Running,
            "a pushed event must never move a run's status"
        );
        let task_after = engine.store.get(&task.id).await.unwrap().unwrap();
        assert_eq!(
            task_after.status, before.status,
            "nor a task's -- only the agent's own report may do that"
        );
    }

    #[tokio::test]
    async fn an_event_with_no_matching_session_is_dropped_quietly() {
        let engine = engine();
        let mut bus = engine.bus.subscribe();
        engine
            .on_runtime_event(RuntimeEvent {
                session: session("nobody-holds-this"),
                kind: RuntimeEventKind::StatusChanged(RuntimeStatus::Blocked),
            })
            .await;
        assert!(bus.try_recv().is_err(), "nothing to attach the push to");
    }

    /// A runtime that pushes, standing in for herdr: `watch()` hands back a
    /// channel and stores the sending half so the test can push through it.
    struct StubRuntime {
        tx: std::sync::Mutex<Option<mpsc::Sender<RuntimeEvent>>>,
    }

    impl StubRuntime {
        fn new() -> Self {
            Self {
                tx: std::sync::Mutex::new(None),
            }
        }
    }

    #[async_trait]
    impl AgentRuntime for StubRuntime {
        fn name(&self) -> &str {
            "stub"
        }
        async fn start(&self, _req: &StartRequest) -> Result<SessionRef> {
            unimplemented!("not exercised here")
        }
        async fn submit(&self, _session: &SessionRef, _text: &str) -> Result<()> {
            Ok(())
        }
        async fn status(&self, _session: &SessionRef) -> Result<RuntimeStatus> {
            Ok(RuntimeStatus::Unknown)
        }
        async fn send_text(&self, _session: &SessionRef, _text: &str) -> Result<()> {
            Ok(())
        }
        async fn send_keys(&self, _session: &SessionRef, _keys: &[String]) -> Result<()> {
            Ok(())
        }
        async fn read(&self, _session: &SessionRef, _lines: u32) -> Result<String> {
            Ok(String::new())
        }
        async fn stop(&self, _session: &SessionRef) -> Result<()> {
            Ok(())
        }
        async fn watch(&self) -> Result<Option<RuntimeEventStream>> {
            let (tx, rx) = mpsc::channel(16);
            *self.tx.lock().unwrap() = Some(tx);
            Ok(Some(rx))
        }
    }

    #[tokio::test]
    async fn watch_runtimes_carries_a_stub_runtimes_pushes_to_the_bus() {
        let mut registry = Registry::with_builtins();
        let stub = Arc::new(StubRuntime::new());
        registry.add_runtime(stub.clone(), "test");
        let engine = engine_with(registry);

        let sess = session("stub-pane-1");
        let agent = standing_agent(&engine, "demo", "watcher", sess.clone()).await;

        let mut bus = engine.bus.subscribe();
        engine.watch_runtimes().await;

        let tx = stub
            .tx
            .lock()
            .unwrap()
            .clone()
            .expect("watch_runtimes should have called the stub's watch()");
        tx.send(RuntimeEvent {
            session: sess,
            kind: RuntimeEventKind::StatusChanged(RuntimeStatus::Blocked),
        })
        .await
        .unwrap();

        let ev = tokio::time::timeout(Duration::from_secs(1), bus.recv())
            .await
            .expect("the pushed event should reach the bus")
            .unwrap();
        match ev {
            Event::AgentActivity {
                subject, agent: a, status, ..
            } => {
                assert_eq!(subject, agent.id);
                assert_eq!(a, "watcher");
                assert_eq!(status, RuntimeStatus::Blocked);
            }
            other => panic!("expected AgentActivity, got {other:?}"),
        }
    }
}
