//! Standing agents: starting them, keeping them, and letting a person type at
//! them.
//!
//! A task agent lives for one run and is failed if it goes quiet. A standing
//! agent is quiet by design -- it is only ever checked for whether its session
//! is still there.

use chrono::Utc;
use factory_core::adapter::agent::AgentContext;
use factory_core::adapter::runtime::{RuntimeStatus, StartRequest};
use factory_core::agent::{AgentSession, AgentState, Lifetime};
use factory_core::config::ScopeAgent;
use factory_core::error::{FactoryError, Result};
use factory_core::event::Event;
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
        if let Some(existing) = self.store.get_agent(&id).await? {
            if existing.state.is_live() && self.agent_alive(&existing).await {
                return Ok(existing);
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
            decl.role,
        );
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
                        a.role = decl.role;
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
                        a.role = decl.role;
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
                            decl.role,
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
}
