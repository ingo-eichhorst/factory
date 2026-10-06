//! L4 releases intent through L3; L2 alone checks and removes owned workspaces.
use crate::engine::Engine;
use factory_core::adapter::runtime::RuntimeStatus;
use factory_core::error::{FactoryError, Result};
use factory_core::task::{TaskEntry, TaskFilter, TaskStatus};
use factory_core::workflow::WorkflowNodeStatus;
use factory_kernel::WorkspaceLifetime;
use std::path::Path;

impl Engine {
    /// Upgrade legacy ownership from authoritative run receipts, never from a
    /// directory/branch scan. Require the exact daemon-generated run directory.
    pub(crate) async fn recover_workspaces(&self) {
        let factory = self.factory_snapshot();
        let Ok(tasks) = self.l4.store.list(&TaskFilter::default()).await else {
            return;
        };
        let mut known = match self.l4.workspaces.records().await {
            Ok(records) => records
                .into_iter()
                .map(|record| record.path)
                .collect::<std::collections::HashSet<_>>(),
            Err(error) => {
                tracing::warn!("workspace recovery: {error}");
                return;
            }
        };
        for task in tasks {
            let Ok(scope) = factory.scope_path(&task.scope) else {
                continue;
            };
            let Ok(attempts) = self.l4.store.runs(&task.id, u32::MAX).await else {
                continue;
            };
            for attempt in attempts {
                let (Some(path), Some(branch)) = (attempt.worktree_path, attempt.worktree_branch)
                else {
                    continue;
                };
                let path = std::path::PathBuf::from(path);
                if known.contains(&path)
                    || !path.exists()
                    || path != factory.worktrees_dir().join(&attempt.id)
                {
                    continue;
                }
                let spec = factory_kernel::WorkspaceSpec {
                    task_id: task.id.clone(),
                    workflow_run_id: task
                        .workflow_origin
                        .as_ref()
                        .map(|origin| origin.workflow_run_id.clone()),
                    lifetime: if task.bench_origin.is_some() {
                        WorkspaceLifetime::Run
                    } else {
                        WorkspaceLifetime::Task
                    },
                };
                match self
                    .l4.workspaces
                    .adopt(spec, &scope, path.clone(), branch)
                    .await
                {
                    Ok(_) => {
                        known.insert(path.clone());
                        self.entry(
                            &task.id,
                            TaskEntry::new(
                                "daemon",
                                "workspace_adopted",
                                format!("adopted recorded legacy workspace {}", path.display()),
                            ),
                        )
                        .await;
                    }
                    Err(error) => {
                        self.entry(
                            &task.id,
                            TaskEntry::new(
                                "daemon",
                                "workspace_retained",
                                format!(
                                    "legacy workspace {} was not adopted: {error}",
                                    path.display()
                                ),
                            ),
                        )
                        .await
                    }
                }
            }
        }
        // Integration workspaces have their own durable workflow receipt,
        // not an agent run. Apply the same exact-path ownership proof.
        if let Ok(workflows) = self.l4.workflows.runs(None, None, u32::MAX).await {
            for workflow in workflows {
                let Some(integration) = workflow.integration else {
                    continue;
                };
                let path = std::path::PathBuf::from(integration.worktree_path);
                if known.contains(&path)
                    || !path.exists()
                    || path
                        != factory
                            .worktrees_dir()
                            .join(format!("integration-{}", workflow.id))
                {
                    continue;
                }
                let Ok(scope) = factory.scope_path(&workflow.scope) else {
                    continue;
                };
                let spec = factory_kernel::WorkspaceSpec {
                    task_id: integration.parent_task_id.clone(),
                    workflow_run_id: Some(workflow.id),
                    lifetime: WorkspaceLifetime::Task,
                };
                match self
                    .l4.workspaces
                    .adopt(spec, &scope, path.clone(), integration.branch)
                    .await
                {
                    Ok(_) => {
                        known.insert(path);
                    }
                    Err(error) => {
                        self.entry(
                            &integration.parent_task_id,
                            TaskEntry::new(
                                "daemon",
                                "workspace_retained",
                                format!(
                                    "legacy integration workspace {} was not adopted: {error}",
                                    path.display()
                                ),
                            ),
                        )
                        .await
                    }
                }
            }
        }
        self.sweep_workspaces().await;
    }

    pub(crate) async fn require_workspace_quiet(
        &self,
        task_id: &str,
        path: &Path,
        excluding: Option<&str>,
    ) -> Result<()> {
        for attempt in self.l4.store.runs(task_id, u32::MAX).await? {
            if excluding == Some(attempt.id.as_str())
                || attempt
                    .worktree_path
                    .as_deref()
                    .map(Path::new)
                    .is_some_and(|previous| previous != path)
            {
                continue;
            }
            if !attempt.status.is_terminal() {
                return Err(FactoryError::BadRequest(
                    "another run still owns this workspace".into(),
                ));
            }
            if let Some(session) = attempt.last_session.as_ref().or(attempt.session.as_ref()) {
                let runtime = self.shared.registry.runtime(&session.runtime)?;
                if !matches!(runtime.status(session).await, Ok(RuntimeStatus::Gone)) {
                    return Err(FactoryError::BadRequest(format!(
                        "workspace retained: session for run {} is not confirmed gone",
                        attempt.id
                    )));
                }
            }
        }
        Ok(())
    }

    /// Startup and periodic backstop. The admission lock protects the whole
    /// eligibility/release interval, not just a stale snapshot of active runs.
    /// No workflow_edit acquisition here: callers may already hold that lock.
    pub(crate) async fn sweep_workspaces(&self) {
        if let Err(error) = self.release_closed_workspaces().await {
            tracing::warn!("workspace sweep: {error}");
        }
    }

    async fn release_closed_workspaces(&self) -> Result<()> {
        let _admission = self.l4.admission_lock.lock().await;
        let records = self
            .l4.workspaces
            .records()
            .await
            .map_err(|e| FactoryError::adapter("workspace", e))?;
        if records.is_empty() {
            return Ok(());
        }
        let tasks = self.l4.store.list(&TaskFilter::default()).await?;
        let active = self.l4.store.active_runs().await?;
        let mut eligible = Vec::new();
        let mut completed_workflows = std::collections::BTreeSet::new();
        let mut workflow_eligibility = std::collections::BTreeMap::new();
        for record in &records {
            // Bench workspaces are evidence, explicitly removed by bench clean.
            if record.spec.lifetime == WorkspaceLifetime::Run {
                continue;
            }
            let task = tasks.iter().find(|task| task.id == record.spec.task_id);
            let workflow = if let Some(id) = &record.spec.workflow_run_id {
                if let Some(eligible_workflow) = workflow_eligibility.get(id) {
                    if *eligible_workflow {
                        eligible.push(record.path.clone());
                    }
                    continue;
                }
                workflow_eligibility.insert(id.clone(), false);
                let Some(workflow) = self.l4.workflows.get_run(id).await? else {
                    continue;
                };
                if !workflow.status.is_terminal() {
                    continue;
                }
                if workflow.nodes.iter().any(|node| {
                    node.task_id
                        .as_ref()
                        .is_some_and(|id| !tasks.iter().any(|task| task.id == *id))
                }) {
                    continue;
                }
                if workflow.nodes.iter().any(|node| {
                    matches!(
                        node.status,
                        WorkflowNodeStatus::Dispatching
                            | WorkflowNodeStatus::Running
                            | WorkflowNodeStatus::Blocked
                            | WorkflowNodeStatus::Verifying
                    )
                }) {
                    continue;
                }
                let siblings: Vec<_> = tasks
                    .iter()
                    .filter(|task| {
                        task.workflow_origin
                            .as_ref()
                            .is_some_and(|origin| origin.workflow_run_id == *id)
                    })
                    .collect();
                if siblings
                    .iter()
                    .any(|task| active.iter().any(|run| run.task_id == task.id))
                {
                    continue;
                }
                // Include verification runs whose tasks may have no graph node.
                if active.iter().any(|attempt| {
                    workflow
                        .nodes
                        .iter()
                        .any(|node| node.task_id.as_deref() == Some(&attempt.task_id))
                }) {
                    continue;
                }
                let mut quiet = true;
                for sibling in &siblings {
                    for attempt in self.l4.store.runs(&sibling.id, u32::MAX).await? {
                        if let Some(path) = attempt.worktree_path {
                            if self
                                .require_workspace_quiet(&sibling.id, Path::new(&path), None)
                                .await
                                .is_err()
                            {
                                quiet = false;
                                break;
                            }
                        }
                    }
                    if !quiet {
                        break;
                    }
                }
                if !quiet {
                    continue;
                }
                workflow_eligibility.insert(id.clone(), true);
                Some(workflow)
            } else {
                let Some(task) = task else {
                    continue;
                }; // Unknown/deleted owner: retain.
                if !matches!(task.status, TaskStatus::Done | TaskStatus::Cancelled)
                    || (task.schedule.is_some() && task.closure.is_none())
                    || active.iter().any(|run| run.task_id == task.id)
                {
                    continue;
                }
                if self
                    .require_workspace_quiet(&task.id, &record.path, None)
                    .await
                    .is_err()
                {
                    continue;
                }
                None
            };
            eligible.push(record.path.clone());
            if let Some(workflow) = workflow {
                completed_workflows.insert(workflow.id);
            }
        }
        for (record, outcome) in crate::assignments::release(&self.l4.workspaces, &eligible)
            .await
            .map_err(|e| FactoryError::adapter("workspace", e))?
        {
            let (kind, description) = match outcome {
                Ok(()) => (
                    "workspace_released",
                    format!("released {} on {}", record.path.display(), record.branch),
                ),
                Err(reason) => (
                    "workspace_retained",
                    format!(
                        "retained {} on {}: {reason}",
                        record.path.display(),
                        record.branch
                    ),
                ),
            };
            self.entry(&record.spec.task_id, TaskEntry::new("daemon", kind, description)
                .with_data(serde_json::json!({ "path": record.path, "branch": record.branch, "workflow_run_id": record.spec.workflow_run_id }))).await;
        }
        let remaining = self
            .l4.workspaces
            .records()
            .await
            .map_err(|e| FactoryError::adapter("workspace", e))?;
        for id in completed_workflows {
            if remaining
                .iter()
                .any(|record| record.spec.workflow_run_id.as_deref() == Some(&id))
            {
                continue;
            }
            if let Some(workflow) = self.l4.workflows.mark_workspace_cleanup(&id).await? {
                self.shared.bus
                    .publish(factory_core::event::Event::WorkflowRunUpdated { run: workflow });
            }
        }
        Ok(())
    }
}
