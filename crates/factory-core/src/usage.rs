//! What a run used: tokens, cost, the model -- as the agent runtime saw it.
//!
//! Factory is the process layer and never reads a harness transcript, never
//! learns a harness's file layout, and never talks to an observability tool
//! directly (issue #117). Usage reaches it through one door only:
//! `AgentRuntime::usage`, which answers with a [`SessionUsage`] -- the
//! versioned L3 contract re-exported below, schema 1, owned by #117. herdr answers it by
//! invoking an Irrlicht plugin's `usage` action; a runtime with no such
//! source answers `None`, and that is an honest "unknown", never a zero.
//!
//! The runtime's numbers are *cumulative per harness session*. A run is
//! bracketed by snapshots -- one at dispatch (the baseline), one at every
//! turn end, one as the run ends -- kept append-only as [`UsageSnapshot`]s,
//! and [`run_usage`] takes the difference. The baseline is what makes a
//! pane reused by a later run safe: whatever a session had already spent
//! before this run was handed to it is subtracted, never billed twice.
//!
//! Three rules hold everywhere in here:
//!
//! * **Never a guessed number.** A count the runtime could not observe is
//!   `None`, and a total with one `None` in it is `None` -- not the sum of
//!   the parts that happened to be known.
//! * **Prices are snapshotted.** A cost carries the `pricing_source` it was
//!   computed with, and nothing here ever re-prices it.
//! * **Usage fields only.** The contract is parsed into typed structs, so
//!   whatever else a plugin sends -- an assistant's last words, a task
//!   summary -- is dropped at the door and never stored.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub use factory_agents::usage::{
    HarnessUsage, RateLimit, RateLimitWindow, SessionUsage, SubagentUsage, TokenCounts, UsageCost,
    USAGE_SCHEMA,
};

trait TokenCountsRunExt {
    fn fields(&self) -> [(&'static str, Option<u64>); 4];
    fn from_fields(values: [Option<u64>; 4]) -> Self;
}

impl TokenCountsRunExt for TokenCounts {
    /// The four counts with their contract names, in a fixed order -- what
    /// a note naming an unknown field, and every sum below, walk.
    fn fields(&self) -> [(&'static str, Option<u64>); 4] {
        [
            ("input", self.input),
            ("output", self.output),
            ("cache_read", self.cache_read),
            ("cache_write", self.cache_write),
        ]
    }

    fn from_fields(values: [Option<u64>; 4]) -> Self {
        let [input, output, cache_read, cache_write] = values;
        Self {
            input,
            output,
            cache_read,
            cache_write,
        }
    }
}

/// When a snapshot was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotPoint {
    /// Right after the session came up, before the task was handed over:
    /// the baseline every later snapshot is measured from.
    Dispatch,
    /// The harness said a turn ended.
    TurnEnded,
    /// The run is ending, its session about to be closed.
    RunEnd,
}

impl SnapshotPoint {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dispatch => "dispatch",
            Self::TurnEnded => "turn_ended",
            Self::RunEnd => "run_end",
        }
    }
}

/// One reading of a run's session, kept append-only. Either `usage` or
/// `unknown` is set: a runtime that could not answer is recorded as such,
/// with its reason, never skipped -- a gap in the record should say why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageSnapshot {
    pub run_id: String,
    pub task_id: String,
    pub point: SnapshotPoint,
    /// When the runtime was asked -- what orders snapshots, rather than
    /// when each happened to be stored.
    pub at: DateTime<Utc>,
    /// The runtime asked.
    pub runtime: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<SessionUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown: Option<String>,
}

/// Whether a run's usage could be worked out at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageState {
    /// Measured from a baseline to a later snapshot. Individual fields may
    /// still be `None`; `notes` says which and why.
    Known,
    /// Not measurable -- `reason` says why.
    Unknown,
}

/// What one run used: the difference between its baseline snapshot and the
/// newest one that answered. Derived, never entered -- recomputed from the
/// snapshots whenever one is added, and mirrored on the `Run`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunUsage {
    pub state: UsageState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// A lower bound rather than the whole of it: the newest snapshot is not
    /// the run's end, or a session seen at the baseline dropped out of view.
    /// `notes` says which.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
    #[serde(default)]
    pub tokens: TokenCounts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plan_share: Vec<PlanShare>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_share_unknown: Option<String>,
    /// Provider-window intervals that could not be attributed. Kept beside
    /// any known shares so a later successful interval cannot make an
    /// earlier gap disappear.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plan_share_unknowns: Vec<PlanShareUnknown>,
    /// Every price table behind `cost_usd`. More than one only when the
    /// sessions in the run were priced differently.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pricing_sources: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    /// Harness sessions counted, subagents included.
    #[serde(default)]
    pub sessions: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_at: Option<DateTime<Utc>>,
    /// The snapshot these figures are as of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_of: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_of_point: Option<SnapshotPoint>,
    /// How many snapshots the run has, answered or not.
    #[serde(default)]
    pub snapshots: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl RunUsage {
    pub fn unknown(reason: impl Into<String>, snapshots: u32) -> Self {
        Self {
            state: UsageState::Unknown,
            reason: Some(reason.into()),
            partial: false,
            tokens: TokenCounts::default(),
            cost_usd: None,
            elapsed_seconds: None,
            active_seconds: None,
            plan_share: Vec::new(),
            plan_share_unknown: None,
            plan_share_unknowns: Vec::new(),
            pricing_sources: Vec::new(),
            models: Vec::new(),
            sessions: 0,
            baseline_at: None,
            as_of: None,
            as_of_point: None,
            snapshots,
            notes: Vec::new(),
        }
    }

    pub fn is_known(&self) -> bool {
        self.state == UsageState::Known
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanShareAttribution {
    Direct,
    Apportioned,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanShare {
    pub provider_account: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
    pub window_minutes: u32,
    pub resets_at: String,
    pub used_percent: f64,
    pub attribution: PlanShareAttribution,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution_quality: Option<String>,
    pub as_of: DateTime<Utc>,
}

/// Evidence that one provider-window interval could not be allocated.
/// Window identity and bounds are optional only for failures that prevented
/// Factory from learning them (for example, a missing provider binding).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanShareUnknown {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<DateTime<Utc>>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReEstimate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<crate::task::TimeEstimateRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_seconds: Option<crate::task::TimeEstimateRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<crate::task::CostEstimateRange>,
    pub sample_count: u32,
    pub scope: String,
    pub agent: String,
    pub category: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_wall_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_active_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EstimateComparison<T> {
    pub low: T,
    pub expected: T,
    pub high: T,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual: Option<T>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_over_expected: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub within_range: Option<bool>,
}

impl EstimateComparison<u64> {
    pub fn new(low: u64, expected: u64, high: u64, actual: Option<u64>) -> Self {
        Self {
            low,
            expected,
            high,
            actual,
            actual_over_expected: actual.filter(|_| expected > 0).map(|v| v as f64 / expected as f64),
            within_range: actual.map(|v| v >= low && v <= high),
        }
    }
}

impl EstimateComparison<f64> {
    pub fn new(low: f64, expected: f64, high: f64, actual: Option<f64>) -> Self {
        Self {
            low,
            expected,
            high,
            actual,
            actual_over_expected: actual.filter(|_| expected > 0.0).map(|v| v / expected),
            within_range: actual.map(|v| v >= low && v <= high),
        }
    }
}

/// One counted thing -- a harness session or a subagent -- with the counts
/// it had at the baseline, if it existed then.
struct Counted<'a> {
    id: &'a str,
    tokens: &'a TokenCounts,
    cost: &'a UsageCost,
    unavailable: Option<&'a BTreeMap<String, String>>,
}

fn flatten(usage: &SessionUsage) -> Vec<Counted<'_>> {
    let mut out = Vec::new();
    for s in &usage.sessions {
        out.push(Counted {
            id: &s.session_id,
            tokens: &s.tokens,
            cost: &s.cost,
            unavailable: Some(&s.unavailable),
        });
        for sub in &s.subagents {
            out.push(Counted {
                id: &sub.session_id,
                tokens: &sub.tokens,
                cost: &sub.cost,
                unavailable: None,
            });
        }
    }
    out
}

/// `now - then` for one cumulative counter. `then` absent means the session
/// did not exist at the baseline, so all of `now` is this run's. A counter
/// that went backwards is not something a difference can be taken of.
fn delta_u64(now: Option<u64>, then: Option<Option<u64>>) -> std::result::Result<Option<u64>, &'static str> {
    match (now, then) {
        (None, _) => Ok(None),
        (Some(n), None) => Ok(Some(n)),
        (Some(_), Some(None)) => Ok(None),
        (Some(n), Some(Some(t))) if n >= t => Ok(Some(n - t)),
        (Some(_), Some(Some(_))) => Err("went backwards since the baseline"),
    }
}

fn delta_f64(now: Option<f64>, then: Option<Option<f64>>) -> std::result::Result<Option<f64>, &'static str> {
    match (now, then) {
        (None, _) => Ok(None),
        (Some(n), None) => Ok(Some(n)),
        (Some(_), Some(None)) => Ok(None),
        // A float that "went backwards" by a rounding hair is not a reset.
        (Some(n), Some(Some(t))) if n + 1e-9 >= t => Ok(Some((n - t).max(0.0))),
        (Some(_), Some(Some(_))) => Err("went backwards since the baseline"),
    }
}

fn reason_for(unavailable: Option<&BTreeMap<String, String>>, path: &str) -> String {
    unavailable
        .and_then(|u| u.get(path))
        .map(|r| format!(": {r}"))
        .unwrap_or_default()
}

/// What a run used, from its snapshots (in any order -- see `at`).
///
/// The baseline is the first `Dispatch` snapshot; the figures are as of the
/// newest snapshot after it that answered. Each harness session -- and
/// each subagent, which the contract lists apart from its parent and which
/// is read as *not* included in the parent's own counts -- contributes its
/// growth since the baseline, or everything it has if it did not exist yet.
/// A field unknown for any one contributor makes that field's total
/// unknown.
pub fn run_usage(snapshots: &[UsageSnapshot]) -> RunUsage {
    run_usage_with_prior(snapshots, None)
}

/// Like [`run_usage`], but for a run that may have resumed another's session
/// (`#178`, `factory task run --continue`): `prior` names the session id
/// being resumed and carries the *previous* run's own `RunEnd` reading for
/// it. When this run's own `Dispatch` snapshot never saw that session --
/// the ordinary case, since the runtime is asked for a baseline the moment
/// the new session comes up, before the resumed harness has reported back in
/// -- that reading is spliced into the baseline in its place, so growth is
/// measured from where the previous run left off rather than credited whole
/// to this one (the same rule [`run_usage`] already applies to any session
/// genuinely born after the baseline, which this deliberately does not
/// touch). `None` behaves exactly like `run_usage`.
pub fn run_usage_with_prior(snapshots: &[UsageSnapshot], prior: Option<(&str, &HarnessUsage)>) -> RunUsage {
    let count = snapshots.len() as u32;
    if snapshots.is_empty() {
        return RunUsage::unknown("no usage was recorded for this run", 0);
    }
    // By when each reading was asked for, not when it was stored: a turn
    // end's snapshot is taken off the hook's own path and may land after
    // the run-end one that followed it. A stable sort keeps store order
    // between two taken in the same instant.
    let mut ordered: Vec<UsageSnapshot> = snapshots.to_vec();
    ordered.sort_by_key(|s| s.at);
    let snapshots = ordered.as_slice();
    let Some(base_at) = snapshots.iter().position(|s| s.point == SnapshotPoint::Dispatch) else {
        return RunUsage::unknown("no baseline was taken when this run was dispatched", count);
    };
    let base = &snapshots[base_at];
    let Some(base_usage) = &base.usage else {
        let why = base.unknown.as_deref().unwrap_or("the runtime did not answer");
        return RunUsage::unknown(format!("no baseline at dispatch: {why}"), count);
    };
    // Splice the resumed session's prior ending in as though the baseline
    // had seen it -- only when it did not, since a runtime quick enough to
    // already report the resumed session at dispatch has the truer number.
    let mut spliced;
    let baseline = match prior {
        Some((session_id, prior_usage)) if !base_usage.sessions.iter().any(|s| s.session_id == session_id) => {
            spliced = base_usage.clone();
            spliced.sessions.push(prior_usage.clone());
            &spliced
        }
        _ => base_usage,
    };
    let later = &snapshots[base_at + 1..];
    let Some(latest_snap) = later.iter().rev().find(|s| s.usage.is_some()) else {
        let why = later
            .iter()
            .rev()
            .find_map(|s| s.unknown.as_deref())
            .map(|w| format!("nothing measured since dispatch: {w}"))
            .unwrap_or_else(|| "nothing measured since dispatch yet".into());
        return RunUsage::unknown(why, count);
    };
    let latest = latest_snap.usage.as_ref().expect("found by is_some above");
    if latest.sessions.is_empty() {
        return RunUsage::unknown("the runtime saw no harness session in this run's session", count);
    }

    let mut notes = Vec::new();
    let mut partial = false;
    // Figures as of a turn end while a later snapshot failed: the run's
    // tail is not in them.
    if let Some(after) = later.iter().rev().take_while(|s| s.usage.is_none()).last() {
        partial = true;
        notes.push(format!(
            "as of the {} snapshot: the {} one had no answer ({})",
            latest_snap.point.as_str(),
            after.point.as_str(),
            after.unknown.as_deref().unwrap_or("no reason given")
        ));
    }

    let before: BTreeMap<&str, Counted<'_>> = flatten(baseline).into_iter().map(|c| (c.id, c)).collect();
    let now = flatten(latest);
    let now_ids: BTreeSet<&str> = now.iter().map(|c| c.id).collect();
    for gone in before.keys().filter(|id| !now_ids.contains(*id)) {
        partial = true;
        notes.push(format!(
            "session {gone} was seen at the baseline but not since; whatever it used during the run is not counted"
        ));
    }

    let mut tokens: [Option<u64>; 4] = [Some(0); 4];
    let mut cost: Option<f64> = Some(0.0);
    for c in &now {
        let then = before.get(c.id);
        for (i, (name, value)) in c.tokens.fields().into_iter().enumerate() {
            let prior = then.map(|t| t.tokens.fields()[i].1);
            match delta_u64(value, prior) {
                Ok(Some(d)) => tokens[i] = tokens[i].map(|sum| sum + d),
                Ok(None) => {
                    if tokens[i].is_some() {
                        notes.push(format!(
                            "tokens.{name} is unknown for session {}{}",
                            c.id,
                            reason_for(c.unavailable, &format!("tokens.{name}"))
                        ));
                    }
                    tokens[i] = None;
                }
                Err(why) => {
                    notes.push(format!("tokens.{name} of session {} {why}", c.id));
                    tokens[i] = None;
                }
            }
        }
        match delta_f64(c.cost.usd, then.map(|t| t.cost.usd)) {
            Ok(Some(d)) => cost = cost.map(|sum| sum + d),
            Ok(None) => {
                if cost.is_some() {
                    notes.push(format!(
                        "cost is unknown for session {}{}",
                        c.id,
                        reason_for(c.unavailable, "cost.usd")
                    ));
                }
                cost = None;
            }
            Err(why) => {
                notes.push(format!("cost of session {} {why}", c.id));
                cost = None;
            }
        }
    }

    let baseline_sessions: BTreeMap<&str, &HarnessUsage> = baseline
        .sessions
        .iter()
        .map(|session| (session.session_id.as_str(), session))
        .collect();
    let mut elapsed = Some(0.0);
    let mut active = Some(0.0);
    for session in &latest.sessions {
        let prior = baseline_sessions.get(session.session_id.as_str()).copied();
        match delta_f64(session.elapsed_seconds, prior.map(|s| s.elapsed_seconds)) {
            Ok(Some(delta)) => elapsed = elapsed.map(|sum| sum + delta),
            Ok(None) => {
                if elapsed.is_some() {
                    notes.push(format!("elapsed time is unknown for session {}", session.session_id));
                }
                elapsed = None;
            }
            Err(why) => {
                notes.push(format!("elapsed time of session {} {why}", session.session_id));
                elapsed = None;
            }
        }
        match delta_f64(session.active_seconds, prior.map(|s| s.active_seconds)) {
            Ok(Some(delta)) => active = active.map(|sum| sum + delta),
            Ok(None) => {
                if active.is_some() {
                    notes.push(format!("active time is unknown for session {}", session.session_id));
                }
                active = None;
            }
            Err(why) => {
                notes.push(format!("active time of session {} {why}", session.session_id));
                active = None;
            }
        }
    }

    let mut pricing: BTreeSet<String> = BTreeSet::new();
    let mut models: BTreeSet<String> = BTreeSet::new();
    for s in &latest.sessions {
        if let Some(m) = &s.model {
            models.insert(m.clone());
        }
        for p in std::iter::once(&s.cost).chain(s.subagents.iter().map(|a| &a.cost)) {
            if let Some(src) = &p.pricing_source {
                pricing.insert(src.clone());
            }
        }
    }
    if cost.is_some() && pricing.is_empty() {
        notes.push("the cost names no pricing source".into());
    }

    RunUsage {
        state: UsageState::Known,
        reason: None,
        partial,
        tokens: TokenCounts::from_fields(tokens),
        cost_usd: cost,
        elapsed_seconds: elapsed,
        active_seconds: active,
        plan_share: Vec::new(),
        plan_share_unknown: None,
        plan_share_unknowns: Vec::new(),
        pricing_sources: pricing.into_iter().collect(),
        models: models.into_iter().collect(),
        sessions: now.len() as u32,
        baseline_at: Some(base.at),
        as_of: Some(latest_snap.at),
        as_of_point: Some(latest_snap.point),
        snapshots: count,
        notes,
    }
}

/// The session id `factory task run --continue` (#178) should resume: from
/// `snapshots` (a previous run's own), the harness session named by the
/// *newest* snapshot that saw one whose `adapter` matches. Older snapshots
/// are never consulted even if a newer one is unknown -- an unanswered
/// reading says nothing either way, so this looks further back only when the
/// newer snapshot's `usage` is missing entirely, never when it simply lacks
/// a session on this adapter (rule 6: never "latest session in this
/// directory", only what this task's own adapter was actually seen running
/// as).
pub fn newest_session_id_for_adapter(snapshots: &[UsageSnapshot], adapter: &str) -> Option<String> {
    let mut ordered: Vec<&UsageSnapshot> = snapshots.iter().collect();
    ordered.sort_by_key(|s| s.at);
    ordered.iter().rev().find_map(|snapshot| {
        snapshot
            .usage
            .as_ref()?
            .sessions
            .iter()
            .find(|s| s.adapter.as_deref() == Some(adapter))
            .map(|s| s.session_id.clone())
    })
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlanShareAllocation {
    pub shares: BTreeMap<String, Vec<PlanShare>>,
    pub unknown: BTreeMap<String, Vec<PlanShareUnknown>>,
    /// Every provider-window interval that was looked at, whatever became
    /// of it -- what the L1 provider card reads its attribution from, so the
    /// badge describes the interval the reading closed rather than who is
    /// running when the page is opened.
    pub intervals: Vec<PlanShareInterval>,
}

/// One interval between two consecutive observations of one provider
/// window, across every run bound to the account.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanShareInterval {
    pub provider_account: String,
    pub window_minutes: u32,
    pub resets_at: String,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    /// Percentage points the window moved over the interval.
    pub delta: f64,
    /// `Direct` or `Apportioned` when a positive delta was allocated;
    /// `None` when there was nothing to allocate or it could not be.
    pub attribution: Option<PlanShareAttribution>,
    /// The runs the delta was allocated to.
    pub consumers: Vec<String>,
    /// Why the delta was not allocated, when it was not.
    pub unknown: Option<String>,
}

impl PlanShareAllocation {
    fn record_unknown(&mut self, run_id: &str, unknown: PlanShareUnknown) {
        self.unknown.entry(run_id.to_string()).or_default().push(unknown);
    }
}

#[derive(Clone)]
struct WindowObservation {
    run_id: String,
    account: String,
    provider: Option<String>,
    plan_type: Option<String>,
    quality: Option<String>,
    minutes: u32,
    resets_at: String,
    used: f64,
    at: DateTime<Utc>,
}

/// One point per sample time. Two runs reading the same account in the
/// same instant -- `sampled_at` is often whole seconds -- are one
/// observation, not an interval of zero length: pairing them would drop the
/// growth up to the second or, if they disagree, charge it twice. Equal
/// readings collapse silently. Readings that disagree say so on every run
/// that took one, and the highest stands for the instant, since a window's
/// use only grows until it resets. `PROVIDER_WINDOW_TIE` is the same rule
/// for the L1 provider card.
fn merge_simultaneous(mut points: Vec<WindowObservation>, result: &mut PlanShareAllocation) -> Vec<WindowObservation> {
    points.sort_by(|a, b| {
        a.at.cmp(&b.at)
            .then_with(|| a.used.total_cmp(&b.used))
            .then_with(|| a.run_id.cmp(&b.run_id))
    });
    let mut merged: Vec<WindowObservation> = Vec::with_capacity(points.len());
    let mut start = 0;
    while start < points.len() {
        let end = start + points[start..].iter().take_while(|p| p.at == points[start].at).count();
        let tied = &points[start..end];
        let highest = tied.last().expect("a group holds its first point").clone();
        if tied.iter().any(|p| (p.used - highest.used).abs() > f64::EPSILON) {
            for point in tied {
                result.record_unknown(&point.run_id, PlanShareUnknown {
                    provider_account: Some(point.account.clone()), window_minutes: Some(point.minutes),
                    resets_at: Some(point.resets_at.clone()), from: Some(point.at), to: Some(point.at),
                    reason: PROVIDER_WINDOW_TIE.into(),
                });
            }
        }
        merged.push(highest);
        start = end;
    }
    merged
}

/// Why a run's reading was set aside in favour of another run's taken at
/// the same instant.
pub const PROVIDER_WINDOW_TIE: &str =
    "readings of this provider window sampled at the same instant disagree; the highest stands for it";

/// When the runtime says it took this reading -- the contract's
/// `sampled_at` -- falling back to when Factory asked for it. A provider
/// window and the token counts beside it are ordered and attributed by
/// this, never by the request time alone: a cached or delayed answer
/// describes the moment it was sampled. `UsageSnapshot::at` still orders
/// a run's own snapshots (`run_usage`).
pub fn observed_at(snapshot: &UsageSnapshot) -> DateTime<Utc> {
    snapshot.usage.as_ref().and_then(|usage| usage.sampled_at).unwrap_or(snapshot.at)
}

fn cumulative_tokens(usage: &SessionUsage) -> Option<u64> {
    usage.sessions.iter().try_fold(0u64, |sum, session| {
        let parent = session.tokens.total()?;
        let children = session
            .subagents
            .iter()
            .try_fold(0u64, |subtotal, child| child.tokens.total().map(|n| subtotal + n))?;
        Some(sum + parent + children)
    })
}

fn token_increment(snapshots: &[UsageSnapshot], from: DateTime<Utc>, to: DateTime<Utc>) -> Option<u64> {
    let before = snapshots
        .iter()
        // A weight is honest only when its readings cover this provider
        // observation interval exactly. Reusing an older reading would also
        // charge tokens consumed before `from` to this interval.
        .filter(|s| observed_at(s) == from)
        .max_by_key(|s| s.at)
        // A run that began inside an account observation interval has no
        // reading at `from`. Its dispatch snapshot is the aligned baseline
        // for the part of the interval in which it existed, provided it also
        // has a reading at the interval end.
        .or_else(|| {
            snapshots
                .iter()
                .filter(|s| s.point == SnapshotPoint::Dispatch && observed_at(s) > from && observed_at(s) <= to)
                .min_by_key(|s| observed_at(s))
        })
        .and_then(|s| s.usage.as_ref())
        .and_then(cumulative_tokens)?;
    let after = snapshots
        .iter()
        .filter(|s| observed_at(s) == to)
        .max_by_key(|s| s.at)
        .and_then(|s| s.usage.as_ref())
        .and_then(cumulative_tokens)?;
    after.checked_sub(before)
}

/// Allocate each positive observed provider-window increment among runs that
/// were active in the interval, weighted by their measured token increment.
/// Missing identity, baselines, or weights remains explicitly unknown.
/// Intervals are bounded by each reading's [`observed_at`].
pub fn allocate_plan_share(
    runs: &[crate::run::Run],
    snapshots: &BTreeMap<String, Vec<UsageSnapshot>>,
) -> PlanShareAllocation {
    let mut result = PlanShareAllocation::default();
    let by_id: BTreeMap<&str, &crate::run::Run> = runs.iter().map(|r| (r.id.as_str(), r)).collect();
    let mut resolved = BTreeSet::new();
    let mut observations: BTreeMap<(String, u32, String), Vec<WindowObservation>> = BTreeMap::new();

    for run in runs {
        let Some(account) = run.provider_account.as_ref() else {
            result.record_unknown(&run.id, PlanShareUnknown {
                provider_account: None, window_minutes: None, resets_at: None,
                from: None, to: None, reason: "run has no provider-account binding".into(),
            });
            continue;
        };
        for snapshot in snapshots.get(&run.id).into_iter().flatten() {
            let Some(usage) = &snapshot.usage else { continue };
            let sampled = observed_at(snapshot);
            for session in &usage.sessions {
                let Some(rate) = &session.rate_limit else { continue };
                for window in &rate.windows {
                    let (Some(minutes), Some(reset), Some(used)) =
                        (window.window_minutes, window.resets_at.clone(), window.used_percent)
                    else {
                        result.record_unknown(&run.id, PlanShareUnknown {
                            provider_account: Some(account.clone()), window_minutes: window.window_minutes,
                            resets_at: window.resets_at.clone(), from: None, to: Some(sampled),
                            reason: "rate-limit window identity or usage is missing".into(),
                        });
                        continue;
                    };
                    if !used.is_finite() {
                        result.record_unknown(&run.id, PlanShareUnknown {
                            provider_account: Some(account.clone()), window_minutes: Some(minutes),
                            resets_at: Some(reset), from: None, to: Some(sampled),
                            reason: "rate-limit usage is not finite".into(),
                        });
                        continue;
                    }
                    observations
                        .entry((account.clone(), minutes, reset.clone()))
                        .or_default()
                        .push(WindowObservation {
                            run_id: run.id.clone(),
                            account: account.clone(),
                            provider: rate.provider.clone(),
                            plan_type: rate.plan_type.clone(),
                            quality: rate.attribution_quality.clone(),
                            minutes,
                            resets_at: reset,
                            used,
                            at: sampled,
                        });
                }
            }
        }
    }

    for ((_account, _minutes, _reset), points) in observations {
        let points = merge_simultaneous(points, &mut result);
        if points.len() < 2 {
            for point in points {
                result.record_unknown(&point.run_id, PlanShareUnknown {
                    provider_account: Some(point.account), window_minutes: Some(point.minutes),
                    resets_at: Some(point.resets_at), from: None, to: Some(point.at),
                    reason: "no earlier observation for this provider window".into(),
                });
            }
            continue;
        }
        for pair in points.windows(2) {
            let from = &pair[0];
            let to = &pair[1];
            let delta = to.used - from.used;
            let interval = |attribution, consumers, unknown: Option<&str>| PlanShareInterval {
                provider_account: to.account.clone(),
                window_minutes: to.minutes,
                resets_at: to.resets_at.clone(),
                from: from.at,
                to: to.at,
                delta,
                attribution,
                consumers,
                unknown: unknown.map(str::to_string),
            };
            if delta < 0.0 {
                let reason = "provider-window usage went backwards";
                result.intervals.push(interval(None, Vec::new(), Some(reason)));
                result.record_unknown(&to.run_id, PlanShareUnknown {
                    provider_account: Some(to.account.clone()), window_minutes: Some(to.minutes),
                    resets_at: Some(to.resets_at.clone()), from: Some(from.at), to: Some(to.at),
                    reason: reason.into(),
                });
                continue;
            }
            if to.at <= from.at {
                continue;
            }
            let candidates: Vec<&crate::run::Run> = runs
                .iter()
                .filter(|run| {
                    run.provider_account.as_deref() == Some(to.account.as_str())
                        && run.started_at < to.at
                        && run.ended_at.map_or(true, |ended| ended > from.at)
                })
                .collect();
            if delta <= f64::EPSILON {
                // Two valid provider observations prove that every active
                // consumer's share of this interval is zero. There is no
                // positive share to emit, but this is measured evidence, not
                // a missing baseline that should fall through to the generic
                // unknown below.
                result.intervals.push(interval(None, Vec::new(), None));
                resolved.extend(candidates.into_iter().map(|candidate| candidate.id.clone()));
                continue;
            }
            if candidates.is_empty() {
                let reason = "no run was active during the provider-window increment";
                result.intervals.push(interval(None, Vec::new(), Some(reason)));
                result.record_unknown(&to.run_id, PlanShareUnknown {
                    provider_account: Some(to.account.clone()), window_minutes: Some(to.minutes),
                    resets_at: Some(to.resets_at.clone()), from: Some(from.at), to: Some(to.at),
                    reason: reason.into(),
                });
                continue;
            }
            let mut weighted = Vec::with_capacity(candidates.len());
            let mut missing = false;
            for candidate in &candidates {
                let weight = snapshots
                    .get(&candidate.id)
                    .and_then(|s| token_increment(s, from.at, to.at));
                match weight {
                    Some(0) => {}
                    Some(weight) => weighted.push((*candidate, weight)),
                    None => missing = true,
                }
            }
            if missing || weighted.is_empty() {
                let reason = if missing {
                    "one or more active runs have no measured token increment for provider-window attribution"
                } else {
                    "provider-window usage increased with no positive measured token increment"
                };
                result.intervals.push(interval(None, Vec::new(), Some(reason)));
                for candidate in &candidates {
                    result.record_unknown(&candidate.id, PlanShareUnknown {
                        provider_account: Some(to.account.clone()), window_minutes: Some(to.minutes),
                        resets_at: Some(to.resets_at.clone()), from: Some(from.at), to: Some(to.at),
                        reason: reason.into(),
                    });
                }
                continue;
            }
            let total: u64 = weighted.iter().map(|(_, weight)| *weight).sum();
            let attribution = if weighted.len() == 1 {
                PlanShareAttribution::Direct
            } else {
                PlanShareAttribution::Apportioned
            };
            result.intervals.push(interval(
                Some(attribution),
                weighted.iter().map(|(candidate, _)| candidate.id.clone()).collect(),
                None,
            ));
            for (candidate, weight) in weighted {
                let share = delta * weight as f64 / total as f64;
                let list = result.shares.entry(candidate.id.clone()).or_default();
                if let Some(existing) = list.iter_mut().find(|share| {
                    share.provider_account == to.account
                        && share.window_minutes == to.minutes
                        && share.resets_at == to.resets_at
                }) {
                    existing.used_percent += share;
                    existing.as_of = existing.as_of.max(to.at);
                    if attribution == PlanShareAttribution::Apportioned {
                        existing.attribution = attribution;
                    }
                } else {
                    list.push(PlanShare {
                        provider_account: to.account.clone(),
                        provider: to.provider.clone(),
                        plan_type: to.plan_type.clone(),
                        window_minutes: to.minutes,
                        resets_at: to.resets_at.clone(),
                        used_percent: share,
                        attribution,
                        attribution_quality: to.quality.clone(),
                        as_of: to.at,
                    });
                }
            }
            resolved.extend(candidates.into_iter().map(|candidate| candidate.id.clone()));
        }
    }

    for run in by_id.values() {
        if run.provider_account.is_some()
            && !result.shares.contains_key(&run.id)
            && !result.unknown.contains_key(&run.id)
            && !resolved.contains(&run.id)
        {
            result.record_unknown(&run.id, PlanShareUnknown {
                provider_account: run.provider_account.clone(), window_minutes: None, resets_at: None,
                from: None, to: None, reason: "provider-window share could not be attributed".into(),
            });
        }
    }
    result
}

// -- rolling up ---------------------------------------------------------------

pub use factory_kernel::{CostGroupBy, CostReport, CostRow, TokenSums};

/// Presentation arithmetic stays outside the plain fact schema.
pub trait TokenSumsExt { fn total(&self) -> u64; }

impl TokenSumsExt for TokenSums {
    fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

/// L4 aggregation; never part of L0's plain fact schema.
pub trait CostRowExt {
    fn add(&mut self, usage: Option<&RunUsage>);
    fn add_estimate(&mut self, estimate: Option<&crate::task::Estimate>, terminal_wall_seconds: Option<u64>);
    fn runs_costed(&self) -> u32;
}

impl CostRowExt for CostRow {
    /// Count one run in. `None` is a run with no usage on record at all.
    fn add(&mut self, usage: Option<&RunUsage>) {
        self.runs += 1;
        let Some(u) = usage.filter(|u| u.is_known()) else {
            self.runs_unknown += 1;
            return;
        };
        if u.partial {
            self.runs_partial += 1;
        }
        let t = &u.tokens;
        self.tokens.input += t.input.unwrap_or(0);
        self.tokens.output += t.output.unwrap_or(0);
        self.tokens.cache_read += t.cache_read.unwrap_or(0);
        self.tokens.cache_write += t.cache_write.unwrap_or(0);
        if t.total().is_none() {
            self.runs_tokens_incomplete += 1;
        }
        match u.cost_usd {
            Some(c) if c.is_finite() && c >= 0.0 && (self.cost_usd + c).is_finite() => self.cost_usd += c,
            _ => self.runs_cost_unknown += 1,
        }
        for p in &u.pricing_sources {
            if !self.pricing_sources.contains(p) {
                self.pricing_sources.push(p.clone());
            }
        }
    }

    /// Count one run's estimate-vs-actual (`#168`). `estimate` is the run's
    /// own `original_estimate`; `terminal_wall_seconds` is its wall time
    /// only when the run is terminal (`None` for one still going, which has
    /// no actual yet). A run with no estimate touches neither counter --
    /// see the fields' own doc comments for why.
    fn add_estimate(&mut self, estimate: Option<&crate::task::Estimate>, terminal_wall_seconds: Option<u64>) {
        let Some(estimate) = estimate else { return };
        self.estimated_runs += 1;
        if let Some(wall) = terminal_wall_seconds {
            if wall >= estimate.time.low && wall <= estimate.time.high {
                self.within_range += 1;
            }
        }
    }

    /// Runs whose cost is fully in `cost_usd`.
    fn runs_costed(&self) -> u32 {
        self.runs - self.runs_unknown - self.runs_cost_unknown
    }
}

/// What `Engine::spend` reads (#164): runs that started in `[from, to)`
/// (`from` defaults to thirty days before `to`, `to` to now), narrowed to
/// `scope`'s own subtree when given, summed per `group_by`. Plain serde
/// data -- the same shape whether it comes off the wire (`Request::Costs`)
/// or is built in-process (`cost_week`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpendQuery {
    #[serde(default)]
    pub basis: factory_kernel::SpendBasis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<DateTime<Utc>>,
    #[serde(default)]
    pub group_by: CostGroupBy,
}

/// Ordering is read-side behavior, not L0 vocabulary.
pub trait CostReportExt { fn sort_rows(rows: &mut [CostRow]); }

impl CostReportExt for CostReport {
    /// Order rows most expensive first, ties by key, so the list reads the
    /// same on every call.
    fn sort_rows(rows: &mut [CostRow]) {
        rows.sort_by(|a, b| {
            b.cost_usd
                .partial_cmp(&a.cost_usd)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.tokens.total().cmp(&a.tokens.total()))
                .then_with(|| a.key.cmp(&b.key))
        });
    }
}

/// One task's usage: every run's, and the sum.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskUsage {
    pub task_id: String,
    pub total: CostRow,
    /// Every run's snapshotted original estimate, summed range by range --
    /// what `time_comparison` and `cost_comparison` compare the summed
    /// actuals with. `None` when a run has none; `cost` is `None` when a run
    /// has no cost range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_estimate: Option<crate::task::Estimate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_wall_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_active_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_comparison: Option<EstimateComparison<u64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_comparison: Option<EstimateComparison<f64>>,
    /// Newest first, like `task.runs`.
    pub runs: Vec<RunUsageEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunUsageEntry {
    pub run_id: String,
    pub attempt: u32,
    pub status: crate::run::RunStatus,
    /// Wall-clock seconds from start to end (or to now, while running).
    pub wall_seconds: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_estimate: Option<crate::task::Estimate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub re_estimate: Option<ReEstimate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_comparison: Option<EstimateComparison<u64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_comparison: Option<EstimateComparison<f64>>,
    pub usage: RunUsage,
}

// -- the two registry metrics (`unit_cost`, `tokens_per_run`) ---------------

/// One cost metric's value over a window, and what it rests on.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageFigure {
    pub value: Option<f64>,
    /// Why there is no value; `None` whenever there is one.
    pub reason: Option<String>,
    /// The newest run end the value rests on.
    pub as_of: Option<DateTime<Utc>>,
}

/// A run whose usage counts toward a metric: it finished, and its usage was
/// measured start to end. A partial reading is a lower bound, and a lower
/// bound averaged in would make every figure look cheaper than it was.
fn measured(run: &crate::run::Run) -> Option<&RunUsage> {
    run.usage.as_ref().filter(|u| u.is_known() && !u.partial)
}

/// `tokens_per_run`, `unit_cost` and `estimate_accuracy` (`#168`) over the
/// runs that ended in `(to - days, to]`. `None` for any other id.
///
/// * `tokens_per_run`: the mean of every token type summed, over finished
///   runs whose usage was fully measured.
/// * `unit_cost`: what those runs cost -- failed and cancelled ones
///   included, since scrap is part of what a finished unit costs -- over
///   how many of them ended `done`. Only runs with a measured cost are in
///   either side of the division.
/// * `estimate_accuracy`: over finished runs that carry an
///   `original_estimate` -- the estimate the run was dispatched with,
///   whatever ended it -- the share whose terminal wall time
///   (`ended_at - started_at`) fell within `[time.low, time.high]`. A run
///   with no `original_estimate` (every run from before `#117`, or one
///   whose task had none) is left out of the denominator, never scored as a
///   miss.
///
/// Runs whose usage is unknown are left out of the first two, never counted
/// as zero; with none left there is no value, and the reason says how many
/// finished runs there were. `estimate_accuracy` follows the same rule for
/// runs with no estimate to compare against.
pub fn usage_metric(id: &str, runs: &[crate::run::Run], to: DateTime<Utc>, days: i64) -> Option<UsageFigure> {
    let from = to - chrono::Duration::days(days);
    usage_metric_in_window(id, runs, from, to)
}

/// The L4 spend port calls this with its exact finished-run query window.
pub fn usage_metric_in_window(id: &str, runs: &[crate::run::Run], from: DateTime<Utc>, to: DateTime<Utc>) -> Option<UsageFigure> {
    let span = to - from;
    let window = if span.num_seconds() % 86400 == 0 {
        format!("the trailing {} days", span.num_days())
    } else { format!("the window {from} through {to}") };
    let finished: Vec<&crate::run::Run> = runs
        .iter()
        .filter(|r| r.status.is_terminal() && r.ended_at.is_some_and(|e| e > from && e <= to))
        .collect();
    let none = |what: &str| UsageFigure {
        value: None,
        reason: Some(if finished.is_empty() {
            format!("no run finished in {window}")
        } else {
            format!(
                "none of the {} runs that finished in {window} has {what}",
                finished.len()
            )
        }),
        as_of: None,
    };
    let newest = |rs: &[&crate::run::Run]| rs.iter().filter_map(|r| r.ended_at).max();
    match id {
        "tokens_per_run" => {
            let counted: Vec<(&crate::run::Run, u64)> = finished
                .iter()
                .filter_map(|r| measured(r).and_then(|u| u.tokens.total()).map(|t| (*r, t)))
                .collect();
            if counted.is_empty() {
                return Some(none("fully measured token usage"));
            }
            let sum: u128 = counted.iter().map(|(_, t)| u128::from(*t)).sum();
            let rs: Vec<&crate::run::Run> = counted.iter().map(|(r, _)| *r).collect();
            Some(UsageFigure {
                value: Some(sum as f64 / counted.len() as f64),
                reason: None,
                as_of: newest(&rs),
            })
        }
        "unit_cost" => {
            let costed: Vec<(&crate::run::Run, f64)> = finished
                .iter()
                .filter_map(|r| measured(r).and_then(|u| u.cost_usd).filter(|c| c.is_finite() && *c >= 0.0).map(|c| (*r, c)))
                .collect();
            let done = costed
                .iter()
                .filter(|(r, _)| r.status == crate::run::RunStatus::Done)
                .count();
            if done == 0 {
                return Some(none("a measured cost and ended done"));
            }
            let spent: f64 = costed.iter().map(|(_, c)| c).sum();
            let rs: Vec<&crate::run::Run> = costed.iter().map(|(r, _)| *r).collect();
            Some(UsageFigure {
                value: Some(spent / done as f64),
                reason: None,
                as_of: newest(&rs),
            })
        }
        "estimate_accuracy" => {
            let estimated: Vec<(&crate::run::Run, bool)> = finished
                .iter()
                .filter_map(|r| {
                    let estimate = r.original_estimate.as_ref()?;
                    let wall = (r.ended_at.unwrap_or(r.started_at) - r.started_at).num_seconds().max(0) as u64;
                    Some((*r, wall >= estimate.time.low && wall <= estimate.time.high))
                })
                .collect();
            if estimated.is_empty() {
                return Some(none("an original estimate"));
            }
            let hits = estimated.iter().filter(|(_, within_range)| *within_range).count();
            let rs: Vec<&crate::run::Run> = estimated.iter().map(|(r, _)| *r).collect();
            Some(UsageFigure {
                value: Some(hits as f64 / estimated.len() as f64),
                reason: None,
                as_of: newest(&rs),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(min: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 25, 12, 0, 0).unwrap() + chrono::Duration::minutes(min)
    }

    fn fixture(name: &str) -> SessionUsage {
        let path = format!("{}/tests/fixtures/usage/{name}", env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        SessionUsage::parse(&text).unwrap()
    }

    fn snap(point: SnapshotPoint, min: i64, usage: Option<SessionUsage>) -> UsageSnapshot {
        let unknown = usage.is_none().then(|| "the plugin timed out".to_string());
        UsageSnapshot {
            run_id: "r1".into(),
            task_id: "t1".into(),
            point,
            at: at(min),
            runtime: "herdr".into(),
            usage,
            unknown,
        }
    }

    fn session(id: &str, input: Option<u64>, output: u64, usd: Option<f64>) -> HarnessUsage {
        HarnessUsage {
            session_id: id.into(),
            adapter: Some("claude-code".into()),
            model: Some("claude-opus-5".into()),
            tokens: TokenCounts {
                input,
                output: Some(output),
                cache_read: Some(0),
                cache_write: Some(0),
            },
            cost: UsageCost {
                usd,
                pricing_source: Some("litellm@2026-09-01".into()),
            },
            elapsed_seconds: None,
            active_seconds: None,
            subagents: Vec::new(),
            rate_limit: None,
            unavailable: BTreeMap::new(),
        }
    }

    fn usage(sessions: Vec<HarnessUsage>) -> SessionUsage {
        SessionUsage {
            schema: 1,
            handle: Some("w1:p1".into()),
            sampled_at: None,
            sessions,
        }
    }

    #[test]
    fn the_contract_fixture_parses_and_drops_everything_but_usage_fields() {
        let u = fixture("claude-run-end.json");
        assert_eq!(u.handle.as_deref(), Some("w3:p1"));
        assert_eq!(u.sessions.len(), 1);
        let s = &u.sessions[0];
        assert_eq!(s.tokens.input, Some(120_000));
        assert_eq!(s.tokens.cache_write, None, "null on the wire stays unknown");
        assert_eq!(
            s.unavailable.get("tokens.cache_write").map(String::as_str),
            Some("not reported by this harness version")
        );
        assert_eq!(s.subagents.len(), 1);
        assert_eq!(s.rate_limit.as_ref().unwrap().windows[0].used_percent, Some(14.0));
        // The fixture carries a `last_assistant_text` a plugin must never
        // send; parsed and re-serialized, it is gone.
        let stored = serde_json::to_string(&u).unwrap();
        assert!(!stored.contains("last_assistant_text"));
        assert!(!stored.contains("secret plan"));
    }

    #[test]
    fn an_unknown_schema_is_refused_not_guessed_at() {
        let err = SessionUsage::parse(r#"{"schema":2,"sessions":[]}"#).unwrap_err();
        assert!(err.contains("schema 2"), "{err}");
        assert!(SessionUsage::parse(r#"{"sessions":[]}"#).is_err());
        assert!(SessionUsage::parse("not json").is_err());
    }

    #[test]
    fn a_run_is_the_difference_between_its_baseline_and_its_end() {
        let snaps = vec![
            snap(SnapshotPoint::Dispatch, 0, Some(fixture("claude-dispatch.json"))),
            snap(SnapshotPoint::RunEnd, 30, Some(fixture("claude-run-end.json"))),
        ];
        let u = run_usage(&snaps);
        assert_eq!(u.state, UsageState::Known);
        assert!(!u.partial, "{:?}", u.notes);
        // The pane was reused: the baseline's 20k input were a previous
        // run's, and the subagent is new since then.
        assert_eq!(u.tokens.input, Some(100_000 + 8_000));
        assert_eq!(u.tokens.output, Some(9_000 + 1_000));
        assert_eq!(u.tokens.cache_read, Some(400_000));
        assert_eq!(u.tokens.cache_write, None, "one null makes the total unknown");
        assert!(u.notes.iter().any(|n| n.contains("tokens.cache_write") && n.contains("not reported")));
        let cost = u.cost_usd.unwrap();
        assert!((cost - (2.70 - 0.50 + 0.20)).abs() < 1e-9, "{cost}");
        assert_eq!(u.pricing_sources, vec!["litellm@2026-09-01".to_string()]);
        assert_eq!(u.models, vec!["claude-opus-5".to_string()]);
        assert_eq!(u.sessions, 2);
        assert_eq!(u.elapsed_seconds, Some(1800.0));
        assert_eq!(u.active_seconds, None);
        assert_eq!(u.as_of_point, Some(SnapshotPoint::RunEnd));
    }

    #[test]
    fn a_turn_end_stored_after_the_run_end_does_not_become_the_latest() {
        let snaps = vec![
            snap(SnapshotPoint::Dispatch, 0, Some(usage(vec![]))),
            snap(SnapshotPoint::RunEnd, 9, Some(usage(vec![session("a", Some(30), 7, Some(0.3))]))),
            snap(SnapshotPoint::TurnEnded, 5, Some(usage(vec![session("a", Some(10), 5, Some(0.1))]))),
        ];
        let u = run_usage(&snaps);
        assert_eq!(u.as_of_point, Some(SnapshotPoint::RunEnd));
        assert_eq!(u.tokens.input, Some(30));
    }

    #[test]
    fn a_session_born_after_the_baseline_counts_whole() {
        let snaps = vec![
            snap(SnapshotPoint::Dispatch, 0, Some(usage(vec![]))),
            snap(SnapshotPoint::TurnEnded, 5, Some(usage(vec![session("a", Some(10), 5, Some(0.1))]))),
            snap(SnapshotPoint::RunEnd, 9, Some(usage(vec![session("a", Some(30), 7, Some(0.3))]))),
        ];
        let u = run_usage(&snaps);
        assert_eq!(u.tokens.input, Some(30));
        assert_eq!(u.tokens.total(), Some(37));
        assert_eq!(u.cost_usd, Some(0.3));
    }

    // -- #178: a resumed session's baseline can fall back to the previous
    // run's own `RunEnd` reading -----------------------------------------

    #[test]
    fn a_resumed_session_missing_from_the_dispatch_baseline_is_measured_from_the_prior_run_end() {
        let snaps = vec![
            snap(SnapshotPoint::Dispatch, 0, Some(usage(vec![]))),
            snap(SnapshotPoint::RunEnd, 9, Some(usage(vec![session("a", Some(30), 7, Some(0.3))]))),
        ];
        // Without a prior, `run_usage` treats "a" as born in this run --
        // the whole reading is credited to it (same as the test above).
        let fresh = run_usage(&snaps);
        assert_eq!(fresh.tokens.input, Some(30));

        // With the previous run's own `RunEnd` reading for "a" as the
        // prior, only the growth since then is this run's.
        let prior = session("a", Some(10), 5, Some(0.1));
        let resumed = run_usage_with_prior(&snaps, Some(("a", &prior)));
        assert_eq!(resumed.tokens.input, Some(20));
        assert_eq!(resumed.tokens.output, Some(2));
        assert_eq!(resumed.tokens.total(), Some(22));
        assert!((resumed.cost_usd.unwrap() - 0.2).abs() < 1e-9, "{:?}", resumed.cost_usd);
    }

    #[test]
    fn a_dispatch_snapshot_that_already_saw_the_resumed_session_keeps_its_own_baseline() {
        let snaps = vec![
            snap(SnapshotPoint::Dispatch, 0, Some(usage(vec![session("a", Some(5), 1, Some(0.05))]))),
            snap(SnapshotPoint::RunEnd, 9, Some(usage(vec![session("a", Some(30), 7, Some(0.3))]))),
        ];
        // Wildly different from the real baseline -- proof it is ignored
        // once the run's own `Dispatch` snapshot already answered for "a".
        let prior = session("a", Some(1000), 1000, Some(1000.0));
        let u = run_usage_with_prior(&snaps, Some(("a", &prior)));
        assert_eq!(u.tokens.input, Some(25), "30 - 5, the run's own baseline, not 30 - 1000");
    }

    #[test]
    fn newest_session_id_for_adapter_matches_by_adapter_and_prefers_the_newest_snapshot() {
        let mut old_codex = session("old-codex", Some(1), 1, None);
        old_codex.adapter = Some("codex".into());
        let mut claude = session("the-claude-one", Some(1), 1, None);
        claude.adapter = Some("claude-code".into());
        let mut new_codex = session("new-codex", Some(1), 1, None);
        new_codex.adapter = Some("codex".into());
        let snaps = vec![
            snap(SnapshotPoint::Dispatch, 0, Some(usage(vec![old_codex]))),
            snap(SnapshotPoint::TurnEnded, 5, Some(usage(vec![claude]))),
            snap(SnapshotPoint::RunEnd, 9, Some(usage(vec![new_codex]))),
        ];
        assert_eq!(newest_session_id_for_adapter(&snaps, "codex"), Some("new-codex".into()));
        assert_eq!(newest_session_id_for_adapter(&snaps, "claude-code"), Some("the-claude-one".into()));
        assert_eq!(newest_session_id_for_adapter(&snaps, "pi"), None, "never seen running as pi");
    }

    #[test]
    fn no_baseline_or_no_answer_is_unknown_with_the_reason() {
        let u = run_usage(&[]);
        assert_eq!(u.state, UsageState::Unknown);
        assert!(u.reason.unwrap().contains("no usage was recorded"));

        let u = run_usage(&[snap(SnapshotPoint::Dispatch, 0, None), snap(SnapshotPoint::RunEnd, 3, Some(usage(vec![])))]);
        assert_eq!(u.state, UsageState::Unknown);
        assert!(u.reason.unwrap().contains("timed out"));

        let u = run_usage(&[snap(SnapshotPoint::RunEnd, 3, Some(usage(vec![session("a", Some(1), 1, Some(0.0))])))]);
        assert!(u.reason.unwrap().contains("no baseline"));

        let u = run_usage(&[snap(SnapshotPoint::Dispatch, 0, Some(usage(vec![])))]);
        assert!(u.reason.unwrap().contains("nothing measured since dispatch"));
    }

    #[test]
    fn a_pane_with_no_harness_session_is_unknown_not_zero() {
        let u = run_usage(&[
            snap(SnapshotPoint::Dispatch, 0, Some(usage(vec![]))),
            snap(SnapshotPoint::RunEnd, 3, Some(usage(vec![]))),
        ]);
        assert_eq!(u.state, UsageState::Unknown);
        assert_eq!(u.cost_usd, None);
    }

    #[test]
    fn a_failed_end_snapshot_leaves_the_last_turn_as_a_lower_bound() {
        let u = run_usage(&[
            snap(SnapshotPoint::Dispatch, 0, Some(usage(vec![]))),
            snap(SnapshotPoint::TurnEnded, 5, Some(usage(vec![session("a", Some(10), 5, Some(0.1))]))),
            snap(SnapshotPoint::RunEnd, 9, None),
        ]);
        assert_eq!(u.state, UsageState::Known);
        assert!(u.partial);
        assert_eq!(u.as_of_point, Some(SnapshotPoint::TurnEnded));
        assert!(u.notes[0].contains("run_end"), "{:?}", u.notes);
    }

    #[test]
    fn a_counter_that_went_backwards_is_unknown_and_a_vanished_session_partial() {
        let u = run_usage(&[
            snap(
                SnapshotPoint::Dispatch,
                0,
                Some(usage(vec![session("a", Some(50), 5, Some(0.5)), session("b", Some(1), 1, Some(0.0))])),
            ),
            snap(SnapshotPoint::RunEnd, 9, Some(usage(vec![session("a", Some(40), 9, Some(0.7))]))),
        ]);
        assert_eq!(u.tokens.input, None);
        assert_eq!(u.tokens.output, Some(4));
        assert!(u.partial);
        assert!(u.notes.iter().any(|n| n.contains("session b")));
        assert!(u.notes.iter().any(|n| n.contains("went backwards")));
    }

    #[test]
    fn a_row_sums_what_is_known_and_counts_what_is_not() {
        let known = run_usage(&[
            snap(SnapshotPoint::Dispatch, 0, Some(usage(vec![]))),
            snap(SnapshotPoint::RunEnd, 9, Some(usage(vec![session("a", Some(10), 5, Some(0.25))]))),
        ]);
        let no_cost = run_usage(&[
            snap(SnapshotPoint::Dispatch, 0, Some(usage(vec![]))),
            snap(SnapshotPoint::RunEnd, 9, Some(usage(vec![session("a", None, 5, None)]))),
        ]);
        let mut row = CostRow::new("t1", None);
        row.add(Some(&known));
        row.add(Some(&no_cost));
        row.add(Some(&RunUsage::unknown("x", 0)));
        row.add(None);
        assert_eq!(row.runs, 4);
        assert_eq!(row.runs_unknown, 2);
        assert_eq!(row.runs_cost_unknown, 1);
        assert_eq!(row.runs_tokens_incomplete, 1);
        assert_eq!(row.runs_costed(), 1);
        assert_eq!(row.cost_usd, 0.25);
        assert_eq!(row.tokens.output, 10);
        assert_eq!(row.tokens.input, 10);
    }

    #[test]
    fn add_estimate_counts_only_runs_with_one_and_only_terminal_ones_can_be_within_range() {
        let estimate = |low: u64, high: u64| crate::task::Estimate {
            time: crate::task::TimeEstimateRange { low, expected: (low + high) / 2, high },
            cost: None,
        };
        let mut row = CostRow::new("t1", None);
        row.add_estimate(Some(&estimate(0, 100)), Some(50)); // within
        row.add_estimate(Some(&estimate(0, 10)), Some(50)); // outside
        row.add_estimate(Some(&estimate(0, 100)), None); // still running: counted, not scored
        row.add_estimate(None, Some(50)); // no estimate at all: not counted either way
        assert_eq!(row.estimated_runs, 3);
        assert_eq!(row.within_range, 1);
    }

    fn finished_run(id: &str, status: crate::run::RunStatus, ended_min: i64, usage: Option<RunUsage>) -> crate::run::Run {
        let mut run: crate::run::Run = serde_json::from_value(serde_json::json!({
            "id": id, "task_id": "t", "attempt": 1, "status": status.as_str(), "trigger": "manual",
            "agent": "builder", "runtime": "herdr", "started_at": at(ended_min - 10), "ended_at": at(ended_min)
        }))
        .unwrap();
        run.usage = usage;
        run
    }

    fn measured_usage(total_in: u64, usd: Option<f64>) -> RunUsage {
        run_usage(&[
            snap(SnapshotPoint::Dispatch, 0, Some(usage(vec![]))),
            snap(SnapshotPoint::RunEnd, 9, Some(usage(vec![session("a", Some(total_in), 0, usd)]))),
        ])
    }

    #[test]
    fn unit_cost_spreads_scrap_over_what_got_done_and_leaves_the_unmeasured_out() {
        use crate::run::RunStatus::{Done, Failed};
        let runs = vec![
            finished_run("a", Done, 0, Some(measured_usage(100, Some(1.0)))),
            finished_run("b", Failed, 1, Some(measured_usage(300, Some(0.5)))),
            finished_run("c", Done, 2, Some(measured_usage(200, Some(1.5)))),
            // Unknown and partial usage is in neither side of either sum.
            finished_run("d", Done, 3, Some(RunUsage::unknown("no plugin", 2))),
            finished_run("e", Done, 4, None),
        ];
        let now = at(10);
        let cost = usage_metric("unit_cost", &runs, now, 28).unwrap();
        assert_eq!(cost.value, Some((1.0 + 0.5 + 1.5) / 2.0));
        assert_eq!(cost.as_of, Some(at(2)));
        let tokens = usage_metric("tokens_per_run", &runs, now, 28).unwrap();
        assert_eq!(tokens.value, Some(200.0));
        assert!(usage_metric("throughput_week", &runs, now, 28).is_none());
    }

    #[test]
    fn with_nothing_measured_a_cost_metric_is_none_with_the_reason() {
        let runs = vec![finished_run("d", crate::run::RunStatus::Done, 3, Some(RunUsage::unknown("x", 1)))];
        let cost = usage_metric("unit_cost", &runs, at(10), 28).unwrap();
        assert_eq!(cost.value, None);
        assert!(cost.reason.unwrap().contains("none of the 1 runs"));
        let empty = usage_metric("tokens_per_run", &[], at(10), 28).unwrap();
        assert!(empty.reason.unwrap().contains("no run finished"));
    }

    /// `finished_run`'s own run, given an `original_estimate` -- `#168`'s
    /// `estimate_accuracy` reads it straight off the run, never the task.
    fn estimated(run: crate::run::Run, low: u64, high: u64) -> crate::run::Run {
        let mut run = run;
        run.original_estimate =
            Some(crate::task::Estimate { time: crate::task::TimeEstimateRange { low, expected: (low + high) / 2, high }, cost: None });
        run
    }

    #[test]
    fn estimate_accuracy_is_the_share_of_estimated_runs_inside_their_own_range() {
        use crate::run::RunStatus::{Done, Failed};
        // Every `finished_run` here has a 10-minute (600s) wall time.
        let runs = vec![
            estimated(finished_run("a", Done, 0, None), 0, 700), // within
            estimated(finished_run("b", Done, 1, None), 0, 100), // outside -- too slow
            // A failed run still counts: it ended, and the estimate was for
            // wall time, not success.
            estimated(finished_run("c", Failed, 2, None), 500, 700), // within
            finished_run("d", Done, 3, None),                        // no estimate at all -- excluded
        ];
        let figure = usage_metric("estimate_accuracy", &runs, at(10), 28).unwrap();
        assert_eq!(figure.value, Some(2.0 / 3.0));
        assert_eq!(figure.as_of, Some(at(2)), "the newest estimated run behind the value, not run d");
    }

    #[test]
    fn estimate_accuracy_with_no_estimated_runs_is_unavailable_not_zero() {
        let runs = vec![finished_run("a", crate::run::RunStatus::Done, 0, None)];
        let figure = usage_metric("estimate_accuracy", &runs, at(10), 28).unwrap();
        assert_eq!(figure.value, None);
        assert!(figure.reason.as_deref().unwrap_or_default().contains("an original estimate"));

        let empty = usage_metric("estimate_accuracy", &[], at(10), 28).unwrap();
        assert!(empty.reason.unwrap().contains("no run finished"));
    }

    #[test]
    fn group_by_round_trips() {
        for g in [
            CostGroupBy::Task,
            CostGroupBy::Issue,
            CostGroupBy::Scope,
            CostGroupBy::Agent,
            CostGroupBy::Provider,
            CostGroupBy::Workflow,
        ] {
            assert_eq!(g.as_str().parse::<CostGroupBy>().unwrap(), g);
            assert_eq!(serde_json::to_value(g).unwrap(), serde_json::json!(g.as_str()));
        }
        assert!("bogus".parse::<CostGroupBy>().is_err());
    }

    #[test]
    fn a_cost_row_from_before_168_still_deserialises_with_nothing_estimated() {
        let json = serde_json::json!({
            "key": "t1", "runs": 2, "runs_unknown": 0, "runs_partial": 0,
            "tokens": {"input": 0, "output": 0, "cache_read": 0, "cache_write": 0},
            "runs_tokens_incomplete": 0, "cost_usd": 1.5, "runs_cost_unknown": 0,
        });
        let row: CostRow = serde_json::from_value(json).unwrap();
        assert_eq!(row.estimated_runs, 0);
        assert_eq!(row.within_range, 0);
        assert_eq!(row.median_actual_over_expected, None);
    }

    fn plan_run(id: &str, ended: Option<i64>, provider: Option<&str>) -> crate::run::Run {
        let mut run: crate::run::Run = serde_json::from_value(serde_json::json!({
            "id": id, "task_id": id, "attempt": 1, "status": if ended.is_some() { "done" } else { "running" },
            "trigger": "manual", "agent": "builder", "runtime": "herdr", "started_at": at(-1),
            "ended_at": ended.map(at)
        }))
        .unwrap();
        run.provider_account = provider.map(str::to_string);
        run
    }

    fn plan_snapshot(
        run: &str,
        minute: i64,
        tokens: u64,
        window: Option<(u32, &str, f64)>,
    ) -> UsageSnapshot {
        let rate_limit = window.map(|(window_minutes, resets_at, used_percent)| RateLimit {
            provider: Some("anthropic".into()),
            plan_type: Some("max".into()),
            attribution_quality: Some("confirmed".into()),
            windows: vec![RateLimitWindow {
                window_minutes: Some(window_minutes),
                used_percent: Some(used_percent),
                resets_at: Some(resets_at.into()),
            }],
        });
        UsageSnapshot {
            run_id: run.into(),
            task_id: run.into(),
            point: if minute == 0 { SnapshotPoint::Dispatch } else { SnapshotPoint::TurnEnded },
            at: at(minute),
            runtime: "herdr".into(),
            usage: Some(SessionUsage {
                schema: USAGE_SCHEMA,
                handle: None,
                sampled_at: Some(at(minute)),
                sessions: vec![HarnessUsage {
                    session_id: run.into(),
                    adapter: Some("claude-code".into()),
                    model: None,
                    tokens: TokenCounts {
                        input: Some(tokens),
                        output: Some(0),
                        cache_read: Some(0),
                        cache_write: Some(0),
                    },
                    cost: UsageCost::default(),
                    elapsed_seconds: None,
                    active_seconds: None,
                    subagents: Vec::new(),
                    rate_limit,
                    unavailable: BTreeMap::new(),
                }],
            }),
            unknown: None,
        }
    }

    #[test]
    fn overlapping_runs_split_positive_window_growth_by_tokens_and_keep_resets_apart() {
        let runs = vec![plan_run("r1", None, Some("claude-max")), plan_run("r2", Some(10), Some("claude-max"))];
        let snapshots = BTreeMap::from([
            (
                "r1".into(),
                vec![
                    plan_snapshot("r1", 0, 0, Some((300, "reset-a", 10.0))),
                    plan_snapshot("r1", 10, 100, Some((300, "reset-a", 14.0))),
                    plan_snapshot("r1", 11, 100, Some((10_080, "reset-b", 20.0))),
                    plan_snapshot("r1", 20, 200, Some((10_080, "reset-b", 21.0))),
                ],
            ),
            (
                "r2".into(),
                vec![plan_snapshot("r2", 0, 0, None), plan_snapshot("r2", 10, 300, None)],
            ),
        ]);
        let allocation = allocate_plan_share(&runs, &snapshots);
        let r1 = &allocation.shares["r1"];
        let r2 = &allocation.shares["r2"];
        assert!((r1.iter().find(|share| share.resets_at == "reset-a").unwrap().used_percent - 1.0).abs() < 1e-9);
        assert!((r2[0].used_percent - 3.0).abs() < 1e-9);
        assert_eq!(r2[0].attribution, PlanShareAttribution::Apportioned);
        let reset = r1.iter().find(|share| share.resets_at == "reset-b").unwrap();
        assert!((reset.used_percent - 1.0).abs() < 1e-9);
        assert_eq!(reset.attribution, PlanShareAttribution::Direct);
    }

    #[test]
    fn measured_zero_consumers_do_not_discard_positive_allocation() {
        let mut newly_started = plan_run("new", None, Some("claude-max"));
        newly_started.started_at = at(5);
        let mut new_baseline = plan_snapshot("new", 5, 0, None);
        new_baseline.point = SnapshotPoint::Dispatch;
        let runs = vec![
            plan_run("consumer", None, Some("claude-max")),
            plan_run("idle", None, Some("claude-max")),
            newly_started,
        ];
        let snapshots = BTreeMap::from([
            ("consumer".into(), vec![
                plan_snapshot("consumer", 0, 0, Some((300, "reset-a", 10.0))),
                plan_snapshot("consumer", 10, 100, Some((300, "reset-a", 14.0))),
            ]),
            ("idle".into(), vec![plan_snapshot("idle", 0, 0, None), plan_snapshot("idle", 10, 0, None)]),
            ("new".into(), vec![new_baseline, plan_snapshot("new", 10, 0, None)]),
        ]);

        let allocation = allocate_plan_share(&runs, &snapshots);
        let shares = &allocation.shares["consumer"];
        assert_eq!(shares.len(), 1);
        assert!((shares[0].used_percent - 4.0).abs() < 1e-9);
        assert_eq!(shares[0].attribution, PlanShareAttribution::Direct);
        assert!(!allocation.unknown.contains_key("consumer"));
        assert!(!allocation.unknown.contains_key("idle"));
        assert!(!allocation.unknown.contains_key("new"));
        let allocated: f64 = allocation.shares.values().flatten().map(|share| share.used_percent).sum();
        assert!((allocated - 4.0).abs() < 1e-9, "the observed provider delta is preserved");
    }

    #[test]
    fn staggered_candidate_readings_never_weight_a_later_provider_interval() {
        let runs = vec![
            plan_run("observer", None, Some("claude-max")),
            plan_run("staggered", None, Some("claude-max")),
        ];
        let snapshots = BTreeMap::from([
            ("observer".into(), vec![
                plan_snapshot("observer", 5, 0, Some((300, "reset-a", 10.0))),
                plan_snapshot("observer", 10, 50, Some((300, "reset-a", 14.0))),
            ]),
            ("staggered".into(), vec![
                plan_snapshot("staggered", 0, 0, None),
                // These tokens were consumed before the provider interval
                // began at t5. The t10 reading must not make them a weight
                // for the t5--t10 provider delta.
                plan_snapshot("staggered", 4, 100, None),
                plan_snapshot("staggered", 10, 100, None),
            ]),
        ]);

        let allocation = allocate_plan_share(&runs, &snapshots);
        assert!(allocation.shares.is_empty(), "unaligned token growth is never allocated");
        for run in ["observer", "staggered"] {
            let gap = &allocation.unknown[run][0];
            assert_eq!(gap.from, Some(at(5)));
            assert_eq!(gap.to, Some(at(10)));
            assert!(gap.reason.contains("no measured token increment"));
        }
    }

    #[test]
    fn a_measured_zero_provider_delta_resolves_active_consumers() {
        let run = plan_run("idle", None, Some("claude-max"));
        let snapshots = BTreeMap::from([("idle".into(), vec![
            plan_snapshot("idle", 0, 0, Some((300, "reset-a", 10.0))),
            plan_snapshot("idle", 10, 100, Some((300, "reset-a", 10.0))),
        ])]);

        let allocation = allocate_plan_share(&[run], &snapshots);
        assert!(allocation.shares.is_empty(), "zero provider growth invents no share");
        assert!(allocation.unknown.is_empty(), "two equal readings are measured zero evidence");

        let missing = allocate_plan_share(
            &[plan_run("missing", None, Some("claude-max"))],
            &BTreeMap::from([("missing".into(), vec![
                plan_snapshot("missing", 10, 100, Some((300, "reset-a", 10.0))),
            ])]),
        );
        assert!(missing.unknown["missing"][0].reason.contains("no earlier observation"));
    }

    #[test]
    fn plan_share_keeps_missing_bindings_and_all_zero_weights_unknown() {
        let runs = vec![plan_run("unbound", None, None), plan_run("zero", None, Some("claude-max"))];
        let snapshots = BTreeMap::from([(
            "zero".into(),
            vec![
                plan_snapshot("zero", 0, 0, Some((300, "reset-a", 1.0))),
                plan_snapshot("zero", 10, 0, Some((300, "reset-a", 2.0))),
            ],
        )]);
        let allocation = allocate_plan_share(&runs, &snapshots);
        assert!(allocation.unknown["unbound"].iter().any(|gap| gap.reason.contains("binding")));
        assert!(allocation.unknown["zero"].iter().any(|gap| gap.reason.contains("token increment")));
        assert!(allocation.shares.is_empty());
    }

    #[test]
    fn a_later_successful_interval_does_not_erase_an_earlier_gap() {
        let runs = vec![
            plan_run("consumer", None, Some("claude-max")),
            plan_run("missing", Some(10), Some("claude-max")),
        ];
        let snapshots = BTreeMap::from([("consumer".into(), vec![
            plan_snapshot("consumer", 0, 0, Some((300, "reset-a", 10.0))),
            plan_snapshot("consumer", 10, 100, Some((300, "reset-a", 12.0))),
            plan_snapshot("consumer", 20, 200, Some((300, "reset-a", 14.0))),
        ])]);

        let allocation = allocate_plan_share(&runs, &snapshots);
        assert!((allocation.shares["consumer"][0].used_percent - 2.0).abs() < 1e-9);
        let gaps = &allocation.unknown["consumer"];
        assert_eq!(gaps.len(), 1, "the first interval remains unresolved");
        assert_eq!(gaps[0].from, Some(at(0)));
        assert_eq!(gaps[0].to, Some(at(10)));
        assert_eq!(gaps[0].window_minutes, Some(300));
    }

    /// `plan_snapshot`, asked for at `requested` but sampled by the
    /// runtime at `sampled`.
    fn sampled_snapshot(
        run: &str,
        requested: i64,
        sampled: i64,
        tokens: u64,
        window: Option<(u32, &str, f64)>,
    ) -> UsageSnapshot {
        let mut snapshot = plan_snapshot(run, sampled, tokens, window);
        snapshot.at = at(requested);
        snapshot
    }

    #[test]
    fn a_delayed_answer_is_attributed_over_its_sample_interval_not_its_request_interval() {
        // The observer's runtime was asked at t5 and t10 but sampled at t2
        // and t6. `late` started at t7 -- inside the request interval, after
        // the sample interval -- so none of the t2..t6 growth is its.
        let mut late = plan_run("late", None, Some("claude-max"));
        late.started_at = at(7);
        let mut late_baseline = plan_snapshot("late", 7, 0, None);
        late_baseline.point = SnapshotPoint::Dispatch;
        let runs = vec![plan_run("observer", None, Some("claude-max")), late];
        let snapshots = BTreeMap::from([
            ("observer".into(), vec![
                sampled_snapshot("observer", 5, 2, 0, Some((300, "reset-a", 10.0))),
                sampled_snapshot("observer", 10, 6, 100, Some((300, "reset-a", 14.0))),
            ]),
            ("late".into(), vec![late_baseline, plan_snapshot("late", 10, 50, None)]),
        ]);

        let allocation = allocate_plan_share(&runs, &snapshots);
        let share = &allocation.shares["observer"][0];
        assert!((share.used_percent - 4.0).abs() < 1e-9, "{share:?}");
        assert_eq!(share.attribution, PlanShareAttribution::Direct);
        assert_eq!(share.as_of, at(6), "as of the sample, not the request");
        assert!(!allocation.shares.contains_key("late"));
        assert_eq!(allocation.intervals.len(), 1);
        let interval = &allocation.intervals[0];
        assert_eq!((interval.from, interval.to), (at(2), at(6)));
        assert_eq!(interval.consumers, vec!["observer".to_string()]);
        assert_eq!(interval.attribution, Some(PlanShareAttribution::Direct));
    }

    #[test]
    fn a_cached_answer_served_twice_is_one_observation_not_an_interval() {
        // Asked at t5 and t10, the runtime answered both times with the
        // reading it sampled at t2: there is no second observation to take
        // a difference from, so nothing is allocated and the gap is named.
        let runs = vec![plan_run("cached", None, Some("claude-max"))];
        let snapshots = BTreeMap::from([("cached".into(), vec![
            sampled_snapshot("cached", 5, 2, 0, Some((300, "reset-a", 10.0))),
            sampled_snapshot("cached", 10, 2, 0, Some((300, "reset-a", 10.0))),
        ])]);

        let allocation = allocate_plan_share(&runs, &snapshots);
        assert!(allocation.shares.is_empty());
        assert!(allocation.intervals.is_empty());
        assert_eq!(allocation.unknown["cached"].len(), 1);
        assert!(allocation.unknown["cached"][0].reason.contains("no earlier observation"));
    }

    fn allocated(allocation: &PlanShareAllocation) -> f64 {
        allocation.shares.values().flatten().map(|share| share.used_percent).sum()
    }

    #[test]
    fn two_runs_sampling_the_window_in_one_instant_are_one_observation() {
        // a@0 10%, a@5 12%, b@5 12%: the two t5 readings agree, so the
        // 2-point growth is allocated once and nothing is dropped.
        let runs = vec![plan_run("a", None, Some("claude-max")), plan_run("b", None, Some("claude-max"))];
        let snapshots = BTreeMap::from([
            ("a".into(), vec![
                plan_snapshot("a", 0, 0, Some((300, "reset-a", 10.0))),
                plan_snapshot("a", 5, 100, Some((300, "reset-a", 12.0))),
            ]),
            ("b".into(), vec![
                plan_snapshot("b", 0, 0, None),
                plan_snapshot("b", 5, 100, Some((300, "reset-a", 12.0))),
            ]),
        ]);
        let allocation = allocate_plan_share(&runs, &snapshots);
        assert!((allocated(&allocation) - 2.0).abs() < 1e-9, "{allocation:?}");
        assert_eq!(allocation.intervals.len(), 1);
        assert_eq!(allocation.intervals[0].attribution, Some(PlanShareAttribution::Apportioned));
        assert!(allocation.unknown.is_empty(), "agreeing readings are no gap: {:?}", allocation.unknown);
    }

    #[test]
    fn simultaneous_readings_that_disagree_are_named_and_never_drop_or_double_growth() {
        // a@0 10, then a@5 13 and b@5 12 in the same instant, then a@10 15.
        // Paired naively the t5 tie either drops growth or charges 1 point
        // twice. The highest stands for t5: 3 then 2, 5 in all.
        let runs = vec![plan_run("a", None, Some("claude-max")), plan_run("b", None, Some("claude-max"))];
        let snapshots = BTreeMap::from([
            ("a".into(), vec![
                plan_snapshot("a", 0, 0, Some((300, "reset-a", 10.0))),
                plan_snapshot("a", 5, 100, Some((300, "reset-a", 13.0))),
                plan_snapshot("a", 10, 200, Some((300, "reset-a", 15.0))),
            ]),
            ("b".into(), vec![
                plan_snapshot("b", 0, 0, None),
                plan_snapshot("b", 5, 0, Some((300, "reset-a", 12.0))),
                plan_snapshot("b", 10, 0, None),
            ]),
        ]);
        let allocation = allocate_plan_share(&runs, &snapshots);
        assert!((allocated(&allocation) - 5.0).abs() < 1e-9, "the observed 10 -> 15, no more: {allocation:?}");
        let deltas: Vec<f64> = allocation.intervals.iter().map(|interval| interval.delta).collect();
        assert_eq!(deltas, vec![3.0, 2.0]);
        for run in ["a", "b"] {
            let gap = allocation.unknown[run].iter().find(|gap| gap.reason == PROVIDER_WINDOW_TIE).expect(run);
            assert_eq!((gap.from, gap.to), (Some(at(5)), Some(at(5))));
        }
    }

    #[test]
    fn with_no_sample_time_the_request_time_stands_in() {
        let mut snapshot = plan_snapshot("r", 3, 0, None);
        snapshot.usage.as_mut().unwrap().sampled_at = None;
        assert_eq!(observed_at(&snapshot), at(3));
        let delayed = sampled_snapshot("r", 9, 4, 0, None);
        assert_eq!(observed_at(&delayed), at(4));
    }

    #[test]
    fn a_known_window_is_kept_beside_an_unknown_window() {
        let run = plan_run("consumer", None, Some("claude-max"));
        let mut first = plan_snapshot("consumer", 0, 0, Some((10_080, "weekly", 20.0)));
        first.usage.as_mut().unwrap().sessions[0].rate_limit.as_mut().unwrap().windows.push(RateLimitWindow {
            window_minutes: Some(300), used_percent: Some(10.0), resets_at: Some("five-hour".into()),
        });
        let snapshots = BTreeMap::from([("consumer".into(), vec![
            first,
            plan_snapshot("consumer", 10, 100, Some((10_080, "weekly", 21.0))),
        ])]);

        let allocation = allocate_plan_share(&[run], &snapshots);
        assert_eq!(allocation.shares["consumer"][0].window_minutes, 10_080);
        assert!((allocation.shares["consumer"][0].used_percent - 1.0).abs() < 1e-9);
        assert_eq!(allocation.unknown["consumer"][0].window_minutes, Some(300));
        assert!(allocation.unknown["consumer"][0].reason.contains("no earlier observation"));
    }
}
