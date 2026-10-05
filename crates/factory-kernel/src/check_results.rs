//! Plain check-result schema. Judgement, precedence, gathering and time
//! evaluation remain with L5, never in this shared vocabulary.
use crate::ControlRef;
use serde::{Deserialize, Serialize};

/// A control's status carries its own reasons, one per check that
/// contributed to it (including a check whose evidence was never gathered,
/// or whose `daemon`/`secrets` name this build does not recognize, whose
/// reason says so) -- so a caller never has to go re-derive why a control is
/// `open` from the checks alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusKind {
    Satisfied,
    Attested,
    Stale,
    Open,
    NotApplicable,
}

impl StatusKind {
    /// The wire's own spelling (`#[serde(rename_all = "snake_case")]`) --
    /// `"not_applicable"`, not the `n/a` shorthand a person reads on the L6
    /// tab or the CLI's status board; a caller that wants that shorter word
    /// does its own translation (`policy-model.js`'s `statusLabel`). One
    /// place for the CLI (`policy_control_text`, formerly its own
    /// `policy_status_str`) and `policy_export::export_markdown` (`#83`) to
    /// agree on the string.
    pub fn as_str(self) -> &'static str {
        match self {
            StatusKind::Satisfied => "satisfied",
            StatusKind::Attested => "attested",
            StatusKind::Stale => "stale",
            StatusKind::Open => "open",
            StatusKind::NotApplicable => "not_applicable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Status {
    /// A check found current evidence.
    Satisfied { reasons: Vec<String> },
    /// An unexpired attestation covers it.
    Attested { reasons: Vec<String> },
    /// Evidence existed but is older than `max_age`, or the attestation
    /// covering it expired.
    Stale { reasons: Vec<String> },
    /// No evidence.
    Open { reasons: Vec<String> },
    /// Does not apply here.
    NotApplicable { reasons: Vec<String> },
}

impl Status {
    pub fn kind(&self) -> StatusKind {
        match self {
            Status::Satisfied { .. } => StatusKind::Satisfied,
            Status::Attested { .. } => StatusKind::Attested,
            Status::Stale { .. } => StatusKind::Stale,
            Status::Open { .. } => StatusKind::Open,
            Status::NotApplicable { .. } => StatusKind::NotApplicable,
        }
    }

    pub fn reasons(&self) -> &[String] {
        match self {
            Status::Satisfied { reasons }
            | Status::Attested { reasons }
            | Status::Stale { reasons }
            | Status::Open { reasons }
            | Status::NotApplicable { reasons } => reasons,
        }
    }
}

/// What kind of thing an `EvidenceRef` points at -- exactly the id spaces
/// `evaluate` ever has one for. `knowledge` evidence has no ref yet:
/// `Evidence.tags` only knows a tag is present, never which page carries it,
/// so there is nothing cheap to point at until that changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceRefKind {
    Task,
    Run,
    WorkflowRun,
    BenchRun,
    Attestation,
}

impl EvidenceRefKind {
    /// The wire's own spelling -- `policy_export::export_markdown` (`#83`)
    /// prints a ref as `<kind>:<id>`, and this is the `<kind>`.
    pub fn as_str(self) -> &'static str {
        match self {
            EvidenceRefKind::Task => "task",
            EvidenceRefKind::Run => "run",
            EvidenceRefKind::WorkflowRun => "workflow_run",
            EvidenceRefKind::BenchRun => "bench_run",
            EvidenceRefKind::Attestation => "attestation",
        }
    }
}

/// A machine-readable pointer alongside a status's human `reasons`, so a UI
/// can link straight to the task, run, workflow run, bench run or
/// attestation that made a control what it is. Additive: only ever added
/// where a check's evidence already carries the id, never invented for the
/// occasion.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub kind: EvidenceRefKind,
    pub id: String,
}

impl EvidenceRef {
    pub fn task(id: impl Into<String>) -> Self {
        Self {
            kind: EvidenceRefKind::Task,
            id: id.into(),
        }
    }
    pub fn run(id: impl Into<String>) -> Self {
        Self {
            kind: EvidenceRefKind::Run,
            id: id.into(),
        }
    }
    pub fn workflow_run(id: impl Into<String>) -> Self {
        Self {
            kind: EvidenceRefKind::WorkflowRun,
            id: id.into(),
        }
    }
    pub fn bench_run(id: impl Into<String>) -> Self {
        Self {
            kind: EvidenceRefKind::BenchRun,
            id: id.into(),
        }
    }
    pub fn attestation(id: impl Into<String>) -> Self {
        Self {
            kind: EvidenceRefKind::Attestation,
            id: id.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvaluationResult<K = ()> {
    pub control: ControlRef,
    pub title: String,
    pub kind: K,
    /// Machine-readable pointers alongside `status`'s reasons -- see
    /// `EvidenceRef`. Empty whenever nothing behind the status carries an
    /// id yet (a `knowledge`/`roles`/`sandbox`/`secrets`/`daemon` check,
    /// `n/a`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<EvidenceRef>,
    #[serde(flatten)]
    pub status: Status,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceFinding {
    pub subject: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeCheckEvaluation {
    pub scope: String,
    pub statuses: Vec<CheckObservation>,
    pub findings: Vec<EvidenceFinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckObservation {
    pub control: ControlRef,
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<EvidenceRef>,
    #[serde(flatten)]
    pub status: Status,
}

/// Live, kindless L5 evaluation. L6 classifications/reports never travel
/// back up through this fact; a declaration owner retains its own kinds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckEvaluationFact {
    pub at: chrono::DateTime<chrono::Utc>,
    pub scopes: Vec<ScopeCheckEvaluation>,
}

/// Two authored subject sets evaluated by L5 against one primary-set
/// evidence gather. Neither this schema nor L6 evaluates lower evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckComparisonFact {
    pub at: chrono::DateTime<chrono::Utc>,
    pub scopes: Vec<ScopeCheckComparison>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeCheckComparison {
    pub scope: String,
    pub primary: Vec<CheckObservation>,
    pub alternative: Vec<CheckObservation>,
}
