//! A metric is a named, documented projection over data Factory already
//! records -- never a query language, so this stays inside design §8 the
//! same way `policy.rs` does. The registry ([`registry`]) is the fixed,
//! v1 vocabulary of metrics the Goals tab (`#99`) and the Scenarios tab
//! (`#100`) both read a KR or a driver from; [`resolve`] is how a metric
//! id an author typed into a YAML file (`goals.rs`'s `metric:` field, or a
//! scenario driver) turns into the definition that describes it, or a
//! reason it can't yet.
//!
//! ## What lives here, and what does not
//!
//! This module only carries the *vocabulary* -- what a metric means, its
//! unit, which way is better, and whether it is computed yet -- plus the
//! pure types a computed value or a time series come back as
//! ([`MetricValue`], [`MetricSeries`]) and a couple of pure helpers over
//! them ([`trend`]). It never reads a `Run`, a `Task`, a `PolicyReport` or
//! anything else Factory stores: actually computing a metric's value is
//! `factory-daemon`'s job (a later ticket), the same one-directional split
//! `policy.rs` draws between "what a check means" (here) and "resolving a
//! task/workflow/gate fact" (the engine).
//!
//! ## Families and bound ids
//!
//! Most v1 metrics are fixed, no-parameter names (`throughput_week`,
//! `first_pass_yield`, `scrap_rate`). A few are *families*: one shape of
//! metric repeated once per framework, dataset, or goal -- `compliance.cra`
//! and `compliance.dsgvo` are both instances of the family the registry
//! lists as `compliance.<framework>`. [`registry`] lists a family under its
//! own pattern, placeholder segments spelled `<name>`; [`resolve`] takes a
//! concrete [`MetricId`] like `compliance.cra`, matches it against every
//! family's pattern (exact segment count, literal segments equal,
//! placeholder segments bind to anything), and returns the [`MetricDef`]
//! with that instance's id, title and description filled in. A `<name>`
//! placeholder is never itself a legal [`MetricId`] segment, so a pattern
//! and a bound id are never mistaken for each other by shape alone.
//!
//! `goal_tasks_done.<objective>.<kr>` is the one family a key result is
//! expected to reference *unbound* -- `goals.rs`'s
//! `KeyResult::bound_metric` is what fills in a KR's own objective and key
//! result id, so an author just writes `metric: goal_tasks_done` once per
//! KR rather than repeating both ids. This module has no notion of a KR at
//! all; binding stays entirely in `goals.rs`, which is the only crate-
//! internal caller allowed to depend on this one (never the other way).
//!
//! ## Deviation from the issue text
//!
//! The issue (and the task that carries it) describes a [`MetricId`]
//! segment as `[a-z0-9_]+`. That shape cannot hold a bound parameter,
//! though: a framework or dataset name is `dataset::is_slug`
//! (`[a-z0-9][a-z0-9-]*`, hyphens, no underscore), and the issue's own
//! example objective and key-result ids (`ship-compliant`,
//! `cra-open-zero`) are hyphenated too. [`MetricId`] widens the charset to
//! `[a-z0-9][a-z0-9_-]*` per segment instead, so a bound id can embed
//! either style of name unmodified. A family's own name segment
//! (`throughput_week`, `goal_tasks_done`) still reads as snake_case by
//! convention; nothing here enforces that convention specifically.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

// ================================================================ MetricId

/// A metric's identity: `.`-separated segments, each starting with a
/// lowercase letter or digit and otherwise `[a-z0-9_-]` -- e.g.
/// `throughput_week`, `compliance.cra`, `bench.resolve_rate.eval-set-a`,
/// `goal_tasks_done.ship-compliant.cra-open-zero`. See the module doc
/// comment for why this is wider than the issue's own `[a-z0-9_]+`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MetricId(String);

impl MetricId {
    pub fn new(id: impl Into<String>) -> std::result::Result<Self, String> {
        let id = id.into();
        if is_valid(&id) {
            Ok(MetricId(id))
        } else {
            Err(format!(
                "{id:?} is not a metric id: `.`-separated segments, each starting with a \
                 lowercase letter or digit and otherwise [a-z0-9_-]"
            ))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_valid(id: &str) -> bool {
    !id.is_empty() && id.split('.').all(is_metric_segment)
}

/// A superset of `dataset::is_slug`'s shape (which is exactly the shape a
/// bound parameter -- a framework, dataset, objective or key-result id --
/// already has): the same leading character rule, `-` still allowed, `_`
/// additionally allowed for a family's own snake_case name segment. A test
/// below checks the superset relationship directly, rather than this
/// module quietly drifting out of step with `is_slug`.
fn is_metric_segment(seg: &str) -> bool {
    let mut chars = seg.chars();
    let starts = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    starts && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

impl std::fmt::Display for MetricId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for MetricId {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        MetricId::new(s)
    }
}

impl TryFrom<String> for MetricId {
    type Error = String;

    fn try_from(s: String) -> std::result::Result<Self, Self::Error> {
        MetricId::new(s)
    }
}

impl From<MetricId> for String {
    fn from(id: MetricId) -> String {
        id.0
    }
}

// =============================================================== registry

/// A metric's unit, for display only -- never used in a computation here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    /// 0..1, typically shown as a percentage.
    Ratio,
    /// A plain, unitless count.
    Count,
    /// A count already expressed as a weekly rate (`throughput_week`).
    PerWeek,
    Seconds,
    /// US dollars, API-equivalent (`unit_cost`) -- what the run's usage
    /// would have cost at the price table it was snapshotted with.
    Usd,
}

/// Which direction is an improvement for this metric -- display and
/// authoring guidance (a UI arrow, a check that a key result's own
/// `baseline`/`target` run the way the metric actually improves), not
/// something a computation here reads: `goals::score`'s linear formula
/// already infers direction from the sign of `target - baseline` alone
/// and never consults this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Better {
    Higher,
    Lower,
}

/// One metric's definition: what it means, its unit, which way is better,
/// which Factory data it projects, and whether it is computed yet.
///
/// [`registry`] lists these with `id` as a family's own pattern
/// (`compliance.<framework>`) where the metric has parameters; [`resolve`]
/// returns one with `id` bound to the concrete [`MetricId`] it was asked
/// about, and `title`/`description` filled in for that instance.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MetricDef {
    pub id: String,
    pub title: String,
    pub description: String,
    pub unit: Unit,
    pub better: Better,
    /// Which Factory data this metric projects -- documentation for a
    /// person or an agent reading the registry, not a pointer this module
    /// itself follows; computing the value is `factory-daemon`'s job.
    pub source: &'static str,
    /// `false` for a metric this module can name but `factory-daemon`
    /// cannot compute yet -- see `unavailable_reason`. [`resolve`] refuses
    /// one of these with [`MetricError::Unavailable`] rather than handing
    /// back a definition nothing can fill in.
    pub available: bool,
    pub unavailable_reason: Option<&'static str>,
}

fn fixed(
    id: &str,
    title: &str,
    description: &str,
    unit: Unit,
    better: Better,
    source: &'static str,
) -> MetricDef {
    MetricDef {
        id: id.to_string(),
        title: title.to_string(),
        description: description.to_string(),
        unit,
        better,
        source,
        available: true,
        unavailable_reason: None,
    }
}

fn throughput_week_def() -> MetricDef {
    fixed(
        "throughput_week",
        "Throughput per week",
        "Finished runs in the trailing 7 days.",
        Unit::PerWeek,
        Better::Higher,
        "production.rs's daily grid (/api/production), summed over the trailing 7 days",
    )
}

fn first_pass_yield_def() -> MetricDef {
    fixed(
        "first_pass_yield",
        "First-pass yield",
        "Finished runs that ended done without being rework -- a re-attempt of work that did \
         not succeed, not merely a repeat firing -- over finished, over the trailing 28 days. \
         Not `1 - reworked/finished`: a run scrapped on every attempt is `0.0` here, whatever \
         `reworked` says.",
        Unit::Ratio,
        Better::Higher,
        "production.rs's daily grid (/api/production): finished and first_pass, trailing 28 days",
    )
}

fn scrap_rate_def() -> MetricDef {
    fixed(
        "scrap_rate",
        "Scrap rate",
        "scrapped/finished, over the trailing 28 days.",
        Unit::Ratio,
        Better::Lower,
        "production.rs's daily grid (/api/production): finished and scrapped, trailing 28 days",
    )
}

/// The operations metrics' shared window: the same trailing 28 days the
/// production ratios use, so a KR over `fail_rate` and one over
/// `scrap_rate` are read over the same stretch of work.
const OPERATIONS_SOURCE: &str =
    "operations.rs's run facts over runs overlapping the trailing 28 days (TaskStore::runs_between)";

fn cycle_time_def(p: u32) -> MetricDef {
    fixed(
        &format!("cycle_time_p{p}"),
        &format!("Cycle time p{p}"),
        &format!(
            "The {p}th percentile (nearest rank) of started_at to ended_at, over runs that \
             ended done in the trailing 28 days. Done only: a run that failed or was \
             cancelled did not complete the work, and its length says nothing about how long \
             the work takes."
        ),
        Unit::Seconds,
        Better::Lower,
        OPERATIONS_SOURCE,
    )
}

fn queue_wait_p95_def() -> MetricDef {
    fixed(
        "queue_wait_p95",
        "Queue wait p95",
        "The 95th percentile (nearest rank) of queued_at to started_at -- how long a run \
         waited between becoming due and being dispatched -- over runs started in the \
         trailing 28 days. Runs from before queued_at was recorded are left out, never \
         counted as zero.",
        Unit::Seconds,
        Better::Lower,
        OPERATIONS_SOURCE,
    )
}

fn fail_rate_def() -> MetricDef {
    fixed(
        "fail_rate",
        "Fail rate",
        "failed/finished, over the trailing 28 days. Narrower than scrap_rate: a cancelled \
         run is scrap, not a failure.",
        Unit::Ratio,
        Better::Lower,
        OPERATIONS_SOURCE,
    )
}

fn rework_rate_def() -> MetricDef {
    fixed(
        "rework_rate",
        "Rework rate",
        "reworked/finished, over the trailing 28 days -- production.rs's own reworked \
         bucket: a retry, or a manual or workflow run of a task whose previous attempt failed \
         or was cancelled. A scheduled firing is never rework, however many came before it.",
        Unit::Ratio,
        Better::Lower,
        OPERATIONS_SOURCE,
    )
}

fn time_to_recover_p50_def() -> MetricDef {
    fixed(
        "time_to_recover_p50",
        "Time to recover p50",
        "The median (nearest rank) time from the first failed run of a failure streak ending \
         to the same task's next run ending done, over recoveries that completed in the \
         trailing 28 days. A streak with no success yet is not a recovery and is left out.",
        Unit::Seconds,
        Better::Lower,
        OPERATIONS_SOURCE,
    )
}

/// The cost metrics' shared source (#117): each run's usage, measured by
/// the agent runtime and mirrored on the run.
const USAGE_SOURCE: &str =
    "each run's usage (Run.usage, from AgentRuntime::usage snapshots) over runs that ended in the trailing 28 days \
     (usage::usage_metric over TaskStore::runs_between)";

fn unit_cost_def() -> MetricDef {
    fixed(
        "unit_cost",
        "Unit cost",
        "API-equivalent USD spent per finished unit: the cost of every run that finished in the \
         trailing 28 days -- failed and cancelled ones included, scrap is part of the price -- over \
         how many of them ended done. Only runs whose usage the runtime measured start to end count \
         on either side; an unmeasured run is left out, never taken as free.",
        Unit::Usd,
        Better::Lower,
        USAGE_SOURCE,
    )
}

fn tokens_per_run_def() -> MetricDef {
    fixed(
        "tokens_per_run",
        "Tokens per run",
        "The mean of every token type summed (input, output, cache read and write), over runs that \
         finished in the trailing 28 days with their usage measured start to end.",
        Unit::Count,
        Better::Lower,
        USAGE_SOURCE,
    )
}

fn compliance_def(framework: &str) -> MetricDef {
    fixed(
        &format!("compliance.{framework}"),
        &format!("Compliance share ({framework})"),
        &format!(
            "Share of {framework}'s counted (regulation/standard) controls that are \
             satisfied, attested, or not applicable, in the whole-instance subtree rollup."
        ),
        Unit::Ratio,
        Better::Higher,
        "policy::rollup over policy::worst_across_scopes, evaluated at the instance root (PolicyReport)",
    )
}

fn open_controls_def(framework: &str) -> MetricDef {
    fixed(
        &format!("open_controls.{framework}"),
        &format!("Open controls ({framework})"),
        &format!("Count of {framework}'s counted controls that are still open or stale, in the whole-instance subtree rollup."),
        Unit::Count,
        Better::Lower,
        "policy::rollup's open+stale counts, evaluated at the instance root (PolicyReport)",
    )
}

fn bench_resolve_rate_def(dataset: &str) -> MetricDef {
    fixed(
        &format!("bench.resolve_rate.{dataset}"),
        &format!("Bench resolve rate ({dataset})"),
        &format!("The newest settled bench run's resolve rate for dataset {dataset}; an unverified run never counts."),
        Unit::Ratio,
        Better::Higher,
        "bench::aggregate's newest settled BenchResult.resolve_rate",
    )
}

fn goal_tasks_done_def(objective: &str, kr: &str) -> MetricDef {
    fixed(
        &format!("goal_tasks_done.{objective}.{kr}"),
        &format!("Goal tasks done ({objective}/{kr})"),
        &format!(
            "Count of tasks labelled goal={objective}/{kr} whose newest run is done. A key \
             result usually writes just `metric: goal_tasks_done`; goals::KeyResult::bound_metric \
             fills in its own objective and key-result id."
        ),
        Unit::Count,
        Better::Higher,
        "task labels (goal=<objective>/<kr>), the newest run per labelled task",
    )
}

fn quality_def(characteristic: &str) -> MetricDef {
    fixed(
        &format!("quality.{characteristic}"),
        &format!("Quality scenarios met ({characteristic})"),
        &format!(
            "Share of every declared quality scenario under ISO 25010 characteristic \
             {characteristic} that is met, counted once per scope it applies in, across the \
             whole instance. A draft or no-data scenario counts against it: declared but not \
             shown to be met is not met. Company-wide only -- a scope name can hold `/`, which a \
             metric id segment cannot."
        ),
        Unit::Ratio,
        Better::Higher,
        "quality::evaluate over every scope's merged utility tree (/api/quality)",
    )
}

/// The environment metrics' shared source (`#185`).
const ENVIRONMENT_SOURCE: &str =
    "environments::report over the health samples and deployments the daemon records (/api/environments)";

/// One `<name>.<env>` metric of a running environment -- an SLA figure or
/// one of DORA's four keys for the promotion path ending there.
fn environment_def(name: &str, env: &str) -> Option<MetricDef> {
    let (title, description, unit, better) = match name {
        "availability" => (
            "Availability",
            "The share of healthy samples of every declared check over the environment's SLO window              (28 days without one). No samples is no value, never 100%.",
            Unit::Ratio,
            Better::Higher,
        ),
        "error_budget" => (
            "Error budget remaining",
            "What is left of the SLO's error budget over its window: 1 untouched, 0 spent, below 0              overspent. Needs an SLO.",
            Unit::Ratio,
            Better::Higher,
        ),
        "incidents" => (
            "Incidents",
            "Incidents -- two or more consecutive failures of a check, until it answers again --              open in the SLO window.",
            Unit::Count,
            Better::Lower,
        ),
        "mttr" => (
            "Mean time to restore",
            "The mean duration of the incidents that ended in the SLO window.",
            Unit::Seconds,
            Better::Lower,
        ),
        "deploy_frequency" => (
            "Deployment frequency (DORA)",
            "Successful deployments per week over the SLO window.",
            Unit::PerWeek,
            Better::Higher,
        ),
        "lead_time_p50" => (
            "Lead time for changes p50 (DORA)",
            "The median time from a released commit's committer time to its deployment finishing,              over successful deployments in the SLO window that recorded when their commit was made.",
            Unit::Seconds,
            Better::Lower,
        ),
        "change_failure_rate" => (
            "Change failure rate (DORA)",
            "The share of finished deployments in the SLO window that failed (a failed post-deploy \
             verification included), were rolled back, or were followed by an incident within an \
             hour, before the next deployment.",
            Unit::Ratio,
            Better::Lower,
        ),
        "time_to_restore_p50" => (
            "Time to restore p50 (DORA)",
            "The median duration of the incidents that ended in the SLO window.",
            Unit::Seconds,
            Better::Lower,
        ),
        _ => return None,
    };
    Some(fixed(
        &format!("{name}.{env}"),
        &format!("{title} ({env})"),
        description,
        unit,
        better,
        ENVIRONMENT_SOURCE,
    ))
}

/// The metric names every environment has, `availability.<env>` and so on.
pub const ENVIRONMENT_METRICS: [&str; 8] = [
    "availability",
    "error_budget",
    "incidents",
    "mttr",
    "deploy_frequency",
    "lead_time_p50",
    "change_failure_rate",
    "time_to_restore_p50",
];

/// The v1 metric registry, sorted by `id` (a family's own pattern for a
/// parameterised metric, e.g. `compliance.<framework>`).
pub fn registry() -> Vec<MetricDef> {
    let mut defs = vec![
        throughput_week_def(),
        first_pass_yield_def(),
        scrap_rate_def(),
        compliance_def("<framework>"),
        open_controls_def("<framework>"),
        bench_resolve_rate_def("<dataset>"),
        goal_tasks_done_def("<objective>", "<kr>"),
        quality_def("<characteristic>"),
        cycle_time_def(50),
        cycle_time_def(85),
        queue_wait_p95_def(),
        fail_rate_def(),
        rework_rate_def(),
        time_to_recover_p50_def(),
        unit_cost_def(),
        tokens_per_run_def(),
    ];
    defs.extend(ENVIRONMENT_METRICS.iter().filter_map(|m| environment_def(m, "<env>")));
    defs.sort_by(|a, b| a.id.cmp(&b.id));
    defs
}

/// Why [`resolve`] refused a [`MetricId`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MetricError {
    /// No family's pattern matches `id` -- an unknown metric name, or a
    /// family referenced unbound (e.g. bare `compliance` with no
    /// framework, or `goal_tasks_done` outside a key result that would
    /// bind it -- see `goals::KeyResult::bound_metric`).
    #[error("{0} is not a known metric")]
    Unknown(MetricId),
    /// The family exists, and `id` is shaped correctly, but
    /// `factory-daemon` cannot compute it yet. No metric is unavailable
    /// today -- `unit_cost` and `tokens_per_run`, the last two, became
    /// computable with #117 -- but the variant stays, and everything that
    /// handles it, so the next metric named before it can be computed has
    /// an honest place to go.
    #[error("{id} is not available yet: {reason}")]
    Unavailable { id: MetricId, reason: &'static str },
}

/// Turn a concrete [`MetricId`] into the [`MetricDef`] that describes it,
/// with any bound parameter (a framework, a dataset, an objective/key
/// result pair) filled into `id`, `title` and `description`. Matches each
/// registry family by exact segment count with every literal segment
/// equal -- `compliance` alone, or `compliance.cra.extra`, matches nothing
/// and is [`MetricError::Unknown`], the same as a name this module has
/// never heard of.
pub fn resolve(id: &MetricId) -> std::result::Result<MetricDef, MetricError> {
    let segments: Vec<&str> = id.as_str().split('.').collect();
    let def = match segments.as_slice() {
        ["throughput_week"] => throughput_week_def(),
        ["first_pass_yield"] => first_pass_yield_def(),
        ["scrap_rate"] => scrap_rate_def(),
        ["cycle_time_p50"] => cycle_time_def(50),
        ["cycle_time_p85"] => cycle_time_def(85),
        ["queue_wait_p95"] => queue_wait_p95_def(),
        ["fail_rate"] => fail_rate_def(),
        ["rework_rate"] => rework_rate_def(),
        ["time_to_recover_p50"] => time_to_recover_p50_def(),
        ["unit_cost"] => unit_cost_def(),
        ["tokens_per_run"] => tokens_per_run_def(),
        ["compliance", framework] => compliance_def(framework),
        ["open_controls", framework] => open_controls_def(framework),
        ["bench", "resolve_rate", dataset] => bench_resolve_rate_def(dataset),
        ["goal_tasks_done", objective, kr] => goal_tasks_done_def(objective, kr),
        // Bound like `compliance.<framework>`: this module knows no quality
        // catalogue (the same one-way rule it keeps with `goals.rs`), so a
        // characteristic ISO 25010 does not name still resolves here, and
        // `factory-daemon` answers it with `value: None` and the reason.
        ["quality", characteristic] => quality_def(characteristic),
        [name, env] if ENVIRONMENT_METRICS.contains(name) => {
            environment_def(name, env).expect("every ENVIRONMENT_METRICS name has a definition")
        }
        _ => return Err(MetricError::Unknown(id.clone())),
    };
    Ok(def)
}

// ================================================================ computed

/// One metric, computed once, as of a moment -- `factory-daemon`'s output,
/// not something this module produces. `value: None` with `reason: Some`
/// is a metric that could not be computed this time (an empty dataset, a
/// framework with no catalogue) -- distinct from [`MetricError::Unavailable`],
/// which is "this metric can never be computed yet", not "not this time".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricValue {
    pub id: MetricId,
    pub value: Option<f64>,
    pub as_of: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One metric's history, one point per day -- `factory-daemon`'s output.
/// A `NaiveDate` rather than a `DateTime<Utc>` per point: every v1 metric
/// this backs (production's daily grid, a bench run's settle date) is
/// already bucketed by calendar day, and a bare date is what a chart's x
/// axis wants without a time-of-day to throw away.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricSeries {
    pub id: MetricId,
    /// Chronological order, oldest first -- [`trend`] assumes it.
    pub points: Vec<(NaiveDate, f64)>,
}

/// Which way a series is moving, read off its two halves rather than just
/// its first and last point -- one noisy day at either end should not flip
/// the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trend {
    Up,
    Down,
    Flat,
}

/// `None` for fewer than two points -- nothing to compare. Otherwise the
/// mean of the first half of `points` against the mean of the second half;
/// a move smaller than 1% of the larger half's own scale reads as
/// [`Trend::Flat`] rather than noise being called a direction.
pub fn trend(series: &MetricSeries) -> Option<Trend> {
    let n = series.points.len();
    if n < 2 {
        return None;
    }
    let mid = n / 2;
    let mean = |slice: &[(NaiveDate, f64)]| slice.iter().map(|(_, v)| v).sum::<f64>() / slice.len() as f64;
    let first = mean(&series.points[..mid]);
    let second = mean(&series.points[mid..]);
    let delta = second - first;
    let scale = first.abs().max(second.abs()).max(1e-9);
    if delta.abs() / scale < 0.01 {
        Some(Trend::Flat)
    } else if delta > 0.0 {
        Some(Trend::Up)
    } else {
        Some(Trend::Down)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::is_slug;

    // -- MetricId ------------------------------------------------------

    #[test]
    fn a_metric_id_round_trips_through_its_string_form() {
        let id = MetricId::new("compliance.cra").unwrap();
        assert_eq!(id.to_string(), "compliance.cra");
        assert_eq!("compliance.cra".parse::<MetricId>().unwrap(), id);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"compliance.cra\"");
        assert_eq!(serde_json::from_str::<MetricId>(&json).unwrap(), id);
    }

    #[test]
    fn a_metric_id_accepts_underscores_and_hyphens_in_the_same_segment() {
        assert!(MetricId::new("goal_tasks_done.ship-compliant.cra-open-zero").is_ok());
        assert!(MetricId::new("bench.resolve_rate.eval-set-a").is_ok());
    }

    #[test]
    fn a_metric_id_refuses_the_wrong_shape() {
        assert!(MetricId::new("").is_err());
        assert!(MetricId::new("Compliance.cra").is_err());
        assert!(MetricId::new("compliance..cra").is_err());
        assert!(MetricId::new(".compliance").is_err());
        assert!(MetricId::new("compliance.").is_err());
        assert!(MetricId::new("compliance.<framework>").is_err());
    }

    #[test]
    fn every_string_dataset_is_slug_accepts_is_also_a_valid_metric_segment() {
        // The relationship the module doc comment claims: `MetricId`'s
        // charset is a strict superset of `dataset::is_slug`'s, so a
        // dataset, framework, objective or key-result id built by that
        // rule always binds cleanly into a metric id.
        for candidate in ["cra", "eval-set-a", "ship-compliant", "cra-open-zero", "a", "a1-b2"] {
            assert!(is_slug(candidate), "test fixture {candidate:?} is not even a slug");
            assert!(is_metric_segment(candidate), "{candidate:?} should be a valid metric segment");
        }
    }

    // -- registry --------------------------------------------------------

    #[test]
    fn the_registry_is_sorted_by_id_and_covers_every_v1_family() {
        let ids: Vec<String> = registry().into_iter().map(|d| d.id).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
        for expect in [
            "throughput_week",
            "first_pass_yield",
            "scrap_rate",
            "compliance.<framework>",
            "open_controls.<framework>",
            "bench.resolve_rate.<dataset>",
            "goal_tasks_done.<objective>.<kr>",
            "quality.<characteristic>",
            "cycle_time_p50",
            "cycle_time_p85",
            "queue_wait_p95",
            "fail_rate",
            "rework_rate",
            "time_to_recover_p50",
            "unit_cost",
            "tokens_per_run",
        ] {
            assert!(ids.iter().any(|id| id == expect), "missing {expect} in {ids:?}");
        }
    }

    #[test]
    fn unit_cost_and_tokens_per_run_are_available_since_runs_record_usage() {
        // #117: a run carries its usage now, so the two metrics that used
        // to be named but uncomputable (design §12.6) resolve like any other.
        for (id, unit) in [("unit_cost", Unit::Usd), ("tokens_per_run", Unit::Count)] {
            let def = resolve(&MetricId::new(id).unwrap()).unwrap();
            assert!(def.available, "{id}");
            assert_eq!(def.unavailable_reason, None, "{id}");
            assert_eq!((def.unit, def.better), (unit, Better::Lower), "{id}");
        }
        assert_eq!(serde_json::to_value(Unit::Usd).unwrap(), serde_json::json!("usd"));
    }

    #[test]
    fn the_operations_metrics_are_seconds_or_ratios_and_all_lower_is_better() {
        for (id, unit) in [
            ("cycle_time_p50", Unit::Seconds),
            ("cycle_time_p85", Unit::Seconds),
            ("queue_wait_p95", Unit::Seconds),
            ("fail_rate", Unit::Ratio),
            ("rework_rate", Unit::Ratio),
            ("time_to_recover_p50", Unit::Seconds),
        ] {
            let def = resolve(&MetricId::new(id).unwrap()).unwrap();
            assert_eq!(def.unit, unit, "{id}");
            assert_eq!(def.better, Better::Lower, "{id}");
            assert!(def.available, "{id}");
        }
    }

    // -- resolve -----------------------------------------------------------

    #[test]
    fn resolve_binds_a_fixed_metric_with_no_parameters() {
        let def = resolve(&MetricId::new("first_pass_yield").unwrap()).unwrap();
        assert_eq!(def.id, "first_pass_yield");
        assert!(def.available);
    }

    #[test]
    fn resolve_binds_a_single_parameter_family() {
        let def = resolve(&MetricId::new("compliance.cra").unwrap()).unwrap();
        assert_eq!(def.id, "compliance.cra");
        assert!(def.title.contains("cra"));
        assert!(def.description.contains("cra"));

        let def = resolve(&MetricId::new("open_controls.dsgvo").unwrap()).unwrap();
        assert_eq!(def.id, "open_controls.dsgvo");

        let def = resolve(&MetricId::new("quality.reliability").unwrap()).unwrap();
        assert_eq!(def.id, "quality.reliability");
        assert_eq!((def.unit, def.better), (Unit::Ratio, Better::Higher));
        assert!(matches!(resolve(&MetricId::new("quality").unwrap()), Err(MetricError::Unknown(_))));
    }

    #[test]
    fn resolve_binds_a_two_and_three_segment_family() {
        let def = resolve(&MetricId::new("bench.resolve_rate.eval-set-a").unwrap()).unwrap();
        assert_eq!(def.id, "bench.resolve_rate.eval-set-a");

        let def = resolve(&MetricId::new("goal_tasks_done.ship-compliant.cra-open-zero").unwrap()).unwrap();
        assert_eq!(def.id, "goal_tasks_done.ship-compliant.cra-open-zero");
        assert!(def.title.contains("ship-compliant/cra-open-zero"));
    }

    #[test]
    fn resolve_refuses_an_unbound_family_or_an_unknown_name() {
        assert_eq!(
            resolve(&MetricId::new("compliance").unwrap()),
            Err(MetricError::Unknown(MetricId::new("compliance").unwrap()))
        );
        assert!(matches!(
            resolve(&MetricId::new("compliance.cra.extra").unwrap()),
            Err(MetricError::Unknown(_))
        ));
        assert!(matches!(
            resolve(&MetricId::new("bogus_metric").unwrap()),
            Err(MetricError::Unknown(_))
        ));
        assert!(matches!(
            resolve(&MetricId::new("goal_tasks_done").unwrap()),
            Err(MetricError::Unknown(_))
        ));
    }

    #[test]
    fn every_fixed_registry_entry_round_trips_through_resolve() {
        // `registry()` and `resolve()` each spell out the fixed (no `<..>`
        // placeholder) metrics independently; this pins them together so a
        // future edit to one that forgets the other fails a test instead of
        // silently drifting.
        for entry in registry() {
            if entry.id.contains('<') {
                continue;
            }
            let id = MetricId::new(entry.id.clone()).unwrap();
            match resolve(&id) {
                Ok(resolved) => assert_eq!(resolved, entry, "resolve({}) != registry()'s own entry", entry.id),
                Err(MetricError::Unavailable { .. }) => assert!(!entry.available, "{} is available in the registry but resolve refuses it", entry.id),
                Err(e) => panic!("resolve({}) failed: {e}", entry.id),
            }
        }
    }

    // -- trend -------------------------------------------------------------

    fn series(values: &[f64]) -> MetricSeries {
        let start = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        MetricSeries {
            id: MetricId::new("throughput_week").unwrap(),
            points: values
                .iter()
                .enumerate()
                .map(|(i, v)| (start + chrono::Duration::days(i as i64), *v))
                .collect(),
        }
    }

    #[test]
    fn trend_is_none_for_fewer_than_two_points() {
        assert_eq!(trend(&series(&[])), None);
        assert_eq!(trend(&series(&[5.0])), None);
    }

    #[test]
    fn trend_reads_a_clear_rise_or_fall() {
        assert_eq!(trend(&series(&[1.0, 1.0, 5.0, 5.0])), Some(Trend::Up));
        assert_eq!(trend(&series(&[5.0, 5.0, 1.0, 1.0])), Some(Trend::Down));
    }

    #[test]
    fn trend_reads_a_tiny_move_as_flat() {
        assert_eq!(trend(&series(&[10.0, 10.0, 10.001, 10.001])), Some(Trend::Flat));
        assert_eq!(trend(&series(&[0.0, 0.0, 0.0, 0.0])), Some(Trend::Flat));
    }
}
