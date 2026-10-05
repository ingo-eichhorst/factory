//! The actual L6 Scenarios request owner. Authored reads and projections
//! stay here; live measurements cross typed L0 fact ports. Explicit request
//! phases preserve historical outside-only metric error preflights without
//! a callback or a precomputed metric report entering this service.
use crate::{
    goals::{self, KrRef},
    goals_store::GoalsStore,
    policy::{self, Attestation},
    policy_intent::{self, Scope},
    policy_service::{classified, inputs},
    remediation,
    scenario::{self, DriverId, Scenario},
    scenarios_view::*,
};
use chrono::{DateTime, Utc};
use factory_assurance::{
    check_evaluation,
    evidence::BudgetIntent,
    metrics::{self, MetricError},
};
use factory_kernel::{
    resolve_scope, scope_subtree, CheckComparisonFact, CheckEvaluationFact, FactoryError, Facts,
    KnowledgeTags, MetricId, MetricValue, MetricValuesFact, ProductionBin, ProductionFact,
    ProductionQuery, Provide, Result, TaskInventoryFact, TaskInventoryQuery, L6,
};
use std::collections::{BTreeMap, BTreeSet};

pub trait Knowledge:
    Provide<KnowledgeTags, Query = (), Value = KnowledgeTags, Error = FactoryError>
{
}
impl<T: Provide<KnowledgeTags, Query = (), Value = KnowledgeTags, Error = FactoryError>> Knowledge
    for T
{
}
pub trait Inventory:
    Provide<
    TaskInventoryFact,
    Query = TaskInventoryQuery,
    Value = Vec<TaskInventoryFact>,
    Error = FactoryError,
>
{
}
impl<
        T: Provide<
            TaskInventoryFact,
            Query = TaskInventoryQuery,
            Value = Vec<TaskInventoryFact>,
            Error = FactoryError,
        >,
    > Inventory for T
{
}
pub trait Checks:
    Provide<
    CheckEvaluationFact,
    Query = check_evaluation::Read,
    Value = CheckEvaluationFact,
    Error = FactoryError,
>
{
}
impl<
        T: Provide<
            CheckEvaluationFact,
            Query = check_evaluation::Read,
            Value = CheckEvaluationFact,
            Error = FactoryError,
        >,
    > Checks for T
{
}
pub trait Comparison:
    Provide<
    CheckComparisonFact,
    Query = check_evaluation::ComparisonRead,
    Value = CheckComparisonFact,
    Error = FactoryError,
>
{
}
impl<
        T: Provide<
            CheckComparisonFact,
            Query = check_evaluation::ComparisonRead,
            Value = CheckComparisonFact,
            Error = FactoryError,
        >,
    > Comparison for T
{
}
pub trait Production:
    Provide<ProductionFact, Query = ProductionQuery, Value = ProductionFact, Error = FactoryError>
{
}
impl<
        T: Provide<
            ProductionFact,
            Query = ProductionQuery,
            Value = ProductionFact,
            Error = FactoryError,
        >,
    > Production for T
{
}

pub struct Service<'a> {
    intent: policy_intent::Service<'a>,
    goals: &'a GoalsStore,
}
pub struct ReportPlan {
    now: DateTime<Utc>,
    scenarios: Vec<Scenario>,
    scenario_findings: Vec<scenario::Finding>,
    real_catalogues: Vec<policy::Catalogue>,
    policy_findings: Vec<policy::Finding>,
    tags: BTreeSet<String>,
    catalogues_with_drafts: Vec<policy::Catalogue>,
    goals_catalogue: goals::GoalsCatalogue,
    checkins: Vec<goals::CheckIn>,
    asked: Option<Scope>,
    target_scopes: Vec<Scope>,
    all_attestations: Vec<Attestation>,
    tasks: Vec<TaskInventoryFact>,
    metric_ids: Vec<MetricId>,
}
pub struct MeasuredReport {
    plan: ReportPlan,
    values: Vec<MetricValue>,
}
impl ReportPlan {
    pub fn metric_ids(&self) -> &[MetricId] {
        &self.metric_ids
    }
    pub fn scope(&self) -> Option<&str> {
        self.asked.as_ref().map(|s| s.name.as_str())
    }
    pub async fn read_metrics<P>(self, provider: &P, query: &P::Query) -> Result<MeasuredReport>
    where
        P: Provide<MetricValuesFact, Value = MetricValuesFact, Error = FactoryError>,
    {
        let fact = Facts::<L6>::new()
            .get::<MetricValuesFact, _>(provider, query)
            .await?;
        Ok(MeasuredReport {
            plan: self,
            values: fact.values,
        })
    }
}

pub struct WhatIfPlan {
    now: DateTime<Utc>,
    asked: Option<Scope>,
    scenario_name: Option<String>,
    overrides: BTreeMap<DriverId, scenario::Override>,
    horizon_weeks: u32,
    backlog_total: f64,
    metric_ids: Vec<MetricId>,
}
pub struct MeasuredWhatIf {
    plan: WhatIfPlan,
    values: Vec<MetricValue>,
}
impl WhatIfPlan {
    pub fn metric_ids(&self) -> &[MetricId] {
        &self.metric_ids
    }
    pub fn scope(&self) -> Option<&str> {
        self.asked.as_ref().map(|s| s.name.as_str())
    }
    pub async fn read_metrics<P>(self, provider: &P, query: &P::Query) -> Result<MeasuredWhatIf>
    where
        P: Provide<MetricValuesFact, Value = MetricValuesFact, Error = FactoryError>,
    {
        let fact = Facts::<L6>::new()
            .get::<MetricValuesFact, _>(provider, query)
            .await?;
        Ok(MeasuredWhatIf {
            plan: self,
            values: fact.values,
        })
    }
}
/// How many weeks of production history [`weekly_throughput_history`]
/// draws from -- `scenario::Horizon::default()`'s own 26 weeks, so a
/// scenario with no explicit `horizon:` bootstrap-samples from a history
/// exactly as long as what it projects forward.
const THROUGHPUT_HISTORY_WEEKS: usize = 26;

/// How many Monte Carlo samples every forecast/goal-probability call in
/// this module draws. `crate::scenario`'s own tests run in the
/// hundreds; 1000 is still cheap (a splitmix64 draw and a few float
/// comparisons per sample) and gives smoother percentiles for a report a
/// person actually reads.
const SAMPLES: u32 = 1000;

/// The one-at-a-time swing [`scenario::tornado`] varies each driver by --
/// the same ±20% `crate::scenario`'s own tests use.
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
fn weekly_throughput_history(daily: &[factory_kernel::ProductionBucket], weeks: usize) -> Vec<f64> {
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
pub fn driver_baseline(values: &BTreeMap<MetricId, MetricValue>) -> BTreeMap<DriverId, f64> {
    let mut out = BTreeMap::new();
    for def in scenario::driver_defs() {
        if let Some(name) = def.metric {
            if let Ok(id) = MetricId::new(name) {
                if let Some(value) = values
                    .get(&id)
                    .filter(|v| {
                        !matches!(def.id, "unit_cost" | "tokens_per_run") || v.reason.is_none()
                    })
                    .and_then(|v| v.value)
                    .filter(|v| v.is_finite() && *v >= 0.0)
                {
                    out.insert(def.id.to_string(), value);
                }
            }
        }
    }
    out.insert("capacity_factor".to_string(), 1.0);
    out
}

pub fn measured_overrides(
    baseline: &BTreeMap<DriverId, f64>,
    overrides: &BTreeMap<DriverId, scenario::Override>,
) -> BTreeMap<DriverId, f64> {
    let mut values = scenario::apply_overrides(baseline, overrides);
    for driver in ["unit_cost", "tokens_per_run"] {
        if !baseline.contains_key(driver) {
            values.remove(driver);
        }
    }
    values
}

pub fn driver_result(
    baseline: &BTreeMap<DriverId, f64>,
    overridden: BTreeMap<DriverId, f64>,
    metrics: &BTreeMap<MetricId, MetricValue>,
) -> ScenarioDrivers {
    let outcomes_before = scenario::evaluate_outcomes(baseline);
    let outcomes_after = scenario::evaluate_outcomes(&overridden);
    let reasons = |values: &BTreeMap<DriverId, f64>| {
        let mut reasons = scenario::outcome_reasons(values);
        for (outcome, driver) in [
            ("weekly_cost", "unit_cost"),
            ("weekly_tokens", "tokens_per_run"),
        ] {
            if let Some(reason) = MetricId::new(driver)
                .ok()
                .and_then(|id| metrics.get(&id))
                .and_then(|v| v.reason.as_ref())
            {
                if reasons.contains_key(outcome) {
                    reasons.insert(outcome.into(), reason.clone());
                }
            }
        }
        reasons
    };
    let outcome_reasons_before = reasons(baseline);
    let outcome_reasons_after = reasons(&overridden);
    let tornados = outcomes_after
        .keys()
        .map(|id| {
            (
                id.clone(),
                scenario::tornado(&overridden, id, TORNADO_SWING),
            )
        })
        .collect();
    let tornado = scenario::tornado(&overridden, KEY_OUTCOME, TORNADO_SWING);
    ScenarioDrivers {
        overridden,
        outcomes_before,
        outcomes_after,
        outcome_reasons_before,
        outcome_reasons_after,
        tornado,
        tornados,
    }
}

/// The multiplier driver overrides put on top of `weekly_throughput_history`
/// before it feeds a forecast: the ratio of `effective_throughput` after
/// `overridden` to before `baseline`, or `1.0` (no change) when the
/// baseline's own effective throughput is zero -- there is nothing to scale
/// proportionally from. Applied to every point in the history rather than
/// only to a single "current" throughput number, so the forecast's bands
/// keep the real history's own week-to-week shape and variance, just scaled
/// -- "effective throughput scaled per `evaluate_outcomes`" (`#100`).
fn throughput_scale_factor(
    baseline: &BTreeMap<DriverId, f64>,
    overridden: &BTreeMap<DriverId, f64>,
) -> f64 {
    let before = scenario::evaluate_outcomes(baseline)
        .get(KEY_OUTCOME)
        .copied()
        .unwrap_or(0.0);
    let after = scenario::evaluate_outcomes(overridden)
        .get(KEY_OUTCOME)
        .copied()
        .unwrap_or(0.0);
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
fn find_kr<'a>(
    catalogue: &'a goals::GoalsCatalogue,
    kr: &KrRef,
) -> Option<(&'a goals::Cycle, &'a goals::Objective, &'a goals::KeyResult)> {
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
        checkins
            .iter()
            .filter(|c| &c.kr == kr_ref)
            .max_by_key(|c| c.at)
            .map(|c| c.value)
    } else {
        kd.bound_metric(objective_id)
            .and_then(|m| values.get(&m))
            .and_then(|v| v.value)
    }
}

/// Re-score one `GoalChange` -- see the report request's documented
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
                reason: Some(format!(
                    "{} names no key result in any loaded cycle",
                    change.kr
                )),
            },
        };
    };

    let effective_target = change.target.unwrap_or(kd.target);
    let effective_by = change.by.unwrap_or_else(|| cycle.ends_on());

    let Some(current_value) = kr_current_value(kd, &objective.id, &change.kr, values, checkins)
    else {
        return ScenarioGoalProbability {
            kr: change.kr.clone(),
            target: Some(effective_target),
            by: Some(effective_by),
            probability: scenario::GoalProbability {
                probability: None,
                reason: Some(
                    "no current value to project from yet -- no metric value or check-in"
                        .to_string(),
                ),
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

    let probability = scenario::goal_probability(
        shape,
        current_value,
        effective_target,
        effective_by,
        now,
        weekly_history,
        SAMPLES,
        seed,
    );
    ScenarioGoalProbability {
        kr: change.kr.clone(),
        target: Some(effective_target),
        by: Some(effective_by),
        probability,
    }
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
    let subtree = scenario::policy_delta(
        &policy::worst_across_scopes(&baseline_all),
        &policy::worst_across_scopes(&scenario_all),
    );
    (per_scope, subtree)
}

/// Non-terminal tasks labelled `goal=<objective>/<kr>` for any of
/// `goals`'s own entries, summed -- unscoped, the same convention
/// `crate::metrics`'s own `goal_tasks_done` reads tasks under: a key
/// result's own scope (if any) is not the scope its serving tasks run in.
pub fn open_goal_task_count(
    tasks: &[TaskInventoryFact],
    goal_changes: &[scenario::GoalChange],
) -> usize {
    let labels: BTreeSet<String> = goal_changes.iter().map(|c| c.kr.to_string()).collect();
    tasks
        .iter()
        .filter(|t| t.open && t.labels.get("goal").is_some_and(|l| labels.contains(l)))
        .count()
}

/// Open tasks in the target scopes: the backlog a forecast burns down. A
/// task blocked by a failure is open work (`#122`); a closed one is not.
pub fn open_backlog(tasks: &[TaskInventoryFact], target_names: &BTreeSet<&str>) -> f64 {
    tasks
        .iter()
        .filter(|t| t.open && target_names.contains(t.scope.as_str()))
        .count() as f64
}

fn push_if_known(ids: &mut Vec<MetricId>, id: &MetricId) {
    if !matches!(metrics::resolve(id), Err(MetricError::Unknown(_))) {
        ids.push(id.clone());
    }
}

impl<'a> Service<'a> {
    pub fn new(intent: policy_intent::Service<'a>, goals: &'a GoalsStore) -> Self {
        Self { intent, goals }
    }

    async fn check_read(
        &self,
        applied: &[(&Scope, Vec<policy::Applied>)],
        tags: &BTreeSet<String>,
        receipts: &[Attestation],
        now: DateTime<Utc>,
    ) -> check_evaluation::Read {
        let budgets: Result<Vec<Option<BudgetIntent>>> = async {
            let config = self
                .intent
                .budget_for(
                    &applied
                        .iter()
                        .map(|(_, subjects)| subjects.as_slice())
                        .collect::<Vec<_>>(),
                )
                .await?;
            Ok(applied
                .iter()
                .map(|(scope, _)| {
                    config
                        .as_ref()
                        .map(|config| self.intent.config.budget_intent(scope, config))
                })
                .collect())
        }
        .await;
        check_evaluation::Read {
            scopes: inputs(applied),
            tags: tags.clone(),
            attestations: receipts.to_vec(),
            budgets,
            now: Some(now),
        }
    }

    async fn evaluate_applied<C: Checks>(
        &self,
        checks: &C,
        applied: &[(&Scope, Vec<policy::Applied>)],
        tags: &BTreeSet<String>,
        receipts: &[Attestation],
        now: DateTime<Utc>,
    ) -> Result<(
        BTreeMap<String, Vec<policy::ControlStatus>>,
        Vec<policy::Finding>,
    )> {
        let read = self.check_read(applied, tags, receipts, now).await;
        let fact = Facts::<L6>::new()
            .get::<CheckEvaluationFact, _>(checks, &read)
            .await?;
        if fact.scopes.len() != applied.len() {
            return Err(FactoryError::Other(anyhow::anyhow!(
                "check results do not match their scopes"
            )));
        }
        let mut statuses = BTreeMap::new();
        let mut findings = Vec::new();
        for (result, (scope, subjects)) in fact.scopes.into_iter().zip(applied) {
            if result.scope != scope.name {
                return Err(FactoryError::Other(anyhow::anyhow!(
                    "check result scope does not match its declaration"
                )));
            }
            findings.extend(result.findings.into_iter().map(|finding| policy::Finding {
                kind: policy::FindingKind::AmbiguousCheckTarget,
                subject: finding.subject,
                detail: finding.detail,
            }));
            statuses.insert(scope.name.clone(), classified(result.statuses, subjects)?);
        }
        Ok((statuses, findings))
    }

    async fn evaluate_baseline_over_scopes<C: Checks>(
        &self,
        checks: &C,
        targets: &[Scope],
        catalogues: &[policy::Catalogue],
        tags: &BTreeSet<String>,
        receipts: &[Attestation],
        now: DateTime<Utc>,
    ) -> Result<(
        BTreeMap<String, Vec<policy::ControlStatus>>,
        Vec<policy::Finding>,
    )> {
        let mut findings = Vec::new();
        let applied: Vec<_> = targets
            .iter()
            .map(|scope| {
                let (subjects, own) =
                    policy::applicable(catalogues, &self.intent.config.chain(&scope.name));
                findings.extend(own);
                (scope, subjects)
            })
            .collect();
        let (statuses, evidence_findings) = self
            .evaluate_applied(checks, &applied, tags, receipts, now)
            .await?;
        findings.extend(evidence_findings);
        Ok((statuses, findings))
    }

    async fn evaluate_scenario_over_scopes<C: Checks>(
        &self,
        checks: &C,
        targets: &[Scope],
        scenario_obj: &Scenario,
        catalogues: &[policy::Catalogue],
        tags: &BTreeSet<String>,
        receipts: &[Attestation],
        now: DateTime<Utc>,
    ) -> Result<(
        BTreeMap<String, Vec<policy::ControlStatus>>,
        Vec<policy::Finding>,
        Vec<scenario::Finding>,
    )> {
        let mut policy_findings = Vec::new();
        let mut scenario_findings = Vec::new();
        let applied: Vec<_> = targets
            .iter()
            .map(|scope| {
                let (chain, own) =
                    scenario::overlay_chain(&self.intent.config.chain(&scope.name), scenario_obj);
                scenario_findings.extend(own);
                let (subjects, own) = policy::applicable(catalogues, &chain);
                policy_findings.extend(own);
                (scope, subjects)
            })
            .collect();
        let (statuses, own) = self
            .evaluate_applied(checks, &applied, tags, receipts, now)
            .await?;
        policy_findings.extend(own);
        Ok((statuses, policy_findings, scenario_findings))
    }

    pub async fn subtree_daily<D: Production>(
        &self,
        production: &D,
        asked: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<Vec<factory_kernel::ProductionBucket>> {
        let fact = Facts::<L6>::new()
            .get::<ProductionFact, _>(
                production,
                &ProductionQuery {
                    scope: asked.map(str::to_string),
                    now,
                    minutes: None,
                    bin: ProductionBin::Day,
                    subtree: true,
                },
            )
            .await?;
        Ok(fact.daily)
    }

    pub async fn prepare_report<K: Knowledge, I: Inventory>(
        &self,
        scope: Option<&str>,
        knowledge: &K,
        inventory: &I,
        now: DateTime<Utc>,
    ) -> Result<ReportPlan> {
        let scenarios_dir = scenario::scenarios_dir(&self.intent.root);
        let (scenarios, mut scenario_findings) = {
            let dir = scenarios_dir.clone();
            tokio::task::spawn_blocking(move || scenario::load(&dir))
                .await
                .map_err(|e| FactoryError::Other(anyhow::anyhow!("scenario directory walk: {e}")))?
        };
        scenario_findings.extend(scenario::stale_findings(&scenarios, now));

        let (real_catalogues, mut policy_findings, tags) =
            self.intent.catalogues_with_tags(knowledge).await?;
        let (draft_catalogues, draft_findings) = {
            let dir = scenario::drafts_dir(&self.intent.root);
            tokio::task::spawn_blocking(move || scenario::load_drafts(&dir))
                .await
                .map_err(|e| {
                    FactoryError::Other(anyhow::anyhow!("draft policy directory walk: {e}"))
                })?
        };
        policy_findings.extend(draft_findings);
        let (catalogues_with_drafts, merge_findings) =
            scenario::merge_catalogues(&real_catalogues, draft_catalogues);
        scenario_findings.extend(merge_findings);

        let goals_catalogue = {
            let dir = goals::goals_dir(&self.intent.root);
            tokio::task::spawn_blocking(move || goals::load(&dir))
                .await
                .map_err(|e| FactoryError::Other(anyhow::anyhow!("goals directory walk: {e}")))?
        };
        let checkins = self.goals.all().await?;

        let (asked, target_scopes) = scope_subtree(&self.intent.config.scopes, scope)?;
        let asked = asked.cloned();
        let target_scopes: Vec<Scope> = target_scopes.into_iter().cloned().collect();
        let all_attestations = self.intent.receipts.all().await?;
        let tasks = Facts::<L6>::new()
            .get::<TaskInventoryFact, _>(
                inventory,
                &TaskInventoryQuery::Members(
                    target_scopes.iter().map(|s| s.name.clone()).collect(),
                ),
            )
            .await?;

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
                // `push_if_known` -- the same filter
                // `goals_metric_ids` applies to a goals catalogue's own
                // metric references, reused here so a scenario author's
                // typo in a `signposts:`/`goals:` metric name is a finding
                // (`scenario::load`'s own `UnknownMetric`), never a
                // `BadRequest` that refuses the whole report -- unlike
                // L5's metric query preparation, which refuses a call naming even
                // one truly unknown id.
                push_if_known(&mut metric_ids, &sp.metric);
            }
            for change in &s.goals {
                if let Some((_, objective, kd)) = find_kr(&goals_catalogue, &change.kr) {
                    if !kd.manual {
                        if let Some(m) = kd.bound_metric(&objective.id) {
                            push_if_known(&mut metric_ids, &m);
                        }
                    }
                }
            }
        }

        Ok(ReportPlan {
            now,
            scenarios,
            scenario_findings,
            real_catalogues,
            policy_findings,
            tags,
            catalogues_with_drafts,
            goals_catalogue,
            checkins,
            asked,
            target_scopes,
            all_attestations,
            tasks,
            metric_ids,
        })
    }

    pub async fn finish_report<C: Checks, D: Production>(
        &self,
        measured: MeasuredReport,
        checks: &C,
        production: &D,
    ) -> Result<ScenariosReport> {
        let MeasuredReport {
            plan,
            values: metric_values,
        } = measured;
        let ReportPlan {
            now,
            scenarios,
            mut scenario_findings,
            real_catalogues,
            mut policy_findings,
            tags,
            catalogues_with_drafts,
            goals_catalogue,
            checkins,
            asked,
            target_scopes,
            all_attestations,
            tasks,
            metric_ids: _,
        } = plan;
        let values: BTreeMap<MetricId, MetricValue> = metric_values
            .iter()
            .map(|v| (v.id.clone(), v.clone()))
            .collect();

        let baseline_drivers = driver_baseline(&values);

        // The plain baseline -- no scenario, no overlay -- once, shared by
        // `ScenarioBaseline::policy` and by every scenario's own delta.
        let (baseline_statuses, baseline_policy_findings) = self
            .evaluate_baseline_over_scopes(
                checks,
                &target_scopes,
                &real_catalogues,
                &tags,
                &all_attestations,
                now,
            )
            .await?;
        policy_findings.extend(baseline_policy_findings);
        let baseline_rollup = policy::rollup(&policy::worst_across_scopes(
            &baseline_statuses.values().cloned().collect::<Vec<_>>(),
        ));

        let target_names: BTreeSet<&str> = target_scopes.iter().map(|s| s.name.as_str()).collect();
        let baseline_backlog = open_backlog(&tasks, &target_names);
        let daily = self
            .subtree_daily(production, asked.as_ref().map(|s| s.name.as_str()), now)
            .await?;
        let history = weekly_throughput_history(&daily, THROUGHPUT_HISTORY_WEEKS);
        let baseline_forecast = scenario::forecast_completion(
            &history,
            baseline_backlog,
            scenario::Horizon::default().weeks(),
            SAMPLES,
            scenario::seed_from(&[
                "baseline",
                asked
                    .as_ref()
                    .map(|s| s.name.as_str())
                    .unwrap_or("instance"),
            ]),
        );

        let mut results = Vec::with_capacity(scenarios.len());
        let mut triggered = Vec::new();
        for s in &scenarios {
            let (scenario_statuses, s_policy_findings, s_scenario_findings) = self
                .evaluate_scenario_over_scopes(
                    checks,
                    &target_scopes,
                    s,
                    &catalogues_with_drafts,
                    &tags,
                    &all_attestations,
                    now,
                )
                .await?;
            policy_findings.extend(s_policy_findings);
            scenario_findings.extend(s_scenario_findings);

            let (per_scope_deltas, subtree_delta) =
                deltas_from_statuses(&target_scopes, &baseline_statuses, &scenario_statuses);

            let overrides = s.driver_overrides();
            let overridden_drivers = measured_overrides(&baseline_drivers, &overrides);

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
                .map(|change| {
                    build_goal_probability(
                        change,
                        &goals_catalogue,
                        &values,
                        &checkins,
                        now,
                        &scaled_history,
                        scenario::seed_from(&[s.name.as_str(), "goal", &change.kr.to_string()]),
                    )
                })
                .collect();

            let signposts = scenario::evaluate_signposts(&s.signposts, &values, now);
            triggered.extend(triggered_of(s, &signposts));

            let subject = format!("{}.yaml", s.name);
            let own_findings: Vec<scenario::Finding> = scenario_findings
                .iter()
                .filter(|f| f.subject == subject)
                .cloned()
                .collect();

            results.push(ScenarioResult {
                scenario: s.clone(),
                findings: own_findings,
                policy: per_scope_deltas,
                policy_subtree: subtree_delta,
                drivers: driver_result(&baseline_drivers, overridden_drivers, &values),
                backlog,
                forecast,
                goals: goal_results,
                signposts,
            });
        }

        scenario_findings.sort_by(|a, b| {
            a.subject
                .cmp(&b.subject)
                .then(a.kind.cmp(&b.kind))
                .then(a.detail.cmp(&b.detail))
        });
        scenario_findings.dedup();
        policy_findings.sort_by(|a, b| {
            a.subject
                .cmp(&b.subject)
                .then(a.kind.cmp(&b.kind))
                .then(a.detail.cmp(&b.detail))
        });
        policy_findings.dedup();

        Ok(ScenariosReport {
            scope: asked.as_ref().map(|s| s.name.clone()),
            baseline: ScenarioBaseline {
                metrics: metric_values,
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

    pub async fn prepare_whatif<K: Knowledge, C: Checks, I: Inventory>(
        &self,
        scenario_name: Option<String>,
        raw_drivers: BTreeMap<DriverId, String>,
        scope: Option<&str>,
        knowledge: &K,
        checks: &C,
        inventory: &I,
        now: DateTime<Utc>,
    ) -> Result<WhatIfPlan> {
        let (asked, target_scopes) = scope_subtree(&self.intent.config.scopes, scope)?;
        let asked = asked.cloned();
        let target_scopes: Vec<Scope> = target_scopes.into_iter().cloned().collect();

        let mut overrides: BTreeMap<DriverId, scenario::Override> = BTreeMap::new();
        let mut horizon_weeks = scenario::Horizon::default().weeks();
        let mut backlog_total = 0.0f64;

        if let Some(name) = &scenario_name {
            let (scenarios, _findings) = {
                let dir = scenario::scenarios_dir(&self.intent.root);
                tokio::task::spawn_blocking(move || scenario::load(&dir))
                    .await
                    .map_err(|e| {
                        FactoryError::Other(anyhow::anyhow!("scenario directory walk: {e}"))
                    })?
            };
            let s = scenarios
                .into_iter()
                .find(|s| &s.name == name)
                .ok_or_else(|| FactoryError::BadRequest(format!("no such scenario: {name:?}")))?;
            overrides = s.driver_overrides();
            horizon_weeks = s.horizon.weeks();

            let (real_catalogues, _pf, tags) = self.intent.catalogues_with_tags(knowledge).await?;
            let (draft_catalogues, _df) = {
                let dir = scenario::drafts_dir(&self.intent.root);
                tokio::task::spawn_blocking(move || scenario::load_drafts(&dir))
                    .await
                    .map_err(|e| {
                        FactoryError::Other(anyhow::anyhow!("draft policy directory walk: {e}"))
                    })?
            };
            let (catalogues_with_drafts, _mf) =
                scenario::merge_catalogues(&real_catalogues, draft_catalogues);
            let all_attestations = self.intent.receipts.all().await?;
            let all_scopes = &target_scopes;

            let (baseline_statuses, _bf) = self
                .evaluate_baseline_over_scopes(
                    checks,
                    &all_scopes,
                    &real_catalogues,
                    &tags,
                    &all_attestations,
                    now,
                )
                .await?;
            let (scenario_statuses, _sf, _svf) = self
                .evaluate_scenario_over_scopes(
                    checks,
                    &all_scopes,
                    &s,
                    &catalogues_with_drafts,
                    &tags,
                    &all_attestations,
                    now,
                )
                .await?;
            let (_per_scope, subtree_delta) =
                deltas_from_statuses(&all_scopes, &baseline_statuses, &scenario_statuses);

            let tasks = Facts::<L6>::new()
                .get::<TaskInventoryFact, _>(
                    inventory,
                    &TaskInventoryQuery::Members(
                        target_scopes.iter().map(|s| s.name.clone()).collect(),
                    ),
                )
                .await?;
            let open_goal_tasks = open_goal_task_count(&tasks, &s.goals);
            backlog_total = (subtree_delta.newly_open.len() + open_goal_tasks) as f64;
        }

        for (id, raw) in &raw_drivers {
            if !scenario::driver_defs().iter().any(|def| def.id == id) {
                return Err(FactoryError::BadRequest(format!(
                    "unknown scenario driver: {id}"
                )));
            }
            let ov = scenario::parse_override(raw)
                .map_err(|e| FactoryError::BadRequest(format!("driver {id:?}: {e}")))?;
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

        Ok(WhatIfPlan {
            now,
            asked,
            scenario_name,
            overrides,
            horizon_weeks,
            backlog_total,
            metric_ids,
        })
    }

    pub async fn finish_whatif<D: Production>(
        &self,
        measured: MeasuredWhatIf,
        production: &D,
    ) -> Result<ScenarioWhatIfResult> {
        let MeasuredWhatIf {
            plan,
            values: metric_values,
        } = measured;
        let WhatIfPlan {
            now,
            asked,
            scenario_name,
            overrides,
            horizon_weeks,
            backlog_total,
            metric_ids: _,
        } = plan;
        let values: BTreeMap<MetricId, MetricValue> = metric_values
            .into_iter()
            .map(|v| (v.id.clone(), v))
            .collect();
        let baseline_drivers = driver_baseline(&values);

        let overridden_drivers = measured_overrides(&baseline_drivers, &overrides);

        let daily = self
            .subtree_daily(production, asked.as_ref().map(|s| s.name.as_str()), now)
            .await?;
        let history = weekly_throughput_history(&daily, THROUGHPUT_HISTORY_WEEKS);
        let factor = throughput_scale_factor(&baseline_drivers, &overridden_drivers);
        let scaled_history = scale_history(&history, factor);

        let seed_name = scenario_name.as_deref().unwrap_or("whatif");
        let forecast = scenario::forecast_completion(
            &scaled_history,
            backlog_total,
            horizon_weeks,
            SAMPLES,
            scenario::seed_from(&[seed_name, "whatif"]),
        );

        Ok(ScenarioWhatIfResult {
            scenario: scenario_name,
            drivers: driver_result(&baseline_drivers, overridden_drivers, &values),
            forecast,
        })
    }

    pub async fn promote<
        K: Knowledge,
        C: Comparison,
        P: factory_assurance::remediation::RemediationCommands,
        I: Inventory,
    >(
        &self,
        scenario_name: String,
        scope: String,
        agent: Option<String>,
        knowledge: &K,
        comparisons: &C,
        commands: &remediation::Service<P, I>,
        now: DateTime<Utc>,
    ) -> Result<remediation::PromotionReceipt> {
        let scope_obj = resolve_scope(&self.intent.config.scopes, &scope)?.clone();
        let (scenarios, _findings) = {
            let dir = scenario::scenarios_dir(&self.intent.root);
            tokio::task::spawn_blocking(move || scenario::load(&dir))
                .await
                .map_err(|e| FactoryError::Other(anyhow::anyhow!("scenario directory walk: {e}")))?
        };
        let s = scenarios
            .into_iter()
            .find(|s| s.name == scenario_name)
            .ok_or_else(|| {
                FactoryError::BadRequest(format!("no such scenario: {scenario_name:?}"))
            })?;
        let (real_catalogues, _pf, tags) = self.intent.catalogues_with_tags(knowledge).await?;
        let (draft_catalogues, _df) = {
            let dir = scenario::drafts_dir(&self.intent.root);
            tokio::task::spawn_blocking(move || scenario::load_drafts(&dir))
                .await
                .map_err(|e| {
                    FactoryError::Other(anyhow::anyhow!("draft policy directory walk: {e}"))
                })?
        };
        let (catalogues_with_drafts, _mf) =
            scenario::merge_catalogues(&real_catalogues, draft_catalogues);
        let base_chain = self.intent.config.chain(&scope_obj.name);
        let (baseline_applied, _cf1) = policy::applicable(&real_catalogues, &base_chain);
        let (overlaid_chain, _of) = scenario::overlay_chain(&base_chain, &s);
        let (scenario_applied, _cf2) = policy::applicable(&catalogues_with_drafts, &overlaid_chain);
        let all_attestations = self.intent.receipts.all().await?;
        let primary = vec![(&scope_obj, scenario_applied.clone())];
        let alternative = vec![(&scope_obj, baseline_applied.clone())];
        let read = check_evaluation::ComparisonRead {
            primary: self
                .check_read(&primary, &tags, &all_attestations, now)
                .await,
            alternative: inputs(&alternative),
        };
        let fact = Facts::<L6>::new()
            .get::<CheckComparisonFact, _>(comparisons, &read)
            .await?;
        let comparison = fact
            .scopes
            .into_iter()
            .find(|row| row.scope == scope_obj.name)
            .ok_or_else(|| {
                FactoryError::Other(anyhow::anyhow!("comparison has no selected scope"))
            })?;
        let baseline_statuses = classified(comparison.alternative, &baseline_applied)?;
        let scenario_statuses = classified(comparison.primary, &scenario_applied)?;
        let delta = scenario::policy_delta(&baseline_statuses, &scenario_statuses);
        commands
            .promote(
                &s.name,
                &scope_obj.name,
                agent,
                &scenario_applied,
                &scenario_statuses,
                &delta,
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy_store::PolicyStore;
    use factory_kernel::{
        CheckObservation, CommandPort, Commands, FactProvider, ScopeCheckEvaluation, Status,
        TaskReceipt, L4, L5,
    };
    use std::{
        path::PathBuf,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        },
    };

    struct Fixture {
        root: PathBuf,
        receipts: PolicyStore,
        goals: GoalsStore,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir()
                .join(format!("factory-scenarios-owner-{}", uuid::Uuid::new_v4()));
            for dir in [
                policy::policies_dir(&root),
                scenario::drafts_dir(&root),
                scenario::scenarios_dir(&root),
            ] {
                std::fs::create_dir_all(dir).unwrap();
            }
            std::fs::write(policy::policies_dir(&root).join("house.yaml"),
                "framework: house\ntitle: House\nkind: regulation\ncontrols:\n  - {id: base, title: Base, evidence: [{check: attestation}]}\n").unwrap();
            std::fs::write(scenario::drafts_dir(&root).join("next.yaml"),
                "framework: next\ntitle: Next\nkind: regulation\ncontrols:\n  - {id: new, title: New, evidence: [{check: attestation}]}\n").unwrap();
            let fixture = Self {
                root,
                receipts: PolicyStore::in_memory().unwrap(),
                goals: GoalsStore::in_memory().unwrap(),
            };
            fixture.write_scenario("Future", "-20%");
            fixture
        }
        fn write_scenario(&self, title: &str, capacity: &str) {
            std::fs::write(scenario::scenarios_dir(&self.root).join("future.yaml"),
                format!("name: future\ntitle: {title}\npolicy: {{add_frameworks: [next]}}\ndrivers: {{capacity_factor: '{capacity}'}}\nsignposts: [{{metric: throughput_week, below: 99}}]\n")).unwrap();
        }
        fn owner(&self) -> Service<'_> {
            let config = policy_intent::Configuration {
                scopes: [
                    ("root", "."),
                    ("work", "projects/work"),
                    ("child", "projects/work/deep"),
                    ("side", "projects/work-other"),
                ]
                .into_iter()
                .map(|(name, path)| Scope {
                    id: name.into(),
                    name: name.into(),
                    path: path.into(),
                    policies: Default::default(),
                })
                .collect(),
                root_policies: serde_yaml_ng::from_str("frameworks: [house]").unwrap(),
                root_name: Some("root".into()),
                instance_name: "example".into(),
            };
            Service::new(
                policy_intent::Service::new(self.root.clone(), config, &self.receipts),
                &self.goals,
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }
    #[derive(Default)]
    struct Log {
        events: Mutex<Vec<String>>,
        fail: Mutex<Option<String>>,
    }
    impl Log {
        fn event(&self, value: impl Into<String>) -> Result<()> {
            let value = value.into();
            self.events.lock().unwrap().push(value.clone());
            if self.fail.lock().unwrap().as_deref() == Some(value.as_str()) {
                return Err(FactoryError::adapter("fact", value));
            }
            Ok(())
        }
        fn events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }
    }
    struct Tags(Arc<Log>);
    impl FactProvider for Tags {
        type Level = L5;
    }
    #[async_trait::async_trait]
    impl Provide<KnowledgeTags> for Tags {
        type Query = ();
        type Value = KnowledgeTags;
        type Error = FactoryError;
        async fn get(&self, _: &()) -> Result<Self::Value> {
            self.0.event("knowledge")?;
            Ok(KnowledgeTags {
                tags: BTreeSet::new(),
            })
        }
    }
    struct Tasks {
        log: Arc<Log>,
        rows: Mutex<Vec<TaskInventoryFact>>,
    }
    impl FactProvider for Tasks {
        type Level = L4;
    }
    #[async_trait::async_trait]
    impl Provide<TaskInventoryFact> for Tasks {
        type Query = TaskInventoryQuery;
        type Value = Vec<TaskInventoryFact>;
        type Error = FactoryError;
        async fn get(&self, q: &TaskInventoryQuery) -> Result<Self::Value> {
            let members = match q {
                TaskInventoryQuery::Members(members) => {
                    self.log.event(format!(
                        "inventory:{}",
                        members.iter().cloned().collect::<Vec<_>>().join(",")
                    ))?;
                    members.clone()
                }
                TaskInventoryQuery::Exact(scope) => {
                    self.log.event(format!("inventory:exact:{scope}"))?;
                    BTreeSet::from([scope.clone()])
                }
                _ => panic!("Scenarios needs explicit members or exact promotion scope"),
            };
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .filter(|row| members.contains(&row.scope))
                .cloned()
                .collect())
        }
    }
    struct Metrics {
        log: Arc<Log>,
        value: Mutex<f64>,
    }
    impl FactProvider for Metrics {
        type Level = L5;
    }
    #[async_trait::async_trait]
    impl Provide<MetricValuesFact> for Metrics {
        type Query = Vec<MetricId>;
        type Value = MetricValuesFact;
        type Error = FactoryError;
        async fn get(&self, ids: &Vec<MetricId>) -> Result<Self::Value> {
            self.log.event("metrics")?;
            Ok(MetricValuesFact {
                values: ids
                    .iter()
                    .map(|id| MetricValue {
                        id: id.clone(),
                        value: Some(*self.value.lock().unwrap()),
                        as_of: now(),
                        reason: None,
                    })
                    .collect(),
            })
        }
    }
    struct Evaluations(Arc<Log>);
    impl FactProvider for Evaluations {
        type Level = L5;
    }
    fn observations(
        subjects: &[factory_assurance::checks::EvaluationSubject],
    ) -> Vec<CheckObservation> {
        subjects
            .iter()
            .map(|subject| CheckObservation {
                control: subject.control.clone(),
                title: subject.title.clone(),
                refs: vec![],
                status: if subject.control.framework == "house" {
                    Status::Satisfied { reasons: vec![] }
                } else {
                    Status::Open {
                        reasons: vec!["not yet evidenced".into()],
                    }
                },
            })
            .collect()
    }
    #[async_trait::async_trait]
    impl Provide<CheckEvaluationFact> for Evaluations {
        type Query = check_evaluation::Read;
        type Value = CheckEvaluationFact;
        type Error = FactoryError;
        async fn get(&self, q: &Self::Query) -> Result<Self::Value> {
            self.0.event("checks")?;
            Ok(CheckEvaluationFact {
                at: q.now.unwrap(),
                scopes: q
                    .scopes
                    .iter()
                    .map(|input| ScopeCheckEvaluation {
                        scope: input.scope.name.clone(),
                        statuses: observations(&input.subjects),
                        findings: vec![],
                    })
                    .collect(),
            })
        }
    }
    #[async_trait::async_trait]
    impl Provide<CheckComparisonFact> for Evaluations {
        type Query = check_evaluation::ComparisonRead;
        type Value = CheckComparisonFact;
        type Error = FactoryError;
        async fn get(&self, q: &Self::Query) -> Result<Self::Value> {
            self.0.event("comparison")?;
            assert_eq!(q.primary.scopes[0].subjects.len(), 2);
            assert_eq!(q.alternative[0].subjects.len(), 1);
            Ok(CheckComparisonFact {
                at: q.primary.now.unwrap(),
                scopes: vec![factory_kernel::ScopeCheckComparison {
                    scope: q.primary.scopes[0].scope.name.clone(),
                    primary: observations(&q.primary.scopes[0].subjects),
                    alternative: observations(&q.alternative[0].subjects),
                }],
            })
        }
    }
    struct History(Arc<Log>);
    impl FactProvider for History {
        type Level = L4;
    }
    #[async_trait::async_trait]
    impl Provide<ProductionFact> for History {
        type Query = ProductionQuery;
        type Value = ProductionFact;
        type Error = FactoryError;
        async fn get(&self, q: &ProductionQuery) -> Result<Self::Value> {
            self.0
                .event(format!("history:{}", q.scope.as_deref().unwrap_or("all")))?;
            assert!(q.subtree && q.minutes.is_none() && q.bin == ProductionBin::Day);
            assert_eq!(q.now, now());
            let daily = (0..371)
                .map(|i| factory_kernel::ProductionBucket {
                    from: now() - chrono::Duration::days(371 - i),
                    to: now() - chrono::Duration::days(370 - i),
                    finished: 1,
                    scrapped: 0,
                    reworked: 0,
                    first_pass: 1,
                    partial: false,
                })
                .collect();
            Ok(ProductionFact {
                bin: q.bin,
                from: now() - chrono::Duration::days(371),
                to: now(),
                buckets: vec![],
                daily,
                earliest_run: None,
            })
        }
    }
    struct Assurance {
        log: Arc<Log>,
        intents: Mutex<Vec<factory_assurance::remediation::Intent>>,
        refuse: AtomicBool,
    }
    impl CommandPort for Assurance {
        type Level = L5;
    }
    #[async_trait::async_trait]
    impl factory_assurance::remediation::RemediationCommands for Assurance {
        async fn remediate(
            &self,
            intent: factory_assurance::remediation::Intent,
        ) -> Result<TaskReceipt> {
            self.log.event("command")?;
            if self.refuse.load(Ordering::SeqCst) {
                return Err(FactoryError::BadRequest("lower refused".into()));
            }
            let mut intents = self.intents.lock().unwrap();
            intents.push(intent);
            Ok(TaskReceipt {
                id: format!("created-{}", intents.len()),
            })
        }
    }
    fn now() -> DateTime<Utc> {
        "2026-10-05T12:00:00Z".parse().unwrap()
    }

    #[tokio::test]
    async fn actual_report_phases_read_typed_live_values_and_reread_authored_scenarios() {
        let fixture = Fixture::new();
        let owner = fixture.owner();
        let log = Arc::new(Log::default());
        let tags = Tags(log.clone());
        let tasks = Tasks {
            log: log.clone(),
            rows: Mutex::new(vec![]),
        };
        let metrics = Metrics {
            log: log.clone(),
            value: Mutex::new(2.0),
        };
        let checks = Evaluations(log.clone());
        let history = History(log.clone());
        for (title, capacity, value) in [("Future", "-20%", 2.0), ("Changed", "=2", 3.0)] {
            fixture.write_scenario(title, capacity);
            *metrics.value.lock().unwrap() = value;
            let plan = owner
                .prepare_report(Some("projects/work"), &tags, &tasks, now())
                .await
                .unwrap();
            assert_eq!(plan.scope(), Some("work"));
            let query = plan.metric_ids().to_vec();
            assert!(query.iter().any(|id| id.as_str() == "throughput_week"));
            let measured = plan.read_metrics(&metrics, &query).await.unwrap();
            let report = owner
                .finish_report(measured, &checks, &history)
                .await
                .unwrap();
            assert_eq!(report.scope.as_deref(), Some("work"));
            assert_eq!(report.scenarios[0].scenario.title, title);
            assert_eq!(
                report.scenarios[0]
                    .policy
                    .iter()
                    .map(|row| row.scope.as_str())
                    .collect::<Vec<_>>(),
                ["work", "child"]
            );
            assert_eq!(
                report.scenarios[0].policy_subtree.newly_open[0].to_string(),
                "next/new"
            );
            assert_eq!(report.baseline.metrics[0].value, Some(value));
            assert_eq!(report.triggered[0].scenario, "future");
            assert!(report.baseline.forecast.reason.is_none());
        }
        assert_eq!(
            log.events(),
            [
                "knowledge",
                "inventory:child,work",
                "metrics",
                "checks",
                "history:work",
                "checks",
                "knowledge",
                "inventory:child,work",
                "metrics",
                "checks",
                "history:work",
                "checks"
            ]
        );
    }

    #[tokio::test]
    async fn report_and_whatif_keep_refusal_and_provider_failure_order_without_cached_success() {
        let fixture = Fixture::new();
        let owner = fixture.owner();
        let log = Arc::new(Log::default());
        let tags = Tags(log.clone());
        let tasks = Tasks {
            log: log.clone(),
            rows: Mutex::new(vec![]),
        };
        let checks = Evaluations(log.clone());
        let metrics = Metrics {
            log: log.clone(),
            value: Mutex::new(2.0),
        };
        let history = History(log.clone());
        *log.fail.lock().unwrap() = Some("knowledge".into());
        assert!(owner
            .prepare_report(Some("missing"), &tags, &tasks, now())
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("knowledge"));
        assert_eq!(log.events(), ["knowledge"]);
        log.events.lock().unwrap().clear();
        assert_eq!(
            owner
                .prepare_whatif(
                    None,
                    BTreeMap::new(),
                    Some("missing"),
                    &tags,
                    &checks,
                    &tasks,
                    now()
                )
                .await
                .err()
                .unwrap()
                .code(),
            "no_such_scope"
        );
        assert!(
            log.events().is_empty(),
            "what-if validates scope before catalogue reads"
        );
        *log.fail.lock().unwrap() = None;
        let error = owner
            .prepare_whatif(
                Some("future".into()),
                BTreeMap::from([("capacity_factor".into(), "banana".into())]),
                Some("work"),
                &tags,
                &checks,
                &tasks,
                now(),
            )
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("capacity_factor"));
        assert_eq!(
            log.events(),
            ["knowledge", "checks", "checks", "inventory:child,work"]
        );
        log.events.lock().unwrap().clear();
        let plan = owner
            .prepare_report(None, &tags, &tasks, now())
            .await
            .unwrap();
        let query = plan.metric_ids().to_vec();
        *log.fail.lock().unwrap() = Some("metrics".into());
        assert!(plan
            .read_metrics(&metrics, &query)
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("metrics"));
        assert!(!log
            .events()
            .iter()
            .any(|event| event == "checks" || event.starts_with("history:")));
        *log.fail.lock().unwrap() = None;
        let plan = owner
            .prepare_report(None, &tags, &tasks, now())
            .await
            .unwrap();
        let query = plan.metric_ids().to_vec();
        let measured = plan.read_metrics(&metrics, &query).await.unwrap();
        *log.fail.lock().unwrap() = Some("checks".into());
        assert!(owner
            .finish_report(measured, &checks, &history)
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("checks"));
        assert!(!log
            .events()
            .iter()
            .any(|event| event.starts_with("history:")));
        *log.fail.lock().unwrap() = None;
        let plan = owner
            .prepare_whatif(
                None,
                BTreeMap::from([("capacity_factor".into(), "=3".into())]),
                Some("work"),
                &tags,
                &checks,
                &tasks,
                now(),
            )
            .await
            .unwrap();
        let query = plan.metric_ids().to_vec();
        let measured = plan.read_metrics(&metrics, &query).await.unwrap();
        let result = owner.finish_whatif(measured, &history).await.unwrap();
        assert_eq!(result.drivers.overridden["capacity_factor"], 3.0);
        assert_eq!(result.forecast.completion_week.p50, Some(0));
    }

    #[tokio::test]
    async fn promotion_owns_selection_comparison_and_actual_adjacent_id_only_submission() {
        let fixture = Fixture::new();
        let owner = fixture.owner();
        let log = Arc::new(Log::default());
        let tags = Tags(log.clone());
        let checks = Evaluations(log.clone());
        let commands = remediation::Service {
            assurance: Commands::new(Assurance {
                log: log.clone(),
                intents: Mutex::new(vec![]),
                refuse: AtomicBool::new(false),
            }),
            inventory: Tasks {
                log: log.clone(),
                rows: Mutex::new(vec![]),
            },
        };
        let receipt = owner
            .promote(
                "future".into(),
                "projects/work".into(),
                Some("worker".into()),
                &tags,
                &checks,
                &commands,
                now(),
            )
            .await
            .unwrap();
        assert_eq!(receipt.scope, "work");
        assert_eq!(receipt.created[0].task.id, "created-1");
        let intents = commands.assurance.port().intents.lock().unwrap();
        assert_eq!(
            intents[0].title,
            "Prepare next/new for scenario future: New"
        );
        assert_eq!(intents[0].scope, "work");
        assert_eq!(intents[0].agent.as_deref(), Some("worker"));
        assert_eq!(intents[0].labels["scenario"], "future");
        drop(intents);
        assert_eq!(
            log.events(),
            ["knowledge", "comparison", "inventory:exact:work", "command"]
        );
        commands
            .inventory
            .rows
            .lock()
            .unwrap()
            .push(TaskInventoryFact {
                id: "created-1".into(),
                scope: "work".into(),
                title: "New".into(),
                open: true,
                labels: BTreeMap::from([("policy".into(), "next/new".into())]),
            });
        let skipped = owner
            .promote(
                "future".into(),
                "work".into(),
                None,
                &tags,
                &checks,
                &commands,
                now(),
            )
            .await
            .unwrap();
        assert!(skipped.created.is_empty());
        assert_eq!(skipped.skipped[0].existing_task, "created-1");
        commands.inventory.rows.lock().unwrap().clear();
        commands
            .assurance
            .port()
            .refuse
            .store(true, Ordering::SeqCst);
        assert!(owner
            .promote(
                "future".into(),
                "work".into(),
                None,
                &tags,
                &checks,
                &commands,
                now()
            )
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("lower refused"));
    }
}
