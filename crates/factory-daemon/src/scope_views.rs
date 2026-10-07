//! The Agents page (`scope_views`) and the runtime connections page. **Pages (D5):**
//! the roster (L3) composed with L4's active runs, the scopes' capacity and the adapter
//! registry. They own no state; they read through the entry point.
use crate::engine::*;

impl Engine {
    /// One probe per effective runtime connection, not one per scope. A probe
    /// that fails becomes that connection's error card; it never prevents a
    /// different runtime from reporting its own state.
    pub(crate) async fn runtime_connections(&self) -> Vec<RuntimeConnectionView> {
        let factory = self.factory_snapshot();
        let mut scopes_by_runtime: std::collections::BTreeMap<String, Vec<String>> =
            Default::default();
        for scope in &factory.config.scopes {
            let runtime = scope
                .runtime
                .clone()
                .unwrap_or_else(|| factory.config.daemon.default_runtime.clone());
            scopes_by_runtime
                .entry(runtime)
                .or_default()
                .push(scope.name.clone());
        }

        let metadata: std::collections::BTreeMap<String, (String, String)> = self
            .shared.registry
            .list()
            .adapters
            .into_iter()
            .filter(|adapter| adapter.kind == "runtime")
            .map(|adapter| (adapter.name, (adapter.source, adapter.description)))
            .collect();

        let mut views = Vec::with_capacity(scopes_by_runtime.len());
        for (runtime_name, scopes) in scopes_by_runtime {
            let checked_at = Utc::now();
            let diagnostic = match self.shared.registry.runtime(&runtime_name) {
                Ok(runtime) => runtime
                    .connection_diagnostic()
                    .await
                    .unwrap_or_else(|error| RuntimeConnectionDiagnostic::error(error.to_string())),
                Err(error) => RuntimeConnectionDiagnostic::error(error.to_string()),
            };
            let (source, description) = metadata.get(&runtime_name).cloned().unwrap_or_else(|| {
                (
                    "missing".into(),
                    "this configured runtime adapter is not registered".into(),
                )
            });
            views.push(RuntimeConnectionView {
                runtime: runtime_name,
                source,
                description,
                scopes,
                checked_at,
                diagnostic,
            });
        }
        views
    }
    pub(crate) async fn scope_views(&self) -> Result<(Vec<ScopeView>, Vec<String>)> {
        let factory = self.factory_snapshot();
        let adapters = self.shared.registry.list();
        let described: std::collections::BTreeMap<String, (String, String)> = adapters
            .adapters
            .iter()
            .filter(|a| a.kind == "agent")
            .map(|a| (a.name.clone(), (a.description.clone(), a.source.clone())))
            .collect();
        let available: Vec<String> = described.keys().cloned().collect();
        let available_stores: Vec<String> = adapters
            .adapters
            .iter()
            .filter(|a| a.kind == "task")
            .map(|a| a.name.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();

        let standing = self.l4.store.agents().await?;
        let active = self.l4.store.active_runs().await?;

        // Runs, grouped by the scope and adapter that are actually doing them.
        let mut work: std::collections::BTreeMap<(String, String), Vec<AgentActivity>> =
            Default::default();
        for run in &active {
            let task = self.l4.store.get(&run.task_id).await?;
            // Canonicalized: a task written before a scope's identity became
            // its path still carries the bare name it was given, and this is
            // what lets its active runs land on the same row as everything
            // else in that scope rather than opening an orphan one next to
            // it.
            let scope = task
                .as_ref()
                .map(|t| factory.canonical_scope_name(&t.scope))
                .unwrap_or_default();
            work.entry((scope.clone(), run.agent.clone()))
                .or_default()
                .push(AgentActivity {
                    run_id: run.id.clone(),
                    task_id: run.task_id.clone(),
                    task_title: task
                        .map(|t| t.title)
                        .unwrap_or_else(|| "(deleted task)".into()),
                    scope,
                    attempt: run.attempt,
                    status: run.status.as_str().to_string(),
                    runtime: run.runtime.clone(),
                    trigger: run.trigger.as_str().to_string(),
                    started_at: run.started_at,
                    session: run.session.as_ref().map(|s| s.handle.clone()),
                });
        }

        let instance_default = factory.config.daemon.default_agent.clone();
        let mut views = Vec::new();

        for scope in &factory.config.scopes {
            let default_agent = scope
                .agent_adapter()
                .unwrap_or(&instance_default)
                .to_string();
            let runtime = scope
                .runtime
                .clone()
                .unwrap_or_else(|| factory.config.daemon.default_runtime.clone());
            let task_store = factory.task_store_for(&scope.name).to_string();
            let scope_dir = factory
                .scope_path(&scope.name)
                .unwrap_or_else(|_| scope.path.clone());
            let (worktree_capable, worktree_reason) = self.worktree_capability(&scope.name, &scope_dir).await;

            let mut agents = Vec::new();
            let mut covered = std::collections::BTreeSet::new();
            let deletable: std::collections::BTreeSet<String> = scope
                .declared_agents()
                .into_iter()
                .map(|agent| agent.name())
                .collect();

            for decl in scope.agents_with(&factory.config.daemon.foreman) {
                let name = decl.name();
                // Runs are keyed by the name a task asked for, which is this
                // name -- not the harness behind it. Key both sides the same
                // way or live runs quietly stop appearing here.
                covered.insert(name.clone());
                let live = decl
                    .lifetime
                    .is_standing()
                    .then(|| {
                        standing
                            .iter()
                            .find(|a| a.id == AgentSession::id_for(&scope.name, &name))
                    })
                    .flatten();
                let (description, source) = described
                    .get(&decl.harness)
                    .cloned()
                    .unwrap_or_else(|| ("not registered".into(), "missing".into()));

                agents.push(AgentView {
                    id: live.map(|a| a.id.clone()),
                    name: name.clone(),
                    adapter: decl.harness.clone(),
                    description,
                    source,
                    lifetime: decl.lifetime.as_str().to_string(),
                    role: live
                        .map(|a| a.role_with(&decl.role))
                        .unwrap_or_else(|| decl.role.clone())
                        .as_str()
                        .to_string(),
                    sandbox: decl.sandbox.as_str().to_string(),
                    assigned_role: live
                        .and_then(|a| a.assigned_role.clone())
                        .map(|r| r.as_str().to_string()),
                    autostart: decl.autostart(),
                    state: live
                        .map(|a| a.state.as_str().to_string())
                        .unwrap_or_else(|| {
                            if decl.lifetime.is_standing() {
                                AgentState::Stopped.as_str().to_string()
                            } else {
                                "task".into()
                            }
                        }),
                    // "default" means a task that names no agent lands here --
                    // which is a question about the name, not the harness.
                    // Three agents sharing a harness are not all the default.
                    is_default: name == default_agent,
                    declared: true,
                    deletable: deletable.contains(&name),
                    attach: live.and_then(|a| a.attach.clone()),
                    session: live
                        .and_then(|a| a.session.as_ref())
                        .map(|s| s.handle.clone()),
                    started_at: live.map(|a| a.started_at),
                    error: live.and_then(|a| a.error.clone()),
                    active: work
                        .get(&(scope.name.clone(), name.clone()))
                        .cloned()
                        .unwrap_or_default(),
                    readiness: (decl.sandbox == Sandbox::Openshell)
                        .then(|| self.l2.provision.readiness(&(scope.name.clone(), name.clone())))
                        .flatten(),
                });
            }

            // The scope's default, when nothing above already named it.
            if !covered.contains(&default_agent) {
                let (description, source) = described
                    .get(&default_agent)
                    .cloned()
                    .unwrap_or_else(|| ("not registered".into(), "missing".into()));
                covered.insert(default_agent.clone());
                agents.insert(
                    0,
                    AgentView {
                        id: None,
                        name: default_agent.clone(),
                        adapter: default_agent.clone(),
                        description,
                        source,
                        lifetime: "task".into(),
                        role: Role::default().as_str().to_string(),
                        // Nothing declared this agent, so there is no
                        // `sandbox:` to read -- today's default, unstated.
                        sandbox: Sandbox::None.as_str().to_string(),
                        assigned_role: None,
                        autostart: false,
                        state: "task".into(),
                        is_default: true,
                        declared: false,
                        deletable: false,
                        attach: None,
                        session: None,
                        started_at: None,
                        error: None,
                        active: work
                            .get(&(scope.name.clone(), default_agent.clone()))
                            .cloned()
                            .unwrap_or_default(),
                        readiness: None,
                    },
                );
            }

            // An agent working here that the scope never declared -- somebody
            // started a task with `--agent`. It is doing work, so it belongs on
            // the page whatever the config says.
            for ((s, adapter), jobs) in &work {
                if s != &scope.name || covered.contains(adapter) {
                    continue;
                }
                let (description, source) = described
                    .get(adapter)
                    .cloned()
                    .unwrap_or_else(|| ("not registered".into(), "missing".into()));
                agents.push(AgentView {
                    id: None,
                    name: adapter.clone(),
                    adapter: adapter.clone(),
                    description,
                    source,
                    lifetime: "task".into(),
                    role: Role::default().as_str().to_string(),
                    sandbox: Sandbox::None.as_str().to_string(),
                    assigned_role: None,
                    autostart: false,
                    state: "task".into(),
                    is_default: false,
                    declared: false,
                    deletable: false,
                    attach: None,
                    session: None,
                    started_at: None,
                    error: None,
                    active: jobs.clone(),
                    readiness: None,
                });
            }

            views.push(ScopeView {
                id: scope.id.clone(),
                name: scope.name.clone(),
                path: scope_dir.display().to_string(),
                default_agent,
                runtime,
                agents,
                // Nothing today gives one scope a different roster of
                // adapters than any other, so there is no per-scope override
                // to carry -- the shared list returned alongside `views` is
                // the whole answer.
                available: None,
                task_store,
                available_stores: available_stores.clone(),
                worktree_capable,
                worktree_reason,
            });
        }

        Ok((views, available))
    }
    /// The agents page: scopes first, then the agents each one declares, then
    /// what they are doing -- and, once, every adapter registered, which
    /// belongs to the whole answer rather than to any one scope in it. One
    /// call, because a page that had to join config, adapters, standing
    /// agents, runs and tasks itself would be showing five different moments
    /// in time.
    /// One scope's worktree capability, asked of `git` at most every
    /// `CAPABILITY_TTL`. See `worktree_caps` for why this is cached at all.
    async fn worktree_capability(&self, name: &str, dir: &Path) -> (bool, Option<String>) {
        if let Some((at, answer)) = self.l4.worktree_caps.lock().unwrap().get(name) {
            if at.elapsed() < CAPABILITY_TTL {
                return answer.clone();
            }
        }
        let answer = worktree::capability(dir).await;
        self.l4.worktree_caps
            .lock()
            .unwrap()
            .insert(name.to_string(), (Instant::now(), answer.clone()));
        answer
    }
}
