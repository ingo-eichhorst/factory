//! Plain L4 spend schema (#164/#193). Aggregation and evaluation stay in their owners.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// What `factory cost` and `GET /api/costs` group by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostGroupBy {
    #[default]
    Task,
    /// The task's `issue=<n>` label.
    Issue,
    Scope,
    /// The agent that ran it, within its scope -- `scope/agent`, since an
    /// agent's name is only unique inside one.
    Agent,
    /// The configured provider account snapshotted on the run.
    Provider,
    /// The task's `workflow_origin.workflow_id`, when it has one (#164).
    Workflow,
}

impl CostGroupBy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Issue => "issue",
            Self::Scope => "scope",
            Self::Agent => "agent",
            Self::Provider => "provider",
            Self::Workflow => "workflow",
        }
    }
}

impl std::str::FromStr for CostGroupBy {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "task" => Self::Task,
            "issue" => Self::Issue,
            "scope" => Self::Scope,
            "agent" => Self::Agent,
            "provider" => Self::Provider,
            "workflow" => Self::Workflow,
            other => return Err(format!(
                "cannot group costs by {other:?}: use task, issue, scope, agent, provider or workflow"
            )),
        })
    }
}

/// Token sums, where each is the total over the runs that knew it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenSums {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

/// One group's sums. Every sum is over the runs that knew that number, and
/// the counts beside it say how many did not -- a run whose usage is
/// unknown is counted, never silently dropped.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CostRow {
    pub key: String,
    /// Something readable for `key` when it is an id -- a task's title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub runs: u32,
    /// Runs whose usage could not be measured at all.
    pub runs_unknown: u32,
    /// Measured runs whose figures are a lower bound.
    pub runs_partial: u32,
    pub tokens: TokenSums,
    /// Measured runs with at least one token type unknown, so `tokens` is
    /// short by whatever they used of it.
    pub runs_tokens_incomplete: u32,
    pub cost_usd: f64,
    /// Measured runs with no cost, so `cost_usd` is short by theirs.
    pub runs_cost_unknown: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pricing_sources: Vec<String>,
    /// Runs with an `original_estimate` at all -- `#168`'s estimate-vs-actual,
    /// alongside the usage sums above rather than replacing them. A run with
    /// no estimate is in neither this nor `within_range`: it has no range to
    /// have missed. `#[serde(default)]`: absent on every report from before
    /// this existed.
    #[serde(default)]
    pub estimated_runs: u32,
    /// Of `estimated_runs`, how many are terminal with a wall time inside
    /// their own `[low, high]`. A run still going is counted in
    /// `estimated_runs` but not here -- it has no actual yet to compare.
    #[serde(default)]
    pub within_range: u32,
    /// The nearest-rank median, over `estimated_runs`' own terminal ones, of
    /// actual wall seconds over the estimate's `expected` -- 1.0 is exactly
    /// on the mark, over 1.0 ran long. `None` with nothing to take a median
    /// of, never 0 or 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub median_actual_over_expected: Option<f64>,
}

impl CostRow {
    pub fn new(key: impl Into<String>, label: Option<String>) -> Self {
        Self {
            key: key.into(),
            label,
            ..Default::default()
        }
    }
}

/// UTC run-start day, not the day a charge was billed by a provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DailySpend {
    pub day: chrono::NaiveDate,
    pub spent: CostRow,
    pub unattributed_runs: u32,
}

/// Spend surfaces charge runs by start time. Measured forecasting inputs
/// preserve the usage metrics' finished-run cohort and `(from, to]` window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpendBasis {
    #[default]
    Started,
    Finished,
}

fn is_started(basis: &SpendBasis) -> bool {
    *basis == SpendBasis::Started
}

/// Plain shared spend query (#164/#193): runs that started in `[from, to)`
/// (`from` defaults to thirty days before `to`, `to` to now), narrowed to
/// `scope`'s own subtree when given, summed per `group_by`. Plain serde
/// data -- the same shape whether it comes off the wire (`Request::Costs`)
/// or is built in-process (`cost_week`). Evaluation stays with each owner.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpendQuery {
    #[serde(default)]
    pub basis: SpendBasis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<DateTime<Utc>>,
    #[serde(default)]
    pub group_by: CostGroupBy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpendFigure {
    pub value: Option<f64>,
    pub reason: Option<String>,
    pub as_of: Option<DateTime<Utc>>,
}

/// Measured-subset values never conceal incomplete cohort coverage: the
/// reason names missing/partial/unattributed observations when applicable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FinishedSpend {
    pub unit_cost: SpendFigure,
    pub tokens_per_run: SpendFigure,
}

/// Live L4 spend, with backward-compatible cost API JSON. Totals contain only known
/// amounts; the adjacent unknown/partial counters are part of the fact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostReport {
    #[serde(default, skip_serializing_if = "is_started")]
    pub basis: SpendBasis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished: Option<FinishedSpend>,
    pub group_by: CostGroupBy,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Most expensive first.
    pub rows: Vec<CostRow>,
    pub total: CostRow,
    /// Runs whose scope cannot be recovered from a task/current scope.
    /// For a narrowed query they are not in its sums but could belong to
    /// that subtree. All-instance sums still include these runs normally.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub unattributed_runs: u32,
    /// Read-on-request history; no second aggregate store or repricing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub daily: Vec<DailySpend>,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}
