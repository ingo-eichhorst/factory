//! Standing agents: starting them, keeping them, and letting a person type at
//! them.
//!
//! A task agent lives for one run and is failed if it goes quiet. A standing
//! agent is quiet by design -- it is only ever checked for whether its session
//! is still there.

use chrono::Utc;
use factory_core::adapter::agent::AgentContext;
use factory_core::adapter::runtime::{RuntimeStatus, Screen, StartRequest};
use factory_core::agent::{AgentSession, AgentState, KeptInstance, KeptUp, Lifetime, ScaleReport};
use factory_core::role::Role;
use factory_core::config::ScopeAgent;
use factory_core::error::{FactoryError, Result};
use factory_core::event::Event;
use factory_core::task::SessionRef;

use crate::engine::append_declared_args;
use crate::l3_service::L3Service;
#[cfg(test)]
use crate::engine::Engine;
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use factory_core::adapter::runtime::{RuntimeEvent, RuntimeEventKind, RuntimeEventStream};

// ============================================================== capacity

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

/// A standing instance's herdr name: `factory-{scope}-{name}`, cleaned up and
/// truncated as `herdr_name` describes. Instance 1 keeps exactly the name a
/// standing agent had before it could have more than one, so a session
/// already running under it is adopted, not duplicated. The others carry their
/// number behind a `#` (`#329`): no clean name contains one, so `herdr_name`
/// always hashes them, and instance 2 of `watcher` never comes out named like
/// instance 1 of an agent called `watcher-2`.
fn runtime_name(scope: &str, name: &str, instance: u32) -> String {
    match instance {
        0 | 1 => herdr_name(&format!("factory-{scope}-{name}"), None),
        n => herdr_name(&format!("factory-{scope}-{name}#{n}"), None),
    }
}

/// Why a scale-down that wanted `instance` gone could not take it.
fn kept(agent: &AgentSession, why: KeptUp) -> KeptInstance {
    KeptInstance { id: agent.id.clone(), instance: agent.instance, why }
}

impl L3Service<'_> {
    /// Which standing agent holds this session, if one does: `(subject, scope, name)`,
    /// a standing agent's own id. A task run's session is L4's to say.
    pub(crate) async fn agent_subject_for_session(&self, session: &SessionRef) -> Option<(String, String, String)> {
        let agents = self.state.agents.agents().await.ok()?;
        let agent = agents.into_iter().find(|a| a.session.as_ref() == Some(session))?;
        Some((agent.id, agent.scope, agent.name))
    }

    fn declared(&self, scope: &str, name: &str) -> Result<ScopeAgent> {
        let factory = self.wiring.snapshot();
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
        let factory = self.wiring.snapshot();
        factory
            .scope(scope)
            .ok()
            .and_then(|s| s.runtime.clone())
            .unwrap_or_else(|| factory.config.daemon.default_runtime.clone())
    }

    /// Every row on file for one declaration, lowest instance first.
    async fn instances_of(&self, scope: &str, name: &str) -> Result<Vec<AgentSession>> {
        let mut rows: Vec<AgentSession> = self
            .state
            .agents
            .agents()
            .await?
            .into_iter()
            .filter(|a| a.scope == scope && a.name == name)
            .collect();
        rows.sort_by_key(|a| a.instance);
        Ok(rows)
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
    ///
    /// Instance 1 only: nothing was ever written under an old id with a `#n`.
    async fn existing_agent(&self, canonical_scope: &str, name: &str) -> Result<Option<AgentSession>> {
        let id = AgentSession::id_for(canonical_scope, name);
        if let Some(found) = self.state.agents.get_agent(&id).await? {
            return Ok(Some(found));
        }
        let Some(legacy_id) = AgentSession::legacy_id_for(canonical_scope, name) else {
            return Ok(None);
        };
        let Some(mut legacy) = self.state.agents.get_agent(&legacy_id).await? else {
            return Ok(None);
        };
        tracing::info!(
            from = %legacy_id,
            to = %id,
            "migrating a standing agent onto its scope's current identity"
        );
        legacy.id = id;
        legacy.scope = canonical_scope.to_string();
        self.state.agents.put_agent(&legacy).await?;
        let _ = self.state.agents.delete_agent(&legacy_id).await;
        Ok(Some(legacy))
    }

    /// Bring a declared standing agent up: every instance its declaration
    /// wants that is not already live (`#329`). Idempotent: an instance already
    /// live is left as it is rather than started twice. Answers the lowest
    /// instance -- instance 1, for every declaration that has one -- and, if an
    /// instance would not start, the first such error once every other has
    /// had its chance.
    pub async fn start_agent(&self, scope: &str, name: &str) -> Result<AgentSession> {
        let factory = self.wiring.snapshot();
        let scope = factory.scope(scope)?.name.clone();
        let decl = self.declared(&scope, name)?;
        let mut first = None;
        let mut failed = None;
        for instance in 1..=decl.instance_count() {
            match self.start_instance(&scope, name, instance).await {
                Ok(agent) => {
                    first.get_or_insert(agent);
                }
                Err(e) => {
                    failed.get_or_insert(e);
                }
            }
        }
        match (failed, first) {
            (Some(e), _) => Err(e),
            (None, Some(agent)) => Ok(agent),
            (None, None) => Err(FactoryError::BadRequest(format!("{name:?} in {scope:?} has no instance to start"))),
        }
    }

    /// Bring one instance of a declared standing agent up. Idempotent: an
    /// instance already live is returned as it is rather than started twice.
    pub async fn start_instance(&self, scope: &str, name: &str, instance: u32) -> Result<AgentSession> {
        let factory = self.wiring.snapshot();
        // Canonical from here down, whatever the caller typed -- a bare name
        // from before a scope's identity became its path still resolves
        // (`Factory::scope`'s fallback), and a session started from it must
        // land under the same id reconciliation and the occupancy chart both
        // expect.
        let scope = factory.scope(scope)?.name.clone();
        let scope = scope.as_str();
        let instance = instance.max(1);

        let decl = self.declared(scope, name)?;
        if !decl.lifetime.is_standing() {
            return Err(FactoryError::BadRequest(format!(
                "{name:?} in {scope:?} is a task agent; start a task with it instead"
            )));
        }

        let existing = if instance == 1 {
            self.existing_agent(scope, name).await?
        } else {
            self.state.agents.get_agent(&AgentSession::instance_id(scope, name, instance)).await?
        };
        if let Some(existing) = &existing {
            if existing.state.is_live() && self.agent_alive(existing).await {
                return Ok(existing.clone());
            }
        }
        // A role is the declaration's: instance 1 holds it, the others wear it.
        let assigned_role = if instance == 1 {
            existing.and_then(|a| a.assigned_role)
        } else {
            self.state
                .agents
                .get_agent(&AgentSession::id_for(scope, name))
                .await?
                .and_then(|a| a.assigned_role)
        };

        let runtime_name_ = self.runtime_for(scope);
        let adapter = self.wiring.registry().agent(&decl.harness)?;
        let runtime = self.wiring.registry().runtime(&runtime_name_)?;
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
        )
        .as_instance(instance);
        // A role somebody gave this agent outlives the session it was given in.
        agent.assigned_role = assigned_role;
        agent.role = agent.role_with(&decl.role);
        // A fresh token each time it comes up: an old session's token must not
        // still speak for the agent that replaced it.
        let identity = factory_core::new_token();
        agent.token = Some(identity.clone());
        agent.state = AgentState::Starting;
        agent.started_at = Utc::now();
        agent.error = None;
        self.state.agents.put_agent(&agent).await?;
        self.wiring.bus().publish(Event::AgentUpdated {
            agent: agent.clone(),
        });

        // Resolved the same way `caller_for` resolves it for every other
        // request, and only after the row above is written, so this reads
        // back the role the access check will actually apply rather than
        // whatever the agent was wearing before this start.
        let role = self.effective_role(scope, name).await;
        let role = self.wiring.roles_for(scope).get(&role).cloned();

        // No task: a standing agent is started to be there, and is told
        // nothing about one -- but it still gets the guide to Factory itself,
        // including which frameworks its scope is committed to (a permanent
        // agent gets this exactly once, at start, the same as its role).
        let policy_frameworks = factory_core::policy::frameworks_in_chain(&self.wiring.policy_chain(scope));
        let ctx = AgentContext {
            scope: scope.to_string(),
            agent_name: name.to_string(),
            cwd: cwd.clone(),
            factory_bin: self.wiring.factory_bin().to_path_buf(),
            socket: factory.socket_path(),
            callback_url: None,
            guides_dir: factory.guides_dir(),
            task: None,
            identity_token: Some(identity),
            role,
            policy_frameworks,
            goal: None,
            // The quality block is a task run's: judging a scope's
            // scenarios reads metrics and evidence as of the dispatch, and a
            // standing agent's guide is written once and kept for its whole
            // life, so any status in it would soon be a stale claim.
            quality: Vec::new(),
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
                name: runtime_name(scope, name, instance),
                // The tab says which one it is once there is more than one.
                label: match instance {
                    1 => name.to_string(),
                    n => format!("{name} #{n}"),
                },
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
        self.state.agents.put_agent(&agent).await?;
        self.wiring.bus().publish(Event::AgentUpdated {
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
        let _ = self.state.agents.put_agent(&agent).await;
        self.wiring.bus().publish(Event::AgentUpdated { agent });
        Err(e)
    }

    /// Stop a standing agent and leave it stopped. Nothing restarts it until
    /// somebody asks, not even a permanent one -- otherwise the button would
    /// do nothing anybody could see. One instance: the one `id` names.
    pub async fn stop_agent(&self, id: &str) -> Result<AgentSession> {
        let mut agent = self.require_agent(id).await?;
        if let Some(session) = &agent.session {
            if let Ok(runtime) = self.wiring.registry().runtime(&session.runtime) {
                let _ = runtime.stop(session).await;
            }
        }
        agent.session = None;
        agent.attach = None;
        agent.token = None;
        agent.state = AgentState::Stopped;
        agent.last_seen_at = Utc::now();
        self.state.agents.put_agent(&agent).await?;
        self.wiring.bus().publish(Event::AgentUpdated {
            agent: agent.clone(),
        });
        self.observe(&agent.id, &agent.scope, &agent.name, RuntimeStatus::Gone);
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
    /// its role is a question for the config. Every instance of it, whichever
    /// one `id` names: two instances of one declaration are the same agent,
    /// and must never be told two different things about what they may do.
    pub async fn set_agent_role(&self, id: &str, role: Option<Role>) -> Result<AgentSession> {
        let agent = self.require_agent(id).await?;
        if let Some(role) = &role {
            // The agent's own scope's roles: one defined in `projects/a` is
            // valid there and does not exist for an agent in `projects/b`.
            let roles = self.wiring.roles_for(&agent.scope);
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
        let mut answer = agent.clone();
        for mut row in self.instances_of(&agent.scope, &agent.name).await? {
            row.assigned_role = role.clone();
            row.role = row.role_with(&declared);
            row.last_seen_at = Utc::now();
            self.state.agents.put_agent(&row).await?;
            self.wiring.bus().publish(Event::AgentUpdated {
                agent: row.clone(),
            });
            if row.id == agent.id {
                answer = row;
            }
        }
        tracing::info!(agent = %answer.id, role = %answer.role, "role set");
        Ok(answer)
    }

    /// Type at an agent. This is how a person answers the prompt an agent is
    /// sitting on -- a trust dialog, a permission question -- without leaving
    /// the page.
    pub async fn agent_input(&self, id: &str, text: Option<&str>, keys: &[String]) -> Result<()> {
        let agent = self.require_agent(id).await?;
        let session = agent.session.as_ref().ok_or_else(|| {
            FactoryError::BadRequest(format!("{id} has no session to type into"))
        })?;
        self.state.attention.touch(&agent.id);
        let runtime = self.wiring.registry().runtime(&session.runtime)?;
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
        self.state.attention.touch(&agent.id);
        let runtime = self.wiring.registry().runtime(&session.runtime)?;
        Ok(runtime.read(session, lines).await.unwrap_or_default())
    }

    /// One frame of a standing agent's screen. A runtime that cannot render one
    /// says so, and the caller shows the transcript instead.
    pub async fn agent_screen(&self, id: &str) -> Result<Option<Screen>> {
        let agent = self.require_agent(id).await?;
        let Some(session) = &agent.session else {
            return Ok(None);
        };
        self.state.attention.touch(&agent.id);
        let runtime = self.wiring.registry().runtime(&session.runtime)?;
        runtime.screen(session).await
    }

    pub async fn require_agent(&self, id: &str) -> Result<AgentSession> {
        self.state.agents
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
        let status = match self.wiring.registry().runtime(&session.runtime) {
            Ok(rt) => rt.status(session).await.unwrap_or(RuntimeStatus::Gone),
            Err(_) => RuntimeStatus::Gone,
        };
        self.observe(&agent.id, &agent.scope, &agent.name, status);
        status != RuntimeStatus::Gone
    }

    /// What, if anything, stopping this instance now would cost somebody: a
    /// person who has it in front of them, a turn it is in the middle of, a
    /// question it is waiting on an answer to. L3's own observation only --
    /// the attention it has seen and the runtime's word on the session. A run
    /// is L4's and never comes into it: no run is ever given a standing
    /// agent's session.
    async fn kept_up_by(&self, agent: &AgentSession) -> Option<KeptUp> {
        if self.state.attention.attended(&agent.id) {
            return Some(KeptUp::Attached);
        }
        let session = agent.session.as_ref()?;
        let status = match self.wiring.registry().runtime(&session.runtime) {
            Ok(rt) => rt.status(session).await.unwrap_or(RuntimeStatus::Unknown),
            Err(_) => RuntimeStatus::Unknown,
        };
        self.observe(&agent.id, &agent.scope, &agent.name, status);
        match status {
            RuntimeStatus::Working => Some(KeptUp::Working),
            RuntimeStatus::Blocked => Some(KeptUp::Blocked),
            _ => None,
        }
    }

    /// Take away an instance its declaration no longer wants, after a
    /// scale-down -- if nobody would lose anything by it: its session is
    /// closed and its row goes. One that `kept_up_by` says is still worth
    /// something stays exactly as it is, except that it is no longer declared
    /// (so nothing restarts it should it die), and the answer says why; the
    /// supervisor asks again on every tick, and it goes once it is idle.
    /// `None` once it is gone.
    async fn drain(&self, mut agent: AgentSession) -> Option<KeptUp> {
        if agent.state.is_live() {
            if let Some(why) = self.kept_up_by(&agent).await {
                if agent.declared {
                    agent.declared = false;
                    let _ = self.state.agents.put_agent(&agent).await;
                    self.wiring.bus().publish(Event::AgentUpdated { agent: agent.clone() });
                    tracing::info!(agent = %agent.id, "surplus standing instance left up because {}", why.words());
                }
                return Some(why);
            }
            if let Some(session) = &agent.session {
                if let Ok(rt) = self.wiring.registry().runtime(&session.runtime) {
                    let _ = rt.stop(session).await;
                }
            }
            self.observe(&agent.id, &agent.scope, &agent.name, RuntimeStatus::Gone);
        }
        let _ = self.state.agents.delete_agent(&agent.id).await;
        self.state.attention.forget(&agent.id);
        self.wiring.bus().publish(Event::AgentRemoved { id: agent.id.clone() });
        tracing::info!(agent = %agent.id, "surplus standing instance stopped");
        None
    }

    /// Line up what the config declares with what is actually still running,
    /// after a restart or a config change. Three different things can be true
    /// of each instance a declaration wants, and each needs its own answer;
    /// an instance above the count it wants is drained.
    pub async fn reconcile_agents(&self) {
        let factory = self.wiring.snapshot();
        let stored = self.state.agents.agents().await.unwrap_or_default();
        let mut seen = std::collections::BTreeSet::new();

        for scope in &factory.config.scopes {
            for decl in scope.standing_agents_with(&factory.config.daemon.foreman) {
                let name = decl.name();
                let wanted = decl.instance_count();
                for instance in 1..=wanted {
                    self.reconcile_instance(&scope.name, &decl, instance, &stored, &mut seen).await;
                }
                // Instances above the count: a scale-down made while this
                // daemon was not running, or a count lowered by hand. Highest
                // first, as a scale-down takes them.
                let mut surplus: Vec<&AgentSession> = stored
                    .iter()
                    .filter(|a| a.scope == scope.name && a.name == name && a.instance > wanted)
                    .collect();
                surplus.sort_by_key(|a| std::cmp::Reverse(a.instance));
                for extra in surplus {
                    seen.insert(extra.id.clone());
                    self.drain(extra.clone()).await;
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
                if let Ok(rt) = self.wiring.registry().runtime(&session.runtime) {
                    let _ = rt.stop(session).await;
                }
            }
            let _ = self.state.agents.delete_agent(&agent.id).await;
            self.wiring.bus().publish(Event::AgentRemoved { id: agent.id });
        }
    }

    /// One instance a declaration wants, lined up with what is on file.
    async fn reconcile_instance(
        &self,
        scope: &str,
        decl: &ScopeAgent,
        instance: u32,
        stored: &[AgentSession],
        seen: &mut std::collections::BTreeSet<String>,
    ) {
        let name = decl.name();
        let id = AgentSession::instance_id(scope, &name, instance);
        // Only instance 1 can be on file under the id its scope's
        // pre-migration bare name would have given it.
        let legacy_id = (instance == 1).then(|| AgentSession::legacy_id_for(scope, &name)).flatten();
        seen.insert(id.clone());
        if let Some(lid) = &legacy_id {
            seen.insert(lid.clone());
        }

        // A session found under its canonical id first; failing that, under
        // the id its scope's pre-migration bare name would have given it --
        // rewritten onto the canonical one before anything below decides what
        // to do with it, so the "no longer declared" sweep after this never
        // sees the id it used to have and closes a session that just moved
        // house. `migrated_from` is only set when the row actually came from
        // the legacy lookup, so the cleanup below never deletes a row this
        // scope did not just claim.
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
                    a.scope = scope.to_string();
                    a
                })
            })
        });

        match found {
            // Still there from before: adopt it rather than opening a second
            // session beside the one already running.
            Some(existing) if existing.state.is_live() && self.agent_alive(&existing).await => {
                let mut a = existing;
                a.declared = true;
                a.lifetime = decl.lifetime;
                a.role = a.role_with(&decl.role);
                a.last_seen_at = Utc::now();
                let _ = self.state.agents.put_agent(&a).await;
                tracing::info!(agent = %id, "adopted a standing agent that outlived the daemon");
            }
            // Known but not running. Start it if it is meant to start itself;
            // a stopped one stays stopped.
            Some(existing) => {
                let mut a = existing;
                a.declared = true;
                a.lifetime = decl.lifetime;
                a.role = a.role_with(&decl.role);
                a.session = None;
                if a.state != AgentState::Stopped {
                    a.state = AgentState::Gone;
                }
                let _ = self.state.agents.put_agent(&a).await;
                if decl.autostart() && a.state != AgentState::Stopped {
                    self.autostart(scope, &name, instance).await;
                }
            }
            // Never seen. Record it so the page can show it, and start it if
            // it wants starting.
            None => {
                let a = AgentSession::new(
                    scope,
                    &name,
                    &decl.harness,
                    &self.runtime_for(scope),
                    decl.lifetime,
                    decl.role.clone(),
                )
                .as_instance(instance);
                let _ = self.state.agents.put_agent(&a).await;
                if decl.autostart() {
                    self.autostart(scope, &name, instance).await;
                }
            }
        }

        if let Some(lid) = migrated_from {
            let _ = self.state.agents.delete_agent(&lid).await;
        }
    }

    async fn autostart(&self, scope: &str, name: &str, instance: u32) {
        if let Err(e) = self.start_instance(scope, name, instance).await {
            tracing::warn!(scope, name, instance, "could not start standing agent: {e}");
        }
    }

    /// Line one declaration's instances up with the count its config now
    /// gives it, once `factory agent scale` has written that count (`#329`).
    ///
    /// Up: the instances it lacks are recorded, and handed back to be started
    /// when the agent is up already -- growing something that is running means
    /// more of it running -- or when it starts itself and nobody has stopped
    /// it. Down: the instances above the count are drained, highest first;
    /// one a person would lose something by is left up and named, never
    /// forced. The router starts what this hands back, in the background, the
    /// way a newly configured agent is started.
    pub async fn scale_agent(&self, scope: &str, name: &str, previous: u32) -> Result<(ScaleReport, Vec<u32>)> {
        let decl = self.declared(scope, name)?;
        let wanted = decl.instance_count();
        let rows = self.instances_of(scope, name).await?;
        let up_already = rows.iter().any(|a| a.state.is_live());
        let held_down = rows.iter().any(|a| a.state == AgentState::Stopped);
        let start = up_already || (decl.autostart() && !held_down);

        let mut report = ScaleReport {
            scope: scope.to_string(),
            name: name.to_string(),
            previous,
            wanted,
            ..ScaleReport::default()
        };
        let mut to_start = Vec::new();
        for instance in 1..=wanted {
            if let Some(row) = rows.iter().find(|a| a.instance == instance) {
                // Left up by an earlier scale-down and wanted again: it is
                // one of the count once more, restarted should it die.
                if !row.declared {
                    let mut row = row.clone();
                    row.declared = true;
                    self.state.agents.put_agent(&row).await?;
                    self.wiring.bus().publish(Event::AgentUpdated { agent: row });
                }
                continue;
            }
            let a = AgentSession::new(scope, name, &decl.harness, &self.runtime_for(scope), decl.lifetime, decl.role.clone())
                .as_instance(instance);
            self.state.agents.put_agent(&a).await?;
            self.wiring.bus().publish(Event::AgentUpdated { agent: a.clone() });
            if start {
                report.started.push(a.id.clone());
                to_start.push(instance);
            }
        }
        for extra in rows.iter().rev().filter(|a| a.instance > wanted) {
            match self.drain(extra.clone()).await {
                None => report.stopped.push(extra.id.clone()),
                Some(why) => report.kept.push(kept(extra, why)),
            }
        }
        report.up = self
            .instances_of(scope, name)
            .await?
            .iter()
            .filter(|a| a.state.is_live())
            .count() as u32;
        Ok((report, to_start))
    }

    /// The standing-agent watchdog. Deliberately not the run watchdog: a
    /// permanent agent that has said nothing for an hour is doing its job.
    pub async fn supervise_agents(&self) {
        let agents = self.state.agents.agents().await.unwrap_or_default();
        for agent in agents {
            let declaration = self.declared(&agent.scope, &agent.name).ok().filter(|d| d.lifetime.is_standing());
            // Above the count its declaration wants: left up by a scale-down
            // because it was busy, and taken away once it is not.
            if declaration.as_ref().is_some_and(|d| agent.instance > d.instance_count()) {
                self.drain(agent).await;
                continue;
            }
            if !agent.declared || agent.state == AgentState::Stopped {
                continue;
            }
            if agent.state.is_live() {
                if self.agent_alive(&agent).await {
                    let mut a = agent;
                    a.last_seen_at = Utc::now();
                    a.state = AgentState::Ready;
                    let _ = self.state.agents.put_agent(&a).await;
                    continue;
                }
                let mut a = agent.clone();
                a.state = AgentState::Gone;
                a.session = None;
                a.attach = None;
                let _ = self.state.agents.put_agent(&a).await;
                self.wiring.bus().publish(Event::AgentUpdated { agent: a });
                tracing::warn!(agent = %agent.id, "standing agent's session is gone");
            }

            // Gone and permanent means bring it back -- this instance, not
            // the first.
            if agent.lifetime == Lifetime::Permanent && declaration.is_some_and(|d| d.autostart()) {
                tracing::info!(agent = %agent.id, "restarting permanent agent");
                self.autostart(&agent.scope, &agent.name, agent.instance).await;
            }
        }
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
        let generated = runtime_name("factory", "Codex-Builder", 1);
        assert!(is_valid_herdr_name(&generated), "{generated}");
        assert_eq!(generated, runtime_name("factory", "Codex-Builder", 1));
    }

    #[test]
    fn normalized_and_truncated_runtime_names_stay_distinct() {
        assert_ne!(runtime_name("demo", "Builder", 1), runtime_name("demo", "builder", 1));
        assert_ne!(
            runtime_name("a-very-long-scope-name", "a-very-long-agent-name-one", 1),
            runtime_name("a-very-long-scope-name", "a-very-long-agent-name-two", 1)
        );
    }

    #[test]
    fn a_clean_name_that_already_fits_is_returned_verbatim() {
        // Unchanged behaviour for the common case -- `factory-demo-watcher`,
        // the exact form the README's attach-command example still uses.
        assert_eq!(runtime_name("demo", "watcher", 1), "factory-demo-watcher");
    }

    #[test]
    fn truncated_names_cut_at_a_word_boundary_not_mid_word() {
        // The exact defect the issue named: `factory-factory-codex-b-e63be92f`
        // cut `builder` mid-word. The hash is unchanged (it still hashes the
        // un-normalized original), so this pins the boundary cut without
        // pinning the hash algorithm to a second, redundant assertion.
        let generated = runtime_name("factory", "Codex Builder", 1);
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
            dashboard: None,
            policies: Default::default(),
            quality: Default::default(),
            infrastructure: Default::default(),
            secrets: Vec::new(),
            channels: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
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
        engine.l4.store.put_agent(&agent).await.unwrap();
        agent
    }

    /// A task with one active run, its session set to `sess`.
    async fn task_and_run(engine: &Engine, scope: &str, agent_name: &str, sess: SessionRef) -> (Task, String) {
        let now = Utc::now();
        let task = Task {
            after: None,
            after_condition: None,
            id: "t1".into(),
            title: "t".into(),
            instructions: String::new(),
            scope: scope.into(),
            agent: agent_name.into(),
            runtime: "herdr".into(),
            status: TaskStatus::Running,
            schedule: None,
            estimate_seconds: None,
            estimate: None,
            result: None,
            routed_to: None,
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
            parent_task_id: None,
            decomposition_part: None,
            depends_on: Vec::new(),
            bench_origin: None,
            retry: None,
            pending_retry: None,
            schedule_paused: false,
            category: None,
            intake: None,
            failure: None,
            closure: None,
            slot_wait: None,
            deliver: None,
        };
        let task = engine.l4.store.create(&task).await.unwrap();
        let run = engine
            .l4.store
            .create_run(&NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Manual,
                agent: agent_name.into(),
                adapter: "pi".into(),
                runtime: "herdr".into(),
                token: "tok".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        let run = engine
            .l4.store
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

        let mut bus = engine.shared.bus.subscribe();
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

        let mut bus = engine.shared.bus.subscribe();
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

        let mut bus = engine.shared.bus.subscribe();
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
            .l4.store
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
        let before = engine.l4.store.get(&task.id).await.unwrap().unwrap();

        engine
            .on_runtime_event(RuntimeEvent {
                session: sess,
                kind: RuntimeEventKind::SessionGone,
            })
            .await;

        let run_after = engine.l4.store.get_run(&run_id).await.unwrap().unwrap();
        assert_eq!(
            run_after.status,
            RunStatus::Running,
            "a pushed event must never move a run's status"
        );
        let task_after = engine.l4.store.get(&task.id).await.unwrap().unwrap();
        assert_eq!(
            task_after.status, before.status,
            "nor a task's -- only the agent's own report may do that"
        );
    }

    #[tokio::test]
    async fn an_event_with_no_matching_session_is_dropped_quietly() {
        let engine = engine();
        let mut bus = engine.shared.bus.subscribe();
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
        /// What `status` answers per session handle; `Unknown` otherwise.
        statuses: std::sync::Mutex<std::collections::HashMap<String, RuntimeStatus>>,
    }

    impl StubRuntime {
        fn new() -> Self {
            Self {
                tx: std::sync::Mutex::new(None),
                starts: std::sync::Mutex::new(Vec::new()),
                stops: std::sync::Mutex::new(Vec::new()),
                statuses: std::sync::Mutex::new(Default::default()),
            }
        }

        /// The handle `start` gives the session it opens for `id`.
        fn handle(id: &str) -> String {
            format!("stub-{id}")
        }

        fn set_status(&self, id: &str, status: RuntimeStatus) {
            self.statuses.lock().unwrap().insert(Self::handle(id), status);
        }

        fn started_ids(&self) -> Vec<String> {
            self.starts.lock().unwrap().iter().map(|s| s.id.clone()).collect()
        }

        fn stopped_handles(&self) -> Vec<String> {
            self.stops.lock().unwrap().iter().map(|s| s.handle.clone()).collect()
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
                handle: Self::handle(&req.id),
                meta: Default::default(),
            })
        }
        async fn submit(&self, _session: &SessionRef, _text: &str) -> Result<()> {
            Ok(())
        }
        async fn status(&self, session: &SessionRef) -> Result<RuntimeStatus> {
            Ok(self.statuses.lock().unwrap().get(&session.handle).copied().unwrap_or(RuntimeStatus::Unknown))
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
        let stub = Arc::new(StubRuntime::new());
        let engine = recording_engine_over(scope_yaml, &root, stub.clone(), Arc::new(SqliteStore::in_memory().unwrap()));
        (engine, stub, root)
    }

    /// The same engine over a store and a runtime that outlive it: a second
    /// one built over both is a daemon restarted under sessions still running.
    fn recording_engine_over(
        scope_yaml: &str,
        root: &std::path::Path,
        stub: Arc<StubRuntime>,
        store: Arc<SqliteStore>,
    ) -> Arc<Engine> {
        let config = Config {
            version: 1,
            instance: Instance {
                id: "i".into(),
                name: "test".into(),
            },
            // Same reason as `engine()`/`engine_with` above: off, so a real
            // run dispatched here never forks a real `caffeinate`. The
            // harness probe off as well: these dispatch `pi` to a stub
            // runtime, and must not depend on `pi` being installed where
            // the tests run (#131).
            daemon: DaemonConfig {
                power_assertion: false,
                harness_health: factory_core::config::HarnessHealthConfig {
                    enabled: false,
                    ..Default::default()
                },
                ..DaemonConfig::default()
            },
            scope: None,
            scopes: vec![serde_yaml_ng::from_str(scope_yaml).unwrap()],
            roles: Default::default(),
            dashboard: None,
            policies: Default::default(),
            quality: Default::default(),
            infrastructure: Default::default(),
            secrets: Vec::new(),
            channels: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let mut registry = Registry::with_builtins();
        registry.add_agent(
            Arc::new(
                HarnessAgent::new("configured", "pi", "configured test agent")
                    .with_args(vec!["--model".into(), "sonnet".into()]),
            ),
            "test",
        );
        registry.add_runtime(stub, "test");
        Arc::new(Engine::new(
            Factory {
                root: root.to_path_buf(),
                config,
            },
            registry,
            store,
            PathBuf::from("factory"),
            vec![],
        ))
    }

    /// A `demo` scope whose config is on disk as well as in the snapshot, so
    /// a scale has a file to write: `agents` is its `agents:` list, written
    /// the way a person would, two spaces in.
    fn scaled_engine(agents: &str) -> (Arc<Engine>, Arc<StubRuntime>, PathBuf) {
        let (engine, stub, root) =
            recording_engine(&format!("id: scope-id\nname: demo\npath: .\nruntime: stub\nagents:\n{agents}"));
        let indented: String = agents.lines().map(|line| format!("  {line}\n")).collect();
        std::fs::create_dir_all(root.join(factory_core::config::FACTORY_DIR)).unwrap();
        std::fs::write(
            config_file(&root),
            format!("version: 1\nscope:\n  id: scope-id\n  name: demo\n  runtime: stub\n  agents:\n{indented}"),
        )
        .unwrap();
        (engine, stub, root)
    }

    fn config_file(root: &std::path::Path) -> PathBuf {
        root.join(factory_core::config::FACTORY_DIR).join(factory_core::config::CONFIG_FILE)
    }

    const THREE_WATCHERS: &str =
        "  - name: watcher\n    harness: configured\n    lifetime: permanent\n    max_sessions: 5\n    instances: 3\n";

    async fn scale(engine: &Arc<Engine>, instances: Option<u32>, by: Option<i64>) -> std::result::Result<ScaleReport, String> {
        match engine
            .handle_request(Request::AgentScale { scope: "demo".into(), name: "watcher".into(), instances, by })
            .await
        {
            Response::Ok { data: Payload::Scaled { scale } } => Ok(scale),
            Response::Error { message, .. } => Err(message),
            other => panic!("unexpected answer {other:?}"),
        }
    }

    /// A scale-up starts its new instances in the background; wait for them.
    async fn started(stub: &StubRuntime, count: usize) -> Vec<String> {
        for _ in 0..300 {
            if stub.starts.lock().unwrap().len() >= count {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        stub.started_ids()
    }

    async fn rows(engine: &Engine) -> Vec<String> {
        let mut ids: Vec<String> = engine.l3.agents.agents().await.unwrap().into_iter().map(|a| a.id).collect();
        ids.sort();
        ids
    }

    #[test]
    fn instance_one_keeps_its_herdr_name_and_the_others_never_collide_with_a_declared_name() {
        assert_eq!(runtime_name("demo", "watcher", 1), "factory-demo-watcher");
        let second = runtime_name("demo", "watcher", 2);
        assert!(is_valid_herdr_name(&second), "{second}");
        assert!(second.starts_with("factory-demo-watcher-2-"), "readable, and carries its number: {second}");
        assert_ne!(second, runtime_name("demo", "watcher-2", 1), "never an agent called watcher-2");
        assert_ne!(second, runtime_name("demo", "watcher", 3));
        assert_eq!(second, runtime_name("demo", "watcher", 2), "stable, so a restart adopts it");
        let long = runtime_name("factory", "business-factory-claude", 2);
        assert!(is_valid_herdr_name(&long), "{long}");
        assert_ne!(long, runtime_name("factory", "business-factory-claude", 1));
    }

    #[tokio::test]
    async fn instances_three_brings_up_three_standing_sessions_and_a_restart_adopts_all_of_them() {
        let root = std::env::temp_dir().join(format!("factory-agent-instances-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let yaml = format!("name: demo\npath: .\nruntime: stub\nagents:\n{THREE_WATCHERS}");
        let stub = Arc::new(StubRuntime::new());
        let store = Arc::new(SqliteStore::in_memory().unwrap());
        let engine = recording_engine_over(&yaml, &root, stub.clone(), store.clone());

        engine.l3_service().reconcile_agents().await;

        let starts = stub.starts.lock().unwrap().clone();
        let ids: Vec<&str> = starts.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["demo/watcher", "demo/watcher#2", "demo/watcher#3"]);
        let labels: Vec<&str> = starts.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["watcher", "watcher #2", "watcher #3"]);
        let names: std::collections::BTreeSet<&str> = starts.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names.len(), 3, "three herdr names: {names:?}");
        assert_eq!(starts[0].name, "factory-demo-watcher", "instance 1 is named as before");

        // `factory agents`: one row, three instances, each its own id and state.
        let Response::Ok { data: Payload::Scopes { scopes, .. } } = engine.handle_request(Request::Agents).await else {
            panic!("the roster");
        };
        let watcher = scopes[0].agents.iter().find(|a| a.name == "watcher").unwrap();
        assert_eq!(watcher.wanted, Some(3));
        assert_eq!(watcher.id.as_deref(), Some("demo/watcher"), "the row itself is instance 1's");
        let listed: Vec<(&str, &str)> = watcher.instances.iter().map(|i| (i.id.as_str(), i.state.as_str())).collect();
        assert_eq!(listed, [("demo/watcher", "ready"), ("demo/watcher#2", "ready"), ("demo/watcher#3", "ready")]);

        // The daemon restarts: a new engine over the same rows and the same
        // sessions adopts every one rather than starting a duplicate.
        drop(engine);
        let restarted = recording_engine_over(&yaml, &root, stub.clone(), store);
        restarted.l3_service().reconcile_agents().await;
        assert_eq!(stub.starts.lock().unwrap().len(), 3, "nothing started twice");
        assert_eq!(rows(&restarted).await, ["demo/watcher", "demo/watcher#2", "demo/watcher#3"]);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_single_instance_declaration_is_unchanged_in_id_name_and_roster() {
        let (engine, stub, root) = recording_engine(
            "name: demo\npath: .\nruntime: stub\nagents:\n  - name: watcher\n    harness: configured\n    lifetime: permanent\n",
        );
        engine.l3_service().reconcile_agents().await;
        let starts = stub.starts.lock().unwrap().clone();
        assert_eq!(starts.len(), 1);
        assert_eq!((starts[0].id.as_str(), starts[0].name.as_str()), ("demo/watcher", "factory-demo-watcher"));
        let row = engine.l3.agents.get_agent("demo/watcher").await.unwrap().unwrap();
        assert_eq!(row.instance, 1);
        assert!(serde_json::to_value(&row).unwrap().get("instance").is_none(), "written exactly as before");
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn every_live_instance_spends_a_slot_of_max_sessions() {
        let (engine, _stub, root) =
            recording_engine(&format!("name: demo\npath: .\nruntime: stub\nagents:\n{THREE_WATCHERS}"));
        engine.l3_service().reconcile_agents().await;
        let cap = engine.l4_service().capacity_for("demo", "watcher", Some(5)).await.unwrap();
        assert_eq!((cap.in_use, cap.scope_in_use), (3, 3), "three standing instances, three slots");
        assert!(!cap.held(), "two left for runs");
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn scaling_up_writes_the_count_and_starts_the_new_instances() {
        let (engine, stub, root) = scaled_engine(
            "  - name: watcher   # the one we talk to\n    harness: configured\n    lifetime: permanent\n    max_sessions: 5\n",
        );
        engine.l3_service().reconcile_agents().await;
        assert_eq!(stub.started_ids(), ["demo/watcher"]);

        let report = scale(&engine, Some(3), None).await.unwrap();
        assert_eq!((report.previous, report.wanted), (1, 3));
        assert_eq!(report.started, ["demo/watcher#2", "demo/watcher#3"]);
        assert!(report.stopped.is_empty() && report.kept.is_empty());
        assert_eq!(started(&stub, 3).await, ["demo/watcher", "demo/watcher#2", "demo/watcher#3"]);

        // Persisted where the YAML is the source of truth, comment and all,
        // and live in the snapshot without a restart.
        let text = std::fs::read_to_string(config_file(&root)).unwrap();
        assert!(text.contains("  - name: watcher   # the one we talk to\n"), "{text}");
        assert!(text.contains("      max_sessions: 5\n      instances: 3\n"), "{text}");
        let live = engine.factory_snapshot();
        assert_eq!(live.scope("demo").unwrap().declared_agents()[0].instances, Some(3));

        // Relative, from the count the file now has.
        let report = scale(&engine, None, Some(1)).await.unwrap();
        assert_eq!((report.previous, report.wanted), (3, 4));
        assert_eq!(started(&stub, 4).await.len(), 4);
        assert!(std::fs::read_to_string(config_file(&root)).unwrap().contains("instances: 4\n"));
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn scaling_down_never_stops_a_busy_or_attended_instance_and_drains_it_once_idle() {
        let (engine, stub, root) = scaled_engine(THREE_WATCHERS);
        engine.l3_service().reconcile_agents().await;
        // #3 is in the middle of a turn; a person has #2's terminal open.
        stub.set_status("demo/watcher#3", RuntimeStatus::Working);
        engine.l3_service().agent_screen("demo/watcher#2").await.unwrap();

        let report = scale(&engine, Some(1), None).await.unwrap();
        assert_eq!((report.previous, report.wanted, report.up), (3, 1, 3));
        assert!(report.stopped.is_empty(), "{report:?}");
        let kept: Vec<(&str, KeptUp)> = report.kept.iter().map(|k| (k.id.as_str(), k.why)).collect();
        assert_eq!(kept, [("demo/watcher#3", KeptUp::Working), ("demo/watcher#2", KeptUp::Attached)]);
        assert!(report.summary().contains("demo/watcher#3 left up because it is working"), "{}", report.summary());
        assert!(stub.stopped_handles().is_empty(), "nothing was forced down");
        assert!(std::fs::read_to_string(config_file(&root)).unwrap().contains("instances: 1\n"));

        // #3 finishes its turn: the next supervision tick takes it. #2 is
        // still in front of somebody, so it stays.
        stub.set_status("demo/watcher#3", RuntimeStatus::Idle);
        engine.l3_service().supervise_agents().await;
        assert_eq!(stub.stopped_handles(), [StubRuntime::handle("demo/watcher#3")]);
        assert_eq!(rows(&engine).await, ["demo/watcher", "demo/watcher#2"]);
        assert!(!engine.l3.agents.get_agent("demo/watcher#2").await.unwrap().unwrap().declared, "left up, not wanted");
        // A surplus instance that dies is not brought back.
        assert_eq!(stub.starts.lock().unwrap().len(), 3);

        // Wanted again before it went: #2 is one of the count once more, and
        // brought back should it die; #3 is started afresh.
        let report = scale(&engine, Some(3), None).await.unwrap();
        assert_eq!(report.started, ["demo/watcher#3"]);
        assert_eq!(started(&stub, 4).await.last().map(String::as_str), Some("demo/watcher#3"));
        assert!(engine.l3.agents.get_agent("demo/watcher#2").await.unwrap().unwrap().declared);
        scale(&engine, Some(1), None).await.unwrap();

        // The terminal is closed: #2 goes too, and #1 is left alone.
        engine.l3.attention.forget("demo/watcher#2");
        engine.l3_service().supervise_agents().await;
        assert_eq!(rows(&engine).await, ["demo/watcher"]);
        assert_eq!(stub.stopped_handles().len(), 3);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn scaling_down_idle_instances_stops_the_highest_first() {
        let (engine, stub, root) = scaled_engine(THREE_WATCHERS);
        engine.l3_service().reconcile_agents().await;
        let report = scale(&engine, None, Some(-2)).await.unwrap();
        assert_eq!(report.stopped, ["demo/watcher#3", "demo/watcher#2"]);
        assert_eq!(report.up, 1);
        assert_eq!(
            stub.stopped_handles(),
            [StubRuntime::handle("demo/watcher#3"), StubRuntime::handle("demo/watcher#2")]
        );
        // Admission sees the slots come back at once.
        let cap = engine.l4_service().capacity_for("demo", "watcher", Some(5)).await.unwrap();
        assert_eq!(cap.in_use, 1);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_scale_no_daemon_could_keep_is_refused_with_the_file_untouched() {
        let (engine, stub, root) = scaled_engine(&format!(
            "{THREE_WATCHERS}  - name: builder\n    harness: configured\n"
        ));
        engine.l3_service().reconcile_agents().await;
        let before = std::fs::read_to_string(config_file(&root)).unwrap();
        let starts = stub.starts.lock().unwrap().len();

        let e = scale(&engine, Some(0), None).await.unwrap_err();
        assert!(e.contains("keep nothing up"), "{e}");
        let e = scale(&engine, None, Some(-3)).await.unwrap_err();
        assert!(e.contains("keep nothing up"), "{e}");
        let e = scale(&engine, Some(6), None).await.unwrap_err();
        assert!(e.contains("instances: 6 but max_sessions: 5"), "names both numbers: {e}");
        let e = match engine
            .handle_request(Request::AgentScale { scope: "demo".into(), name: "builder".into(), instances: Some(2), by: None })
            .await
        {
            Response::Error { message, .. } => message,
            other => panic!("{other:?}"),
        };
        assert!(e.contains("task agent"), "{e}");
        let e = match engine
            .handle_request(Request::AgentScale { scope: "demo".into(), name: "nobody".into(), instances: Some(2), by: None })
            .await
        {
            Response::Error { message, .. } => message,
            other => panic!("{other:?}"),
        };
        assert!(e.contains("no local agent declaration"), "{e}");

        assert_eq!(std::fs::read_to_string(config_file(&root)).unwrap(), before, "byte for byte");
        assert_eq!(stub.starts.lock().unwrap().len(), starts, "nothing started");
        assert!(stub.stopped_handles().is_empty(), "nothing stopped");
        // The same count again is not an error, and writes nothing.
        let report = scale(&engine, Some(3), None).await.unwrap();
        assert_eq!((report.previous, report.wanted, report.up), (3, 3, 3));
        assert_eq!(std::fs::read_to_string(config_file(&root)).unwrap(), before);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_gone_instance_is_brought_back_as_itself() {
        let (engine, stub, root) =
            recording_engine(&format!("name: demo\npath: .\nruntime: stub\nagents:\n{THREE_WATCHERS}"));
        engine.l3_service().reconcile_agents().await;
        stub.set_status("demo/watcher#2", RuntimeStatus::Gone);
        engine.l3_service().supervise_agents().await;
        assert_eq!(stub.started_ids().last().map(String::as_str), Some("demo/watcher#2"));
        assert_eq!(stub.starts.lock().unwrap().len(), 4, "only the one that went");
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_role_given_to_one_instance_is_worn_by_every_instance() {
        let (engine, _stub, root) =
            recording_engine(&format!("name: demo\npath: .\nruntime: stub\nagents:\n{THREE_WATCHERS}"));
        engine.l3_service().reconcile_agents().await;
        engine.l3_service().set_agent_role("demo/watcher#3", Some(Role::foreman())).await.unwrap();
        for id in ["demo/watcher", "demo/watcher#2", "demo/watcher#3"] {
            let row = engine.l3.agents.get_agent(id).await.unwrap().unwrap();
            assert_eq!((row.role.clone(), row.assigned_role.clone()), (Role::foreman(), Some(Role::foreman())), "{id}");
        }
        assert_eq!(engine.l3_service().effective_role("demo", "watcher").await, Role::foreman());
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn standing_agent_arguments_follow_the_adapter_defaults() {
        let (engine, stub, root) = recording_engine(
            "name: demo\npath: .\nruntime: stub\nagents:\n  - name: watcher\n    harness: configured\n    lifetime: permanent\n    args: [--model, opus]\n",
        );

        engine.l3_service().start_agent("demo", "watcher").await.unwrap();

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
        let standing = engine.l3_service().start_agent("demo", "Watcher").await.unwrap();
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
        assert!(engine.l4.store.get_agent(&standing.id).await.unwrap().is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn task_agent_arguments_follow_the_adapter_defaults() {
        let (engine, stub, root) = recording_engine(
            "name: demo\npath: .\nruntime: stub\nagents:\n  - name: builder\n    harness: configured\n    args: [--model, opus]\n",
        );
        let task = engine
            .l4_service()
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

        engine.l4_service().start_run(&task.id, Trigger::Manual).await;

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
        // `engine.l4_service().create()` already canonicalizes a task's scope on the way
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
            after: None,
            after_condition: None,
            id: "legacy-task".into(),
            title: "legacy run".into(),
            instructions: "true".into(),
            scope: "demo".into(), // the scope's pre-migration bare name
            agent: "shell".into(),
            runtime: "stub".into(),
            status: TaskStatus::Pending,
            schedule: None,
            estimate_seconds: None,
            estimate: None,
            result: None,
            routed_to: None,
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
            parent_task_id: None,
            decomposition_part: None,
            depends_on: Vec::new(),
            bench_origin: None,
            retry: None,
            pending_retry: None,
            schedule_paused: false,
            category: None,
            intake: None,
            failure: None,
            closure: None,
            slot_wait: None,
            deliver: None,
        };
        engine.l4.store.create(&task).await.unwrap();

        engine.l4_service().start_run(&task.id, Trigger::Manual).await;

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
            .l4_service()
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

        engine.l4_service().start_run(&task.id, Trigger::Manual).await;

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
            .l4_service()
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

        engine.l4_service().start_run(&task.id, Trigger::Manual).await;

        let guide = factory_core::adapter::agent::run_guide_path(&engine.factory_snapshot().guides_dir(), &task.id);
        assert!(guide.exists(), "still there once the harness is up and running");

        let run = engine.l4.store.active_run(&task.id).await.unwrap().unwrap();
        engine
            .l4_service()
            .report(
                &task.id,
                TaskReport {
                    artifacts: Vec::new(),
                    status: Some(RunStatus::Done),
                    message: None,
                    result: Some("ok".into()),
                    send_to: None,
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
            .l4_service()
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

        engine.l4_service().start_run(&task.id, Trigger::Manual).await;

        let guide = factory_core::adapter::agent::run_guide_path(&engine.factory_snapshot().guides_dir(), &task.id);
        assert!(guide.exists());

        let run = engine.l4.store.active_run(&task.id).await.unwrap().unwrap();
        engine.l4_service().fail_run(&run.id, factory_core::run::FailKind::AckTimeout, "gave up waiting").await;

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

        let mut bus = engine.shared.bus.subscribe();
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
