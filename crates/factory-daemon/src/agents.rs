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

use crate::engine::{append_declared_args, Engine};

/// Herdr names are lowercase identifiers of at most 32 characters, starting
/// with a lowercase letter. `factory-` stays the outer namespace: herdr agent
/// names are global and `start()` adopts any existing agent carrying the name
/// it is about to ask for (see `adopt_named`), so nothing generated here may
/// collide with a name a person gave their own hand-made agent just because
/// the scope prefix was dropped.
///
/// `readable` is returned verbatim when it already fits in 32 characters and
/// is already clean (lowercase letters, digits, `-`, `_` -- nothing to clean
/// up). Otherwise it is normalized -- lowercased, any run of characters
/// outside `[a-z0-9_-]` collapsed to a single `-`, no leading, trailing or
/// doubled `-` -- and its readable part is cut at a `-` boundary rather than
/// mid-word, then a suffix is appended so two different inputs that would
/// otherwise collide once truncated still don't:
///
/// - `required_suffix` given (a task run's own id fragment): that literal
///   suffix, verbatim and never itself truncated. It is already the
///   discriminator -- two runs of the same scope and agent always carry
///   different ids -- so no further hash is needed, and stacking one on top
///   would only eat into the budget for the readable part.
/// - `required_suffix` absent (a standing agent): a stable hash of the whole
///   *un-normalized* `readable` string, so case and punctuation variants that
///   would otherwise normalize to identical text still produce different
///   names.
pub(crate) fn herdr_name(readable: &str, required_suffix: Option<&str>) -> String {
    const MAX: usize = 32;

    if required_suffix.is_none() && is_clean_herdr_name(readable) && readable.len() <= MAX {
        return readable.to_string();
    }

    let suffix = match required_suffix {
        Some(s) => s.to_string(),
        None => {
            let mut hash = 0x811c_9dc5u32;
            for byte in readable.as_bytes() {
                hash ^= u32::from(*byte);
                hash = hash.wrapping_mul(0x0100_0193);
            }
            format!("{hash:08x}")
        }
    };

    let clean = normalize_herdr_name(readable);
    let budget = MAX.saturating_sub(suffix.len() + 1);
    let cut = cut_herdr_name_at_boundary(&clean, budget);
    format!("{cut}-{suffix}")
}

fn is_clean_herdr_name(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Lowercase; collapse any run of characters outside `[a-z0-9_-]` -- `-`
/// itself included, so a source that already has doubled dashes comes out
/// clean too -- to a single `-`; never emit a leading or trailing `-`.
fn normalize_herdr_name(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_dash = false;
    for c in s.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c);
        } else if !out.is_empty() {
            // A `-` in the source and any other disallowed character are
            // treated the same way here: both just mark a boundary. Pushed
            // lazily, so a run of several never yields more than one `-`.
            pending_dash = true;
        }
    }
    out
}

/// Cut `clean` down to `budget` characters, preferring the last `-` boundary
/// so a truncated name reads as a shortened word list rather than a word cut
/// in half. A boundary too close to the start would throw away almost all of
/// the readable signal, so it is only honoured when it leaves a reasonable
/// amount behind; otherwise this hard-cuts at `budget` instead.
fn cut_herdr_name_at_boundary(clean: &str, budget: usize) -> String {
    const MIN_READABLE: usize = 8;
    if clean.len() <= budget {
        return clean.to_string();
    }
    let slice = &clean[..budget];
    match slice.rfind('-') {
        Some(pos) if pos >= MIN_READABLE => slice[..pos].to_string(),
        _ => slice.trim_end_matches('-').to_string(),
    }
}

/// A standing agent's herdr name: `factory-{scope}-{name}`, cleaned up and
/// truncated as `herdr_name` describes.
fn runtime_name(scope: &str, name: &str) -> String {
    herdr_name(&format!("factory-{scope}-{name}"), None)
}

impl Engine {
    fn declared(&self, scope: &str, name: &str) -> Result<ScopeAgent> {
        let factory = self.factory_snapshot();
        factory
            .scope(scope)?
            .agents_with(&factory.config.daemon.foreman)
            .into_iter()
            .find(|a| a.name() == name)
            .ok_or_else(|| {
                FactoryError::BadRequest(format!("scope {scope:?} declares no agent named {name:?}"))
            })
    }

    fn runtime_for(&self, scope: &str) -> String {
        let factory = self.factory_snapshot();
        factory
            .scope(scope)
            .ok()
            .and_then(|s| s.runtime.clone())
            .unwrap_or_else(|| factory.config.daemon.default_runtime.clone())
    }

    /// The session already on file for `(canonical_scope, name)`, migrating
    /// it onto its canonical id first if it is only on file under the id a
    /// scope's pre-migration bare name would have given it -- the same
    /// `<scope>/<name>` scheme, just a shorter `scope`. Without this, a
    /// standing agent started before a scope's identity became its path
    /// looks gone the first time this daemon starts under the new scheme,
    /// and gets started a second time right next to the one already running.
    /// Self-healing rather than permanent: once this has run for a session,
    /// `AgentSession::id_for(canonical_scope, name)` is the only id it has,
    /// and every future lookup finds it on the first try.
    async fn existing_agent(&self, canonical_scope: &str, name: &str) -> Result<Option<AgentSession>> {
        let id = AgentSession::id_for(canonical_scope, name);
        if let Some(found) = self.store.get_agent(&id).await? {
            return Ok(Some(found));
        }
        let Some(legacy_id) = AgentSession::legacy_id_for(canonical_scope, name) else {
            return Ok(None);
        };
        let Some(mut legacy) = self.store.get_agent(&legacy_id).await? else {
            return Ok(None);
        };
        tracing::info!(
            from = %legacy_id,
            to = %id,
            "migrating a standing agent onto its scope's current identity"
        );
        legacy.id = id;
        legacy.scope = canonical_scope.to_string();
        self.store.put_agent(&legacy).await?;
        let _ = self.store.delete_agent(&legacy_id).await;
        Ok(Some(legacy))
    }

    /// Bring a declared standing agent up. Idempotent: an agent already live is
    /// returned as it is rather than started twice.
    pub async fn start_agent(&self, scope: &str, name: &str) -> Result<AgentSession> {
        let factory = self.factory_snapshot();
        // Canonical from here down, whatever the caller typed -- a bare name
        // from before a scope's identity became its path still resolves
        // (`Factory::scope`'s fallback), and a session started from it must
        // land under the same id reconciliation and the occupancy chart both
        // expect.
        let scope = factory.scope(scope)?.name.clone();
        let scope = scope.as_str();

        let decl = self.declared(scope, name)?;
        if !decl.lifetime.is_standing() {
            return Err(FactoryError::BadRequest(format!(
                "{name:?} in {scope:?} is a task agent; start a task with it instead"
            )));
        }

        let existing = self.existing_agent(scope, name).await?;
        if let Some(existing) = &existing {
            if existing.state.is_live() && self.agent_alive(existing).await {
                return Ok(existing.clone());
            }
        }

        let runtime_name_ = self.runtime_for(scope);
        let adapter = self.registry.agent(&decl.harness)?;
        let runtime = self.registry.runtime(&runtime_name_)?;
        let cwd = factory.scope_path(scope)?;
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

        // Resolved the same way `caller_for` resolves it for every other
        // request, and only after the row above is written, so this reads
        // back the role the access check will actually apply rather than
        // whatever the agent was wearing before this start.
        let role = self.effective_role(scope, name).await;
        let role = self.roles_for(scope).get(&role).cloned();

        // No task: a standing agent is started to be there, and is told
        // nothing about one -- but it still gets the guide to Factory itself.
        let ctx = AgentContext {
            scope: scope.to_string(),
            agent_name: name.to_string(),
            cwd: cwd.clone(),
            factory_bin: self.factory_bin.clone(),
            socket: factory.socket_path(),
            guides_dir: factory.guides_dir(),
            task: None,
            identity_token: Some(identity),
            role,
        };

        let mut launch = match adapter.launch_spec(&ctx).await {
            Ok(l) => l,
            Err(e) => return self.mark_agent_failed(agent, e).await,
        };
        append_declared_args(&mut launch, Some(&decl));

        let session = match runtime
            .start(&StartRequest {
                id: agent.id.clone(),
                scope: scope.to_string(),
                name: runtime_name(scope, name),
                label: name.to_string(),
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
            // The agent's own scope's roles: one defined in `projects/a` is
            // valid there and does not exist for an agent in `projects/b`.
            let roles = self.roles_for(&agent.scope);
            if !roles.contains(role) {
                return Err(FactoryError::BadRequest(format!(
                    "no role named {:?} in {}. The roles available there are: {}",
                    role.as_str(),
                    agent.scope,
                    roles.names().join(", ")
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
        let factory = self.factory_snapshot();
        let stored = self.store.agents().await.unwrap_or_default();
        let mut seen = std::collections::BTreeSet::new();

        for scope in &factory.config.scopes {
            for decl in scope.standing_agents_with(&factory.config.daemon.foreman) {
                let id = AgentSession::id_for(&scope.name, &decl.name());
                let legacy_id = AgentSession::legacy_id_for(&scope.name, &decl.name());
                seen.insert(id.clone());
                if let Some(lid) = &legacy_id {
                    seen.insert(lid.clone());
                }

                // A session found under its canonical id first; failing
                // that, under the id its scope's pre-migration bare name
                // would have given it -- rewritten onto the canonical one
                // before anything below decides what to do with it, so the
                // "no longer declared" sweep after this loop never sees the
                // id it used to have and closes a session that just moved
                // house. `migrated_from` is only set when the row actually
                // came from the legacy lookup, so the cleanup below never
                // deletes a row this scope did not just claim.
                let mut migrated_from: Option<String> = None;
                let found = stored.iter().find(|a| a.id == id).cloned().or_else(|| {
                    legacy_id.as_deref().and_then(|lid| {
                        stored.iter().find(|a| a.id == lid).map(|a| {
                            tracing::info!(
                                from = %lid, to = %id,
                                "migrating a standing agent onto its scope's current identity"
                            );
                            migrated_from = Some(lid.to_string());
                            let mut a = a.clone();
                            a.id = id.clone();
                            a.scope = scope.name.clone();
                            a
                        })
                    })
                });

                match found {
                    // Still there from before: adopt it rather than opening a
                    // second session beside the one already running.
                    Some(existing) if existing.state.is_live() && self.agent_alive(&existing).await => {
                        let mut a = existing;
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
                        let mut a = existing;
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

                if let Some(lid) = migrated_from {
                    let _ = self.store.delete_agent(&lid).await;
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
    use factory_core::protocol::{Payload, Request, Response};
    use factory_core::run::{NewRun, RunPatch, RunStatus, Trigger};
    use factory_core::task::{NewTask, Task, TaskReport, TaskStatus};
    use factory_plugins::registry::Registry;
    use factory_plugins::{HarnessAgent, SqliteStore};
    use std::path::PathBuf;
    use std::time::Duration;
    use tokio::sync::mpsc;

    fn is_valid_herdr_name(name: &str) -> bool {
        (1..=32).contains(&name.len())
            && name.starts_with(|c: char| c.is_ascii_lowercase())
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    }

    #[test]
    fn runtime_names_obey_herdrs_identifier_contract() {
        let generated = runtime_name("factory", "Codex-Builder");
        assert!(is_valid_herdr_name(&generated), "{generated}");
        assert_eq!(generated, runtime_name("factory", "Codex-Builder"));
    }

    #[test]
    fn normalized_and_truncated_runtime_names_stay_distinct() {
        assert_ne!(runtime_name("demo", "Builder"), runtime_name("demo", "builder"));
        assert_ne!(
            runtime_name("a-very-long-scope-name", "a-very-long-agent-name-one"),
            runtime_name("a-very-long-scope-name", "a-very-long-agent-name-two")
        );
    }

    #[test]
    fn a_clean_name_that_already_fits_is_returned_verbatim() {
        // Unchanged behaviour for the common case -- `factory-demo-watcher`,
        // the exact form the README's attach-command example still uses.
        assert_eq!(runtime_name("demo", "watcher"), "factory-demo-watcher");
    }

    #[test]
    fn truncated_names_cut_at_a_word_boundary_not_mid_word() {
        // The exact defect the issue named: `factory-factory-codex-b-e63be92f`
        // cut `builder` mid-word. The hash is unchanged (it still hashes the
        // un-normalized original), so this pins the boundary cut without
        // pinning the hash algorithm to a second, redundant assertion.
        let generated = runtime_name("factory", "Codex Builder");
        assert!(is_valid_herdr_name(&generated), "{generated}");
        assert_eq!(generated, "factory-factory-codex-e63be92f");
        assert!(
            !generated.contains("-b-"),
            "must not cut mid-word: {generated}"
        );
    }

    #[test]
    fn run_names_obey_herdrs_identifier_contract() {
        let generated = herdr_name(&format!("factory-{}-{}", "demo", "shell"), Some("a1b2c3d4"));
        assert!(is_valid_herdr_name(&generated), "{generated}");
        assert_eq!(generated, "factory-demo-shell-a1b2c3d4");
    }

    #[test]
    fn two_runs_of_the_same_scope_and_agent_differ() {
        let a = herdr_name(&format!("factory-{}-{}", "demo", "shell"), Some("aaaaaaaa"));
        let b = herdr_name(&format!("factory-{}-{}", "demo", "shell"), Some("bbbbbbbb"));
        assert_ne!(a, b);
    }

    #[test]
    fn the_run_id_fragment_survives_even_long_scope_and_agent_names() {
        let generated = herdr_name(
            &format!(
                "factory-{}-{}",
                "a-very-long-scope-name-indeed", "a-very-long-agent-name-here-too"
            ),
            Some("deadbeef"),
        );
        assert!(is_valid_herdr_name(&generated), "{generated}");
        assert!(
            generated.ends_with("-deadbeef"),
            "the run's own discriminator must never be truncated away: {generated}"
        );
    }

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
            // `power_assertion` off: this file's tests dispatch real runs
            // through `start_run`, and the default would fork a real
            // `caffeinate` on whatever machine runs `cargo test` -- see the
            // same note on `engine::tests::test_engine`.
            daemon: DaemonConfig { power_assertion: false, ..DaemonConfig::default() },
            scope: None,
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
            estimate_seconds: None,
            result: None,
            error: None,
            runs: 1,
            ack_timeout_seconds: None,
            timeout_seconds: None,
            blocked_timeout_seconds: None,
            worktree: false,
            knowledge_hints: false,
            labels: Default::default(),
            created_at: now,
            updated_at: now,
            last_run_at: None,
            next_run_at: None,
            workflow_origin: None,
            bench_origin: None,
            retry: None,
            pending_retry: None,
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
        starts: std::sync::Mutex<Vec<StartRequest>>,
        stops: std::sync::Mutex<Vec<SessionRef>>,
    }

    impl StubRuntime {
        fn new() -> Self {
            Self {
                tx: std::sync::Mutex::new(None),
                starts: std::sync::Mutex::new(Vec::new()),
                stops: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl AgentRuntime for StubRuntime {
        fn name(&self) -> &str {
            "stub"
        }
        async fn start(&self, req: &StartRequest) -> Result<SessionRef> {
            self.starts.lock().unwrap().push(req.clone());
            Ok(SessionRef {
                runtime: "stub".into(),
                handle: format!("stub-{}", req.id),
                meta: Default::default(),
            })
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
        async fn stop(&self, session: &SessionRef) -> Result<()> {
            self.stops.lock().unwrap().push(session.clone());
            Ok(())
        }
        async fn watch(&self) -> Result<Option<RuntimeEventStream>> {
            let (tx, rx) = mpsc::channel(16);
            *self.tx.lock().unwrap() = Some(tx);
            Ok(Some(rx))
        }
    }

    fn recording_engine(scope_yaml: &str) -> (Arc<Engine>, Arc<StubRuntime>, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("factory-agent-args-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let config = Config {
            version: 1,
            instance: Instance {
                id: "i".into(),
                name: "test".into(),
            },
            // Same reason as `engine()`/`engine_with` above: off, so a real
            // run dispatched here never forks a real `caffeinate`.
            daemon: DaemonConfig { power_assertion: false, ..DaemonConfig::default() },
            scope: None,
            scopes: vec![serde_yaml_ng::from_str(scope_yaml).unwrap()],
            roles: Default::default(),
            plugins_dir: None,
        };
        let mut registry = Registry::with_builtins();
        registry.add_agent(
            Arc::new(
                HarnessAgent::new("configured", "pi", "configured test agent")
                    .with_args(vec!["--model".into(), "sonnet".into()]),
            ),
            "test",
        );
        let stub = Arc::new(StubRuntime::new());
        registry.add_runtime(stub.clone(), "test");
        let engine = Arc::new(Engine::new(
            Factory {
                root: root.clone(),
                config,
            },
            registry,
            Arc::new(SqliteStore::in_memory().unwrap()),
            PathBuf::from("factory"),
            vec![],
        ));
        (engine, stub, root)
    }

    #[tokio::test]
    async fn standing_agent_arguments_follow_the_adapter_defaults() {
        let (engine, stub, root) = recording_engine(
            "name: demo\npath: .\nruntime: stub\nagents:\n  - name: watcher\n    harness: configured\n    lifetime: permanent\n    args: [--model, opus]\n",
        );

        engine.start_agent("demo", "watcher").await.unwrap();

        let starts = stub.starts.lock().unwrap();
        let args = &starts[0].launch.args;
        // The adapter's own default first, then the guide `pi` gets injected
        // as its own flag, then the scope's declared `args:` last of all --
        // so a declaration can still override or add to what the adapter and
        // the guide put there.
        assert_eq!(&args[0..2], ["--model", "sonnet"]);
        assert_eq!(args[2], "--append-system-prompt");
        assert_eq!(&args[args.len() - 2..], ["--model", "opus"]);
        // The scope names the workspace a runtime like herdr should group
        // this session into; the label is now the bare agent name, since
        // `factory: scope/name` used to carry both jobs at once.
        assert_eq!(starts[0].scope, "demo");
        assert_eq!(starts[0].label, "watcher");
        drop(starts);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn deleting_a_standing_declaration_closes_its_session() {
        let (engine, stub, root) = recording_engine(
            "id: scope-id\nname: demo\npath: .\nruntime: stub\nagents:\n  - name: Watcher\n    harness: configured\n    lifetime: permanent\n",
        );
        std::fs::create_dir_all(root.join(factory_core::config::FACTORY_DIR)).unwrap();
        std::fs::write(
            root.join(factory_core::config::FACTORY_DIR)
                .join(factory_core::config::CONFIG_FILE),
            "version: 1\nscope:\n  id: scope-id\n  name: demo\n  runtime: stub\n  agents:\n    - name: Watcher\n      harness: configured\n      lifetime: permanent\n",
        )
        .unwrap();
        let standing = engine.start_agent("demo", "Watcher").await.unwrap();
        let session = standing.session.clone().unwrap();

        let response = engine
            .handle_request(Request::AgentDelete {
                scope: "demo".into(),
                name: "Watcher".into(),
            })
            .await;

        assert!(matches!(
            response,
            Response::Ok {
                data: Payload::Deleted { deleted: true }
            }
        ));
        assert_eq!(stub.stops.lock().unwrap().as_slice(), &[session]);
        assert!(engine.store.get_agent(&standing.id).await.unwrap().is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn task_agent_arguments_follow_the_adapter_defaults() {
        let (engine, stub, root) = recording_engine(
            "name: demo\npath: .\nruntime: stub\nagents:\n  - name: builder\n    harness: configured\n    args: [--model, opus]\n",
        );
        let task = engine
            .create(NewTask {
                title: "exercise configured args".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("builder".into()),
                worktree: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();

        engine.start_run(&task.id, Trigger::Manual).await;

        let starts = stub.starts.lock().unwrap();
        let args = &starts[0].launch.args;
        assert_eq!(&args[0..2], ["--model", "sonnet"], "the adapter's own defaults come first");
        assert_eq!(args[2], "--append-system-prompt", "then the guide's flag");
        assert_eq!(&args[args.len() - 2..], ["--model", "opus"], "the declared override lands last");
        // A run's scope is its task's canonical scope, its label the task's
        // title (truncated), and its name carries the scope, the agent, and
        // the run's own discriminator -- never `factory-run-<hex>` alone.
        assert_eq!(starts[0].scope, "demo");
        assert_eq!(starts[0].label, "exercise configured args");
        assert!(starts[0].name.starts_with("factory-demo-builder-"), "{}", starts[0].name);
        drop(starts);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_legacy_named_tasks_run_lands_in_the_scopes_canonical_workspace() {
        // `engine.create()` already canonicalizes a task's scope on the way
        // in (see its own comment), so a legacy-named row can only exist the
        // way an old one on disk would: written directly, bypassing it.
        //
        // Scope and agent names are kept short enough that neither is a
        // truncation casualty -- this test is about which scope name wins,
        // not about the truncation behaviour covered elsewhere.
        let (engine, stub, root) = recording_engine(
            "name: proj/demo\npath: .\nruntime: stub\nagents:\n  - name: shell\n    harness: configured\n    args: [--model, opus]\n",
        );
        let now = Utc::now();
        let task = Task {
            id: "legacy-task".into(),
            title: "legacy run".into(),
            instructions: "true".into(),
            scope: "demo".into(), // the scope's pre-migration bare name
            agent: "shell".into(),
            runtime: "stub".into(),
            status: TaskStatus::Pending,
            schedule: None,
            estimate_seconds: None,
            result: None,
            error: None,
            runs: 0,
            ack_timeout_seconds: None,
            timeout_seconds: None,
            blocked_timeout_seconds: None,
            worktree: false,
            knowledge_hints: false,
            labels: Default::default(),
            created_at: now,
            updated_at: now,
            last_run_at: None,
            next_run_at: None,
            workflow_origin: None,
            bench_origin: None,
            retry: None,
            pending_retry: None,
        };
        engine.store.create(&task).await.unwrap();

        engine.start_run(&task.id, Trigger::Manual).await;

        let starts = stub.starts.lock().unwrap();
        assert_eq!(
            starts[0].scope, "proj/demo",
            "a legacy-named task's run must resolve to the scope's canonical name, \
             the same workspace its standing agents use"
        );
        assert!(
            starts[0].name.starts_with("factory-proj-demo-shell-"),
            "{}",
            starts[0].name
        );
        drop(starts);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_bare_adapter_does_not_borrow_arguments_from_another_declaration() {
        let (engine, stub, root) = recording_engine(
            "name: demo\npath: .\nruntime: stub\nagents:\n  - name: builder\n    harness: configured\n    args: [--model, opus]\n",
        );
        let task = engine
            .create(NewTask {
                title: "use the adapter directly".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("configured".into()),
                worktree: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();

        engine.start_run(&task.id, Trigger::Manual).await;

        let starts = stub.starts.lock().unwrap();
        // No declaration named "configured" itself, so nothing to append --
        // just the adapter's own default and, after it, the guide's flag.
        let args = &starts[0].launch.args;
        assert_eq!(args.len(), 4, "{args:?}");
        assert_eq!(&args[0..3], ["--model", "sonnet", "--append-system-prompt"]);
        drop(starts);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_task_runs_guide_file_outlives_dispatch_and_is_gone_once_the_run_ends() {
        // Pins the fix for the bug this replaced: the guide was deleted right
        // after the prompt was submitted, on the assumption every harness had
        // already read it by then. That is false for `opencode`, which
        // re-resolves its configured instruction paths on every request
        // rather than once at startup -- an early delete made the guide
        // silently vanish partway through the run. The file must survive
        // dispatch and disappear only once the run actually ends, through
        // whichever path closes it.
        let (engine, _stub, root) = recording_engine(
            "name: demo\npath: .\nruntime: stub\nagents:\n  - name: builder\n    harness: configured\n",
        );
        let task = engine
            .create(NewTask {
                title: "exercise guide cleanup".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("builder".into()),
                worktree: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();

        engine.start_run(&task.id, Trigger::Manual).await;

        let guide = factory_core::adapter::agent::run_guide_path(&engine.factory_snapshot().guides_dir(), &task.id);
        assert!(guide.exists(), "still there once the harness is up and running");

        let run = engine.store.active_run(&task.id).await.unwrap().unwrap();
        engine
            .report(
                &task.id,
                TaskReport {
                    status: Some(RunStatus::Done),
                    message: None,
                    result: Some("ok".into()),
                    error: None,
                    token: run.token.clone(),
                },
            )
            .await
            .unwrap();

        assert!(!guide.exists(), "gone once the agent's own terminal report closes the run");
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_task_runs_guide_file_is_also_cleaned_up_when_the_watchdog_gives_up_on_it() {
        // The same cleanup, reached through `fail_run` instead of a report --
        // `close_session` is the one place both paths (and a cancel, and a
        // task deleted out from under an active run) go through.
        let (engine, _stub, root) = recording_engine(
            "name: demo\npath: .\nruntime: stub\nagents:\n  - name: builder\n    harness: configured\n",
        );
        let task = engine
            .create(NewTask {
                title: "exercise watchdog cleanup".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("builder".into()),
                worktree: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();

        engine.start_run(&task.id, Trigger::Manual).await;

        let guide = factory_core::adapter::agent::run_guide_path(&engine.factory_snapshot().guides_dir(), &task.id);
        assert!(guide.exists());

        let run = engine.store.active_run(&task.id).await.unwrap().unwrap();
        engine.fail_run(&run.id, "gave up waiting").await;

        assert!(!guide.exists(), "gone once the watchdog closes the run too");
        std::fs::remove_dir_all(root).ok();
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
