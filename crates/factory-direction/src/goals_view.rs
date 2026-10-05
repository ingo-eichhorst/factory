//! Plain L6 Goals response projections; wire paths canonically re-export these.
use serde::{Deserialize, Serialize};

/// One cycle's place in `GoalsReport::cycles`: enough to draw a picker or a
/// timeline without evaluating every cycle's full report, which
/// `Request::Goals` only ever does for the one asked (or current) cycle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CycleSummary {
    pub id: String,
    pub from: chrono::NaiveDate,
    pub to: chrono::NaiveDate,
    pub status: crate::goals::CycleStatus,
    /// The mean of every scope-filtered objective's own score in this
    /// cycle -- `None` when none of them are scored yet, the same
    /// "unscored, not zero" rule `goals::ObjectiveResult::score` follows.
    pub score: Option<f64>,
}

/// The north star metric, as `Request::Goals` shows it: `direction.yaml`'s
/// own `why`, plus its current computed value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NorthStarView {
    pub metric: factory_assurance::metrics::MetricId,
    pub why: String,
    pub value: factory_assurance::metrics::MetricValue,
}

/// One of `direction.yaml`'s `inputs`, with its current computed value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputView {
    pub metric: factory_assurance::metrics::MetricId,
    pub value: factory_assurance::metrics::MetricValue,
}

/// The L6 Goals tab's whole answer: `Request::Goals`'s response. Goals
/// enforce nothing (design §8) -- this is a read of what is authored and
/// what the data says about it, never a status this itself computes and
/// keeps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoalsReport {
    /// `None` when the whole instance was asked about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<crate::goals::Direction>,
    /// Every cycle on disk, oldest first, whatever cycle was asked for.
    pub cycles: Vec<CycleSummary>,
    /// The asked cycle's (or, with none named, the current one's) full
    /// graded report, scope-filtered the same way `cycles`' own scores are.
    /// `None` when no cycle was asked for and none is current right now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<crate::goals::CycleReport>,
    pub findings: Vec<crate::goals::Finding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub north_star: Option<NorthStarView>,
    pub inputs: Vec<InputView>,
    /// Scope-filtered the same way `report`'s own objectives are -- an item
    /// with no `scope` of its own belongs to the root, exactly like an
    /// objective without one.
    pub roadmap: Vec<crate::goals::RoadmapItem>,
    /// Every manual key result's own check-in history, oldest first, for a
    /// sparkline -- across every cycle, not narrowed to `report`'s own one,
    /// since a key result's history outlives the cycle it happens to be
    /// asked about. `goals::KrResult::confidence` already carries the
    /// latest one; this is the series behind it.
    pub checkins: std::collections::BTreeMap<crate::goals::KrRef, Vec<crate::goals::CheckIn>>,
}
