//! Shared metric identity and plain live L5 values. Registry resolution,
//! arithmetic, series shaping and trend evaluation stay in L5.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
pub fn is_metric_segment(seg: &str) -> bool {
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

/// One metric, computed by L5 as of a moment. `value: None` with `reason: Some`
/// is a metric that could not be computed this time (an empty dataset, a
/// framework with no catalogue) -- distinct from L5's permanently unavailable metrics,
/// which is "this metric can never be computed yet", not "not this time".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricValue {
    pub id: MetricId,
    pub value: Option<f64>,
    pub as_of: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Current metric values, computed by L5 from its live lower fact ports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricValuesFact {
    pub values: Vec<MetricValue>,
}
