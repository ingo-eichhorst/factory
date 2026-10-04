//! One-shot upstream triggers and workflow-owned waiting tasks.
use crate::access::Caller;
use crate::engine::Engine;
use chrono::Utc;
use factory_core::error::{FactoryError, Result};
use factory_core::task::{
    CloseReason, Task, TaskClosure, TaskEntry, TaskPatch, TaskStatus, WorkflowOrigin,
};
use factory_core::workflow::{
    WorkflowNodeKind, WorkflowNodeStatus, WorkflowRun, WorkflowRunStatus,
};
use std::collections::BTreeSet;

impl Engine {
    pub(crate) async fn validate_after(&self,task_id:Option<&str>,after:&[String])->Result<()>{
        factory_process::creation::validate_after(self.store.as_ref(),task_id,after).await
    }
    pub(crate) async fn waiting_description(&self, task: &Task) -> Result<String> {
        let mut names = Vec::new();
        for id in task.after.iter().flatten() {
            names.push(match self.store.get(id).await? {
                Some(parent) => format!("{} ({id})", parent.title),
                None => format!("{id} (missing)"),
            });
        }
        let mut description = if names.is_empty() {
            "waiting for workflow release".into()
        } else {
            format!("waiting on {}", names.join(", "))
        };
        if let Some(condition) = &task.after_condition {
            description.push_str(&format!("; {condition}"));
        }
        Ok(description)
    }

    pub(crate) async fn override_waiting(
        &self,
        task: &Task,
        caller: &Caller,
        reason: &str,
    ) -> Result<()> {
        let _guard = self.workflow_edit.lock().await;
        let _admission = self.admission_lock.lock().await;
        let task = self.require(&task.id).await?;
        if task.after.is_none() {
            return Err(FactoryError::BadRequest(
                "--override-wait requires an after trigger".into(),
            ));
        }
        if let Some(origin) = &task.workflow_origin {
            let mut workflow = self.workflow_run(&origin.workflow_run_id).await?;
            if workflow.status.is_terminal() {
                return Err(FactoryError::BadRequest(
                    "the workflow already ended; its waiting work cannot be released".into(),
                ));
            }
            if let Some(node) = workflow
                .nodes
                .iter_mut()
                .find(|node| node.node_id == origin.node_id)
            {
                node.status = WorkflowNodeStatus::Pending;
            }
            self.workflows.put_run(&workflow).await?;
        }
        self.store
            .update(
                &task.id,
                &TaskPatch {
                    clear_after: true,
                    ..Default::default()
                },
            )
            .await?;
        let asked = crate::operations::Asked::new(caller, Some(reason.into()));
        self.entry(
            &task.id,
            asked.entry(
                "waiting_override",
                format!("upstream wait explicitly overridden {}", asked.words()),
                serde_json::json!({"after": task.after, "condition": task.after_condition}),
            ),
        )
        .await;
        self.publish_task(&task.id).await;
        Ok(())
    }

    /// Called under workflow_edit. Choose all ids, persist the decision,
    /// then create tasks without releasing any. A receipt makes deletion
    /// different from an interrupted persist-before-create sequence.
    pub(crate) async fn materialize_workflow_tasks(&self, run: &mut WorkflowRun) -> Result<()> {
        if run.status.is_terminal() {
            return Ok(());
        }
        for node in &mut run.nodes {
            let executable = run.definition.nodes.iter().any(|definition| {
                definition.id == node.node_id
                    && matches!(
                        definition.kind,
                        WorkflowNodeKind::Task | WorkflowNodeKind::Review
                    )
            });
            if executable && node.task_id.is_none() && !node.status.is_terminal() {
                node.task_id = Some(uuid::Uuid::new_v4().to_string());
                node.task_created = false;
            }
        }
        self.workflows.put_run(run).await?;
        for index in 0..run.nodes.len() {
            let node = run.nodes[index].clone();
            let Some(task_id) = node.task_id.clone() else {
                continue;
            };
            let task = self.store.get(&task_id).await?;
            let Some(definition) = run
                .definition
                .nodes
                .iter()
                .find(|definition| definition.id == node.node_id)
            else {
                continue;
            };
            if !matches!(
                definition.kind,
                WorkflowNodeKind::Task | WorkflowNodeKind::Review
            ) {
                continue;
            }
            let after = upstream_tasks(run, &node.node_id);
            let condition = route_condition(run, &node.node_id);
            if task.is_some() {
                run.nodes[index].task_created = true;
                // A new feedback round waits again, but an explicit early
                // run or a queued admission is not put back behind its gate.
                let previous = self.store.runs(&task_id, 1).await?.into_iter().next();
                if node.status == WorkflowNodeStatus::Unstarted
                    && node.round > 0
                    && previous
                        .as_ref()
                        .is_some_and(|previous| previous.workflow_round < node.round)
                    && self.store.active_run(&task_id).await?.is_none()
                {
                    self.store
                        .update(
                            &task_id,
                            &TaskPatch {
                                status: Some(TaskStatus::Pending),
                                clear_after: true,
                                after: Some(after),
                                after_condition: condition,
                                clear_result: true,
                                clear_error: true,
                                clear_routed_to: true,
                                clear_failure: true,
                                clear_closure: true,
                                ..Default::default()
                            },
                        )
                        .await?;
                    self.publish_task(&task_id).await;
                }
                continue;
            }
            if node.task_created {
                continue;
            } // deleted, never regenerate it
            let caller = self.caller_for_actor(&run.started_by).await;
            let mut template = definition.task.clone();
            if definition.kind == WorkflowNodeKind::Review {
                template.worktree = Some(false);
            }
            template.category = Some(run.definition.node_category(definition));
            template.after = Some(after);
            template.after_condition = condition;
            if run.integration.is_some() || template.decomposition_part.is_some() {
                template.depends_on = template.after.clone().unwrap_or_default();
            }
            if let Err(error) = self.authorize_workflow_spawn(&caller, &template).await {
                fail_materialization(run, index, error.to_string());
                self.workflows.put_run(run).await?;
                return Ok(());
            }
            let origin = WorkflowOrigin {
                workflow_id: run.workflow_id.clone(),
                workflow_run_id: run.id.clone(),
                node_id: node.node_id.clone(),
                workspace: run.task_workspace(),
            };
            let created = if definition.kind == WorkflowNodeKind::Review {
                self.create_review_task(template, Some(origin), task_id)
                    .await
            } else {
                self.create_workflow_task(template, origin, task_id).await
            };
            if let Err(error) = created {
                fail_materialization(run, index, error.to_string());
                self.workflows.put_run(run).await?;
                return Ok(());
            }
            run.nodes[index].task_created = true;
            self.workflows.put_run(run).await?;
        }
        Ok(())
    }

    /// Close only work never admitted. Running/blocked siblings keep their
    /// authoritative outcome; even abandon-cancel closes waiting tasks.
    pub(crate) async fn close_workflow_waits(&self, run: &WorkflowRun) -> Result<()> {
        for node in &run.nodes {
            if !run.status.is_terminal()
                && !matches!(
                    node.status,
                    WorkflowNodeStatus::Skipped | WorkflowNodeStatus::SkippedByRoute
                )
            {
                continue;
            }
            let Some(id) = &node.task_id else { continue };
            let Some(task) = self.store.get(id).await? else {
                continue;
            };
            if (task.after.is_none() && task.status != TaskStatus::Pending)
                || task.status.is_terminal()
                || self.store.active_run(id).await?.is_some()
            {
                continue;
            }
            let note = node.skip_reason.clone().unwrap_or_else(|| {
                format!("workflow {} ended without admitting this step", run.id)
            });
            self.store
                .update(
                    id,
                    &TaskPatch {
                        status: Some(TaskStatus::Cancelled),
                        clear_after: true,
                        clear_slot_wait: true,
                        clear_pending_retry: true,
                        closure: Some(TaskClosure {
                            reason: CloseReason::NotPlanned,
                            duplicate_of: None,
                            note: Some(note.clone()),
                            by: "the workflow".into(),
                            at: Utc::now(),
                        }),
                        ..Default::default()
                    },
                )
                .await?;
            self.entry(
                id,
                TaskEntry::new("daemon", "closed", format!("not_planned: {note}")),
            )
            .await;
            self.publish_task(id).await;
        }
        Ok(())
    }
}

fn fail_materialization(run: &mut WorkflowRun, index: usize, error: String) {
    run.status = WorkflowRunStatus::Failed;
    run.failure_node_id = Some(run.nodes[index].node_id.clone());
    run.error = Some(error.clone());
    run.nodes[index].task_id = None;
    run.nodes[index].status = WorkflowNodeStatus::Failed;
    run.nodes[index].error = Some(error);
    for node in &mut run.nodes {
        if node.status == WorkflowNodeStatus::Unstarted {
            node.status = WorkflowNodeStatus::Skipped;
        }
    }
}

/// Gates are annotations without harness tasks; walk through them to the
/// nearest executable parents. Eligibility still belongs to the graph.
fn upstream_tasks(run: &WorkflowRun, target: &str) -> Vec<String> {
    let mut pending: Vec<String> = run
        .definition
        .edges
        .iter()
        .filter(|edge| edge.to == target)
        .map(|edge| edge.from.clone())
        .collect();
    let mut seen = BTreeSet::new();
    let mut tasks = BTreeSet::new();
    while let Some(node) = pending.pop() {
        if !seen.insert(node.clone()) {
            continue;
        }
        if let Some(task) = run
            .nodes
            .iter()
            .find(|candidate| candidate.node_id == node)
            .and_then(|candidate| candidate.task_id.clone())
        {
            tasks.insert(task);
        } else {
            pending.extend(
                run.definition
                    .edges
                    .iter()
                    .filter(|edge| edge.to == node)
                    .map(|edge| edge.from.clone()),
            );
        }
    }
    tasks.into_iter().collect()
}

fn route_condition(run: &WorkflowRun, target: &str) -> Option<String> {
    let mut choices = Vec::new();
    for from in &run.definition.nodes {
        for exit in &from.exits {
            if !run.definition.ancestors(&exit.to).contains(&from.id) {
                continue;
            }
            let mut routed = run.clone();
            routed.route_forward(&from.id, &exit.to);
            if routed.nodes.iter().any(|node| {
                node.node_id == target && node.status == WorkflowNodeStatus::SkippedByRoute
            }) {
                choices.push(format!("{} → {}", from.task.title, exit.to));
            }
        }
    }
    (!choices.is_empty()).then(|| {
        format!(
            "conditional: skipped if {} is selected",
            choices.join(" or ")
        )
    })
}
