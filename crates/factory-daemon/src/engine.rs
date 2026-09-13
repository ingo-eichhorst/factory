//! The core. Everything an interface can ask for arrives here as a `Request`
//! and leaves as a `Response`; the interfaces themselves hold no logic.

use chrono::Utc;
use factory_core::adapter::agent::{AgentContext, LaunchSpec, TaskBinding};
use factory_core::adapter::runtime::{
    RuntimeConnectionDiagnostic, RuntimeStatus, Screen, StartRequest, StatusReport, StatusSource,
};
use factory_core::adapter::TaskStore;
use factory_core::config::{Factory, ScopeAgent};
use factory_core::error::{FactoryError, Result};
use factory_core::event::{Event, EventBus};
use factory_core::protocol::{
    AgentActivity, AgentView, Envelope, Payload, Request, Response, RuntimeConnectionView,
    ScopeView, StatusInfo,
};
use factory_core::agent::{AgentSession, AgentState};
use factory_core::role::{Role, Roles};
use factory_core::run::{BlockSource, NewRun, Run, RunPatch, RunStatus, Trigger};
use factory_core::task::{
    NewTask, Task, TaskEntry, TaskFilter, TaskPatch, TaskReport, TaskStatus,
};
use factory_plugins::registry::Registry;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::schedule;
use crate::worktree;

/// How long a scope's worktree capability is trusted for. Shorter than
/// `site::WALK_TTL`: a person who just ran `git init` in a scope to make the
/// checkbox available should not have to wait five minutes to see it, and the
/// answer is two `git` calls rather than a directory walk.
const CAPABILITY_TTL: Duration = Duration::from_secs(60);

/// Put declaration-specific arguments after the adapter's defaults. Harnesses
/// generally let the last occurrence of a flag win, so this ordering lets one
/// scope override an adapter default as well as add to it.
pub(crate) fn append_declared_args(launch: &mut LaunchSpec, declaration: Option<&ScopeAgent>) {
    if let Some(declaration) = declaration {
        launch.args.extend(declaration.args.iter().cloned());
    }
}

pub struct Engine {
    /// The instance settings are stable, while a successful scope-config edit
    /// replaces the affected scope in this snapshot. Readers clone it before
    /// awaiting so no filesystem or runtime operation holds the lock.
    factory: std::sync::RwLock<Factory>,
    /// Serializes read-modify-write edits to local scope config files.
    pub(crate) configuration_edit: std::sync::Mutex<()>,
    pub registry: Registry,
    /// Every role this instance knows, resolved once. Nothing asks the config
    /// again: two answers to "what may this agent do" is how they drift.
    pub roles: Roles,
    pub store: Arc<dyn TaskStore>,
    pub bus: EventBus,
    pub factory_bin: PathBuf,
    started: Instant,
    interfaces: Vec<String>,
    /// The last liveness we wrote down for each session, so a poll that finds
    /// no change writes nothing. Lost on restart, which is right: after a
    /// restart the first observation is genuinely new information.
    pub(crate) seen_status: std::sync::Mutex<std::collections::HashMap<String, RuntimeStatus>>,
    /// The last walk of each scope's directory, with when it was taken. The
    /// site view asks for a scope's size on every run and agent event now, and
    /// a repository does not change size between two of them -- see
    /// `site::WALK_TTL`.
    pub(crate) site_walks:
        std::sync::Mutex<std::collections::HashMap<String, (Instant, crate::site::Measured)>>,
    /// Whether each scope's directory can host a worktree, with when that was
    /// asked. `worktree::capability` is one or two `git` subprocesses, and
    /// `scope_views` asks it per configured scope, potentially many on a real
    /// instance, on an endpoint the
    /// Agents view refetches on every run and agent event. A directory does
    /// not become a git repository between two of those. Cached for
    /// `CAPABILITY_TTL`; the first board after a restart still pays in full,
    /// the same trade `site::WALK_TTL` already makes.
    pub(crate) worktree_caps:
        std::sync::Mutex<std::collections::HashMap<String, (Instant, (bool, Option<String>))>>,
    /// The tier and activity level each hall was last drawn at, which is what
    /// makes both steps sticky instead of flipping whenever a metric sits on a
    /// threshold. Lost on restart, like `seen_status`, and for the same
    /// reason: the first answer after one is genuinely new.
    pub(crate) site_memory: std::sync::Mutex<
        std::collections::HashMap<
            String,
            (
                factory_core::building::Tier,
                factory_core::building::ActivityLevel,
            ),
        >,
    >,
}

impl Engine {
    pub fn new(
        factory: Factory,
        registry: Registry,
        store: Arc<dyn TaskStore>,
        factory_bin: PathBuf,
        interfaces: Vec<String>,
    ) -> Self {
        // `Factory::load` refuses a config whose roles do not resolve, so this
        // can only fail for an instance assembled in code. Say so and carry on
        // with the two that ship rather than taking the daemon down.
        let roles = factory.config.roles().unwrap_or_else(|e| {
            tracing::error!("{e}; falling back to the built-in roles");
            Roles::presets()
        });
        Self {
            factory: std::sync::RwLock::new(factory),
            configuration_edit: Default::default(),
            registry,
            roles,
            store,
            bus: EventBus::default(),
            factory_bin,
            started: Instant::now(),
            interfaces,
            seen_status: Default::default(),
            site_walks: Default::default(),
            worktree_caps: Default::default(),
            site_memory: Default::default(),
        }
    }

    /// A coherent configuration snapshot for one operation. A poisoned lock
    /// still contains the last value; recovering it keeps a failed request
    /// from taking the daemon down with it.
    pub(crate) fn factory_snapshot(&self) -> Factory {
        self.factory
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(crate) fn replace_scope(&self, id: &str, replacement: factory_core::config::Scope) {
        let mut factory = self
            .factory
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(scope) = factory.config.scopes.iter_mut().find(|scope| scope.id == id) {
            *scope = replacement;
        }
    }

    // -- the API every interface speaks ------------------------------------

    /// Every request goes through here: who is asking, may they, then do it.
    pub async fn handle(self: &Arc<Self>, envelope: Envelope) -> Response {
        let caller = match self.caller_for(envelope.token.as_deref()).await {
            Ok(c) => c,
            Err(e) => return Response::error(e.code(), e.to_string()),
        };
        let request = self.bind_caller(&caller, envelope.request);
        if let Err(e) = self.authorize(&caller, &request).await {
            return Response::error(e.code(), e.to_string());
        }
        match self.dispatch_request(request).await {
            Ok(payload) => Response::ok(payload),
            Err(e) => Response::error(e.code(), e.to_string()),
        }
    }

    /// An agent's own scope is the one it means. A foreman that creates a task
    /// without naming a scope means its own, not the instance's first.
    fn bind_caller(&self, caller: &crate::access::Caller, request: Request) -> Request {
        let Some(scope) = caller.scope() else {
            return request;
        };
        match request {
            Request::TaskCreate(mut new) => {
                if new.scope.is_none() {
                    new.scope = Some(scope.to_string());
                }
                Request::TaskCreate(new)
            }
            other => other,
        }
    }

    /// For callers inside the daemon, which are always the owner.
    pub async fn handle_request(self: &Arc<Self>, request: Request) -> Response {
        self.handle(Envelope::from(request)).await
    }

    async fn dispatch_request(self: &Arc<Self>, req: Request) -> Result<Payload> {
        match req {
            Request::Status => Ok(Payload::Status {
                status: self.status().await?,
            }),
            Request::Adapters => Ok(self.registry.list().into()),
            Request::RuntimeConnections => Ok(Payload::RuntimeConnections {
                runtimes: self.runtime_connections().await,
            }),
            Request::Agents => {
                let (scopes, available) = self.scope_views().await?;
                Ok(Payload::Scopes {
                    scopes,
                    available,
                    roles: self.role_views(),
                })
            }
            Request::Occupancy { minutes } => Ok(Payload::Occupancy {
                occupancy: self.occupancy(minutes).await?,
            }),
            Request::Production { minutes, bin, scope } => Ok(Payload::Production {
                production: self.production(minutes, bin, scope).await?,
            }),
            Request::SiteFootprint => Ok(Payload::SiteFootprint {
                footprint: self.site_footprint().await?,
            }),
            Request::AgentStart { scope, name } => Ok(Payload::Agent {
                agent: self.start_agent(&scope, &name).await?.redacted(),
            }),
            Request::AgentConfigure { scope, agent } => {
                let (scope, agent) = self.configure_agent(&scope, agent)?;
                let name = agent.name();
                let autostart = agent.lifetime.is_standing() && agent.autostart();
                self.bus.publish(Event::AgentConfigured {
                    scope: scope.clone(),
                    name: name.clone(),
                });
                if autostart {
                    let engine = self.clone();
                    tokio::spawn(async move {
                        if let Err(error) = engine.start_agent(&scope, &name).await {
                            tracing::warn!(scope, name, "could not start newly configured agent: {error}");
                        }
                    });
                }
                Ok(Payload::Ok)
            }
            Request::AgentDelete { scope, name } => {
                let (scope, removed) = self.delete_agent_declaration(&scope, &name)?;
                let name = removed.name();
                self.bus.publish(Event::AgentDeleted { scope, name });
                self.reconcile_agents().await;
                Ok(Payload::Deleted { deleted: true })
            }
            Request::AgentStop { id } => Ok(Payload::Agent {
                agent: self.stop_agent(&id).await?.redacted(),
            }),
            Request::AgentRole { id, role } => Ok(Payload::Agent {
                agent: self
                    .set_agent_role(&id, role.map(Role::new))
                    .await?
                    .redacted(),
            }),
            Request::AgentInput { id, text, keys } => {
                self.agent_input(&id, text.as_deref(), &keys).await?;
                Ok(Payload::Ok)
            }
            Request::AgentOutput { id, lines } => Ok(Payload::Text {
                text: self.agent_output(&id, lines.unwrap_or(200)).await?,
            }),
            Request::AgentScreen { id } => match self.agent_screen(&id).await? {
                Some(screen) => Ok(Payload::Screen { screen }),
                None => Err(FactoryError::BadRequest(
                    "this runtime cannot render a screen for that session".into(),
                )),
            },
            Request::RunScreen { id } => {
                let run = self.require_run(&id).await?;
                match self.run_screen(&run).await? {
                    Some(screen) => Ok(Payload::Screen { screen }),
                    None => Err(FactoryError::BadRequest(
                        "this run has no session to show".into(),
                    )),
                }
            }
            Request::RunInput { id, text, keys } => {
                self.run_input(&id, text.as_deref(), &keys).await?;
                Ok(Payload::Ok)
            }

            Request::TaskCreate(new) => Ok(Payload::Task {
                task: self.create(new).await?,
            }),
            Request::TaskGet { id } => Ok(Payload::Task {
                task: self.require(&id).await?,
            }),
            Request::TaskList(filter) => Ok(Payload::Tasks {
                tasks: self.store.list(&filter).await?,
            }),
            Request::TaskUpdate { id, patch } => Ok(Payload::Task {
                task: self.update(&id, patch).await?,
            }),
            Request::TaskDelete { id } => {
                if let Some(run) = self.store.active_run(&id).await? {
                    self.close_session(&run).await;
                }
                let deleted = self.store.delete(&id).await?;
                if deleted {
                    self.bus.publish(Event::TaskDeleted { id });
                }
                Ok(Payload::Deleted { deleted })
            }
            Request::TaskRun { id } => {
                self.require(&id).await?;
                // A task is a standing intent; a run is one attempt at it. Two
                // attempts at once would race for the same working directory.
                if let Some(run) = self.store.active_run(&id).await? {
                    return Err(FactoryError::BadRequest(format!(
                        "attempt {} of this task is still {}; cancel it before starting another",
                        run.attempt,
                        run.status.as_str()
                    )));
                }
                let engine = self.clone();
                // Dispatch can take a minute: opening a pane, waiting for an
                // agent to be ready. The caller gets its answer now.
                tokio::spawn(async move {
                    engine.start_run(&id, Trigger::Manual).await;
                });
                Ok(Payload::Ok)
            }
            Request::TaskCancel { id } => {
                let run = self.store.active_run(&id).await?.ok_or_else(|| {
                    FactoryError::BadRequest(format!("task {id} has no run to cancel"))
                })?;
                self.close_session(&run).await;
                let run = self
                    .finish_run(
                        &run.id,
                        RunStatus::Cancelled,
                        RunPatch {
                            status: Some(RunStatus::Cancelled),
                            ..Default::default()
                        },
                        "cancelled by request",
                    )
                    .await?;
                Ok(Payload::Run { run: run.redacted() })
            }
            Request::TaskReport { id, report } => Ok(Payload::Run {
                run: self.report(&id, report).await?.redacted(),
            }),
            Request::TaskEntries { id, limit } => Ok(Payload::Entries {
                entries: self.store.entries(&id, limit.unwrap_or(200)).await?,
            }),
            Request::TaskOutput { id, lines } => {
                let latest = self.store.runs(&id, 1).await?.into_iter().next();
                Ok(Payload::Text {
                    text: match latest {
                        Some(run) => self.output(&run, lines.unwrap_or(200)).await,
                        None => String::new(),
                    },
                })
            }

            Request::RunList { task_id, limit } => Ok(Payload::Runs {
                runs: self
                    .store
                    .runs(&task_id, limit.unwrap_or(50))
                    .await?
                    .into_iter()
                    .map(|r| r.redacted())
                    .collect(),
            }),
            Request::RunGet { id } => Ok(Payload::Run {
                run: self.require_run(&id).await?.redacted(),
            }),
            Request::RunEntries { id, limit } => Ok(Payload::Entries {
                entries: self.store.run_entries(&id, limit.unwrap_or(200)).await?,
            }),
            Request::RunOutput { id, lines } => {
                let run = self.require_run(&id).await?;
                Ok(Payload::Text {
                    text: self.output(&run, lines.unwrap_or(200)).await,
                })
            }

            Request::Subscribe => Err(FactoryError::BadRequest(
                "this interface does not stream events on the request channel".into(),
            )),
        }
    }

    async fn status(&self) -> Result<StatusInfo> {
        let factory = self.factory_snapshot();
        let tasks = self.store.list(&TaskFilter::default()).await?;
        Ok(StatusInfo {
            instance: factory.config.instance.name.clone(),
            instance_id: factory.config.instance.id.clone(),
            root: factory.root.display().to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_seconds: self.started.elapsed().as_secs(),
            tasks_total: tasks.len(),
            tasks_active: self.store.active_runs().await?.len(),
            subscribers: self.bus.subscriber_count(),
            interfaces: self.interfaces.clone(),
            scopes: factory.scope_names(),
        })
    }

    /// One probe per effective runtime connection, not one per scope. A probe
    /// that fails becomes that connection's error card; it never prevents a
    /// different runtime from reporting its own state.
    async fn runtime_connections(&self) -> Vec<RuntimeConnectionView> {
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
            .registry
            .list()
            .adapters
            .into_iter()
            .filter(|adapter| adapter.kind == "runtime")
            .map(|adapter| (adapter.name, (adapter.source, adapter.description)))
            .collect();

        let mut views = Vec::with_capacity(scopes_by_runtime.len());
        for (runtime_name, scopes) in scopes_by_runtime {
            let checked_at = Utc::now();
            let diagnostic = match self.registry.runtime(&runtime_name) {
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

    /// The agents page: scopes first, then the agents each one declares, then
    /// what they are doing -- and, once, every adapter registered, which
    /// belongs to the whole answer rather than to any one scope in it. One
    /// call, because a page that had to join config, adapters, standing
    /// agents, runs and tasks itself would be showing five different moments
    /// in time.
    /// One scope's worktree capability, asked of `git` at most every
    /// `CAPABILITY_TTL`. See `worktree_caps` for why this is cached at all.
    async fn worktree_capability(&self, name: &str, dir: &Path) -> (bool, Option<String>) {
        if let Some((at, answer)) = self.worktree_caps.lock().unwrap().get(name) {
            if at.elapsed() < CAPABILITY_TTL {
                return answer.clone();
            }
        }
        let answer = worktree::capability(dir).await;
        self.worktree_caps
            .lock()
            .unwrap()
            .insert(name.to_string(), (Instant::now(), answer.clone()));
        answer
    }

    pub(crate) async fn scope_views(&self) -> Result<(Vec<ScopeView>, Vec<String>)> {
        let factory = self.factory_snapshot();
        let adapters = self.registry.list();
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

        let standing = self.store.agents().await?;
        let active = self.store.active_runs().await?;

        // Runs, grouped by the scope and adapter that are actually doing them.
        let mut work: std::collections::BTreeMap<(String, String), Vec<AgentActivity>> =
            Default::default();
        for run in &active {
            let task = self.store.get(&run.task_id).await?;
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

    /// Every role this instance knows, for a roster and for a picker.
    pub fn role_views(&self) -> Vec<factory_core::protocol::RoleView> {
        self.roles
            .all()
            .map(|r| factory_core::protocol::RoleView {
                name: r.name.as_str().to_string(),
                describe: r.describe.clone(),
                grants: r.written().into_iter().map(str::to_string).collect(),
                reach: r.reach.as_str().to_string(),
            })
            .collect()
    }

    // -- naming an agent ----------------------------------------------------

    /// Turn what a task asked for into the agent it will actually run as.
    ///
    /// A task names a concrete agent in its scope -- `assistant`, `scratch` --
    /// and the adapter behind it follows from the config. An adapter name
    /// still works for a scope that declares nothing, or for a one-off with
    /// `--agent claude-code`.
    pub fn resolve_agent(
        &self,
        scope_name: &str,
        name: &str,
    ) -> Result<(String, String, Option<ScopeAgent>)> {
        let factory = self.factory_snapshot();
        let scope = factory.scope(scope_name)?;
        let declared_here = scope.agents_with(&factory.config.daemon.foreman);
        if let Some(declared) = declared_here.iter().find(|a| a.name() == name).cloned() {
            // The name resolves; the adapter behind it still has to exist.
            self.registry.agent(&declared.harness)?;
            return Ok((
                declared.name(),
                declared.harness.clone(),
                Some(declared),
            ));
        }
        if self.registry.agent(name).is_ok() {
            return Ok((name.to_string(), name.to_string(), None));
        }

        let declared: Vec<String> = declared_here.iter().map(|a| a.name()).collect();
        let adapters: Vec<String> = self
            .registry
            .list()
            .adapters
            .into_iter()
            .filter(|a| a.kind == "agent")
            .map(|a| a.name)
            .collect();
        Err(FactoryError::BadRequest(format!(
            "scope {scope_name:?} has no agent named {name:?}. It declares: {}. \
             Any adapter also works: {}.",
            if declared.is_empty() { "none".into() } else { declared.join(", ") },
            adapters.join(", "),
        )))
    }

    /// Edit a task. Anything that has to stay true of a task is checked here
    /// rather than in the store: an agent that does not resolve, a scope that
    /// does not exist, and -- the one that is easy to miss -- a schedule set
    /// after creation, which would otherwise never fire because nothing
    /// recomputed when it is next due.
    pub async fn update(&self, id: &str, mut patch: TaskPatch) -> Result<Task> {
        let factory = self.factory_snapshot();
        let current = self.require(id).await?;
        // The bookkeeping is the daemon's, not a caller's.
        patch.runs = None;
        if patch.estimate_seconds == Some(0) {
            return Err(FactoryError::BadRequest(
                "a task estimate must be at least one second".into(),
            ));
        }

        let scope = patch.scope.clone().unwrap_or_else(|| current.scope.clone());
        if patch.scope.is_some() {
            // Store the identity the scope actually has, not necessarily the
            // one the caller typed -- a bare name from before scopes had
            // paths still resolves (`Factory::scope`'s fallback), but writing
            // it back down unchanged would keep manufacturing the very
            // ambiguity that fallback exists to paper over.
            patch.scope = Some(factory.scope(&scope)?.name.clone());
        }
        match &patch.agent {
            Some(agent) => {
                let (name, _, _) = self.resolve_agent(&scope, agent)?;
                patch.agent = Some(name);
            }
            // A task moved to another scope must still have an agent there.
            None if patch.scope.is_some() => {
                self.resolve_agent(&scope, &current.agent)?;
            }
            None => {}
        }
        if let Some(rt) = &patch.runtime {
            self.registry.runtime(rt)?;
        }
        if let Some(s) = &patch.schedule {
            patch.next_run_at = Some(schedule::next_after(s, Utc::now())?);
        }

        let task = self.store.update(id, &patch).await?;
        self.bus.publish(Event::TaskUpdated { task: task.clone() });
        Ok(task)
    }

    // -- creating ----------------------------------------------------------

    pub async fn create(&self, new: NewTask) -> Result<Task> {
        let factory = self.factory_snapshot();
        if new.title.trim().is_empty() {
            return Err(FactoryError::BadRequest("a task needs a title".into()));
        }
        if new.estimate_seconds == Some(0) {
            return Err(FactoryError::BadRequest(
                "a task estimate must be at least one second".into(),
            ));
        }

        let scope = match new.scope.clone() {
            Some(s) => s,
            None => factory
                .config
                .scopes
                .first()
                .map(|s| s.name.clone())
                .ok_or_else(|| {
                    FactoryError::BadRequest("no scope given and the instance declares none".into())
                })?,
        };
        let declared = factory.scope(&scope)?.clone();

        let agent = new
            .agent
            .clone()
            .or_else(|| declared.agent_adapter().map(str::to_string))
            .unwrap_or_else(|| factory.config.daemon.default_agent.clone());
        let runtime = new
            .runtime
            .clone()
            .or_else(|| declared.runtime.clone())
            .unwrap_or_else(|| factory.config.daemon.default_runtime.clone());

        // Refuse now, with the list of what this scope offers, rather than at
        // dispatch time when whoever asked has stopped watching.
        let (agent, _adapter, _) = self.resolve_agent(&scope, &agent)?;
        self.registry.runtime(&runtime)?;

        // The scope's canonical identity, not necessarily what the caller
        // typed -- `declared` is already resolved through the bare-name
        // fallback above, and storing its own name keeps a freshly created
        // task from starting life needing that fallback itself.
        let mut task = factory_core::adapter::store::task_from_new(new, declared.name.clone(), agent, runtime);
        if let Some(s) = &task.schedule {
            task.next_run_at = Some(schedule::next_after(s, Utc::now())?);
        }

        let task = self.store.create(&task).await?;
        self.entry(
            &task.id,
            TaskEntry::new("daemon", "created", format!("created: {}", task.title)),
        )
        .await;
        self.bus.publish(Event::TaskCreated { task: task.clone() });
        Ok(task)
    }

    // -- running -----------------------------------------------------------

    /// Start one attempt at a task and hand it to an agent. Failures here end
    /// the run rather than escaping, because nobody is waiting on the answer.
    pub async fn start_run(self: &Arc<Self>, task_id: &str, trigger: Trigger) {
        let run = match self.dispatch(task_id, trigger).await {
            Ok(run) => run,
            Err(e) => {
                // The run may or may not exist yet; if it does, close it.
                if let Ok(Some(run)) = self.store.active_run(task_id).await {
                    self.fail_run(&run.id, &format!("dispatch failed: {e}")).await;
                } else {
                    self.entry(
                        task_id,
                        TaskEntry::new("daemon", "failed", format!("dispatch failed: {e}")),
                    )
                    .await;
                    let _ = self
                        .store
                        .update(
                            task_id,
                            &TaskPatch {
                                status: Some(TaskStatus::Failed),
                                error: Some(e.to_string()),
                                ..Default::default()
                            },
                        )
                        .await;
                }
                return;
            }
        };
        tracing::info!(task = task_id, run = %run.id, attempt = run.attempt, "dispatched");
    }

    async fn dispatch(self: &Arc<Self>, task_id: &str, trigger: Trigger) -> Result<Run> {
        let task = self.require(task_id).await?;
        // Resolve again rather than trusting what was written down: the config
        // may have changed since the task was created.
        let (agent_name, adapter_name, declaration) =
            self.resolve_agent(&task.scope, &task.agent)?;
        let agent = self.registry.agent(&adapter_name)?;
        let runtime = self.registry.runtime(&task.runtime)?;
        let factory = self.factory_snapshot();
        let scope_path = factory.scope_path(&task.scope)?;
        if !scope_path.is_dir() {
            return Err(FactoryError::BadRequest(format!(
                "scope {:?} points at {}, which is not a directory",
                task.scope,
                scope_path.display()
            )));
        }

        let token = factory_core::new_token();
        let run = self
            .store
            .create_run(&NewRun {
                task_id: task.id.clone(),
                trigger,
                agent: agent_name.clone(),
                adapter: adapter_name.clone(),
                runtime: task.runtime.clone(),
                token: token.clone(),
            })
            .await?;

        self.bus.publish(Event::RunStarted { run: run.clone() });
        self.publish_task(task_id).await;
        self.entry(
            task_id,
            TaskEntry::new(
                "daemon",
                "started",
                format!("attempt {} started ({})", run.attempt, trigger.as_str()),
            )
            .in_run(&run.id),
        )
        .await;

        // A worktree of its own, made now rather than left to the harness --
        // the run row already exists, so it is named after it. Nothing below
        // this point may hand the agent the scope itself when the checkbox is
        // on: a failure here ends the run right here, with git's own
        // complaint, rather than quietly falling back to the scope.
        let (cwd, run) = self.place_run(&task, run, &scope_path).await?;

        // Resolved the same way `caller_for` resolves it for every other
        // request, off the agent this run actually landed on rather than
        // whatever the task's own record says -- `resolve_agent` may have
        // fallen back to a bare adapter name the task did not ask for.
        let role = self.effective_role(&task.scope, &agent_name).await;
        let role = self.roles.get(&role).cloned();

        let ctx = AgentContext {
            scope: task.scope.clone(),
            agent_name: agent_name.clone(),
            cwd: cwd.clone(),
            factory_bin: self.factory_bin.clone(),
            socket: factory.socket_path(),
            guides_dir: factory.guides_dir(),
            task: Some(TaskBinding {
                task: task.clone(),
                run_id: run.id.clone(),
                attempt: run.attempt,
                token,
                worktree_branch: run.worktree_branch.clone(),
            }),
            identity_token: None,
            role,
        };

        let mut launch = agent.launch_spec(&ctx).await?;
        append_declared_args(&mut launch, declaration.as_ref());
        let session = runtime
            .start(&StartRequest {
                id: run.id.clone(),
                name: format!("factory-run-{}", &run.id[..8.min(run.id.len())]),
                label: format!("factory: {}", truncate(&task.title, 40)),
                cwd,
                launch,
            })
            .await?;

        let run = self
            .store
            .update_run(
                &run.id,
                &RunPatch {
                    session: Some(session.clone()),
                    ..Default::default()
                },
            )
            .await?;
        self.bus.publish(Event::RunUpdated { run: run.clone() });

        let prompt = agent.prompt(&ctx).await?;
        runtime.submit(&session, &prompt).await?;

        self.entry(
            task_id,
            TaskEntry::new(
                "daemon",
                "dispatched",
                format!("handed to {agent_name} ({adapter_name}) on {}", session.runtime),
            )
            .in_run(&run.id)
            .with_data(serde_json::json!({ "session": session })),
        )
        .await;
        Ok(run)
    }

    /// Where a run actually works: its own worktree, or the scope directly.
    /// Pulled out of `dispatch` so the decision -- and the one way it can
    /// fail -- has no need of a real agent or runtime on the other end of it,
    /// which is what lets it be tested on its own.
    ///
    /// `task.worktree` off is the whole of the "quietly ignored" case this
    /// function refuses to have: it is checked once, here, and every path out
    /// of it either returns the scope path unchanged or a worktree that
    /// `git worktree add` actually made. There is no third path.
    async fn place_run(&self, task: &Task, run: Run, scope_path: &Path) -> Result<(PathBuf, Run)> {
        if !task.worktree {
            return Ok((scope_path.to_path_buf(), run));
        }
        let branch = worktree::branch_name(&task.id, &task.title, run.attempt);
        let dir = self.factory_snapshot().worktrees_dir().join(&run.id);
        worktree::create(scope_path, &dir, &branch)
            .await
            .map_err(|e| FactoryError::adapter("git", e))?;
        let run = self
            .store
            .update_run(
                &run.id,
                &RunPatch {
                    worktree_path: Some(dir.display().to_string()),
                    worktree_branch: Some(branch.clone()),
                    ..Default::default()
                },
            )
            .await?;
        self.bus.publish(Event::RunUpdated { run: run.clone() });
        self.entry(
            &run.task_id,
            TaskEntry::new(
                "daemon",
                "worktree",
                format!("working in {} on {branch}", dir.display()),
            )
            .in_run(&run.id),
        )
        .await;
        Ok((dir, run))
    }

    /// What an agent says about its own run. The token is what makes this a
    /// report rather than anyone on the socket closing anyone's run.
    pub async fn report(&self, task_id: &str, report: TaskReport) -> Result<Run> {
        let run = self.store.active_run(task_id).await?.ok_or_else(|| {
            FactoryError::BadRequest(format!(
                "task {task_id} has no run in progress; reports are no longer accepted"
            ))
        })?;

        if let Some(expected) = &run.token {
            match &report.token {
                Some(given) if given == expected => {}
                Some(_) => {
                    return Err(FactoryError::Denied(format!(
                        "wrong token for attempt {} of task {task_id}",
                        run.attempt
                    )))
                }
                None => {
                    return Err(FactoryError::Denied(format!(
                        "attempt {} of task {task_id} needs its run token; it is FACTORY_TASK_TOKEN in the session, or pass --run-token",
                        run.attempt
                    )))
                }
            }
        }

        let message = report.message.clone().unwrap_or_else(|| {
            report
                .result
                .clone()
                .or_else(|| report.error.clone())
                .unwrap_or_else(|| "(no message)".into())
        });

        self.entry(
            task_id,
            TaskEntry::new(
                "agent",
                report
                    .status
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_else(|| "note".into()),
                message,
            )
            .in_run(&run.id),
        )
        .await;

        let mut patch = RunPatch {
            status: report.status,
            result: report.result,
            error: report.error,
            ..Default::default()
        };

        // The agent's own report is the one thing that may set or clear
        // `Blocked` honestly for its own sake -- see `AGENTS.md` and issue
        // #7. Reporting `blocked` again while already blocked leaves
        // `blocked_since` alone, so the clock still reads from when the
        // block actually began; reporting anything else always lets go of
        // it, agent-set or not, because this report is the agent speaking.
        match report.status {
            Some(RunStatus::Blocked) => {
                patch.blocked_source = Some(BlockSource::Agent);
                if run.status != RunStatus::Blocked {
                    patch.blocked_since = Some(Utc::now());
                }
            }
            Some(_) => patch.clear_blocked = true,
            None => {}
        }

        match report.status {
            Some(status) if status.is_terminal() => {
                self.close_session(&run).await;
                self.finish_run(&run.id, status, patch, &format!("attempt {} ended", run.attempt))
                    .await
            }
            _ => {
                let run = self.store.update_run(&run.id, &patch).await?;
                self.bus.publish(Event::RunUpdated { run: run.clone() });
                self.mirror_to_task(&run).await;
                Ok(run)
            }
        }
    }

    /// End a run and settle the task behind it. A task with a schedule goes
    /// back to `pending` so the scheduler will pick it up again; one without
    /// keeps the run's own outcome.
    async fn finish_run(
        &self,
        run_id: &str,
        status: RunStatus,
        patch: RunPatch,
        _why: &str,
    ) -> Result<Run> {
        let run = self
            .store
            .update_run(
                run_id,
                &RunPatch {
                    status: Some(status),
                    clear_session: true,
                    clear_token: true,
                    // A run that has ended is not waiting on anybody, so the
                    // block's own clock and the runtime's standing guess both
                    // go with the session -- `blocked_since` is documented to
                    // be `None` whenever the status is not `Blocked`, and a
                    // finished run is the one path that could otherwise leave
                    // it set. Forced here rather than left to `patch`: every
                    // terminal status comes through this function, and only
                    // the agent's own report remembered to clear it.
                    clear_blocked: true,
                    clear_block_suspicion: true,
                    ended_at: Some(Utc::now()),
                    ..patch
                },
            )
            .await?;
        self.bus.publish(Event::RunUpdated { run: run.clone() });
        // The run's session is released here, so nothing will poll it again.
        // Close its liveness span now or the chart draws the agent as still
        // working, forever.
        if let Ok(Some(task)) = self.store.get(&run.task_id).await {
            self.record_gone(&format!("run:{}", run.id), &task.scope, &run.agent)
                .await;
        }
        self.mirror_to_task(&run).await;
        Ok(run)
    }

    /// The task's own row carries the latest run's outcome, so a list does not
    /// have to read every run.
    ///
    /// `pub(crate)`: `occupancy::record_run_liveness` mirrors a run it just
    /// moved into or out of `Blocked` the same way `report` does here --
    /// the same pattern as `record_gone`, which already crosses this
    /// boundary the other way.
    pub(crate) async fn mirror_to_task(&self, run: &Run) {
        let recurring = self
            .store
            .get(&run.task_id)
            .await
            .ok()
            .flatten()
            .and_then(|t| t.schedule)
            .is_some();

        let status = if run.status.is_terminal() && recurring {
            TaskStatus::Pending
        } else {
            run.status.as_task_status()
        };

        if run.status.is_terminal() && recurring {
            self.entry(
                &run.task_id,
                TaskEntry::new(
                    "daemon",
                    "rearmed",
                    "recurring task is pending again, waiting for its next turn",
                ),
            )
            .await;
        }

        // The task mirrors the newest run, not the union of every run: an
        // attempt that succeeded must not leave the previous one's error
        // standing next to its own result.
        let patch = TaskPatch {
            status: Some(status),
            result: run.result.clone(),
            clear_result: run.result.is_none(),
            error: run.error.clone(),
            clear_error: run.error.is_none(),
            ..Default::default()
        };
        if let Ok(task) = self.store.update(&run.task_id, &patch).await {
            self.bus.publish(Event::TaskUpdated { task });
        }
    }

    /// A run that will never report back.
    pub async fn fail_run(&self, run_id: &str, why: &str) {
        let Ok(run) = self.require_run(run_id).await else {
            return;
        };
        self.close_session(&run).await;
        self.entry(
            &run.task_id,
            TaskEntry::new("daemon", "failed", why.to_string()).in_run(run_id),
        )
        .await;
        let _ = self
            .finish_run(
                run_id,
                RunStatus::Failed,
                RunPatch {
                    error: Some(why.to_string()),
                    ..Default::default()
                },
                why,
            )
            .await;
    }

    /// Keep the last of what the agent saw, then let the session go. Every
    /// path that ends a run -- a terminal report, a cancel, a task deleted
    /// out from under an active run, or the watchdog giving up on it --
    /// comes through here, which is what makes this the one place that
    /// actually knows a run is over rather than guessing from one caller's
    /// reason for closing it.
    async fn close_session(&self, run: &Run) {
        // The guide file, if this run's harness wrote one, is named after the
        // task rather than the run and nothing else removes it. It cannot be
        // deleted right after launch: `opencode` re-resolves its configured
        // instruction paths on every request rather than reading them once at
        // startup, so an early delete would make the guide silently vanish
        // partway through the run instead of lasting it. Here, once the run
        // is actually over, is safe for every harness. One that carried the
        // guide as inline text or not at all (`codex`, `shell`) never wrote a
        // file, so this is a harmless no-op for those, and a run whose
        // session never even came up still gets whatever `launch_spec`
        // managed to write before it failed cleaned up.
        let guide = self.factory_snapshot().guides_dir().join(format!("run-{}.md", run.task_id));
        let _ = std::fs::remove_file(guide);

        let Some(session) = &run.session else { return };
        if let Ok(runtime) = self.registry.runtime(&session.runtime) {
            if let Ok(text) = runtime.read(session, 400).await {
                if !text.trim().is_empty() {
                    self.entry(
                        &run.task_id,
                        TaskEntry::new("daemon", "transcript", "final terminal output")
                            .in_run(&run.id)
                            .with_data(serde_json::json!({ "text": text })),
                    )
                    .await;
                }
            }
            let _ = runtime.stop(session).await;
        }
    }

    /// Terminal output for a run: live while it is running, the transcript kept
    /// at the end once it is not, and an empty string in the moment between a
    /// run starting and its session existing. Never an error -- a view that
    /// polls this should show a blank pane, not a red one.
    pub async fn output(&self, run: &Run, lines: u32) -> String {
        if let Some(session) = &run.session {
            if let Ok(runtime) = self.registry.runtime(&session.runtime) {
                if let Ok(text) = runtime.read(session, lines).await {
                    return text;
                }
            }
        }
        let entries = self.store.run_entries(&run.id, 500).await.unwrap_or_default();
        for entry in entries.iter().rev() {
            if entry.kind == "transcript" {
                if let Some(text) = entry
                    .data
                    .as_ref()
                    .and_then(|d| d.get("text"))
                    .and_then(|v| v.as_str())
                {
                    return text.to_string();
                }
            }
        }
        String::new()
    }

    // -- what the scheduler needs ------------------------------------------

    pub async fn due_now(&self) -> Result<Vec<Task>> {
        self.store.due(Utc::now()).await
    }

    pub async fn active_runs(&self) -> Result<Vec<Run>> {
        self.store.active_runs().await
    }

    /// Move a scheduled task's next firing forward so it is not picked up twice
    /// while it runs.
    pub async fn advance_schedule(&self, task: &Task) -> Result<()> {
        let Some(s) = &task.schedule else {
            return Ok(());
        };
        let next = schedule::next_after(s, Utc::now())?;
        let updated = self
            .store
            .update(
                &task.id,
                &TaskPatch {
                    next_run_at: Some(next),
                    ..Default::default()
                },
            )
            .await?;
        self.bus.publish(Event::TaskUpdated { task: updated });
        Ok(())
    }

    /// One frame of a run's session. `None` once the run has ended and its
    /// session is released -- what is left then is the transcript.
    pub async fn run_screen(&self, run: &Run) -> Result<Option<Screen>> {
        let Some(session) = &run.session else {
            return Ok(None);
        };
        let runtime = self.registry.runtime(&session.runtime)?;
        runtime.screen(session).await
    }

    pub async fn session_status(&self, run: &Run) -> RuntimeStatus {
        let Some(session) = &run.session else {
            return RuntimeStatus::Unknown;
        };
        match self.registry.runtime(&session.runtime) {
            Ok(rt) => rt.status(session).await.unwrap_or(RuntimeStatus::Unknown),
            Err(_) => RuntimeStatus::Unknown,
        }
    }

    /// `session_status`, plus where the answer came from. The one caller that
    /// needs provenance is `record_run_liveness` -- the scheduler's `Gone`
    /// check and `supervise_agents` only ever need the status, so they keep
    /// calling `status` through `session_status` rather than paying for a
    /// question they do not ask.
    pub async fn session_status_report(&self, run: &Run) -> StatusReport {
        let unknown = StatusReport {
            status: RuntimeStatus::Unknown,
            source: StatusSource::Unknown,
        };
        let Some(session) = &run.session else {
            return unknown;
        };
        match self.registry.runtime(&session.runtime) {
            Ok(rt) => rt.status_report(session).await.unwrap_or(unknown),
            Err(_) => unknown,
        }
    }

    // -- small helpers ------------------------------------------------------

    async fn require(&self, id: &str) -> Result<Task> {
        self.store
            .get(id)
            .await?
            .ok_or_else(|| FactoryError::TaskNotFound(id.to_string()))
    }

    async fn require_run(&self, id: &str) -> Result<Run> {
        self.store
            .get_run(id)
            .await?
            .ok_or_else(|| FactoryError::TaskNotFound(format!("run {id}")))
    }

    async fn publish_task(&self, id: &str) {
        if let Ok(Some(task)) = self.store.get(id).await {
            self.bus.publish(Event::TaskUpdated { task });
        }
    }

    /// `pub(crate)`: `occupancy::record_run_liveness` journals a hook-reported
    /// block or unblock the same way any other daemon-caused change is
    /// journaled here.
    pub(crate) async fn entry(&self, task_id: &str, entry: TaskEntry) {
        if let Err(e) = self.store.append_entry(task_id, &entry).await {
            tracing::warn!(task = task_id, "could not record journal entry: {e}");
        }
        self.bus.publish(Event::TaskEntry {
            id: task_id.to_string(),
            entry,
        });
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let head: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_core::adapter::RuntimeConnectionState;
    use factory_core::config::{Config, DaemonConfig, Instance, Scope};
    use factory_core::run::RunStatus;
    use factory_plugins::{Registry, SqliteStore};

    /// A scope pointed at `scope_path`, one store in memory, and every
    /// built-in adapter registered -- enough to dispatch a task without a
    /// real herdr or a real agent, since the paths under test here never
    /// reach either.
    fn test_engine(scope_path: PathBuf) -> Arc<Engine> {
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
                id: "scope-id".into(),
                name: "demo".into(),
                path: scope_path,
                agent: None,
                agents: Vec::new(),
                runtime: None,
                git: None,
                task_store: None,
            }],
            plugins_dir: None,
        };
        let factory = Factory {
            root: std::env::temp_dir().join(format!("factory-engine-test-{}", uuid::Uuid::new_v4())),
            config,
        };
        let registry = Registry::with_builtins();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        Arc::new(Engine::new(
            factory,
            registry,
            store,
            PathBuf::from("factory"),
            Vec::new(),
        ))
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("factory-engine-test-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn runtime_diagnostics_group_scopes_and_isolate_a_missing_adapter_as_data() {
        let scope_dir = temp_dir("runtime-diagnostic");
        let engine = test_engine(scope_dir.clone());
        {
            let mut factory = engine.factory.write().unwrap();
            factory.config.scopes[0].runtime = Some("not-registered".into());
            let mut second = factory.config.scopes[0].clone();
            second.id = "second-id".into();
            second.name = "other".into();
            second.path = scope_dir.join("other");
            factory.config.scopes.push(second);
        }

        let views = engine.runtime_connections().await;
        assert_eq!(views.len(), 1, "one connection is probed once for both scopes");
        assert_eq!(views[0].runtime, "not-registered");
        assert_eq!(views[0].scopes, vec!["demo", "other"]);
        assert_eq!(views[0].source, "missing");
        assert_eq!(views[0].diagnostic.state, RuntimeConnectionState::Error);
        assert!(
            views[0]
                .diagnostic
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("not-registered"),
            "the card says which configured adapter is absent"
        );

        std::fs::remove_dir_all(scope_dir).ok();
    }

    #[tokio::test]
    async fn a_run_whose_worktree_creation_fails_does_not_get_a_session_and_reports_gits_error() {
        // Not a git repository, so `git worktree add` has nothing to work
        // with. The checkbox defaults to on, so this is the ordinary case for
        // a scope nobody has run `git init` in yet -- exactly what a person
        // must never see silently turn into a run in the scope itself.
        let scope_dir = temp_dir("scope");
        let engine = test_engine(scope_dir.clone());

        let task = engine
            .create(NewTask {
                title: "try the worktree".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(task.worktree, "on by default, and this task never said otherwise");

        engine.start_run(&task.id, Trigger::Manual).await;

        let runs = engine.store.runs(&task.id, 10).await.unwrap();
        assert_eq!(runs.len(), 1, "the run row was made before the worktree was attempted");
        let run = &runs[0];
        assert_eq!(run.status, RunStatus::Failed);
        assert!(run.session.is_none(), "it never got as far as opening a session");
        assert!(run.worktree_path.is_none(), "nothing to record -- the worktree never existed");
        let error = run.error.clone().unwrap_or_default();
        assert!(
            error.contains("not a git repository"),
            "run.error should carry git's own complaint, got: {error:?}"
        );

        let entries = engine.store.run_entries(&run.id, 50).await.unwrap();
        assert!(
            entries.iter().any(|e| e.message.contains("not a git repository")),
            "the journal gets git's complaint too"
        );

        std::fs::remove_dir_all(&scope_dir).ok();
    }

    // `blocked_since` promises to be `None` whenever the status is not
    // `Blocked`. The agent's own report honoured that; the daemon giving up on
    // a run did not, so a failed run kept saying it was still waiting for
    // somebody.
    #[tokio::test]
    async fn a_run_the_daemon_fails_out_of_a_block_stops_claiming_to_be_waiting() {
        let scope_dir = temp_dir("scope");
        let engine = test_engine(scope_dir.clone());

        let task = engine
            .create(NewTask {
                title: "asks a question and is given up on".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                worktree: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();

        let run = engine
            .store
            .create_run(&factory_core::run::NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Manual,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: task.runtime.clone(),
                token: "tok".into(),
            })
            .await
            .unwrap();

        let blocked = engine
            .store
            .update_run(
                &run.id,
                &RunPatch {
                    status: Some(RunStatus::Blocked),
                    blocked_since: Some(Utc::now()),
                    blocked_source: Some(BlockSource::Agent),
                    block_suspected_since: Some(Utc::now()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(blocked.blocked_since.is_some(), "the block is on before we fail it");

        engine.fail_run(&run.id, "nobody ever answered").await;

        let failed = engine.store.get_run(&run.id).await.unwrap().unwrap();
        assert_eq!(failed.status, RunStatus::Failed);
        assert!(failed.blocked_since.is_none(), "a finished run is not still waiting");
        assert!(failed.blocked_source.is_none(), "and nobody is holding it");
        assert!(
            failed.block_suspected_since.is_none(),
            "a guess about a session that is gone is not worth keeping either"
        );
    }

    #[tokio::test]
    async fn a_task_with_the_checkbox_off_still_runs_in_the_scope_even_when_it_is_not_a_git_repository() {
        // `place_run` is called directly rather than through `dispatch`,
        // which would go on to call a real runtime -- this machine actually
        // has herdr installed, and a unit test has no business starting a
        // real session. `place_run` is the whole of the decision `dispatch`
        // makes here, so exercising it alone is exercising the real thing.
        let scope_dir = temp_dir("scope"); // not a git repository, on purpose
        let engine = test_engine(scope_dir.clone());

        let task = engine
            .create(NewTask {
                title: "stay in the scope".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                worktree: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(!task.worktree);

        let run = engine
            .store
            .create_run(&factory_core::run::NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Manual,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: task.runtime.clone(),
                token: "tok".into(),
            })
            .await
            .unwrap();

        let (cwd, run) = engine.place_run(&task, run, &scope_dir).await.unwrap();
        assert_eq!(cwd, scope_dir, "the checkbox is off, so this stays the scope itself");
        assert!(run.worktree_path.is_none());
        assert!(run.worktree_branch.is_none());
    }

    /// `scope_views` asks `git` per configured scope, potentially many
    /// subprocesses on an endpoint the Agents view refetches on every event.
    /// The answer is cached, so the second board
    /// within `CAPABILITY_TTL` runs no `git` at all -- observed here as the
    /// cached answer surviving a change on disk that would flip it.
    #[tokio::test]
    async fn a_scopes_worktree_capability_is_asked_of_git_once_per_ttl() {
        let scope_dir = temp_dir("caps");
        let engine = test_engine(scope_dir.clone());

        // No repository yet, so the first board says so.
        let (first, _) = engine.scope_views().await.unwrap();
        assert_eq!(first[0].id, "scope-id", "the scope identity reaches the API view");
        assert!(!first[0].worktree_capable);

        for args in [
            &["init", "-q"][..],
            &["config", "user.email", "factory@example.com"][..],
            &["config", "user.name", "factory"][..],
            &["commit", "-q", "--allow-empty", "-m", "base"][..],
        ] {
            assert!(std::process::Command::new("git")
                .args(args)
                .current_dir(&scope_dir)
                .status()
                .unwrap()
                .success());
        }

        let (second, _) = engine.scope_views().await.unwrap();
        assert!(
            !second[0].worktree_capable,
            "within the TTL the cached answer stands, git is not asked again"
        );

        engine.worktree_caps.lock().unwrap().clear();
        let (third, _) = engine.scope_views().await.unwrap();
        assert!(third[0].worktree_capable, "once stale, git is asked and sees the repository");
    }

    #[tokio::test]
    async fn a_capable_scope_gives_the_run_its_own_worktree_and_the_run_remembers_where() {
        let scope_dir = temp_dir("scope");
        for args in [
            &["init", "-q"][..],
            &["config", "user.email", "factory@example.com"][..],
            &["config", "user.name", "factory"][..],
            &["commit", "-q", "--allow-empty", "-m", "base"][..],
        ] {
            assert!(std::process::Command::new("git")
                .args(args)
                .current_dir(&scope_dir)
                .status()
                .unwrap()
                .success());
        }

        let engine = test_engine(scope_dir.clone());
        let root = engine.factory_snapshot().root.clone();
        let task = engine
            .create(NewTask {
                title: "do the thing".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                ..Default::default()
            })
            .await
            .unwrap();

        let run = engine
            .store
            .create_run(&factory_core::run::NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Manual,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: task.runtime.clone(),
                token: "tok".into(),
            })
            .await
            .unwrap();
        let run_id = run.id.clone();

        let (cwd, run) = engine.place_run(&task, run, &scope_dir).await.unwrap();
        assert_ne!(cwd, scope_dir, "the checkbox is on, so this is not the scope itself");
        assert_eq!(cwd, engine.factory_snapshot().worktrees_dir().join(&run_id), "named after the run");
        assert!(cwd.join(".git").exists(), "a real worktree, not just a path");
        assert_eq!(run.worktree_path.as_deref(), Some(cwd.display().to_string().as_str()));
        assert!(
            run.worktree_branch.as_deref().unwrap_or_default().starts_with("factory/"),
            "the branch reads as this daemon's, got {:?}",
            run.worktree_branch
        );

        std::fs::remove_dir_all(&scope_dir).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    // -- args land in the order the daemon promises -------------------------

    #[test]
    fn declared_args_land_after_whatever_the_guide_already_injected() {
        // `launch_spec` puts the adapter's own defaults first and then the
        // guide's flag; this pins the half `append_declared_args` owns -- a
        // scope's own `args:` still lands after both, last word wins.
        let mut launch = LaunchSpec {
            kind: factory_core::adapter::agent::LaunchKind::Named("pi".into()),
            args: vec!["--append-system-prompt".into(), "/tmp/guide.md".into()],
            env: Default::default(),
        };
        let declared: ScopeAgent = serde_yaml_ng::from_str(
            "name: watcher\nharness: pi\nlifetime: permanent\nargs: [--model, opus]\n",
        )
        .unwrap();

        append_declared_args(&mut launch, Some(&declared));

        assert_eq!(
            launch.args,
            vec!["--append-system-prompt", "/tmp/guide.md", "--model", "opus"],
            "the declared override comes after the injected flag, not before it"
        );
    }

    #[tokio::test]
    async fn a_dispatched_runs_declared_args_follow_the_guide_launch_spec_injects() {
        // The same guarantee, exercised through the real adapter rather than
        // a hand-built `LaunchSpec`: `HarnessAgent::launch_spec` still puts
        // its own defaults first and the guide's flag after them, so
        // `append_declared_args` has something correctly ordered to append to.
        use factory_core::role::Role;
        use factory_core::Agent as _;
        use factory_plugins::HarnessAgent;

        let root = std::env::temp_dir().join(format!("factory-args-order-test-{}", uuid::Uuid::new_v4()));
        let ctx = AgentContext {
            scope: "demo".into(),
            agent_name: "watcher".into(),
            cwd: root.join("cwd"),
            factory_bin: PathBuf::from("factory"),
            socket: root.join("factory.sock"),
            guides_dir: root.join("guides"),
            task: None,
            identity_token: Some("identity".into()),
            role: Some(
                factory_core::role::Roles::presets()
                    .get(&Role::worker())
                    .unwrap()
                    .clone(),
            ),
        };
        let agent = HarnessAgent::pi().with_args(vec!["--model".into(), "sonnet".into()]);
        let mut launch = agent.launch_spec(&ctx).await.unwrap();
        let declared: ScopeAgent = serde_yaml_ng::from_str(
            "name: watcher\nharness: pi\nlifetime: permanent\nargs: [--model, opus]\n",
        )
        .unwrap();

        append_declared_args(&mut launch, Some(&declared));

        assert_eq!(&launch.args[0..2], ["--model", "sonnet"], "the adapter's own defaults come first");
        assert_eq!(launch.args[2], "--append-system-prompt", "then the guide's flag");
        assert_eq!(
            &launch.args[launch.args.len() - 2..],
            ["--model", "opus"],
            "the scope's declared override lands last of all"
        );

        std::fs::remove_dir_all(&root).ok();
    }
}
