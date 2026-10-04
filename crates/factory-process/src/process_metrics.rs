//! L4 owns all process metric reads. L6 receives measurements and production
//! buckets, never a task store, run objects or another level's wire report.
use crate::measurements::{MeasurementProvider, ProcessMetricsQuery, ProductionQuery};
use crate::{
    task::{TaskFilter, TaskStatus},
    window::Window,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use factory_kernel::{FactoryError, Result};
use factory_kernel::{ProcessMetricFact, ProductionBucket, ProductionFact, Provide};
use std::collections::{BTreeMap, BTreeSet};

const OPERATIONS: &[&str] = &[
    "cycle_time_p50",
    "cycle_time_p85",
    "queue_wait_p95",
    "fail_rate",
    "rework_rate",
    "time_to_recover_p50",
];
const INTAKE: &[&str] = &[
    "ready_rate",
    "needs_info_rate",
    "duplicate_rate",
    "intake_lead_time",
];
const INTAKE_KINDS: &[&str] = &[
    crate::intake::TRIAGE_VERDICT_KIND,
    "intake_needs_info",
    "intake_closed",
    "intake_split",
];

fn fact(
    name: &str,
    value: Option<f64>,
    as_of: DateTime<Utc>,
    reason: Option<String>,
) -> ProcessMetricFact {
    ProcessMetricFact {
        name: name.into(),
        value,
        as_of,
        reason,
    }
}

#[async_trait]
impl Provide<ProcessMetricFact> for MeasurementProvider<'_> {
    type Query = ProcessMetricsQuery;
    type Value = BTreeMap<String, ProcessMetricFact>;
    type Error = FactoryError;
    async fn get(&self, query: &Self::Query) -> Result<Self::Value> {
        if query
            .window_days
            .is_some_and(|days| !matches!(days, 1 | 14 | 90))
        {
            return Err(FactoryError::BadRequest(
                "unsupported process metric window".into(),
            ));
        }
        let snapshot = &self.scopes;
        let (asked, scopes) = snapshot.subtree_scopes(query.scope.as_deref())?;
        let selected: BTreeSet<_> = scopes.iter().map(|scope| scope.name.clone()).collect();
        let days = query.window_days.unwrap_or(28);
        let now = query.now;
        let window = Window::trailing(now, days);
        let needs_runs = query
            .names
            .iter()
            .any(|name| OPERATIONS.contains(&name.as_str()) || name == "estimate_accuracy");
        let needs_intake = query
            .names
            .iter()
            .any(|name| INTAKE.contains(&name.as_str()));
        let needs_goals = query
            .names
            .iter()
            .any(|name| name.starts_with("goal_tasks_done."));
        let tasks = if (needs_runs && asked.is_some()) || needs_intake || needs_goals {
            self.store.list(&TaskFilter::default()).await?
        } else {
            Vec::new()
        };
        let mut values = BTreeMap::new();
        if needs_runs {
            // Recovery streaks may begin before the metric window. Preserve
            // the existing twice-window fetch and each evaluator's own cut.
            let mut runs = self
                .store
                .runs_between(now - chrono::Duration::days(2 * days), now)
                .await?;
            if asked.is_some() {
                let ids: BTreeSet<_> = tasks
                    .iter()
                    .filter(|task| selected.contains(&snapshot.canonical_scope_name(&task.scope)))
                    .map(|task| task.id.as_str())
                    .collect();
                runs.retain(|run| ids.contains(run.task_id.as_str()));
            }
            for name in query
                .names
                .iter()
                .filter(|name| OPERATIONS.contains(&name.as_str()))
            {
                let figure = crate::operations::registry_metric(name, &runs, &window)
                    .ok_or_else(|| FactoryError::BadRequest("unknown process metric".into()))?;
                let at = figure
                    .value
                    .and(crate::operations::registry_metric_as_of(
                        name, &runs, &window,
                    ))
                    .unwrap_or(now);
                values.insert(name.clone(), fact(name, figure.value, at, figure.reason));
            }
            if query.names.contains("estimate_accuracy") {
                let figure = crate::usage::usage_metric("estimate_accuracy", &runs, now, days)
                    .ok_or_else(|| FactoryError::BadRequest("unknown usage metric".into()))?;
                values.insert(
                    "estimate_accuracy".into(),
                    fact(
                        "estimate_accuracy",
                        figure.value,
                        figure.as_of.unwrap_or(now),
                        figure.reason,
                    ),
                );
            }
        }
        if query.names.contains("agent_hours") || query.names.contains("blocked_hours") {
            let hours_days = query.window_days.unwrap_or(14);
            let observed_now = Utc::now();
            let (from, to) = crate::occupancy_history::window_bounds(
                None,
                Some(now - chrono::Duration::days(hours_days)),
                Some(now),
                observed_now,
            )?;
            let blocks =
                crate::occupancy_history::read_blocks(self.store, snapshot, from, to, observed_now)
                    .await?;
            let mut busy = 0i64;
            let mut blocked = 0i64;
            for ((scope, _agent), mut row) in blocks {
                if asked.is_some() && !selected.contains(&scope) {
                    continue;
                }
                let (busy_seconds, blocked_seconds, _, _) =
                    crate::occupancy_history::lay_out(&mut row, from, observed_now.min(to));
                busy += busy_seconds;
                blocked += blocked_seconds;
            }
            for (name, seconds) in [("agent_hours", busy), ("blocked_hours", blocked)] {
                if query.names.contains(name) {
                    values.insert(
                        name.into(),
                        fact(name, Some(seconds as f64 / 3600.0), now, None),
                    );
                }
            }
        }
        if needs_intake {
            let entries = self
                .store
                .entries_of_kinds(INTAKE_KINDS, window.from)
                .await?;
            let tasks: BTreeMap<_, _> = tasks.iter().map(|task| (task.id.as_str(), task)).collect();
            let mut decisions = Vec::new();
            let mut skipped = 0;
            for (task_id, entry) in entries {
                let Some(task) = tasks.get(task_id.as_str()) else {
                    continue;
                };
                let Some(record) = &task.intake else {
                    continue;
                };
                if asked.is_some()
                    && !selected.contains(&snapshot.canonical_scope_name(&task.scope))
                {
                    continue;
                }
                match crate::intake::decision_event(
                    &entry.kind,
                    entry.data.as_ref(),
                    entry.at,
                    record.received_at,
                ) {
                    Some(Ok(decision)) => decisions.push(decision),
                    Some(Err(())) => skipped += 1,
                    None => {}
                }
            }
            for name in query
                .names
                .iter()
                .filter(|name| INTAKE.contains(&name.as_str()))
            {
                let figure =
                    crate::intake::registry_metric(name, &decisions, &window, skipped, days)
                        .ok_or_else(|| FactoryError::BadRequest("unknown intake metric".into()))?;
                values.insert(
                    name.clone(),
                    fact(
                        name,
                        figure.value,
                        figure.as_of.unwrap_or(now),
                        figure.reason,
                    ),
                );
            }
        }
        // Goal labels are deliberately instance-wide, as before: the goal
        // itself may own a different subtree than this metric request.
        for name in query
            .names
            .iter()
            .filter(|name| name.starts_with("goal_tasks_done."))
        {
            let rest = name.strip_prefix("goal_tasks_done.").unwrap();
            let (objective, kr) = rest
                .split_once('.')
                .ok_or_else(|| FactoryError::BadRequest("malformed goal task metric".into()))?;
            let label = format!("{objective}/{kr}");
            let done = tasks
                .iter()
                .filter(|task| {
                    task.status == TaskStatus::Done && task.labels.get("goal") == Some(&label)
                })
                .count();
            values.insert(name.clone(), fact(name, Some(done as f64), now, None));
        }
        if values.len() != query.names.len() {
            return Err(FactoryError::BadRequest(
                "a requested metric is not produced by L4".into(),
            ));
        }
        Ok(values)
    }
}

#[async_trait]
impl Provide<ProductionFact> for MeasurementProvider<'_> {
    type Query = ProductionQuery;
    type Value = ProductionFact;
    type Error = FactoryError;
    async fn get(&self, query: &Self::Query) -> Result<Self::Value> {
        let snapshot = &self.scopes;
        let (asked, scopes) = if query.subtree {
            snapshot.subtree_scopes(query.scope.as_deref())?
        } else {
            (None, Vec::new())
        };
        if !query.subtree || asked.is_none() {
            return self
                .production_at(
                    query.minutes,
                    Some(query.bin),
                    query.scope.clone(),
                    query.now,
                )
                .await;
        }
        let mut total = None;
        for scope in scopes {
            let next = self
                .production_at(
                    query.minutes,
                    Some(query.bin),
                    Some(scope.name.clone()),
                    query.now,
                )
                .await?;
            match &mut total {
                None => total = Some(next),
                Some(total) => merge_production(total, next),
            }
        }
        total.ok_or_else(|| FactoryError::BadRequest("scope has no configured subtree".into()))
    }
}

fn merge_production(total: &mut ProductionFact, next: ProductionFact) {
    total.from = total.from.min(next.from);
    total.to = total.to.max(next.to);
    total.earliest_run = match (total.earliest_run, next.earliest_run) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    merge_buckets(&mut total.buckets, next.buckets);
    merge_buckets(&mut total.daily, next.daily);
}
fn merge_buckets(total: &mut Vec<ProductionBucket>, next: Vec<ProductionBucket>) {
    let mut positions: BTreeMap<DateTime<Utc>, usize> = total
        .iter()
        .enumerate()
        .map(|(index, bucket)| (bucket.from, index))
        .collect();
    for bucket in next {
        let day = bucket.from;
        if let Some(index) = positions.get(&day).copied() {
            let target = &mut total[index];
            target.from = target.from.min(bucket.from);
            target.to = target.to.max(bucket.to);
            target.finished += bucket.finished;
            target.scrapped += bucket.scrapped;
            target.reworked += bucket.reworked;
            target.first_pass += bucket.first_pass;
            target.partial |= bucket.partial;
        } else {
            positions.insert(day, total.len());
            total.push(bucket);
        }
    }
    total.sort_by_key(|bucket| bucket.from);
}
