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
//! own daily grid through L4's `ProductionFact`, rather than re-deriving "finished"/
//! "scrapped"/"reworked" a second time -- that module's own doc comment is
//! the one place those words are defined, and this one only sums buckets it
//! already produced. `compliance.<fw>`/`open_controls.<fw>` read
//! `Engine::policy_report(scope)`'s subtree rollup. `bench.resolve_rate.<dataset>`
//! reads L5's `BenchResolutionFact`, whose producer uses `bench::aggregate`,
//! the same function `bench show` uses. L4's `ProcessMetricFact` owns the
//! run, occupancy, intake and goal-task measurements. Its goal counts remain
//! instance-wide; its other scope-aware figures follow the requested path
//! subtree. This module has no direct process or benchmark store access.
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
//! `unit_cost` and `tokens_per_run` read the finished cohort of L4's
//! `CostReport`, with explicit unknown and partial usage. The L4
//! `estimate_accuracy` measurement reads each
//! run's own `original_estimate` and wall time, and leaves out a run with
//! no estimate to compare against. With none left, the
//! value is `None` and the reason says how many finished runs there were.
//!
//! `cost_week` (#164) also reads L4's `CostReport` -- the trailing 7 days of runs
//! *started*, not the `ended_at` rule the
//! three metrics above use. `None`, with a reason naming the known sum and
//! the counts, whenever any run in the window is unknown, cost-unknown or
//! partial.
//!
//! The four intake metrics (`ready_rate`, `needs_info_rate`,
//! `duplicate_rate`, `intake_lead_time`, #165) are
//! `factory_core::intake::registry_metric` over decision events read off the
//! task journal (`TaskStore::entries_of_kinds` over `triage_verdict`,
//! `intake_needs_info`, `intake_closed`, `intake_split`), the same trailing
//! 28 days by default -- `Intake.decision` is never read here, since it
//! keeps only the latest decision and this needs every one. Scope narrows
//! by the task's own current scope, canonicalised, the same rule the
//! run-backed families follow.
//!
//! `backup_age_hours` and `backup_verified_age_days` (#154) are read off one
//! `Engine::backup_fact(now)` call, at most once per `metrics_for` call --
//! the same `BackupFact` a policy `daemon` check reads, never the report's
//! own repository or Time Machine probes. Both are `as_of` the fact's own
//! `at` (the `now` passed to `backup_fact`), not the moment the metric was
//! asked for -- the same rule every other family here keeps. A value with no
//! backup configured, an unreachable destination, no snapshot yet, no
//! verification yet, or a failed newest verification is `None`, with the
//! reason spelled out rather than a bare `0`.
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
use factory_kernel::{EnvironmentMetricFact, BackupFact, AttestedRun, L6};
use crate::facts::{Facts, AttestedQuery};
use factory_core::error::{FactoryError, Result};
use factory_core::goals::GoalsCatalogue;
use factory_core::metrics::{
    self, MetricDef, MetricError, MetricId, MetricSeries, MetricValue, MetricsWindow,
};
use factory_core::protocol::{
    MetricDefView, PolicyReport, ProductionBin, ProductionBucket,
};
use factory_core::quality::ScenarioStatus;

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
    id == "estimate_accuracy"
}

fn is_hours_metric(id: &str) -> bool {
    matches!(id, "agent_hours" | "blocked_hours")
}

/// `#154`'s two backup metrics -- both read off one `Engine::backup_fact`
/// call, at most once per `metrics_for` call.
fn is_backup_metric(id: &str) -> bool {
    matches!(id, "backup_age_hours" | "backup_verified_age_days")
}

fn is_intake_metric(id: &str) -> bool {
    matches!(id, "ready_rate" | "needs_info_rate" | "duplicate_rate" | "intake_lead_time")
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

/// `#158`: conformance, gate failure and independent review rejection, the
/// registry families `Engine::attested_runs` backs -- one read shared
/// by all, whatever mix of categories and however many a
/// request actually asks for.
fn is_attestation_metric(id: &str) -> bool {
    id.starts_with("conformance_rate.") || id == "gate_fail_rate" || id == "review_reject_rate"
}

impl Engine {
    /// Compute every id in `ids` (deduplicated), lazily: nothing not asked
    /// for is ever touched, and each backing read is shared by every id that
    /// needs it (production is read once per exact scope when a subtree must
    /// be aggregated).
    pub(crate) async fn metrics(self: &Arc<Self>, ids: &[MetricId], now: DateTime<Utc>) -> Result<Metrics> {
        self.metrics_for(ids, now, None, None).await
    }

    /// The request-facing form of [`Engine::metrics`]: optionally narrow
    /// every scope-aware family to one scope's `Scope.path` subtree and
    /// override each run-backed family's established default interval.
    pub(crate) async fn metrics_for(
        self: &Arc<Self>,
        ids: &[MetricId],
        now: DateTime<Utc>,
        scope: Option<&str>,
        window: Option<MetricsWindow>,
    ) -> Result<Metrics> {
        // Resolve once even when the caller asks only for an instance-wide
        // family: an unknown scope is a bad request, never an empty-looking
        // bench or goal value.
        let snapshot = self.factory_snapshot();
        let (asked_scope, target_scopes) = snapshot.subtree_scopes(scope)?;
        let canonical_scope = asked_scope.as_ref().map(|s| s.name.as_str());
        let target_scope_names: BTreeSet<String> = target_scopes.iter().map(|s| s.name.clone()).collect();

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
            Some(self.quality_inputs(canonical_scope, false).await.map_err(|e| e.to_string()))
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
        let needs_spend = computing.iter().any(|(id, r)| r.is_ok() && matches!(id.as_str(), "unit_cost" | "tokens_per_run"));
        let spend = if needs_spend {
            Some(crate::facts::Facts::<factory_kernel::L6>::new(self).get::<factory_kernel::CostReport>(&factory_core::usage::SpendQuery {
                basis: factory_kernel::SpendBasis::Finished,
                scope: canonical_scope.map(str::to_string),
                from: Some(now - chrono::Duration::days(window.map(MetricsWindow::days).unwrap_or(OPERATIONS_WINDOW_DAYS))),
                to: Some(now), group_by: factory_core::usage::CostGroupBy::Scope,
            }).await?)
        } else { None };
        let needs_backup = computing.iter().any(|(id, r)| r.is_ok() && is_backup_metric(id.as_str()));
        let needs_attested = computing.iter().any(|(id, r)| r.is_ok() && is_attestation_metric(id.as_str()));

        let facts = Facts::<L6>::new(self);
        let production = if needs_production {
            Some(facts.get::<factory_kernel::ProductionFact>(&crate::facts::ProductionQuery {
                scope: canonical_scope.map(str::to_string), now,
                minutes: window.map(|window| (window.days() * 24 * 60) as u32).or(Some(5)),
                bin: ProductionBin::Day,
            }).await?)
        } else { None };
        let process_names: BTreeSet<String> = computing.iter().filter(|(id, result)| result.is_ok()
            && (is_operations_metric(id.as_str()) || is_usage_metric(id.as_str())
                || is_hours_metric(id.as_str()) || is_intake_metric(id.as_str())
                || id.as_str().starts_with("goal_tasks_done.")))
            .map(|(id, _)| id.to_string()).collect();
        let process = if process_names.is_empty() { BTreeMap::new() } else {
            facts.get::<factory_kernel::ProcessMetricFact>(&crate::facts::ProcessMetricsQuery {
                scope: canonical_scope.map(str::to_string), now, window, names: process_names,
            }).await?
        };
        let policy_report = if needs_policy {
            Some(self.policy_report(canonical_scope).await?)
        } else {
            None
        };
        // `#154`: never spawns `git`/`tmutil` -- `Engine::backup_fact` shares
        // `capture` with `backup_report` but not its repository or Time
        // Machine probes.
        let backup_fact = if needs_backup { Some(facts.get::<BackupFact>(&now).await?) } else { None };
        // `#158`: one read shared by `conformance_rate.<category>` (any
        // number of distinct categories a request asks for) and
        // `gate_fail_rate` (every category) -- `categories: None`, so the
        // pure figure functions do their own per-category filtering rather
        // than this fetching once per category asked for.
        let attested = if needs_attested {
            let days = window
                .map(MetricsWindow::days)
                .unwrap_or(OPERATIONS_WINDOW_DAYS);
            let attestation_window = factory_core::operations::Window::trailing(now, days);
            let scopes = canonical_scope.map(|_| &target_scope_names);
            Some(facts.get::<AttestedRun>(&AttestedQuery {
                scopes: scopes.cloned(), categories: None, window: attestation_window,
            }).await?)
        } else {
            None
        };
        let needs_environments =
            computing.iter().any(|(id, r)| r.is_ok() && environment_metric(id.as_str()).is_some());
        let environments = if needs_environments {
            Some(facts.get::<EnvironmentMetricFact>(&canonical_scope.map(str::to_string)).await?)
        } else { None };

        let sources = ComputeSources {
            spend: spend.as_ref(),
            production: production.as_ref(),
            policy_report: policy_report.as_ref(),
            process: &process,
            attested: attested.as_deref(),
            backup: backup_fact.as_ref(),
            environments: environments.as_ref(),
        };
        let mut computed: BTreeMap<MetricId, MetricValue> = BTreeMap::new();
        let mut computed_series: BTreeMap<MetricId, MetricSeries> = BTreeMap::new();
        for (id, def_result) in &computing {
            if def_result.is_err() {
                continue;
            }
            let (value, series) = self.compute_one(id, &sources, now, window, canonical_scope).await?;
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
    /// it has one, off the backing reads `sources` already gathered (each
    /// field `Some` exactly when some id needs it). `scope` is the request's
    /// own canonicalised scope name (or `None`, unscoped) -- always given,
    /// unlike `sources`' fields, since it costs nothing to pass and
    /// `cost_week` needs it to call `Engine::spend` directly.
    async fn compute_one(
        &self,
        id: &MetricId,
        sources: &ComputeSources<'_>,
        now: DateTime<Utc>,
        window: Option<MetricsWindow>,
        scope: Option<&str>,
    ) -> Result<(MetricValue, Option<MetricSeries>)> {
        let production = sources.production;
        let policy_report = sources.policy_report;
        if let Some(measurement) = sources.process.get(id.as_str()) {
            return Ok((MetricValue {
                id: id.clone(), value: measurement.value, as_of: measurement.as_of,
                reason: measurement.reason.clone(),
            }, None));
        }
        let daily = || &production.expect("needs_production set").daily;
        Ok(if id.as_str() == "throughput_week" {
            let days = window.map(MetricsWindow::days).unwrap_or(7) as usize;
            let mut s = throughput_series(daily(), days);
            let value = match window {
                Some(_) => exact_production_value(id, &production.expect("needs_production set").buckets, days, now),
                None => value_from_series(&s, now, "no finished runs recorded yet"),
            };
            align_series_end(&mut s, &value, now);
            (value, Some(s))
        } else if id.as_str() == "first_pass_yield" {
            let days = window.map(MetricsWindow::days).unwrap_or(28) as usize;
            let mut s = first_pass_yield_series(daily(), days);
            let value = match window {
                Some(_) => exact_production_value(id, &production.expect("needs_production set").buckets, days, now),
                None => ratio_value(value_from_series(&s, now, &no_recent_runs(days)), daily(), days, now),
            };
            align_series_end(&mut s, &value, now);
            (value, Some(s))
        } else if id.as_str() == "scrap_rate" {
            let days = window.map(MetricsWindow::days).unwrap_or(28) as usize;
            let mut s = scrap_rate_series(daily(), days);
            let value = match window {
                Some(_) => exact_production_value(id, &production.expect("needs_production set").buckets, days, now),
                None => ratio_value(value_from_series(&s, now, &no_recent_runs(days)), daily(), days, now),
            };
            align_series_end(&mut s, &value, now);
            (value, Some(s))
        } else if matches!(id.as_str(), "unit_cost" | "tokens_per_run") {
            let cohort = sources.spend.expect("needs_spend set").finished.as_ref().expect("finished spend query");
            let figure = if id.as_str() == "unit_cost" { &cohort.unit_cost } else { &cohort.tokens_per_run };
            (MetricValue { id: id.clone(), value: figure.value, as_of: figure.as_of.unwrap_or(now), reason: figure.reason.clone() }, None)
        } else if id.as_str() == "cost_week" {
            (self.cost_week_value(id, scope, now, window).await?, None)
        } else if is_backup_metric(id.as_str()) {
            (backup_metric_value(id, sources.backup.expect("needs_backup set")), None)
        } else if let Some((name, env)) = environment_metric(id.as_str()) {
            (environment_value(id, sources.environments.expect("needs_environments set"), name, env, now), None)
        } else if let Some(framework) = id.as_str().strip_prefix("compliance.") {
            (compliance_value(id, policy_report.expect("needs_policy set"), framework, now), None)
        } else if let Some(framework) = id.as_str().strip_prefix("open_controls.") {
            (open_controls_value(id, policy_report.expect("needs_policy set"), framework, now), None)
        } else if let Some(category) = id.as_str().strip_prefix("conformance_rate.") {
            (attestation_metric_value(id, sources.attested.expect("needs_attested set"), category, now), None)
        } else if id.as_str() == "gate_fail_rate" {
            let figure = factory_core::conformance::gate_fail_rate(sources.attested.expect("needs_attested set"));
            (figure_to_value(id, &figure, now), None)
        } else if id.as_str() == "review_reject_rate" {
            let figure = factory_core::conformance::review_reject_rate(sources.attested.expect("needs_attested set"));
            (figure_to_value(id, &figure, now), None)
        } else if let Some(dataset) = id.as_str().strip_prefix("bench.resolve_rate.") {
            (self.bench_resolve_rate_value(id, dataset, now).await?, None)
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
        let (frameworks, catalogue, characteristics, attested_categories): (
            Vec<String>,
            GoalsCatalogue,
            BTreeSet<String>,
            BTreeSet<String>,
        ) = tokio::task::spawn_blocking(move || {
            let (catalogues, _findings) = factory_core::policy::load_all(&policies_dir);
            // `#158`: every literal category (`*` skipped -- it names no
            // one category) any loaded catalogue's `requires:` names, the
            // `conformance_rate.<category>` twin of `compliance.<framework>`
            // above.
            let mut attested_categories: BTreeSet<String> = BTreeSet::new();
            for cat in &catalogues {
                for control in &cat.controls {
                    for requirement in &control.requires {
                        attested_categories.extend(
                            requirement
                                .applies_to
                                .iter()
                                .filter(|c| c.as_str() != "*")
                                .cloned(),
                        );
                    }
                }
            }
            let frameworks = catalogues.into_iter().map(|c| c.framework).collect();
            let catalogue = factory_core::goals::load(&goals_dir);
            let quality_catalogue = factory_core::quality::load(&quality_dir);
            // Every characteristic any loaded profile declares an
            // attribute under -- the `quality.<characteristic>` twin of
            // one `compliance.<framework>` per loaded catalogue.
            let characteristics = quality_catalogue
                .profiles
                .values()
                .flat_map(|p| &p.attributes)
                .map(|a| factory_core::quality::characteristic_of(&a.id).to_string())
                .collect();
            for profile in quality_catalogue.profiles.values() {
                for attribute in &profile.attributes {
                    for requirement in &attribute.requires {
                        attested_categories.extend(
                            requirement
                                .applies_to
                                .iter()
                                .filter(|c| c.as_str() != "*")
                                .cloned(),
                        );
                    }
                }
            }
            (frameworks, catalogue, characteristics, attested_categories)
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
        for category in &attested_categories {
            if let Ok(id) = MetricId::new(format!("conformance_rate.{category}")) {
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
        let measured = Facts::<L6>::new(self).get::<factory_kernel::BenchResolutionFact>(&dataset.to_string()).await?;
        let Some(run) = measured else {
            return Ok(MetricValue {
                id: id.clone(), value: None, as_of: now,
                reason: Some(format!("no settled bench run for dataset {dataset:?}")),
            });
        };
        let (pass, fail) = (run.passed, run.failed);
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

    /// `cost_week` (#164): `Engine::spend`'s own known sum over runs
    /// started in the trailing window -- `window`'s own days when given, 7
    /// otherwise, an unnormalised sum either way (`throughput_week`'s own
    /// rule for an explicit window). `Engine::spend`, not the `needs_runs`
    /// prefetch: there is exactly one path to a spend figure, and this is
    /// it. `None`, with a reason naming the known sum and the counts,
    /// whenever the window holds a run whose usage is unknown, whose cost
    /// is unknown, or whose reading is a lower bound -- any one of those
    /// makes the sum something other than the whole truth.
    async fn cost_week_value(
        &self,
        id: &MetricId,
        scope: Option<&str>,
        now: DateTime<Utc>,
        window: Option<MetricsWindow>,
    ) -> Result<MetricValue> {
        let days = window.map(MetricsWindow::days).unwrap_or(7);
        let report = crate::facts::Facts::<factory_kernel::L6>::new(self)
            .get::<factory_kernel::CostReport>(&factory_core::usage::SpendQuery {
                scope: scope.map(str::to_string),
                from: Some(now - chrono::Duration::days(days)),
                to: Some(now),
                group_by: factory_core::usage::CostGroupBy::Scope,
                ..Default::default()
            })
            .await?;
        let total = &report.total;
        let unmeasured = total.runs_unknown + total.runs_cost_unknown;
        let lower_bound = total.runs_partial;
        let unattributed = if scope.is_some() { report.unattributed_runs } else { 0 };
        if unmeasured > 0 || lower_bound > 0 || unattributed > 0 {
            let known_runs = total.runs.saturating_sub(unmeasured).saturating_sub(lower_bound);
            return Ok(MetricValue {
                id: id.clone(),
                value: None,
                as_of: now,
                reason: Some(format!(
                    "${:.2} known over {known_runs} of {} runs started in the trailing {days} days; \
                     {unmeasured} unmeasured, {lower_bound} a lower bound, {unattributed} unattributed -- see factory cost --since {days}d",
                    total.cost_usd, total.runs,
                )),
            });
        }
        Ok(MetricValue { id: id.clone(), value: Some(total.cost_usd), as_of: now, reason: None })
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

fn throughput_series(
    daily: &[factory_core::protocol::ProductionBucket],
    window: usize,
) -> MetricSeries {
    MetricSeries {
        id: MetricId::new("throughput_week").expect("fixed id"),
        points: rolling_series(daily, window, |finished, _, _, _| Some(f64::from(finished))),
    }
}

/// `first_pass / finished`, never `1 - reworked / finished` -- a bucket
/// where nothing was ever retried but nothing ever succeeded either
/// (`reworked: 0`, `first_pass: 0`) is a real `0.0`, not a manufactured
/// `1.0`. See `production.rs`'s module doc comment for `first_pass`'s own
/// definition.
fn first_pass_yield_series(
    daily: &[factory_core::protocol::ProductionBucket],
    window: usize,
) -> MetricSeries {
    MetricSeries {
        id: MetricId::new("first_pass_yield").expect("fixed id"),
        points: rolling_series(daily, window, |finished, _, _, first_pass| {
            if finished == 0 {
                None
            } else {
                Some(f64::from(first_pass) / f64::from(finished))
            }
        }),
    }
}

fn scrap_rate_series(
    daily: &[factory_core::protocol::ProductionBucket],
    window: usize,
) -> MetricSeries {
    MetricSeries {
        id: MetricId::new("scrap_rate").expect("fixed id"),
        points: rolling_series(daily, window, |finished, scrapped, _, _| {
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

/// The explicit request window is an exact timestamp interval. Production's
/// `buckets` cover that interval (including its partial first day), whereas
/// `daily` is the calendar-aligned 53-week history used for sparklines.
fn exact_production_value(
    id: &MetricId,
    buckets: &[ProductionBucket],
    window_days: usize,
    now: DateTime<Utc>,
) -> MetricValue {
    let (finished, scrapped, first_pass) =
        buckets
            .iter()
            .fold((0u32, 0u32, 0u32), |(f, s, p), bucket| {
                (
                    f + bucket.finished,
                    s + bucket.scrapped,
                    p + bucket.first_pass,
                )
            });
    if id.as_str() == "throughput_week" {
        return MetricValue {
            id: id.clone(),
            value: Some(f64::from(finished)),
            as_of: now,
            reason: None,
        };
    }
    if finished == 0 {
        return MetricValue {
            id: id.clone(),
            value: None,
            as_of: now,
            reason: Some(no_recent_runs(window_days)),
        };
    }
    let numerator = if id.as_str() == "first_pass_yield" {
        first_pass
    } else {
        scrapped
    };
    let as_of = buckets
        .iter()
        .rev()
        .find(|bucket| bucket.finished > 0)
        .map(|bucket| bucket.to)
        .unwrap_or(now);
    MetricValue {
        id: id.clone(),
        value: Some(f64::from(numerator) / f64::from(finished)),
        as_of,
        reason: None,
    }
}

fn align_series_end(series: &mut MetricSeries, value: &MetricValue, now: DateTime<Utc>) {
    let Some(value) = value.value else { return };
    match series.points.last_mut() {
        Some((day, current)) if *day == now.date_naive() => *current = value,
        _ => series.points.push((now.date_naive(), value)),
    }
}

fn no_recent_runs(window: usize) -> String {
    format!("no finished runs in the trailing {window} days")
}

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
            value.reason = Some(no_recent_runs(window));
        }
    }
    value
}

/// Every backing read a `metrics_for` call may have gathered, bundled so
/// `compute_one` takes one reference instead of one parameter per family --
/// each field `Some` exactly when some asked id needed it.
struct ComputeSources<'a> {
    spend: Option<&'a factory_kernel::CostReport>,
    production: Option<&'a factory_core::protocol::Production>,
    policy_report: Option<&'a PolicyReport>,
    process: &'a BTreeMap<String, factory_kernel::ProcessMetricFact>,
    backup: Option<&'a factory_core::backup::BackupFact>,
    environments: Option<&'a BTreeMap<String, EnvironmentMetricFact>>,
    /// `#158`: `Engine::attested_runs`'s finished runs, shared by
    /// `conformance_rate.<category>` and `gate_fail_rate`.
    attested: Option<&'a [factory_core::conformance::AttestedRun]>,
}

/// One environment metric off the card the Operations tab draws, so the
/// two can never disagree. A figure with nothing to compute it from is
/// `None` with the reason.
fn environment_value(
    id: &MetricId,
    cards: &BTreeMap<String, EnvironmentMetricFact>,
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
    match name {
        "availability" => answer(card.availability, no_samples),
        "error_budget" if !card.has_slo => answer(None, &format!("environment {env:?} declares no SLO")),
        "error_budget" => answer(card.error_budget, no_samples),
        "incidents" => answer(card.availability.map(|_| card.incidents as f64), no_samples),
        "mttr" => answer(card.mttr, "no incident ended in the window"),
        "time_to_restore_p50" => answer(card.time_to_restore_p50, "no incident ended in the window"),
        "deploy_frequency" => answer(card.deploy_frequency, "no successful deployment in the window"),
        "lead_time_p50" => answer(
            card.lead_time_p50,
            "no successful deployment in the window says when its commit was made",
        ),
        "change_failure_rate" => answer(card.change_failure_rate, "no deployment finished in the window"),
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

/// `backup_age_hours`/`backup_verified_age_days` (#154), off one
/// `Engine::backup_fact` call -- `as_of` is always `fact.at`, the instant it
/// was derived, never the moment the metric was asked for. `fact.recent`/
/// `fact.verified` being `None` is `resolve_backup_fact`'s own signal that
/// the destination could not be reached at all -- the only way either field
/// reads indeterminate rather than a plain `false`.
fn backup_metric_value(id: &MetricId, fact: &factory_core::backup::BackupFact) -> MetricValue {
    if !fact.configured {
        return unavailable_value(id, "no backup is configured", fact.at);
    }
    match id.as_str() {
        "backup_age_hours" => match (fact.recent, fact.newest) {
            (None, _) => unavailable_value(id, "the destination is not reachable", fact.at),
            (Some(_), None) => unavailable_value(id, "no snapshot yet", fact.at),
            (Some(_), Some(newest)) => MetricValue {
                id: id.clone(),
                value: Some(fact.at.signed_duration_since(newest).num_seconds() as f64 / 3600.0),
                as_of: fact.at,
                reason: None,
            },
        },
        "backup_verified_age_days" => {
            if fact.verified.is_none() {
                return unavailable_value(id, "the destination is not reachable", fact.at);
            }
            if fact.newest.is_none() {
                return unavailable_value(id, "no snapshot yet", fact.at);
            }
            match &fact.last_verified {
                None => unavailable_value(id, "no snapshot in the destination has been verified", fact.at),
                Some(v) if !v.ok => unavailable_value(
                    id,
                    &format!("the newest verification failed at {}", v.at.format("%Y-%m-%d %H:%M UTC")),
                    fact.at,
                ),
                Some(v) => MetricValue {
                    id: id.clone(),
                    value: Some(fact.at.signed_duration_since(v.at).num_seconds() as f64 / 86400.0),
                    as_of: fact.at,
                    reason: None,
                },
            }
        }
        // `metrics::resolve` names only the two ids above for this family;
        // an id it never returns cannot reach here (`compute_one`'s own
        // catch-all doc comment).
        _ => unavailable_value(id, "no computation wired for this metric yet", fact.at),
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
/// `characteristic`, counted once per scope it applies in within the selected
/// subtree, the share that is `met`. A draft or `no_data` scenario counts in
/// the denominator -- declared but not shown to be met is not met, the same
/// "never green without evidence" rule the tab itself keeps. `None`, with the
/// reason, for a characteristic ISO 25010 does not name or one no selected
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

/// `#158`: a `factory_core::conformance::ConformanceFigure` as a
/// `MetricValue` -- `as_of` falls back to `now` only when the figure itself
/// has none, the same rule `intake_value` and the operations metrics keep.
fn figure_to_value(
    id: &MetricId,
    figure: &factory_core::conformance::ConformanceFigure,
    now: DateTime<Utc>,
) -> MetricValue {
    MetricValue {
        id: id.clone(),
        value: figure.value,
        as_of: figure.as_of.unwrap_or(now),
        reason: figure.reason.clone(),
    }
}

fn attestation_metric_value(
    id: &MetricId,
    runs: &[factory_core::conformance::AttestedRun],
    category: &str,
    now: DateTime<Utc>,
) -> MetricValue {
    figure_to_value(
        id,
        &factory_core::conformance::conformance_rate(runs, category),
        now,
    )
}

#[cfg(test)]
mod tests {
    //! Engine-level computation, on a temporary instance -- not
    //! `factory_core::metrics` itself (covered on its own), but this module
    //! glued to real data the way a real request sees it: real runs and
    //! tasks in the store, and a real policy catalogue on disk.

    use super::*;
    use crate::access::Caller;
    use crate::intake::TRIAGE_VERDICT_KIND;
    use factory_core::adapter::store::task_from_new;
    use factory_core::adapter::TaskStore;
    use factory_core::config::{Config, DaemonConfig, Factory, Instance, PolicyDeclaration, Scope};
    use factory_core::run::{NewRun, Run, RunPatch, RunStatus, Trigger};
    use factory_core::task::{NewTask, TaskEntry, TaskPatch, TaskStatus};
    use factory_core::usage::{RunUsage, TokenCounts, UsageState};
    use factory_plugins::{Registry, SqliteStore};
    use rusqlite::params;
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
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let factory = Factory { root, config };
        let registry = Registry::with_builtins();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        Arc::new(Engine::new(factory, registry, store, PathBuf::from("factory"), Vec::new()))
    }

    /// A path-shaped scope tree whose names deliberately do not share
    /// prefixes: `work` owns `nested`; `side` is a sibling. Policy is n/a in
    /// the work subtree but open at the sibling, and the quality profile's
    /// sandbox scenario is met in work/nested but not at side.
    fn scoped_engine() -> (Arc<Engine>, PathBuf) {
        let root = std::env::temp_dir().join(format!("factory-scoped-metrics-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".factory/policies")).unwrap();
        std::fs::create_dir_all(root.join(".factory/quality")).unwrap();
        std::fs::write(
            root.join(".factory/policies/cra.yaml"),
            "framework: cra\ntitle: CRA\nkind: regulation\ncontrols:\n  - id: a\n    title: A\n    evidence:\n      - check: attestation\n",
        )
        .unwrap();
        std::fs::write(
            root.join(".factory/quality/baseline.yaml"),
            "attributes:\n  - id: reliability\n    importance: H\n    difficulty: M\n    scenarios:\n      - { id: sandboxed, measure: { check: sandbox } }\n",
        )
        .unwrap();

        let mut root_scope: Scope = serde_yaml_ng::from_str("id: root-id\nname: company\n").unwrap();
        root_scope.path = PathBuf::from(".");
        let mut work: Scope = serde_yaml_ng::from_str(
            "id: work-id\nname: work\nagents:\n  - { name: worker, harness: shell, sandbox: docker }\npolicies:\n  not_applicable:\n    - { control: cra/a, rationale: test }\n",
        )
        .unwrap();
        work.path = PathBuf::from("projects/work");
        let mut nested: Scope = serde_yaml_ng::from_str(
            "id: nested-id\nname: nested\nagents:\n  - { name: worker, harness: shell, sandbox: docker }\n",
        )
        .unwrap();
        nested.path = PathBuf::from("projects/work/nested");
        let mut side: Scope = serde_yaml_ng::from_str(
            "id: side-id\nname: side\nagents:\n  - { name: worker, harness: shell, sandbox: none }\n",
        )
        .unwrap();
        side.path = PathBuf::from("projects/side");

        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig::default(),
            scope: Some(root_scope.clone()),
            scopes: vec![root_scope, work, nested, side],
            roles: Default::default(),
            policies: PolicyDeclaration { frameworks: vec!["cra".into()], ..Default::default() },
            quality: vec!["baseline".into()],
            infrastructure: Default::default(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
            dashboard: None,
        };
        let database = root.join(".factory/metrics.sqlite");
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open(&database).unwrap());
        let engine = Arc::new(Engine::new(
            Factory { root, config },
            Registry::with_builtins(),
            store,
            PathBuf::from("factory"),
            Vec::new(),
        ));
        (engine, database)
    }

    fn measured(usd: f64, tokens: u64) -> RunUsage {
        RunUsage {
            state: UsageState::Known,
            reason: None,
            tokens: TokenCounts { input: Some(tokens), output: Some(0), cache_read: Some(0), cache_write: Some(0) },
            cost_usd: Some(usd),
            ..RunUsage::unknown("", 2)
        }
    }

    /// The sqlite adapter owns run timestamps, so a fixture that needs exact
    /// historical boundaries rewrites its just-created record as one atomic
    /// row update, including the JSON source of truth.
    fn store_run_at(database: &std::path::Path, run: &Run) {
        let connection = rusqlite::Connection::open(database).unwrap();
        let data = serde_json::to_string(run).unwrap();
        connection
            .execute(
                "UPDATE runs SET status = ?2, started_at = ?3, ended_at = ?4, data = ?5 WHERE id = ?1",
                params![run.id, run.status.as_str(), run.started_at.to_rfc3339(), run.ended_at.map(|at| at.to_rfc3339()), data],
            )
            .unwrap();
    }

    async fn timed_run(
        engine: &Arc<Engine>,
        database: &std::path::Path,
        title: &str,
        scope: &str,
        status: RunStatus,
        started_at: DateTime<Utc>,
        ended_at: Option<DateTime<Utc>>,
        usage: Option<RunUsage>,
    ) -> (factory_core::task::Task, Run) {
        let task = engine
            .store
            .create(&task_from_new(
                NewTask { title: title.into(), ..Default::default() },
                scope.into(),
                "worker".into(),
                "shell".into(),
            ))
            .await
            .unwrap();
        let mut run = engine
            .store
            .create_run(&NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Manual,
                agent: "worker".into(),
                adapter: "shell".into(),
                runtime: "shell".into(),
                token: "tok".into(),
                queued_at: Some(started_at),
                scheduled_for: None,
            })
            .await
            .unwrap();
        run.status = status;
        run.started_at = started_at;
        run.ended_at = ended_at;
        run.usage = usage;
        store_run_at(database, &run);
        (task, run)
    }

    async fn transition(engine: &Arc<Engine>, task_id: &str, run_id: &str, kind: &str, at: DateTime<Utc>) {
        let mut entry = TaskEntry::new("agent", kind, kind).in_run(run_id);
        entry.at = at;
        engine.store.append_entry(task_id, &entry).await.unwrap();
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

    // ------------------------------------------------------- scope/window

    fn metric<'a>(computed: &'a Metrics, id: &str) -> &'a MetricValue {
        computed
            .values
            .iter()
            .find(|value| value.id.as_str() == id)
            .unwrap()
    }

    #[tokio::test]
    async fn a_scope_includes_descendants_and_excludes_siblings_across_metric_families() {
        let (engine, database) = scoped_engine();
        let now = Utc::now();
        let (parent_task, parent_run) = timed_run(
            &engine,
            &database,
            "parent done",
            "work",
            RunStatus::Done,
            now - chrono::Duration::hours(3),
            Some(now - chrono::Duration::hours(2)),
            Some(measured(2.0, 200)),
        )
        .await;
        transition(
            &engine,
            &parent_task.id,
            &parent_run.id,
            "blocked",
            now - chrono::Duration::minutes(165),
        )
        .await;
        transition(
            &engine,
            &parent_task.id,
            &parent_run.id,
            "unblocked",
            now - chrono::Duration::minutes(150),
        )
        .await;
        let (child_task, child_run) = timed_run(
            &engine,
            &database,
            "child done",
            "nested",
            RunStatus::Done,
            now - chrono::Duration::hours(2),
            Some(now - chrono::Duration::hours(1)),
            Some(measured(4.0, 400)),
        )
        .await;
        transition(
            &engine,
            &child_task.id,
            &child_run.id,
            "blocked",
            now - chrono::Duration::minutes(105),
        )
        .await;
        transition(
            &engine,
            &child_task.id,
            &child_run.id,
            "unblocked",
            now - chrono::Duration::minutes(75),
        )
        .await;
        let (sibling_task, sibling_run) = timed_run(
            &engine,
            &database,
            "sibling failure",
            "side",
            RunStatus::Failed,
            now - chrono::Duration::hours(2),
            Some(now - chrono::Duration::minutes(30)),
            Some(measured(90.0, 9_000)),
        )
        .await;
        transition(
            &engine,
            &sibling_task.id,
            &sibling_run.id,
            "blocked",
            now - chrono::Duration::minutes(90),
        )
        .await;
        transition(
            &engine,
            &sibling_task.id,
            &sibling_run.id,
            "unblocked",
            now - chrono::Duration::minutes(30),
        )
        .await;

        let ids: Vec<MetricId> = [
            "throughput_week",
            "fail_rate",
            "unit_cost",
            "tokens_per_run",
            "compliance.cra",
            "open_controls.cra",
            "quality.reliability",
            "agent_hours",
            "blocked_hours",
        ]
        .into_iter()
        .map(|id| MetricId::new(id).unwrap())
        .collect();
        let scoped = engine
            .metrics_for(&ids, now, Some("work"), Some(MetricsWindow::Day))
            .await
            .unwrap();

        assert_eq!(metric(&scoped, "throughput_week").value, Some(2.0));
        assert_eq!(metric(&scoped, "fail_rate").value, Some(0.0));
        assert_eq!(metric(&scoped, "unit_cost").value, Some(3.0));
        assert_eq!(metric(&scoped, "tokens_per_run").value, Some(300.0));
        assert_eq!(metric(&scoped, "compliance.cra").value, Some(1.0));
        assert_eq!(metric(&scoped, "open_controls.cra").value, Some(0.0));
        assert_eq!(metric(&scoped, "quality.reliability").value, Some(1.0));
        assert_eq!(metric(&scoped, "agent_hours").value, Some(2.0));
        assert_eq!(metric(&scoped, "blocked_hours").value, Some(0.75));

        let occupancy = engine
            .occupancy(None, Some(now - chrono::Duration::days(1)), Some(now))
            .await
            .unwrap();
        let subtree_rows = occupancy
            .scopes
            .iter()
            .filter(|scope| matches!(scope.name.as_str(), "work" | "nested"))
            .flat_map(|scope| &scope.rows);
        let (subtree_busy, subtree_blocked) = subtree_rows
            .fold((0, 0), |(busy, blocked), row| {
                (busy + row.busy_seconds, blocked + row.blocked_seconds)
            });
        let side = occupancy
            .scopes
            .iter()
            .find(|scope| scope.name == "side")
            .unwrap();
        assert_eq!(
            metric(&scoped, "agent_hours").value,
            Some(subtree_busy as f64 / 3600.0),
            "scope hours match the work subtree's occupancy rows"
        );
        assert_eq!(
            metric(&scoped, "blocked_hours").value,
            Some(subtree_blocked as f64 / 3600.0),
            "blocked hours match the work subtree's occupancy rows"
        );
        assert_eq!(
            side.rows.iter().map(|row| row.busy_seconds).sum::<i64>() as f64 / 3600.0,
            1.5,
            "the excluded sibling has distinct busy time"
        );
        assert_eq!(
            side.rows.iter().map(|row| row.blocked_seconds).sum::<i64>() as f64 / 3600.0,
            1.0,
            "the excluded sibling has distinct blocked time"
        );

        let all = engine
            .metrics_for(&ids, now, None, Some(MetricsWindow::Day))
            .await
            .unwrap();
        assert_eq!(metric(&all, "throughput_week").value, Some(3.0));
        assert_eq!(metric(&all, "fail_rate").value, Some(1.0 / 3.0));
        assert_eq!(
            metric(&all, "unit_cost").value,
            Some(48.0),
            "all cost divided by the two done units"
        );
        assert!(metric(&all, "compliance.cra").value.unwrap() < 1.0);
        assert_eq!(metric(&all, "open_controls.cra").value, Some(1.0));
        assert!(metric(&all, "quality.reliability").value.unwrap() < 1.0);
        assert_eq!(metric(&all, "agent_hours").value, Some(3.5));
        assert_eq!(metric(&all, "blocked_hours").value, Some(1.75));
    }

    #[tokio::test]
    async fn explicit_windows_override_run_backed_metrics_and_omission_keeps_legacy_windows() {
        let (engine, database) = scoped_engine();
        let now = Utc::now();
        for (name, age, status, usd) in [
            ("recent", chrono::Duration::hours(12), RunStatus::Done, 1.0),
            (
                "eight days",
                chrono::Duration::days(8),
                RunStatus::Failed,
                2.0,
            ),
            (
                "thirty days",
                chrono::Duration::days(30),
                RunStatus::Failed,
                3.0,
            ),
            (
                "hundred days",
                chrono::Duration::days(100),
                RunStatus::Done,
                4.0,
            ),
        ] {
            let end = now - age;
            timed_run(
                &engine,
                &database,
                name,
                "work",
                status,
                end - chrono::Duration::hours(1),
                Some(end),
                Some(measured(usd, (usd * 100.0) as u64)),
            )
            .await;
        }

        let ids: Vec<MetricId> = [
            "throughput_week",
            "first_pass_yield",
            "fail_rate",
            "unit_cost",
        ]
        .into_iter()
        .map(|id| MetricId::new(id).unwrap())
        .collect();
        for (window, finished, failures, cost) in [
            (MetricsWindow::Day, 1.0, 0.0, 1.0),
            (MetricsWindow::FourteenDays, 2.0, 0.5, 3.0),
            (MetricsWindow::NinetyDays, 3.0, 2.0 / 3.0, 6.0),
        ] {
            let computed = engine
                .metrics_for(&ids, now, Some("work"), Some(window))
                .await
                .unwrap();
            assert_eq!(
                metric(&computed, "throughput_week").value,
                Some(finished),
                "{window}"
            );
            assert_eq!(
                metric(&computed, "fail_rate").value,
                Some(failures),
                "{window}"
            );
            assert_eq!(metric(&computed, "unit_cost").value, Some(cost), "{window}");
        }

        let legacy = engine
            .metrics_for(&ids, now, Some("work"), None)
            .await
            .unwrap();
        assert_eq!(
            metric(&legacy, "throughput_week").value,
            Some(1.0),
            "legacy throughput is seven days"
        );
        assert_eq!(
            metric(&legacy, "first_pass_yield").value,
            Some(0.5),
            "legacy production ratio is 28 days"
        );
        assert_eq!(
            metric(&legacy, "fail_rate").value,
            Some(0.5),
            "legacy operations is 28 days"
        );
        assert_eq!(
            metric(&legacy, "unit_cost").value,
            Some(3.0),
            "legacy usage is 28 days"
        );
    }

    #[tokio::test]
    async fn explicit_window_uses_one_half_open_cutoff_across_run_backed_families() {
        let (engine, database) = scoped_engine();
        // Keep the request clock deliberately distinct from the wall clock:
        // every family must use this one captured bound, not sample its own.
        let now = chrono::SubsecRound::trunc_subsecs(Utc::now(), 0)
            + chrono::Duration::hours(2);
        let cutoff = now - chrono::Duration::days(1);
        let (cutoff_task, cutoff_run) = timed_run(
            &engine,
            &database,
            "exactly at cutoff",
            "work",
            RunStatus::Failed,
            cutoff - chrono::Duration::hours(1),
            Some(cutoff),
            Some(measured(99.0, 9_900)),
        )
        .await;
        transition(
            &engine,
            &cutoff_task.id,
            &cutoff_run.id,
            "blocked",
            cutoff - chrono::Duration::hours(1),
        )
        .await;
        transition(&engine, &cutoff_task.id, &cutoff_run.id, "unblocked", cutoff).await;

        let (inside_task, inside_run) = timed_run(
            &engine,
            &database,
            "inside cutoff",
            "work",
            RunStatus::Done,
            cutoff,
            Some(cutoff + chrono::Duration::hours(1)),
            Some(measured(1.0, 100)),
        )
        .await;
        transition(&engine, &inside_task.id, &inside_run.id, "blocked", cutoff).await;
        transition(
            &engine,
            &inside_task.id,
            &inside_run.id,
            "unblocked",
            cutoff + chrono::Duration::hours(1),
        )
        .await;

        let ids: Vec<MetricId> = [
            "throughput_week",
            "fail_rate",
            "unit_cost",
            "agent_hours",
            "blocked_hours",
        ]
            .into_iter()
            .map(|id| MetricId::new(id).unwrap())
            .collect();
        let computed = engine
            .metrics_for(&ids, now, Some("work"), Some(MetricsWindow::Day))
            .await
            .unwrap();
        assert_eq!(metric(&computed, "throughput_week").value, Some(1.0));
        assert_eq!(metric(&computed, "fail_rate").value, Some(0.0));
        assert_eq!(metric(&computed, "unit_cost").value, Some(1.0));
        assert_eq!(metric(&computed, "agent_hours").value, Some(1.0));
        assert_eq!(metric(&computed, "blocked_hours").value, Some(1.0));
    }

    #[tokio::test]
    async fn hour_metrics_match_occupancy_unions_and_keep_blocked_time_inside_total_time() {
        let (engine, database) = scoped_engine();
        let now = chrono::SubsecRound::trunc_subsecs(Utc::now(), 0);
        let (first_task, first) = timed_run(
            &engine,
            &database,
            "overlap a",
            "work",
            RunStatus::Done,
            now - chrono::Duration::hours(4),
            Some(now - chrono::Duration::hours(1)),
            None,
        )
        .await;
        let (second_task, second) = timed_run(
            &engine,
            &database,
            "overlap b",
            "work",
            RunStatus::Done,
            now - chrono::Duration::hours(3),
            Some(now - chrono::Duration::hours(2)),
            None,
        )
        .await;
        transition(
            &engine,
            &first_task.id,
            &first.id,
            "blocked",
            now - chrono::Duration::minutes(210),
        )
        .await;
        transition(
            &engine,
            &first_task.id,
            &first.id,
            "unblocked",
            now - chrono::Duration::minutes(150),
        )
        .await;
        transition(
            &engine,
            &second_task.id,
            &second.id,
            "blocked",
            now - chrono::Duration::hours(3),
        )
        .await;
        transition(
            &engine,
            &second_task.id,
            &second.id,
            "unblocked",
            now - chrono::Duration::hours(2),
        )
        .await;
        timed_run(
            &engine,
            &database,
            "sibling work",
            "side",
            RunStatus::Done,
            now - chrono::Duration::hours(10),
            Some(now - chrono::Duration::hours(1)),
            None,
        )
        .await;

        let ids = [
            MetricId::new("agent_hours").unwrap(),
            MetricId::new("blocked_hours").unwrap(),
        ];
        let computed = engine
            .metrics_for(&ids, now, Some("work"), Some(MetricsWindow::Day))
            .await
            .unwrap();
        assert_eq!(
            metric(&computed, "agent_hours").value,
            Some(3.0),
            "overlapping blocks are a three-hour union"
        );
        assert_eq!(
            metric(&computed, "blocked_hours").value,
            Some(1.5),
            "overlapping blocked segments are a 90-minute union"
        );
        assert!(
            metric(&computed, "agent_hours").value.unwrap()
                > metric(&computed, "blocked_hours").value.unwrap(),
            "blocked time remains part of total time"
        );

        let occupancy = engine
            .occupancy(None, Some(now - chrono::Duration::days(1)), Some(now))
            .await
            .unwrap();
        let work = occupancy
            .scopes
            .iter()
            .find(|scope| scope.name == "work")
            .unwrap();
        assert_eq!(
            work.rows.iter().map(|row| row.busy_seconds).sum::<i64>() as f64 / 3600.0,
            3.0
        );
        assert_eq!(
            work.rows.iter().map(|row| row.blocked_seconds).sum::<i64>() as f64 / 3600.0,
            1.5
        );
    }

    #[tokio::test]
    async fn hour_metrics_follow_every_window_preset_and_default_to_fourteen_days() {
        let (engine, database) = scoped_engine();
        let now = chrono::SubsecRound::trunc_subsecs(Utc::now(), 0);
        let day = now - chrono::Duration::days(1);
        let fourteen_days = now - chrono::Duration::days(14);
        let ninety_days = now - chrono::Duration::days(90);

        // Each cutoff has one run ending exactly at it and one starting
        // exactly at it. The former contributes nothing to that window; the
        // latter contributes its whole block and blocked segment.
        for (title, start, end, blocked_from, blocked_to) in [
            (
                "inside day",
                day,
                day + chrono::Duration::hours(2),
                day + chrono::Duration::minutes(30),
                day + chrono::Duration::minutes(90),
            ),
            (
                "before day",
                day - chrono::Duration::hours(2),
                day,
                day - chrono::Duration::minutes(90),
                day - chrono::Duration::minutes(30),
            ),
            (
                "inside fourteen days",
                fourteen_days,
                fourteen_days + chrono::Duration::hours(4),
                fourteen_days + chrono::Duration::hours(1),
                fourteen_days + chrono::Duration::hours(3),
            ),
            (
                "before fourteen days",
                fourteen_days - chrono::Duration::hours(4),
                fourteen_days,
                fourteen_days - chrono::Duration::hours(3),
                fourteen_days - chrono::Duration::hours(1),
            ),
            (
                "inside ninety days",
                ninety_days,
                ninety_days + chrono::Duration::hours(6),
                ninety_days + chrono::Duration::hours(1),
                ninety_days + chrono::Duration::hours(4),
            ),
            (
                "before ninety days",
                ninety_days - chrono::Duration::hours(6),
                ninety_days,
                ninety_days - chrono::Duration::hours(5),
                ninety_days - chrono::Duration::hours(2),
            ),
        ] {
            let (task, run) = timed_run(
                &engine,
                &database,
                title,
                "work",
                RunStatus::Done,
                start,
                Some(end),
                None,
            )
            .await;
            transition(&engine, &task.id, &run.id, "blocked", blocked_from).await;
            transition(&engine, &task.id, &run.id, "unblocked", blocked_to).await;
        }

        let ids = [
            MetricId::new("agent_hours").unwrap(),
            MetricId::new("blocked_hours").unwrap(),
        ];
        for (window, days, expected_busy, expected_blocked) in [
            (Some(MetricsWindow::Day), 1, 2.0, 1.0),
            (Some(MetricsWindow::FourteenDays), 14, 8.0, 4.0),
            (Some(MetricsWindow::NinetyDays), 90, 18.0, 9.0),
            (None, 14, 8.0, 4.0),
        ] {
            let label = window.map_or_else(|| "omitted".to_string(), |value| value.to_string());
            let computed = engine
                .metrics_for(&ids, now, Some("work"), window)
                .await
                .unwrap();
            assert_eq!(
                metric(&computed, "agent_hours").value,
                Some(expected_busy),
                "{label} agent hours"
            );
            assert_eq!(
                metric(&computed, "blocked_hours").value,
                Some(expected_blocked),
                "{label} blocked hours"
            );

            let occupancy = engine
                .occupancy(None, Some(now - chrono::Duration::days(days)), Some(now))
                .await
                .unwrap();
            let work = occupancy
                .scopes
                .iter()
                .find(|scope| scope.name == "work")
                .unwrap();
            assert_eq!(
                work.rows.iter().map(|row| row.busy_seconds).sum::<i64>() as f64 / 3600.0,
                expected_busy,
                "{label} occupancy busy hours"
            );
            assert_eq!(
                work.rows.iter().map(|row| row.blocked_seconds).sum::<i64>() as f64 / 3600.0,
                expected_blocked,
                "{label} occupancy blocked hours"
            );
        }
    }

    #[tokio::test]
    async fn scoped_requests_validate_scope_but_keep_goal_and_bench_metrics_instance_wide() {
        let (engine, _database) = scoped_engine();
        let mut labelled = NewTask {
            title: "sibling goal".into(),
            ..Default::default()
        };
        labelled.labels.insert("goal".into(), "ship/kr".into());
        let task = engine
            .store
            .create(&task_from_new(
                labelled,
                "side".into(),
                "worker".into(),
                "shell".into(),
            ))
            .await
            .unwrap();
        engine
            .store
            .update(
                &task.id,
                &TaskPatch {
                    status: Some(TaskStatus::Done),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let ids = [
            MetricId::new("goal_tasks_done.ship.kr").unwrap(),
            MetricId::new("bench.resolve_rate.missing").unwrap(),
        ];
        let computed = engine
            .metrics_for(&ids, Utc::now(), Some("work"), None)
            .await
            .unwrap();
        assert_eq!(
            metric(&computed, "goal_tasks_done.ship.kr").value,
            Some(1.0)
        );
        assert!(computed
            .registry
            .iter()
            .all(|def| def.coverage == metrics::MetricCoverage::InstanceWide));

        let error = engine
            .metrics_for(&ids, Utc::now(), Some("unknown"), None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("unknown"), "{error}");
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
    async fn process_fact_ports_read_live_changes_and_do_not_make_unknown_figures_zero() {
        let engine = test_engine(Vec::new());
        let name = "goal_tasks_done.live.kr";
        let query = crate::facts::ProcessMetricsQuery {
            scope: Some("root".into()), now: Utc::now(), window: None,
            names: BTreeSet::from([name.into(), "fail_rate".into(), "ready_rate".into()]),
        };
        let reader = Facts::<L6>::new(&engine);
        let before = reader.get::<factory_kernel::ProcessMetricFact>(&query).await.unwrap();
        assert_eq!(before[name].value, Some(0.0));
        assert_eq!(before["fail_rate"].value, None);
        assert!(before["fail_rate"].reason.is_some());
        assert_eq!(before["ready_rate"].value, None);
        let mut new = NewTask { title: "live evidence".into(), ..Default::default() };
        new.labels.insert("goal".into(), "live/kr".into());
        let task = engine.store.create(&task_from_new(new, "root".into(), "shell".into(), "herdr".into())).await.unwrap();
        engine.store.update(&task.id, &TaskPatch { status: Some(TaskStatus::Done), ..Default::default() }).await.unwrap();
        let after = reader.get::<factory_kernel::ProcessMetricFact>(&query).await.unwrap();
        assert_eq!(after[name].value, Some(1.0));
        assert_eq!(after[name].as_of, query.now);
        assert_eq!(after["fail_rate"].value, None, "a task without a run does not invent a completed run");
        let unknown = crate::facts::ProcessMetricsQuery {
            names: BTreeSet::from(["compliance.cra".into()]), ..query
        };
        assert!(reader.get::<factory_kernel::ProcessMetricFact>(&unknown).await.is_err(),
            "L4 cannot produce a policy measurement");
    }

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

    // ----------------------------------------------------------- cost_week (#164)

    #[tokio::test]
    async fn scenario_cost_baselines_share_the_finished_spend_port_and_scope_not_started_cohorts() {
        let (engine, database) = scoped_engine();
        let now = Utc::now();
        let from = now - chrono::Duration::days(28);
        for (title, scope, status, start, end, usd, tokens) in [
            ("long measured", "work", RunStatus::Done, now - chrono::Duration::days(40), Some(now - chrono::Duration::hours(2)), 2.0, 1000),
            ("failed nested", "nested", RunStatus::Failed, now - chrono::Duration::hours(3), Some(now - chrono::Duration::hours(1)), 1.0, 3000),
            ("outside", "side", RunStatus::Done, now - chrono::Duration::hours(3), Some(now - chrono::Duration::hours(1)), 100.0, 90000),
            ("boundary excluded", "work", RunStatus::Done, from - chrono::Duration::hours(1), Some(from), 50.0, 50000),
            ("still running", "work", RunStatus::Running, now - chrono::Duration::hours(1), None, 20.0, 20000),
        ] {
            timed_run(&engine, &database, title, scope, status, start, end, Some(measured(usd, tokens))).await;
        }
        let port = crate::facts::Facts::<factory_kernel::L6>::new(&engine).get::<factory_core::usage::CostReport>(&factory_core::usage::SpendQuery {
            basis: factory_kernel::SpendBasis::Finished, scope: Some("work".into()), from: Some(from), to: Some(now), ..Default::default()
        }).await.unwrap();
        let facts = port.finished.unwrap();
        assert_eq!(port.total.runs, 2);
        assert_eq!(facts.unit_cost.value, Some(3.0));
        assert_eq!(facts.tokens_per_run.value, Some(2000.0));
        assert!(facts.unit_cost.reason.is_none());
        let ids = [MetricId::new("unit_cost").unwrap(), MetricId::new("tokens_per_run").unwrap()];
        let metrics = engine.metrics_for(&ids, now, Some("work"), None).await.unwrap();
        assert_eq!(metric(&metrics, "unit_cost").value, facts.unit_cost.value);
        assert_eq!(metric(&metrics, "tokens_per_run").value, facts.tokens_per_run.value);
        let report = engine.scenarios_report(Some("work")).await.unwrap();
        assert_eq!(report.baseline.drivers["unit_cost"], 3.0);
        let result = engine.scenario_whatif(None, BTreeMap::from([("throughput_week".into(), "=10".into()), ("first_pass_yield".into(), "=1".into())]), Some("work")).await.unwrap();
        assert_eq!(result.drivers.outcomes_after["weekly_cost"], 30.0);
        assert_eq!(result.drivers.outcomes_after["weekly_tokens"], 20000.0);
        assert!(result.drivers.tornados["weekly_cost"].iter().find(|bar| bar.driver == "unit_cost").unwrap().span > 0.0);
        timed_run(&engine, &database, "unknown nested", "nested", RunStatus::Done, now - chrono::Duration::hours(2), Some(now - chrono::Duration::minutes(30)), None).await;
        let result = engine.scenario_whatif(None, BTreeMap::from([("unit_cost".into(), "=0".into())]), Some("work")).await.unwrap();
        assert!(!result.drivers.outcomes_after.contains_key("weekly_cost"));
        assert!(result.drivers.outcome_reasons_after["weekly_cost"].contains("unmeasured"));
    }

    #[tokio::test]
    async fn budget_policy_uses_full_ancestor_spend_and_preserves_unknown_in_report_and_detail() {
        let (engine, database) = scoped_engine();
        let snapshot = engine.factory_snapshot();
        std::fs::create_dir_all(snapshot.root.join(".factory/budgets")).unwrap();
        let catalogue = snapshot.root.join(".factory/budgets/limits.yaml");
        std::fs::write(&catalogue, "version: 1\nscopes:\n  root-id: {monthly_usd: 5}\n  work-id: {monthly_usd: 50}\n").unwrap();
        std::fs::write(snapshot.root.join(".factory/policies/cra.yaml"), "framework: cra\ntitle: CRA\nkind: regulation\ncontrols:\n  - id: budget\n    title: Budget\n    evidence:\n      - check: budget_within\n").unwrap();
        let now = Utc::now();
        for (scope, usd) in [("work", 2.0), ("side", 10.0)] {
            timed_run(&engine, &database, scope, scope, RunStatus::Done, now - chrono::Duration::hours(1), Some(now - chrono::Duration::minutes(30)), Some(measured(usd, 100))).await;
        }
        let detail = engine.policy_control("cra/budget".parse().unwrap(), "work").await.unwrap();
        assert!(serde_json::to_string(&detail).unwrap().contains("budget_within: over"));
        std::fs::write(&catalogue, "version: 1\nscopes:\n  root-id: {monthly_usd: 50}\n  work-id: {monthly_usd: 50}\n").unwrap();
        let report = engine.policy_report(Some("work")).await.unwrap();
        assert!(serde_json::to_string(&report).unwrap().contains("satisfied"));
        timed_run(&engine, &database, "unknown", "nested", RunStatus::Done, now - chrono::Duration::minutes(20), Some(now - chrono::Duration::minutes(10)), None).await;
        let report = engine.policy_report(Some("work")).await.unwrap();
        assert!(serde_json::to_string(&report).unwrap().contains("budget_within: unknown"));
        let detail = engine.policy_control("cra/budget".parse().unwrap(), "work").await.unwrap();
        assert!(serde_json::to_string(&detail).unwrap().contains("budget_within: unknown"));
        std::fs::write(&catalogue, "version: 1\nscopes: {work-id: {monthly_usd: -1}}\n").unwrap();
        assert!(serde_json::to_string(&engine.policy_report(Some("work")).await.unwrap()).unwrap().contains("budget_within: unknown"));
    }

    #[tokio::test]
    async fn cost_week_is_none_with_the_known_sum_and_counts_until_every_run_is_measured() {
        let (engine, database) = scoped_engine();
        let now = Utc::now();
        let ids = [MetricId::new("cost_week").unwrap()];

        timed_run(
            &engine,
            &database,
            "costed",
            "work",
            RunStatus::Done,
            now - chrono::Duration::hours(3),
            Some(now - chrono::Duration::hours(2)),
            Some(measured(2.0, 200)),
        )
        .await;
        let computed = engine.metrics(&ids, now).await.unwrap();
        assert_eq!(metric(&computed, "cost_week").value, Some(2.0), "the only run in the window, fully measured");

        // An unknown run joins the window: the value drops out, with a
        // reason naming the known sum and the counts.
        timed_run(
            &engine,
            &database,
            "unmeasured",
            "work",
            RunStatus::Done,
            now - chrono::Duration::hours(2),
            Some(now - chrono::Duration::hours(1)),
            Some(RunUsage::unknown("no source for usage", 1)),
        )
        .await;
        let computed = engine.metrics(&ids, now).await.unwrap();
        let value = metric(&computed, "cost_week");
        assert_eq!(value.value, None);
        let reason = value.reason.as_deref().unwrap();
        assert!(reason.contains("$2.00"), "{reason}");
        assert!(reason.contains("1 of 2"), "{reason}");
        assert!(reason.contains("1 unmeasured"), "{reason}");
        assert!(reason.contains("0 a lower bound"), "{reason}");
        assert!(reason.contains("factory cost --since 7d"), "{reason}");

        // A partial run (a lower bound) joins too: still no value, and its
        // own count says so.
        let mut lower_bound = measured(0.5, 50);
        lower_bound.partial = true;
        timed_run(
            &engine,
            &database,
            "partial",
            "work",
            RunStatus::Done,
            now - chrono::Duration::hours(1),
            Some(now - chrono::Duration::minutes(30)),
            Some(lower_bound),
        )
        .await;
        let computed = engine.metrics(&ids, now).await.unwrap();
        let value = metric(&computed, "cost_week");
        assert_eq!(value.value, None);
        assert!(value.reason.as_deref().unwrap().contains("1 a lower bound"), "{value:?}");
    }

    #[tokio::test]
    async fn cost_week_follows_the_scope_and_window_the_request_asks_for() {
        let (engine, database) = scoped_engine();
        let now = Utc::now();
        let ids = [MetricId::new("cost_week").unwrap()];

        timed_run(
            &engine,
            &database,
            "work run",
            "work",
            RunStatus::Done,
            now - chrono::Duration::hours(2),
            Some(now - chrono::Duration::hours(1)),
            Some(measured(2.0, 200)),
        )
        .await;
        timed_run(
            &engine,
            &database,
            "side run",
            "side",
            RunStatus::Done,
            now - chrono::Duration::hours(2),
            Some(now - chrono::Duration::hours(1)),
            Some(measured(90.0, 9_000)),
        )
        .await;
        // Outside the default 7-day window, inside a 14-day one.
        timed_run(
            &engine,
            &database,
            "nine days ago",
            "work",
            RunStatus::Done,
            now - chrono::Duration::days(9),
            Some(now - chrono::Duration::days(9) + chrono::Duration::hours(1)),
            Some(measured(4.0, 400)),
        )
        .await;

        let scoped = engine.metrics_for(&ids, now, Some("work"), None).await.unwrap();
        assert_eq!(metric(&scoped, "cost_week").value, Some(2.0), "only the work run, inside the default window");

        let all = engine.metrics_for(&ids, now, None, None).await.unwrap();
        assert_eq!(metric(&all, "cost_week").value, Some(92.0), "work + side, both inside the default window");

        let widened = engine.metrics_for(&ids, now, Some("work"), Some(MetricsWindow::FourteenDays)).await.unwrap();
        assert_eq!(
            metric(&widened, "cost_week").value,
            Some(6.0),
            "the 14-day window also catches the nine-days-ago run, unnormalised"
        );
    }

    /// A run that *started* before the trailing week but *ended* inside it
    /// must be absent from `cost_week` -- the mutation this guards against
    /// is re-deriving the metric from `usage_metric`'s own `ended_at` rule
    /// (or the `needs_runs` prefetch) instead of calling `Engine::spend`.
    #[tokio::test]
    async fn cost_week_counts_a_run_by_when_it_started_not_when_it_ended() {
        let (engine, database) = scoped_engine();
        let now = Utc::now();
        let ids = [MetricId::new("cost_week").unwrap()];

        timed_run(
            &engine,
            &database,
            "started before the window",
            "work",
            RunStatus::Done,
            now - chrono::Duration::days(8),
            Some(now - chrono::Duration::hours(1)),
            Some(measured(50.0, 5_000)),
        )
        .await;
        timed_run(
            &engine,
            &database,
            "inside the window",
            "work",
            RunStatus::Done,
            now - chrono::Duration::hours(2),
            Some(now - chrono::Duration::hours(1)),
            Some(measured(3.0, 300)),
        )
        .await;

        let computed = engine.metrics_for(&ids, now, Some("work"), None).await.unwrap();
        assert_eq!(
            metric(&computed, "cost_week").value,
            Some(3.0),
            "the eight-day-old run started outside the trailing week, however recently it ended"
        );

        let spend = crate::facts::Facts::<factory_kernel::L6>::new(&engine)
            .get::<factory_kernel::CostReport>(&factory_core::usage::SpendQuery {
                scope: Some("work".into()),
                from: Some(now - chrono::Duration::days(7)),
                to: Some(now),
                group_by: factory_core::usage::CostGroupBy::Scope,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            metric(&computed, "cost_week").value,
            Some(spend.total.cost_usd),
            "cost_week is exactly Engine::spend's own total for the same window and scope"
        );
    }

    #[tokio::test]
    async fn estimate_accuracy_reads_each_runs_original_estimate_against_its_wall_time() {
        let engine = test_engine(Vec::new());
        let ids = [MetricId::new("estimate_accuracy").unwrap()];

        // Nothing carries an original_estimate yet: unavailable, never zero.
        finished_run(&engine, "no-estimate", RunStatus::Done, Trigger::Manual, chrono::Duration::hours(1)).await;
        let computed = engine.metrics(&ids, Utc::now()).await.unwrap();
        let value = &computed.values[0];
        assert_eq!(value.value, None);
        assert!(value.reason.as_deref().unwrap().contains("an original estimate"), "{value:?}");

        // `finished_run` only moves `ended_at` back by `ended_ago`;
        // `started_at` stays at creation, so every one of its runs has a
        // wall time that clamps to ~0 seconds -- exploited here to make
        // "within range" and "outside range" deterministic without
        // controlling the clock.
        for (label, low, high) in [("within", 0u64, 100u64), ("outside", 100u64, 200u64)] {
            let task = finished_run(&engine, label, RunStatus::Done, Trigger::Manual, chrono::Duration::hours(1)).await;
            let run = engine.store.runs(&task.id, 1).await.unwrap().remove(0);
            engine
                .store
                .update_run(
                    &run.id,
                    &RunPatch {
                        original_estimate: Some(factory_core::task::Estimate {
                            time: factory_core::task::TimeEstimateRange { low, expected: (low + high) / 2, high },
                            cost: None,
                        }),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
        }
        let computed = engine.metrics(&ids, Utc::now()).await.unwrap();
        let value = &computed.values[0];
        assert_eq!(value.value, Some(0.5), "1 of the 2 estimated runs is within its own range; the third has no estimate at all: {value:?}");
    }

    // -------------------------------------------------------------- backup (#154)

    /// A throwaway instance with `infrastructure.backup` configured, for the
    /// two backup metrics -- the daemon-side twin of `backup::tests`'
    /// `engine_backing_up`, since that fixture is private to its own module.
    fn backup_metrics_test_engine(destination: &str) -> (Arc<Engine>, PathBuf) {
        let base = std::env::temp_dir().join(format!("factory-metrics-backup-{}", uuid::Uuid::new_v4()));
        let root = base.join("instance");
        std::fs::create_dir_all(root.join(".factory/knowledge")).unwrap();
        std::fs::write(root.join(".factory/config.yaml"), "version: 1\ninstance:\n  id: test\n  name: test\n").unwrap();
        std::fs::write(root.join(".factory/knowledge/page.md"), "# A page\n").unwrap();
        let database = root.join(".factory/factory.sqlite");
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open(&database).unwrap());
        let mut company: Scope = serde_yaml_ng::from_str("id: company-id\nname: company\n").unwrap();
        company.path = PathBuf::from(".");
        let destination = base.join(destination);
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig::default(),
            scope: Some(company.clone()),
            scopes: vec![company],
            roles: Default::default(),
            dashboard: None,
            policies: PolicyDeclaration::default(),
            quality: Default::default(),
            infrastructure: serde_yaml_ng::from_str(&format!(
                "backup:\n  destination: {}\n  keep: {{ daily: 7 }}\n",
                destination.display()
            ))
            .unwrap(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let factory = Factory { root, config };
        let engine = Engine::new(factory, Registry::with_builtins(), store, PathBuf::from("factory"), Vec::new())
            .with_backup_store(crate::backup::BackupStore::open(&database).unwrap());
        (Arc::new(engine), base)
    }

    #[tokio::test]
    async fn backup_metrics_are_none_with_a_reason_when_no_backup_is_configured() {
        let engine = test_engine(Vec::new());
        let ids = [MetricId::new("backup_age_hours").unwrap(), MetricId::new("backup_verified_age_days").unwrap()];
        let computed = engine.metrics(&ids, Utc::now()).await.unwrap();
        for id in ["backup_age_hours", "backup_verified_age_days"] {
            let v = computed.values.iter().find(|v| v.id.as_str() == id).unwrap();
            assert_eq!(v.value, None, "{id}");
            assert_eq!(v.reason.as_deref(), Some("no backup is configured"), "{id}");
        }
    }

    #[tokio::test]
    async fn backup_metrics_read_the_newest_snapshot_and_verification_as_of_the_capture_instant() {
        let (engine, base) = backup_metrics_test_engine("destination");
        engine.backup_run(factory_core::backup::BackupTrigger::Manual, "owner".into()).await.unwrap();
        engine.backup_verify(None, None, "owner".into()).await.unwrap();

        let now = Utc::now();
        let ids = [MetricId::new("backup_age_hours").unwrap(), MetricId::new("backup_verified_age_days").unwrap()];
        let computed = engine.metrics(&ids, now).await.unwrap();

        let age = computed.values.iter().find(|v| v.id.as_str() == "backup_age_hours").unwrap();
        assert_eq!(age.reason, None, "{age:?}");
        assert!(age.value.is_some_and(|h| (0.0..0.05).contains(&h)), "just taken: {age:?}");
        assert_eq!(age.as_of, now, "as_of is the capture instant passed to Engine::metrics, never a stored timestamp");

        let verified = computed.values.iter().find(|v| v.id.as_str() == "backup_verified_age_days").unwrap();
        assert_eq!(verified.reason, None, "{verified:?}");
        assert!(verified.value.is_some_and(|d| (0.0..0.05).contains(&d)), "just verified: {verified:?}");
        assert_eq!(verified.as_of, now);

        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn backup_verified_age_days_is_none_with_a_reason_before_anything_is_verified() {
        let (engine, base) = backup_metrics_test_engine("destination");
        engine.backup_run(factory_core::backup::BackupTrigger::Manual, "owner".into()).await.unwrap();

        let computed = engine
            .metrics(&[MetricId::new("backup_verified_age_days").unwrap()], Utc::now())
            .await
            .unwrap();
        let v = &computed.values[0];
        assert_eq!(v.value, None);
        assert_eq!(v.reason.as_deref(), Some("no snapshot in the destination has been verified"));

        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn a_failed_verification_reads_backup_verified_age_days_none_but_leaves_backup_age_hours_alone() {
        let (engine, base) = backup_metrics_test_engine("destination");
        let snapshot = engine.backup_run(factory_core::backup::BackupTrigger::Manual, "owner".into()).await.unwrap();

        // Flip a byte in the middle of the archive -- the same corruption
        // `archive::tests::a_damaged_archive_fails_verification_and_says_so`
        // uses -- so verification fails honestly rather than being refused
        // outright (a lock held, or the snapshot not found).
        let mut bytes = std::fs::read(&snapshot.path).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0xff;
        std::fs::write(&snapshot.path, &bytes).unwrap();
        let verification = engine.backup_verify(None, None, "owner".into()).await.unwrap();
        assert!(!verification.ok, "{:?}", verification.checks);

        let ids = [MetricId::new("backup_age_hours").unwrap(), MetricId::new("backup_verified_age_days").unwrap()];
        let computed = engine.metrics(&ids, Utc::now()).await.unwrap();

        let age = computed.values.iter().find(|v| v.id.as_str() == "backup_age_hours").unwrap();
        assert!(age.value.is_some(), "the snapshot exists whether or not it verifies: {age:?}");

        let verified = computed.values.iter().find(|v| v.id.as_str() == "backup_verified_age_days").unwrap();
        assert_eq!(verified.value, None, "never a bare 0 for a failed verification: {verified:?}");
        assert!(
            verified.reason.as_deref().is_some_and(|r| r.starts_with("the newest verification failed at")),
            "{verified:?}"
        );

        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn backup_age_hours_reads_the_destination_unreachable_before_it_is_ever_mounted() {
        // Before any backup has run, the configured destination has never
        // been created -- indeterminate, never "no snapshot yet", which
        // would claim more than a missing mount point can honestly say.
        let (engine, base) = backup_metrics_test_engine("destination");
        let computed = engine
            .metrics(&[MetricId::new("backup_age_hours").unwrap()], Utc::now())
            .await
            .unwrap();
        let v = &computed.values[0];
        assert_eq!(v.value, None);
        assert_eq!(v.reason.as_deref(), Some("the destination is not reachable"));
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn backup_age_hours_reads_no_snapshot_yet_once_the_destination_exists_but_is_empty() {
        let (engine, base) = backup_metrics_test_engine("destination");
        std::fs::create_dir_all(base.join("destination")).unwrap();
        let computed = engine
            .metrics(&[MetricId::new("backup_age_hours").unwrap()], Utc::now())
            .await
            .unwrap();
        let v = &computed.values[0];
        assert_eq!(v.value, None);
        assert_eq!(v.reason.as_deref(), Some("no snapshot yet"));
        std::fs::remove_dir_all(base).ok();
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
        assert!(has("cost_week"), "a fixed metric, so in the default set (#164)");
        assert!(has("estimate_accuracy"), "a fixed metric, so in the default set (#168)");
        assert!(has("compliance.cra"), "implied by the loaded policy catalogue");
        assert!(has("open_controls.cra"));
        assert!(has("compliance.cra"), "also implied by the goals catalogue's own key result");
        assert!(has("ready_rate"), "a fixed metric, so in the default set (#165)");
        assert!(has("needs_info_rate"));
        assert!(has("duplicate_rate"));
        assert!(has("intake_lead_time"));
    }

    // ----------------------------------------------------- intake (#165)

    fn intake_assessment(scope: &str) -> factory_core::intake::Assessment {
        factory_core::intake::Assessment {
            axes: factory_core::intake::Axis::ALL
                .into_iter()
                .map(|axis| factory_core::intake::AxisCheck {
                    axis,
                    pass: axis != factory_core::intake::Axis::Verifiability,
                    evidence: format!("{} checked", axis.as_str()),
                    cost: None,
                })
                .collect(),
            category: "bugfix".into(),
            impact: factory_core::intake::Level::High,
            urgency: factory_core::intake::Level::Medium,
            complexity: 3,
            estimate: None,
            routing: factory_core::intake::Routing { scope: scope.into(), agent: Some("worker".into()), ..Default::default() },
            summary: "bounded".into(),
            questions: vec![],
            split: vec![],
            duplicates: vec![],
            checks: vec![],
            areas: vec![],
        }
    }

    fn ready_assessment(scope: &str) -> factory_core::intake::Assessment {
        let mut a = intake_assessment(scope);
        for check in &mut a.axes {
            check.pass = true;
        }
        a
    }

    async fn add_intake_item(engine: &Arc<Engine>, scope: &str, title: &str) -> factory_core::task::Task {
        engine
            .intake_add(
                &Caller::Owner,
                factory_core::intake::NewIntake {
                    title: title.into(),
                    instructions: "fix it".into(),
                    scope: Some(scope.into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
    }

    /// Rewrites a decision-kind journal entry's own `at` (and, when it
    /// carries one, its `data.decision.at`) after the fact -- the same
    /// technique `store_run_at` uses for a run's timestamps, needed because
    /// `intake_decide` always journals at the real `Utc::now()` and a
    /// window/rollup test needs decisions spread across specific days
    /// relative to one `now` captured once at the top of the test.
    fn backdate_decision(database: &std::path::Path, task_id: &str, kind: &str, at: DateTime<Utc>) {
        let at_str = at.to_rfc3339();
        let conn = rusqlite::Connection::open(database).unwrap();
        conn.execute(
            "UPDATE task_entries SET at = ?1, data = json_set(data, '$.at', ?1) \
             WHERE task_id = ?2 AND json_extract(data, '$.kind') = ?3",
            params![at_str, task_id, kind],
        )
        .unwrap();
        conn.execute(
            "UPDATE task_entries SET data = json_set(data, '$.data.decision.at', ?1) \
             WHERE task_id = ?2 AND json_extract(data, '$.kind') = ?3 AND json_extract(data, '$.data.decision') IS NOT NULL",
            params![at_str, task_id, kind],
        )
        .unwrap();
    }

    /// Backdates an item's own `Intake.received_at` -- direct store
    /// manipulation, like `backdate_decision`, since `intake_decide` always
    /// carries forward whatever `received_at` a task already had.
    async fn backdate_received_at(engine: &Arc<Engine>, task_id: &str, at: DateTime<Utc>) {
        let task = engine.store.get(task_id).await.unwrap().unwrap();
        let mut record = task.intake.unwrap();
        record.received_at = at;
        engine.store.update(task_id, &TaskPatch { intake: Some(record), ..Default::default() }).await.unwrap();
    }

    #[tokio::test]
    async fn intake_metrics_roll_up_the_subtree_and_read_the_window_override_with_as_of() {
        use factory_core::intake::{Decision, WontfixReason};

        let (engine, database) = scoped_engine();
        let now = Utc::now();

        // A: released ready in "work", decided 2 days ago, received 10 days
        // before that -- an 8-day lead time.
        let a = add_intake_item(&engine, "work", "A").await;
        backdate_received_at(&engine, &a.id, now - chrono::Duration::days(10)).await;
        engine.intake_assess(&Caller::Owner, &a.id, ready_assessment("work"), true).await.unwrap();
        backdate_decision(&database, &a.id, TRIAGE_VERDICT_KIND, now - chrono::Duration::days(2));

        // B: sent back for information in "nested" (a child of "work"),
        // decided 47 hours ago -- the newest decision in the work subtree.
        let b = add_intake_item(&engine, "nested", "B").await;
        engine.intake_assess(&Caller::Owner, &b.id, intake_assessment("nested"), true).await.unwrap();
        backdate_decision(&database, &b.id, TRIAGE_VERDICT_KIND, now - chrono::Duration::hours(47));

        // C: closed as a duplicate in "side" (a sibling of "work"), decided
        // 2 days ago -- excluded from the "work" subtree.
        let c = add_intake_item(&engine, "side", "C").await;
        engine
            .intake_decide(
                &Caller::Owner,
                &c.id,
                Decision::Wontfix { reason: WontfixReason::Duplicate, evidence: "same as A".into(), duplicate_of: Some(a.id.clone()) },
            )
            .await
            .unwrap();
        backdate_decision(&database, &c.id, "intake_closed", now - chrono::Duration::days(2));

        // D: closed as invalid in "work", decided 20 days ago -- inside the
        // default (28-day) and 90-day windows, outside a 14-day one.
        let d = add_intake_item(&engine, "work", "D").await;
        engine
            .intake_decide(
                &Caller::Owner,
                &d.id,
                Decision::Wontfix { reason: WontfixReason::Invalid, evidence: "not reproducible".into(), duplicate_of: None },
            )
            .await
            .unwrap();
        backdate_decision(&database, &d.id, "intake_closed", now - chrono::Duration::days(20));

        let ids: Vec<MetricId> = ["ready_rate", "needs_info_rate", "duplicate_rate", "intake_lead_time"]
            .into_iter()
            .map(|id| MetricId::new(id).unwrap())
            .collect();

        // Default window (28 days), scope "work": A, B and D all count;
        // C (in "side") does not. Newest of the three is B.
        let work_default = engine.metrics_for(&ids, now, Some("work"), None).await.unwrap();
        assert_eq!(metric(&work_default, "ready_rate").value, Some(1.0 / 3.0));
        assert_eq!(metric(&work_default, "needs_info_rate").value, Some(1.0 / 3.0));
        assert_eq!(metric(&work_default, "duplicate_rate").value, Some(0.0));
        assert_eq!(metric(&work_default, "ready_rate").as_of, now - chrono::Duration::hours(47));
        assert_eq!(
            metric(&work_default, "intake_lead_time").value,
            Some(chrono::Duration::days(8).num_seconds() as f64)
        );
        assert_eq!(metric(&work_default, "intake_lead_time").as_of, now - chrono::Duration::days(2));

        // A 14-day window excludes D (20 days ago): only A and B remain.
        let work_14d = engine.metrics_for(&ids, now, Some("work"), Some(MetricsWindow::FourteenDays)).await.unwrap();
        assert_eq!(metric(&work_14d, "ready_rate").value, Some(0.5));
        assert_eq!(metric(&work_14d, "needs_info_rate").value, Some(0.5));

        // The excluded sibling scope has its own, independent rollup.
        let side = engine.metrics_for(&ids, now, Some("side"), None).await.unwrap();
        assert_eq!(metric(&side, "ready_rate").value, Some(0.0));
        assert_eq!(metric(&side, "duplicate_rate").value, Some(1.0));
        assert_eq!(metric(&side, "duplicate_rate").as_of, now - chrono::Duration::days(2));

        // Unscoped rolls up all four decisions.
        let all = engine.metrics_for(&ids, now, None, None).await.unwrap();
        assert_eq!(metric(&all, "ready_rate").value, Some(0.25));
        assert_eq!(metric(&all, "needs_info_rate").value, Some(0.25));
        assert_eq!(metric(&all, "duplicate_rate").value, Some(0.25));
        assert_eq!(metric(&all, "ready_rate").as_of, now - chrono::Duration::hours(47));
    }

    #[tokio::test]
    async fn a_deleted_intake_item_drops_out_of_the_intake_metrics() {
        use factory_core::intake::{Decision, WontfixReason};

        let (engine, database) = scoped_engine();
        let now = Utc::now();
        let e = add_intake_item(&engine, "work", "E").await;
        engine
            .intake_decide(
                &Caller::Owner,
                &e.id,
                Decision::Wontfix { reason: WontfixReason::Duplicate, evidence: "dup".into(), duplicate_of: Some("x".into()) },
            )
            .await
            .unwrap();
        backdate_decision(&database, &e.id, "intake_closed", now - chrono::Duration::hours(1));

        let ids: Vec<MetricId> = ["duplicate_rate"].into_iter().map(|id| MetricId::new(id).unwrap()).collect();
        let before = engine.metrics_for(&ids, now, Some("work"), None).await.unwrap();
        assert_eq!(metric(&before, "duplicate_rate").value, Some(1.0));

        assert!(engine.store.delete(&e.id).await.unwrap());
        let after = engine.metrics_for(&ids, now, Some("work"), None).await.unwrap();
        assert_eq!(metric(&after, "duplicate_rate").value, None);
        assert_eq!(
            metric(&after, "duplicate_rate").reason.as_deref(),
            Some("no triage decisions in the trailing 28 days")
        );
    }

    // -- conformance_rate / gate_fail_rate (#158) ----------------------------

    #[test]
    fn is_attestation_metric_names_only_the_conformance_families() {
        assert!(is_attestation_metric("gate_fail_rate"));
        assert!(is_attestation_metric("review_reject_rate"));
        assert!(is_attestation_metric("conformance_rate.feature"));
        assert!(
            !is_attestation_metric("first_pass_yield"),
            "a request for it alone must never trigger the read"
        );
        assert!(!is_attestation_metric("fail_rate"));
    }

    /// `root` (`.`), with `work` (`projects/work`) and its sibling `side`
    /// (`projects/side`) -- no policy or quality catalogue at all: neither
    /// `conformance_rate.<category>` nor `gate_fail_rate` reads one, only
    /// `Engine::attested_runs`.
    fn subtree_engine() -> Arc<Engine> {
        let root = std::env::temp_dir().join(format!(
            "factory-metrics-conformance-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut root_scope: Scope =
            serde_yaml_ng::from_str("id: root-id\nname: company\n").unwrap();
        root_scope.path = PathBuf::from(".");
        let mut work: Scope = serde_yaml_ng::from_str("id: work-id\nname: work\n").unwrap();
        work.path = PathBuf::from("projects/work");
        let mut side: Scope = serde_yaml_ng::from_str("id: side-id\nname: side\n").unwrap();
        side.path = PathBuf::from("projects/side");
        let config = Config {
            version: 1,
            instance: Instance {
                id: "test".into(),
                name: "test".into(),
            },
            daemon: DaemonConfig::default(),
            scope: Some(root_scope.clone()),
            scopes: vec![root_scope, work, side],
            roles: Default::default(),
            dashboard: None,
            policies: Default::default(),
            quality: Default::default(),
            infrastructure: Default::default(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        Arc::new(Engine::new(
            Factory { root, config },
            Registry::with_builtins(),
            store,
            PathBuf::from("factory"),
            Vec::new(),
        ))
    }

    /// A finished, held `category` run in `scope`: one gate step (`tests`),
    /// attested by `GATE_ACTOR` with `verdict`, both dated `ended_at` --
    /// built directly through the store and `PolicyStore`, the same shape
    /// `policies::tests::attested_feature_run` builds for the check itself.
    async fn held_run(
        engine: &Arc<Engine>,
        scope: &str,
        category: &str,
        ended_at: DateTime<Utc>,
        verdict: factory_core::control_plan::AttestationVerdict,
    ) -> Run {
        use factory_core::control_plan::{RequiredStep, StepAttestation, StepKind, GATE_ACTOR};
        let task = engine
            .store
            .create(&task_from_new(
                NewTask {
                    title: format!("t-{}", uuid::Uuid::new_v4()),
                    category: Some(category.into()),
                    ..Default::default()
                },
                scope.into(),
                "worker".into(),
                "shell".into(),
            ))
            .await
            .unwrap();
        let run = engine
            .store
            .create_run(&NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Manual,
                agent: "worker".into(),
                adapter: "shell".into(),
                runtime: "shell".into(),
                token: "tok".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        let required = vec![RequiredStep {
            step: "tests".into(),
            kind: StepKind::Gate,
            command: Some("true".into()),
            timeout_seconds: None,
            required_by: Vec::new(),
            node_id: None,
            actor: None,
            by: None,
        }];
        let run = engine
            .store
            .update_run(
                &run.id,
                &RunPatch {
                    status: Some(RunStatus::Done),
                    ended_at: Some(ended_at),
                    required_steps: Some(required),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let attestation = StepAttestation {
            id: uuid::Uuid::new_v4().to_string(),
            run_id: run.id.clone(),
            task_id: task.id.clone(),
            scope: scope.into(),
            category: category.into(),
            step: "tests".into(),
            kind: StepKind::Gate,
            actor: GATE_ACTOR.to_string(),
            verdict,
            required_by: Vec::new(),
            command: Some("true".into()),
            exit_code: Some(
                if verdict == factory_core::control_plan::AttestationVerdict::Pass {
                    0
                } else {
                    1
                },
            ),
            output: None,
            dir: "/tmp".into(),
            commit: None,
            dirty: None,
            node_id: None,
            at: ended_at,
            findings: None,
            round: 0,
            worktree_digest: None,
        };
        engine
            .policies
            .append_step_attestation(&attestation)
            .await
            .unwrap();
        run
    }

    #[tokio::test]
    async fn conformance_rate_and_gate_fail_rate_read_exact_ratios_and_narrow_by_scope() {
        use factory_core::control_plan::AttestationVerdict::{Fail, Pass};
        let engine = subtree_engine();
        let base = Utc::now();

        // `work`: one conforming, one failing.
        held_run(&engine, "work", "feature", base, Pass).await;
        held_run(&engine, "work", "feature", base, Fail).await;
        // `side`, a sibling of `work`: one conforming -- must not count once
        // scoped to `work`.
        held_run(&engine, "side", "feature", base, Pass).await;

        let ids: Vec<MetricId> = ["conformance_rate.feature", "gate_fail_rate"]
            .into_iter()
            .map(|id| MetricId::new(id).unwrap())
            .collect();

        // `now` is captured fresh, after every fixture run's own `started_at`
        // was auto-assigned by the store -- `runs_between`'s own
        // `started_at <= to` guard needs `to` no earlier than that.
        let now = Utc::now();
        let work_only = engine
            .metrics_for(&ids, now, Some("work"), None)
            .await
            .unwrap();
        assert_eq!(
            metric(&work_only, "conformance_rate.feature").value,
            Some(0.5)
        );
        assert_eq!(metric(&work_only, "gate_fail_rate").value, Some(0.5));

        // Unscoped: 2 of 3 held runs conform; 1 of 3 gate attestations failed.
        let all = engine.metrics_for(&ids, now, None, None).await.unwrap();
        assert_eq!(
            metric(&all, "conformance_rate.feature").value,
            Some(2.0 / 3.0)
        );
        assert_eq!(metric(&all, "gate_fail_rate").value, Some(1.0 / 3.0));
    }

    async fn reviewed_run(
        engine: &Arc<Engine>,
        scope: &str,
        ended_at: DateTime<Utc>,
        verdicts: &[factory_core::control_plan::AttestationVerdict],
    ) {
        use factory_core::control_plan::{AttestationVerdict, StepKind};
        let run = held_run(engine, scope, "feature", ended_at, AttestationVerdict::Pass).await;
        let mut required = run.required_steps.clone();
        let mut step = required[0].clone();
        step.step = "review".into();
        step.kind = StepKind::Review;
        step.command = None;
        step.actor = Some("reviewer".into());
        required.push(step);
        engine
            .store
            .update_run(
                &run.id,
                &factory_core::run::RunPatch {
                    required_steps: Some(required),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let base = engine.run_attestations(&run.id).await.unwrap().remove(0);
        for (round, verdict) in verdicts.iter().enumerate() {
            let mut a = base.clone();
            a.id = uuid::Uuid::new_v4().to_string();
            a.step = "review".into();
            a.kind = StepKind::Review;
            a.actor = "reviewer".into();
            a.command = None;
            a.exit_code = None;
            a.verdict = *verdict;
            a.round = round as u32;
            engine.policies.append_step_attestation(&a).await.unwrap();
        }
    }

    #[tokio::test]
    async fn review_reject_rate_uses_one_fact_read_for_scope_windows_goals_and_signposts() {
        use factory_core::control_plan::AttestationVerdict::{Fail, Pass};
        let engine = subtree_engine();
        let root = engine.factory_snapshot().root.clone();
        std::fs::create_dir_all(root.join(".factory/goals")).unwrap();
        std::fs::create_dir_all(root.join(".factory/scenarios")).unwrap();
        std::fs::write(root.join(".factory/goals/2026-q4.yaml"),
            "cycle: {id: 2026-q4, from: 2026-10-01, to: 2026-12-31}\nobjectives:\n  - id: quality\n    scope: work\n    title: Independent review\n    key_results:\n      - {id: rejection, title: Fewer rejections, kind: committed, metric: review_reject_rate, baseline: 1, target: 0}\n").unwrap();
        std::fs::write(root.join(".factory/scenarios/review-watch.yaml"),
            "name: review-watch\ntitle: Review rejections\nsignposts:\n  - {metric: review_reject_rate, above: 0.4}\n").unwrap();
        let id = MetricId::new("review_reject_rate").unwrap();
        let empty = engine
            .metrics_for(std::slice::from_ref(&id), Utc::now(), Some("work"), None)
            .await
            .unwrap();
        assert!(metric(&empty, "review_reject_rate").value.is_none());
        assert!(metric(&empty, "review_reject_rate")
            .reason
            .as_ref()
            .unwrap()
            .contains("no independent review"));
        let base = Utc::now();
        reviewed_run(&engine, "work", base, &[Fail, Pass]).await;
        reviewed_run(&engine, "work", base - chrono::Duration::days(2), &[Fail]).await;
        reviewed_run(&engine, "side", base, &[Pass]).await;
        let now = Utc::now();
        let scoped = engine
            .metrics_for(std::slice::from_ref(&id), now, Some("work"), None)
            .await
            .unwrap();
        assert_eq!(metric(&scoped, "review_reject_rate").value, Some(2.0 / 3.0));
        assert_eq!(metric(&scoped, "review_reject_rate").as_of, base);
        let day = engine
            .metrics_for(
                std::slice::from_ref(&id),
                now,
                Some("work"),
                Some(MetricsWindow::Day),
            )
            .await
            .unwrap();
        assert_eq!(metric(&day, "review_reject_rate").value, Some(0.5));
        let all = engine
            .metrics_for(std::slice::from_ref(&id), now, None, None)
            .await
            .unwrap();
        assert_eq!(metric(&all, "review_reject_rate").value, Some(0.5));
        let goals = engine
            .goals_report(Some("work"), Some("2026-q4"))
            .await
            .unwrap();
        let kr = &goals.report.as_ref().unwrap().objectives[0].key_results[0];
        // Existing Goals semantics: scope filters visible objectives, while
        // metric values are computed over the whole instance.
        assert_eq!(kr.value, Some(0.5));
        assert_eq!(kr.score, Some(0.5));
        let scenarios = engine.scenarios_report(Some("work")).await.unwrap();
        assert!(scenarios
            .triggered
            .iter()
            .any(|s| s.scenario == "review-watch"));
    }

    #[tokio::test]
    async fn conformance_rate_is_none_with_a_reason_for_an_unknown_category() {
        let engine = subtree_engine();
        let now = Utc::now();
        let ids = vec![MetricId::new("conformance_rate.nonexistent").unwrap()];
        let metrics = engine.metrics_for(&ids, now, None, None).await.unwrap();
        assert_eq!(metric(&metrics, "conformance_rate.nonexistent").value, None);
        assert!(metric(&metrics, "conformance_rate.nonexistent")
            .reason
            .is_some());
    }

    #[tokio::test]
    async fn an_explicit_day_window_drops_a_run_the_default_28d_window_would_still_count() {
        use factory_core::control_plan::AttestationVerdict::Pass;
        let engine = subtree_engine();
        let base = Utc::now();
        held_run(&engine, "work", "feature", base, Pass).await;
        held_run(
            &engine,
            "work",
            "feature",
            base - chrono::Duration::days(2),
            Pass,
        )
        .await;

        let ids = vec![MetricId::new("conformance_rate.feature").unwrap()];
        let now = Utc::now();
        let default_window = engine
            .metrics_for(&ids, now, Some("work"), None)
            .await
            .unwrap();
        assert_eq!(
            metric(&default_window, "conformance_rate.feature").value,
            Some(1.0),
            "both runs conform"
        );

        // `queue_wait_p95`'s own family reads runs by `ended_at`, doubled for
        // recovery streaks; `attested_runs` reads a plain single window, so
        // an explicit `day` keeps only the run from the last 24h.
        let day_only = engine
            .metrics_for(&ids, now, Some("work"), Some(MetricsWindow::Day))
            .await
            .unwrap();
        assert_eq!(
            metric(&day_only, "conformance_rate.feature").value,
            Some(1.0),
            "still 1.0 -- the older run is simply gone"
        );

        // A held run in the trailing 28d that the `day` window drops changes
        // the denominator, provable by adding a failing one just inside the
        // day window and a passing one just outside it.
        held_run(
            &engine,
            "work",
            "feature",
            base,
            factory_core::control_plan::AttestationVerdict::Fail,
        )
        .await;
        let now = Utc::now();
        let day_only = engine
            .metrics_for(&ids, now, Some("work"), Some(MetricsWindow::Day))
            .await
            .unwrap();
        assert_eq!(
            metric(&day_only, "conformance_rate.feature").value,
            Some(0.5),
            "the 2-day-old pass is out of the day window"
        );
        let default_window = engine
            .metrics_for(&ids, now, Some("work"), None)
            .await
            .unwrap();
        assert_eq!(
            metric(&default_window, "conformance_rate.feature").value,
            Some(2.0 / 3.0),
            "the default 28d window still counts all three"
        );
    }
}
