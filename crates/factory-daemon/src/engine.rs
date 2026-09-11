//! The core. Everything an interface can ask for arrives here as a `Request`
//! and leaves as a `Response`; the interfaces themselves hold no logic.

use chrono::Utc;
use factory_core::adapter::agent::AgentContext;
use factory_core::adapter::runtime::{RuntimeStatus, StartRequest};
use factory_core::adapter::TaskStore;
use factory_core::config::Factory;
use factory_core::error::{FactoryError, Result};
use factory_core::event::{Event, EventBus};
use factory_core::protocol::{AdapterList, Payload, Request, Response, StatusInfo};
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
    pub store: Arc<dyn TaskStore>,
    pub bus: EventBus,
    pub factory_bin: PathBuf,
    started: Instant,
    interfaces: Vec<String>,
}

impl Engine {
    pub fn new(
        factory: Factory,
        registry: Registry,
        store: Arc<dyn TaskStore>,
        factory_bin: PathBuf,
        interfaces: Vec<String>,
    ) -> Self {
        Self {
            factory,
            registry,
            store,
            bus: EventBus::default(),
            factory_bin,
            started: Instant::now(),
            interfaces,
        }
    }

    // -- the API every interface speaks ------------------------------------

    pub async fn handle(self: &Arc<Self>, req: Request) -> Response {
        match self.dispatch_request(req).await {
            Ok(payload) => Response::ok(payload),
            Err(e) => Response::error(e.code(), e.to_string()),
        }
    }

    async fn dispatch_request(self: &Arc<Self>, req: Request) -> Result<Payload> {
        match req {
            Request::Status => Ok(Payload::Status { status: self.status().await? }),
            Request::Adapters => Ok(self.adapters().into()),
            Request::TaskCreate(new) => Ok(Payload::Task { task: self.create(new).await?.redacted() }),
            Request::TaskGet { id } => Ok(Payload::Task { task: self.require(&id).await?.redacted() }),
            Request::TaskList(filter) => Ok(Payload::Tasks {
                tasks: self
                    .store
                    .list(&filter)
                    .await?
                    .into_iter()
                    .map(|t| t.redacted())
                    .collect(),
            }),
            Request::TaskUpdate { id, patch } => {
                // A caller must not be able to hand itself a task's token.
                let mut patch = patch;
                patch.token = None;
                let task = self.store.update(&id, &patch).await?;
                self.bus.publish(Event::TaskUpdated { task: task.clone() });
                Ok(Payload::Task { task: task.redacted() })
            }
            Request::TaskDelete { id } => {
                let task = self.store.get(&id).await?;
                if let Some(session) = task.and_then(|t| t.session) {
                    let _ = self.stop_session(&session).await;
                }
                let deleted = self.store.delete(&id).await?;
                if deleted {
                    self.bus.publish(Event::TaskDeleted { id });
                }
                Ok(Payload::Deleted { deleted })
            }
            Request::TaskRun { id } => {
                let task = self.require(&id).await?;
                if !task.status.is_terminal() && task.status != TaskStatus::Pending {
                    return Err(FactoryError::BadRequest(format!(
                        "task {id} is already {}; cancel it before running it again",
                        task.status.as_str()
                    )));
                }
                let engine = self.clone();
                // Dispatch can take a minute: starting a pane, waiting for an
                // agent to be ready. The caller gets its answer now.
                tokio::spawn(async move {
                    if let Err(e) = engine.dispatch(&id).await {
                        engine.fail(&id, &format!("dispatch failed: {e}")).await;
                    }
                });
                Ok(Payload::Ok)
            }
            Request::TaskCancel { id } => {
                let task = self.require(&id).await?;
                if let Some(session) = &task.session {
                    let _ = self.stop_session(session).await;
                }
                let task = self
                    .store
                    .update(
                        &id,
                        &TaskPatch {
                            status: Some(TaskStatus::Cancelled),
                            ..Default::default()
                        },
                    )
                    .await?;
                self.entry(&id, TaskEntry::new("daemon", "cancelled", "cancelled by request"))
                    .await;
                self.bus.publish(Event::TaskUpdated { task: task.clone() });
                Ok(Payload::Task { task: task.redacted() })
            }
            Request::TaskReport { id, report } => {
                Ok(Payload::Task { task: self.report(&id, report).await?.redacted() })
            }
            Request::TaskEntries { id, limit } => Ok(Payload::Entries {
                entries: self.store.entries(&id, limit.unwrap_or(200)).await?,
            }),
            Request::TaskOutput { id, lines } => {
                let task = self.require(&id).await?;
                let session = task.session.ok_or_else(|| {
                    FactoryError::BadRequest(format!("task {id} has no live session"))
                })?;
                let runtime = self.registry.runtime(&session.runtime)?;
                Ok(Payload::Text {
                    text: runtime.read(&session, lines.unwrap_or(120)).await?,
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
            tasks_active: tasks.iter().filter(|t| !t.status.is_terminal()).count(),
            subscribers: self.bus.subscriber_count(),
            interfaces: self.interfaces.clone(),
            scopes: self.factory.scope_names(),
        })
    }

    fn adapters(&self) -> AdapterList {
        self.registry.list()
    }

    // -- creating and running ----------------------------------------------

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
                    FactoryError::BadRequest(
                        "no scope given and the instance declares none".into(),
                    )
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

        // Refuse now, with the list of what exists, rather than at dispatch
        // time when whoever asked has stopped watching.
        self.registry.agent(&agent)?;
        self.registry.runtime(&runtime)?;

        let mut task =
            factory_core::adapter::store::task_from_new(new, scope, agent, runtime);
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

    /// Open a session, start the agent, hand it the task. Returns once the
    /// prompt is in; the agent reports the rest itself.
    pub async fn dispatch(self: &Arc<Self>, id: &str) -> Result<()> {
        let task = self.require(id).await?;
        let agent = self.registry.agent(&task.agent)?;
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
        let task = self
            .store
            .update(
                id,
                &TaskPatch {
                    status: Some(TaskStatus::Dispatching),
                    token: Some(token.clone()),
                    last_run_at: Some(Utc::now()),
                    ..Default::default()
                },
            )
            .await?;
        self.bus.publish(Event::TaskUpdated { task: task.clone() });

        let ctx = AgentContext {
            task: task.clone(),
            cwd: cwd.clone(),
            factory_bin: self.factory_bin.clone(),
            socket: self.factory.socket_path(),
            token,
        };

        let launch = agent.launch_spec(&ctx).await?;
        let session = runtime
            .start(&StartRequest {
                task_id: task.id.clone(),
                label: format!("factory: {}", truncate(&task.title, 40)),
                cwd,
                launch,
            })
            .await?;

        let task = self
            .store
            .update(
                id,
                &TaskPatch {
                    session: Some(session.clone()),
                    ..Default::default()
                },
            )
            .await?;
        self.bus.publish(Event::TaskUpdated { task });

        let prompt = agent.prompt(&ctx).await?;
        runtime.submit(&session, &prompt).await?;

        self.entry(
            id,
            TaskEntry::new(
                "daemon",
                "dispatched",
                format!("handed to {} on {}", ctx.task.agent, session.runtime),
            )
            .with_data(serde_json::json!({ "session": session })),
        )
        .await;
        Ok(())
    }

    /// What an agent says about its own task. The token is what makes this a
    /// report rather than anyone on the socket closing anyone's task.
    pub async fn report(&self, id: &str, report: TaskReport) -> Result<Task> {
        let task = self.require(id).await?;

        if task.status.is_terminal() {
            return Err(FactoryError::BadRequest(format!(
                "task {id} is already {}; reports on it are no longer accepted",
                task.status.as_str()
            )));
        }

        if let Some(expected) = &task.token {
            match &report.token {
                Some(given) if given == expected => {}
                Some(_) => return Err(FactoryError::Denied(format!("wrong token for task {id}"))),
                None => {
                    return Err(FactoryError::Denied(format!(
                        "task {id} needs its token; pass --token or set FACTORY_TASK_TOKEN"
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
            id,
            TaskEntry::new(
                "agent",
                report
                    .status
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_else(|| "note".into()),
                message,
            ),
        )
        .await;

        let terminal = report.status.map(TaskStatus::is_terminal).unwrap_or(false);

        // Keep the last of what the agent saw before the session goes away.
        if terminal {
            if let Some(session) = &task.session {
                if let Ok(runtime) = self.registry.runtime(&session.runtime) {
                    if let Ok(text) = runtime.read(session, 200).await {
                        self.entry(
                            id,
                            TaskEntry::new("daemon", "transcript", "final terminal output")
                                .with_data(serde_json::json!({ "text": text })),
                        )
                        .await;
                    }
                }
                let _ = self.stop_session(session).await;
            }
        }

        // A recurring task goes back to pending rather than staying done: the
        // scheduler only fires pending tasks, and its last result stays on it
        // until the next run replaces it. One task, not one per firing -- the
        // simplification this prototype makes instead of modelling runs.
        let recurring = task.schedule.is_some();
        let status = match (report.status, terminal, recurring) {
            (Some(_), true, true) => Some(TaskStatus::Pending),
            (other, _, _) => other,
        };
        if terminal && recurring {
            self.entry(
                id,
                TaskEntry::new(
                    "daemon",
                    "rearmed",
                    "recurring task is pending again, waiting for its next turn",
                ),
            )
            .await;
        }

        let updated = self
            .store
            .update(
                id,
                &TaskPatch {
                    status,
                    result: report.result,
                    error: report.error,
                    // A finished task has no more use for its token.
                    token: if terminal { Some(String::new()) } else { None },
                    // A session that has been closed must not be reported as live.
                    clear_session: terminal,
                    ..Default::default()
                },
            )
            .await?;

        self.bus.publish(Event::TaskUpdated {
            task: updated.clone(),
        });
        Ok(updated)
    }

    // -- what the scheduler and watchdog need -------------------------------

    pub async fn due_now(&self) -> Result<Vec<Task>> {
        self.store.due(Utc::now()).await
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

    /// A task that will never report back. Closes the session and says why.
    pub async fn fail(&self, id: &str, why: &str) {
        if let Ok(Some(task)) = self.store.get(id).await {
            if let Some(session) = &task.session {
                let _ = self.stop_session(session).await;
            }
        }
        self.entry(id, TaskEntry::new("daemon", "failed", why.to_string()))
            .await;
        let recurring = self
            .store
            .get(id)
            .await
            .ok()
            .flatten()
            .and_then(|t| t.schedule)
            .is_some();
        if let Ok(task) = self
            .store
            .update(
                id,
                &TaskPatch {
                    status: Some(if recurring {
                        TaskStatus::Pending
                    } else {
                        TaskStatus::Failed
                    }),
                    error: Some(why.to_string()),
                    token: Some(String::new()),
                    clear_session: true,
                    ..Default::default()
                },
            )
            .await
        {
            self.bus.publish(Event::TaskUpdated { task });
        }
    }

    pub async fn session_status(&self, task: &Task) -> RuntimeStatus {
        let Some(session) = &task.session else {
            return RuntimeStatus::Unknown;
        };
        match self.registry.runtime(&session.runtime) {
            Ok(rt) => rt.status(session).await.unwrap_or(RuntimeStatus::Unknown),
            Err(_) => RuntimeStatus::Unknown,
        }
    }

    async fn stop_session(&self, session: &factory_core::task::SessionRef) -> Result<()> {
        self.registry.runtime(&session.runtime)?.stop(session).await
    }

    async fn require(&self, id: &str) -> Result<Task> {
        self.store
            .get(id)
            .await?
            .ok_or_else(|| FactoryError::TaskNotFound(id.to_string()))
    }

    async fn entry(&self, id: &str, entry: TaskEntry) {
        if let Err(e) = self.store.append_entry(id, &entry).await {
            tracing::warn!(task = id, "could not record journal entry: {e}");
        }
        self.bus.publish(Event::TaskEntry {
            id: id.to_string(),
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
