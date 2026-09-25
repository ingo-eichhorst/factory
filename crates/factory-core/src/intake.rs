//! Intake (`#119`): the inbound quality gate at the line's commitment point.
//!
//! Work that arrives through intake is a task in `TaskStatus::Intake` -- held
//! back from dispatch, with no run -- carrying an [`Intake`]
//! record: where it came from, who asked, and how far triage has got. Triage
//! is Irrlicht's `ir:triage` skill generalised: seven readiness axes, each
//! pass or fail with an evidence sentence; a category; a priority from impact
//! x urgency; a range estimate from a complexity level; and a route to a
//! scope and, optionally, a workflow. [`evaluate`] turns an assessment into
//! the verdict the rules give, and [`Decision`] is what a triager does with
//! it: release it (`ready`), send it back (`needs-info`), or close it
//! (`wontfix`, only with a verified reason).
//!
//! Everything here is pure: no store, no clock but the one passed in. The
//! daemon (`factory-daemon/src/intake.rs`) owns the transitions.
//!
//! What v1 leaves out, on purpose: the GitHub-issue source, duplicate
//! detection, the security fast lane with its CRA clock, the `triager` role
//! and the metrics -- all `#119` v2 -- and every outbound effect. An
//! assessment is never posted anywhere; it is only journaled.

use crate::task::{Task, TaskStatus};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ------------------------------------------------------------------ receipt

/// How far an intake item has got. `Ready` and `Wontfix` are where it leaves
/// intake; the rest keep the task in `TaskStatus::Intake`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntakeStage {
    /// Arrived, nobody has looked at it yet -- or information it was waiting
    /// for has come in and it is back in the queue.
    Received,
    /// A triage run is working on it, or an assessment is recorded and waits
    /// for a decision.
    Triaging,
    /// Sent back to the requester with the failed axes and the questions.
    NeedsInfo,
    /// Released into the line.
    Ready,
    /// Closed with a verified reason.
    Wontfix,
}

impl IntakeStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Received => "received",
            Self::Triaging => "triaging",
            Self::NeedsInfo => "needs_info",
            Self::Ready => "ready",
            Self::Wontfix => "wontfix",
        }
    }

    /// Still inside the gate: the task is `TaskStatus::Intake`.
    pub fn is_open(self) -> bool {
        matches!(self, Self::Received | Self::Triaging | Self::NeedsInfo)
    }
}

/// Which way an item came in. v1 has the three the daemon itself can see;
/// the GitHub issue, email and chat sources are v2 and v3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// `factory intake add`, typed by a person.
    Cli,
    /// The web UI's Intake view.
    Ui,
    /// An agent handing work on instead of creating a task directly -- the
    /// delegation path. Set by the daemon from the caller, never claimed.
    Agent,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Ui => "ui",
            Self::Agent => "agent",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntakeSource {
    pub kind: SourceKind,
    /// Whatever identifies the item where it came from -- an issue URL, a
    /// mail id, the task an agent was working on. Free text, never followed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

/// What a caller sends to put something into intake.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NewIntake {
    pub title: String,
    #[serde(default)]
    pub instructions: String,
    /// The scope it is thought to belong to. Triage routes it, and may move
    /// it; absent means the instance's first scope, as for a task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// `cli` or `ui`. An agent's request is always `agent`, whatever it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// On whose behalf, when that is not the caller -- a customer, a
    /// colleague. Absent means the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

/// The intake record on a task. Lives in the task's JSON row, so it needed
/// no schema change; absent on every task that never went through intake.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Intake {
    pub stage: IntakeStage,
    pub source: IntakeSource,
    /// Who asked: a person's name, `the owner`, or `agent <name>`.
    pub requester: String,
    pub received_at: DateTime<Utc>,
    /// The latest assessment. A re-triage replaces it; every earlier one is
    /// still in the journal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triage: Option<Triage>,
    /// The task a triage run is working in, when one was started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triage_task: Option<String>,
    /// The open questions of a `needs-info`, cleared once information comes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<String>,
    /// How it left intake, once it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<DecisionRecord>,
}

// ------------------------------------------------------------------ triage

/// The seven readiness axes of `ir:triage`, in its order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    Scope,
    Specification,
    Verifiability,
    Observability,
    Context,
    Independence,
    Reversibility,
}

impl Axis {
    pub const ALL: [Axis; 7] = [
        Axis::Scope,
        Axis::Specification,
        Axis::Verifiability,
        Axis::Observability,
        Axis::Context,
        Axis::Independence,
        Axis::Reversibility,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scope => "scope",
            Self::Specification => "specification",
            Self::Verifiability => "verifiability",
            Self::Observability => "observability",
            Self::Context => "context",
            Self::Independence => "independence",
            Self::Reversibility => "reversibility",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Scope => "Scope",
            Self::Specification => "Specification",
            Self::Verifiability => "Verifiability",
            Self::Observability => "Observability",
            Self::Context => "Context",
            Self::Independence => "Independence",
            Self::Reversibility => "Reversibility",
        }
    }

    /// The pass condition, verbatim from `ir:triage` but for the product.
    pub fn pass_condition(self) -> &'static str {
        match self {
            Self::Scope => "The affected behaviour and likely components are bounded.",
            Self::Specification => "No external or hard-to-reverse decision remains.",
            Self::Verifiability => "A concrete signal can distinguish success from failure.",
            Self::Observability => "Existing or small new instrumentation can expose that signal.",
            Self::Context => "Relevant code, documents and prior work are cited or directly findable.",
            Self::Independence => "No unresolved issue, task, credential or human decision blocks the work.",
            Self::Reversibility => "The change is local and can be reverted safely.",
        }
    }
}

impl std::str::FromStr for Axis {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Axis::ALL
            .into_iter()
            .find(|a| a.as_str() == s.trim().to_ascii_lowercase())
            .ok_or_else(|| {
                let names: Vec<&str> = Axis::ALL.iter().map(|a| a.as_str()).collect();
                format!("unknown readiness axis {s:?}; the seven are {}", names.join(", "))
            })
    }
}

/// What closing a failed Observability axis would take. Low and medium are
/// work the task itself can do first; high is a prerequisite of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservabilityCost {
    /// Run an existing recording, replay, CLI, metric or snapshot tool.
    Low,
    /// Extend an existing event, metric, fixture or snapshot surface.
    Medium,
    /// Build a new observation system.
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxisCheck {
    pub axis: Axis,
    pub pass: bool,
    /// One sentence of evidence, pass or fail. An axis without one is not
    /// assessed, and is refused.
    pub evidence: String,
    /// Only for a failed Observability axis, where it is required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<ObservabilityCost>,
}

/// The categories `#118`'s control plans key on. A category is a slug, and
/// one not listed here is accepted -- the list is what the UI offers first,
/// not a closed set.
pub const CATEGORIES: [&str; 8] = [
    "feature",
    "bugfix",
    "release",
    "publish",
    "docs",
    "chore",
    "security-report",
    "customer-request",
];

/// Impact or urgency, each on ITIL's three levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    High,
    Medium,
    Low,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

impl std::str::FromStr for Level {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "high" | "h" => Self::High,
            "medium" | "med" | "m" => Self::Medium,
            "low" | "l" => Self::Low,
            other => return Err(format!("unknown level {other:?}; use high, medium or low")),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Priority {
    P1,
    P2,
    P3,
    P4,
}

impl Priority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::P1 => "P1",
            Self::P2 => "P2",
            Self::P3 => "P3",
            Self::P4 => "P4",
        }
    }
}

/// Impact x urgency, ITIL's matrix folded onto four priorities: the sum of
/// the two levels (high 0, medium 1, low 2) picks the row, so one step down
/// either axis costs one priority, and low/low shares P4 with the two
/// neighbours next to it rather than getting a fifth level nobody acts on.
///
/// | impact \ urgency | high | medium | low |
/// |---|---|---|---|
/// | high   | P1 | P2 | P3 |
/// | medium | P2 | P3 | P4 |
/// | low    | P3 | P4 | P4 |
pub fn priority(impact: Level, urgency: Level) -> Priority {
    let rank = |l: Level| match l {
        Level::High => 0,
        Level::Medium => 1,
        Level::Low => 2,
    };
    match rank(impact) + rank(urgency) {
        0 => Priority::P1,
        1 => Priority::P2,
        2 => Priority::P3,
        _ => Priority::P4,
    }
}

/// A time range, agent-active. The range is the estimate; `midpoint` is the
/// single number a task's advisory `estimate_seconds` gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Estimate {
    pub min_seconds: u64,
    pub max_seconds: u64,
}

impl Estimate {
    pub fn new(min_seconds: u64, max_seconds: u64) -> Self {
        Self { min_seconds, max_seconds }
    }

    /// `ir:triage`'s complexity table. Level 9 and 10 have no range: work
    /// that big is split before it is estimated.
    pub fn from_complexity(level: u8) -> Option<Self> {
        const MIN: u64 = 60;
        const HOUR: u64 = 3600;
        Some(match level {
            1 | 2 => Self::new(15 * MIN, 45 * MIN),
            3 | 4 => Self::new(45 * MIN, 2 * HOUR),
            5 | 6 => Self::new(90 * MIN, 3 * HOUR),
            7 | 8 => Self::new(150 * MIN, 5 * HOUR),
            _ => return None,
        })
    }

    pub fn midpoint(self) -> u64 {
        (self.min_seconds + self.max_seconds) / 2
    }

    /// `45m-2h`, the spelling a label and a card use.
    pub fn describe(self) -> String {
        format!("{}-{}", short_duration(self.min_seconds), short_duration(self.max_seconds))
    }
}

fn short_duration(seconds: u64) -> String {
    let minutes = seconds / 60;
    if minutes < 60 {
        format!("{minutes}m")
    } else if minutes % 60 == 0 {
        format!("{}h", minutes / 60)
    } else {
        format!("{}h{}m", minutes / 60, minutes % 60)
    }
}

/// Where a ready item goes. `agent` absent means the scope's own agent, as
/// for any task; `workflow` names a workflow definition in that scope to
/// start instead of running the item as a task of its own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Routing {
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
}

/// What a triager -- a person or a triage run -- submits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Assessment {
    /// All seven, each once.
    pub axes: Vec<AxisCheck>,
    pub category: String,
    pub impact: Level,
    pub urgency: Level,
    /// 1-10, `ir:triage`'s scale: expected touch points, plus one level per
    /// material design uncertainty.
    pub complexity: u8,
    /// A range of the triager's own, replacing the complexity table's --
    /// only with a named driver, which belongs in `summary`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate: Option<Estimate>,
    pub routing: Routing,
    /// A sentence or two: what the item is, and why the call.
    #[serde(default)]
    pub summary: String,
    /// The questions to put to the requester if this is `needs-info`. When
    /// empty, the failed axes' evidence is what gets asked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<String>,
}

/// What the rules make of an assessment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Verdict {
    Ready,
    NeedsInfo {
        /// One line per reason, in axis order, the complexity rule last.
        blockers: Vec<String>,
    },
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::NeedsInfo { .. } => "needs_info",
        }
    }
}

/// An assessment as recorded: what was submitted, what the rules made of it,
/// and who submitted it when.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Triage {
    pub assessment: Assessment,
    pub priority: Priority,
    /// The assessment's own range, or the complexity table's; absent for
    /// complexity 9-10 without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate: Option<Estimate>,
    pub verdict: Verdict,
    pub by: String,
    pub at: DateTime<Utc>,
}

/// Refuse an assessment that is not one: every axis once with evidence, a
/// category slug, a complexity on the scale, a cost on a failed
/// Observability, a sensible range and somewhere to route it.
pub fn validate(a: &Assessment) -> Result<(), String> {
    for axis in Axis::ALL {
        let checks: Vec<&AxisCheck> = a.axes.iter().filter(|c| c.axis == axis).collect();
        match checks.as_slice() {
            [] => return Err(format!("the {} axis is not assessed; all seven are needed", axis.as_str())),
            [check] => {
                if check.evidence.trim().is_empty() {
                    return Err(format!("the {} axis needs one sentence of evidence", axis.as_str()));
                }
                if axis == Axis::Observability && !check.pass && check.cost.is_none() {
                    return Err(
                        "a failed observability axis needs its cost: low, medium or high".into(),
                    );
                }
            }
            _ => return Err(format!("the {} axis is assessed more than once", axis.as_str())),
        }
    }
    let category = a.category.trim();
    if category.is_empty()
        || !category.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!(
            "category {:?} is not a slug (lowercase letters, digits and dashes); the usual ones are {}",
            a.category,
            CATEGORIES.join(", ")
        ));
    }
    if !(1..=10).contains(&a.complexity) {
        return Err(format!("complexity {} is off the 1-10 scale", a.complexity));
    }
    if let Some(e) = a.estimate {
        if e.min_seconds == 0 || e.min_seconds > e.max_seconds {
            return Err("an estimate needs 0 < min <= max".into());
        }
    }
    if a.routing.scope.trim().is_empty() {
        return Err("an assessment has to route the item to a scope".into());
    }
    Ok(())
}

/// `ir:triage`'s rules. Any failed axis gives `needs-info`, except
/// Observability at low or medium cost: that gap is closed by the task
/// itself, instrument first. Complexity 9-10 is a subsystem and is split
/// before it is taken on. Everything else is ready -- a bounded, reversible
/// item does not wait on an implementation choice the work can make.
pub fn evaluate(a: &Assessment, by: impl Into<String>, at: DateTime<Utc>) -> Triage {
    let mut blockers = Vec::new();
    for axis in Axis::ALL {
        let Some(check) = a.axes.iter().find(|c| c.axis == axis) else { continue };
        if check.pass {
            continue;
        }
        let tolerated = axis == Axis::Observability
            && matches!(check.cost, Some(ObservabilityCost::Low | ObservabilityCost::Medium));
        if !tolerated {
            blockers.push(format!("{}: {}", axis.label(), check.evidence.trim()));
        }
    }
    if a.complexity >= 9 {
        blockers.push(format!(
            "Complexity {}: a subsystem or multi-phase change -- split it or bound it to one phase",
            a.complexity
        ));
    }
    let verdict = if blockers.is_empty() { Verdict::Ready } else { Verdict::NeedsInfo { blockers } };
    Triage {
        assessment: a.clone(),
        priority: priority(a.impact, a.urgency),
        estimate: a.estimate.or_else(|| Estimate::from_complexity(a.complexity)),
        verdict,
        by: by.into(),
        at,
    }
}

// ---------------------------------------------------------------- decision

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WontfixReason {
    Duplicate,
    Invalid,
    OutOfScope,
}

impl WontfixReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Duplicate => "duplicate",
            Self::Invalid => "invalid",
            Self::OutOfScope => "out_of_scope",
        }
    }
}

/// What a triager does with an item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    /// Release it. Needs an assessment whose verdict is ready -- the gate is
    /// the point, so there is no releasing past it.
    Ready {
        /// Dispatch the released task at once, rather than leave it pending.
        #[serde(default)]
        run: bool,
    },
    /// Send it back. Empty questions fall back to the assessment's own, then
    /// to the failed axes.
    NeedsInfo {
        #[serde(default)]
        questions: Vec<String>,
    },
    /// Close it -- only for a verified duplicate, an invalid report or
    /// something that is not ours to do, and never without the evidence.
    Wontfix {
        reason: WontfixReason,
        evidence: String,
        /// For a duplicate: what it duplicates (a task id, an issue URL).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duplicate_of: Option<String>,
    },
}

impl Decision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ready { .. } => "ready",
            Self::NeedsInfo { .. } => "needs_info",
            Self::Wontfix { .. } => "wontfix",
        }
    }
}

/// How an item left intake, or was last sent back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub decision: Decision,
    pub by: String,
    pub at: DateTime<Utc>,
    /// The workflow run a ready item was released into, when it was routed
    /// to one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run: Option<String>,
}

/// Check a decision against the item before anything is written. Returns
/// the questions a `needs-info` will ask, empty for the other two.
pub fn check_decision(intake: &Intake, decision: &Decision) -> Result<Vec<String>, String> {
    if !intake.stage.is_open() {
        return Err(format!(
            "this item already left intake ({}); a decision is made once",
            intake.stage.as_str()
        ));
    }
    match decision {
        Decision::Ready { .. } => match &intake.triage {
            None => Err("nothing to release on: assess the item first".into()),
            Some(Triage { verdict: Verdict::NeedsInfo { blockers }, .. }) => Err(format!(
                "the assessment says needs-info, so it cannot be released: {}",
                blockers.join("; ")
            )),
            Some(Triage { verdict: Verdict::Ready, .. }) => Ok(Vec::new()),
        },
        Decision::NeedsInfo { questions } => {
            let asked: Vec<String> =
                questions.iter().map(|q| q.trim().to_string()).filter(|q| !q.is_empty()).collect();
            if !asked.is_empty() {
                return Ok(asked);
            }
            let from_triage = intake.triage.as_ref().map(|t| {
                if !t.assessment.questions.is_empty() {
                    t.assessment.questions.clone()
                } else if let Verdict::NeedsInfo { blockers } = &t.verdict {
                    blockers.clone()
                } else {
                    Vec::new()
                }
            });
            match from_triage {
                Some(q) if !q.is_empty() => Ok(q),
                _ => Err("say what is missing: give at least one question for the requester".into()),
            }
        }
        Decision::Wontfix { reason, evidence, duplicate_of } => {
            if evidence.trim().is_empty() {
                return Err("wontfix needs the evidence that verifies it -- no silent rejection".into());
            }
            if *reason == WontfixReason::Duplicate
                && duplicate_of.as_deref().map(str::trim).unwrap_or("").is_empty()
            {
                return Err("a duplicate is named, not guessed: say what it duplicates".into());
            }
            Ok(Vec::new())
        }
    }
}

/// The labels a released task carries -- the keys `#118`'s control plan and
/// `#117`'s reference classes read until they have fields of their own.
pub fn release_labels(triage: &Triage) -> BTreeMap<String, String> {
    let mut labels = BTreeMap::new();
    labels.insert("triage".into(), "ready".into());
    labels.insert("category".into(), triage.assessment.category.trim().to_string());
    labels.insert("priority".into(), triage.priority.as_str().into());
    if let Some(e) = triage.estimate {
        labels.insert("estimate".into(), e.describe());
    }
    if let Some(w) = &triage.assessment.routing.workflow {
        labels.insert("workflow".into(), w.clone());
    }
    labels
}

// ------------------------------------------------------------------- board

/// One item as the Intake view draws it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntakeCard {
    pub id: String,
    pub title: String,
    pub scope: String,
    pub status: TaskStatus,
    pub stage: IntakeStage,
    pub source: IntakeSource,
    pub requester: String,
    pub received_at: DateTime<Utc>,
    /// Since receipt, for the open columns; receipt to release for ready.
    pub age_seconds: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triage: Option<Triage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triage_task: Option<String>,
    /// That triage task's status, so a card can say it is still going -- or
    /// that it ended without an assessment and the item is back in received.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triage_task_status: Option<TaskStatus>,
    /// That triage task has ended and nothing is running it: closed, or
    /// blocked by a failure (`Task::is_settled`). The status alone cannot
    /// say so since `#122` -- a failed triage run leaves its task
    /// `blocked`, which is also what one waiting on a question is.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub triage_task_ended: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<DecisionRecord>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IntakeColumns {
    pub received: Vec<IntakeCard>,
    pub triaging: Vec<IntakeCard>,
    pub needs_info: Vec<IntakeCard>,
    pub ready: Vec<IntakeCard>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntakeBoard {
    pub now: DateTime<Utc>,
    /// How far back the ready column reaches.
    pub ready_window_days: i64,
    pub columns: IntakeColumns,
    /// Closed as wontfix in the same window -- counted, not drawn.
    pub wontfix: usize,
    /// The seven axes with their pass conditions, so a form and a legend
    /// never keep a copy of their own.
    pub axes: Vec<AxisInfo>,
    pub categories: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxisInfo {
    pub axis: Axis,
    pub label: String,
    pub pass_condition: String,
}

pub const READY_WINDOW_DAYS: i64 = 14;

/// The board over every task that went through intake. Open items sort
/// oldest first -- the one waiting longest is the one to look at -- and
/// ready ones newest first. A `triaging` item whose triage task ended with
/// no assessment is back in `received`: nothing is working on it any more.
pub fn board(tasks: &[Task], now: DateTime<Utc>) -> IntakeBoard {
    let by_id: BTreeMap<&str, &Task> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();
    let since = now - chrono::Duration::days(READY_WINDOW_DAYS);
    let mut columns = IntakeColumns::default();
    let mut wontfix = 0;
    for task in tasks {
        let Some(intake) = &task.intake else { continue };
        let triage = intake.triage_task.as_deref().and_then(|id| by_id.get(id).copied());
        let triage_task_status = triage.map(|t| t.status);
        let triage_task_ended = triage.is_some_and(Task::is_settled);
        let decided_at = intake.decision.as_ref().map(|d| d.at);
        let card = IntakeCard {
            id: task.id.clone(),
            title: task.title.clone(),
            scope: task.scope.clone(),
            status: task.status,
            stage: intake.stage,
            source: intake.source.clone(),
            requester: intake.requester.clone(),
            received_at: intake.received_at,
            age_seconds: (decided_at.filter(|_| !intake.stage.is_open()).unwrap_or(now) - intake.received_at)
                .num_seconds()
                .max(0),
            triage: intake.triage.clone(),
            questions: intake.questions.clone(),
            triage_task: intake.triage_task.clone(),
            triage_task_status,
            triage_task_ended,
            decision: intake.decision.clone(),
        };
        match intake.stage {
            IntakeStage::Received => columns.received.push(card),
            IntakeStage::Triaging => {
                let stalled = intake.triage.is_none() && triage_task_ended;
                if stalled { columns.received.push(card) } else { columns.triaging.push(card) }
            }
            IntakeStage::NeedsInfo => columns.needs_info.push(card),
            IntakeStage::Ready => {
                if decided_at.is_some_and(|at| at >= since) {
                    columns.ready.push(card);
                }
            }
            IntakeStage::Wontfix => {
                if decided_at.is_some_and(|at| at >= since) {
                    wontfix += 1;
                }
            }
        }
    }
    for column in [&mut columns.received, &mut columns.triaging, &mut columns.needs_info] {
        column.sort_by(|a, b| a.received_at.cmp(&b.received_at).then(a.id.cmp(&b.id)));
    }
    columns.ready.sort_by(|a, b| {
        let at = |c: &IntakeCard| c.decision.as_ref().map(|d| d.at);
        at(b).cmp(&at(a)).then(a.id.cmp(&b.id))
    });
    IntakeBoard {
        now,
        ready_window_days: READY_WINDOW_DAYS,
        columns,
        wontfix,
        axes: Axis::ALL
            .into_iter()
            .map(|axis| AxisInfo {
                axis,
                label: axis.label().into(),
                pass_condition: axis.pass_condition().into(),
            })
            .collect(),
        categories: CATEGORIES.iter().map(|c| c.to_string()).collect(),
    }
}

// ------------------------------------------------------------- triage node

/// The instructions of a triage run: `ir:triage` generalised from one
/// GitHub repository to any item in any scope. The run reads, assesses and
/// submits; it changes no file and posts nothing anywhere.
pub fn triage_instructions(item: &Task, scopes: &[String], bin: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Triage intake item {id} against the definition of ready. Do not do the work itself, \
         change no file, and post nothing outside Factory -- no comment, label or reply \
         anywhere. Your whole output is one assessment submitted to Factory.\n\n\
         The item, as it was handed in (scope {scope}):\n\n# {title}\n\n{body}\n\n",
        id = item.id,
        scope = item.scope,
        title = item.title,
        body = if item.instructions.trim().is_empty() { "(no description)" } else { item.instructions.trim() },
    ));
    out.push_str(
        "## How to triage\n\n\
         1. Read the item. Inspect the code, documents and prior work it touches, read-only; \
         `factory knowledge search` and `factory task list` find related work.\n\
         2. Treat a dismissal (\"already fixed\", \"not relevant\") as an assumption until \
         something you can cite verifies it.\n\
         3. Score each readiness axis pass or fail with one evidence-based sentence:\n",
    );
    for axis in Axis::ALL {
        out.push_str(&format!("   - {} -- {}\n", axis.as_str(), axis.pass_condition()));
    }
    out.push_str(
        "   A failed observability axis carries its cost: low (run an existing tool), medium \
         (extend an existing event, metric or fixture) or high (build a new observation \
         system).\n\
         4. Rules: any failed axis gives needs-info, except observability at low or medium \
         cost. Complexity 9-10 gives needs-info (split it). Otherwise it is ready -- do not \
         block on a reversible implementation choice.\n\
         5. Category: one slug -- ",
    );
    out.push_str(&CATEGORIES.join(", "));
    out.push_str(
        " -- or another if none fits.\n\
         6. Impact and urgency, each high, medium or low; the priority (P1-P4) follows from \
         the two.\n\
         7. Complexity 1-10 from the expected touch points, one level more per material \
         uncertainty: 1-2 one file, 3-4 one function or two files, 5-6 one slice across two to \
         four files, 7-8 cross-cutting, 9-10 a subsystem. It sets the estimate range (1-2: \
         15-45m, 3-4: 45m-2h, 5-6: 1.5-3h, 7-8: 2.5-5h). Give your own estimate only for a \
         named driver, and name it in the summary.\n\
         8. Route it to the scope that owns it -- one of: ",
    );
    out.push_str(&scopes.join(", "));
    out.push_str(
        " -- and, if one clearly fits, an agent there. Name a workflow only if the scope \
         already has one for exactly this.\n\n\
         ## Submit\n\n\
         Write the assessment as JSON to a file and submit it:\n\n",
    );
    out.push_str(&format!(
        "    {bin} intake assess {id} --file assessment.json --decide\n\n\
         `--decide` applies what the rules give: ready releases the item into its scope, \
         needs-info sends your questions back to the requester. The file's shape:\n\n",
        id = item.id,
    ));
    out.push_str(
        r#"    {
      "axes": [
        {"axis": "scope", "pass": true, "evidence": "..."},
        {"axis": "specification", "pass": true, "evidence": "..."},
        {"axis": "verifiability", "pass": true, "evidence": "..."},
        {"axis": "observability", "pass": false, "evidence": "...", "cost": "low"},
        {"axis": "context", "pass": true, "evidence": "..."},
        {"axis": "independence", "pass": true, "evidence": "..."},
        {"axis": "reversibility", "pass": true, "evidence": "..."}
      ],
      "category": "bugfix",
      "impact": "medium",
      "urgency": "high",
      "complexity": 4,
      "routing": {"scope": "<scope>", "agent": "<optional>"},
      "summary": "One or two sentences: what it is, and why this call.",
      "questions": ["Only for needs-info: one concrete question per gap."]
    }
"#,
    );
    out.push_str(
        "\nUse wontfix only for a verified duplicate, an invalid report or something out of \
         scope -- and then do not decide it yourself: say so in your task report with the \
         evidence, and a person closes it.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_pass() -> Vec<AxisCheck> {
        Axis::ALL
            .into_iter()
            .map(|axis| AxisCheck { axis, pass: true, evidence: format!("{} holds", axis.as_str()), cost: None })
            .collect()
    }

    fn assessment() -> Assessment {
        Assessment {
            axes: all_pass(),
            category: "bugfix".into(),
            impact: Level::Medium,
            urgency: Level::High,
            complexity: 4,
            estimate: None,
            routing: Routing { scope: "demo".into(), ..Default::default() },
            summary: "A bounded fix.".into(),
            questions: vec![],
        }
    }

    fn failing(axis: Axis, cost: Option<ObservabilityCost>) -> Assessment {
        let mut a = assessment();
        let check = a.axes.iter_mut().find(|c| c.axis == axis).unwrap();
        check.pass = false;
        check.evidence = format!("{} is open", axis.as_str());
        check.cost = cost;
        a
    }

    fn at() -> DateTime<Utc> {
        "2026-09-25T10:00:00Z".parse().unwrap()
    }

    #[test]
    fn the_priority_matrix_is_impact_times_urgency() {
        use Level::*;
        let grid = [
            (High, High, Priority::P1),
            (High, Medium, Priority::P2),
            (Medium, High, Priority::P2),
            (High, Low, Priority::P3),
            (Medium, Medium, Priority::P3),
            (Low, High, Priority::P3),
            (Medium, Low, Priority::P4),
            (Low, Medium, Priority::P4),
            (Low, Low, Priority::P4),
        ];
        for (impact, urgency, want) in grid {
            assert_eq!(priority(impact, urgency), want, "{impact:?} x {urgency:?}");
        }
    }

    #[test]
    fn complexity_maps_to_ir_triages_ranges_and_nine_has_none() {
        assert_eq!(Estimate::from_complexity(1).unwrap().describe(), "15m-45m");
        assert_eq!(Estimate::from_complexity(4).unwrap().describe(), "45m-2h");
        assert_eq!(Estimate::from_complexity(6).unwrap().describe(), "1h30m-3h");
        assert_eq!(Estimate::from_complexity(8).unwrap().describe(), "2h30m-5h");
        assert_eq!(Estimate::from_complexity(9), None);
        assert_eq!(Estimate::from_complexity(4).unwrap().midpoint(), (45 * 60 + 2 * 3600) / 2);
    }

    #[test]
    fn all_seven_passing_is_ready_with_priority_and_estimate() {
        let t = evaluate(&assessment(), "the owner", at());
        assert_eq!(t.verdict, Verdict::Ready);
        assert_eq!(t.priority, Priority::P2);
        assert_eq!(t.estimate, Estimate::from_complexity(4));
    }

    #[test]
    fn any_failed_axis_is_needs_info_naming_it() {
        for axis in Axis::ALL.into_iter().filter(|a| *a != Axis::Observability) {
            let t = evaluate(&failing(axis, None), "x", at());
            let Verdict::NeedsInfo { blockers } = t.verdict else { panic!("{axis:?} should block") };
            assert_eq!(blockers, vec![format!("{}: {} is open", axis.label(), axis.as_str())]);
        }
    }

    #[test]
    fn observability_at_low_or_medium_cost_is_tolerated_and_high_is_not() {
        for cost in [ObservabilityCost::Low, ObservabilityCost::Medium] {
            let t = evaluate(&failing(Axis::Observability, Some(cost)), "x", at());
            assert_eq!(t.verdict, Verdict::Ready, "{cost:?}");
        }
        let t = evaluate(&failing(Axis::Observability, Some(ObservabilityCost::High)), "x", at());
        assert!(matches!(t.verdict, Verdict::NeedsInfo { .. }));
    }

    #[test]
    fn complexity_nine_or_ten_is_needs_info_even_when_every_axis_passes() {
        let mut a = assessment();
        a.complexity = 9;
        let t = evaluate(&a, "x", at());
        let Verdict::NeedsInfo { blockers } = t.verdict else { panic!("should block") };
        assert!(blockers[0].starts_with("Complexity 9"), "{blockers:?}");
        assert_eq!(t.estimate, None);
    }

    #[test]
    fn an_own_estimate_replaces_the_tables() {
        let mut a = assessment();
        a.estimate = Some(Estimate::new(600, 1200));
        assert_eq!(evaluate(&a, "x", at()).estimate, Some(Estimate::new(600, 1200)));
    }

    #[test]
    fn validation_refuses_a_missing_duplicated_or_unevidenced_axis() {
        let mut a = assessment();
        a.axes.pop();
        assert!(validate(&a).unwrap_err().contains("reversibility axis is not assessed"));

        let mut a = assessment();
        a.axes.push(a.axes[0].clone());
        assert!(validate(&a).unwrap_err().contains("more than once"));

        let mut a = assessment();
        a.axes[2].evidence = "  ".into();
        assert!(validate(&a).unwrap_err().contains("evidence"));

        let a = failing(Axis::Observability, None);
        assert!(validate(&a).unwrap_err().contains("cost"));
    }

    #[test]
    fn validation_refuses_a_bad_category_complexity_estimate_or_route() {
        let mut a = assessment();
        a.category = "Bug Fix".into();
        assert!(validate(&a).unwrap_err().contains("not a slug"));
        let mut a = assessment();
        a.complexity = 0;
        assert!(validate(&a).is_err());
        let mut a = assessment();
        a.estimate = Some(Estimate::new(900, 600));
        assert!(validate(&a).is_err());
        let mut a = assessment();
        a.routing.scope = " ".into();
        assert!(validate(&a).is_err());
        assert!(validate(&assessment()).is_ok());
        let mut a = assessment();
        a.category = "marketing-request".into();
        assert!(validate(&a).is_ok(), "a category off the usual list is still a category");
    }

    fn open(triage: Option<Triage>) -> Intake {
        Intake {
            stage: IntakeStage::Triaging,
            source: IntakeSource { kind: SourceKind::Cli, reference: None },
            requester: "the owner".into(),
            received_at: at(),
            triage,
            triage_task: None,
            questions: vec![],
            decision: None,
        }
    }

    #[test]
    fn ready_is_refused_without_an_assessment_or_against_a_needs_info_verdict() {
        let ready = Decision::Ready { run: false };
        assert!(check_decision(&open(None), &ready).unwrap_err().contains("assess"));
        let blocked = evaluate(&failing(Axis::Scope, None), "x", at());
        let err = check_decision(&open(Some(blocked)), &ready).unwrap_err();
        assert!(err.contains("needs-info") && err.contains("Scope: scope is open"), "{err}");
        let fine = evaluate(&assessment(), "x", at());
        assert!(check_decision(&open(Some(fine)), &ready).is_ok());
    }

    #[test]
    fn needs_info_asks_the_given_questions_then_the_assessments_then_the_blockers() {
        let blocked = evaluate(&failing(Axis::Verifiability, None), "x", at());
        let item = open(Some(blocked.clone()));
        let given = Decision::NeedsInfo { questions: vec!["What does done look like?".into()] };
        assert_eq!(check_decision(&item, &given).unwrap(), vec!["What does done look like?"]);
        let none = Decision::NeedsInfo { questions: vec![] };
        assert_eq!(check_decision(&item, &none).unwrap(), vec!["Verifiability: verifiability is open"]);

        let mut asked = blocked;
        asked.assessment.questions = vec!["Which endpoint?".into()];
        assert_eq!(check_decision(&open(Some(asked)), &none).unwrap(), vec!["Which endpoint?"]);

        assert!(check_decision(&open(None), &none).is_err(), "nothing to ask is refused");
    }

    #[test]
    fn wontfix_needs_evidence_and_a_duplicate_names_what_it_duplicates() {
        let item = open(None);
        let bare = Decision::Wontfix { reason: WontfixReason::Invalid, evidence: " ".into(), duplicate_of: None };
        assert!(check_decision(&item, &bare).unwrap_err().contains("evidence"));
        let unnamed =
            Decision::Wontfix { reason: WontfixReason::Duplicate, evidence: "same bug".into(), duplicate_of: None };
        assert!(check_decision(&item, &unnamed).unwrap_err().contains("named"));
        let named = Decision::Wontfix {
            reason: WontfixReason::Duplicate,
            evidence: "same stack trace".into(),
            duplicate_of: Some("t-42".into()),
        };
        assert!(check_decision(&item, &named).is_ok());
    }

    #[test]
    fn a_decided_item_takes_no_second_decision() {
        let mut item = open(Some(evaluate(&assessment(), "x", at())));
        item.stage = IntakeStage::Ready;
        assert!(check_decision(&item, &Decision::Ready { run: false }).unwrap_err().contains("already left"));
    }

    #[test]
    fn release_labels_carry_category_priority_estimate_and_the_triage_mark() {
        let mut a = assessment();
        a.routing.workflow = Some("release-train".into());
        let labels = release_labels(&evaluate(&a, "x", at()));
        assert_eq!(labels["triage"], "ready");
        assert_eq!(labels["category"], "bugfix");
        assert_eq!(labels["priority"], "P2");
        assert_eq!(labels["estimate"], "45m-2h");
        assert_eq!(labels["workflow"], "release-train");
    }

    fn task(id: &str, status: TaskStatus, intake: Option<Intake>) -> Task {
        let mut t = crate::adapter::store::task_from_new(
            crate::task::NewTask { title: id.into(), ..Default::default() },
            "demo".into(),
            "shell".into(),
            "herdr".into(),
        );
        t.id = id.into();
        t.status = status;
        t.intake = intake;
        t
    }

    #[test]
    fn the_board_sorts_items_into_the_four_columns_and_counts_wontfix() {
        let now = at();
        let mut received = open(None);
        received.stage = IntakeStage::Received;
        received.received_at = now - chrono::Duration::hours(3);
        let mut older = received.clone();
        older.received_at = now - chrono::Duration::hours(5);
        let mut needs = open(None);
        needs.stage = IntakeStage::NeedsInfo;
        needs.questions = vec!["which?".into()];
        let mut ready = open(Some(evaluate(&assessment(), "x", now)));
        ready.stage = IntakeStage::Ready;
        ready.received_at = now - chrono::Duration::hours(10);
        ready.decision = Some(DecisionRecord {
            decision: Decision::Ready { run: false },
            by: "x".into(),
            at: now - chrono::Duration::hours(1),
            workflow_run: None,
        });
        let mut stale_ready = ready.clone();
        stale_ready.decision.as_mut().unwrap().at = now - chrono::Duration::days(30);
        let mut closed = open(None);
        closed.stage = IntakeStage::Wontfix;
        closed.decision = Some(DecisionRecord {
            decision: Decision::Wontfix { reason: WontfixReason::Invalid, evidence: "e".into(), duplicate_of: None },
            by: "x".into(),
            at: now,
            workflow_run: None,
        });
        let tasks = vec![
            task("r1", TaskStatus::Intake, Some(received)),
            task("r0", TaskStatus::Intake, Some(older)),
            task("tr", TaskStatus::Intake, Some(open(None))),
            task("ni", TaskStatus::Intake, Some(needs)),
            task("rd", TaskStatus::Pending, Some(ready)),
            task("old", TaskStatus::Done, Some(stale_ready)),
            task("wf", TaskStatus::Cancelled, Some(closed)),
            task("plain", TaskStatus::Pending, None),
        ];
        let b = board(&tasks, now);
        let ids = |c: &[IntakeCard]| c.iter().map(|c| c.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&b.columns.received), vec!["r0", "r1"], "oldest first");
        assert_eq!(ids(&b.columns.triaging), vec!["tr"]);
        assert_eq!(ids(&b.columns.needs_info), vec!["ni"]);
        assert_eq!(ids(&b.columns.ready), vec!["rd"], "only inside the window");
        assert_eq!(b.wontfix, 1);
        assert_eq!(b.columns.received[0].age_seconds, 5 * 3600);
        assert_eq!(b.columns.ready[0].age_seconds, 9 * 3600, "receipt to release");
        assert_eq!(b.axes.len(), 7);
    }

    #[test]
    fn a_triage_run_that_ended_without_an_assessment_puts_the_item_back_in_received() {
        let mut item = open(None);
        item.triage_task = Some("triage-run".into());
        // A failed triage run leaves its task blocked on the failure
        // (`#122`), not closed -- and that still means nothing is triaging.
        let mut failed = task("triage-run", TaskStatus::Blocked, None);
        failed.failure = Some(crate::task::TaskFailure {
            kind: Some(crate::run::FailKind::AgentFailed),
            run_id: Some("r1".into()),
            attempt: Some(1),
            at: at(),
        });
        let tasks = vec![task("item", TaskStatus::Intake, Some(item.clone())), failed];
        let b = board(&tasks, at());
        assert_eq!(b.columns.received.len(), 1);
        assert_eq!(b.columns.received[0].triage_task_status, Some(TaskStatus::Blocked));
        assert!(b.columns.received[0].triage_task_ended);
        assert!(b.columns.triaging.is_empty());

        // One blocked on a question is still working on it.
        let asking = task("triage-run", TaskStatus::Blocked, None);
        let b = board(&[task("item", TaskStatus::Intake, Some(item)), asking], at());
        assert_eq!(b.columns.triaging.len(), 1);
        assert!(!b.columns.triaging[0].triage_task_ended);
    }

    #[test]
    fn the_triage_instructions_carry_the_item_the_axes_the_rules_and_the_submit_command() {
        let item = task("item-1", TaskStatus::Intake, Some(open(None)));
        let text = triage_instructions(&item, &["demo".into(), "web".into()], "/bin/factory");
        for axis in Axis::ALL {
            assert!(text.contains(axis.pass_condition()), "{axis:?}");
        }
        assert!(text.contains("/bin/factory intake assess item-1 --file assessment.json --decide"));
        assert!(text.contains("demo, web"));
        assert!(text.contains("post nothing outside Factory"));
    }

    #[test]
    fn an_assessment_round_trips_in_the_shape_the_instructions_show() {
        let json = r#"{
            "axes": [
              {"axis": "scope", "pass": true, "evidence": "a"},
              {"axis": "specification", "pass": true, "evidence": "a"},
              {"axis": "verifiability", "pass": true, "evidence": "a"},
              {"axis": "observability", "pass": false, "evidence": "a", "cost": "low"},
              {"axis": "context", "pass": true, "evidence": "a"},
              {"axis": "independence", "pass": true, "evidence": "a"},
              {"axis": "reversibility", "pass": true, "evidence": "a"}
            ],
            "category": "bugfix", "impact": "medium", "urgency": "high", "complexity": 4,
            "routing": {"scope": "demo"}, "summary": "s"
        }"#;
        let a: Assessment = serde_json::from_str(json).unwrap();
        assert!(validate(&a).is_ok());
        assert_eq!(evaluate(&a, "x", at()).verdict, Verdict::Ready);
        let decision: Decision = serde_json::from_str(r#"{"decision":"needs_info","questions":["q"]}"#).unwrap();
        assert_eq!(decision, Decision::NeedsInfo { questions: vec!["q".into()] });
    }
}
