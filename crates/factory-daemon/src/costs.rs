//! Cost per run (#117): taking usage snapshots, and reading them back.
//!
//! Usage comes from the agent runtime and nowhere else --
//! `AgentRuntime::usage`, which herdr answers through a plugin. A run is
//! read three times: once its session is up and before it is handed the
//! task (the baseline), at each turn the harness says ended, and as it ends,
//! before its session is closed. Each reading is appended to the store,
//! answered or not; `factory_core::usage::run_usage` turns the lot into the
//! run's `usage`, which is written back onto the run so every reader of a
//! run -- `run show`, the UI, `/api/costs`, the metrics -- sees it without
//! knowing snapshots exist.
//!
//! Nothing here can fail a run. A snapshot that cannot be taken is recorded
//! as unknown with the reason, and a store that cannot keep it is logged
//! and passed over: usage is an observation, and a run's outcome never
//! depends on it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use factory_core::error::Result;
use factory_core::event::Event;
use factory_core::run::{Run, RunPatch};
#[cfg(test)]
use factory_core::task::Task;
use factory_core::usage::{
    allocate_plan_share, run_usage, run_usage_with_prior, CostRow, CostRowExt, EstimateComparison,
    HarnessUsage, ReEstimate, RunUsage, RunUsageEntry, SnapshotPoint, TaskUsage, UsageSnapshot,
};
#[cfg(test)]
use factory_core::usage::{CostGroupBy, CostReport, SpendQuery};

use crate::l4_service::L4Service;
use crate::engine::Engine;

#[cfg(test)]
use factory_process::measurements::{NO_ISSUE, NO_WORKFLOW};

impl L4Service<'_> {
    /// Ask the run's runtime what its session has used, keep the answer (or
    /// why there was none), and bring the run's derived `usage` up to date.
    /// A run with no session has nothing to ask about and gets no snapshot.
    pub(crate) async fn snapshot_usage(&self, run: &Run, point: SnapshotPoint) {
        let Some(session) = &run.session else { return };
        let at = Utc::now();
        if point == SnapshotPoint::TurnEnded {
            self.state.turn_usage_pending
                .lock()
                .unwrap()
                .entry(run.id.clone())
                .or_default()
                .insert(at);
        }
        self.capture_usage_snapshot(run, point, at, session).await;
        if point == SnapshotPoint::TurnEnded {
            let earliest_settled = {
                let mut pending = self.state.turn_usage_pending.lock().unwrap();
                let times = pending.entry(run.id.clone()).or_default();
                times.remove(&at);
                let ready = !times.iter().any(|pending_at| *pending_at < at);
                if times.is_empty() {
                    pending.remove(&run.id);
                }
                ready
            };
            if earliest_settled {
                // Serialize this with snapshot writes. A later turn or run-end
                // may already be stored, so record_re_estimate independently
                // selects and caps itself at the first turn observation.
                let _guard = self.state.usage_edit.lock().await;
                self.record_re_estimate(run).await;
            }
        }
    }

    async fn capture_usage_snapshot(
        &self,
        run: &Run,
        point: SnapshotPoint,
        at: DateTime<Utc>,
        session: &factory_core::task::SessionRef,
    ) {
        let (usage, unknown) = match self.wiring.registry().runtime(&session.runtime) {
            Err(e) => (None, Some(e.to_string())),
            Ok(runtime) => match runtime.usage(session).await {
                Ok(Some(u)) => (Some(u), None),
                Ok(None) => (
                    None,
                    Some(format!("the {} runtime has no source for usage", runtime.name())),
                ),
                Err(e) => (None, Some(e.to_string())),
            },
        };
        let snapshot = UsageSnapshot {
            run_id: run.id.clone(),
            task_id: run.task_id.clone(),
            point,
            at,
            runtime: session.runtime.clone(),
            usage,
            unknown,
        };
        // Held across append, read and write-back, never across the runtime
        // call above: two snapshots of one run finishing together must not
        // leave the older sum written last.
        let _guard = self.state.usage_edit.lock().await;
        if let Err(e) = self.state.store.append_usage(&snapshot).await {
            tracing::warn!(run = %run.id, point = point.as_str(), error = %e, "could not keep a usage snapshot");
            return;
        }
        let mut recent = match self
            .state.store
            .runs_between(at - Duration::days(8), at + Duration::seconds(1))
            .await
        {
            Ok(runs) => runs,
            Err(e) => {
                tracing::warn!(run = %run.id, error = %e, "could not read runs for usage attribution");
                return;
            }
        };
        if let Ok(active) = self.state.store.active_runs().await {
            for active_run in active {
                if !recent.iter().any(|candidate| candidate.id == active_run.id) {
                    recent.push(active_run);
                }
            }
        }
        let mut snapshots_by_run = BTreeMap::new();
        for recent_run in &recent {
            if let Ok(snapshots) = self.state.store.usage_snapshots(&recent_run.id).await {
                snapshots_by_run.insert(recent_run.id.clone(), snapshots);
            }
        }
        let allocation = allocate_plan_share(&recent, &snapshots_by_run);
        for recent_run in &recent {
            let Some(snapshots) = snapshots_by_run.get(&recent_run.id) else { continue };
            // `#178`: a run that resumed another's session gets that
            // session's baseline backfilled from the previous run's own
            // `RunEnd` reading, when this run's `Dispatch` snapshot never
            // saw it -- see `run_usage_with_prior`.
            let prior = match (&recent_run.resumed_session, &recent_run.continued_from) {
                (Some(session_id), Some(previous_run_id)) => self
                    .prior_run_end_usage(session_id, previous_run_id)
                    .await
                    .map(|usage| (session_id.clone(), usage)),
                _ => None,
            };
            let mut usage = run_usage_with_prior(snapshots, prior.as_ref().map(|(id, u)| (id.as_str(), u)));
            usage.plan_share = allocation.shares.get(&recent_run.id).cloned().unwrap_or_default();
            usage.plan_share_unknowns = allocation.unknown.get(&recent_run.id).cloned().unwrap_or_default();
            if !usage.plan_share_unknowns.is_empty() {
                let reasons: BTreeSet<&str> = usage.plan_share_unknowns.iter().map(|gap| gap.reason.as_str()).collect();
                usage.plan_share_unknown = Some(if usage.plan_share_unknowns.len() == 1 {
                    reasons.into_iter().next().unwrap_or("provider-window share could not be attributed").into()
                } else {
                    format!("{} provider-window gaps: {}", usage.plan_share_unknowns.len(), reasons.into_iter().collect::<Vec<_>>().join("; "))
                });
            }
            let patch = RunPatch { usage: Some(usage), ..Default::default() };
            match self.state.store.update_run(&recent_run.id, &patch).await {
                Ok(updated) => self.wiring.bus().publish(Event::RunUpdated { run: updated.redacted() }),
                Err(e) => tracing::warn!(run = %recent_run.id, error = %e, "could not record a run's usage"),
            }
        }
    }

    /// `#178`: the previous run's newest `RunEnd` reading for `session_id`,
    /// looked up by hand rather than out of `recent` above -- a resumed
    /// session's previous run ended long enough ago that it can easily sit
    /// outside `capture_usage_snapshot`'s own 8-day attribution window.
    async fn prior_run_end_usage(&self, session_id: &str, previous_run_id: &str) -> Option<HarnessUsage> {
        let snapshots = self.state.store.usage_snapshots(previous_run_id).await.ok()?;
        let mut run_ends: Vec<&UsageSnapshot> =
            snapshots.iter().filter(|s| s.point == SnapshotPoint::RunEnd).collect();
        run_ends.sort_by_key(|s| s.at);
        run_ends.iter().rev().find_map(|s| {
            s.usage.as_ref()?.sessions.iter().find(|h| h.session_id == session_id).cloned()
        })
    }

    async fn record_re_estimate(&self, run: &Run) {
        let Ok(Some(current)) = self.state.store.get_run(&run.id).await else { return };
        if current.re_estimate.is_some() {
            return;
        }
        let Ok(current_snapshots) = self.state.store.usage_snapshots(&run.id).await else { return };
        let Some(first_at) = current_snapshots
            .iter()
            .filter(|snapshot| snapshot.point == SnapshotPoint::TurnEnded)
            .map(|snapshot| snapshot.at)
            .min()
        else { return };
        let Ok(Some(task)) = self.state.store.get(&run.task_id).await else { return };
        let snapshot = self.wiring.snapshot();
        let scope = snapshot.canonical_scope_name(&task.scope);
        let category = factory_core::control_plan::effective_category(task.category.as_deref()).to_string();
        let Ok(tasks) = self.state.store.list(&factory_core::task::TaskFilter::default()).await else { return };
        let mut wall_ratios = Vec::new();
        let mut active_ratios = Vec::new();
        let mut cost_ratios = Vec::new();
        let mut sample_count = 0u32;
        for peer in tasks.iter().filter(|peer| {
            snapshot.canonical_scope_name(&peer.scope) == scope
                && factory_core::control_plan::effective_category(peer.category.as_deref()) == category
        }) {
            let Ok(runs) = self.state.store.runs(&peer.id, u32::MAX).await else { continue };
            for completed in runs
                .into_iter()
                .filter(|candidate| {
                    candidate.id != current.id
                        && candidate.status.is_terminal()
                        && candidate.agent == current.agent
                })
            {
                let Ok(snapshots) = self.state.store.usage_snapshots(&completed.id).await else { continue };
                let Some(first_at) = snapshots
                    .iter()
                    .filter(|s| s.point == SnapshotPoint::TurnEnded)
                    .map(|s| s.at)
                    .min()
                else { continue };
                let first_usage = run_usage(
                    &snapshots.iter().filter(|s| s.at <= first_at).cloned().collect::<Vec<_>>(),
                );
                let final_usage = run_usage(&snapshots);
                let first_wall = (first_at - completed.started_at).num_milliseconds() as f64 / 1000.0;
                let final_wall = (completed.ended_at.unwrap_or(first_at) - completed.started_at)
                    .num_milliseconds() as f64
                    / 1000.0;
                if first_wall > 0.0 && final_wall >= first_wall {
                    wall_ratios.push(final_wall / first_wall);
                }
                // Wall time comes from the completed run itself. Active time
                // and cost come from the runtime, so only a successful,
                // complete run-end reading is final enough for cohort ratios.
                if final_usage.as_of_point == Some(SnapshotPoint::RunEnd) && !final_usage.partial {
                    if let (Some(first), Some(final_value)) = (first_usage.active_seconds, final_usage.active_seconds) {
                        if first > 0.0 && final_value >= first {
                            active_ratios.push(final_value / first);
                        }
                    }
                    if let (Some(first), Some(final_value)) = (first_usage.cost_usd, final_usage.cost_usd) {
                        if first > 0.0 && final_value >= first {
                            cost_ratios.push(final_value / first);
                        }
                    }
                }
                sample_count += 1;
            }
        }
        let current_usage = run_usage(
            &current_snapshots
                .iter()
                .filter(|snapshot| snapshot.at <= first_at)
                .cloned()
                .collect::<Vec<_>>(),
        );
        let observed_wall = ((first_at - current.started_at).num_milliseconds() >= 0)
            .then(|| (first_at - current.started_at).num_milliseconds() as f64 / 1000.0);
        let time = estimate_time(observed_wall, &wall_ratios);
        let active_seconds = estimate_time(current_usage.active_seconds, &active_ratios);
        let cost = estimate_cost(current_usage.cost_usd, &cost_ratios);
        let reason = (time.is_none() && active_seconds.is_none() && cost.is_none())
            .then(|| "no completed runs with a measurable first-turn-to-final ratio".into());
        let estimate = ReEstimate {
            time,
            active_seconds,
            cost,
            sample_count,
            scope,
            agent: current.agent,
            category,
            observed_wall_seconds: observed_wall,
            observed_active_seconds: current_usage.active_seconds,
            observed_cost_usd: current_usage.cost_usd,
            reason,
        };
        let Ok(updated) = self
            .state.store
            .update_run(&current.id, &RunPatch { re_estimate: Some(estimate.clone()), ..Default::default() })
            .await
        else { return };
        self.wiring.bus().publish(Event::RunUpdated { run: updated.redacted() });
        self.entry(
            &current.task_id,
            factory_core::task::TaskEntry::new("daemon", "re_estimate", "recorded the first-turn re-estimate")
                .in_run(&current.id)
                .with_data(serde_json::to_value(&estimate).unwrap_or_default()),
        )
        .await;
    }

    /// `Request::TaskUsage`: every run's usage and their sum.
    pub(crate) async fn task_usage(&self, task_id: &str) -> Result<TaskUsage> {
        let task = self.require(task_id).await?;
        let runs = self.state.store.runs(&task.id, u32::MAX).await?;
        let now = Utc::now();
        let mut total = CostRow::new(task.id.clone(), Some(task.title.clone()));
        let mut entries = Vec::with_capacity(runs.len());
        let mut actual_wall = 0u64;
        let mut actual_active = Some(0.0);
        let mut actual_cost = Some(0.0);
        // The task's estimate is the sum of what each run was estimated at
        // when it started, over the same runs whose actuals are summed --
        // never the newest run's estimate against every run's actuals.
        let mut estimate_sum = EstimateSum::default();
        let mut all_terminal = true;
        let mut all_cost_measurements_final = true;
        for run in runs {
            total.add(run.usage.as_ref());
            let wall = (run.ended_at.unwrap_or(now) - run.started_at).num_seconds().max(0) as u64;
            actual_wall += wall;
            let terminal = run.status.is_terminal();
            let final_usage = run.usage.as_ref().filter(|usage| {
                terminal && usage.as_of_point == Some(SnapshotPoint::RunEnd) && !usage.partial
            });
            all_terminal &= terminal;
            all_cost_measurements_final &= final_usage.is_some();
            estimate_sum.add(run.original_estimate.as_ref());
            actual_active = match (actual_active, run.usage.as_ref().and_then(|usage| usage.active_seconds)) {
                (Some(sum), Some(value)) => Some(sum + value),
                _ => None,
            };
            actual_cost = match (actual_cost, run.usage.as_ref().and_then(|usage| usage.cost_usd)) {
                (Some(sum), Some(value)) => Some(sum + value),
                _ => None,
            };
            let time_comparison = run.original_estimate.as_ref().map(|estimate| {
                EstimateComparison::<u64>::new(
                    estimate.time.low,
                    estimate.time.expected,
                    estimate.time.high,
                    terminal.then_some(wall),
                )
            });
            let cost_comparison = run.original_estimate.as_ref().and_then(|estimate| estimate.cost.as_ref()).map(|estimate| {
                EstimateComparison::<f64>::new(
                    estimate.low,
                    estimate.expected,
                    estimate.high,
                    final_usage.and_then(|usage| usage.cost_usd),
                )
            });
            entries.push(RunUsageEntry {
                wall_seconds: wall as i64,
                original_estimate: run.original_estimate,
                re_estimate: run.re_estimate,
                time_comparison,
                cost_comparison,
                usage: run
                    .usage
                    .clone()
                    .unwrap_or_else(|| RunUsage::unknown("no usage was recorded for this run", 0)),
                run_id: run.id,
                attempt: run.attempt,
                status: run.status,
            });
        }
        let original_estimate = estimate_sum.total();
        Ok(TaskUsage {
            task_id: task.id,
            total,
            time_comparison: original_estimate.as_ref().map(|estimate| {
                EstimateComparison::<u64>::new(
                    estimate.time.low,
                    estimate.time.expected,
                    estimate.time.high,
                    all_terminal.then_some(actual_wall),
                )
            }),
            cost_comparison: original_estimate.as_ref().and_then(|estimate| estimate.cost.as_ref()).map(|estimate| {
                EstimateComparison::<f64>::new(
                    estimate.low,
                    estimate.expected,
                    estimate.high,
                    all_cost_measurements_final.then_some(actual_cost).flatten(),
                )
            }),
            original_estimate,
            actual_wall_seconds: Some(actual_wall),
            actual_active_seconds: actual_active,
            actual_cost_usd: actual_cost,
            runs: entries,
        })
    }

}


/// The runs' snapshotted estimates, summed range by range. One run with no
/// estimate makes the time sum unknown, and one with no cost range the cost
/// sum: a total that leaves a run out would be compared with actuals that
/// include it.
#[derive(Default)]
struct EstimateSum {
    runs: u32,
    time: Option<factory_core::task::TimeEstimateRange>,
    cost: Option<factory_core::task::CostEstimateRange>,
    time_missing: bool,
    cost_missing: bool,
}

impl EstimateSum {
    fn add(&mut self, estimate: Option<&factory_core::task::Estimate>) {
        self.runs += 1;
        let Some(estimate) = estimate else {
            self.time_missing = true;
            self.cost_missing = true;
            return;
        };
        let time = self.time.get_or_insert(factory_core::task::TimeEstimateRange { low: 0, expected: 0, high: 0 });
        time.low += estimate.time.low;
        time.expected += estimate.time.expected;
        time.high += estimate.time.high;
        match &estimate.cost {
            Some(range) => {
                let cost = self.cost.get_or_insert(factory_core::task::CostEstimateRange { low: 0.0, expected: 0.0, high: 0.0 });
                cost.low += range.low;
                cost.expected += range.expected;
                cost.high += range.high;
            }
            None => self.cost_missing = true,
        }
    }

    fn total(self) -> Option<factory_core::task::Estimate> {
        if self.runs == 0 || self.time_missing {
            return None;
        }
        Some(factory_core::task::Estimate {
            time: self.time?,
            cost: if self.cost_missing { None } else { self.cost },
        })
    }
}

fn factors(values: &[f64]) -> Option<(f64, f64, f64)> {
    if values.is_empty() {
        return None;
    }
    let mut values = values.to_vec();
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some((values[0], values[values.len() / 2], values[values.len() - 1]))
}

fn estimate_time(observed: Option<f64>, ratios: &[f64]) -> Option<factory_core::task::TimeEstimateRange> {
    let observed = observed?;
    let (low, expected, high) = factors(ratios)?;
    Some(factory_core::task::TimeEstimateRange {
        low: (observed * low).round().max(1.0) as u64,
        expected: (observed * expected).round().max(1.0) as u64,
        high: (observed * high).round().max(1.0) as u64,
    })
}

fn estimate_cost(observed: Option<f64>, ratios: &[f64]) -> Option<factory_core::task::CostEstimateRange> {
    let observed = observed?;
    let (low, expected, high) = factors(ratios)?;
    Some(factory_core::task::CostEstimateRange {
        low: observed * low,
        expected: observed * expected,
        high: observed * high,
    })
}

/// `Engine::snapshot_usage` needs `&self` only; the turn-ended path wants it
/// off the hook's own request, so it is spawned from an `Arc`.
pub(crate) fn spawn_snapshot(engine: &Arc<Engine>, run: Run, point: SnapshotPoint) {
    let engine = engine.clone();
    tokio::spawn(async move { engine.l4_service().snapshot_usage(&run, point).await });
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_core::adapter::{AgentRuntime, RuntimeStatus, StartRequest};
    use factory_core::config::{Config, DaemonConfig, Factory, Instance, Scope};
    use factory_core::error::FactoryError;
    use factory_core::run::{NewRun, RunStatus, Trigger};
    use factory_core::task::{NewTask, SessionRef, TaskPatch, TaskReport, WorkflowOrigin};
    use factory_core::usage::{HarnessUsage, SessionUsage, TokenCounts, UsageCost, UsageState};
    use factory_core::workflow::{WorkflowDefinition, WorkflowDraft};
    use factory_plugins::{Registry, SqliteStore};
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::Mutex;

    type Answer = std::result::Result<Option<SessionUsage>, String>;

    /// A runtime that answers `usage` from a script, one answer per call,
    /// and `None` once the script runs out.
    struct MeteredRuntime {
        answers: Mutex<VecDeque<Answer>>,
        delays: Mutex<VecDeque<std::time::Duration>>,
    }

    #[async_trait::async_trait]
    impl AgentRuntime for MeteredRuntime {
        fn name(&self) -> &str {
            "metered"
        }
        async fn start(&self, req: &StartRequest) -> Result<SessionRef> {
            Ok(SessionRef {
                runtime: "metered".into(),
                handle: req.id.clone(),
                meta: Default::default(),
            })
        }
        async fn submit(&self, _: &SessionRef, _: &str) -> Result<()> {
            Ok(())
        }
        async fn status(&self, _: &SessionRef) -> Result<RuntimeStatus> {
            Ok(RuntimeStatus::Working)
        }
        async fn send_text(&self, _: &SessionRef, _: &str) -> Result<()> {
            Ok(())
        }
        async fn send_keys(&self, _: &SessionRef, _: &[String]) -> Result<()> {
            Ok(())
        }
        async fn read(&self, _: &SessionRef, _: u32) -> Result<String> {
            Ok(String::new())
        }
        async fn stop(&self, _: &SessionRef) -> Result<()> {
            Ok(())
        }
        async fn usage(&self, _: &SessionRef) -> Result<Option<SessionUsage>> {
            let answer = self.answers.lock().unwrap().pop_front();
            let delay = self.delays.lock().unwrap().pop_front().unwrap_or_default();
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            match answer {
                Some(Ok(u)) => Ok(u),
                Some(Err(e)) => Err(FactoryError::adapter("metered", e)),
                None => Ok(None),
            }
        }
    }

    fn usage(input: u64, usd: f64) -> SessionUsage {
        SessionUsage {
            schema: 1,
            handle: None,
            sampled_at: None,
            sessions: vec![HarnessUsage {
                session_id: "s1".into(),
                adapter: Some("claude-code".into()),
                model: Some("claude-opus-5".into()),
                tokens: TokenCounts {
                    input: Some(input),
                    output: Some(input / 10),
                    cache_read: Some(0),
                    cache_write: Some(0),
                },
                cost: UsageCost {
                    usd: Some(usd),
                    pricing_source: Some("litellm@test".into()),
                },
                elapsed_seconds: None,
                active_seconds: None,
                subagents: Vec::new(),
                rate_limit: None,
                unavailable: Default::default(),
            }],
        }
    }

    fn usage_with_active(input: u64, usd: f64, active_seconds: f64) -> SessionUsage {
        let mut measured = usage(input, usd);
        measured.sessions[0].active_seconds = Some(active_seconds);
        measured
    }

    /// A run's own already-derived `usage` (`Run.usage`), for the tests that
    /// write it onto a run directly rather than driving it through a
    /// `MeteredRuntime` snapshot.
    fn known_run_usage(input: u64, usd: f64) -> RunUsage {
        RunUsage {
            state: UsageState::Known,
            reason: None,
            tokens: TokenCounts { input: Some(input), output: Some(0), cache_read: Some(0), cache_write: Some(0) },
            cost_usd: Some(usd),
            ..RunUsage::unknown("", 2)
        }
    }

    fn engine(answers: Vec<Answer>) -> Arc<Engine> {
        engine_with_delays(answers, Vec::new())
    }

    fn engine_with_delays(
        answers: Vec<Answer>,
        delays: Vec<std::time::Duration>,
    ) -> Arc<Engine> {
        let root = std::env::temp_dir().join(format!("factory-costs-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let config = Config {
            version: 1,
            instance: Instance {
                id: "test".into(),
                name: "test".into(),
            },
            // Off: these tests dispatch real runs, and the default would fork
            // a real `caffeinate` on whatever machine runs them.
            daemon: DaemonConfig {
                power_assertion: false,
                ..DaemonConfig::default()
            },
            roles: Default::default(),
            dashboard: None,
            policies: Default::default(),
            quality: Default::default(),
            scope: None,
            scopes: vec![Scope {
                id: "demo-id".into(),
                name: "demo".into(),
                path: PathBuf::new(),
                agent: None,
                agents: Vec::new(),
                runtime: Some("metered".into()),
                git: None,
                task_store: None,
                max_sessions: None,
                roles: Default::default(),
                dashboard: None,
                policies: Default::default(),
                quality: Default::default(),
                intake: Default::default(),
                dependencies: Default::default(),
                environments: Vec::new(),
                renewals: Vec::new(),
                metrics: None,
                backup: None,
            }],
            infrastructure: Default::default(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let mut registry = Registry::with_builtins();
        registry.add_runtime(
            Arc::new(MeteredRuntime {
                answers: Mutex::new(answers.into()),
                delays: Mutex::new(delays.into()),
            }),
            "test",
        );
        Arc::new(Engine::new(
            Factory { root, config },
            registry,
            Arc::new(SqliteStore::in_memory().unwrap()),
            PathBuf::from("factory"),
            Vec::new(),
        ))
    }

    async fn dispatched(engine: &Arc<Engine>, issue: Option<&str>) -> (Task, Run) {
        let task = engine
            .l4_service()
            .create(NewTask {
                title: "costly".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                estimate_seconds: Some(900),
                worktree: Some(false),
                labels: issue.map(|n| [("issue".to_string(), n.to_string())].into()).unwrap_or_default(),
                ..Default::default()
            })
            .await
            .unwrap();
        engine.l4_service().start_run(&task.id, Trigger::Manual).await;
        let run = engine.l4.store.active_run(&task.id).await.unwrap().expect("dispatched");
        (task, run)
    }

    /// The L4 spend fact's query, spelled the way `costs_report`'s old
    /// positional call used to read.
    fn spend_query(
        group_by: CostGroupBy,
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
        scope: Option<&str>,
    ) -> SpendQuery {
        SpendQuery { scope: scope.map(str::to_string), from, to, group_by, ..Default::default() }
    }

    async fn done(engine: &Arc<Engine>, task: &Task, run: &Run) -> Run {
        engine
            .l4_service()
            .report(
                &task.id,
                TaskReport {
                    artifacts: Vec::new(),
                    status: Some(RunStatus::Done),
                    message: None,
                    result: Some("ok".into()),
                    send_to: None,
                    error: None,
                    token: run.token.clone(),
                },
            )
            .await
            .unwrap();
        engine.l4_service().require_run(&run.id).await.unwrap()
    }

    #[tokio::test]
    async fn a_run_is_read_at_dispatch_and_at_its_end_and_carries_the_difference() {
        let engine = engine(vec![Ok(Some(usage(1_000, 0.10))), Ok(Some(usage(5_000, 0.50)))]);
        let (task, run) = dispatched(&engine, Some("117")).await;
        let baseline = engine.l4.store.usage_snapshots(&run.id).await.unwrap();
        assert_eq!(baseline.len(), 1);
        assert_eq!(baseline[0].point, SnapshotPoint::Dispatch);
        assert_eq!(run.original_estimate.as_ref().unwrap().time.expected, 900);

        engine
            .update(
                &task.id,
                factory_core::task::TaskPatch { estimate_seconds: Some(1800), ..Default::default() },
                None,
            )
            .await
            .unwrap();
        assert_eq!(engine.l4_service().require_run(&run.id).await.unwrap().original_estimate.unwrap().time.expected, 900);

        let ended = done(&engine, &task, &run).await;
        let u = ended.usage.expect("the run carries its usage");
        assert_eq!(u.state, UsageState::Known, "{u:?}");
        assert_eq!(u.tokens.input, Some(4_000));
        assert_eq!(u.tokens.output, Some(400));
        assert!((u.cost_usd.unwrap() - 0.40).abs() < 1e-9);
        assert_eq!(u.as_of_point, Some(SnapshotPoint::RunEnd));
        assert_eq!(u.pricing_sources, vec!["litellm@test".to_string()]);

        let tu = engine.l4_service().task_usage(&task.id).await.unwrap();
        assert_eq!(tu.total.runs, 1);
        assert!((tu.total.cost_usd - 0.40).abs() < 1e-9);
        assert_eq!(tu.runs[0].attempt, 1);
    }

    #[tokio::test]
    async fn a_turn_end_takes_a_reading_off_the_hooks_own_path() {
        let engine = engine(vec![Ok(Some(usage(0, 0.0))), Ok(Some(usage(2_000, 0.20)))]);
        let (task, run) = dispatched(&engine, None).await;
        engine
            .turn_ended(
                &task.id,
                factory_core::task::TurnEnded {
                    event: factory_core::task::TurnEndEvent::Stop,
                    pending_background: 0,
                    error: None,
                    error_details: None,
                    last_message: None,
                    token: run.token.clone(),
                    session_id: None,
                },
            )
            .await
            .unwrap();
        // Written back after the snapshot is stored, so wait on the run.
        let mut u = None;
        for _ in 0..100 {
            u = engine.l4_service().require_run(&run.id).await.unwrap().usage;
            if u.as_ref().and_then(|u| u.as_of_point) == Some(SnapshotPoint::TurnEnded) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let u = u.unwrap();
        assert_eq!(u.as_of_point, Some(SnapshotPoint::TurnEnded), "{u:?}");
        assert_eq!(engine.l4.store.usage_snapshots(&run.id).await.unwrap().len(), 2);
        assert_eq!(u.tokens.input, Some(2_000));
        let updated = engine.l4_service().require_run(&run.id).await.unwrap();
        assert!(updated.re_estimate.as_ref().unwrap().reason.as_deref().unwrap().contains("no completed runs"));
        let entries = engine.l4.store.entries(&task.id, 100).await.unwrap();
        assert_eq!(entries.iter().filter(|entry| entry.kind == "re_estimate").count(), 1);
    }

    #[tokio::test]
    async fn a_first_turn_re_estimate_uses_matching_completed_runs_and_is_never_rewritten() {
        let engine = engine(vec![
            Ok(Some(usage(0, 0.0))),
            Ok(Some(usage(1_000, 0.10))),
            Ok(Some(usage(2_000, 0.20))),
            Ok(Some(usage(0, 0.0))),
            Ok(Some(usage(500, 0.05))),
            Ok(Some(usage(1_500, 0.15))),
        ]);
        let (reference_task, reference_run) = dispatched(&engine, None).await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        engine
            .turn_ended(
                &reference_task.id,
                factory_core::task::TurnEnded {
                    event: factory_core::task::TurnEndEvent::Stop,
                    pending_background: 0,
                    error: None,
                    error_details: None,
                    last_message: None,
                    token: reference_run.token.clone(),
                    session_id: None,
                },
            )
            .await
            .unwrap();
        for _ in 0..100 {
            if engine.l4_service().require_run(&reference_run.id).await.unwrap().re_estimate.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        done(&engine, &reference_task, &reference_run).await;

        let (task, run) = dispatched(&engine, None).await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        engine
            .turn_ended(
                &task.id,
                factory_core::task::TurnEnded {
                    event: factory_core::task::TurnEndEvent::Stop,
                    pending_background: 0,
                    error: None,
                    error_details: None,
                    last_message: None,
                    token: run.token.clone(),
                    session_id: None,
                },
            )
            .await
            .unwrap();
        let mut first = None;
        for _ in 0..100 {
            first = engine.l4_service().require_run(&run.id).await.unwrap().re_estimate;
            if first.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let first = first.expect("the first turn records a re-estimate");
        assert_eq!(first.sample_count, 1);
        assert!(first.time.is_some());
        assert!((first.cost.as_ref().unwrap().expected - 0.10).abs() < 1e-9, "{first:?}");

        done(&engine, &task, &run).await;
        assert_eq!(engine.l4_service().require_run(&run.id).await.unwrap().re_estimate, Some(first));
        let entries = engine.l4.store.entries(&task.id, 100).await.unwrap();
        assert_eq!(entries.iter().filter(|entry| entry.kind == "re_estimate").count(), 1);
    }

    #[tokio::test]
    async fn out_of_order_snapshot_completion_still_uses_the_first_turn_once() {
        let engine = engine_with_delays(
            vec![
                Ok(Some(usage_with_active(0, 0.0, 0.0))),
                Ok(Some(usage_with_active(1_000, 0.10, 10.0))),
                Ok(Some(usage_with_active(2_000, 0.20, 20.0))),
                Ok(Some(usage_with_active(3_000, 0.30, 30.0))),
            ],
            vec![
                std::time::Duration::ZERO,
                std::time::Duration::from_millis(100),
                std::time::Duration::ZERO,
                std::time::Duration::ZERO,
            ],
        );
        let (task, run) = dispatched(&engine, None).await;

        let first_engine = engine.clone();
        let first_run = run.clone();
        let first = tokio::spawn(async move {
            first_engine.l4_service().snapshot_usage(&first_run, SnapshotPoint::TurnEnded).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let later_engine = engine.clone();
        let later_run = run.clone();
        let later = tokio::spawn(async move {
            later_engine.l4_service().snapshot_usage(&later_run, SnapshotPoint::TurnEnded).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let ended = done(&engine, &task, &run).await;
        first.await.unwrap();
        later.await.unwrap();

        assert!((ended.usage.as_ref().unwrap().cost_usd.unwrap() - 0.30).abs() < 1e-9);
        let updated = engine.l4_service().require_run(&run.id).await.unwrap();
        let estimate = updated.re_estimate.expect("the first turn records a re-estimate");
        assert!((estimate.observed_cost_usd.unwrap() - 0.10).abs() < 1e-9, "{estimate:?}");
        assert!((estimate.observed_active_seconds.unwrap() - 10.0).abs() < 1e-9, "{estimate:?}");
        let entries = engine.l4.store.entries(&task.id, 100).await.unwrap();
        assert_eq!(entries.iter().filter(|entry| entry.kind == "re_estimate").count(), 1);
    }

    #[tokio::test]
    async fn a_failed_run_end_snapshot_is_not_a_final_active_or_cost_cohort_measurement() {
        let engine = engine(vec![
            Ok(Some(usage_with_active(0, 0.0, 0.0))),
            Ok(Some(usage_with_active(1_000, 1.0, 10.0))),
            Err("run-end usage unavailable".into()),
            Ok(Some(usage_with_active(0, 0.0, 0.0))),
            Ok(Some(usage_with_active(500, 0.5, 5.0))),
        ]);
        let (reference_task, reference_run) = dispatched(&engine, None).await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        engine
            .turn_ended(
                &reference_task.id,
                factory_core::task::TurnEnded {
                    event: factory_core::task::TurnEndEvent::Stop,
                    pending_background: 0,
                    error: None,
                    error_details: None,
                    last_message: None,
                    token: reference_run.token.clone(),
                    session_id: None,
                },
            )
            .await
            .unwrap();
        for _ in 0..100 {
            if engine.l4_service().require_run(&reference_run.id).await.unwrap().re_estimate.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let ended = done(&engine, &reference_task, &reference_run).await;
        assert!(ended.usage.as_ref().unwrap().partial);
        assert_eq!(ended.usage.as_ref().unwrap().as_of_point, Some(SnapshotPoint::TurnEnded));

        let (task, run) = dispatched(&engine, None).await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        engine
            .turn_ended(
                &task.id,
                factory_core::task::TurnEnded {
                    event: factory_core::task::TurnEndEvent::Stop,
                    pending_background: 0,
                    error: None,
                    error_details: None,
                    last_message: None,
                    token: run.token.clone(),
                    session_id: None,
                },
            )
            .await
            .unwrap();
        let mut estimate = None;
        for _ in 0..100 {
            estimate = engine.l4_service().require_run(&run.id).await.unwrap().re_estimate;
            if estimate.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let estimate = estimate.expect("the first turn records a re-estimate");
        assert_eq!(estimate.sample_count, 1);
        assert!(estimate.time.is_some(), "completed wall time remains a valid cohort input");
        assert_eq!(estimate.active_seconds, None, "partial active time is not final");
        assert_eq!(estimate.cost, None, "partial cost is not final");
    }

    fn ranged_estimate() -> factory_core::task::Estimate {
        factory_core::task::Estimate {
            time: factory_core::task::TimeEstimateRange { low: 1, expected: 900, high: 1800 },
            cost: Some(factory_core::task::CostEstimateRange {
                low: 1.0,
                expected: 1.5,
                high: 2.0,
            }),
        }
    }

    #[tokio::test]
    async fn active_run_comparisons_do_not_claim_final_accuracy() {
        let engine = engine(vec![Ok(Some(usage(0, 0.0)))]);
        let (task, run) = dispatched(&engine, None).await;
        engine
            .l4.store
            .update_run(
                &run.id,
                &RunPatch { original_estimate: Some(ranged_estimate()), ..Default::default() },
            )
            .await
            .unwrap();

        let usage = engine.l4_service().task_usage(&task.id).await.unwrap();
        let entry = &usage.runs[0];
        assert!(!entry.status.is_terminal());
        assert_eq!(entry.time_comparison.as_ref().unwrap().actual, None);
        assert_eq!(entry.time_comparison.as_ref().unwrap().actual_over_expected, None);
        assert_eq!(entry.time_comparison.as_ref().unwrap().within_range, None);
        assert_eq!(usage.time_comparison.as_ref().unwrap().actual, None);
        assert_eq!(usage.time_comparison.as_ref().unwrap().within_range, None);
    }

    #[tokio::test]
    async fn partial_run_end_cost_is_not_a_final_accuracy_verdict() {
        let engine = engine(vec![
            Ok(Some(usage(0, 0.0))),
            Ok(Some(usage(1_500, 1.5))),
            Err("run-end usage unavailable".into()),
        ]);
        let (task, run) = dispatched(&engine, None).await;
        engine
            .l4.store
            .update_run(
                &run.id,
                &RunPatch { original_estimate: Some(ranged_estimate()), ..Default::default() },
            )
            .await
            .unwrap();
        engine.l4_service().snapshot_usage(&run, SnapshotPoint::TurnEnded).await;
        let ended = done(&engine, &task, &run).await;
        assert!(ended.usage.as_ref().unwrap().partial);
        assert!((ended.usage.as_ref().unwrap().cost_usd.unwrap() - 1.5).abs() < 1e-9);

        let usage = engine.l4_service().task_usage(&task.id).await.unwrap();
        let comparison = usage.runs[0].cost_comparison.as_ref().unwrap();
        assert_eq!(comparison.actual, None);
        assert_eq!(comparison.actual_over_expected, None);
        assert_eq!(comparison.within_range, None);
        assert_eq!(usage.cost_comparison.as_ref().unwrap().actual, None);
        assert_eq!(usage.cost_comparison.as_ref().unwrap().within_range, None);
    }

    #[tokio::test]
    async fn a_task_compares_every_runs_actuals_with_every_runs_own_estimate() {
        let engine = engine(vec![
            Ok(Some(usage(0, 0.0))),
            Ok(Some(usage(1_000, 0.10))),
            Ok(Some(usage(0, 0.0))),
            Ok(Some(usage(1_000, 0.10))),
        ]);
        let task = engine
            .l4_service()
            .create(NewTask {
                title: "recurring".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                estimate_seconds: Some(600),
                worktree: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        // Each attempt takes exactly ten minutes of wall time.
        let ten_minutes = |run: &Run| RunPatch {
            ended_at: Some(run.started_at + Duration::seconds(600)),
            ..Default::default()
        };

        engine.l4_service().start_run(&task.id, Trigger::Manual).await;
        let first = engine.l4.store.active_run(&task.id).await.unwrap().expect("dispatched");
        let first = done(&engine, &task, &first).await;
        engine.l4.store.update_run(&first.id, &ten_minutes(&first)).await.unwrap();

        // The estimate is edited between attempts: the second run is held
        // to the new range, the first keeps the one it started with.
        engine
            .update(
                &task.id,
                factory_core::task::TaskPatch {
                    estimate: Some(factory_core::task::Estimate {
                        time: factory_core::task::TimeEstimateRange { low: 300, expected: 600, high: 1200 },
                        cost: Some(factory_core::task::CostEstimateRange { low: 0.05, expected: 0.1, high: 0.2 }),
                    }),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        engine.l4_service().start_run(&task.id, Trigger::Manual).await;
        let second = engine.l4.store.active_run(&task.id).await.unwrap().expect("dispatched again");
        let second = done(&engine, &task, &second).await;
        engine.l4.store.update_run(&second.id, &ten_minutes(&second)).await.unwrap();

        let usage = engine.l4_service().task_usage(&task.id).await.unwrap();
        assert_eq!(usage.runs.len(), 2);
        for entry in &usage.runs {
            let comparison = entry.time_comparison.as_ref().unwrap();
            assert_eq!(comparison.actual, Some(600));
            assert_eq!(comparison.actual_over_expected, Some(1.0), "each run met its own estimate");
            assert_eq!(comparison.within_range, Some(true));
        }
        let time = usage.time_comparison.as_ref().expect("both runs carry an estimate");
        assert_eq!((time.low, time.expected, time.high), (600 + 300, 600 + 600, 600 + 1200));
        assert_eq!(time.actual, Some(1200));
        assert_eq!(time.actual_over_expected, Some(1.0), "two on-estimate runs are on estimate together");
        assert_eq!(time.within_range, Some(true));
        let summed = usage.original_estimate.as_ref().unwrap();
        assert_eq!(summed.time.expected, time.expected);
        // The first run had no cost range, so there is no task-wide one.
        assert_eq!(summed.cost, None);
        assert_eq!(usage.cost_comparison, None);
    }

    #[test]
    fn a_run_with_no_estimate_leaves_the_task_total_out() {
        let ranged = factory_core::task::Estimate {
            time: factory_core::task::TimeEstimateRange { low: 10, expected: 20, high: 40 },
            cost: Some(factory_core::task::CostEstimateRange { low: 1.0, expected: 2.0, high: 3.0 }),
        };
        let mut sum = EstimateSum::default();
        sum.add(Some(&ranged));
        sum.add(Some(&ranged));
        let total = sum.total().unwrap();
        assert_eq!((total.time.low, total.time.expected, total.time.high), (20, 40, 80));
        assert_eq!(total.cost.map(|c| (c.low, c.expected, c.high)), Some((2.0, 4.0, 6.0)));

        // A run from before estimates were snapshotted carries none: a total
        // that left it out would be compared with actuals that include it.
        let mut sum = EstimateSum::default();
        sum.add(Some(&ranged));
        sum.add(None);
        assert_eq!(sum.total(), None);
        assert_eq!(EstimateSum::default().total(), None, "no runs, no estimate");
    }

    #[tokio::test]
    async fn a_runtime_with_no_usage_leaves_the_run_unknown_with_the_reason() {
        let engine = engine(vec![]);
        let (task, run) = dispatched(&engine, None).await;
        let ended = done(&engine, &task, &run).await;
        assert_eq!(ended.status, RunStatus::Done, "no usage never fails a run");
        let u = ended.usage.unwrap();
        assert_eq!(u.state, UsageState::Unknown);
        assert!(u.reason.unwrap().contains("metered runtime has no source for usage"));
        assert_eq!(u.snapshots, 2);
    }

    #[tokio::test]
    async fn a_failed_baseline_is_kept_and_named() {
        let engine = engine(vec![Err("plugin exploded".into()), Ok(Some(usage(10, 0.01)))]);
        let (task, run) = dispatched(&engine, None).await;
        let ended = done(&engine, &task, &run).await;
        let u = ended.usage.unwrap();
        assert_eq!(u.state, UsageState::Unknown);
        assert!(u.reason.unwrap().contains("plugin exploded"));
    }

    #[tokio::test]
    async fn costs_group_by_issue_scope_agent_and_task_and_count_the_unknown() {
        let engine = engine(vec![
            Ok(Some(usage(0, 0.0))),
            Ok(Some(usage(1_000, 1.25))),
            // The second run's runtime has nothing to say at all.
        ]);
        let (t1, r1) = dispatched(&engine, Some("117")).await;
        done(&engine, &t1, &r1).await;
        let (t2, r2) = dispatched(&engine, None).await;
        done(&engine, &t2, &r2).await;

        let by_issue = crate::facts::Facts::<factory_kernel::L6>::new(&engine).get::<CostReport>(&spend_query(CostGroupBy::Issue, None, None, None)).await.unwrap();
        assert_eq!(by_issue.rows.len(), 2);
        assert_eq!(by_issue.rows[0].key, "issue=117", "most expensive first");
        assert!((by_issue.rows[0].cost_usd - 1.25).abs() < 1e-9);
        assert_eq!(by_issue.rows[1].key, NO_ISSUE);
        assert_eq!(by_issue.rows[1].runs_unknown, 1);
        assert_eq!(by_issue.total.runs, 2);
        assert_eq!(by_issue.total.runs_unknown, 1, "counted, never dropped");
        assert_eq!(by_issue.total.tokens.input, 1_000);

        let by_agent = crate::facts::Facts::<factory_kernel::L6>::new(&engine).get::<CostReport>(&spend_query(CostGroupBy::Agent, None, None, None)).await.unwrap();
        assert_eq!(by_agent.rows.len(), 1);
        assert_eq!(by_agent.rows[0].key, "demo/shell");
        assert_eq!(by_agent.rows[0].runs, 2);

        let by_scope = crate::facts::Facts::<factory_kernel::L6>::new(&engine).get::<CostReport>(&spend_query(CostGroupBy::Scope, None, None, Some("demo"))).await.unwrap();
        assert_eq!(by_scope.rows[0].key, "demo");
        assert_eq!(by_scope.scope.as_deref(), Some("demo"));

        let by_task = crate::facts::Facts::<factory_kernel::L6>::new(&engine).get::<CostReport>(&spend_query(CostGroupBy::Task, None, None, None)).await.unwrap();
        assert_eq!(by_task.rows[0].key, t1.id);
        assert_eq!(by_task.rows[0].label.as_deref(), Some("costly"));

        engine
            .l4.store
            .update_run(&r1.id, &RunPatch { provider_account: Some("claude-max".into()), ..Default::default() })
            .await
            .unwrap();
        let by_provider = crate::facts::Facts::<factory_kernel::L6>::new(&engine).get::<CostReport>(&spend_query(CostGroupBy::Provider, None, None, None)).await.unwrap();
        assert!(by_provider.rows.iter().any(|row| row.key == "claude-max"));

        // A window before any of it holds nothing; an empty one is refused.
        let past = Utc::now() - Duration::days(400);
        let none = crate::facts::Facts::<factory_kernel::L6>::new(&engine)
            .get::<CostReport>(&spend_query(CostGroupBy::Task, Some(past), Some(past + Duration::days(1)), None))
            .await
            .unwrap();
        assert!(none.rows.is_empty());
        assert!(crate::facts::Facts::<factory_kernel::L6>::new(&engine).get::<CostReport>(&spend_query(CostGroupBy::Task, Some(past), Some(past), None)).await.is_err());
        assert!(crate::facts::Facts::<factory_kernel::L6>::new(&engine).get::<CostReport>(&spend_query(CostGroupBy::Task, None, None, Some("nope"))).await.is_err());
    }

    #[tokio::test]
    async fn costs_group_by_workflow_labels_the_live_one_and_counts_the_rest() {
        let engine = engine(vec![]);

        // A standalone task, dispatched the ordinary way -- no runtime
        // answer is queued for it, so it lands unknown and is counted, not
        // dropped. Done first: `capture_usage_snapshot` recomputes usage
        // for every run in its own 8-day attribution window, which would
        // otherwise clobber the two hand-set rows built below.
        let (standalone_task, standalone_run) = dispatched(&engine, None).await;
        done(&engine, &standalone_task, &standalone_run).await;

        // A task whose workflow definition is still around: keyed by its
        // id, labelled with its name. Built straight against the store,
        // past the runtime entirely, so no further snapshot capture touches
        // it.
        let definition = WorkflowDefinition::from_draft(WorkflowDraft {
            name: "release train".into(),
            scope: "demo".into(),
            ..Default::default()
        });
        engine.l4.workflows.put_definition(&definition).await.unwrap();
        let origin = WorkflowOrigin {
            workflow_id: definition.id.clone(),
            workflow_run_id: "wfrun-1".into(),
            node_id: "n1".into(),
            workspace: None,
        };
        let wf_task = engine
            .l4_service()
            .create_workflow_task(
                NewTask {
                    title: "wf task".into(),
                    instructions: "true".into(),
                    scope: Some("demo".into()),
                    agent: Some("shell".into()),
                    worktree: Some(false),
                    ..Default::default()
                },
                origin,
                "wf-task-1".into(),
            )
            .await
            .unwrap();
        let wf_run = engine
            .l4.store
            .create_run(&NewRun {
                task_id: wf_task.id.clone(),
                trigger: Trigger::Manual,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: "metered".into(),
                token: "tok".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        engine
            .l4.store
            .update_run(
                &wf_run.id,
                &RunPatch {
                    status: Some(RunStatus::Done),
                    ended_at: Some(Utc::now()),
                    usage: Some(known_run_usage(1_000, 2.0)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        // A run whose task was deleted: built straight against the store,
        // so no task row was ever created for it -- `store.get` on its
        // `task_id` answers `None`, the same as a task that existed and was
        // removed. Its workflow is unknown, never "standalone".
        let ghost_run = engine
            .l4.store
            .create_run(&NewRun {
                task_id: "ghost-task".into(),
                trigger: Trigger::Manual,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: "metered".into(),
                token: "tok".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        engine
            .l4.store
            .update_run(
                &ghost_run.id,
                &RunPatch {
                    status: Some(RunStatus::Done),
                    ended_at: Some(Utc::now()),
                    usage: Some(known_run_usage(500, 1.0)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let report = crate::facts::Facts::<factory_kernel::L6>::new(&engine).get::<CostReport>(&spend_query(CostGroupBy::Workflow, None, None, None)).await.unwrap();
        let by_key = |key: &str| report.rows.iter().find(|r| r.key == key);

        let wf_row = by_key(&definition.id).expect("the live workflow's own row");
        assert_eq!(wf_row.label.as_deref(), Some("release train"));
        assert!((wf_row.cost_usd - 2.0).abs() < 1e-9);

        let deleted_row = by_key("(deleted task)").expect("the deleted task's own row");
        assert_eq!(deleted_row.label, None, "a deleted task's workflow is unknown, never labelled");
        assert!((deleted_row.cost_usd - 1.0).abs() < 1e-9);

        let standalone_row = by_key(NO_WORKFLOW).expect("the standalone task's own row");
        assert_eq!(standalone_row.runs_unknown, 1, "no runtime answer was queued for it");

        assert_eq!(report.total.runs, 3);
    }

    #[tokio::test]
    async fn costs_report_carries_estimate_vs_actual_per_group() {
        let engine = engine(vec![]);
        // `dispatched`'s own point(900) estimate: a real test run never
        // takes exactly 900 seconds, so this one always lands outside its
        // own range.
        let (t1, r1) = dispatched(&engine, None).await;
        done(&engine, &t1, &r1).await;

        // A range starting at zero any wall time clears -- written straight
        // to the store, past `Engine::update`'s "at least one second" check,
        // since a real test run may complete inside the same second it
        // started.
        let wide = engine
            .l4_service()
            .create(NewTask {
                title: "wide".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                worktree: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        engine
            .l4.store
            .update(
                &wide.id,
                &TaskPatch {
                    estimate: Some(factory_core::task::Estimate {
                        time: factory_core::task::TimeEstimateRange { low: 0, expected: 1, high: 3600 },
                        cost: None,
                    }),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        engine.l4_service().start_run(&wide.id, Trigger::Manual).await;
        let wide_run = engine.l4.store.active_run(&wide.id).await.unwrap().expect("dispatched");
        done(&engine, &wide, &wide_run).await;

        let report = crate::facts::Facts::<factory_kernel::L6>::new(&engine).get::<CostReport>(&spend_query(CostGroupBy::Task, None, None, None)).await.unwrap();
        let t1_row = report.rows.iter().find(|r| r.key == t1.id).expect("t1's own row");
        assert_eq!(t1_row.estimated_runs, 1);
        assert_eq!(t1_row.within_range, 0, "a real run is never exactly 900s long");
        let wide_row = report.rows.iter().find(|r| r.key == wide.id).expect("wide's own row");
        assert_eq!(wide_row.estimated_runs, 1);
        assert_eq!(wide_row.within_range, 1);
        assert!(wide_row.median_actual_over_expected.is_some());
        assert_eq!(report.total.estimated_runs, 2);
        assert_eq!(report.total.within_range, 1);
        assert!(report.total.median_actual_over_expected.is_some());
    }
}
