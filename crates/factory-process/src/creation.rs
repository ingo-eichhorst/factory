//! L4 owns task creation: validation, persisted rows and creation journals.
use crate::{
    store::TaskStore,
    task::{NewTask, Task, TaskEntry, TaskStatus, WorkflowOrigin},
};
use chrono::Utc;
use factory_agents::{roster::AgentRef, selection::Selection};
use factory_kernel::{CommandPort, Commands, FactoryError, Result, ScopeIdentity, TaskReceipt, L4};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

pub const REVIEW_RUN_LABEL: &str = "factory.review_run";
pub const REVIEW_STEP_LABEL: &str = "factory.review_step";
pub const REVIEW_DIGEST_LABEL: &str = "factory.review_digest";
#[derive(Clone)]
pub struct Scope {
    pub name: String,
    pub path: PathBuf,
    pub agent: Option<AgentRef>,
    pub runtime: Option<String>,
}
impl ScopeIdentity for Scope {
    fn scope_name(&self) -> &str {
        &self.name
    }
    fn scope_path(&self) -> &Path {
        &self.path
    }
}
pub struct Inputs {
    pub scopes: Vec<Scope>,
    pub default_agent: String,
    pub default_runtime: String,
}
/// Lossy outward notifications only, never a fact or command route back up.
pub trait Observer: Send + Sync {
    fn entry(&self, id: &str, entry: TaskEntry);
    fn created(&self, task: Task);
}
#[async_trait::async_trait]
pub trait TaskCommands: CommandPort<Level = L4> + Send + Sync {
    async fn submit(&self, new: NewTask) -> Result<TaskReceipt>;
}
#[async_trait::async_trait]
impl<P: TaskCommands + ?Sized> TaskCommands for &P {
    async fn submit(&self, new: NewTask) -> Result<TaskReceipt> {
        (**self).submit(new).await
    }
}
pub struct Service<'a, P: Selection> {
    pub store: &'a dyn TaskStore,
    pub inputs: Inputs,
    pub agents: Commands<L4, P>,
    pub observer: &'a dyn Observer,
}
impl<P: Selection> CommandPort for Service<'_, P> {
    type Level = L4;
}
#[async_trait::async_trait]
impl<P: Selection> TaskCommands for Service<'_, P> {
    async fn submit(&self, new: NewTask) -> Result<TaskReceipt> {
        self.create_extended(new, None, None, None, None, false)
            .await
    }
}
impl<P: Selection> Service<'_, P> {
    #[allow(clippy::too_many_arguments)]
    pub async fn create_extended(
        &self,
        new: NewTask,
        workflow: Option<WorkflowOrigin>,
        origin: Option<crate::origin::OriginRef>,
        id: Option<String>,
        intake: Option<crate::intake::Intake>,
        review: bool,
    ) -> Result<TaskReceipt> {
        let task = self
            .persist(new, workflow, origin, id, intake, review)
            .await?;
        Ok(TaskReceipt { id: task.id })
    }
    async fn persist(
        &self,
        mut new: NewTask,
        workflow_origin: Option<WorkflowOrigin>,
        bench_origin: Option<crate::origin::OriginRef>,
        id: Option<String>,
        intake: Option<crate::intake::Intake>,
        internal_review: bool,
    ) -> Result<Task> {
        let factory = &self.inputs;
        if new.after.is_some() && new.schedule.is_some() {
            return Err(FactoryError::BadRequest(
                "after and schedule are exclusive triggers".into(),
            ));
        }
        if workflow_origin.is_none() && new.after.as_ref().is_some_and(Vec::is_empty) {
            return Err(FactoryError::BadRequest(
                "after needs at least one upstream task".into(),
            ));
        }
        if workflow_origin.is_none() {
            if let Some(after) = &new.after {
                validate_after(self.store, id.as_deref(), after).await?;
            }
        }
        if new.title.trim().is_empty() {
            return Err(FactoryError::BadRequest("a task needs a title".into()));
        }
        if new.estimate_seconds == Some(0) {
            return Err(FactoryError::BadRequest(
                "a task estimate must be at least one second".into(),
            ));
        }
        if !internal_review
            && (new.labels.contains_key(REVIEW_RUN_LABEL)
                || new.labels.contains_key(REVIEW_STEP_LABEL)
                || new.labels.contains_key(REVIEW_DIGEST_LABEL))
        {
            return Err(FactoryError::BadRequest(
                "factory.review_* labels are reserved for daemon-created review tasks".into(),
            ));
        }
        if let Some(estimate) = &new.estimate {
            estimate.validate().map_err(FactoryError::BadRequest)?;
            new.estimate_seconds = Some(estimate.time.expected);
        } else if let Some(seconds) = new.estimate_seconds {
            new.estimate = Some(crate::task::Estimate::point(seconds));
        }
        // A retry policy governs what happens after a *scheduled* run fails
        // (the retry scheduler never looks at it for a task with no
        // `schedule`) -- refused here, at creation, rather than accepted and
        // silently ignored until whoever set it notices nothing ever
        // retries.
        if new.retry.is_some() && new.schedule.is_none() {
            return Err(FactoryError::BadRequest(
                "a retry policy only means something for a scheduled task; add a schedule too, or drop retry".into(),
            ));
        }
        if let Some(category) = &new.category {
            crate::control_plan::check_category(category).map_err(FactoryError::BadRequest)?;
        }

        let scope = match new.scope.clone() {
            Some(s) => s,
            None => factory
                .scopes
                .first()
                .map(|s| s.name.clone())
                .ok_or_else(|| {
                    FactoryError::BadRequest("no scope given and the instance declares none".into())
                })?,
        };
        let declared = factory_kernel::resolve_scope(&factory.scopes, &scope)?.clone();

        let agent = new
            .agent
            .clone()
            .or_else(|| declared.agent.as_ref().map(|a| a.adapter().to_string()))
            .unwrap_or_else(|| factory.default_agent.clone());
        let runtime = new
            .runtime
            .clone()
            .or_else(|| declared.runtime.clone())
            .unwrap_or_else(|| factory.default_runtime.clone());

        // Refuse now, with the list of what this scope offers, rather than at
        // dispatch time when whoever asked has stopped watching.
        let agent = self.agents.port().select(&scope, &agent, &runtime)?;

        // The scope's canonical identity, not necessarily what the caller
        // typed -- `declared` is already resolved through the bare-name
        // fallback above, and storing its own name keeps a freshly created
        // task from starting life needing that fallback itself.
        let mut task = crate::store::task_from_new(new, declared.name.clone(), agent, runtime);
        if let Some(id) = id {
            task.id = id;
        }
        task.workflow_origin = workflow_origin;
        task.bench_origin = bench_origin;
        if let Some(intake) = intake {
            task.status = TaskStatus::Intake;
            task.intake = Some(intake);
        }
        if let Some(s) = &task.schedule {
            task.next_run_at = Some(factory_kernel::schedule_grid::next_after(s, Utc::now())?);
        }

        let task = self.store.create(&task).await?;
        let entry = TaskEntry::new("daemon", "created", format!("created: {}", task.title));
        if let Err(e) = self.store.append_entry(&task.id, &entry).await {
            tracing::warn!(task = task.id, "could not record journal entry: {e}");
        }
        self.observer.entry(&task.id, entry);
        self.observer.created(task.clone());
        Ok(task)
    }
}
pub async fn validate_after(
    store: &dyn TaskStore,
    task_id: Option<&str>,
    after: &[String],
) -> Result<()> {
    if after.is_empty() {
        return Err(FactoryError::BadRequest(
            "after needs at least one upstream task".into(),
        ));
    }
    let mut pending = after.to_vec();
    let mut seen = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if task_id == Some(id.as_str()) {
            return Err(FactoryError::BadRequest(
                "after must not create a dependency cycle".into(),
            ));
        }
        if !seen.insert(id.clone()) {
            continue;
        }
        let parent = store
            .get(&id)
            .await?
            .ok_or_else(|| FactoryError::BadRequest(format!("no upstream task {id}")))?;
        pending.extend(parent.after.into_iter().flatten());
        pending.extend(parent.depends_on);
    }
    Ok(())
}
