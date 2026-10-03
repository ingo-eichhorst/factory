//! Process-owned providers. Ambiguous names never acquire run history.
use super::{AttestedQuery, NamedQuery};
use crate::engine::Engine;
use async_trait::async_trait;
use chrono::Utc;
use factory_core::{
    control_plan::{self, RequiredStep},
    operations::Window,
    run::RunStatus,
};
use factory_core::{
    error::{FactoryError, Result},
    task::{Task, TaskFilter},
    workflow::WorkflowDefinition,
};
use factory_kernel::{
    AttestedRun, ConfirmedSecurityReport, Provide, RunFact, TaskFact, WorkflowFact, WorkflowRunFact,
};
use std::collections::{BTreeMap, BTreeSet};
const RUN_LOOKBACK: u32 = 20;
pub(crate) struct Provider<'a> {
    pub(super) engine: &'a Engine,
}
impl factory_kernel::FactProvider for Provider<'_> {
    type Level = factory_kernel::L4;
}
impl Provider<'_> {
    /// Finished runs in (from, to], with exact canonical scopes/categories.
    /// Frozen dispatch-time requirements and one batch of attestations are
    /// carried unchanged; bench attempts never enter the denominator.
    async fn attested_runs(
        &self,
        scopes: Option<&BTreeSet<String>>,
        categories: Option<&BTreeSet<String>>,
        window: Window,
    ) -> Result<Vec<AttestedRun>> {
        let mut runs = self
            .engine
            .store
            .runs_between(window.from, window.to)
            .await?;
        runs.retain(|r| r.ended_at.is_some_and(|ended| window.contains(ended)));
        if runs.is_empty() {
            return Ok(Vec::new());
        }
        let snapshot = self.engine.factory_snapshot();
        let tasks = self.engine.store.list(&TaskFilter::default()).await?;
        let tasks_by_id: BTreeMap<&str, &Task> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();

        struct Resolved {
            run_id: String,
            task_id: String,
            scope: String,
            category: String,
            agent: String,
            status: RunStatus,
            ended_at: chrono::DateTime<Utc>,
            fail_kind: Option<factory_core::run::FailKind>,
            required_steps: Vec<RequiredStep>,
        }
        let mut resolved = Vec::new();
        for run in runs {
            let Some(task) = tasks_by_id.get(run.task_id.as_str()) else {
                continue;
            };
            if task.bench_origin.is_some() {
                continue;
            }
            let scope = snapshot.canonical_scope_name(&task.scope);
            if let Some(scopes) = scopes {
                if !scopes.contains(&scope) {
                    continue;
                }
            }
            let category = control_plan::effective_category(task.category.as_deref()).to_string();
            if let Some(categories) = categories {
                if !categories.contains(&category) {
                    continue;
                }
            }
            resolved.push(Resolved {
                run_id: run.id.clone(),
                task_id: task.id.clone(),
                scope,
                category,
                agent: run.agent.clone(),
                status: run.status,
                ended_at: run.ended_at.expect("retained above"),
                fail_kind: run.fail_kind,
                required_steps: run.required_steps.clone(),
            });
        }
        if resolved.is_empty() {
            return Ok(Vec::new());
        }
        let run_ids: Vec<String> = resolved.iter().map(|r| r.run_id.clone()).collect();
        let mut attestations_by_run = self.engine.policies.step_attestations_for(&run_ids).await?;
        Ok(resolved
            .into_iter()
            .map(|r| AttestedRun {
                attestations: attestations_by_run.remove(&r.run_id).unwrap_or_default(),
                run_id: r.run_id,
                task_id: r.task_id,
                scope: r.scope,
                category: r.category,
                agent: r.agent,
                status: r.status,
                ended_at: r.ended_at,
                fail_kind: r.fail_kind,
                required_steps: r.required_steps,
            })
            .collect())
    }
    async fn task_facts_for(&self, scoped: &[Task], name: &str) -> Result<Vec<TaskFact>> {
        if let Some(task) = scoped.iter().find(|t| t.id == name) {
            return Ok(vec![self.task_fact(task).await?]);
        }
        let matches: Vec<&Task> = scoped.iter().filter(|t| t.title == name).collect();
        if let [only] = matches.as_slice() {
            return Ok(vec![self.task_fact(only).await?]);
        }
        Ok(matches
            .into_iter()
            .map(|t| TaskFact {
                id: t.id.clone(),
                title: t.title.clone(),
                runs: Vec::new(),
            })
            .collect())
    }

    async fn task_fact(&self, task: &Task) -> Result<TaskFact> {
        // Newest first (`TaskStore::runs`'s own contract), bounded to
        // `RUN_LOOKBACK` rather than the task's whole history: `evaluate`
        // only ever needs to walk past however many runs are still in
        // progress to find the newest *finished* one, and a task normally
        // has at most one of those at a time.
        let runs = self
            .engine
            .store
            .runs(&task.id, RUN_LOOKBACK)
            .await?
            .into_iter()
            .map(|r| RunFact {
                id: r.id,
                status: r.status,
                started_at: r.started_at,
                ended_at: r.ended_at,
            })
            .collect();
        Ok(TaskFact {
            id: task.id.clone(),
            title: task.title.clone(),
            runs,
        })
    }

    async fn workflow_facts_for(
        &self,
        defs: &[WorkflowDefinition],
        scope: &str,
        name: &str,
    ) -> Result<Vec<WorkflowFact>> {
        if let Some(def) = defs.iter().find(|d| d.id == name) {
            return Ok(vec![self.workflow_fact(def, scope).await?]);
        }
        let matches: Vec<&WorkflowDefinition> = defs.iter().filter(|d| d.name == name).collect();
        if let [only] = matches.as_slice() {
            return Ok(vec![self.workflow_fact(only, scope).await?]);
        }
        Ok(matches
            .into_iter()
            .map(|d| WorkflowFact {
                id: d.id.clone(),
                name: d.name.clone(),
                runs: Vec::new(),
            })
            .collect())
    }

    async fn workflow_fact(&self, def: &WorkflowDefinition, scope: &str) -> Result<WorkflowFact> {
        let runs = self
            .engine
            .workflows
            .runs(Some(&def.id), Some(scope), RUN_LOOKBACK)
            .await?
            .into_iter()
            .map(|r| WorkflowRunFact {
                id: r.id,
                status: r.status,
                updated_at: r.updated_at,
            })
            .collect();
        Ok(WorkflowFact {
            id: def.id.clone(),
            name: def.name.clone(),
            runs,
        })
    }
}
#[async_trait]
impl Provide<TaskFact> for Provider<'_> {
    type Query = NamedQuery;
    type Value = BTreeMap<String, Vec<TaskFact>>;
    type Error = FactoryError;
    async fn get(&self, q: &NamedQuery) -> Result<Self::Value> {
        if q.names.is_empty() {
            return Ok(BTreeMap::new());
        }
        let snapshot = self.engine.factory_snapshot();
        let scope = snapshot.scope(&q.scope)?.name.clone();
        let tasks = self
            .engine
            .store
            .list(&TaskFilter {
                scope: Some(scope),
                ..Default::default()
            })
            .await?;
        let mut facts = BTreeMap::new();
        for name in &q.names {
            facts.insert(name.clone(), self.task_facts_for(&tasks, name).await?);
        }
        Ok(facts)
    }
}
#[async_trait]
impl Provide<WorkflowFact> for Provider<'_> {
    type Query = NamedQuery;
    type Value = BTreeMap<String, Vec<WorkflowFact>>;
    type Error = FactoryError;
    async fn get(&self, q: &NamedQuery) -> Result<Self::Value> {
        if q.names.is_empty() {
            return Ok(BTreeMap::new());
        }
        let snapshot = self.engine.factory_snapshot();
        let scope = snapshot.scope(&q.scope)?.name.clone();
        let defs = self.engine.workflows.definitions(Some(&scope)).await?;
        let mut facts = BTreeMap::new();
        for name in &q.names {
            facts.insert(
                name.clone(),
                self.workflow_facts_for(&defs, &scope, name).await?,
            );
        }
        Ok(facts)
    }
}
#[async_trait]
impl Provide<AttestedRun> for Provider<'_> {
    type Query = AttestedQuery;
    type Value = Vec<AttestedRun>;
    type Error = FactoryError;
    async fn get(&self, q: &AttestedQuery) -> Result<Self::Value> {
        self.attested_runs(q.scopes.as_ref(), q.categories.as_ref(), q.window)
            .await
    }
}
#[async_trait]
impl Provide<ConfirmedSecurityReport> for Provider<'_> {
    type Query = Option<String>;
    type Value = Vec<ConfirmedSecurityReport>;
    type Error = FactoryError;
    async fn get(&self, scope: &Self::Query) -> Result<Self::Value> {
        self.engine
            .confirmed_security_reports(scope.as_deref())
            .await
    }
}
