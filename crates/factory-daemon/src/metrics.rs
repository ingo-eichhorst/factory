//! Metric computation: `factory_core::metrics`'s registry names the v1
//! vocabulary; this turns a `MetricId` into a `MetricValue` (and, for the
//! three production-based fixed metrics, a `MetricSeries`) off data the
//! daemon already has -- lazy, like a policy fact: only the ids actually
//! asked for are computed, and nothing here is a query language (design
//! §8), the same restraint `policies/mod.rs` holds toward evidence.
//!
//! ## Reuse, not reimplementation
//!
//! `throughput_week`/`first_pass_yield`/`scrap_rate` read `production.rs`'s
//! own daily grid (`Engine::production`) rather than re-deriving "finished"/
//! "scrapped"/"reworked" a second time -- that module's own doc comment is
//! the one place those words are defined, and this one only sums buckets it
//! already produced. `compliance.<fw>`/`open_controls.<fw>` read
//! `Engine::policy_report(None)`'s subtree rollup. `bench.resolve_rate.<dataset>`
//! reads the newest settled bench run through `bench::aggregate`, the same
//! function `bench show`'s own results table uses.
//! `goal_tasks_done.<objective>.<kr>` is the one metric with no existing
//! aggregate to lean on: it counts tasks itself, off the `TaskStore` trait
//! (`self.store`) so `ScopedStores` still shards correctly by scope --
//! unscoped, the same way `production.rs` reads every run before narrowing.
//!
//! The six operations metrics (`cycle_time_p50`/`_p85`, `queue_wait_p95`,
//! `fail_rate`, `rework_rate`, `time_to_recover_p50`) are
//! `factory_core::operations::registry_metric` over the trailing 28 days of
//! runs -- the same functions the Line tab's health strip calls, so a
//! KR over one of them and the tab can never disagree about its value.
//! Each is `as_of` the newest run behind it
//! (`operations::registry_metric_as_of`), not the moment it was asked for
//! -- the rule the production ratios keep (`ratio_value`).
//!
//! ## Series
//!
//! Only the three production-based metrics carry a history: one point per
//! day over `production.rs`'s own 53-week daily grid, each point the
//! metric's own trailing-window definition evaluated as of that day (a
//! rolling 7-day sum for `throughput_week`, a rolling 28-day ratio for the
//! other two). The last point always equals the metric's own current value.
//! Every other metric is a single number with no time axis of its own to
//! draw yet.
//!
//! `unit_cost` and `tokens_per_run` (#117) are
//! `factory_core::usage::usage_metric` over the same trailing 28 days of
//! runs, reading the usage each run carries -- measured by the agent
//! runtime, never guessed. A run whose usage is unknown is left out of both
//! sides of the figure; with none left the value is `None` and the reason
//! says how many finished runs there were.
//!
//! ## Unknown vs. unavailable
//!
//! An id `metrics::resolve` has never heard of refuses the whole call --
//! `Engine::metrics` is the one place a typo becomes a `BadRequest` rather
//! than a quiet `None`. An id it knows but cannot compute yet comes back as
//! `value: None` with the registry's own reason. No metric is in that state
//! today -- the last two, `unit_cost` and `tokens_per_run`, became
//! computable with #117 -- but the path stays for the next one. `default_metric_ids`, used only
//! when a caller's own `ids` was empty, filters the other way: it never
//! offers an *unknown* id (nothing built it), but does offer an
//! *unavailable* one, since "every non-parameterised metric" names both.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use chrono::{DateTime, NaiveDate, Utc};
use factory_core::environments::EnvironmentCard;
use factory_core::error::{FactoryError, Result};
use factory_core::goals::GoalsCatalogue;
use factory_core::metrics::{self, MetricDef, MetricError, MetricId, MetricSeries, MetricValue};
use factory_core::protocol::{MetricDefView, PolicyReport, ProductionBin};
use factory_core::quality::ScenarioStatus;
use factory_core::task::{TaskFilter, TaskStatus};

use crate::engine::Engine;

/// `Request::Metrics`'s answer, and the same shape `#100`'s Scenarios tab
/// calls `Engine::metrics` for directly -- a plain struct rather than
/// `Payload`, so an in-process caller never has to pattern-match its own
/// dispatch's wire type back apart.
#[derive(Debug)]
pub struct Metrics {
    pub values: Vec<MetricValue>,
    pub series: Vec<MetricSeries>,
    pub registry: Vec<MetricDefView>,
}

fn is_production_metric(id: &str) -> bool {
    matches!(id, "throughput_week" | "first_pass_yield" | "scrap_rate")
}

fn is_operations_metric(id: &str) -> bool {
    matches!(
        id,
        "cycle_time_p50" | "cycle_time_p85" | "queue_wait_p95" | "fail_rate" | "rework_rate" | "time_to_recover_p50"
    )
}

/// The window the operations metrics are read over -- the trailing 28 days
/// the production ratios use.
const OPERATIONS_WINDOW_DAYS: i64 = 28;

fn is_usage_metric(id: &str) -> bool {
    matches!(id, "unit_cost" | "tokens_per_run")
}

fn is_policy_metric(id: &str) -> bool {
    id.starts_with("compliance.") || id.starts_with("open_controls.")
}

fn is_quality_metric(id: &str) -> bool {
    id.starts_with("quality.")
}

/// `availability.<env>` and its siblings (`#185`): the metric's name and
/// the environment it names.
fn environment_metric(id: &str) -> Option<(&str, &str)> {
    let (name, env) = id.split_once('.')?;
    (metrics::ENVIRONMENT_METRICS.contains(&name) && !env.contains('.')).then_some((name, env))
}

impl Engine {
    /// Compute every id in `ids` (deduplicated), lazily: nothing not asked
    /// for is ever touched, and `production`/`policy_report`/the bench store
    /// are each read at most once per call regardless of how many ids ask
    /// for something behind them.
    pub(crate) async fn metrics(self: &Arc<Self>, ids: &[MetricId], now: DateTime<Utc>) -> Result<Metrics> {
        let mut wanted: Vec<MetricId> = Vec::new();
        for id in ids {
            if !wanted.contains(id) {
                wanted.push(id.clone());
            }
        }

        // Resolved up front: an `Unknown` id refuses the whole call before
        // anything is computed, rather than a partial answer with a gap in
        // it nobody asked to reason about.
        let mut resolved: Vec<(MetricId, std::result::Result<MetricDef, &'static str>)> = Vec::new();
        for id in &wanted {
            match metrics::resolve(id) {
                Ok(def) => resolved.push((id.clone(), Ok(def))),
                Err(MetricError::Unavailable { reason, .. }) => resolved.push((id.clone(), Err(reason))),
                Err(MetricError::Unknown(_)) => {
                    return Err(FactoryError::BadRequest(format!("{id} is not a known metric")));
                }
            }
        }

        // `quality.<characteristic>` is computed from a quality evaluation,
        // and that evaluation reads metrics of its own (a scenario on
        // `scrap_rate`, `compliance.cra`). Those are folded into this same
        // pass -- `extra` below -- rather than the evaluation calling back
        // into this function, so `production`/`policy_report` are read at
        // most once per call however the ids split between the caller and
        // quality. A quality evaluation that cannot load or be judged fails
        // only its own `quality.*` values, never the whole call: a dashboard
        // or Goals read must not break over one bad profile.
        let needs_quality = resolved.iter().any(|(id, r)| r.is_ok() && is_quality_metric(id.as_str()));
        let quality_inputs = if needs_quality {
            Some(self.quality_inputs(None, false).await.map_err(|e| e.to_string()))
        } else {
            None
        };
        let mut computing: Vec<(MetricId, std::result::Result<MetricDef, &'static str>)> =
            resolved.iter().filter(|(id, _)| !is_quality_metric(id.as_str())).cloned().collect();
        if let Some(Ok(inputs)) = &quality_inputs {
            for id in inputs.metric_ids() {
                if computing.iter().any(|(have, _)| have == &id) {
                    continue;
                }
                match metrics::resolve(&id) {
                    Ok(def) => computing.push((id, Ok(def))),
                    Err(MetricError::Unavailable { reason, .. }) => computing.push((id, Err(reason))),
                    // `metric_ids` already dropped every unknown id.
                    Err(MetricError::Unknown(_)) => {}
                }
            }
        }

        let needs_production = computing.iter().any(|(id, r)| r.is_ok() && is_production_metric(id.as_str()));
        let needs_policy = computing.iter().any(|(id, r)| r.is_ok() && is_policy_metric(id.as_str()));
        let needs_runs = computing
            .iter()
            .any(|(id, r)| r.is_ok() && (is_operations_metric(id.as_str()) || is_usage_metric(id.as_str())));

        let production = if needs_production {
            Some(self.production(Some(5), Some(ProductionBin::Day), None).await?)
        } else {
            None
        };
        // Twice the window: a recovery that ends inside it may have started
        // failing before it, and cutting the history at the window's edge
        // would shorten that streak rather than leave it out.
        let runs = if needs_runs {
            Some(
                self.store
                    .runs_between(now - chrono::Duration::days(2 * OPERATIONS_WINDOW_DAYS), now)
                    .await?,
            )
        } else {
            None
        };
        let policy_report = if needs_policy {
            Some(self.policy_report(None).await?)
        } else {
            None
        };
        let needs_environments =
            computing.iter().any(|(id, r)| r.is_ok() && environment_metric(id.as_str()).is_some());
        let environments = if needs_environments { Some(self.environment_cards().await?) } else { None };

        let mut computed: BTreeMap<MetricId, MetricValue> = BTreeMap::new();
        let mut computed_series: BTreeMap<MetricId, MetricSeries> = BTreeMap::new();
        for (id, def_result) in &computing {
            if def_result.is_err() {
                continue;
            }
            let (value, series) = self
                .compute_one(id, production.as_ref(), policy_report.as_ref(), runs.as_deref(), environments.as_ref(), now)
                .await?;
            computed.insert(id.clone(), value);
            if let Some(s) = series {
                computed_series.insert(id.clone(), s);
            }
        }

        let quality_report: Option<std::result::Result<QualityRollup, String>> = match &quality_inputs {
            None => None,
            Some(Err(e)) => Some(Err(e.clone())),
            Some(Ok(inputs)) => {
                let mut values = computed.clone();
                for (id, def) in &computing {
                    if let Err(reason) = def {
                        values.insert(id.clone(), unavailable_value(id, reason, now));
                    }
                }
                Some(
                    self.judge_quality(inputs, &values, now)
                        .await
                        .map(|(reports, _)| QualityRollup(reports))
                        .map_err(|e| e.to_string()),
                )
            }
        };

        let mut values = Vec::new();
        let mut series = Vec::new();
        let mut registry = Vec::new();

        for (id, def_result) in resolved {
            let def = match def_result {
                Ok(def) => def,
                Err(reason) => {
                    values.push(unavailable_value(&id, reason, now));
                    // `metrics::registry()` lists the fixed, unbound family
                    // for a metric this module can name but not compute --
                    // v1 has no *parameterised* unavailable family, so an
                    // exact-id lookup always finds it for today's two.
                    if let Some(found) = metrics::registry().into_iter().find(|d| d.id == id.as_str()) {
                        registry.push(found.into());
                    }
                    continue;
                }
            };
            let value = if let Some(characteristic) = id.as_str().strip_prefix("quality.") {
                match quality_report.as_ref().expect("needs_quality set") {
                    Ok(rollup) => quality_value(&id, rollup, characteristic, now),
                    Err(error) => MetricValue {
                        id: id.clone(),
                        value: None,
                        as_of: now,
                        reason: Some(format!("the quality evaluation failed: {error}")),
                    },
                }
            } else {
                if let Some(s) = computed_series.remove(&id) {
                    series.push(s);
                }
                computed.remove(&id).expect("every available non-quality id was computed")
            };
            values.push(value);
            registry.push(def.into());
        }

        Ok(Metrics { values, series, registry })
    }

    /// One available, non-`quality.*` metric's value, and its series when
    /// it has one, off the `production`/`policy_report`/`runs` this call
    /// already read (each `Some` exactly when some id needs it).
    async fn compute_one(
        &self,
        id: &MetricId,
        production: Option<&factory_core::protocol::Production>,
        policy_report: Option<&PolicyReport>,
        runs: Option<&[factory_core::run::Run]>,
        environments: Option<&BTreeMap<String, EnvironmentCard>>,
        now: DateTime<Utc>,
    ) -> Result<(MetricValue, Option<MetricSeries>)> {
        let daily = || &production.expect("needs_production set").daily;
        Ok(if id.as_str() == "throughput_week" {
            let s = throughput_week_series(daily());
            (value_from_series(&s, now, "no finished runs recorded yet"), Some(s))
        } else if id.as_str() == "first_pass_yield" {
            let s = first_pass_yield_series(daily());
            (ratio_value(value_from_series(&s, now, NO_RECENT_RUNS), daily(), 28, now), Some(s))
        } else if id.as_str() == "scrap_rate" {
            let s = scrap_rate_series(daily());
            (ratio_value(value_from_series(&s, now, NO_RECENT_RUNS), daily(), 28, now), Some(s))
        } else if is_operations_metric(id.as_str()) {
            (operations_value(id, runs.expect("needs_runs set"), now), None)
        } else if is_usage_metric(id.as_str()) {
            (usage_value(id, runs.expect("needs_runs set"), now), None)
        } else if let Some((name, env)) = environment_metric(id.as_str()) {
            (environment_value(id, environments.expect("needs_environments set"), name, env, now), None)
        } else if let Some(framework) = id.as_str().strip_prefix("compliance.") {
            (compliance_value(id, policy_report.expect("needs_policy set"), framework, now), None)
        } else if let Some(framework) = id.as_str().strip_prefix("open_controls.") {
            (open_controls_value(id, policy_report.expect("needs_policy set"), framework, now), None)
        } else if let Some(dataset) = id.as_str().strip_prefix("bench.resolve_rate.") {
            (self.bench_resolve_rate_value(id, dataset, now).await?, None)
        } else if let Some(rest) = id.as_str().strip_prefix("goal_tasks_done.") {
            let (objective, kr) = rest
                .split_once('.')
                .ok_or_else(|| FactoryError::Other(anyhow::anyhow!("malformed goal_tasks_done id {id}")))?;
            (self.goal_tasks_done_value(id, objective, kr, now).await?, None)
        } else {
            // Every family `metrics::resolve` returns `Ok` for today has
            // a branch above; a future metric added to the registry
            // without one here comes back honestly unresolved rather
            // than panicking a request that merely asked for it --
            // `factory-core` and `factory-daemon` are separate crates,
            // so the compiler cannot force the two to be added together.
            (
                MetricValue {
                    id: id.clone(),
                    value: None,
                    as_of: now,
                    reason: Some("no computation wired for this metric yet".to_string()),
                },
                None,
            )
        })
    }

    /// The default `ids` for `Request::Metrics` when a caller's own list is
    /// empty: every non-parameterised metric (available or not) plus, for
    /// every loaded policy catalogue, its `compliance`/`open_controls`
    /// pair, plus every metric id the goals catalogue itself names --
    /// `direction.yaml`'s `north_star`/`inputs` and every key result's
    /// bound metric. An id the catalogues name that `metrics::resolve` has
    /// never heard of is silently left out here (its own `UnknownMetric`
    /// finding already says so at `factory goals`), never turned into a
    /// refusal the way an explicitly typed unknown id is.
    pub(crate) async fn default_metric_ids(self: &Arc<Self>) -> Vec<MetricId> {
        let mut ids: Vec<MetricId> = metrics::registry()
            .into_iter()
            .filter(|d| !d.id.contains('<'))
            .filter_map(|d| MetricId::new(d.id).ok())
            .collect();

        let snapshot = self.factory_snapshot();
        let policies_dir = snapshot.policies_dir();
        let goals_dir = factory_core::goals::goals_dir(&snapshot.root);
        let quality_dir = snapshot.quality_dir();
        let (frameworks, catalogue, characteristics): (Vec<String>, GoalsCatalogue, BTreeSet<String>) =
            tokio::task::spawn_blocking(move || {
                let (catalogues, _findings) = factory_core::policy::load_all(&policies_dir);
                let frameworks = catalogues.into_iter().map(|c| c.framework).collect();
                let catalogue = factory_core::goals::load(&goals_dir);
                // Every characteristic any loaded profile declares an
                // attribute under -- the `quality.<characteristic>` twin of
                // one `compliance.<framework>` per loaded catalogue.
                let characteristics = factory_core::quality::load(&quality_dir)
                    .profiles
                    .values()
                    .flat_map(|p| &p.attributes)
                    .map(|a| factory_core::quality::characteristic_of(&a.id).to_string())
                    .collect();
                (frameworks, catalogue, characteristics)
            })
            .await
            .unwrap_or_default();

        for framework in &frameworks {
            if let Ok(id) = MetricId::new(format!("compliance.{framework}")) {
                ids.push(id);
            }
            if let Ok(id) = MetricId::new(format!("open_controls.{framework}")) {
                ids.push(id);
            }
        }

        ids.extend(goals_metric_ids(&catalogue));
        // Every declared environment's SLA figures and DORA keys (`#185`).
        for (_, decl) in snapshot.config.environments() {
            for name in metrics::ENVIRONMENT_METRICS {
                if let Ok(id) = MetricId::new(format!("{name}.{}", decl.name)) {
                    ids.push(id);
                }
            }
        }
        for characteristic in &characteristics {
            if let Ok(id) = MetricId::new(format!("quality.{characteristic}")) {
                ids.push(id);
            }
        }

        let mut seen = BTreeSet::new();
        ids.retain(|id| seen.insert(id.clone()));
        ids
    }

    async fn bench_resolve_rate_value(&self, id: &MetricId, dataset: &str, now: DateTime<Utc>) -> Result<MetricValue> {
        let runs = self.bench.runs(Some(dataset), 200).await?;
        let Some(run) = runs.into_iter().find(|r| r.settled()) else {
            return Ok(MetricValue {
                id: id.clone(),
                value: None,
                as_of: now,
                reason: Some(format!("no settled bench run for dataset {dataset:?}")),
            });
        };
        let results = factory_core::bench::aggregate(&run.attempts);
        let (pass, fail) = results.iter().fold((0u32, 0u32), |(p, f), r| (p + r.pass, f + r.fail));
        if pass + fail == 0 {
            return Ok(MetricValue {
                id: id.clone(),
                value: None,
                as_of: now,
                reason: Some(format!("nothing gated in the newest settled run of {dataset:?}")),
            });
        }
        // As of when that run settled, not when this was asked: a resolve
        // rate is exactly as old as the run it came from, and a reader that
        // holds it to a freshness window (a quality scenario's `max_age`)
        // has to see that. A settled run with no end recorded falls back to
        // its start, the older of the two, never to `now`.
        Ok(MetricValue {
            id: id.clone(),
            value: Some(f64::from(pass) / f64::from(pass + fail)),
            as_of: run.ended_at.unwrap_or(run.started_at),
            reason: None,
        })
    }

    /// Unscoped, like `production.rs`'s own read: a goal label names an
    /// objective/key-result pair, not a scope, and the objective it belongs
    /// to may itself carry any scope (or none). `ScopedStores::list` with no
    /// `scope` in the filter fans out over every configured store and
    /// merges the result, so this counts correctly regardless of where the
    /// labelled tasks actually live.
    async fn goal_tasks_done_value(&self, id: &MetricId, objective: &str, kr: &str, now: DateTime<Utc>) -> Result<MetricValue> {
        let label = format!("{objective}/{kr}");
        let tasks = self.store.list(&TaskFilter::default()).await?;
        let count = tasks
            .iter()
            .filter(|t| t.labels.get("goal").map(String::as_str) == Some(label.as_str()) && t.status == TaskStatus::Done)
            .count();
        Ok(MetricValue {
            id: id.clone(),
            value: Some(count as f64),
            as_of: now,
            reason: None,
        })
    }
}

/// Every metric id `catalogue` itself names -- `direction.yaml`'s
/// `north_star`/`inputs` and every key result's own bound metric -- filtered
/// to ids `metrics::resolve` has at least heard of (`Unknown` ones are
/// dropped; `Unavailable` ones are kept, so they still come back with their
/// reason rather than silently vanishing from a report).
pub(crate) fn goals_metric_ids(catalogue: &GoalsCatalogue) -> Vec<MetricId> {
    let mut ids = Vec::new();
    if let Some(direction) = &catalogue.direction {
        if let Some(north_star) = &direction.north_star {
            push_if_known(&mut ids, &north_star.metric);
        }
        for input in &direction.inputs {
            push_if_known(&mut ids, input);
        }
    }
    for cycle in &catalogue.cycles {
        for objective in &cycle.objectives {
            for kr in &objective.key_results {
                if let Some(bound) = kr.bound_metric(&objective.id) {
                    push_if_known(&mut ids, &bound);
                }
            }
        }
    }
    ids
}

pub(crate) fn push_if_known(ids: &mut Vec<MetricId>, id: &MetricId) {
    if !matches!(metrics::resolve(id), Err(MetricError::Unknown(_))) {
        ids.push(id.clone());
    }
}

/// Sum `finished`/`scrapped`/`reworked`/`first_pass` over a `window`-day
/// trailing window ending at each day in `daily` (clipped at the start of
/// the grid, so the earliest few points are over a shorter window than
/// `window`), and hand each sum to `calc` -- `None` skips that day's point
/// entirely (used for a ratio with nothing finished yet to divide by)
/// rather than fabricating a number.
fn rolling_series<F>(daily: &[factory_core::protocol::ProductionBucket], window: usize, calc: F) -> Vec<(NaiveDate, f64)>
where
    F: Fn(u32, u32, u32, u32) -> Option<f64>,
{
    let mut points = Vec::new();
    for i in 0..daily.len() {
        let start = i.saturating_sub(window.saturating_sub(1));
        let (mut finished, mut scrapped, mut reworked, mut first_pass) = (0u32, 0u32, 0u32, 0u32);
        for bucket in &daily[start..=i] {
            finished += bucket.finished;
            scrapped += bucket.scrapped;
            reworked += bucket.reworked;
            first_pass += bucket.first_pass;
        }
        if let Some(value) = calc(finished, scrapped, reworked, first_pass) {
            points.push((daily[i].from.date_naive(), value));
        }
    }
    points
}

fn throughput_week_series(daily: &[factory_core::protocol::ProductionBucket]) -> MetricSeries {
    MetricSeries {
        id: MetricId::new("throughput_week").expect("fixed id"),
        points: rolling_series(daily, 7, |finished, _, _, _| Some(f64::from(finished))),
    }
}

/// `first_pass / finished`, never `1 - reworked / finished` -- a bucket
/// where nothing was ever retried but nothing ever succeeded either
/// (`reworked: 0`, `first_pass: 0`) is a real `0.0`, not a manufactured
/// `1.0`. See `production.rs`'s module doc comment for `first_pass`'s own
/// definition.
fn first_pass_yield_series(daily: &[factory_core::protocol::ProductionBucket]) -> MetricSeries {
    MetricSeries {
        id: MetricId::new("first_pass_yield").expect("fixed id"),
        points: rolling_series(daily, 28, |finished, _, _, first_pass| {
            if finished == 0 {
                None
            } else {
                Some(f64::from(first_pass) / f64::from(finished))
            }
        }),
    }
}

fn scrap_rate_series(daily: &[factory_core::protocol::ProductionBucket]) -> MetricSeries {
    MetricSeries {
        id: MetricId::new("scrap_rate").expect("fixed id"),
        points: rolling_series(daily, 28, |finished, scrapped, _, _| {
            if finished == 0 {
                None
            } else {
                Some(f64::from(scrapped) / f64::from(finished))
            }
        }),
    }
}

/// A series' own last point is always the metric's current value -- this is
/// the one place that invariant is enforced, so `throughput_week`/
/// `first_pass_yield`/`scrap_rate` can never disagree with their own
/// sparkline.
fn value_from_series(series: &MetricSeries, now: DateTime<Utc>, empty_reason: &str) -> MetricValue {
    let value = series.points.last().map(|(_, v)| *v);
    MetricValue {
        id: series.id.clone(),
        value,
        as_of: now,
        reason: if value.is_none() { Some(empty_reason.to_string()) } else { None },
    }
}

const NO_RECENT_RUNS: &str = "no finished runs in the trailing 28 days";

/// A production ratio as its registry entry defines it -- over the trailing
/// `window` days -- with an honest `as_of`.
///
/// The rolling series skips a day whose own window finished nothing, so
/// its last point can be a *weeks-old* day's ratio; taken as-is, a team
/// that stopped running things 35 days ago would still read a value.
/// When nothing in the trailing `window` finished, there is no value for
/// "the trailing 28 days" at all: `None`, with the reason. Otherwise the
/// value is `as_of` the end of the newest daily bucket in that window that
/// finished anything -- when the data behind it stopped, to the day,
/// rather than the moment it was asked for -- so a freshness window (a
/// quality scenario's `max_age`) can read it as stale. The last bucket's
/// `to` is already clipped to the query's own moment
/// (`ProductionBucket::to`), so a run finished today reads as of now.
///
/// `throughput_week` deliberately keeps `now`: it is a count over a window
/// that ends now, so even a zero is a current fact, not an old one.
fn ratio_value(mut value: MetricValue, daily: &[factory_core::protocol::ProductionBucket], window: usize, now: DateTime<Utc>) -> MetricValue {
    let start = daily.len().saturating_sub(window);
    match daily[start..].iter().rev().find(|b| b.finished > 0) {
        Some(newest) if value.value.is_some() => value.as_of = newest.to,
        _ => {
            value.value = None;
            value.as_of = now;
            value.reason = Some(NO_RECENT_RUNS.to_string());
        }
    }
    value
}

/// One of the six operations metrics over the trailing
/// `OPERATIONS_WINDOW_DAYS`, `as_of` the newest run behind it
/// (`operations::registry_metric_as_of`) -- the rule `ratio_value` keeps
/// for the production ratios: a percentile or a rate over a window is as
/// old as the newest data in it, not the moment it was asked for, so a
/// quality scenario's `max_age` reads a figure nothing has moved in weeks
/// as stale. With no value, `now`, beside the reason -- the same as a
/// production ratio with nothing finished.
fn operations_value(id: &MetricId, runs: &[factory_core::run::Run], now: DateTime<Utc>) -> MetricValue {
    let window = factory_core::operations::Window::trailing(now, OPERATIONS_WINDOW_DAYS);
    match factory_core::operations::registry_metric(id.as_str(), runs, &window) {
        Some(figure) => MetricValue {
            id: id.clone(),
            as_of: figure
                .value
                .and(factory_core::operations::registry_metric_as_of(id.as_str(), runs, &window))
                .unwrap_or(now),
            value: figure.value,
            reason: figure.reason,
        },
        None => MetricValue {
            id: id.clone(),
            value: None,
            as_of: now,
            reason: Some("no computation wired for this metric yet".to_string()),
        },
    }
}

/// `unit_cost`/`tokens_per_run`, `as_of` the newest run end behind the
/// value, like the operations metrics -- `now` beside a reason when there
/// is none.
fn usage_value(id: &MetricId, runs: &[factory_core::run::Run], now: DateTime<Utc>) -> MetricValue {
    match factory_core::usage::usage_metric(id.as_str(), runs, now, OPERATIONS_WINDOW_DAYS) {
        Some(figure) => MetricValue {
            id: id.clone(),
            value: figure.value,
            as_of: figure.as_of.unwrap_or(now),
            reason: figure.reason,
        },
        None => MetricValue {
            id: id.clone(),
            value: None,
            as_of: now,
            reason: Some("no computation wired for this metric yet".to_string()),
        },
    }
}

/// One environment metric off the card the Operations tab draws, so the
/// two can never disagree. A figure with nothing to compute it from is
/// `None` with the reason.
fn environment_value(
    id: &MetricId,
    cards: &BTreeMap<String, EnvironmentCard>,
    name: &str,
    env: &str,
    now: DateTime<Utc>,
) -> MetricValue {
    let answer = |value: Option<f64>, reason: &str| MetricValue {
        id: id.clone(),
        value,
        as_of: now,
        reason: value.is_none().then(|| reason.to_string()),
    };
    let Some(card) = cards.get(env) else {
        return answer(None, &format!("no environment named {env:?} is declared or deployed to"));
    };
    let no_samples = "no health samples in the window yet";
    let dora = &card.dora;
    match name {
        "availability" => answer(card.uptime_window, no_samples),
        "error_budget" if card.slo.is_none() => answer(None, &format!("environment {env:?} declares no SLO")),
        "error_budget" => answer(card.error_budget, no_samples),
        "incidents" => answer(card.uptime_window.map(|_| card.incidents.len() as f64), no_samples),
        "mttr" => answer(dora.mttr, "no incident ended in the window"),
        "time_to_restore_p50" => answer(dora.time_to_restore_p50, "no incident ended in the window"),
        "deploy_frequency" => answer(dora.deploy_frequency, "no successful deployment in the window"),
        "lead_time_p50" => answer(
            dora.lead_time_p50,
            "no successful deployment in the window says when its commit was made",
        ),
        "change_failure_rate" => answer(dora.change_failure_rate, "no deployment finished in the window"),
        _ => answer(None, "no computation wired for this metric yet"),
    }
}

fn unavailable_value(id: &MetricId, reason: &str, now: DateTime<Utc>) -> MetricValue {
    MetricValue {
        id: id.clone(),
        value: None,
        as_of: now,
        reason: Some(reason.to_string()),
    }
}

/// The per-scope reports a `quality.<characteristic>` value is summed
/// from -- `QualityReport::scopes`' trees without the report around them,
/// since `Engine::metrics` judges them itself (`judge_quality`) rather than
/// asking for a whole report.
struct QualityRollup(Vec<factory_core::quality::ScopeReport>);

fn compliance_value(id: &MetricId, report: &PolicyReport, framework: &str, now: DateTime<Utc>) -> MetricValue {
    let Some(rollup) = report.rollup.iter().find(|r| r.framework == framework) else {
        return MetricValue {
            id: id.clone(),
            value: None,
            as_of: now,
            reason: Some(format!("no catalogue loaded for framework {framework:?}")),
        };
    };
    let c = &rollup.counts;
    let counted = c.satisfied + c.attested + c.stale + c.open + c.not_applicable;
    if counted == 0 {
        return MetricValue {
            id: id.clone(),
            value: None,
            as_of: now,
            reason: Some(format!("framework {framework:?} has no regulation/standard controls counted")),
        };
    }
    let value = (c.satisfied + c.attested + c.not_applicable) as f64 / counted as f64;
    MetricValue { id: id.clone(), value: Some(value), as_of: now, reason: None }
}

/// `quality.<characteristic>`: of every declared scenario under
/// `characteristic`, counted once per scope it applies in across the whole
/// instance, the share that is `met`. A draft or `no_data` scenario counts
/// in the denominator -- declared but not shown to be met is not met, the
/// same "never green without evidence" rule the tab itself keeps. `None`,
/// with the reason, for a characteristic ISO 25010 does not name or one no
/// scope declares anything under: nothing declared is not "all met".
fn quality_value(id: &MetricId, rollup: &QualityRollup, characteristic: &str, now: DateTime<Utc>) -> MetricValue {
    let unknown = factory_core::quality::CATALOGUE.iter().all(|c| c.id != characteristic);
    if unknown {
        return MetricValue {
            id: id.clone(),
            value: None,
            as_of: now,
            reason: Some(format!("{characteristic:?} is not an ISO 25010 characteristic")),
        };
    }
    let (met, declared) = rollup
        .0
        .iter()
        .flat_map(|s| &s.attributes)
        .filter(|a| a.characteristic == characteristic)
        .flat_map(|a| &a.scenarios)
        .fold((0u32, 0u32), |(met, all), s| (met + u32::from(s.status == ScenarioStatus::Met), all + 1));
    if declared == 0 {
        return MetricValue {
            id: id.clone(),
            value: None,
            as_of: now,
            reason: Some(format!("no scope declares a quality scenario under {characteristic}")),
        };
    }
    MetricValue {
        id: id.clone(),
        value: Some(f64::from(met) / f64::from(declared)),
        as_of: now,
        reason: None,
    }
}

fn open_controls_value(id: &MetricId, report: &PolicyReport, framework: &str, now: DateTime<Utc>) -> MetricValue {
    let Some(rollup) = report.rollup.iter().find(|r| r.framework == framework) else {
        return MetricValue {
            id: id.clone(),
            value: None,
            as_of: now,
            reason: Some(format!("no catalogue loaded for framework {framework:?}")),
        };
    };
    let c = &rollup.counts;
    MetricValue {
        id: id.clone(),
        value: Some((c.open + c.stale) as f64),
        as_of: now,
        reason: None,
    }
}

#[cfg(test)]
mod tests {
    //! Engine-level computation, on a temporary instance -- not
    //! `factory_core::metrics` itself (covered on its own), but this module
    //! glued to real data the way a real request sees it: real runs and
    //! tasks in the store, and a real policy catalogue on disk.

    use super::*;
    use factory_core::adapter::store::task_from_new;
    use factory_core::adapter::TaskStore;
    use factory_core::config::{Config, DaemonConfig, Factory, Instance, PolicyDeclaration, Scope};
    use factory_core::run::{NewRun, RunPatch, RunStatus, Trigger};
    use factory_core::task::{NewTask, TaskPatch};
    use factory_plugins::{Registry, SqliteStore};
    use std::path::PathBuf;

    fn scope_at(id: &str, name: &str, path: &str) -> Scope {
        let mut scope: Scope = serde_yaml_ng::from_str(&format!("id: {id}\nname: {name}\n")).unwrap();
        scope.path = PathBuf::from(path);
        scope
    }

    /// A one-scope instance, with `frameworks` (if any) committed at the
    /// root the same way `policies:` at the instance root always does. The
    /// root scope is declared (`config.scope`), so `goals.checkin`-shaped
    /// authorization checks in other modules' tests can reuse this too.
    fn test_engine(frameworks: Vec<String>) -> Arc<Engine> {
        let root = std::env::temp_dir().join(format!("factory-metrics-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let root_scope = scope_at("root-id", "root", ".");
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig::default(),
            scope: Some(root_scope.clone()),
            scopes: vec![root_scope],
            roles: Default::default(),
            dashboard: None,
            policies: PolicyDeclaration { frameworks, ..Default::default() },
            quality: Default::default(),
            infrastructure: Default::default(),
            plugins_dir: None,
        };
        let factory = Factory { root, config };
        let registry = Registry::with_builtins();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        Arc::new(Engine::new(factory, registry, store, PathBuf::from("factory"), Vec::new()))
    }

    /// A finished run for a fresh task in `root` (or the next attempt of an
    /// existing one with the same `label` -- the store assigns the attempt
    /// number), `status`, `trigger`, ended `ended_ago` before now. `trigger`
    /// is not the rework signal by itself any more than `attempt` is --
    /// `production.rs`'s `is_rework` also reads the *previous* attempt's own
    /// status, which a second call with the same `label` supplies for free.
    /// Returns the task so a caller can patch its own status afterward
    /// (`goal_tasks_done`'s own tests).
    async fn finished_run(
        engine: &Arc<Engine>,
        label: &str,
        status: RunStatus,
        trigger: Trigger,
        ended_ago: chrono::Duration,
    ) -> factory_core::task::Task {
        let existing = engine
            .store
            .list(&Default::default())
            .await
            .unwrap()
            .into_iter()
            .find(|t: &factory_core::task::Task| t.title == label);
        let task = match existing {
            Some(t) => t,
            None => {
                let new_task = task_from_new(
                    NewTask { title: label.to_string(), ..Default::default() },
                    "root".to_string(),
                    "assistant".to_string(),
                    "shell".to_string(),
                );
                engine.store.create(&new_task).await.unwrap()
            }
        };
        let run = engine
            .store
            .create_run(&NewRun {
                task_id: task.id.clone(),
                trigger,
                agent: "assistant".to_string(),
                adapter: "shell".to_string(),
                runtime: "shell".to_string(),
                token: "tok".to_string(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        engine
            .store
            .update_run(
                &run.id,
                &RunPatch {
                    status: Some(status),
                    ended_at: Some(Utc::now() - ended_ago),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        task
    }

    // ------------------------------------------------------- operations

    #[tokio::test]
    async fn the_operations_metrics_are_computed_from_real_runs() {
        let engine = test_engine(Vec::new());
        finished_run(&engine, "clean", RunStatus::Done, Trigger::Manual, chrono::Duration::hours(1)).await;
        finished_run(&engine, "flaky", RunStatus::Failed, Trigger::Manual, chrono::Duration::hours(3)).await;
        finished_run(&engine, "flaky", RunStatus::Done, Trigger::Manual, chrono::Duration::hours(1)).await;

        let ids: Vec<MetricId> = ["fail_rate", "rework_rate", "time_to_recover_p50", "queue_wait_p95", "cycle_time_p50"]
            .into_iter()
            .map(|id| MetricId::new(id).unwrap())
            .collect();
        let computed = engine.metrics(&ids, Utc::now()).await.unwrap();
        let get = |id: &str| computed.values.iter().find(|v| v.id.as_str() == id).unwrap().clone();

        assert_eq!(get("fail_rate").value, Some(1.0 / 3.0));
        assert_eq!(get("rework_rate").value, Some(1.0 / 3.0), "flaky's second attempt");
        let recover = get("time_to_recover_p50").value.unwrap();
        assert!((recover - 7200.0).abs() < 5.0, "failed three hours ago, done one hour ago: {recover}");
        assert!(get("queue_wait_p95").value.is_some(), "every run made now records its queue wait");
        assert!(get("cycle_time_p50").value.is_some());
        // As of the newest run behind each, never the moment asked: "flaky"
        // ended done an hour ago, the newest end any of them counts.
        for id in ["fail_rate", "rework_rate", "time_to_recover_p50", "cycle_time_p50"] {
            let age = Utc::now() - get(id).as_of;
            assert!((age.num_seconds() - 3600).abs() < 60, "{id} is as of {age} ago");
        }
        assert_eq!(computed.registry.len(), ids.len(), "each comes with its definition");
    }

    #[tokio::test]
    async fn an_operations_metric_with_no_runs_is_none_with_a_reason() {
        let engine = test_engine(Vec::new());
        let ids = vec![MetricId::new("cycle_time_p85").unwrap()];
        let computed = engine.metrics(&ids, Utc::now()).await.unwrap();
        assert_eq!(computed.values[0].value, None);
        assert_eq!(
            computed.values[0].reason.as_deref(),
            Some(factory_core::operations::METRIC_EMPTY_NO_DONE)
        );
    }

    // ------------------------------------------------------- production

    #[tokio::test]
    async fn throughput_yield_and_scrap_are_computed_from_real_runs() {
        let engine = test_engine(Vec::new());
        // "clean": done, attempt 1, no predecessor -- first-pass.
        // "retried": attempt 1 failed (scrapped, not first-pass), attempt 2
        // done -- its predecessor failed, so it is rework, not first-pass.
        // "requeued": attempt 1 done (first-pass), attempt 2 also done --
        // its predecessor already finished `done`, so re-running it is new
        // work, not rework, and attempt 2 is first-pass too (the bug this
        // module's `production.rs` counterpart exists to fix: a task run
        // again after succeeding is not a correction just because its own
        // `attempt` climbed).
        // "scrapped": failed, attempt 1, no predecessor -- scrapped only.
        // Six finished runs total: three first-pass, one reworked, two
        // scrapped.
        finished_run(&engine, "clean", RunStatus::Done, Trigger::Manual, chrono::Duration::hours(1)).await;
        finished_run(&engine, "retried", RunStatus::Failed, Trigger::Manual, chrono::Duration::hours(3)).await;
        finished_run(&engine, "retried", RunStatus::Done, Trigger::Manual, chrono::Duration::hours(2)).await;
        finished_run(&engine, "requeued", RunStatus::Done, Trigger::Manual, chrono::Duration::hours(3)).await;
        finished_run(&engine, "requeued", RunStatus::Done, Trigger::Manual, chrono::Duration::hours(1)).await;
        finished_run(&engine, "scrapped", RunStatus::Failed, Trigger::Manual, chrono::Duration::hours(1)).await;

        let now = Utc::now();
        let ids = vec![
            MetricId::new("throughput_week").unwrap(),
            MetricId::new("first_pass_yield").unwrap(),
            MetricId::new("scrap_rate").unwrap(),
        ];
        let computed = engine.metrics(&ids, now).await.unwrap();

        let get = |id: &str| computed.values.iter().find(|v| v.id.as_str() == id).unwrap().clone();
        assert_eq!(get("throughput_week").value, Some(6.0), "six finished runs in the trailing week");
        assert_eq!(get("first_pass_yield").value, Some(3.0 / 6.0), "clean and both of requeued's runs are first-pass");
        assert_eq!(get("scrap_rate").value, Some(2.0 / 6.0), "retried's own first attempt and scrapped were both scrap");

        // The series' own last point always equals the metric's value.
        for id in ["throughput_week", "first_pass_yield", "scrap_rate"] {
            let series = computed.series.iter().find(|s| s.id.as_str() == id).unwrap();
            let last = series.points.last().unwrap().1;
            assert_eq!(Some(last), get(id).value, "{id}'s series must end on its own value");
        }
    }

    #[tokio::test]
    async fn every_run_scrapped_is_a_zero_first_pass_yield_not_a_perfect_one() {
        // The bug `first_pass` (production.rs) exists to fix: five finished
        // runs, all failed on their only attempt -- `1 - reworked/finished`
        // would read this as `1.0` (nothing was ever reworked); the true
        // fact is `0.0` (nothing ever succeeded on the first try either).
        let engine = test_engine(Vec::new());
        for i in 0..5 {
            finished_run(&engine, &format!("scrapped-{i}"), RunStatus::Failed, Trigger::Manual, chrono::Duration::hours(1)).await;
        }
        let now = Utc::now();
        let computed = engine.metrics(&[MetricId::new("first_pass_yield").unwrap()], now).await.unwrap();
        assert_eq!(computed.values[0].value, Some(0.0));
    }

    #[tokio::test]
    async fn a_task_reworked_then_done_does_not_count_as_first_pass() {
        // Attempt 1 fails, attempt 2 (a manual re-run of the same task)
        // succeeds: the run that actually ended `done` is not first-pass,
        // because it re-runs a run that failed -- `first_pass_yield` must
        // not credit a task for succeeding only after being retried.
        let engine = test_engine(Vec::new());
        finished_run(&engine, "retried", RunStatus::Failed, Trigger::Manual, chrono::Duration::hours(2)).await;
        finished_run(&engine, "retried", RunStatus::Done, Trigger::Manual, chrono::Duration::hours(1)).await;
        let now = Utc::now();
        let computed = engine.metrics(&[MetricId::new("first_pass_yield").unwrap()], now).await.unwrap();
        assert_eq!(computed.values[0].value, Some(0.0), "two finished runs, neither of them not-rework and done");
    }

    #[tokio::test]
    async fn a_task_rerun_after_done_counts_as_first_pass_both_times() {
        // The bug `is_rework` (production.rs) exists to fix: re-running a
        // task whose previous run already finished `done` is new work on a
        // standing task, not a correction, however much `attempt` climbs --
        // unlike a retried, previously-failed task, both runs are first-pass.
        let engine = test_engine(Vec::new());
        finished_run(&engine, "requeued", RunStatus::Done, Trigger::Manual, chrono::Duration::hours(2)).await;
        finished_run(&engine, "requeued", RunStatus::Done, Trigger::Manual, chrono::Duration::hours(1)).await;
        let now = Utc::now();
        let computed = engine.metrics(&[MetricId::new("first_pass_yield").unwrap()], now).await.unwrap();
        assert_eq!(computed.values[0].value, Some(1.0), "neither run re-attempts a failure, so both are first-pass");
    }

    #[tokio::test]
    async fn a_retry_triggered_run_is_rework_regardless_of_the_previous_runs_status() {
        // `Trigger::Retry` is the daemon's own automatic retry of a run that
        // just failed (`scheduler.rs`'s `resume_from_retry`) -- always
        // rework, the same as a manual re-run after a failure, without
        // needing this test to construct a failed predecessor at all.
        let engine = test_engine(Vec::new());
        finished_run(&engine, "auto-retried", RunStatus::Done, Trigger::Retry, chrono::Duration::hours(1)).await;
        let now = Utc::now();
        let computed = engine.metrics(&[MetricId::new("first_pass_yield").unwrap()], now).await.unwrap();
        assert_eq!(computed.values[0].value, Some(0.0), "done but rework, so not first-pass");
    }

    #[tokio::test]
    async fn a_scheduled_tasks_repeated_firings_are_never_rework() {
        // The bug this whole change exists to fix: a task fired on a
        // schedule racks up `attempt`s the same as any other task, but none
        // of its firings are rework -- see `production.rs`'s module doc
        // comment. Ten done, scheduled firings of the same task: all ten are
        // first-pass, none are reworked.
        let engine = test_engine(Vec::new());
        for i in 0..10 {
            finished_run(&engine, "scheduled", RunStatus::Done, Trigger::Schedule, chrono::Duration::minutes(30 * (10 - i))).await;
        }
        let now = Utc::now();
        let ids = vec![MetricId::new("throughput_week").unwrap(), MetricId::new("first_pass_yield").unwrap()];
        let computed = engine.metrics(&ids, now).await.unwrap();
        let get = |id: &str| computed.values.iter().find(|v| v.id.as_str() == id).unwrap().clone();
        assert_eq!(get("throughput_week").value, Some(10.0));
        assert_eq!(get("first_pass_yield").value, Some(1.0), "every scheduled firing is first-pass, none reworked");
    }

    #[tokio::test]
    async fn no_finished_runs_gives_none_with_a_reason() {
        let engine = test_engine(Vec::new());
        let now = Utc::now();
        let ids = vec![MetricId::new("first_pass_yield").unwrap(), MetricId::new("scrap_rate").unwrap()];
        let computed = engine.metrics(&ids, now).await.unwrap();
        for v in &computed.values {
            assert_eq!(v.value, None, "{}", v.id);
            assert!(v.reason.is_some(), "{} should carry a reason", v.id);
        }
    }

    // ---------------------------------------------------------- compliance

    #[tokio::test]
    async fn compliance_and_open_controls_read_the_policy_rollup() {
        let engine = test_engine(vec!["cra".to_string()]);
        {
            let snapshot = engine.factory_snapshot();
            std::fs::create_dir_all(snapshot.policies_dir()).unwrap();
            std::fs::write(
                snapshot.policies_dir().join("cra.yaml"),
                "framework: cra\ntitle: CRA\nkind: regulation\ncontrols:\n\
                 \x20\x20- id: a\n\x20\x20\x20\x20title: A\n\x20\x20\x20\x20evidence:\n\x20\x20\x20\x20\x20\x20- check: knowledge\n\
                 \x20\x20- id: b\n\x20\x20\x20\x20title: B\n\x20\x20\x20\x20evidence:\n\x20\x20\x20\x20\x20\x20- check: attestation\n",
            )
            .unwrap();
            std::fs::create_dir_all(snapshot.root.join(".factory/knowledge")).unwrap();
            std::fs::write(
                snapshot.root.join(".factory/knowledge/page.md"),
                "---\ntags: [control/cra/a]\n---\n# Page\n",
            )
            .unwrap();
        }

        let now = Utc::now();
        let ids = vec![MetricId::new("compliance.cra").unwrap(), MetricId::new("open_controls.cra").unwrap()];
        let computed = engine.metrics(&ids, now).await.unwrap();
        let compliance = computed.values.iter().find(|v| v.id.as_str() == "compliance.cra").unwrap();
        let open = computed.values.iter().find(|v| v.id.as_str() == "open_controls.cra").unwrap();
        assert_eq!(compliance.value, Some(0.5), "one of two counted controls is satisfied");
        assert_eq!(open.value, Some(1.0), "the other is open");
    }

    #[tokio::test]
    async fn an_unloaded_framework_is_none_with_a_reason() {
        let engine = test_engine(Vec::new());
        let now = Utc::now();
        let computed = engine.metrics(&[MetricId::new("compliance.nope").unwrap()], now).await.unwrap();
        assert_eq!(computed.values[0].value, None);
        assert!(computed.values[0].reason.as_deref().unwrap().contains("nope"));
    }

    // ------------------------------------------------------- bench.resolve_rate

    /// A settled bench run with two attempts (one pass, one fail) across two
    /// configurations, summed into one resolve rate for the whole run --
    /// `bench.resolve_rate.<dataset>` names no agent or configuration of its
    /// own, unlike `bench show`'s own per-configuration table.
    #[tokio::test]
    async fn bench_resolve_rate_sums_pass_and_fail_across_every_configuration_in_the_newest_settled_run() {
        use factory_core::bench::{BenchAttempt, BenchRun, BenchRunStatus, Verdict};

        let engine = test_engine(Vec::new());
        let mut pass = BenchAttempt::pending("a1".into(), "case1".into(), "agent-a".into(), 1);
        pass.verdict = Some(Verdict::Pass);
        let mut fail = BenchAttempt::pending("a2".into(), "case2".into(), "agent-b".into(), 1);
        fail.verdict = Some(Verdict::Fail);
        let run = BenchRun {
            id: "run-1".into(),
            dataset: "eval-set-a".into(),
            dataset_revision: 1,
            cases: Vec::new(),
            case_bases: Default::default(),
            agents: vec!["agent-a".into(), "agent-b".into()],
            attempts_per_case: 1,
            concurrency: 1,
            status: BenchRunStatus::Done,
            attempts: vec![pass.clone(), fail.clone()],
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
        };
        // `put_run` never trusts `attempts` embedded on the struct -- they
        // live in their own table (`BenchStore::attempts`), written
        // separately, the same way `bench::engine` itself writes them.
        engine.bench.put_run(&run).await.unwrap();
        engine.bench.put_attempt(&run.id, &pass).await.unwrap();
        engine.bench.put_attempt(&run.id, &fail).await.unwrap();

        let now = Utc::now();
        let id = MetricId::new("bench.resolve_rate.eval-set-a").unwrap();
        let computed = engine.metrics(&[id], now).await.unwrap();
        assert_eq!(computed.values[0].value, Some(0.5), "one pass and one fail across two configurations");
    }

    /// `as_of` is when the data behind a value is from, not when it was
    /// asked for -- otherwise a freshness window (a quality scenario's
    /// `max_age`) could never read anything as stale.
    #[tokio::test]
    async fn a_ratio_and_a_resolve_rate_are_as_of_their_newest_data_not_the_moment_asked() {
        use factory_core::bench::{BenchAttempt, BenchRun, BenchRunStatus, Verdict};

        let engine = test_engine(Vec::new());
        finished_run(&engine, "old", RunStatus::Done, Trigger::Manual, chrono::Duration::days(10)).await;
        let settled = Utc::now() - chrono::Duration::days(3);
        let mut pass = BenchAttempt::pending("a1".into(), "case1".into(), "agent-a".into(), 1);
        pass.verdict = Some(Verdict::Pass);
        let run = BenchRun {
            id: "run-1".into(),
            dataset: "smoke".into(),
            dataset_revision: 1,
            cases: Vec::new(),
            case_bases: Default::default(),
            agents: vec!["agent-a".into()],
            attempts_per_case: 1,
            concurrency: 1,
            status: BenchRunStatus::Done,
            attempts: vec![pass.clone()],
            started_at: settled - chrono::Duration::hours(1),
            ended_at: Some(settled),
        };
        engine.bench.put_run(&run).await.unwrap();
        engine.bench.put_attempt(&run.id, &pass).await.unwrap();

        let now = Utc::now();
        let ids: Vec<MetricId> = ["first_pass_yield", "scrap_rate", "throughput_week", "bench.resolve_rate.smoke"]
            .into_iter()
            .map(|id| MetricId::new(id).unwrap())
            .collect();
        let computed = engine.metrics(&ids, now).await.unwrap();
        let get = |id: &str| computed.values.iter().find(|v| v.id.as_str() == id).unwrap().clone();

        for id in ["first_pass_yield", "scrap_rate"] {
            let age = now - get(id).as_of;
            assert!(
                age >= chrono::Duration::days(9) && age <= chrono::Duration::days(10),
                "{id} is as old as the day its newest finished run ended, to the day: {age}"
            );
        }
        assert_eq!(get("throughput_week").as_of, now, "a trailing count ending now is a current fact, even a zero");
        assert_eq!(get("bench.resolve_rate.smoke").as_of, settled, "as of the run it came from");
    }

    /// The rolling series skips a day whose window finished nothing, so its
    /// last point can be a weeks-old day's ratio. A ratio over "the
    /// trailing 28 days" with nothing finished in them has no value.
    #[tokio::test]
    async fn a_ratio_with_nothing_finished_in_its_window_is_none_not_an_old_value() {
        let engine = test_engine(Vec::new());
        finished_run(&engine, "ancient", RunStatus::Done, Trigger::Manual, chrono::Duration::days(35)).await;
        let now = Utc::now();
        let ids = vec![MetricId::new("first_pass_yield").unwrap(), MetricId::new("scrap_rate").unwrap()];
        let computed = engine.metrics(&ids, now).await.unwrap();
        for v in &computed.values {
            assert_eq!(v.value, None, "{}", v.id);
            assert_eq!(v.reason.as_deref(), Some("no finished runs in the trailing 28 days"), "{}", v.id);
            assert_eq!(v.as_of, now);
        }
    }

    #[tokio::test]
    async fn bench_resolve_rate_is_none_with_a_reason_when_no_settled_run_exists() {
        let engine = test_engine(Vec::new());
        let now = Utc::now();
        let id = MetricId::new("bench.resolve_rate.eval-set-a").unwrap();
        let computed = engine.metrics(&[id], now).await.unwrap();
        assert_eq!(computed.values[0].value, None);
        assert!(computed.values[0].reason.as_deref().unwrap().contains("no settled bench run"));
    }

    // ------------------------------------------------------- goal_tasks_done

    #[tokio::test]
    async fn goal_tasks_done_counts_only_the_labelled_and_done_tasks() {
        let engine = test_engine(Vec::new());

        let mut labelled_done = NewTask { title: "done".into(), ..Default::default() };
        labelled_done.labels.insert("goal".to_string(), "ship-compliant/cra-open-zero".to_string());
        let t1 = engine
            .store
            .create(&task_from_new(labelled_done, "root".to_string(), "assistant".to_string(), "shell".to_string()))
            .await
            .unwrap();
        engine
            .store
            .update(&t1.id, &TaskPatch { status: Some(factory_core::task::TaskStatus::Done), ..Default::default() })
            .await
            .unwrap();

        let mut labelled_running = NewTask { title: "running".into(), ..Default::default() };
        labelled_running.labels.insert("goal".to_string(), "ship-compliant/cra-open-zero".to_string());
        engine
            .store
            .create(&task_from_new(labelled_running, "root".to_string(), "assistant".to_string(), "shell".to_string()))
            .await
            .unwrap();

        let mut other_label_done = NewTask { title: "other".into(), ..Default::default() };
        other_label_done.labels.insert("goal".to_string(), "raise-quality/fpy-90".to_string());
        let t3 = engine
            .store
            .create(&task_from_new(other_label_done, "root".to_string(), "assistant".to_string(), "shell".to_string()))
            .await
            .unwrap();
        engine
            .store
            .update(&t3.id, &TaskPatch { status: Some(factory_core::task::TaskStatus::Done), ..Default::default() })
            .await
            .unwrap();

        let now = Utc::now();
        let id = MetricId::new("goal_tasks_done.ship-compliant.cra-open-zero").unwrap();
        let computed = engine.metrics(&[id], now).await.unwrap();
        assert_eq!(computed.values[0].value, Some(1.0), "only the done task under this exact objective/kr counts");
    }

    // ---------------------------------------------------- unknown/unavailable

    #[tokio::test]
    async fn an_unknown_id_refuses_the_whole_call() {
        let engine = test_engine(Vec::new());
        let err = engine.metrics(&[MetricId::new("bogus_metric").unwrap()], Utc::now()).await.unwrap_err();
        assert!(err.to_string().contains("bogus_metric"), "{err}");
    }

    #[tokio::test]
    async fn unit_cost_and_tokens_per_run_read_the_usage_runs_carry() {
        use factory_core::usage::{RunUsage, TokenCounts, UsageState};
        let engine = test_engine(Vec::new());
        let ids = [MetricId::new("unit_cost").unwrap(), MetricId::new("tokens_per_run").unwrap()];

        // Nothing measured yet: no value, and a reason -- never a zero.
        finished_run(&engine, "unmeasured", RunStatus::Done, Trigger::Manual, chrono::Duration::hours(3)).await;
        let computed = engine.metrics(&ids, Utc::now()).await.unwrap();
        for v in &computed.values {
            assert_eq!(v.value, None, "{v:?}");
            assert!(v.reason.as_deref().unwrap().contains("none of the 1 runs"), "{v:?}");
        }
        assert!(computed.registry.iter().all(|d| d.available));

        let measured = |usd: f64, tokens: u64| RunUsage {
            state: UsageState::Known,
            reason: None,
            tokens: TokenCounts { input: Some(tokens), output: Some(0), cache_read: Some(0), cache_write: Some(0) },
            cost_usd: Some(usd),
            ..RunUsage::unknown("", 2)
        };
        for (label, status, usd, tokens) in
            [("a", RunStatus::Done, 2.0, 1_000), ("b", RunStatus::Failed, 1.0, 3_000)]
        {
            let task = finished_run(&engine, label, status, Trigger::Manual, chrono::Duration::hours(2)).await;
            let run = engine.store.runs(&task.id, 1).await.unwrap().remove(0);
            engine
                .store
                .update_run(&run.id, &RunPatch { usage: Some(measured(usd, tokens)), ..Default::default() })
                .await
                .unwrap();
        }
        let computed = engine.metrics(&ids, Utc::now()).await.unwrap();
        let get = |id: &str| computed.values.iter().find(|v| v.id.as_str() == id).unwrap();
        assert_eq!(get("unit_cost").value, Some(3.0), "a failed run's cost is spread over what got done");
        assert_eq!(get("tokens_per_run").value, Some(2_000.0));
    }

    // -------------------------------------------------------- default ids

    #[tokio::test]
    async fn default_ids_include_every_fixed_metric_and_what_the_catalogues_imply() {
        let engine = test_engine(vec!["cra".to_string()]);
        {
            let snapshot = engine.factory_snapshot();
            std::fs::create_dir_all(snapshot.policies_dir()).unwrap();
            std::fs::write(
                snapshot.policies_dir().join("cra.yaml"),
                "framework: cra\ntitle: CRA\nkind: regulation\ncontrols:\n\
                 \x20\x20- id: a\n\x20\x20\x20\x20title: A\n\x20\x20\x20\x20evidence:\n\x20\x20\x20\x20\x20\x20- check: knowledge\n",
            )
            .unwrap();
            let goals_dir = factory_core::goals::goals_dir(&snapshot.root);
            std::fs::create_dir_all(&goals_dir).unwrap();
            std::fs::write(
                goals_dir.join("direction.yaml"),
                "vision: V\nmission: M\nnorth_star: {metric: first_pass_yield, why: because}\ninputs: [throughput_week]\n",
            )
            .unwrap();
            std::fs::write(
                goals_dir.join("2026-q4.yaml"),
                "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
                 objectives:\n\
                 \x20\x20- id: obj\n\x20\x20\x20\x20title: T\n\x20\x20\x20\x20key_results:\n\
                 \x20\x20\x20\x20\x20\x20- {id: kr, title: K, kind: committed, metric: compliance.cra, baseline: 0, target: 1}\n",
            )
            .unwrap();
        }

        let ids = engine.default_metric_ids().await;
        let has = |s: &str| ids.iter().any(|id| id.as_str() == s);
        assert!(has("throughput_week"), "fixed metrics are always in the default set");
        assert!(has("unit_cost"), "a fixed metric, so in the default set");
        assert!(has("tokens_per_run"));
        assert!(has("compliance.cra"), "implied by the loaded policy catalogue");
        assert!(has("open_controls.cra"));
        assert!(has("compliance.cra"), "also implied by the goals catalogue's own key result");
    }
}
