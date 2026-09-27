//! Where scenario requests are served (`#100`, the L6 Scenarios tab). Like
//! `policies/mod.rs` and `goals/mod.rs`: the authored files under
//! `<root>/.factory/scenarios/` are the source of truth for *what* a
//! scenario says (`factory_core::scenario`, pure and tested on its own),
//! re-read on every request; this module assembles the evidence, metrics
//! and task history that already live elsewhere in the daemon and folds
//! them against it.
//!
//! ## Reuse, not reimplementation
//!
//! The exact policy delta reuses `policies::Engine::dataset_level_facts`/
//! `evidence_for_scope` -- the same two functions `policy_report` itself
//! calls, extracted from it for exactly this (see their own doc comments in
//! `policies/mod.rs`). A scenario's overlay chain can ask a check kind the
//! real chain never did (an `add_frameworks` draft, say); calling those two
//! functions again for the scenario's own, larger `Applied` set is what
//! picks up the extra fact lazily, without this module inventing a second
//! evidence-gathering path that could quietly drift from the real one.
//! `Engine::metrics` supplies every driver, signpost and goal-KR value, the
//! same registry `#99`'s Goals tab reads.
//!
//! ## Throughput history: non-overlapping weeks, not the rolling series
//!
//! [`weekly_throughput_history`] sums `production.rs`'s own daily grid into
//! [`THROUGHPUT_HISTORY_WEEKS`] non-overlapping 7-day buckets, ending today.
//! Deliberately *not* a bootstrap sample of `throughput_week`'s own
//! registry series: that series is a rolling 7-day sum taken once per day,
//! so any two points within six days of each other share up to six of
//! their seven days -- resampling that with replacement (Magennis's own
//! method, `scenario::forecast_completion`) draws heavily autocorrelated
//! "weeks" and understates real week-to-week variance, producing bands that
//! read falsely tight. Independent, non-overlapping weeks are what the
//! method assumes.
//!
//! ## Never writes config
//!
//! [`Engine::scenarios_report`] and [`Engine::scenario_whatif`] are fully
//! read-only. [`Engine::scenario_promote`] writes ordinary tasks, through
//! the exact `Engine::create` path `Request::TaskCreate`/`policy_remediate`
//! themselves use -- never a scope's own `.factory/config.yaml`, never a
//! scenario file, never a real policy catalogue. A scenario is data a
//! person authored and Factory only ever reads; turning one into real work
//! is always this one explicit, owner/agent-driven action (design §8).
//!
//! ## Signposts outside the tab
//!
//! `#100` asks that a triggered signpost be visible outside the Scenarios
//! tab -- on the dashboard, in the inbox -- as an observation, never an
//! automatic consequence. The Inbox (`ui/js/dashboard.js`'s `inboxItems`)
//! is built entirely client-side, from the task list alone; there is no
//! daemon-side inbox aggregate to add to. This module instead surfaces
//! [`ScenariosReport::triggered`], every currently-`Triggered` signpost
//! across every scenario, flattened out of `scenarios[].signposts` so a
//! dashboard/inbox reader does not have to walk every card itself. The L6
//! Scenarios UI slice (`ui/js/{scenarios,scenarios-model}.js`, not part of
//! this slice) is expected to read this field and render it on the
//! dashboard; see the README's "Scenarios" section.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use factory_core::config::{Factory, Scope};
use factory_core::error::{FactoryError, Result};
use factory_core::goals::{self, KrRef};
use factory_core::metrics::{MetricId, MetricValue};
use factory_core::policy::{self, Attestation};
use factory_core::protocol::{
    PromotedControl, ScenarioBacklog, ScenarioBaseline, ScenarioDrivers, ScenarioGoalProbability,
    ScenarioPromoteResult, ScenarioResult, ScenarioScopeDelta, ScenarioWhatIfResult, ScenariosReport,
    SkippedControl, TriggeredSignpost,
};
use factory_core::scenario::{self, DriverId, Scenario};
use factory_core::task::{NewTask, Task, TaskFilter};

use crate::engine::Engine;
use crate::policies::subtree_scopes;

/// How many weeks of production history [`weekly_throughput_history`]
/// draws from -- `scenario::Horizon::default()`'s own 26 weeks, so a
/// scenario with no explicit `horizon:` bootstrap-samples from a history
/// exactly as long as what it projects forward.
const THROUGHPUT_HISTORY_WEEKS: usize = 26;

/// How many Monte Carlo samples every forecast/goal-probability call in
/// this module draws. `factory_core::scenario`'s own tests run in the
/// hundreds; 1000 is still cheap (a splitmix64 draw and a few float
/// comparisons per sample) and gives smoother percentiles for a report a
/// person actually reads.
const SAMPLES: u32 = 1000;

/// The one-at-a-time swing [`scenario::tornado`] varies each driver by --
/// the same ±20% `factory_core::scenario`'s own tests use.
const TORNADO_SWING: f64 = 0.2;

/// v1's only computed outcome (`scenario::evaluate_outcomes`'s own
/// `effective_throughput`) -- what "the scenario's key outcome" (`#100`)
/// names until a second outcome exists to choose between.
const KEY_OUTCOME: &str = "effective_throughput";

/// The triggered ones among one scenario's evaluated signposts, flattened
/// for `ScenariosReport::triggered` and the Operations attention queue.
fn triggered_of(s: &Scenario, statuses: &[scenario::SignpostStatus]) -> Vec<TriggeredSignpost> {
    statuses
        .iter()
        .filter(|status| status.state == scenario::SignpostState::Triggered)
        .map(|status| TriggeredSignpost {
            scenario: s.name.clone(),
            metric: status.metric.clone(),
            reason: status.reason.clone(),
        })
        .collect()
}

/// Sum `daily` (`production.rs`'s own 53-week grid, oldest first) into
/// `weeks` non-overlapping 7-day buckets ending on the grid's own last day,
/// oldest bucket first -- see the module doc comment for why this, not the
/// `throughput_week` registry series. `daily` is always a fixed 371-entry
/// grid (`production.rs`), so this only ever returns fewer than `weeks`
/// buckets when `weeks` itself asks for more than 53.
fn weekly_throughput_history(daily: &[factory_core::protocol::ProductionBucket], weeks: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(weeks);
    let mut end = daily.len();
    for _ in 0..weeks {
        if end == 0 {
            break;
        }
        let start = end.saturating_sub(7);
        let sum: u32 = daily[start..end].iter().map(|b| b.finished).sum();
        out.push(f64::from(sum));
        end = start;
    }
    out.reverse();
    out
}

/// Every driver's baseline value: a registry-backed driver's current metric
/// value, when `values` carries one; `capacity_factor`'s neutral `1.0` (no
/// adjustment -- the same default `scenario::evaluate_outcomes` itself
/// falls back to when a driver is absent from a map at all). That one is
/// this module's own choice, not registry-derived -- see
/// `ScenarioBaseline::drivers`'s own doc comment. A registry-backed driver
/// with no current value (metric unavailable, or no data yet) is simply
/// absent from the result, the same "nothing to be relative to" rule
/// `scenario::apply_overrides` already holds for a driver `baseline` does
/// not carry.
fn driver_baseline(values: &BTreeMap<MetricId, MetricValue>) -> BTreeMap<DriverId, f64> {
    let mut out = BTreeMap::new();
    for def in scenario::driver_defs() {
        if let Some(name) = def.metric {
            if let Ok(id) = MetricId::new(name) {
                if let Some(value) = values.get(&id).and_then(|v| v.value) {
                    out.insert(def.id.to_string(), value);
                }
            }
        }
    }
    out.insert("capacity_factor".to_string(), 1.0);
    out
}

/// The multiplier driver overrides put on top of `weekly_throughput_history`
/// before it feeds a forecast: the ratio of `effective_throughput` after
/// `overridden` to before `baseline`, or `1.0` (no change) when the
/// baseline's own effective throughput is zero -- there is nothing to scale
/// proportionally from. Applied to every point in the history rather than
/// only to a single "current" throughput number, so the forecast's bands
/// keep the real history's own week-to-week shape and variance, just scaled
/// -- "effective throughput scaled per `evaluate_outcomes`" (`#100`).
fn throughput_scale_factor(baseline: &BTreeMap<DriverId, f64>, overridden: &BTreeMap<DriverId, f64>) -> f64 {
    let before = scenario::evaluate_outcomes(baseline).get(KEY_OUTCOME).copied().unwrap_or(0.0);
    let after = scenario::evaluate_outcomes(overridden).get(KEY_OUTCOME).copied().unwrap_or(0.0);
    if before > 0.0 {
        after / before
    } else {
        1.0
    }
}

fn scale_history(history: &[f64], factor: f64) -> Vec<f64> {
    history.iter().map(|w| (w * factor).max(0.0)).collect()
}

/// One key result's place in the loaded goals catalogue -- its cycle (for
/// the cycle's own end date, a `GoalChange`'s deadline fallback) and its
/// objective (for `KeyResult::bound_metric`, which needs the objective's own
/// id). `None` when no loaded cycle defines it.
fn find_kr<'a>(catalogue: &'a goals::GoalsCatalogue, kr: &KrRef) -> Option<(&'a goals::Cycle, &'a goals::Objective, &'a goals::KeyResult)> {
    for cycle in &catalogue.cycles {
        for objective in &cycle.objectives {
            if objective.id == kr.objective {
                if let Some(kd) = objective.key_results.iter().find(|k| k.id == kr.kr) {
                    return Some((cycle, objective, kd));
                }
            }
        }
    }
    None
}

/// A key result's current value: the latest check-in for a manual one (by
/// `at`, `goals::evaluate`'s own tie-break), or `values`' own entry for a
/// computed one's `bound_metric`. `None` when neither has anything yet --
/// "unscored", never a manufactured `0.0` (the same rule `goals::evaluate`
/// itself holds).
fn kr_current_value(
    kd: &goals::KeyResult,
    objective_id: &str,
    kr_ref: &KrRef,
    values: &BTreeMap<MetricId, MetricValue>,
    checkins: &[goals::CheckIn],
) -> Option<f64> {
    if kd.manual {
        checkins.iter().filter(|c| &c.kr == kr_ref).max_by_key(|c| c.at).map(|c| c.value)
    } else {
        kd.bound_metric(objective_id).and_then(|m| values.get(&m)).and_then(|v| v.value)
    }
}

/// Re-score one `GoalChange` -- see [`Engine::scenarios_report`]'s own doc
/// comment for `target`/`by` defaulting and the manual-KR shape deviation.
#[allow(clippy::too_many_arguments)]
fn build_goal_probability(
    change: &scenario::GoalChange,
    catalogue: &goals::GoalsCatalogue,
    values: &BTreeMap<MetricId, MetricValue>,
    checkins: &[goals::CheckIn],
    now: DateTime<Utc>,
    weekly_history: &[f64],
    seed: u64,
) -> ScenarioGoalProbability {
    let Some((cycle, objective, kd)) = find_kr(catalogue, &change.kr) else {
        return ScenarioGoalProbability {
            kr: change.kr.clone(),
            target: change.target,
            by: change.by,
            probability: scenario::GoalProbability {
                probability: None,
                reason: Some(format!("{} names no key result in any loaded cycle", change.kr)),
            },
        };
    };

    let effective_target = change.target.unwrap_or(kd.target);
    let effective_by = change.by.unwrap_or_else(|| cycle.ends_on());

    let Some(current_value) = kr_current_value(kd, &objective.id, &change.kr, values, checkins) else {
        return ScenarioGoalProbability {
            kr: change.kr.clone(),
            target: Some(effective_target),
            by: Some(effective_by),
            probability: scenario::GoalProbability {
                probability: None,
                reason: Some("no current value to project from yet -- no metric value or check-in".to_string()),
            },
        };
    };

    // A manual key result has no metric id to look a `KrShape` up from --
    // conservatively `Ratio` (never a fabricated probability), the same
    // deviation `KrShape::for_metric`'s own doc comment documents for a
    // metric `metrics::resolve` refuses outright.
    let shape = match kd.bound_metric(&objective.id) {
        Some(m) if !kd.manual => scenario::KrShape::for_metric(&m, current_value, effective_target),
        _ => scenario::KrShape::Ratio,
    };

    let probability = scenario::goal_probability(shape, current_value, effective_target, effective_by, now, weekly_history, SAMPLES, seed);
    ScenarioGoalProbability {
        kr: change.kr.clone(),
        target: Some(effective_target),
        by: Some(effective_by),
        probability,
    }
}

/// Every control status `catalogues` folds into, over every scope in
/// `target_scopes`, evaluated against the real chain (`Engine::policy_chain`)
/// alone -- the baseline side. Shares its evidence-gathering with the
/// scenario side and with `policy_report` through
/// `policies::Engine::dataset_level_facts`/`evidence_for_scope`.
async fn evaluate_baseline_over_scopes(
    engine: &Engine,
    snapshot: &Factory,
    target_scopes: &[Scope],
    catalogues: &[policy::Catalogue],
    tags: &BTreeSet<String>,
    all_attestations: &[Attestation],
    now: DateTime<Utc>,
) -> Result<(BTreeMap<String, Vec<policy::ControlStatus>>, Vec<policy::Finding>)> {
    let mut findings = Vec::new();
    let mut per_scope_applied: Vec<(&Scope, Vec<policy::Applied>)> = Vec::new();
    for t in target_scopes {
        let chain = engine.policy_chain(&t.name);
        let (applied, chain_findings) = policy::applicable(catalogues, &chain);
        findings.extend(chain_findings);
        per_scope_applied.push((t, applied));
    }

    let (gates, daemon_fact, credential_rows, backup_fact) = engine.dataset_level_facts(&per_scope_applied).await?;
    let mut statuses_by_scope = BTreeMap::new();
    for (t, applied) in &per_scope_applied {
        let evidence = engine
            .evidence_for_scope(
                snapshot,
                t,
                applied,
                tags,
                all_attestations,
                &gates,
                daemon_fact,
                &credential_rows,
                backup_fact.clone(),
            )
            .await?;
        findings.extend(policy::evidence_findings(&evidence, &t.name));
        let statuses = policy::evaluate(applied, &evidence, now);
        statuses_by_scope.insert(t.name.clone(), statuses);
    }
    Ok((statuses_by_scope, findings))
}

/// The scenario side of the same evaluation -- `scenario_obj`'s own overlay
/// (`scenario::overlay_chain`), built fresh from each scope's own real
/// chain, evaluated against `catalogues_with_drafts` (the real catalogues
/// plus whatever `.factory/policies/drafts/` resolved, `scenario::merge_catalogues`).
/// Returns the overlay's own `scenario::Finding`s (e.g.
/// `DropNotApplicableHasNoEffect`) alongside the `policy::Finding`s
/// `evaluate_baseline_over_scopes` also produces, since the two are
/// different types naming different things.
#[allow(clippy::too_many_arguments)]
async fn evaluate_scenario_over_scopes(
    engine: &Engine,
    snapshot: &Factory,
    target_scopes: &[Scope],
    scenario_obj: &Scenario,
    catalogues_with_drafts: &[policy::Catalogue],
    tags: &BTreeSet<String>,
    all_attestations: &[Attestation],
    now: DateTime<Utc>,
) -> Result<(BTreeMap<String, Vec<policy::ControlStatus>>, Vec<policy::Finding>, Vec<scenario::Finding>)> {
    let mut policy_findings = Vec::new();
    let mut scenario_findings = Vec::new();
    let mut per_scope_applied: Vec<(&Scope, Vec<policy::Applied>)> = Vec::new();
    for t in target_scopes {
        let base_chain = engine.policy_chain(&t.name);
        let (overlaid_chain, overlay_findings) = scenario::overlay_chain(&base_chain, scenario_obj);
        scenario_findings.extend(overlay_findings);
        let (applied, chain_findings) = policy::applicable(catalogues_with_drafts, &overlaid_chain);
        policy_findings.extend(chain_findings);
        per_scope_applied.push((t, applied));
    }

    let (gates, daemon_fact, credential_rows, backup_fact) = engine.dataset_level_facts(&per_scope_applied).await?;
    let mut statuses_by_scope = BTreeMap::new();
    for (t, applied) in &per_scope_applied {
        let evidence = engine
            .evidence_for_scope(
                snapshot,
                t,
                applied,
                tags,
                all_attestations,
                &gates,
                daemon_fact,
                &credential_rows,
                backup_fact.clone(),
            )
            .await?;
        policy_findings.extend(policy::evidence_findings(&evidence, &t.name));
        let statuses = policy::evaluate(applied, &evidence, now);
        statuses_by_scope.insert(t.name.clone(), statuses);
    }
    Ok((statuses_by_scope, policy_findings, scenario_findings))
}

/// Turn matching per-scope status maps into a scenario's own delta: one
/// `ScenarioScopeDelta` per scope where either side has anything applicable
/// at all (omitted otherwise, the same "nothing to show" rule
/// `PolicyReport::rows` follows), and the subtree-wide delta built the same
/// way `PolicyReport::rollup` aggregates -- `policy::worst_across_scopes` on
/// each side, then `scenario::policy_delta` over the two rollups. Pure: no
/// I/O, callers already hold both status maps.
fn deltas_from_statuses(
    target_scopes: &[Scope],
    baseline: &BTreeMap<String, Vec<policy::ControlStatus>>,
    scenario_statuses: &BTreeMap<String, Vec<policy::ControlStatus>>,
) -> (Vec<ScenarioScopeDelta>, scenario::PolicyDelta) {
    let mut per_scope = Vec::new();
    let mut baseline_all = Vec::new();
    let mut scenario_all = Vec::new();
    for t in target_scopes {
        let empty = Vec::new();
        let base = baseline.get(&t.name).unwrap_or(&empty);
        let scen = scenario_statuses.get(&t.name).unwrap_or(&empty);
        if !base.is_empty() || !scen.is_empty() {
            per_scope.push(ScenarioScopeDelta {
                scope: t.name.clone(),
                delta: scenario::policy_delta(base, scen),
            });
        }
        baseline_all.push(base.clone());
        scenario_all.push(scen.clone());
    }
    let subtree = scenario::policy_delta(&policy::worst_across_scopes(&baseline_all), &policy::worst_across_scopes(&scenario_all));
    (per_scope, subtree)
}

/// Non-terminal tasks labelled `goal=<objective>/<kr>` for any of
/// `goals`'s own entries, summed -- unscoped, the same convention
/// `crate::metrics`'s own `goal_tasks_done` reads tasks under: a key
/// result's own scope (if any) is not the scope its serving tasks run in.
fn open_goal_task_count(tasks: &[Task], goal_changes: &[scenario::GoalChange]) -> usize {
    let labels: BTreeSet<String> = goal_changes.iter().map(|c| c.kr.to_string()).collect();
    tasks
        .iter()
        .filter(|t| !t.status.is_terminal() && t.labels.get("goal").is_some_and(|l| labels.contains(l)))
        .count()
}

/// Open tasks in the target scopes: the backlog a forecast burns down. A
/// task blocked by a failure is open work (`#122`); a closed one is not.
fn open_backlog(tasks: &[Task], target_names: &BTreeSet<&str>) -> f64 {
    tasks
        .iter()
        .filter(|t| !t.status.is_terminal() && target_names.contains(t.scope.as_str()))
        .count() as f64
}

impl Engine {
    /// The subtree-wide daily production grid `scenarios_report` samples
    /// throughput history from. `Engine::production`'s own `scope` filter is
    /// *exact*-match only -- "matches the scope named in the request,
    /// nothing wider" (`production.rs`'s own doc comment) -- unlike every
    /// other scope filter in this module, which rolls up a whole subtree.
    /// So a request scoped to an ancestor (`scope: Some("projects")`, work
    /// actually running in `projects/demo`) sums every target scope's own
    /// exact-match daily grid, element-wise, rather than asking `production`
    /// once for the ancestor alone and silently missing every descendant's
    /// throughput -- each scope's own `daily` is the same fixed,
    /// date-anchored 371-day grid (`production.rs`'s own "53 weeks, always"
    /// rule), so the sum lines up index by index without re-deriving
    /// anything. `asked: None` (the whole instance) is the one unscoped call
    /// `Engine::production` itself already answers correctly.
    async fn subtree_daily(self: &Arc<Self>, target_scopes: &[Scope], asked: Option<&str>) -> Result<Vec<factory_core::protocol::ProductionBucket>> {
        if asked.is_none() {
            return Ok(self.production(None, None, None).await?.daily);
        }
        let mut summed: Vec<factory_core::protocol::ProductionBucket> = Vec::new();
        for t in target_scopes {
            let scope_daily = self.production(None, None, Some(t.name.clone())).await?.daily;
            if summed.is_empty() {
                summed = scope_daily;
            } else {
                for (acc, d) in summed.iter_mut().zip(scope_daily.iter()) {
                    acc.finished += d.finished;
                    acc.scrapped += d.scrapped;
                    acc.reworked += d.reworked;
                    acc.first_pass += d.first_pass;
                }
            }
        }
        Ok(summed)
    }

    /// The L6 Scenarios tab: `Request::Scenarios`.
    ///
    /// ## Deviations, documented like `factory_core::scenario`'s own
    ///
    /// 1. **Baseline forecast** (`ScenarioBaseline::forecast`) is
    ///    `forecast_completion` over the asked subtree's own weekly
    ///    throughput history, backlog = every non-terminal task in the
    ///    subtree right now, `Horizon::default()`'s 26 weeks -- chosen over
    ///    a bare metric trend (`forecast_metric`) so it is the *same*
    ///    `Forecast` shape every `ScenarioResult::forecast` carries, and a
    ///    fan chart can draw the baseline band and a scenario's band on one
    ///    axis rather than two different ones.
    /// 2. **A scenario's own backlog** is `newly_open` controls from its
    ///    subtree-wide policy delta (one remediation item each) plus every
    ///    non-terminal task labelled for one of its own `goals:` key
    ///    results -- `newly_stale` is deliberately excluded: stale evidence
    ///    needs refreshing, which is real work, but a different kind from a
    ///    from-scratch remediation, and folding it into the same count
    ///    would make "backlog" mean two different sizes of thing at once.
    ///    `newly_stale` is still visible in the delta itself.
    /// 3. **Tornado is always against `effective_throughput`** -- v1's only
    ///    computed outcome (`scenario::evaluate_outcomes`); "the scenario's
    ///    key outcome" has nothing else to name yet.
    /// 4. **A `goals:` entry's effective `target`/`by`** default to the key
    ///    result's own authored `target` and its cycle's own `ends_on()`
    ///    when the change leaves either unset -- surfaced on
    ///    `ScenarioGoalProbability` so a card never has to re-resolve what
    ///    "unwritten" defaulted to.
    /// 5. **A manual key result is always `KrShape::Ratio`** -- it has no
    ///    metric id for `KrShape::for_metric` to read a direction from, and
    ///    `Ratio` is the conservative default that never fabricates a
    ///    probability (see `KrShape`'s own doc comment).
    pub(crate) async fn scenarios_report(self: &Arc<Self>, scope: Option<&str>) -> Result<ScenariosReport> {
        let now = Utc::now();
        let snapshot = self.factory_snapshot();

        let scenarios_dir = scenario::scenarios_dir(&snapshot.root);
        let (scenarios, mut scenario_findings) = {
            let dir = scenarios_dir.clone();
            tokio::task::spawn_blocking(move || scenario::load(&dir))
                .await
                .map_err(|e| FactoryError::Other(anyhow::anyhow!("scenario directory walk: {e}")))?
        };
        scenario_findings.extend(scenario::stale_findings(&scenarios, now));

        let (real_catalogues, mut policy_findings, tags) = self.load_catalogues_and_tags().await?;
        let (draft_catalogues, draft_findings) = {
            let dir = scenario::drafts_dir(&snapshot.root);
            tokio::task::spawn_blocking(move || scenario::load_drafts(&dir))
                .await
                .map_err(|e| FactoryError::Other(anyhow::anyhow!("draft policy directory walk: {e}")))?
        };
        policy_findings.extend(draft_findings);
        let (catalogues_with_drafts, merge_findings) = scenario::merge_catalogues(&real_catalogues, draft_catalogues);
        scenario_findings.extend(merge_findings);

        let goals_catalogue = {
            let dir = goals::goals_dir(&snapshot.root);
            tokio::task::spawn_blocking(move || goals::load(&dir))
                .await
                .map_err(|e| FactoryError::Other(anyhow::anyhow!("goals directory walk: {e}")))?
        };
        let checkins = self.goals.all().await?;

        let (asked, target_scopes) = subtree_scopes(&snapshot, scope)?;
        let all_attestations = self.policies.all().await?;
        let tasks = self.store.list(&TaskFilter::default()).await?;

        // Every metric any loaded scenario's drivers, signposts or goal
        // changes reference, computed once and shared by every scenario --
        // the same "compute once, project many ways" shape
        // `dataset_level_facts` already follows for policy facts.
        let mut metric_ids: Vec<MetricId> = Vec::new();
        for def in scenario::driver_defs() {
            if let Some(name) = def.metric {
                if let Ok(id) = MetricId::new(name) {
                    metric_ids.push(id);
                }
            }
        }
        for s in &scenarios {
            for sp in &s.signposts {
                // `crate::metrics::push_if_known` -- the same filter
                // `goals_metric_ids` applies to a goals catalogue's own
                // metric references, reused here so a scenario author's
                // typo in a `signposts:`/`goals:` metric name is a finding
                // (`scenario::load`'s own `UnknownMetric`), never a
                // `BadRequest` that refuses the whole report -- unlike
                // `Engine::metrics` itself, which refuses a call naming even
                // one truly unknown id.
                crate::metrics::push_if_known(&mut metric_ids, &sp.metric);
            }
            for change in &s.goals {
                if let Some((_, objective, kd)) = find_kr(&goals_catalogue, &change.kr) {
                    if !kd.manual {
                        if let Some(m) = kd.bound_metric(&objective.id) {
                            crate::metrics::push_if_known(&mut metric_ids, &m);
                        }
                    }
                }
            }
        }
        let computed = self.metrics(&metric_ids, now).await?;
        let values: BTreeMap<MetricId, MetricValue> = computed.values.iter().map(|v| (v.id.clone(), v.clone())).collect();

        let baseline_drivers = driver_baseline(&values);

        // The plain baseline -- no scenario, no overlay -- once, shared by
        // `ScenarioBaseline::policy` and by every scenario's own delta.
        let (baseline_statuses, baseline_policy_findings) =
            evaluate_baseline_over_scopes(self, &snapshot, &target_scopes, &real_catalogues, &tags, &all_attestations, now).await?;
        policy_findings.extend(baseline_policy_findings);
        let baseline_rollup = policy::rollup(&policy::worst_across_scopes(&baseline_statuses.values().cloned().collect::<Vec<_>>()));

        let target_names: BTreeSet<&str> = target_scopes.iter().map(|s| s.name.as_str()).collect();
        let baseline_backlog = open_backlog(&tasks, &target_names);
        let daily = self.subtree_daily(&target_scopes, asked.as_ref().map(|s| s.name.as_str())).await?;
        let history = weekly_throughput_history(&daily, THROUGHPUT_HISTORY_WEEKS);
        let baseline_forecast = scenario::forecast_completion(
            &history,
            baseline_backlog,
            scenario::Horizon::default().weeks(),
            SAMPLES,
            scenario::seed_from(&["baseline", asked.as_ref().map(|s| s.name.as_str()).unwrap_or("instance")]),
        );

        let mut results = Vec::with_capacity(scenarios.len());
        let mut triggered = Vec::new();
        for s in &scenarios {
            let (scenario_statuses, s_policy_findings, s_scenario_findings) =
                evaluate_scenario_over_scopes(self, &snapshot, &target_scopes, s, &catalogues_with_drafts, &tags, &all_attestations, now).await?;
            policy_findings.extend(s_policy_findings);
            scenario_findings.extend(s_scenario_findings);

            let (per_scope_deltas, subtree_delta) = deltas_from_statuses(&target_scopes, &baseline_statuses, &scenario_statuses);

            let overrides = s.driver_overrides();
            let overridden_drivers = scenario::apply_overrides(&baseline_drivers, &overrides);
            let outcomes_before = scenario::evaluate_outcomes(&baseline_drivers);
            let outcomes_after = scenario::evaluate_outcomes(&overridden_drivers);
            let tornado = scenario::tornado(&overridden_drivers, KEY_OUTCOME, TORNADO_SWING);

            let newly_open_controls = subtree_delta.newly_open.len();
            let open_goal_tasks = open_goal_task_count(&tasks, &s.goals);
            let backlog = ScenarioBacklog {
                total: (newly_open_controls + open_goal_tasks) as f64,
                newly_open_controls,
                open_goal_tasks,
            };

            let factor = throughput_scale_factor(&baseline_drivers, &overridden_drivers);
            let scaled_history = scale_history(&history, factor);
            let forecast = scenario::forecast_completion(
                &scaled_history,
                backlog.total,
                s.horizon.weeks(),
                SAMPLES,
                scenario::seed_from(&[s.name.as_str(), "forecast"]),
            );

            let goal_results: Vec<ScenarioGoalProbability> = s
                .goals
                .iter()
                .map(|change| build_goal_probability(change, &goals_catalogue, &values, &checkins, now, &scaled_history, scenario::seed_from(&[s.name.as_str(), "goal", &change.kr.to_string()])))
                .collect();

            let signposts = scenario::evaluate_signposts(&s.signposts, &values, now);
            triggered.extend(triggered_of(s, &signposts));

            let subject = format!("{}.yaml", s.name);
            let own_findings: Vec<scenario::Finding> = scenario_findings.iter().filter(|f| f.subject == subject).cloned().collect();

            results.push(ScenarioResult {
                scenario: s.clone(),
                findings: own_findings,
                policy: per_scope_deltas,
                policy_subtree: subtree_delta,
                drivers: ScenarioDrivers {
                    overridden: overridden_drivers,
                    outcomes_before,
                    outcomes_after,
                    tornado,
                },
                backlog,
                forecast,
                goals: goal_results,
                signposts,
            });
        }

        scenario_findings.sort_by(|a, b| a.subject.cmp(&b.subject).then(a.kind.cmp(&b.kind)).then(a.detail.cmp(&b.detail)));
        scenario_findings.dedup();
        policy_findings.sort_by(|a, b| a.subject.cmp(&b.subject).then(a.kind.cmp(&b.kind)).then(a.detail.cmp(&b.detail)));
        policy_findings.dedup();

        Ok(ScenariosReport {
            scope: asked.as_ref().map(|s| s.name.clone()),
            baseline: ScenarioBaseline {
                metrics: computed.values,
                drivers: baseline_drivers,
                policy: baseline_rollup,
                forecast: baseline_forecast,
            },
            scenarios: results,
            findings: scenario_findings,
            policy_findings,
            triggered,
        })
    }

    /// Every signpost that is past its threshold right now, across every
    /// scenario on disk -- `ScenariosReport::triggered` without the rest of
    /// the report. For the Operations attention queue (`#106`), which reads
    /// it on every call and so cannot afford what the full report costs:
    /// the policy overlay per scope and a Monte Carlo forecast per scenario.
    /// Only the metrics the signposts name are computed, and none at all
    /// when no scenario sets one. The same [`triggered_of`] the full report
    /// uses, over the same `Engine::metrics`, so the two cannot disagree.
    pub(crate) async fn triggered_signposts(self: &Arc<Self>, now: DateTime<Utc>) -> Result<Vec<TriggeredSignpost>> {
        let dir = scenario::scenarios_dir(&self.factory_snapshot().root);
        let (scenarios, _) = tokio::task::spawn_blocking(move || scenario::load(&dir))
            .await
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("scenario directory walk: {e}")))?;
        let mut metric_ids: Vec<MetricId> = Vec::new();
        for s in &scenarios {
            for sp in &s.signposts {
                crate::metrics::push_if_known(&mut metric_ids, &sp.metric);
            }
        }
        if metric_ids.is_empty() {
            return Ok(Vec::new());
        }
        let computed = self.metrics(&metric_ids, now).await?;
        let values: BTreeMap<MetricId, MetricValue> = computed.values.into_iter().map(|v| (v.id.clone(), v)).collect();
        Ok(scenarios
            .iter()
            .flat_map(|s| triggered_of(s, &scenario::evaluate_signposts(&s.signposts, &values, now)))
            .collect())
    }

    /// Turn a scenario into real work: `Request::ScenarioPromote`. One task
    /// per newly-open control in `scope`'s own slice of `scenario`'s policy
    /// delta, skipping a control that already has a non-terminal task
    /// labelled `policy=<framework>/<id>` in `scope` -- the same rule
    /// `policy_remediate` refuses a second call under, except promote
    /// silently skips rather than refusing outright, since a promote is
    /// asking about many controls at once and one of them already having an
    /// open task is the ordinary case, not a mistake to stop the whole
    /// action over. `access.rs` checks `task.create` in `scope` before this
    /// ever runs, the same reach rule `Request::TaskCreate` itself is
    /// checked against.
    pub(crate) async fn scenario_promote(&self, scenario_name: String, scope: String, agent: Option<String>) -> Result<ScenarioPromoteResult> {
        let now = Utc::now();
        let snapshot = self.factory_snapshot();
        let scope_obj = snapshot.scope(&scope)?.clone();

        let (scenarios, _findings) = {
            let dir = scenario::scenarios_dir(&snapshot.root);
            tokio::task::spawn_blocking(move || scenario::load(&dir))
                .await
                .map_err(|e| FactoryError::Other(anyhow::anyhow!("scenario directory walk: {e}")))?
        };
        let s = scenarios
            .into_iter()
            .find(|s| s.name == scenario_name)
            .ok_or_else(|| FactoryError::BadRequest(format!("no such scenario: {scenario_name:?}")))?;

        let (real_catalogues, _pf, tags) = self.load_catalogues_and_tags().await?;
        let (draft_catalogues, _df) = {
            let dir = scenario::drafts_dir(&snapshot.root);
            tokio::task::spawn_blocking(move || scenario::load_drafts(&dir))
                .await
                .map_err(|e| FactoryError::Other(anyhow::anyhow!("draft policy directory walk: {e}")))?
        };
        let (catalogues_with_drafts, _mf) = scenario::merge_catalogues(&real_catalogues, draft_catalogues);

        let base_chain = self.policy_chain(&scope_obj.name);
        let (baseline_applied, _cf1) = policy::applicable(&real_catalogues, &base_chain);
        let (overlaid_chain, _of) = scenario::overlay_chain(&base_chain, &s);
        let (scenario_applied, _cf2) = policy::applicable(&catalogues_with_drafts, &overlaid_chain);

        let all_attestations = self.policies.all().await?;
        let per_scope_applied = vec![(&scope_obj, scenario_applied.clone())];
        let (gates, daemon_fact, credential_rows, backup_fact) = self.dataset_level_facts(&per_scope_applied).await?;
        let evidence = self
            .evidence_for_scope(
                &snapshot,
                &scope_obj,
                &scenario_applied,
                &tags,
                &all_attestations,
                &gates,
                daemon_fact,
                &credential_rows,
                backup_fact,
            )
            .await?;

        let baseline_statuses = policy::evaluate(&baseline_applied, &evidence, now);
        let scenario_statuses = policy::evaluate(&scenario_applied, &evidence, now);
        let delta = scenario::policy_delta(&baseline_statuses, &scenario_statuses);

        let existing_tasks = self
            .store
            .list(&TaskFilter { scope: Some(scope_obj.name.clone()), ..Default::default() })
            .await?;

        let mut created = Vec::new();
        let mut skipped = Vec::new();
        for control in &delta.newly_open {
            let label = control.to_string();
            if let Some(existing) = existing_tasks
                .iter()
                .find(|t| !t.status.is_terminal() && t.labels.get("policy").map(String::as_str) == Some(label.as_str()))
            {
                skipped.push(SkippedControl { control: control.clone(), existing_task: existing.id.clone() });
                continue;
            }

            let applied_entry = scenario_applied
                .iter()
                .find(|a| &a.control == control)
                .expect("a newly_open control is always in the scenario's own applied set");
            let status_entry = scenario_statuses
                .iter()
                .find(|status| &status.control == control)
                .expect("a newly_open control is always in the scenario's own evaluated statuses");

            let mut labels = BTreeMap::new();
            labels.insert("policy".to_string(), label);
            labels.insert("scenario".to_string(), s.name.clone());
            let new_task = NewTask {
                title: format!("Prepare {control} for scenario {}: {}", s.name, applied_entry.title),
                instructions: policy::remediation_instructions(control, applied_entry.remediation.as_deref(), &status_entry.status, &applied_entry.evidence),
                scope: Some(scope_obj.name.clone()),
                agent: agent.clone(),
                labels,
                ..Default::default()
            };
            let task = self.create(new_task).await?;
            created.push(PromotedControl { control: control.clone(), task });
        }

        Ok(ScenarioPromoteResult {
            scenario: s.name,
            scope: scope_obj.name,
            created,
            skipped,
        })
    }

    /// Recompute driver outcomes, tornado and forecast with slider
    /// overrides applied: `Request::ScenarioWhatIf`. Pure and read-only
    /// (`Needs::Nothing`, `access.rs`) -- nothing here is written, whatever
    /// `scenario` and `drivers` say.
    ///
    /// Layering: the named scenario's own `drivers:` overrides (none, with
    /// `scenario: None`), then `drivers` on top, request wins driver by
    /// driver -- so a UI slider can override one driver a scenario itself
    /// also names without having to resend the scenario's other overrides.
    /// Each entry in `drivers` is parsed with `scenario::parse_override`,
    /// the same authored syntax (`×2`, `+20%`, `+5`, `=0.9`); a value that
    /// does not parse is a `BadRequest` naming the driver, not a finding --
    /// this is typed input from a live request, not an authored file
    /// `load` can leave partly wrong and still serve the rest of.
    ///
    /// Backlog: with `scenario: Some`, the same subtree-wide policy-delta
    /// and goal-task backlog `scenarios_report` computes for that scenario
    /// (over the whole instance -- this request carries no `scope`); with
    /// `scenario: None`, `0.0`, so the forecast is honestly a bare
    /// throughput projection with nothing to clear, not a guessed number.
    /// Recomputing the policy delta here costs the same real work
    /// `scenarios_report` does even though only the driver sliders moved --
    /// backlog genuinely does not depend on drivers, so this is more work
    /// than a slider tick strictly needs, but it is the honest number the
    /// issue's own data shape asks for; the UI is expected to debounce
    /// calls rather than this endpoint pretending backlog is free.
    pub(crate) async fn scenario_whatif(self: &Arc<Self>, scenario_name: Option<String>, raw_drivers: BTreeMap<DriverId, String>) -> Result<ScenarioWhatIfResult> {
        let now = Utc::now();
        let snapshot = self.factory_snapshot();

        let mut overrides: BTreeMap<DriverId, scenario::Override> = BTreeMap::new();
        let mut horizon_weeks = scenario::Horizon::default().weeks();
        let mut backlog_total = 0.0f64;

        if let Some(name) = &scenario_name {
            let (scenarios, _findings) = {
                let dir = scenario::scenarios_dir(&snapshot.root);
                tokio::task::spawn_blocking(move || scenario::load(&dir))
                    .await
                    .map_err(|e| FactoryError::Other(anyhow::anyhow!("scenario directory walk: {e}")))?
            };
            let s = scenarios
                .into_iter()
                .find(|s| &s.name == name)
                .ok_or_else(|| FactoryError::BadRequest(format!("no such scenario: {name:?}")))?;
            overrides = s.driver_overrides();
            horizon_weeks = s.horizon.weeks();

            let (real_catalogues, _pf, tags) = self.load_catalogues_and_tags().await?;
            let (draft_catalogues, _df) = {
                let dir = scenario::drafts_dir(&snapshot.root);
                tokio::task::spawn_blocking(move || scenario::load_drafts(&dir))
                    .await
                    .map_err(|e| FactoryError::Other(anyhow::anyhow!("draft policy directory walk: {e}")))?
            };
            let (catalogues_with_drafts, _mf) = scenario::merge_catalogues(&real_catalogues, draft_catalogues);
            let all_attestations = self.policies.all().await?;
            let (_asked, all_scopes) = subtree_scopes(&snapshot, None)?;

            let (baseline_statuses, _bf) =
                evaluate_baseline_over_scopes(self, &snapshot, &all_scopes, &real_catalogues, &tags, &all_attestations, now).await?;
            let (scenario_statuses, _sf, _svf) =
                evaluate_scenario_over_scopes(self, &snapshot, &all_scopes, &s, &catalogues_with_drafts, &tags, &all_attestations, now).await?;
            let (_per_scope, subtree_delta) = deltas_from_statuses(&all_scopes, &baseline_statuses, &scenario_statuses);

            let tasks = self.store.list(&TaskFilter::default()).await?;
            let open_goal_tasks = open_goal_task_count(&tasks, &s.goals);
            backlog_total = (subtree_delta.newly_open.len() + open_goal_tasks) as f64;
        }

        for (id, raw) in &raw_drivers {
            let ov = scenario::parse_override(raw).map_err(|e| FactoryError::BadRequest(format!("driver {id:?}: {e}")))?;
            overrides.insert(id.clone(), ov);
        }

        let mut metric_ids: Vec<MetricId> = Vec::new();
        for def in scenario::driver_defs() {
            if let Some(name) = def.metric {
                if let Ok(id) = MetricId::new(name) {
                    metric_ids.push(id);
                }
            }
        }
        let computed = self.metrics(&metric_ids, now).await?;
        let values: BTreeMap<MetricId, MetricValue> = computed.values.into_iter().map(|v| (v.id.clone(), v)).collect();
        let baseline_drivers = driver_baseline(&values);

        let overridden_drivers = scenario::apply_overrides(&baseline_drivers, &overrides);
        let outcomes_before = scenario::evaluate_outcomes(&baseline_drivers);
        let outcomes_after = scenario::evaluate_outcomes(&overridden_drivers);
        let tornado = scenario::tornado(&overridden_drivers, KEY_OUTCOME, TORNADO_SWING);

        let daily = self.production(None, None, None).await?.daily;
        let history = weekly_throughput_history(&daily, THROUGHPUT_HISTORY_WEEKS);
        let factor = throughput_scale_factor(&baseline_drivers, &overridden_drivers);
        let scaled_history = scale_history(&history, factor);

        let seed_name = scenario_name.as_deref().unwrap_or("whatif");
        let forecast = scenario::forecast_completion(&scaled_history, backlog_total, horizon_weeks, SAMPLES, scenario::seed_from(&[seed_name, "whatif"]));

        Ok(ScenarioWhatIfResult {
            scenario: scenario_name,
            drivers: ScenarioDrivers {
                overridden: overridden_drivers,
                outcomes_before,
                outcomes_after,
                tornado,
            },
            forecast,
        })
    }
}

#[cfg(test)]
mod tests {
    //! Engine-level scenario reports and writes, on a temporary instance --
    //! not `factory_core::scenario` itself (covered on its own), but this
    //! module glued to a real scope tree, real policy catalogues (one real,
    //! one draft), a real goals cycle, and the store behind `Engine`, the
    //! way a real request sees them.

    use super::*;
    use factory_core::adapter::TaskStore;
    use factory_core::config::{Config, DaemonConfig, Instance, PolicyDeclaration};
    use factory_plugins::{Registry, SqliteStore};
    use std::path::PathBuf;

    /// Since #106 `rework_rate` is the registry metric of that name, not a
    /// hardcoded `0.0`: its baseline is the metric's value, and absent --
    /// nothing to be relative to -- when the metric has none.
    #[test]
    fn the_rework_rate_driver_takes_its_baseline_from_the_registry_metric() {
        let id = MetricId::new("rework_rate").unwrap();
        let mut values = BTreeMap::new();
        values.insert(
            id.clone(),
            MetricValue { id: id.clone(), value: Some(0.2), as_of: chrono::Utc::now(), reason: None },
        );
        assert_eq!(driver_baseline(&values).get("rework_rate"), Some(&0.2));
        assert_eq!(driver_baseline(&BTreeMap::new()).get("rework_rate"), None);
        assert_eq!(driver_baseline(&BTreeMap::new()).get("capacity_factor"), Some(&1.0));
    }

    fn scope_at(id: &str, name: &str, path: &str) -> Scope {
        let mut scope: Scope = serde_yaml_ng::from_str(&format!("id: {id}\nname: {name}\n")).unwrap();
        scope.path = PathBuf::from(path);
        scope
    }

    const CRA_YAML: &str = "framework: cra\n\
         title: Cyber Resilience Act\n\
         kind: regulation\n\
         controls:\n\
         \x20\x20- id: a\n\x20\x20\x20\x20title: Control A\n\x20\x20\x20\x20evidence:\n\x20\x20\x20\x20\x20\x20- check: knowledge\n\
         \x20\x20- id: b\n\x20\x20\x20\x20title: Control B\n\x20\x20\x20\x20evidence:\n\x20\x20\x20\x20\x20\x20- check: attestation\n";

    const AI_ACT_DRAFT_YAML: &str = "framework: ai-act\n\
         title: EU AI Act (draft)\n\
         kind: regulation\n\
         controls:\n\
         \x20\x20- id: oversight\n\x20\x20\x20\x20title: Human oversight\n\x20\x20\x20\x20remediation: |\n\x20\x20\x20\x20\x20\x20Record an attestation.\n\x20\x20\x20\x20evidence:\n\x20\x20\x20\x20\x20\x20- check: attestation\n";

    const SCENARIO_YAML: &str = "name: ai-act-2027\n\
         title: EU AI Act applies from 2027\n\
         kind: [policy, drivers, goals]\n\
         horizon: 13w\n\
         policy:\n\
         \x20\x20add_frameworks: [ai-act]\n\
         drivers:\n\
         \x20\x20capacity_factor: \"-20%\"\n\
         goals:\n\
         \x20\x20- { kr: obj/kr-ratio, by: 2027-01-01 }\n\
         signposts:\n\
         \x20\x20- { metric: throughput_week, below: 999 }\n";

    const GOALS_CYCLE_YAML: &str = "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
         objectives:\n\
         \x20\x20- id: obj\n\x20\x20\x20\x20title: Objective\n\x20\x20\x20\x20key_results:\n\
         \x20\x20\x20\x20\x20\x20- { id: kr-ratio, title: KR, kind: committed, metric: first_pass_yield, baseline: 0, target: 1 }\n";

    /// `company` (`.`) is root and commits the instance to `cra`, exactly
    /// like `policies::tests::test_engine`. `projects` and, below it,
    /// `demo-app` sit under it; `sibling` sits outside `projects` entirely,
    /// to prove a scope query never reaches sideways (the same shape every
    /// other module's own subtree test uses). One scenario,
    /// `ai-act-2027.yaml`, overlays the `ai-act` draft catalogue
    /// (`policies/drafts/ai-act.yaml`), a driver override, one `goals:`
    /// entry naming a real (ratio-shaped) key result, and one signpost that
    /// is always triggered (`throughput_week` starts at `0`, `below: 999`).
    fn test_engine() -> Arc<Engine> {
        let root = std::env::temp_dir().join(format!("factory-scenarios-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".factory/policies/drafts")).unwrap();
        std::fs::write(root.join(".factory/policies/cra.yaml"), CRA_YAML).unwrap();
        std::fs::write(root.join(".factory/policies/drafts/ai-act.yaml"), AI_ACT_DRAFT_YAML).unwrap();

        std::fs::create_dir_all(root.join(".factory/knowledge")).unwrap();
        std::fs::write(root.join(".factory/knowledge/page.md"), "---\ntags: [control/cra/a]\n---\n# Page\n").unwrap();

        std::fs::create_dir_all(root.join(".factory/scenarios")).unwrap();
        std::fs::write(root.join(".factory/scenarios/ai-act-2027.yaml"), SCENARIO_YAML).unwrap();

        std::fs::create_dir_all(root.join(".factory/goals")).unwrap();
        std::fs::write(root.join(".factory/goals/2026-q4.yaml"), GOALS_CYCLE_YAML).unwrap();

        let mut config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig::default(),
            scope: None,
            scopes: Vec::new(),
            roles: Default::default(),
            dashboard: None,
            policies: PolicyDeclaration { frameworks: vec!["cra".to_string()], ..Default::default() },
            quality: Default::default(),
            infrastructure: Default::default(),
            plugins_dir: None,
        };
        config.scopes = vec![
            scope_at("company-id", "company", "."),
            scope_at("projects-id", "projects", "projects"),
            scope_at("demo-app-id", "demo-app", "projects/demo"),
            scope_at("sibling-id", "sibling", "other"),
        ];

        let factory = Factory { root, config };
        let registry = Registry::with_builtins();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        Arc::new(Engine::new(factory, registry, store, PathBuf::from("factory"), Vec::new()))
    }

    fn find<'a>(report: &'a ScenariosReport, name: &str) -> &'a ScenarioResult {
        report.scenarios.iter().find(|s| s.scenario.name == name).unwrap_or_else(|| panic!("no scenario {name:?} in {report:#?}"))
    }

    // -- policy delta: real catalogue + draft --------------------------------

    #[tokio::test]
    async fn policy_delta_is_exact_against_a_real_catalogue_and_a_draft() {
        let engine = test_engine();
        let report = engine.scenarios_report(Some("company")).await.unwrap();
        let scenario = find(&report, "ai-act-2027");

        // The real catalogue's own controls are unaffected: `a` stays
        // satisfied (the knowledge tag), `b` stays open (no attestation) --
        // neither shows up as newly anything.
        let newly_open: Vec<String> = scenario.policy_subtree.newly_open.iter().map(|c| c.to_string()).collect();
        assert_eq!(newly_open, vec!["ai-act/oversight".to_string()], "{:#?}", scenario.policy_subtree);
        assert!(scenario.policy_subtree.newly_applicable_but_covered.is_empty());

        // Every scope in the subtree gets its own delta row, since the
        // draft framework applies everywhere the overlay was evaluated.
        let scopes: std::collections::BTreeSet<&str> = scenario.policy.iter().map(|d| d.scope.as_str()).collect();
        assert_eq!(scopes, std::collections::BTreeSet::from(["company", "projects", "demo-app", "sibling"]));
        for row in &scenario.policy {
            assert_eq!(
                row.delta.newly_open.iter().map(|c| c.to_string()).collect::<Vec<_>>(),
                vec!["ai-act/oversight".to_string()],
                "scope {} should see the same newly-open control",
                row.scope
            );
        }

        // The baseline itself never sees the draft framework at all.
        assert!(report.baseline.policy.iter().all(|r| r.framework != "ai-act"), "{:#?}", report.baseline.policy);
        assert!(report.baseline.policy.iter().any(|r| r.framework == "cra"));

        // A draft that never collided with a real catalogue is not a
        // finding; report-level findings are all about this scenario file
        // or the loader, never a phantom collision.
        assert!(report.findings.iter().all(|f| f.kind != factory_core::scenario::FindingKind::DraftCollidesWithReal));
    }

    #[tokio::test]
    async fn a_draft_that_collides_with_a_real_framework_is_dropped_and_reported() {
        let engine = test_engine();
        // A second draft, alongside `drafts/ai-act.yaml`, whose own
        // `framework` collides with the real `cra` catalogue -- its file
        // stem has to match its own `framework` (`policy::load_all`'s own
        // rule, which `load_drafts` shares) for it to load at all.
        let snapshot = engine.factory_snapshot();
        std::fs::write(
            snapshot.root.join(".factory/policies/drafts/cra.yaml"),
            "framework: cra\ntitle: Fake CRA\nkind: regulation\ncontrols: []\n",
        )
        .unwrap();

        let report = engine.scenarios_report(None).await.unwrap();
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.kind == factory_core::scenario::FindingKind::DraftCollidesWithReal && f.subject == "cra.yaml"),
            "{:#?}",
            report.findings
        );
        // The colliding draft never reaches the merged set at all, but the
        // unrelated `ai-act` draft the scenario actually names still does --
        // one bad draft never stops another from loading, the same
        // "one file never takes the rest down" rule every loader here holds.
        let scenario = find(&report, "ai-act-2027");
        assert_eq!(
            scenario.policy_subtree.newly_open.iter().map(|c| c.to_string()).collect::<Vec<_>>(),
            vec!["ai-act/oversight".to_string()],
            "{:#?}",
            scenario.policy_subtree
        );
    }

    // -- determinism ----------------------------------------------------------

    #[tokio::test]
    async fn the_report_is_deterministic_for_the_same_inputs() {
        let engine = test_engine();
        let a = engine.scenarios_report(Some("company")).await.unwrap();
        let b = engine.scenarios_report(Some("company")).await.unwrap();

        let sa = find(&a, "ai-act-2027");
        let sb = find(&b, "ai-act-2027");
        assert_eq!(sa.forecast.per_week, sb.forecast.per_week);
        assert_eq!(sa.forecast.completion_week, sb.forecast.completion_week);
        assert_eq!(sa.forecast.seed, sb.forecast.seed);
        assert_eq!(sa.drivers.tornado, sb.drivers.tornado);
        assert_eq!(sa.drivers.outcomes_before, sb.drivers.outcomes_before);
        assert_eq!(sa.drivers.outcomes_after, sb.drivers.outcomes_after);
        assert_eq!(sa.goals.len(), sb.goals.len());
        for (ga, gb) in sa.goals.iter().zip(&sb.goals) {
            assert_eq!(ga.probability, gb.probability);
        }
        assert_eq!(sa.backlog.total, sb.backlog.total);
        assert_eq!(a.baseline.forecast.per_week, b.baseline.forecast.per_week);
    }

    // -- goal scenarios: ratio KRs never fabricate a probability -------------

    #[tokio::test]
    async fn a_ratio_key_result_re_scores_to_none_with_a_reason_never_a_number() {
        let engine = test_engine();
        let report = engine.scenarios_report(None).await.unwrap();
        let scenario = find(&report, "ai-act-2027");
        assert_eq!(scenario.goals.len(), 1);
        let g = &scenario.goals[0];
        assert_eq!(g.kr.to_string(), "obj/kr-ratio");
        assert_eq!(g.probability.probability, None);
        assert!(g.probability.reason.is_some(), "{:?}", g.probability);
    }

    // -- signposts --------------------------------------------------------------

    #[tokio::test]
    async fn a_triggered_signpost_is_surfaced_at_the_top_level() {
        let engine = test_engine();
        let report = engine.scenarios_report(None).await.unwrap();
        let scenario = find(&report, "ai-act-2027");
        assert_eq!(scenario.signposts[0].state, scenario::SignpostState::Triggered, "{:?}", scenario.signposts);

        assert!(
            report.triggered.iter().any(|t| t.scenario == "ai-act-2027" && t.metric.as_str() == "throughput_week"),
            "{:#?}",
            report.triggered
        );
    }

    // -- scope filter -----------------------------------------------------------

    #[tokio::test]
    async fn scope_filter_matches_the_asked_scope_and_its_descendants_only() {
        let engine = test_engine();
        let report = engine.scenarios_report(Some("projects")).await.unwrap();
        let scenario = find(&report, "ai-act-2027");
        let scopes: std::collections::BTreeSet<&str> = scenario.policy.iter().map(|d| d.scope.as_str()).collect();
        assert_eq!(
            scopes,
            std::collections::BTreeSet::from(["projects", "demo-app"]),
            "company (an ancestor) and sibling (unrelated) must not appear"
        );
    }

    // -- promote ------------------------------------------------------------

    #[tokio::test]
    async fn promote_creates_a_task_per_newly_open_control_and_skips_it_next_time() {
        let engine = test_engine();
        let result = engine.scenario_promote("ai-act-2027".to_string(), "company".to_string(), None).await.unwrap();

        assert_eq!(result.created.len(), 1, "{:#?}", result.created);
        assert!(result.skipped.is_empty());
        let created = &result.created[0];
        assert_eq!(created.control.to_string(), "ai-act/oversight");
        assert_eq!(created.task.title, "Prepare ai-act/oversight for scenario ai-act-2027: Human oversight");
        assert_eq!(created.task.labels.get("policy").map(String::as_str), Some("ai-act/oversight"));
        assert_eq!(created.task.labels.get("scenario").map(String::as_str), Some("ai-act-2027"));
        assert!(created.task.instructions.contains("Record an attestation"), "{:?}", created.task.instructions);
        assert!(created.task.instructions.contains("Missing evidence"), "{:?}", created.task.instructions);

        // A second promote finds the same open task rather than creating a
        // duplicate.
        let again = engine.scenario_promote("ai-act-2027".to_string(), "company".to_string(), None).await.unwrap();
        assert!(again.created.is_empty(), "{:#?}", again.created);
        assert_eq!(again.skipped.len(), 1);
        assert_eq!(again.skipped[0].control.to_string(), "ai-act/oversight");
        assert_eq!(again.skipped[0].existing_task, created.task.id);
    }

    /// `#122`: a promoted task whose run failed is blocked, not closed, so
    /// the next promote still skips it rather than making a duplicate.
    #[tokio::test]
    async fn promote_skips_a_failed_task_rather_than_creating_a_duplicate() {
        let engine = test_engine();
        let result = engine.scenario_promote("ai-act-2027".to_string(), "company".to_string(), None).await.unwrap();
        let created = &result.created[0];
        engine.fail_task_for_test(&created.task.id, factory_core::run::FailKind::RunTimeout).await;

        let again = engine.scenario_promote("ai-act-2027".to_string(), "company".to_string(), None).await.unwrap();
        assert!(again.created.is_empty(), "{:#?}", again.created);
        assert_eq!(again.skipped.len(), 1);
        assert_eq!(again.skipped[0].existing_task, created.task.id);
    }

    /// `#122`: the goal-task count and the baseline backlog both count a
    /// task blocked by a failure as open work -- it is -- and a closed one
    /// as not.
    #[test]
    fn open_work_counts_a_failed_task_and_not_a_closed_one() {
        use factory_core::task::TaskStatus;
        let task = |id: &str, status: TaskStatus, failed: bool| {
            let mut t = factory_core::adapter::store::task_from_new(
                factory_core::task::NewTask { title: id.into(), ..Default::default() },
                "demo".into(),
                "shell".into(),
                "herdr".into(),
            );
            t.id = id.into();
            t.status = status;
            t.labels.insert("goal".into(), "ship/kr1".into());
            if failed {
                t.failure = Some(factory_core::task::TaskFailure {
                    kind: Some(factory_core::run::FailKind::AgentFailed),
                    run_id: Some("r".into()),
                    attempt: Some(1),
                    at: chrono::Utc::now(),
                });
            }
            t
        };
        let tasks = vec![
            task("failed", TaskStatus::Blocked, true),
            task("done", TaskStatus::Done, false),
            task("wont", TaskStatus::Cancelled, false),
            task("pending", TaskStatus::Pending, false),
        ];
        let change = scenario::GoalChange { kr: "ship/kr1".parse().unwrap(), target: None, by: None };
        assert_eq!(open_goal_task_count(&tasks, &[change]), 2);
        let targets = BTreeSet::from(["demo"]);
        assert_eq!(open_backlog(&tasks, &targets), 2.0);
    }

    #[tokio::test]
    async fn promote_refuses_an_unknown_scenario() {
        let engine = test_engine();
        let err = engine.scenario_promote("nope".to_string(), "company".to_string(), None).await.unwrap_err();
        assert!(err.to_string().contains("nope"), "{err}");
    }

    // -- subtree_daily: throughput from a descendant reaches an ancestor's own report --

    async fn finished_run_in(engine: &Arc<Engine>, scope: &str, label: &str) {
        let new_task = factory_core::adapter::store::task_from_new(
            NewTask { title: label.to_string(), ..Default::default() },
            scope.to_string(),
            "assistant".to_string(),
            "shell".to_string(),
        );
        let task = engine.store.create(&new_task).await.unwrap();
        let run = engine
            .store
            .create_run(&factory_core::run::NewRun {
                task_id: task.id.clone(),
                trigger: factory_core::run::Trigger::Manual,
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
                &factory_core::run::RunPatch {
                    status: Some(factory_core::run::RunStatus::Done),
                    ended_at: Some(Utc::now() - chrono::Duration::hours(1)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }

    /// `Engine::production`'s own scope filter is exact-match only; a
    /// request scoped to `projects` (an ancestor of `demo-app`, the scope
    /// that actually did the work) must still see that throughput in its
    /// own baseline forecast -- see `Engine::subtree_daily`'s own doc
    /// comment for why summing per target scope is what makes that true,
    /// unlike a single `production(scope: "projects")` call.
    #[tokio::test]
    async fn a_scope_scoped_report_sees_a_descendants_own_throughput() {
        let engine = test_engine();
        for i in 0..3 {
            finished_run_in(&engine, "demo-app", &format!("run-{i}")).await;
        }

        let (_asked, projects_subtree) = crate::policies::subtree_scopes(&engine.factory_snapshot(), Some("projects")).unwrap();
        let daily = engine.subtree_daily(&projects_subtree, Some("projects")).await.unwrap();
        let total: u32 = daily.iter().map(|b| b.finished).sum();
        assert_eq!(total, 3, "the ancestor's own subtree_daily must include its descendant's throughput");

        // Pin the bug this method exists to avoid regressing: an exact-match
        // call at the ancestor alone sees none of it.
        let exact_only = engine.production(None, None, Some("projects".to_string())).await.unwrap().daily;
        let exact_total: u32 = exact_only.iter().map(|b| b.finished).sum();
        assert_eq!(exact_total, 0, "an exact-match production call at the ancestor alone sees none of the descendant's work");

        // And it shows up in the real report's own baseline forecast, not
        // just in the grid this test reaches into directly.
        let report = engine.scenarios_report(Some("projects")).await.unwrap();
        assert!(report.baseline.forecast.reason.is_none(), "{:?}", report.baseline.forecast.reason);
    }

    // -- whatif ---------------------------------------------------------------

    #[tokio::test]
    async fn whatif_layers_request_drivers_over_the_named_scenarios_own() {
        let engine = test_engine();

        // With no request-level override, the scenario's own "-20%" holds.
        let named = engine.scenario_whatif(Some("ai-act-2027".to_string()), Default::default()).await.unwrap();
        let baseline_capacity = 1.0; // capacity_factor's own neutral baseline
        assert!((named.drivers.overridden["capacity_factor"] - baseline_capacity * 0.8).abs() < 1e-9, "{:#?}", named.drivers);

        // A request-level override for the same driver wins outright.
        let mut overrides = BTreeMap::new();
        overrides.insert("capacity_factor".to_string(), "=2.0".to_string());
        let overridden = engine.scenario_whatif(Some("ai-act-2027".to_string()), overrides).await.unwrap();
        assert_eq!(overridden.drivers.overridden["capacity_factor"], 2.0);

        // With no scenario named at all, only the request's own overrides
        // apply, and the forecast has nothing to clear.
        let mut bare = BTreeMap::new();
        bare.insert("capacity_factor".to_string(), "=3.0".to_string());
        let none_named = engine.scenario_whatif(None, bare).await.unwrap();
        assert_eq!(none_named.drivers.overridden["capacity_factor"], 3.0);
        assert_eq!(none_named.forecast.completion_week.p50, Some(0), "nothing to clear with no scenario named");
    }

    #[tokio::test]
    async fn whatif_refuses_a_bad_override_syntax() {
        let engine = test_engine();
        let mut overrides = BTreeMap::new();
        overrides.insert("capacity_factor".to_string(), "banana".to_string());
        let err = engine.scenario_whatif(None, overrides).await.unwrap_err();
        assert!(err.to_string().contains("capacity_factor"), "{err}");
    }

    // -- #156: backup metrics as signposts ---------------------------------
    //
    // `backup_age_hours`/`backup_verified_age_days` (`#154`) are registry
    // metrics like any other -- no scenario code changed to support them.
    // These are lock tests proving the wiring holds: a scenario naming one
    // loads with no `UnknownMetric` finding, and `evaluate_signpost` (tested
    // on its own in `factory_core::scenario`) reads a real value through it.

    /// A throwaway instance with `infrastructure.backup` configured and one
    /// scenario, `backup-watch`, whose only signpost is
    /// `{ metric: backup_age_hours, above: 30 }`.
    fn backup_signpost_test_engine(destination: &str) -> Arc<Engine> {
        let base = std::env::temp_dir().join(format!("factory-scenarios-backup-signpost-{}", uuid::Uuid::new_v4()));
        let root = base.join("instance");
        std::fs::create_dir_all(root.join(".factory/knowledge")).unwrap();
        std::fs::write(root.join(".factory/config.yaml"), "version: 1\ninstance:\n  id: test\n  name: test\n").unwrap();
        std::fs::write(root.join(".factory/knowledge/page.md"), "# A page\n").unwrap();
        std::fs::create_dir_all(root.join(".factory/scenarios")).unwrap();
        std::fs::write(
            root.join(".factory/scenarios/backup-watch.yaml"),
            "name: backup-watch\ntitle: Backup age\nsignposts:\n  - { metric: backup_age_hours, above: 30 }\n",
        )
        .unwrap();
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
        };
        let factory = Factory { root, config };
        let engine = Engine::new(factory, Registry::with_builtins(), store, PathBuf::from("factory"), Vec::new())
            .with_backup_store(crate::backup::BackupStore::open(&database).unwrap());
        Arc::new(engine)
    }

    #[tokio::test]
    async fn a_signpost_on_backup_age_hours_loads_with_no_unknown_metric_finding_and_is_quiet_for_a_fresh_backup() {
        let engine = backup_signpost_test_engine("destination");
        engine.backup_run(factory_core::backup::BackupTrigger::Manual, "owner".into()).await.unwrap();

        let report = engine.scenarios_report(None).await.unwrap();
        assert!(
            !report.findings.iter().any(|f| f.kind == scenario::FindingKind::UnknownMetric),
            "{:?}",
            report.findings
        );
        let result = find(&report, "backup-watch");
        let sp = result.signposts.iter().find(|s| s.metric.as_str() == "backup_age_hours").unwrap();
        assert_eq!(sp.state, scenario::SignpostState::Quiet, "{sp:?}");
        assert!(!report.triggered.iter().any(|t| t.scenario == "backup-watch"), "{:?}", report.triggered);
    }

    #[tokio::test]
    async fn a_signpost_on_backup_age_hours_triggers_once_the_newest_snapshot_is_old_enough() {
        let engine = backup_signpost_test_engine("destination");
        let snapshot = engine.backup_run(factory_core::backup::BackupTrigger::Manual, "owner".into()).await.unwrap();

        // `list_archives` reads a snapshot's age off its file name, never
        // its mtime, so renaming it back 31 hours -- past the signpost's
        // `above: 30` -- is enough, the same way `#152`'s own
        // `retention_prunes_across_a_mixed_plaintext_and_encrypted_history`
        // fixture plants an aged archive.
        let old_name = factory_core::backup::archive_name("test", snapshot.at - chrono::Duration::hours(31), false);
        let destination = snapshot.path.rsplit_once('/').unwrap().0.to_string();
        std::fs::rename(&snapshot.path, format!("{destination}/{old_name}")).unwrap();

        let report = engine.scenarios_report(None).await.unwrap();
        let result = find(&report, "backup-watch");
        let sp = result.signposts.iter().find(|s| s.metric.as_str() == "backup_age_hours").unwrap();
        assert_eq!(sp.state, scenario::SignpostState::Triggered, "{sp:?}");
        assert!(report.triggered.iter().any(|t| t.scenario == "backup-watch"), "{:?}", report.triggered);

        let live = engine.triggered_signposts(chrono::Utc::now()).await.unwrap();
        assert!(live.iter().any(|t| t.scenario == "backup-watch"), "{live:?}");
    }
}
