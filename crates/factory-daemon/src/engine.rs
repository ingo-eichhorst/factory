//! The core. Everything an interface can ask for arrives here as a `Request`
//! and leaves as a `Response`; the interfaces themselves hold no logic.

use chrono::Utc;
use factory_core::adapter::agent::{AgentContext, TaskBinding};
use factory_core::adapter::runtime::{RuntimeStatus, Screen, StartRequest};
use factory_core::adapter::TaskStore;
use factory_core::config::Factory;
use factory_core::error::{FactoryError, Result};
use factory_core::event::{Event, EventBus};
use factory_core::protocol::{
    AgentActivity, AgentView, Envelope, Payload, Request, Response, ScopeView, StatusInfo,
};
use factory_core::agent::{AgentSession, AgentState};
use factory_core::role::{Role, Roles};
use factory_core::run::{NewRun, Run, RunPatch, RunStatus, Trigger};
use factory_core::task::{
    NewTask, Task, TaskEntry, TaskFilter, TaskPatch, TaskReport, TaskStatus,
};
use factory_plugins::registry::Registry;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use crate::schedule;

pub struct Engine {
    pub factory: Factory,
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
            factory,
            registry,
            roles,
            store,
            bus: EventBus::default(),
            factory_bin,
            started: Instant::now(),
            interfaces,
            seen_status: Default::default(),
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
        let tasks = self.store.list(&TaskFilter::default()).await?;
        Ok(StatusInfo {
            instance: self.factory.config.instance.name.clone(),
            instance_id: self.factory.config.instance.id.clone(),
            root: self.factory.root.display().to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_seconds: self.started.elapsed().as_secs(),
            tasks_total: tasks.len(),
            tasks_active: self.store.active_runs().await?.len(),
            subscribers: self.bus.subscriber_count(),
            interfaces: self.interfaces.clone(),
            scopes: self.factory.scope_names(),
        })
    }

    /// The agents page: scopes first, then the agents each one declares, then
    /// what they are doing -- and, once, every adapter registered, which
    /// belongs to the whole answer rather than to any one scope in it. One
    /// call, because a page that had to join config, adapters, standing
    /// agents, runs and tasks itself would be showing five different moments
    /// in time.
    pub(crate) async fn scope_views(&self) -> Result<(Vec<ScopeView>, Vec<String>)> {
        let adapters = self.registry.list();
        let described: std::collections::BTreeMap<String, (String, String)> = adapters
            .adapters
            .iter()
            .filter(|a| a.kind == "agent")
            .map(|a| (a.name.clone(), (a.description.clone(), a.source.clone())))
            .collect();
        let available: Vec<String> = described.keys().cloned().collect();

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
                .map(|t| self.factory.canonical_scope_name(&t.scope))
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

        let instance_default = self.factory.config.daemon.default_agent.clone();
        let mut views = Vec::new();

        for scope in &self.factory.config.scopes {
            let default_agent = scope
                .agent_adapter()
                .unwrap_or(&instance_default)
                .to_string();
            let runtime = scope
                .runtime
                .clone()
                .unwrap_or_else(|| self.factory.config.daemon.default_runtime.clone());

            let mut agents = Vec::new();
            let mut covered = std::collections::BTreeSet::new();

            for decl in scope.agents_with(&self.factory.config.daemon.foreman) {
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
                    attach: None,
                    session: None,
                    started_at: None,
                    error: None,
                    active: jobs.clone(),
                });
            }

            views.push(ScopeView {
                name: scope.name.clone(),
                path: self
                    .factory
                    .scope_path(&scope.name)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| scope.path.display().to_string()),
                default_agent,
                runtime,
                agents,
                // Nothing today gives one scope a different roster of
                // adapters than any other, so there is no per-scope override
                // to carry -- the shared list returned alongside `views` is
                // the whole answer.
                available: None,
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
    pub fn resolve_agent(&self, scope_name: &str, name: &str) -> Result<(String, String)> {
        let scope = self.factory.scope(scope_name)?;
        let declared_here = scope.agents_with(&self.factory.config.daemon.foreman);
        if let Some(declared) = declared_here.iter().find(|a| a.name() == name).cloned() {
            // The name resolves; the adapter behind it still has to exist.
            self.registry.agent(&declared.harness)?;
            return Ok((declared.name(), declared.harness));
        }
        if self.registry.agent(name).is_ok() {
            return Ok((name.to_string(), name.to_string()));
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
        let current = self.require(id).await?;
        // The bookkeeping is the daemon's, not a caller's.
        patch.runs = None;

        let scope = patch.scope.clone().unwrap_or_else(|| current.scope.clone());
        if patch.scope.is_some() {
            // Store the identity the scope actually has, not necessarily the
            // one the caller typed -- a bare name from before scopes had
            // paths still resolves (`Factory::scope`'s fallback), but writing
            // it back down unchanged would keep manufacturing the very
            // ambiguity that fallback exists to paper over.
            patch.scope = Some(self.factory.scope(&scope)?.name.clone());
        }
        match &patch.agent {
            Some(agent) => {
                let (name, _) = self.resolve_agent(&scope, agent)?;
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
        if new.title.trim().is_empty() {
            return Err(FactoryError::BadRequest("a task needs a title".into()));
        }

        let scope = match new.scope.clone() {
            Some(s) => s,
            None => self
                .factory
                .config
                .scopes
                .first()
                .map(|s| s.name.clone())
                .ok_or_else(|| {
                    FactoryError::BadRequest("no scope given and the instance declares none".into())
                })?,
        };
        let declared = self.factory.scope(&scope)?.clone();

        let agent = new
            .agent
            .clone()
            .or_else(|| declared.agent_adapter().map(str::to_string))
            .unwrap_or_else(|| self.factory.config.daemon.default_agent.clone());
        let runtime = new
            .runtime
            .clone()
            .or_else(|| declared.runtime.clone())
            .unwrap_or_else(|| self.factory.config.daemon.default_runtime.clone());

        // Refuse now, with the list of what this scope offers, rather than at
        // dispatch time when whoever asked has stopped watching.
        let (agent, _adapter) = self.resolve_agent(&scope, &agent)?;
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
        let (agent_name, adapter_name) = self.resolve_agent(&task.scope, &task.agent)?;
        let agent = self.registry.agent(&adapter_name)?;
        let runtime = self.registry.runtime(&task.runtime)?;
        let cwd = self.factory.scope_path(&task.scope)?;
        if !cwd.is_dir() {
            return Err(FactoryError::BadRequest(format!(
                "scope {:?} points at {}, which is not a directory",
                task.scope,
                cwd.display()
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

        let ctx = AgentContext {
            scope: task.scope.clone(),
            cwd: cwd.clone(),
            factory_bin: self.factory_bin.clone(),
            socket: self.factory.socket_path(),
            task: Some(TaskBinding {
                task: task.clone(),
                run_id: run.id.clone(),
                attempt: run.attempt,
                token,
            }),
            identity_token: None,
        };

        let launch = agent.launch_spec(&ctx).await?;
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

        let patch = RunPatch {
            status: report.status,
            result: report.result,
            error: report.error,
            ..Default::default()
        };

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
    async fn mirror_to_task(&self, run: &Run) {
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

    /// Keep the last of what the agent saw, then let the session go.
    async fn close_session(&self, run: &Run) {
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

    async fn entry(&self, task_id: &str, entry: TaskEntry) {
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
