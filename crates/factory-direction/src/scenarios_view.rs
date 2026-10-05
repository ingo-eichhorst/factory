//! Canonical L6 Scenarios response data; transport paths re-export it.
use serde::{Deserialize, Serialize};

// ============================================================= scenarios

/// One scope's exact policy delta under a scenario -- `factory_core::scenario::PolicyDelta`
/// paired with which scope it is about, since a report carries one per scope
/// in the asked subtree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioScopeDelta {
    pub scope: String,
    pub delta: crate::scenario::PolicyDelta,
}

/// The state every scenario in a `ScenariosReport` is compared against --
/// computed once per request and shared, the same "compute once, project
/// many ways" shape `Request::Policy`'s own dataset/daemon facts already
/// follow, rather than recomputed per card.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioBaseline {
    /// Every metric id any loaded scenario's signposts, goal changes, or the
    /// built-in driver tree itself reference -- computed once
    /// (`Engine::metrics`), not once per scenario.
    pub metrics: Vec<factory_assurance::metrics::MetricValue>,
    /// `factory_core::scenario::driver_defs()`'s own baseline value for
    /// every driver: a registry-backed driver's current metric value (when
    /// it has one to read), `capacity_factor`'s neutral `1.0` (no
    /// adjustment, the same default `evaluate_outcomes` itself falls back to
    /// when a driver is absent), and `rework_rate`'s neutral `0.0` (no
    /// rework) -- see `Engine::driver_baseline`'s own doc comment for why
    /// those two, not `evaluate_outcomes`' formula, get a value here.
    pub drivers: std::collections::BTreeMap<crate::scenario::DriverId, f64>,
    /// The current policy rollup over the asked subtree -- the same
    /// `policy::FrameworkRollup` list `PolicyReport::rollup` carries,
    /// computed from the very statuses every scenario's own delta is
    /// diffed against, not a second, separate call to `Request::Policy`.
    pub policy: Vec<crate::policy::FrameworkRollup>,
    /// What happens with no scenario at all: `forecast_completion` over the
    /// asked subtree's own weekly throughput history, backlog = every
    /// non-terminal task in scope right now, `Horizon::default()`'s 26
    /// weeks -- the same `Forecast` shape every `ScenarioResult::forecast`
    /// carries, so a fan chart can draw baseline and scenario side by side.
    /// See `Engine::scenarios_report`'s own doc comment for why this reading
    /// of "baseline forecast" was chosen over a bare metric trend.
    pub forecast: crate::scenario::Forecast,
}

/// One scenario's own computed answer -- the exact policy delta, the driver
/// outcomes and tornado, the Monte Carlo forecast, goal-scenario
/// probabilities, and signposts, all evaluated against
/// `ScenariosReport::baseline`. See the module doc comment on
/// `factory_core::scenario` for how these three kinds of answer (exact,
/// probabilistic, qualitative) are never mixed into one number.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioResult {
    pub scenario: crate::scenario::Scenario,
    /// This scenario's own findings -- its load-time validation, its own
    /// staleness check, and its own `overlay_chain` findings -- a subset of
    /// `ScenariosReport::findings` repeated here so a card can show just its
    /// own without re-filtering the whole report's list by subject.
    pub findings: Vec<crate::scenario::Finding>,
    /// Empty when the scenario carries no `policy:` overlay at all -- there
    /// is nothing to diff.
    pub policy: Vec<ScenarioScopeDelta>,
    /// `policy`, aggregated over the whole asked subtree the same way
    /// `PolicyReport::rollup` aggregates per-scope statuses --
    /// `scenario::policy_delta` over each side's own `policy::worst_across_scopes`.
    pub policy_subtree: crate::scenario::PolicyDelta,
    pub drivers: ScenarioDrivers,
    /// The backlog `forecast` was run against, and where it came from --
    /// carried alongside the forecast itself since "how big is the backlog"
    /// is exactly what a scenario's policy delta and `goals:` labels decide,
    /// not a fixed number a card can otherwise guess at.
    pub backlog: ScenarioBacklog,
    pub forecast: crate::scenario::Forecast,
    /// One per `scenario.goals` entry, in authored order.
    pub goals: Vec<ScenarioGoalProbability>,
    pub signposts: Vec<crate::scenario::SignpostStatus>,
}

/// A scenario's own driver tree: the shared baseline, this scenario's own
/// overrides applied on top, the outcome before and after, and the tornado
/// ranking the swing by driver -- everything `ScenarioDrivers` needs to draw
/// a driver panel and its drill-down without a second round trip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioDrivers {
    pub overridden: std::collections::BTreeMap<crate::scenario::DriverId, f64>,
    pub outcomes_before: std::collections::BTreeMap<crate::scenario::OutcomeId, f64>,
    pub outcomes_after: std::collections::BTreeMap<crate::scenario::OutcomeId, f64>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub outcome_reasons_before: std::collections::BTreeMap<crate::scenario::OutcomeId, String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub outcome_reasons_after: std::collections::BTreeMap<crate::scenario::OutcomeId, String>,
    /// Sensitivity for every measured outcome, including USD and tokens.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub tornados:
        std::collections::BTreeMap<crate::scenario::OutcomeId, Vec<crate::scenario::TornadoBar>>,
    /// Against `effective_throughput`, retained for existing clients.
    pub tornado: Vec<crate::scenario::TornadoBar>,
}

/// Where a scenario's `forecast` backlog came from -- see
/// `Engine::scenarios_report`'s own doc comment for the exact rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioBacklog {
    pub total: f64,
    /// Newly-open controls, from this scenario's own `policy_subtree` delta
    /// -- one remediation item each.
    pub newly_open_controls: usize,
    /// Non-terminal tasks labelled `goal=<objective>/<kr>` for one of this
    /// scenario's own `goals:` entries, summed across every entry.
    pub open_goal_tasks: usize,
}

/// One `scenario.goals` entry's re-scored probability -- see
/// `factory_core::scenario::goal_probability`. `target`/`by` are the
/// *effective* values (the change's own, or the key result's/cycle's
/// current one when the change leaves it unnamed), not merely echoing the
/// authored `GoalChange`, so a card never has to re-resolve what "unwritten"
/// defaulted to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioGoalProbability {
    pub kr: crate::goals::KrRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<chrono::NaiveDate>,
    pub probability: crate::scenario::GoalProbability,
}

/// A signpost that is `Triggered`, named alongside the scenario it belongs
/// to -- `ScenariosReport::triggered`'s own element. Exists because a
/// triggered signpost is meant to be visible outside the Scenarios tab too
/// (on the dashboard, in the inbox, as an observation with no automatic
/// consequence -- design §8) and a reader building that view should not have
/// to walk every `ScenarioResult::signposts` and filter by state itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggeredSignpost {
    pub scenario: String,
    pub metric: factory_assurance::metrics::MetricId,
    pub reason: String,
}
impl TryFrom<factory_kernel::SignpostObservation> for TriggeredSignpost {
    type Error = String;
    fn try_from(row: factory_kernel::SignpostObservation) -> Result<Self, Self::Error> {
        Ok(Self {
            scenario: row.scenario,
            metric: factory_assurance::metrics::MetricId::new(row.metric)?,
            reason: row.reason,
        })
    }
}

/// The L6 Scenarios tab's whole answer: `Request::Scenarios`'s response.
/// `#100`. A scenario file never changes the real config -- everything here
/// is computed fresh, in memory, against `.factory/scenarios/`,
/// `.factory/policies/` (and its `drafts/` subdirectory), and whatever
/// `baseline` itself reads, on every call, like `PolicyReport`/`GoalsReport`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenariosReport {
    /// `None` when the whole instance was asked about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    pub baseline: ScenarioBaseline,
    /// Every scenario on disk, sorted by name (`scenario::load`'s own order).
    pub scenarios: Vec<ScenarioResult>,
    /// Load-time validation, staleness, and overlay findings --
    /// `factory_core::scenario::Finding`s only; a catalogue's own load or
    /// applicability findings (real or draft) are `policy_findings` below,
    /// the same split `PolicyReport::findings` already draws between a
    /// catalogue mistake and an applicability one.
    pub findings: Vec<crate::scenario::Finding>,
    /// `policy::load_all`'s and `scenario::load_drafts`'s own catalogue
    /// findings, plus every scope's `policy::applicable`/`evidence_findings`
    /// output across both the baseline and every scenario's own evaluation,
    /// deduplicated.
    pub policy_findings: Vec<crate::policy::Finding>,
    /// Every currently `Triggered` signpost across every scenario -- see
    /// `TriggeredSignpost`'s own doc comment for why this is surfaced
    /// outside `scenarios[].signposts` too.
    pub triggered: Vec<TriggeredSignpost>,
}

/// The answer to `Request::ScenarioWhatIf`: the driver tree recomputed with
/// the request's own overrides layered over the named scenario's (if any),
/// and the forecast that follows from it -- see `Engine::scenario_whatif`'s
/// own doc comment for exactly how backlog is chosen with no scenario named.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioWhatIfResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario: Option<String>,
    pub drivers: ScenarioDrivers,
    pub forecast: crate::scenario::Forecast,
}
