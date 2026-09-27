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
//! it: release it (`ready`), send it back (`needs-info`), split it into
//! smaller items (`split`), or close it (`wontfix`, only with a verified
//! reason). An item the rules hold back is never a dead end:
//! [`next_actions`] says what would move each blocker.
//!
//! Everything here is pure: no store, no clock but the one passed in. The
//! daemon (`factory-daemon/src/intake.rs`) owns the transitions.
//!
//! Duplicates (`#166`) are found the same way: [`duplicate_candidates`] is a
//! pure search over the open tasks the daemon hands it, run at receipt and
//! again at triage; [`DuplicateCandidate`] is what it finds, what a triager
//! answers, and what a triager adds itself from a vault search. A confirmed
//! one blocks the verdict in [`evaluate`], same as a failed axis.
//!
//! What this slice leaves out, on purpose: auto-closing a duplicate, any
//! embedding or LLM search, closed tasks or GitHub search, the per-child
//! check (`#180`), the security fast lane with its CRA clock, the `triager`
//! role, and every outbound effect. An assessment is never posted anywhere;
//! it is only journaled. The four intake registry metrics (`#165`,
//! [`registry_metric`]) are the one exception: they read the decision
//! events this module already defines, but still no store --
//! `factory-daemon` builds the facts from the task journal and hands them
//! over.

use crate::operations::Window;
use crate::task::{Task, TaskStatus};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ------------------------------------------------------------------ receipt

/// How far an intake item has got. `Ready`, `Split` and `Wontfix` are where
/// it leaves intake; the rest keep the task in `TaskStatus::Intake`.
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
    /// Replaced by smaller items, each handed back into intake to be
    /// triaged on its own.
    Split,
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
            Self::Split => "split",
            Self::Wontfix => "wontfix",
        }
    }

    /// Still inside the gate: the task is `TaskStatus::Intake`.
    pub fn is_open(self) -> bool {
        matches!(self, Self::Received | Self::Triaging | Self::NeedsInfo)
    }
}

/// Which way an item came in.
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
    /// An open GitHub issue carrying `needs-triage`. Set only by the daemon's
    /// read-only poller, never accepted as caller-supplied provenance.
    Github,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Ui => "ui",
            Self::Agent => "agent",
            Self::Github => "github",
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
    /// `cli` or `ui`. An agent's request is always `agent`, whatever it says,
    /// and trusted external-source kinds are never accepted from callers.
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
    /// Possible duplicates the daemon found -- at receipt, and again when
    /// triage starts (`#166`). What a triager made of them travels with the
    /// assessment instead, in `Triage::assessment::duplicates`; see
    /// [`candidates_with_verdicts`] for the two put together.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<DuplicateCandidate>,
}

// --------------------------------------------------------------- duplicates

/// Where a possible duplicate lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateKind {
    /// An ordinary open task -- released, or never gated by intake at all.
    Task,
    /// Another item still held in intake (`TaskStatus::Intake`).
    IntakeItem,
    /// A page in the knowledge vault, named by a triager's own `factory
    /// knowledge search` -- intake never reads the vault itself, so this is
    /// always given, never found.
    Knowledge,
}

impl DuplicateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::IntakeItem => "intake_item",
            Self::Knowledge => "knowledge",
        }
    }
}

/// How a candidate was found: the same source reference -- the strongest
/// evidence, the same GitHub issue or mail id -- or a text match, a
/// normalised token overlap of title and instructions above
/// [`TEXT_MATCH_THRESHOLD`], carried as a percentage in `score`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateMatch {
    Source,
    Text,
}

/// Where a triager's answer to a candidate stands. A candidate the daemon
/// just found, or a triager has not yet answered, is `unverified`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateVerdict {
    #[default]
    Unverified,
    Confirmed,
    Rejected,
}

impl DuplicateVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unverified => "unverified",
            Self::Confirmed => "confirmed",
            Self::Rejected => "rejected",
        }
    }
}

/// One possible duplicate: what [`duplicate_candidates`] found (on
/// [`Intake::candidates`]), or what a triager answered -- confirming or
/// rejecting one of those, or adding a `knowledge` one of its own found
/// with `factory knowledge search` (on `Assessment::duplicates`).
/// `reference` is a task id for `task` and `intake_item`, a vault page path
/// for `knowledge` -- free text, never followed by intake itself, same as
/// [`IntakeSource::reference`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuplicateCandidate {
    pub kind: DuplicateKind,
    pub reference: String,
    pub title: String,
    /// One sentence: why this is a candidate, or why a triager confirmed or
    /// rejected it.
    pub evidence: String,
    #[serde(rename = "match")]
    pub matched: DuplicateMatch,
    /// A percentage, only for a `text` match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<u8>,
    #[serde(default)]
    pub verdict: DuplicateVerdict,
}

/// Above this normalised token overlap of title and the start of the
/// instructions (as a percentage), two open items count as a text match.
/// Picked from the fixtures below: high enough that sharing a boilerplate
/// phrase never trips it alone, low enough that the same report in
/// different words does.
const TEXT_MATCH_THRESHOLD: u8 = 60;

/// A text match also needs at least this many tokens in common. A single
/// shared word can carry the whole percentage when both signatures are
/// short (a bare title, a one-line instruction); two is the least an
/// overlap can mean anything by.
const MIN_SHARED_TOKENS: usize = 2;

/// How many words of the instructions the signature looks at -- the
/// opening, where an issue or a request says what it is, not whatever a
/// template or a long paste adds after.
const INSTRUCTION_PREFIX_WORDS: usize = 40;

/// At most this many stored candidates. More is noise for a triager to wade
/// through, and a false negative is cheaper to catch by hand than five red
/// herrings are to dismiss one by one.
pub const MAX_CANDIDATES: usize = 5;

/// Function words common enough that sharing them alone proves nothing.
/// Short, on purpose: everything else in a title or a request's opening is
/// left to carry the signal.
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "are", "but", "not", "you", "all", "can", "her", "was", "one", "our",
    "out", "has", "him", "his", "how", "see", "two", "way", "did", "its", "let", "say", "she",
    "too", "use", "with", "this", "that", "from", "have", "will", "your", "into", "also", "been",
    "were", "when", "what", "then", "than", "only", "just",
];

/// A title's, or the start of a request's, words boiled down to what might
/// carry signal: lowercase, split on anything that is not a letter or a
/// digit, three characters or more, not a stopword.
fn normalised_tokens(text: &str) -> Vec<String> {
    text.to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() >= 3 && !STOPWORDS.contains(w))
        .map(str::to_string)
        .collect()
}

/// The set of tokens a title and the opening of its instructions boil down
/// to, for the text-match rule.
fn signature(title: &str, instructions: &str) -> std::collections::BTreeSet<String> {
    let prefix = instructions.split_whitespace().take(INSTRUCTION_PREFIX_WORDS).collect::<Vec<_>>().join(" ");
    let mut tokens = normalised_tokens(title);
    tokens.extend(normalised_tokens(&prefix));
    tokens.into_iter().collect()
}

/// Percentage overlap of two token sets, and how many they share. `(0, 0)`
/// when either is empty -- a title-less, instruction-less item never
/// "matches" everything by having nothing in common with it either.
fn overlap(a: &std::collections::BTreeSet<String>, b: &std::collections::BTreeSet<String>) -> (u8, usize) {
    if a.is_empty() || b.is_empty() {
        return (0, 0);
    }
    let shared = a.intersection(b).count();
    let union = a.union(b).count().max(1);
    (((shared * 100) / union) as u8, shared)
}

/// Duplicate candidates for `item` among `tasks`: those still open
/// (`!status.is_terminal()` -- not `Task::is_settled`, since a task blocked
/// by a failure is still open work a duplicate would pile up behind),
/// including other open intake items, excluding the item itself, triage
/// bookkeeping tasks (`TRIAGE_LABEL`), the item's own split parent and
/// parts (`PARENT_LABEL`), and its siblings -- a split's parts all carry
/// the whole original request in their own instructions (`intake_split`),
/// which would otherwise make every part look like a duplicate of every
/// other for a reason that has nothing to do with the work.
///
/// The same source reference is the strongest evidence; otherwise, a text
/// match at or above [`TEXT_MATCH_THRESHOLD`] with at least
/// [`MIN_SHARED_TOKENS`] in common. At most [`MAX_CANDIDATES`], ordered by
/// score then reference, so the same input always gives the same list.
pub fn duplicate_candidates(item: &Task, tasks: &[Task]) -> Vec<DuplicateCandidate> {
    let item_parent = item.labels.get(PARENT_LABEL).map(String::as_str);
    let item_source = item.intake.as_ref().map(|i| &i.source);
    let item_tokens = signature(&item.title, &item.instructions);

    let mut scored: Vec<(u16, DuplicateCandidate)> = Vec::new();
    for t in tasks {
        if t.id == item.id || t.status.is_terminal() || t.labels.contains_key(TRIAGE_LABEL) {
            continue;
        }
        if item_parent == Some(t.id.as_str()) {
            continue; // t is the item it was split from
        }
        let t_parent = t.labels.get(PARENT_LABEL).map(String::as_str);
        if t_parent == Some(item.id.as_str()) {
            continue; // t is one of the item's own parts
        }
        if item_parent.is_some() && item_parent == t_parent {
            continue; // t is a sibling part of the same split
        }

        let kind = if t.status == TaskStatus::Intake { DuplicateKind::IntakeItem } else { DuplicateKind::Task };

        if let (Some(a), Some(b)) = (item_source, t.intake.as_ref().map(|i| &i.source)) {
            let reference = a.reference.as_deref().map(str::trim).filter(|r| !r.is_empty());
            if a.kind == b.kind && reference.is_some() && reference == b.reference.as_deref().map(str::trim) {
                scored.push((
                    u16::MAX,
                    DuplicateCandidate {
                        kind,
                        reference: t.id.clone(),
                        title: t.title.clone(),
                        evidence: format!(
                            "same {} reference as {} ({}): {}",
                            a.kind.as_str(),
                            t.id,
                            t.title,
                            reference.unwrap_or_default()
                        ),
                        matched: DuplicateMatch::Source,
                        score: None,
                        verdict: DuplicateVerdict::Unverified,
                    },
                ));
                continue;
            }
        }

        let t_tokens = signature(&t.title, &t.instructions);
        let (score, shared) = overlap(&item_tokens, &t_tokens);
        if score >= TEXT_MATCH_THRESHOLD && shared >= MIN_SHARED_TOKENS {
            scored.push((
                score as u16,
                DuplicateCandidate {
                    kind,
                    reference: t.id.clone(),
                    title: t.title.clone(),
                    evidence: format!("{score}% overlap in title and instructions with {} ({})", t.id, t.title),
                    matched: DuplicateMatch::Text,
                    score: Some(score),
                    verdict: DuplicateVerdict::Unverified,
                },
            ));
        }
    }

    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.reference.cmp(&b.1.reference)));
    scored.truncate(MAX_CANDIDATES);
    scored.into_iter().map(|(_, c)| c).collect()
}

/// Refuse an assessment that leaves a stored candidate unanswered, or
/// answers one -- stored, or a triager's own `knowledge` find -- without
/// evidence. A stored candidate is matched to its answer by `(kind,
/// reference)`: a triager copies the daemon's own value back with its
/// verdict and evidence changed, so an answer that names no stored
/// candidate (a `knowledge` one, typically) is only checked for evidence
/// here.
pub fn validate_duplicates(stored: &[DuplicateCandidate], answered: &[DuplicateCandidate]) -> Result<(), String> {
    for d in answered {
        if d.evidence.trim().is_empty() {
            return Err(format!("the {} candidate {} needs evidence for its verdict", d.kind.as_str(), d.reference));
        }
    }
    for s in stored {
        let answer = answered.iter().find(|d| d.kind == s.kind && d.reference == s.reference);
        if !matches!(answer, Some(d) if d.verdict != DuplicateVerdict::Unverified) {
            return Err(format!(
                "possible duplicate {} ({}) needs a verdict: confirm or reject it",
                s.reference, s.title
            ));
        }
    }
    Ok(())
}

/// What a card, `intake show` and the item modal display: the daemon's
/// stored candidates overlaid with the triager's own answer by `(kind,
/// reference)` where there is one, plus any candidate -- a `knowledge` one,
/// typically -- the triager added that the search never found.
/// [`Intake::candidates`] itself stays "what the daemon found", unanswered.
pub fn candidates_with_verdicts(intake: &Intake) -> Vec<DuplicateCandidate> {
    let answered: &[DuplicateCandidate] =
        intake.triage.as_ref().map(|t| t.assessment.duplicates.as_slice()).unwrap_or(&[]);
    let mut out: Vec<DuplicateCandidate> = intake
        .candidates
        .iter()
        .map(|c| {
            answered.iter().find(|d| d.kind == c.kind && d.reference == c.reference).cloned().unwrap_or_else(|| c.clone())
        })
        .collect();
    for d in answered {
        if !intake.candidates.iter().any(|c| c.kind == d.kind && c.reference == d.reference) {
            out.push(d.clone());
        }
    }
    out
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
/// start instead of running the item as a task of its own, with `inputs`
/// for the run and `agents` choosing who does each step.
///
/// A step's model is chosen by choosing its agent: a task carries no model
/// of its own, and each harness spells the flag differently, so the model
/// lives in an agent's declared `args` (`--model opus`) and a route picks
/// between declared agents. [`RouteOptions`] lists them with their models.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Routing {
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    /// The workflow run's inputs, by the names it declares. Only with a
    /// workflow.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, String>,
    /// Task node id -> the agent that runs that step, replacing the one the
    /// definition names. Steps left out keep theirs. Only with a workflow.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<String, String>,
}

/// One smaller item an assessment proposes, or a person writes, when an item
/// is too big to be ready (`#180`'s plan, before its `expand` node exists).
/// A split makes each part an intake item of its own, so each is triaged --
/// and has to pass the seven axes -- on its own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitPart {
    /// Short and unique within the split: what `depends_on` names.
    pub id: String,
    pub title: String,
    /// The part's own slice of the work, standalone.
    #[serde(default)]
    pub instructions: String,
    /// The ids of parts that have to be done first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// How to tell the part is done -- a command, or a sentence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<String>,
}

/// At most this many parts: more is a plan, not a split, and each part is
/// still triaged by hand.
pub const MAX_SPLIT_PARTS: usize = 8;

/// Refuse a split that is not one: two to eight parts, each with a unique
/// slug id and a title, depending only on other parts, without a cycle.
pub fn validate_split(parts: &[SplitPart]) -> Result<(), String> {
    if parts.len() < 2 {
        return Err("a split needs at least two parts".into());
    }
    if parts.len() > MAX_SPLIT_PARTS {
        return Err(format!("a split has at most {MAX_SPLIT_PARTS} parts, not {}", parts.len()));
    }
    let mut ids = std::collections::BTreeSet::new();
    for p in parts {
        let id = p.id.trim();
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err(format!("part id {:?} is not a slug (lowercase letters, digits and dashes)", p.id));
        }
        if !ids.insert(id) {
            return Err(format!("part id {id:?} is used twice"));
        }
        if p.title.trim().is_empty() {
            return Err(format!("part {id} needs a title"));
        }
    }
    for p in parts {
        for d in &p.depends_on {
            if d.trim() == p.id.trim() {
                return Err(format!("part {} depends on itself", p.id));
            }
            if !ids.contains(d.trim()) {
                return Err(format!("part {} depends on {d:?}, which is not a part", p.id));
            }
        }
    }
    if split_order(parts).is_none() {
        return Err("the parts' dependencies go round in a circle".into());
    }
    Ok(())
}

/// The parts in an order where each comes after everything it depends on,
/// otherwise as written; `None` for a cycle.
pub fn split_order(parts: &[SplitPart]) -> Option<Vec<&SplitPart>> {
    let mut out: Vec<&SplitPart> = Vec::with_capacity(parts.len());
    let mut placed = std::collections::BTreeSet::new();
    while out.len() < parts.len() {
        let next = parts.iter().find(|p| {
            !placed.contains(p.id.trim()) && p.depends_on.iter().all(|d| placed.contains(d.trim()))
        })?;
        placed.insert(next.id.trim());
        out.push(next);
    }
    Some(out)
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
    /// A proposed split, for an item too big or too loose to be ready as one.
    /// Only proposed: the verdict is unchanged, and a person decides `split`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub split: Vec<SplitPart>,
    /// Every stored candidate (`Intake::candidates`) answered confirmed or
    /// rejected, plus any `knowledge` one the triager found itself
    /// (`#166`). [`validate_duplicates`] refuses one left unanswered, or
    /// answered without evidence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub duplicates: Vec<DuplicateCandidate>,
}

/// What the rules make of an assessment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Verdict {
    Ready,
    NeedsInfo {
        /// One line per reason: a confirmed duplicate first, then axis
        /// order, the complexity rule last.
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
    if a.routing.workflow.is_none() && !(a.routing.inputs.is_empty() && a.routing.agents.is_empty()) {
        return Err("workflow inputs and per-step agents need a workflow to route to".into());
    }
    if !a.split.is_empty() {
        validate_split(&a.split).map_err(|e| format!("the proposed split: {e}"))?;
    }
    Ok(())
}

/// `ir:triage`'s rules, plus one of its own (`#166`): a confirmed duplicate
/// always gives `needs-info`, so `--decide` can never release one. Any
/// failed axis gives `needs-info` too, except Observability at low or
/// medium cost: that gap is closed by the task itself, instrument first.
/// Complexity 9-10 is a subsystem and is split before it is taken on.
/// Everything else is ready -- a bounded, reversible item does not wait on
/// an implementation choice the work can make.
pub fn evaluate(a: &Assessment, by: impl Into<String>, at: DateTime<Utc>) -> Triage {
    let mut blockers = Vec::new();
    for d in a.duplicates.iter().filter(|d| d.verdict == DuplicateVerdict::Confirmed) {
        blockers.push(format!("Duplicate: confirmed duplicate of {} -- {}", d.reference, d.evidence.trim()));
    }
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
    /// Replace it with smaller items, each handed back into intake. Empty
    /// parts take the assessment's proposal. Needs no verdict: an item
    /// nobody could assess as one is exactly the one to split.
    Split {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        parts: Vec<SplitPart>,
    },
}

impl Decision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ready { .. } => "ready",
            Self::NeedsInfo { .. } => "needs_info",
            Self::Wontfix { .. } => "wontfix",
            Self::Split { .. } => "split",
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
    /// The intake items a split made, in the parts' order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<String>,
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
        Decision::Split { parts } => split_parts(intake, parts).map(|_| Vec::new()),
    }
}

/// The parts a `split` makes: the given ones, or else the assessment's
/// proposal -- checked either way.
pub fn split_parts(intake: &Intake, given: &[SplitPart]) -> Result<Vec<SplitPart>, String> {
    let parts: Vec<SplitPart> = if given.is_empty() {
        intake.triage.as_ref().map(|t| t.assessment.split.clone()).unwrap_or_default()
    } else {
        given.to_vec()
    };
    if parts.is_empty() {
        return Err("say how to split it: no parts were given and the assessment proposes none".into());
    }
    validate_split(&parts)?;
    Ok(parts
        .into_iter()
        .map(|p| SplitPart {
            id: p.id.trim().to_string(),
            title: p.title.trim().to_string(),
            instructions: p.instructions.trim().to_string(),
            depends_on: p.depends_on.iter().map(|d| d.trim().to_string()).collect(),
            acceptance: p.acceptance.map(|a| a.trim().to_string()).filter(|a| !a.is_empty()),
        })
        .collect())
}

// ------------------------------------------------------------ next actions

/// What would move a held-back item forward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NextActionKind {
    /// A candidate was confirmed: close it, naming what it duplicates.
    CloseDuplicate,
    /// Too big or unbounded: split it into items that pass on their own.
    Split,
    /// Missing a fact or a decision only the requester has: answer the
    /// questions, then triage it again.
    AddInfo,
}

impl NextActionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CloseDuplicate => "close_duplicate",
            Self::Split => "split",
            Self::AddInfo => "add_info",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NextAction {
    pub action: NextActionKind,
    /// The blockers this answers, one line each.
    pub reasons: Vec<String>,
    /// What to do, in a sentence.
    pub hint: String,
    /// `close_duplicate` only: the confirmed candidate's own reference, so
    /// a caller can name it in `--duplicate-of` rather than print a
    /// placeholder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

/// Every blocker of a needs-info verdict, turned into what would clear it:
/// a confirmed duplicate by closing it; a failed Scope, a high-cost
/// Observability gap and complexity 9-10 by splitting; the other axes by
/// information. A confirmed duplicate comes first -- closing makes
/// splitting or asking questions moot -- then split, since a part is
/// triaged again anyway and questions asked of the whole may not apply to
/// any part. Empty for an item the rules do not hold back.
pub fn next_actions(intake: &Intake) -> Vec<NextAction> {
    let Some(triage) = &intake.triage else { return Vec::new() };
    if !intake.stage.is_open() || triage.verdict == Verdict::Ready {
        return Vec::new();
    }
    let a = &triage.assessment;
    let mut out = Vec::new();
    let confirmed: Vec<&DuplicateCandidate> =
        a.duplicates.iter().filter(|d| d.verdict == DuplicateVerdict::Confirmed).collect();
    if !confirmed.is_empty() {
        out.push(NextAction {
            action: NextActionKind::CloseDuplicate,
            reasons: confirmed
                .iter()
                .map(|d| format!("Duplicate: confirmed duplicate of {} -- {}", d.reference, d.evidence.trim()))
                .collect(),
            hint: "Close it as the duplicate it was confirmed to be -- a person still decides, never \
                   the triage run."
                .into(),
            reference: Some(confirmed[0].reference.clone()),
        });
    }
    let mut split = Vec::new();
    let mut info = Vec::new();
    for check in a.axes.iter().filter(|c| !c.pass) {
        let line = format!("{}: {}", check.axis.label(), check.evidence.trim());
        match (check.axis, check.cost) {
            (Axis::Observability, Some(ObservabilityCost::Low | ObservabilityCost::Medium)) => {}
            (Axis::Scope, _) | (Axis::Observability, _) => split.push(line),
            _ => info.push(line),
        }
    }
    if a.complexity >= 9 {
        split.push(format!("Complexity {}: a subsystem or multi-phase change", a.complexity));
    }
    if !split.is_empty() {
        let hint = if a.split.is_empty() {
            "Split it into bounded items -- two to eight, each triaged on its own. Write the parts, \
             or triage it again for a proposal."
                .to_string()
        } else {
            format!(
                "Split it as the assessment proposes, into {} parts: {}. Each goes back into intake and is triaged on its own.",
                a.split.len(),
                a.split.iter().map(|p| p.title.as_str()).collect::<Vec<_>>().join("; ")
            )
        };
        out.push(NextAction { action: NextActionKind::Split, reasons: split, hint, reference: None });
    }
    if !info.is_empty() {
        out.push(NextAction {
            action: NextActionKind::AddInfo,
            reasons: info,
            hint: "Answer the questions with what is missing; the item goes back into the queue to be \
                   triaged again."
                .into(),
            reference: None,
        });
    }
    out
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
    /// What would move it forward, when the rules hold it back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_actions: Vec<NextAction>,
    /// The item it was split from, when it is a part of one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Possible duplicates, the daemon's own search overlaid with a
    /// triager's answer (`#166`, [`candidates_with_verdicts`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<DuplicateCandidate>,
}

/// The label a part carries: the id of the item it was split from.
pub const PARENT_LABEL: &str = "intake-parent";
/// The label a part carries: its id within the split.
pub const PART_LABEL: &str = "intake-part";
/// The label a triage task carries: the id of the item it triages. Defined
/// here, not only where it is set (`factory-daemon`'s `intake_triage`),
/// because [`duplicate_candidates`] has to exclude these bookkeeping tasks
/// from the search too.
pub const TRIAGE_LABEL: &str = "intake-triage";

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
    /// Split in the same window -- counted, not drawn; the parts are the
    /// cards.
    #[serde(default)]
    pub split: usize,
    /// The seven axes with their pass conditions, so a form and a legend
    /// never keep a copy of their own.
    pub axes: Vec<AxisInfo>,
    pub categories: Vec<String>,
    /// Where an item can be routed: every scope with its agents and
    /// workflows. Filled in by the daemon, which has them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<RouteOptions>,
}

/// A scope an item can be routed to, and what can run it there -- what a
/// triager chooses a workflow, its inputs and each step's agent from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteOptions {
    pub scope: String,
    /// The agent a task there runs on unless it says otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_agent: Option<String>,
    #[serde(default)]
    pub agents: Vec<AgentOption>,
    #[serde(default)]
    pub workflows: Vec<WorkflowOption>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentOption {
    pub name: String,
    pub harness: String,
    /// Read off the agent's declared args; absent is the harness's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowOption {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub inputs: Vec<crate::workflow::WorkflowInput>,
    /// Its task nodes, in the definition's order -- the steps an agent can
    /// be chosen for.
    #[serde(default)]
    pub steps: Vec<WorkflowStep>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowStep {
    pub id: String,
    pub title: String,
    /// The agent the definition names, if it names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

/// The model an agent's args choose: `--model X`, `--model=X` or `-m X`.
pub fn model_of(args: &[String]) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if let Some(v) = a.strip_prefix("--model=") {
            return Some(v.to_string());
        }
        if a == "--model" || a == "-m" {
            return it.next().cloned();
        }
    }
    None
}

impl WorkflowOption {
    pub fn from_definition(d: &crate::workflow::WorkflowDefinition) -> Self {
        Self {
            id: d.id.clone(),
            name: d.name.clone(),
            description: d.description.clone(),
            inputs: d.inputs.clone(),
            steps: d
                .nodes
                .iter()
                .filter(|n| n.kind == crate::workflow::WorkflowNodeKind::Task)
                .map(|n| WorkflowStep { id: n.id.clone(), title: n.task.title.clone(), agent: n.task.agent.clone() })
                .collect(),
        }
    }
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
    let mut split = 0;
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
            next_actions: next_actions(intake),
            parent: task.labels.get(PARENT_LABEL).cloned(),
            candidates: candidates_with_verdicts(intake),
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
            IntakeStage::Split => {
                if decided_at.is_some_and(|at| at >= since) {
                    split += 1;
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
        split,
        axes: Axis::ALL
            .into_iter()
            .map(|axis| AxisInfo {
                axis,
                label: axis.label().into(),
                pass_condition: axis.pass_condition().into(),
            })
            .collect(),
        categories: CATEGORIES.iter().map(|c| c.to_string()).collect(),
        routes: Vec::new(),
    }
}

// ------------------------------------------------------------- triage node

/// The instructions of a triage run: `ir:triage` generalised from one
/// GitHub repository to any item in any scope. The run reads, assesses and
/// submits; it changes no file and posts nothing anywhere. `routes` is every
/// scope with its agents and workflows, so the run chooses from what exists.
/// `record` is the item's own intake record, freshly searched for
/// duplicates (`#166`) -- not necessarily `item.intake`, which may still be
/// the record from before that search.
pub fn triage_instructions(item: &Task, record: &Intake, routes: &[RouteOptions], bin: &str) -> String {
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
    out.push_str("## Possible duplicates\n\n");
    if record.candidates.is_empty() {
        out.push_str("None found among the open tasks Factory searched.\n\n");
    } else {
        out.push_str(
            "Factory found these among the open tasks -- confirm or reject each in your assessment's \
             `duplicates`, copying it back with `verdict` set to `confirmed` or `rejected` and your \
             own evidence sentence. Leaving one out, or leaving its verdict `unverified`, is refused, \
             and so is an answer with no evidence:\n\n",
        );
        out.push_str(&serde_json::to_string_pretty(&record.candidates).unwrap_or_default());
        out.push_str("\n\n");
    }
    out.push_str(
        "Also run `factory knowledge search` for a documented duplicate in the vault. Add any you \
         find as your own candidate in `duplicates` -- `kind: \"knowledge\"`, its vault path as \
         `reference`, `match: \"text\"` with a `score` of your choosing, `verdict: \"confirmed\"`, \
         and the evidence. Factory stores it exactly as given and never checks it against the vault \
         itself.\n\n",
    );
    out.push_str(
        "## How to triage\n\n\
         1. Read the item. Inspect the code, documents and prior work it touches, read-only; \
         `factory knowledge search` and `factory task list` find related work. Answer every \
         possible duplicate above.\n\
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
         4. Rules: a confirmed duplicate always gives needs-info, and `--decide` can never \
         release it. Any failed axis gives needs-info too, except observability at low or \
         medium cost. Complexity 9-10 gives needs-info (split it). Otherwise it is ready -- do \
         not block on a reversible implementation choice.\n\
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
         8. Route it: the scope that owns it, and the way of working that suits it best. \
         Choose a workflow when one fits the kind of work -- give every input it declares \
         in `routing.inputs`, and pick each step's agent in `routing.agents` (step id -> \
         agent) where the default is a poor fit: a stronger model for design and review, a \
         cheaper one for mechanical steps. A step's model is its agent's; there is no \
         separate model setting. With no fitting workflow, route to the scope and, if one \
         clearly fits, an agent there. Say why in the summary.\n\
         9. If the scope axis fails or complexity is 9-10, propose a split in `split`: two to \
         eight parts, each bounded enough to pass the seven axes on its own, with a short id, \
         a title, its own instructions, `depends_on` naming the parts that come first, and \
         an acceptance check. It is a proposal -- the verdict stays needs-info and a person \
         decides to split -- so write each part to stand alone.\n\n\
         ## Where it can go\n\n",
    );
    out.push_str(&routes_text(routes));
    out.push_str(
        "\n## Submit\n\n\
         Write the assessment as JSON to a file and submit it:\n\n",
    );
    out.push_str(&format!(
        "    {bin} intake assess {id} --file assessment.json --decide\n\n\
         `--decide` applies what the rules give: ready releases the item into its route, \
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
      "routing": {
        "scope": "<scope>",
        "agent": "<optional, without a workflow>",
        "workflow": "<optional workflow name>",
        "inputs": {"<input>": "<value>"},
        "agents": {"<step id>": "<agent>"}
      },
      "summary": "One or two sentences: what it is, and why this call and this route.",
      "questions": ["Only for needs-info: one concrete question per gap."],
      "split": [
        {"id": "first", "title": "...", "instructions": "...", "acceptance": "..."},
        {"id": "second", "title": "...", "instructions": "...", "depends_on": ["first"], "acceptance": "..."}
      ],
      "duplicates": [
        {"kind": "task", "reference": "<id>", "title": "...", "evidence": "...", "match": "source", "verdict": "confirmed"},
        {"kind": "knowledge", "reference": "<vault/path>", "title": "...", "evidence": "...", "match": "text", "score": 80, "verdict": "confirmed"}
      ]
    }
"#,
    );
    out.push_str(
        "\nLeave out what does not apply: `split` unless it is too big, `inputs` and `agents` \
         without a workflow, `duplicates` only when nothing was found and your own search of \
         the vault found nothing either. Use wontfix only for a verified duplicate, an invalid \
         report or something out of scope -- and then do not decide it yourself: say so in \
         your task report with the evidence, and a person closes it.\n",
    );
    out
}

/// The routes as the triage instructions list them: one block per scope,
/// its agents with their models, then its workflows with inputs and steps.
pub fn routes_text(routes: &[RouteOptions]) -> String {
    let mut out = String::new();
    for r in routes {
        out.push_str(&format!("- scope `{}`", r.scope));
        if let Some(a) = &r.default_agent {
            out.push_str(&format!(" (default agent {a})"));
        }
        out.push('\n');
        if !r.agents.is_empty() {
            let agents: Vec<String> = r
                .agents
                .iter()
                .map(|a| {
                    let model = a.model.as_deref().unwrap_or("default model");
                    format!("{} ({}, {model})", a.name, a.harness)
                })
                .collect();
            out.push_str(&format!("  agents: {}\n", agents.join(", ")));
        }
        for w in &r.workflows {
            out.push_str(&format!("  workflow `{}`", w.name));
            if !w.description.trim().is_empty() {
                out.push_str(&format!(" -- {}", w.description.trim()));
            }
            out.push('\n');
            for i in &w.inputs {
                out.push_str(&format!("    input `{}`", i.name));
                if !i.description.trim().is_empty() {
                    out.push_str(&format!(": {}", i.description.trim()));
                }
                out.push('\n');
            }
            for s in &w.steps {
                out.push_str(&format!(
                    "    step `{}` {} [{}]\n",
                    s.id,
                    s.title,
                    s.agent.as_deref().unwrap_or("the scope's agent")
                ));
            }
        }
    }
    if out.is_empty() {
        out.push_str("(no scopes)\n");
    }
    out
}

// ==================================================================== metrics (#165)
//
// Four registry metrics over the gate's own history: `ready_rate`,
// `needs_info_rate`, `duplicate_rate` and `intake_lead_time`. Read from the
// task journal, not [`Intake::decision`] -- that field keeps only the
// latest decision, so an item sent back for information and later released
// would otherwise lose the needs-info half of its history the moment it
// left intake.

/// One decision a triager made about an item, as the journal recorded it --
/// the unit every intake metric below is built from. `factory-daemon` turns
/// each qualifying journal entry into one with [`decision_event`], already
/// narrowed to a scope subtree; this module never reads a store itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntakeDecisionKind {
    Ready,
    NeedsInfo,
    /// A `wontfix` closed as [`WontfixReason::Duplicate`].
    Duplicate,
    /// A `wontfix` closed as [`WontfixReason::Invalid`] or
    /// [`WontfixReason::OutOfScope`] -- counts in the shared denominator
    /// only, the same as [`Self::Split`].
    OtherWontfix,
    Split,
}

/// One decision event: what it was, when, and the item's own receipt time
/// -- carried on every fact, ready or not, since this module has no task to
/// look back at later; only [`registry_metric`]'s `intake_lead_time` arm
/// ever reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntakeDecisionFact {
    pub kind: IntakeDecisionKind,
    /// `data.decision.at` off the journal entry, falling back to the
    /// entry's own time for `intake_needs_info`, the one decision kind
    /// `intake_decide` journals with no [`DecisionRecord`] to attach --
    /// sent back before an assessment exists for [`Self::at`] to travel
    /// with (see `factory-daemon/src/intake.rs`'s `intake_decide`).
    pub at: DateTime<Utc>,
    pub received_at: DateTime<Utc>,
}

/// What one journal entry means for the intake metrics, or why it does
/// not -- `factory-daemon`'s `TaskStore::entries_of_kinds` hands over
/// `kind` and `data` verbatim for every entry of one of the four decision
/// kinds (`"triage_verdict"`, `"intake_needs_info"`, `"intake_closed"`,
/// `"intake_split"` -- literals here, the same as the daemon's own
/// `intake_closed`/`intake_split`, and kept in step with it by the daemon
/// metrics test that drives the real `intake_decide` rather than by the
/// type system); this is the one place both it and a pure fixture below
/// turn that pair into a fact.
///
/// `None` for a `kind` this module does not treat as a decision event --
/// defensive only, since the daemon asks the store for exactly these four
/// kinds and nothing routes another one through here. `Some(Err(()))` for
/// one of the four whose `data` does not have the shape `intake_decide`
/// writes; the caller counts these into whatever reason a value that came
/// out empty or partial already carries, so a bad row is never silently
/// missing from a count it should have been in.
pub fn decision_event(
    kind: &str,
    data: Option<&serde_json::Value>,
    entry_at: DateTime<Utc>,
    received_at: DateTime<Utc>,
) -> Option<Result<IntakeDecisionFact, ()>> {
    let fact = |kind, at| Some(Ok(IntakeDecisionFact { kind, at, received_at }));
    match kind {
        "intake_needs_info" => fact(IntakeDecisionKind::NeedsInfo, entry_at),
        "triage_verdict" | "intake_closed" | "intake_split" => {
            let Some(record) = data.and_then(|d| d.get("decision")) else { return Some(Err(())) };
            let Ok(record) = serde_json::from_value::<DecisionRecord>(record.clone()) else {
                return Some(Err(()));
            };
            let kind = match &record.decision {
                Decision::Ready { .. } => IntakeDecisionKind::Ready,
                Decision::NeedsInfo { .. } => IntakeDecisionKind::NeedsInfo,
                Decision::Wontfix { reason: WontfixReason::Duplicate, .. } => IntakeDecisionKind::Duplicate,
                Decision::Wontfix { .. } => IntakeDecisionKind::OtherWontfix,
                Decision::Split { .. } => IntakeDecisionKind::Split,
            };
            fact(kind, record.at)
        }
        _ => None,
    }
}

/// One intake metric's value over a window, and what it rests on -- the
/// same shape `usage::UsageFigure` uses for `unit_cost`/`tokens_per_run`.
#[derive(Debug, Clone, PartialEq)]
pub struct IntakeFigure {
    pub value: Option<f64>,
    pub reason: Option<String>,
    /// The newest decision behind the value -- the whole denominator for a
    /// rate, the ready decisions a lead time's median is drawn from --
    /// `None` exactly when `value` is.
    pub as_of: Option<DateTime<Utc>>,
}

fn with_caveat(reason: String, caveat: &Option<String>) -> String {
    match caveat {
        Some(c) => format!("{reason} ({c})"),
        None => reason,
    }
}

fn rate(denom: usize, hits: usize, empty_reason: &str, caveat: &Option<String>, as_of: Option<DateTime<Utc>>) -> IntakeFigure {
    if denom == 0 {
        return IntakeFigure { value: None, reason: Some(empty_reason.to_string()), as_of: None };
    }
    IntakeFigure { value: Some(hits as f64 / denom as f64), reason: caveat.clone(), as_of }
}

/// The four intake metrics (`ready_rate`, `needs_info_rate`,
/// `duplicate_rate`, `intake_lead_time`), from `facts` already narrowed to a
/// scope subtree and read straight off the task journal -- mirrors
/// `operations::registry_metric` for the run-backed families, the one entry
/// point `factory-daemon`'s metric wiring calls so a Goals or Scenarios
/// read can never disagree with another reader of the same journal.
/// `None` for an id that is not one of the four.
///
/// The three rates share one denominator: every decision event in `window`.
/// `ready_rate` and `needs_info_rate` count two events for an item sent
/// back once and later released; `duplicate_rate` counts only a wontfix
/// closed as a duplicate -- an invalid or out-of-scope one, like a split,
/// counts in the denominator only, so the three shares add to one only when
/// there are none of those.
/// `intake_lead_time` is the nearest-rank median, in seconds, of a ready
/// decision's own time minus the item's `received_at`, over items released
/// ready in `window` -- [`crate::scenario::nearest_rank`], the same rule
/// `operations::registry_metric`'s `cycle_time_p50` uses.
///
/// `skipped` -- how many decision-kind entries [`decision_event`] could not
/// parse in this same read -- is folded into whatever reason a value that
/// came out `None` or partial already carries, never silently dropped from
/// the count it should have been in.
pub fn registry_metric(
    id: &str,
    facts: &[IntakeDecisionFact],
    window: &Window,
    skipped: usize,
    window_days: i64,
) -> Option<IntakeFigure> {
    let in_window: Vec<&IntakeDecisionFact> = facts.iter().filter(|f| window.contains(f.at)).collect();
    let denom = in_window.len();
    let caveat = (skipped > 0).then(|| {
        format!(
            "{skipped} decision entr{} could not be read and {} left out of this count",
            if skipped == 1 { "y" } else { "ies" },
            if skipped == 1 { "was" } else { "were" },
        )
    });
    let no_decisions = with_caveat(format!("no triage decisions in the trailing {window_days} days"), &caveat);
    let count = |wanted: IntakeDecisionKind| in_window.iter().filter(|f| f.kind == wanted).count();
    let newest = || in_window.iter().map(|f| f.at).max();
    Some(match id {
        "ready_rate" => rate(denom, count(IntakeDecisionKind::Ready), &no_decisions, &caveat, newest()),
        "needs_info_rate" => rate(denom, count(IntakeDecisionKind::NeedsInfo), &no_decisions, &caveat, newest()),
        "duplicate_rate" => rate(denom, count(IntakeDecisionKind::Duplicate), &no_decisions, &caveat, newest()),
        "intake_lead_time" => {
            let mut ready: Vec<&IntakeDecisionFact> =
                in_window.iter().copied().filter(|f| f.kind == IntakeDecisionKind::Ready).collect();
            if ready.is_empty() {
                let reason = if denom == 0 {
                    no_decisions
                } else {
                    with_caveat(format!("no items released ready in the trailing {window_days} days"), &caveat)
                };
                return Some(IntakeFigure { value: None, reason: Some(reason), as_of: None });
            }
            ready.sort_by_key(|f| f.at);
            let mut lead_times: Vec<f64> =
                ready.iter().map(|f| (f.at - f.received_at).num_seconds() as f64).collect();
            lead_times.sort_by(|a, b| a.total_cmp(b));
            let median = lead_times[crate::scenario::nearest_rank(lead_times.len(), 0.5)];
            IntakeFigure { value: Some(median), reason: caveat, as_of: ready.last().map(|f| f.at) }
        }
        _ => return None,
    })
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
            split: vec![],
            duplicates: vec![],
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
    fn github_source_kind_has_a_stable_wire_name() {
        let json = serde_json::to_string(&SourceKind::Github).unwrap();
        assert_eq!(json, "\"github\"");
        assert_eq!(serde_json::from_str::<SourceKind>(&json).unwrap(), SourceKind::Github);
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
            candidates: vec![],
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

    /// A task with its own title and instructions -- what
    /// `duplicate_candidates`'s text match reads -- and an optional intake
    /// record for the source match and the "open intake item" kind.
    fn item_task(id: &str, title: &str, instructions: &str, status: TaskStatus, intake: Option<Intake>) -> Task {
        let mut t = task(id, status, intake);
        t.title = title.into();
        t.instructions = instructions.into();
        t
    }

    fn received(reference: Option<&str>) -> Intake {
        Intake {
            stage: IntakeStage::Received,
            source: IntakeSource { kind: SourceKind::Github, reference: reference.map(String::from) },
            requester: "the owner".into(),
            received_at: at(),
            triage: None,
            triage_task: None,
            questions: vec![],
            decision: None,
            candidates: vec![],
        }
    }

    #[test]
    fn the_same_source_reference_is_the_strongest_evidence() {
        let item = item_task(
            "new",
            "Different words entirely",
            "nothing in common with the other one's text",
            TaskStatus::Intake,
            Some(received(Some("https://github.com/acme/widgets/issues/9"))),
        );
        let same_ref = item_task(
            "old",
            "Totally unrelated title too",
            "and unrelated instructions as well",
            TaskStatus::Intake,
            Some(received(Some("https://github.com/acme/widgets/issues/9"))),
        );
        let other_ref = item_task(
            "other",
            "Also unrelated",
            "also unrelated",
            TaskStatus::Intake,
            Some(received(Some("https://github.com/acme/widgets/issues/10"))),
        );
        let candidates = duplicate_candidates(&item, &[same_ref, other_ref]);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].reference, "old");
        assert_eq!(candidates[0].kind, DuplicateKind::IntakeItem);
        assert_eq!(candidates[0].matched, DuplicateMatch::Source);
        assert_eq!(candidates[0].verdict, DuplicateVerdict::Unverified);
        assert_eq!(candidates[0].score, None);
        assert!(candidates[0].evidence.contains("github"), "{:?}", candidates[0].evidence);
    }

    #[test]
    fn a_near_identical_title_gives_a_text_match() {
        // This pair's overlap lands at exactly 60%, `TEXT_MATCH_THRESHOLD`
        // itself -- pinned there on purpose, to lock the boundary being
        // inclusive (`>=`). A future change to the stopword list or the
        // instruction-prefix length can move this score; if it drops the
        // match, that is the signal to look at the rule, not this fixture.
        let item = item_task(
            "new",
            "Checkout crashes when applying a coupon code",
            "Customers report the checkout page crashes whenever a coupon code is applied at checkout.",
            TaskStatus::Intake,
            None,
        );
        let near_duplicate = item_task(
            "old",
            "Coupon code crashes the checkout page",
            "The checkout page crashes when a coupon code is applied at checkout.",
            TaskStatus::Pending,
            None,
        );
        let candidates = duplicate_candidates(&item, &[near_duplicate]);
        assert_eq!(candidates.len(), 1, "{candidates:?}");
        assert_eq!(candidates[0].reference, "old");
        assert_eq!(candidates[0].kind, DuplicateKind::Task, "released, not in intake");
        assert_eq!(candidates[0].matched, DuplicateMatch::Text);
        assert_eq!(candidates[0].score, Some(60), "pinned at the threshold itself, see above");
    }

    #[test]
    fn unrelated_work_gives_no_candidate() {
        let item = item_task(
            "new",
            "Checkout crashes when applying a coupon code",
            "Customers report the checkout page crashes whenever a coupon code is applied at checkout.",
            TaskStatus::Intake,
            None,
        );
        let unrelated =
            item_task("other", "Add dark mode to the dashboard", "Give the dashboard a dark theme option.", TaskStatus::Pending, None);
        assert_eq!(duplicate_candidates(&item, &[unrelated]), Vec::new());
    }

    #[test]
    fn a_pair_of_generic_items_with_shared_boilerplate_never_matches_on_that_alone() {
        // Two items whose only common ground is a stock phrase used across
        // many fixtures ("fix it") must not become duplicates of each other
        // just because they share that one word -- `MIN_SHARED_TOKENS`.
        let a = item_task("a", "Broken link", "fix it", TaskStatus::Intake, None);
        let b = item_task("b", "Something else", "fix it", TaskStatus::Intake, None);
        assert_eq!(duplicate_candidates(&a, &[b]), Vec::new());
    }

    #[test]
    fn exclusions_the_item_itself_triage_tasks_its_parent_and_its_parts() {
        let mut item = item_task("item", "Checkout crashes on coupon", "checkout page crashes on coupon code", TaskStatus::Intake, None);
        item.labels.insert(PARENT_LABEL.into(), "parent".into());

        let itself = item.clone();
        let mut triage_run = item_task("triage-task", "Checkout crashes on coupon", "checkout page crashes on coupon code", TaskStatus::Pending, None);
        triage_run.labels.insert(TRIAGE_LABEL.into(), "item".into());
        let mut parent = item_task("parent", "Checkout crashes on coupon", "checkout page crashes on coupon code", TaskStatus::Intake, None);
        parent.labels.insert(PARENT_LABEL.into(), "grandparent".into());
        let mut part = item_task("part", "Checkout crashes on coupon", "checkout page crashes on coupon code", TaskStatus::Intake, None);
        part.labels.insert(PARENT_LABEL.into(), "item".into());
        let mut sibling = item_task("sibling", "Checkout crashes on coupon", "checkout page crashes on coupon code", TaskStatus::Intake, None);
        sibling.labels.insert(PARENT_LABEL.into(), "parent".into());

        let candidates = duplicate_candidates(&item, &[itself, triage_run, parent, part, sibling]);
        assert_eq!(candidates, Vec::new(), "{candidates:?}");
    }

    #[test]
    fn closed_tasks_are_never_candidates_even_when_identical() {
        let item = item_task("item", "Checkout crashes on coupon", "checkout page crashes on coupon code", TaskStatus::Intake, None);
        let done = item_task("done", "Checkout crashes on coupon", "checkout page crashes on coupon code", TaskStatus::Done, None);
        let cancelled = item_task("cancelled", "Checkout crashes on coupon", "checkout page crashes on coupon code", TaskStatus::Cancelled, None);
        assert_eq!(duplicate_candidates(&item, &[done, cancelled]), Vec::new());
    }

    #[test]
    fn a_task_blocked_by_a_failure_is_still_open_work() {
        let item = item_task("item", "Checkout crashes on coupon", "checkout page crashes on coupon code", TaskStatus::Intake, None);
        let mut blocked =
            item_task("blocked", "Checkout crashes on coupon", "checkout page crashes on coupon code", TaskStatus::Blocked, None);
        blocked.failure = Some(crate::task::TaskFailure {
            kind: Some(crate::run::FailKind::AgentFailed),
            run_id: Some("r1".into()),
            attempt: Some(1),
            at: at(),
        });
        let candidates = duplicate_candidates(&item, &[blocked]);
        assert_eq!(candidates.len(), 1, "Task::is_settled says closed; it is not -- #166 uses is_terminal");
    }

    #[test]
    fn stable_order_and_a_cap_of_five() {
        let item = item_task("item", "Widget", "widget", TaskStatus::Intake, Some(received(Some("dup"))));
        let others: Vec<Task> = ["g", "f", "e", "d", "c", "b", "a"]
            .into_iter()
            .map(|id| item_task(id, "Unrelated title", "unrelated text", TaskStatus::Intake, Some(received(Some("dup")))))
            .collect();
        let candidates = duplicate_candidates(&item, &others);
        assert_eq!(candidates.len(), 5, "capped");
        let ids: Vec<&str> = candidates.iter().map(|c| c.reference.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c", "d", "e"], "same score, so ordered by id");
        // Same input, same output.
        assert_eq!(duplicate_candidates(&item, &others), candidates);
    }

    #[test]
    fn an_intake_row_without_candidates_still_deserialises() {
        let json = r#"{
            "stage": "received",
            "source": {"kind": "cli"},
            "requester": "the owner",
            "received_at": "2026-09-25T10:00:00Z"
        }"#;
        let intake: Intake = serde_json::from_str(json).unwrap();
        assert!(intake.candidates.is_empty());
        assert!(candidates_with_verdicts(&intake).is_empty());
    }

    #[test]
    fn a_confirmed_duplicate_blocks_release_and_close_duplicate_leads_the_next_actions() {
        let mut a = assessment();
        a.duplicates = vec![DuplicateCandidate {
            kind: DuplicateKind::Task,
            reference: "t-1".into(),
            title: "The original report".into(),
            evidence: "same bug, reported twice".into(),
            matched: DuplicateMatch::Source,
            score: None,
            verdict: DuplicateVerdict::Confirmed,
        }];
        let triage = evaluate(&a, "x", at());
        let Verdict::NeedsInfo { blockers } = &triage.verdict else { panic!("expected needs-info") };
        assert!(blockers[0].contains("confirmed duplicate of t-1"), "{blockers:?}");

        let mut intake = received(None);
        intake.triage = Some(triage);
        let actions = next_actions(&intake);
        assert_eq!(actions[0].action, NextActionKind::CloseDuplicate);
        assert_eq!(actions[0].reference.as_deref(), Some("t-1"));
        assert!(actions[0].hint.contains("never the triage run"));
    }

    #[test]
    fn a_rejected_duplicate_leaves_the_verdict_ready() {
        let mut a = assessment();
        a.duplicates = vec![DuplicateCandidate {
            kind: DuplicateKind::Task,
            reference: "t-1".into(),
            title: "Not actually the same".into(),
            evidence: "different root cause".into(),
            matched: DuplicateMatch::Source,
            score: None,
            verdict: DuplicateVerdict::Rejected,
        }];
        assert_eq!(evaluate(&a, "x", at()).verdict, Verdict::Ready);
    }

    #[test]
    fn validation_refuses_a_stored_candidate_left_unanswered_or_answered_without_evidence() {
        let stored = vec![DuplicateCandidate {
            kind: DuplicateKind::Task,
            reference: "t-1".into(),
            title: "The original".into(),
            evidence: "same github reference".into(),
            matched: DuplicateMatch::Source,
            score: None,
            verdict: DuplicateVerdict::Unverified,
        }];
        assert!(validate_duplicates(&stored, &[]).unwrap_err().contains("needs a verdict"));

        let unverified = vec![DuplicateCandidate { verdict: DuplicateVerdict::Unverified, ..stored[0].clone() }];
        assert!(validate_duplicates(&stored, &unverified).unwrap_err().contains("needs a verdict"));

        let no_evidence =
            vec![DuplicateCandidate { verdict: DuplicateVerdict::Confirmed, evidence: "  ".into(), ..stored[0].clone() }];
        assert!(validate_duplicates(&stored, &no_evidence).unwrap_err().contains("needs evidence"));

        let answered = vec![DuplicateCandidate { verdict: DuplicateVerdict::Rejected, ..stored[0].clone() }];
        assert!(validate_duplicates(&stored, &answered).is_ok());

        // A brand-new knowledge candidate, naming no stored one, is fine as
        // long as it has its own evidence.
        let knowledge = vec![DuplicateCandidate {
            kind: DuplicateKind::Knowledge,
            reference: "specs/duplicate-detection.md".into(),
            title: "Duplicate detection design".into(),
            evidence: "documents the same decision".into(),
            matched: DuplicateMatch::Text,
            score: Some(75),
            verdict: DuplicateVerdict::Confirmed,
        }];
        assert!(validate_duplicates(&[], &knowledge).is_ok());
        assert!(validate_duplicates(&stored, &knowledge).is_err(), "the stored one is still unanswered");
    }

    #[test]
    fn candidates_with_verdicts_overlays_the_answer_and_keeps_a_knowledge_addition() {
        let mut intake = received(None);
        intake.candidates = vec![DuplicateCandidate {
            kind: DuplicateKind::Task,
            reference: "t-1".into(),
            title: "The original".into(),
            evidence: "same github reference".into(),
            matched: DuplicateMatch::Source,
            score: None,
            verdict: DuplicateVerdict::Unverified,
        }];
        assert_eq!(candidates_with_verdicts(&intake), intake.candidates);

        let mut a = assessment();
        a.duplicates = vec![
            DuplicateCandidate { verdict: DuplicateVerdict::Confirmed, evidence: "yes, same one".into(), ..intake.candidates[0].clone() },
            DuplicateCandidate {
                kind: DuplicateKind::Knowledge,
                reference: "specs/checkout.md".into(),
                title: "Checkout design".into(),
                evidence: "documents the same flow".into(),
                matched: DuplicateMatch::Text,
                score: Some(70),
                verdict: DuplicateVerdict::Confirmed,
            },
        ];
        intake.triage = Some(evaluate(&a, "x", at()));
        let overlaid = candidates_with_verdicts(&intake);
        assert_eq!(overlaid.len(), 2);
        assert_eq!(overlaid[0].verdict, DuplicateVerdict::Confirmed);
        assert_eq!(overlaid[0].evidence, "yes, same one");
        assert_eq!(overlaid[1].kind, DuplicateKind::Knowledge);
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
            parts: vec![],
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
            parts: vec![],
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
        let record = open(None);
        let item = task("item-1", TaskStatus::Intake, Some(record.clone()));
        let routes = vec![
            RouteOptions { scope: "demo".into(), ..Default::default() },
            RouteOptions {
                scope: "web".into(),
                default_agent: Some("claude-code".into()),
                agents: vec![AgentOption { name: "reviewer".into(), harness: "codex".into(), model: Some("gpt-5".into()) }],
                workflows: vec![WorkflowOption {
                    id: "w1".into(),
                    name: "github-issue".into(),
                    description: "issue to PR".into(),
                    inputs: vec![crate::workflow::WorkflowInput { name: "issue".into(), description: "the number".into() }],
                    steps: vec![WorkflowStep { id: "review".into(), title: "Review #{{issue}}".into(), agent: Some("builder".into()) }],
                }],
            },
        ];
        let text = triage_instructions(&item, &record, &routes, "/bin/factory");
        for axis in Axis::ALL {
            assert!(text.contains(axis.pass_condition()), "{axis:?}");
        }
        assert!(text.contains("/bin/factory intake assess item-1 --file assessment.json --decide"));
        assert!(text.contains("- scope `demo`") && text.contains("- scope `web` (default agent claude-code)"));
        assert!(text.contains("agents: reviewer (codex, gpt-5)"), "{text}");
        assert!(text.contains("workflow `github-issue` -- issue to PR"));
        assert!(text.contains("input `issue`: the number"));
        assert!(text.contains("step `review` Review #{{issue}} [builder]"));
        assert!(text.contains("\"split\""), "the shape shows a split");
        assert!(text.contains("\"duplicates\""), "the shape shows duplicates too");
        assert!(text.contains("post nothing outside Factory"));
        assert!(text.contains("None found among the open tasks"), "no candidates were given");
    }

    #[test]
    fn the_triage_instructions_list_stored_candidates_for_the_run_to_answer() {
        let mut record = open(None);
        record.candidates = vec![DuplicateCandidate {
            kind: DuplicateKind::IntakeItem,
            reference: "item-9".into(),
            title: "Same bug, reported again".into(),
            evidence: "same github reference as item-9".into(),
            matched: DuplicateMatch::Source,
            score: None,
            verdict: DuplicateVerdict::Unverified,
        }];
        let item = task("item-1", TaskStatus::Intake, Some(record.clone()));
        let text = triage_instructions(&item, &record, &[], "/bin/factory");
        assert!(text.contains("item-9"));
        assert!(text.contains("\"match\": \"source\""));
        assert!(text.contains("confirm or reject each"));
        assert!(text.contains("factory knowledge search"));
        assert!(!text.contains("None found among the open tasks"));
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

    fn part(id: &str, deps: &[&str]) -> SplitPart {
        SplitPart {
            id: id.into(),
            title: format!("Part {id}"),
            instructions: format!("do {id}"),
            depends_on: deps.iter().map(|d| d.to_string()).collect(),
            acceptance: None,
        }
    }

    #[test]
    fn a_split_needs_two_to_eight_unique_parts_that_depend_only_on_each_other_without_a_cycle() {
        assert!(validate_split(&[part("a", &[])]).unwrap_err().contains("two"));
        let many: Vec<SplitPart> = (0..9).map(|i| part(&format!("p{i}"), &[])).collect();
        assert!(validate_split(&many).unwrap_err().contains("at most 8"));
        assert!(validate_split(&[part("a", &[]), part("a", &[])]).unwrap_err().contains("twice"));
        assert!(validate_split(&[part("A b", &[]), part("c", &[])]).unwrap_err().contains("slug"));
        assert!(validate_split(&[part("a", &["zz"]), part("b", &[])]).unwrap_err().contains("not a part"));
        assert!(validate_split(&[part("a", &["a"]), part("b", &[])]).unwrap_err().contains("itself"));
        assert!(validate_split(&[part("a", &["b"]), part("b", &["a"])]).unwrap_err().contains("circle"));
        let mut untitled = part("b", &[]);
        untitled.title = " ".into();
        assert!(validate_split(&[part("a", &[]), untitled]).unwrap_err().contains("title"));

        let parts = [part("rework", &["resume"]), part("resume", &[]), part("waiting", &[])];
        assert!(validate_split(&parts).is_ok());
        let order: Vec<&str> = split_order(&parts).unwrap().iter().map(|p| p.id.as_str()).collect();
        assert_eq!(order, vec!["resume", "rework", "waiting"], "dependencies first, otherwise as written");
    }

    #[test]
    fn an_assessment_may_propose_a_split_and_the_verdict_is_unchanged() {
        let mut a = failing(Axis::Scope, None);
        a.complexity = 10;
        a.split = vec![part("a", &[])];
        assert!(validate(&a).unwrap_err().contains("proposed split"));
        a.split = vec![part("a", &[]), part("b", &["a"])];
        assert!(validate(&a).is_ok());
        assert!(matches!(evaluate(&a, "x", at()).verdict, Verdict::NeedsInfo { .. }));
    }

    #[test]
    fn inputs_and_step_agents_need_a_workflow() {
        let mut a = assessment();
        a.routing.inputs.insert("issue".into(), "178".into());
        assert!(validate(&a).unwrap_err().contains("need a workflow"));
        a.routing.workflow = Some("github-issue".into());
        a.routing.agents.insert("review".into(), "codex".into());
        assert!(validate(&a).is_ok());
        let json = serde_json::to_value(&a.routing).unwrap();
        assert_eq!(json["inputs"]["issue"], "178");
        assert_eq!(json["agents"]["review"], "codex");
        let bare: Routing = serde_json::from_str(r#"{"scope":"demo"}"#).unwrap();
        assert!(bare.inputs.is_empty() && bare.agents.is_empty(), "older routes still load");
    }

    #[test]
    fn split_takes_the_given_parts_or_the_proposal_and_refuses_neither() {
        let mut a = failing(Axis::Scope, None);
        a.split = vec![part("a", &[]), part("b", &["a"])];
        let item = open(Some(evaluate(&a, "x", at())));
        let from_proposal = split_parts(&item, &[]).unwrap();
        assert_eq!(from_proposal.len(), 2);
        let given = split_parts(&item, &[part("x", &[]), part("y", &[])]).unwrap();
        assert_eq!(given[0].id, "x");
        assert!(check_decision(&item, &Decision::Split { parts: vec![] }).is_ok());

        let bare = open(None);
        assert!(check_decision(&bare, &Decision::Split { parts: vec![] }).unwrap_err().contains("no parts"));
        assert!(
            check_decision(&bare, &Decision::Split { parts: vec![part("a", &[]), part("b", &[])] }).is_ok(),
            "a person splits an item nobody assessed"
        );
        let decision: Decision = serde_json::from_str(r#"{"decision":"split"}"#).unwrap();
        assert_eq!(decision, Decision::Split { parts: vec![] });
    }

    #[test]
    fn next_actions_turn_each_blocker_into_split_or_add_info() {
        // Scope and complexity 10: split, with the proposal named.
        let mut a = failing(Axis::Scope, None);
        a.complexity = 10;
        let v = a.axes.iter_mut().find(|c| c.axis == Axis::Verifiability).unwrap();
        v.pass = false;
        v.evidence = "no done signal".into();
        let mut item = open(Some(evaluate(&a, "x", at())));
        item.stage = IntakeStage::NeedsInfo;
        let actions = next_actions(&item);
        assert_eq!(actions.iter().map(|n| n.action).collect::<Vec<_>>(), vec![NextActionKind::Split, NextActionKind::AddInfo]);
        assert_eq!(actions[0].reasons.len(), 2, "{:?}", actions[0].reasons);
        assert!(actions[0].reasons[1].starts_with("Complexity 10"));
        assert!(actions[0].hint.contains("triage it again for a proposal"));
        assert_eq!(actions[1].reasons, vec!["Verifiability: no done signal"]);

        a.split = vec![part("a", &[]), part("b", &[])];
        item.triage = Some(evaluate(&a, "x", at()));
        assert!(next_actions(&item)[0].hint.contains("2 parts: Part a; Part b"));

        // A high-cost observability gap is split out; a tolerated one is nothing.
        let high = open(Some(evaluate(&failing(Axis::Observability, Some(ObservabilityCost::High)), "x", at())));
        assert_eq!(next_actions(&high)[0].action, NextActionKind::Split);
        let low = open(Some(evaluate(&failing(Axis::Observability, Some(ObservabilityCost::Low)), "x", at())));
        assert!(next_actions(&low).is_empty(), "ready: nothing to unblock");
        assert!(next_actions(&open(None)).is_empty(), "not assessed: nothing to say yet");
    }

    #[test]
    fn the_model_is_read_off_an_agents_args() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(model_of(&args(&["--model", "opus"])), Some("opus".into()));
        assert_eq!(model_of(&args(&["--yolo", "--model=gpt-5"])), Some("gpt-5".into()));
        assert_eq!(model_of(&args(&["-m", "o3"])), Some("o3".into()));
        assert_eq!(model_of(&args(&["--yolo"])), None);
    }

    #[test]
    fn the_board_counts_a_split_and_marks_its_parts() {
        let now = at();
        let mut split = open(None);
        split.stage = IntakeStage::Split;
        split.decision = Some(DecisionRecord {
            decision: Decision::Split { parts: vec![] },
            by: "x".into(),
            at: now,
            workflow_run: None,
            parts: vec!["c1".into()],
        });
        let mut child = open(None);
        child.stage = IntakeStage::Received;
        let mut part_task = task("c1", TaskStatus::Intake, Some(child));
        part_task.labels.insert(PARENT_LABEL.into(), "big".into());
        let b = board(&[task("big", TaskStatus::Done, Some(split)), part_task], now);
        assert_eq!(b.split, 1);
        assert_eq!(b.columns.received.len(), 1);
        assert_eq!(b.columns.received[0].parent.as_deref(), Some("big"));
    }

    // -- intake metrics (#165) ------------------------------------------

    fn dfact(kind: IntakeDecisionKind, at: DateTime<Utc>, received_at: DateTime<Utc>) -> IntakeDecisionFact {
        IntakeDecisionFact { kind, at, received_at }
    }

    #[test]
    fn ready_needs_info_and_duplicate_rate_share_one_denominator() {
        let now = at();
        let window = Window::trailing(now, 28);
        // Six decision events: 2 ready, 1 needs-info, 1 duplicate, 1 other
        // wontfix, 1 split -- the last two count in the shared denominator
        // only, so the three shares below never add to more than 4/6.
        let facts = vec![
            dfact(IntakeDecisionKind::Ready, now - chrono::Duration::days(1), now - chrono::Duration::days(10)),
            dfact(IntakeDecisionKind::Ready, now - chrono::Duration::days(2), now - chrono::Duration::days(9)),
            dfact(IntakeDecisionKind::NeedsInfo, now - chrono::Duration::days(3), now - chrono::Duration::days(3)),
            dfact(IntakeDecisionKind::Duplicate, now - chrono::Duration::days(4), now - chrono::Duration::days(4)),
            dfact(IntakeDecisionKind::OtherWontfix, now - chrono::Duration::days(5), now - chrono::Duration::days(5)),
            dfact(IntakeDecisionKind::Split, now - chrono::Duration::days(6), now - chrono::Duration::days(6)),
        ];
        let ready = registry_metric("ready_rate", &facts, &window, 0, 28).unwrap();
        let needs_info = registry_metric("needs_info_rate", &facts, &window, 0, 28).unwrap();
        let duplicate = registry_metric("duplicate_rate", &facts, &window, 0, 28).unwrap();
        assert_eq!(ready.value, Some(2.0 / 6.0));
        assert_eq!(needs_info.value, Some(1.0 / 6.0));
        assert_eq!(duplicate.value, Some(1.0 / 6.0));
        assert_eq!(ready.value.unwrap() + needs_info.value.unwrap() + duplicate.value.unwrap(), 4.0 / 6.0);
        // `as_of` is the newest event behind the whole denominator, not
        // just a metric's own numerator -- here the ready decision one day
        // ago, even for `duplicate_rate`, whose own hit is four days old.
        assert_eq!(ready.as_of, Some(now - chrono::Duration::days(1)));
        assert_eq!(duplicate.as_of, Some(now - chrono::Duration::days(1)));
    }

    #[test]
    fn a_needs_info_then_a_later_ready_decision_counts_as_two_events() {
        let now = at();
        let window = Window::trailing(now, 28);
        let facts = vec![
            dfact(IntakeDecisionKind::NeedsInfo, now - chrono::Duration::days(5), now - chrono::Duration::days(6)),
            dfact(IntakeDecisionKind::Ready, now - chrono::Duration::days(1), now - chrono::Duration::days(6)),
        ];
        assert_eq!(registry_metric("ready_rate", &facts, &window, 0, 28).unwrap().value, Some(0.5));
        assert_eq!(registry_metric("needs_info_rate", &facts, &window, 0, 28).unwrap().value, Some(0.5));
    }

    #[test]
    fn a_duplicate_wontfix_counts_apart_from_an_invalid_or_out_of_scope_one() {
        let now = at();
        let window = Window::trailing(now, 28);
        let facts = vec![
            dfact(IntakeDecisionKind::Duplicate, now - chrono::Duration::days(1), now - chrono::Duration::days(1)),
            dfact(IntakeDecisionKind::OtherWontfix, now - chrono::Duration::days(2), now - chrono::Duration::days(2)),
            dfact(IntakeDecisionKind::OtherWontfix, now - chrono::Duration::days(3), now - chrono::Duration::days(3)),
        ];
        assert_eq!(registry_metric("duplicate_rate", &facts, &window, 0, 28).unwrap().value, Some(1.0 / 3.0));
    }

    #[test]
    fn intake_lead_time_is_the_nearest_rank_median_in_seconds() {
        let now = at();
        let window = Window::trailing(now, 28);
        // Four ready decisions with lead times of 1, 4, 2 and 3 days, in
        // that creation order -- nearest_rank(4, 0.5) = round(0.5 * 3) = 2
        // (0-based), so sorted ascending [1, 2, 3, 4] days picks the
        // third-smallest: 3 days.
        let facts: Vec<IntakeDecisionFact> = [1_i64, 4, 2, 3]
            .into_iter()
            .enumerate()
            .map(|(i, lead_days)| {
                let decided = now - chrono::Duration::hours(i as i64);
                dfact(IntakeDecisionKind::Ready, decided, decided - chrono::Duration::days(lead_days))
            })
            .collect();
        let figure = registry_metric("intake_lead_time", &facts, &window, 0, 28).unwrap();
        assert_eq!(figure.value, Some(chrono::Duration::days(3).num_seconds() as f64));
        // i = 0 (lead 1 day) decided exactly `now`, the newest of the four.
        assert_eq!(figure.as_of, Some(now));
    }

    #[test]
    fn an_empty_window_gives_none_never_zero() {
        let now = at();
        let window = Window::trailing(now, 28);
        let ready = registry_metric("ready_rate", &[], &window, 0, 28).unwrap();
        assert_eq!(ready.value, None);
        assert_eq!(ready.as_of, None);
        assert_eq!(ready.reason.as_deref(), Some("no triage decisions in the trailing 28 days"));

        let lead_time = registry_metric("intake_lead_time", &[], &window, 0, 28).unwrap();
        assert_eq!(lead_time.value, None);
        assert_eq!(lead_time.reason.as_deref(), Some("no triage decisions in the trailing 28 days"));

        // Decisions exist but none was released ready: a distinct reason
        // from the shared "no decisions at all" one.
        let only_needs_info =
            [dfact(IntakeDecisionKind::NeedsInfo, now - chrono::Duration::days(1), now - chrono::Duration::days(2))];
        let lead_time_no_ready = registry_metric("intake_lead_time", &only_needs_info, &window, 0, 28).unwrap();
        assert_eq!(lead_time_no_ready.value, None);
        assert_eq!(
            lead_time_no_ready.reason.as_deref(),
            Some("no items released ready in the trailing 28 days")
        );

        // A decision outside the window is simply not in `in_window` --
        // same empty result, no special case needed.
        let outside = [dfact(IntakeDecisionKind::Ready, now - chrono::Duration::days(40), now - chrono::Duration::days(50))];
        assert_eq!(registry_metric("ready_rate", &outside, &window, 0, 28).unwrap().value, None);

        assert_eq!(registry_metric("bogus_metric", &[], &window, 0, 28), None);
    }

    #[test]
    fn decision_event_skips_unparseable_data_and_the_reason_counts_it() {
        // `intake_needs_info` needs no `data` at all -- the kind alone is
        // the whole signal.
        assert!(matches!(
            decision_event("intake_needs_info", None, at(), at()),
            Some(Ok(IntakeDecisionFact { kind: IntakeDecisionKind::NeedsInfo, .. }))
        ));
        // The other three decision kinds need `data.decision` to parse as a
        // `DecisionRecord`; missing or malformed, both refuse.
        assert_eq!(decision_event("triage_verdict", None, at(), at()), Some(Err(())));
        assert_eq!(
            decision_event("intake_closed", Some(&serde_json::json!({"nope": true})), at(), at()),
            Some(Err(()))
        );
        assert_eq!(
            decision_event("intake_split", Some(&serde_json::json!({"decision": {"not": "a record"}})), at(), at()),
            Some(Err(()))
        );
        // An unrelated kind is not a decision event at all.
        assert_eq!(decision_event("task_updated", None, at(), at()), None);

        // The registry folds a caller-supplied skip count into the reason,
        // whatever the value -- a bad row is never invisible.
        let now = at();
        let window = Window::trailing(now, 28);
        let empty = registry_metric("ready_rate", &[], &window, 2, 28).unwrap();
        assert_eq!(
            empty.reason.as_deref(),
            Some("no triage decisions in the trailing 28 days (2 decision entries could not be read and were left out of this count)")
        );
        let one_skip = registry_metric(
            "ready_rate",
            &[dfact(IntakeDecisionKind::Ready, now - chrono::Duration::days(1), now - chrono::Duration::days(2))],
            &window,
            1,
            28,
        )
        .unwrap();
        assert_eq!(one_skip.value, Some(1.0));
        assert_eq!(
            one_skip.reason.as_deref(),
            Some("1 decision entry could not be read and was left out of this count")
        );
    }
}
