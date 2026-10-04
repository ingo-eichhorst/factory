//! Process-owned providers. Ambiguous names never acquire run history.
use super::{AttestedQuery, NamedQuery, RecoveryQuery};
use crate::engine::Engine;
use async_trait::async_trait;
use chrono::{Duration, Utc};
use crate::costs::{group_key, DEFAULT_WINDOW_DAYS};
use factory_core::usage::{CostGroupBy, CostReport, CostReportExt, CostRow, CostRowExt, SpendQuery};
use factory_core::{
    control_plan::{self, RequiredStep},
    operations::Window,
    run::{Run, RunStatus},
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

#[async_trait]
impl Provide<factory_kernel::EnvironmentRecoveryFact> for Provider<'_> {
    type Query = RecoveryQuery;
    type Value = Vec<factory_kernel::EnvironmentRecoveryFact>;
    type Error = FactoryError;
    async fn get(&self, query: &RecoveryQuery) -> Result<Self::Value> {
        use factory_kernel::{RECOVERY_COMMIT_LABEL, RECOVERY_ENVIRONMENT_LABEL, RECOVERY_REASON_LABEL};
        let limit = query.limit.clamp(1, 200);
        let scopes: Vec<Option<&str>> = match &query.scopes {
            Some(scopes) => scopes.iter().map(|scope| Some(scope.as_str())).collect(),
            None => vec![None],
        };
        let mut workflows = Vec::new();
        for scope in scopes {
            workflows.extend(self.engine.workflows.tagged_runs(RECOVERY_ENVIRONMENT_LABEL, scope, limit).await?);
        }
        workflows.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then_with(|| a.id.cmp(&b.id)));
        workflows.truncate(limit as usize);
        let mut facts = Vec::new();
        for workflow in workflows {
            let Some(node) = workflow.definition.nodes.iter().find(|node| node.task.labels.contains_key(RECOVERY_ENVIRONMENT_LABEL)) else { continue; };
            let task_id = workflow.nodes.iter().find(|run| run.node_id == node.id).and_then(|run| run.task_id.clone());
            let run = match &task_id {
                Some(id) => self.engine.store.runs(id, 1).await?.into_iter().next().map(|run| RunFact {
                    id: run.id, status: run.status, started_at: run.started_at, ended_at: run.ended_at,
                }),
                None => None,
            };
            let requested_by = match workflow.started_by {
                factory_core::workflow::WorkflowActor::Owner => "owner".into(),
                factory_core::workflow::WorkflowActor::Agent { scope, name } => format!("{scope}/{name}"),
            };
            facts.push(factory_kernel::EnvironmentRecoveryFact {
                scope: workflow.scope, environment: node.task.labels[RECOVERY_ENVIRONMENT_LABEL].clone(),
                workflow_id: workflow.workflow_id, workflow_run_id: workflow.id, status: workflow.status,
                requested_at: workflow.created_at, requested_by,
                reason: node.task.labels.get(RECOVERY_REASON_LABEL).cloned().unwrap_or_default(),
                expected_commit: node.task.labels.get(RECOVERY_COMMIT_LABEL).cloned(), task_id, run,
            });
        }
        Ok(facts)
    }
}
pub(crate) struct Provider<'a> {
    pub(super) engine: &'a Engine,
}
impl factory_kernel::FactProvider for Provider<'_> {
    type Level = factory_kernel::L4;
}
impl Provider<'_> {
    /// The only spend aggregation, owned by L4; every consumer uses its typed port.
    async fn spend(&self, q: &SpendQuery) -> Result<CostReport> {
        let to = q.to.unwrap_or_else(Utc::now);
        let from = q.from.unwrap_or(to - Duration::days(DEFAULT_WINDOW_DAYS));
        if from >= to {
            return Err(factory_core::FactoryError::BadRequest(format!(
                "the window is empty: {from} is not before {to}"
            )));
        }
        let snapshot = self.engine.factory_snapshot();
        // A scope means its whole subtree, the reading Operations, Policy
        // and Scenarios give it; a name that resolves to nothing is refused.
        let (scope_name, members) = match q.scope.as_deref() {
            None => (None, None),
            Some(name) => {
                let asked = snapshot.scope(name)?;
                let mut members: BTreeSet<String> = snapshot.config.scopes.iter()
                    .filter(|scope| scope.path == asked.path || snapshot.config.ancestors_of(scope)
                        .iter().any(|parent| parent.path == asked.path))
                    .map(|scope| scope.name.clone()).collect();
                members.insert(asked.name.clone());
                (Some(asked.name.clone()), Some(members))
            }
        };

        let runs: Vec<Run> = self.engine
            .store
            .runs_between(from, to)
            .await?
            .into_iter()
            .filter(|r| match q.basis {
                factory_kernel::SpendBasis::Started => r.started_at >= from && r.started_at < to,
                factory_kernel::SpendBasis::Finished => r.status.is_terminal()
                    && r.ended_at.is_some_and(|end| end > from && end <= to),
            })
            .collect();
        let mut tasks: BTreeMap<String, Option<Task>> = BTreeMap::new();
        for run in &runs {
            if !tasks.contains_key(&run.task_id) {
                let task = self.engine.store.get(&run.task_id).await?;
                tasks.insert(run.task_id.clone(), task);
            }
        }

        // Every workflow definition a task in this window points to, looked
        // up once per id and cached -- the `tasks` map's own pattern above.
        // Only built for `CostGroupBy::Workflow`, so no other grouping pays
        // for a workflow store round trip it never asked for. A deleted
        // definition (`get_definition` answers `None`, or errors) caches as
        // `None`: the group still keys by `workflow_id`, just unlabelled.
        let mut workflow_names: BTreeMap<String, Option<String>> = BTreeMap::new();
        if q.group_by == CostGroupBy::Workflow {
            for task in tasks.values().flatten() {
                let Some(origin) = &task.workflow_origin else { continue };
                if workflow_names.contains_key(&origin.workflow_id) {
                    continue;
                }
                let name = self.engine
                    .workflows
                    .get_definition(&origin.workflow_id)
                    .await
                    .ok()
                    .flatten()
                    .map(|def| def.name);
                workflow_names.insert(origin.workflow_id.clone(), name);
            }
        }

        let now = Utc::now();
        // A deleted task, failed lookup or removed scope cannot establish
        // which subtree owns the run. Preserve that uncertainty separately
        // from scoped sums so a budget never treats it as free.
        let unattributed_runs = runs.iter().filter(|run| {
            tasks.get(&run.task_id).and_then(Option::as_ref)
                .is_none_or(|task| snapshot.scope(&task.scope).is_err())
        }).count().min(u32::MAX as usize) as u32;
        let mut rows: BTreeMap<String, CostRow> = BTreeMap::new();
        let mut total = CostRow::new("total", None);
        // `median_actual_over_expected` (`#168`) needs every ratio at once
        // (a nearest-rank median), so each group's own ratios are gathered
        // here and folded into its `CostRow` after the loop, rather than
        // carried on the row itself the way a running sum would be.
        let mut ratios: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        let mut total_ratios: Vec<f64> = Vec::new();
        let mut daily: BTreeMap<chrono::NaiveDate, factory_kernel::DailySpend> = BTreeMap::new();
        let mut cohort = Vec::new();
        for run in &runs {
            let task = tasks.get(&run.task_id).and_then(Option::as_ref);
            let day = run.started_at.date_naive();
            let daily_row = daily.entry(day).or_insert_with(|| factory_kernel::DailySpend {
                day, spent: CostRow::new(day.to_string(), None), unattributed_runs: 0,
            });
            if task.is_none_or(|task| snapshot.scope(&task.scope).is_err()) {
                daily_row.unattributed_runs = daily_row.unattributed_runs.saturating_add(1);
            }
            if let Some(members) = &members {
                // A deleted task's scope is unknown, so it is in no scope.
                if !task.is_some_and(|t| members.contains(&snapshot.canonical_scope_name(&t.scope))) {
                    continue;
                }
            }
            let (key, label) = group_key(q.group_by, run, task, |s| snapshot.canonical_scope_name(s), &workflow_names);
            if q.basis == factory_kernel::SpendBasis::Finished { cohort.push(run.clone()); }
            let terminal_wall = run
                .status
                .is_terminal()
                .then(|| (run.ended_at.unwrap_or(now) - run.started_at).num_seconds().max(0) as u64);
            if let (Some(estimate), Some(wall)) = (run.original_estimate.as_ref(), terminal_wall) {
                if estimate.time.expected > 0 {
                    let ratio = wall as f64 / estimate.time.expected as f64;
                    ratios.entry(key.clone()).or_default().push(ratio);
                    total_ratios.push(ratio);
                }
            }
            let row = rows.entry(key.clone()).or_insert_with(|| CostRow::new(key, label));
            row.add(run.usage.as_ref());
            row.add_estimate(run.original_estimate.as_ref(), terminal_wall);
            total.add(run.usage.as_ref());
            total.add_estimate(run.original_estimate.as_ref(), terminal_wall);
            daily_row.spent.add(run.usage.as_ref());
        }
        let median = |values: &mut [f64]| {
            (!values.is_empty()).then(|| {
                values.sort_by(f64::total_cmp);
                values[factory_kernel::nearest_rank(values.len(), 0.5)]
            })
        };
        for row in rows.values_mut() {
            if let Some(values) = ratios.get_mut(&row.key) {
                row.median_actual_over_expected = median(values);
            }
        }
        total.median_actual_over_expected = median(&mut total_ratios);
        let mut rows: Vec<CostRow> = rows.into_values().collect();
        CostReport::sort_rows(&mut rows);
        let finished = if q.basis == factory_kernel::SpendBasis::Finished {
            let figure = |id: &str, missing: u32| {
                let mut f = factory_core::usage::usage_metric_in_window(id, &cohort, from, to)
                    .expect("the two usage metric names are compiled-in vocabulary");
                let unattributed = if scope_name.is_some() { unattributed_runs } else { 0 };
                if missing > 0 || total.runs_partial > 0 || unattributed > 0 {
                    let coverage = format!("{missing} unmeasured, {} partial, {unattributed} unattributed finished runs; measured-subset figures are not a complete forecasting baseline", total.runs_partial);
                    f.reason = Some(match f.reason { Some(reason) => format!("{reason}; {coverage}"), None => coverage });
                }
                if f.value.is_some_and(|v| !v.is_finite() || v < 0.0) {
                    f.value = None;
                    f.reason = Some("the measured result is not finite and nonnegative".into());
                }
                factory_kernel::SpendFigure { value: f.value, reason: f.reason, as_of: f.as_of }
            };
            Some(factory_kernel::FinishedSpend {
                unit_cost: figure("unit_cost", total.runs_unknown.saturating_add(total.runs_cost_unknown)),
                tokens_per_run: figure("tokens_per_run", total.runs_unknown.saturating_add(total.runs_tokens_incomplete)),
            })
        } else { None };
        Ok(CostReport {
            basis: q.basis, finished,
            group_by: q.group_by,
            from,
            to,
            scope: scope_name,
            rows,
            total,
            unattributed_runs,
            daily: daily.into_values().filter(|day| day.spent.runs > 0 || day.unattributed_runs > 0).collect(),
        })
    }
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
impl Provide<CostReport> for Provider<'_> {
    type Query = SpendQuery;
    type Value = CostReport;
    type Error = FactoryError;
    async fn get(&self, q: &SpendQuery) -> Result<CostReport> {
        self.spend(q).await
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
impl Provide<factory_kernel::ArtifactProvenance> for Provider<'_> {
    type Query = String;
    type Value = Vec<factory_kernel::ArtifactProvenance>;
    type Error = FactoryError;
    async fn get(&self, id: &String) -> Result<Self::Value> {
        self.engine.run_provenance(id).await
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
