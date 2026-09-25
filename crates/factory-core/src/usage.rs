//! What a run used: tokens, cost, the model -- as the agent runtime saw it.
//!
//! Factory is the process layer and never reads a harness transcript, never
//! learns a harness's file layout, and never talks to an observability tool
//! directly (issue #117). Usage reaches it through one door only:
//! `AgentRuntime::usage`, which answers with a [`SessionUsage`] -- the
//! versioned contract below, schema 1, owned by #117. herdr answers it by
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

/// The contract version this Factory reads.
pub const USAGE_SCHEMA: u32 = 1;

/// Tokens by type. Each count is `None` when the observer could not see it,
/// never 0.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCounts {
    #[serde(default)]
    pub input: Option<u64>,
    #[serde(default)]
    pub output: Option<u64>,
    #[serde(default)]
    pub cache_read: Option<u64>,
    #[serde(default)]
    pub cache_write: Option<u64>,
}

impl TokenCounts {
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

    /// Every token of every type, or `None` if any one type is unknown.
    /// Cache reads and writes count: they are tokens the model processed and
    /// the provider bills, if at a different rate.
    pub fn total(&self) -> Option<u64> {
        self.fields().iter().try_fold(0u64, |sum, (_, v)| v.map(|v| sum + v))
    }
}

/// What the tokens cost, in API-equivalent US dollars, and the price table
/// that said so.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageCost {
    #[serde(default)]
    pub usd: Option<f64>,
    /// e.g. `litellm@2026-09-01`. Kept with every number it priced, because a
    /// price table changes and history must not be re-priced silently.
    #[serde(default)]
    pub pricing_source: Option<String>,
}

/// One subagent a harness session spawned. Listed apart from its parent,
/// which is read as *excluding* it -- see [`run_usage`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubagentUsage {
    pub session_id: String,
    #[serde(default)]
    pub tokens: TokenCounts,
    #[serde(default)]
    pub cost: UsageCost,
}

/// One subscription rate-limit window, as the provider reports it. Stored
/// with every snapshot for the plan-share work of #117's v2; nothing in v1
/// computes with it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RateLimitWindow {
    #[serde(default)]
    pub window_minutes: Option<u32>,
    #[serde(default)]
    pub used_percent: Option<f64>,
    #[serde(default)]
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RateLimit {
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub plan_type: Option<String>,
    /// How sure the observer is that this window belongs to this session's
    /// account -- `confirmed`, or something weaker.
    #[serde(default)]
    pub attribution_quality: Option<String>,
    #[serde(default)]
    pub windows: Vec<RateLimitWindow>,
}

/// One harness session the runtime saw in a Factory session -- a Claude Code
/// or Codex conversation running in a herdr pane, say. Cumulative since that
/// harness session began.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarnessUsage {
    pub session_id: String,
    /// Which harness: `claude-code`, `codex`, ...
    #[serde(default)]
    pub adapter: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub tokens: TokenCounts,
    #[serde(default)]
    pub cost: UsageCost,
    #[serde(default)]
    pub elapsed_seconds: Option<f64>,
    #[serde(default)]
    pub active_seconds: Option<f64>,
    #[serde(default)]
    pub subagents: Vec<SubagentUsage>,
    #[serde(default)]
    pub rate_limit: Option<RateLimit>,
    /// Why a field is `null`, keyed by its dotted path (`tokens.cache_write`)
    /// -- the contract's "null with a reason". Optional on the wire: a
    /// `null` with no reason given is still unknown, just unexplained.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unavailable: BTreeMap<String, String>,
}

/// The runtime's answer to "what has this session used?" -- the #117
/// contract, schema 1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionUsage {
    pub schema: u32,
    /// The runtime's own name for where it looked: herdr's pane id. Called
    /// `pane_id` on the wire, since that is the contract's word; kept under
    /// a runtime-neutral one here.
    #[serde(default, alias = "pane_id", skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampled_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub sessions: Vec<HarnessUsage>,
}

impl SessionUsage {
    /// Read a runtime's answer. Refuses a schema this Factory does not know
    /// rather than guessing at what its fields mean; everything outside the
    /// typed fields above is dropped, which is what keeps transcript text a
    /// plugin might send from ever being stored.
    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        let value: serde_json::Value =
            serde_json::from_str(text.trim()).map_err(|e| format!("usage is not JSON: {e}"))?;
        match value.get("schema").and_then(serde_json::Value::as_u64) {
            Some(s) if s == u64::from(USAGE_SCHEMA) => {}
            Some(s) => {
                return Err(format!(
                    "usage schema {s} is not one this Factory reads (it reads {USAGE_SCHEMA})"
                ))
            }
            None => return Err("usage carries no schema version".into()),
        }
        serde_json::from_value(value).map_err(|e| format!("usage does not match schema {USAGE_SCHEMA}: {e}"))
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
    let Some(baseline) = &base.usage else {
        let why = base.unknown.as_deref().unwrap_or("the runtime did not answer");
        return RunUsage::unknown(format!("no baseline at dispatch: {why}"), count);
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

// -- rolling up ---------------------------------------------------------------

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
}

impl CostGroupBy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Issue => "issue",
            Self::Scope => "scope",
            Self::Agent => "agent",
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
            other => return Err(format!("cannot group costs by {other:?}: use task, issue, scope or agent")),
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

impl TokenSums {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
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
}

impl CostRow {
    pub fn new(key: impl Into<String>, label: Option<String>) -> Self {
        Self {
            key: key.into(),
            label,
            ..Default::default()
        }
    }

    /// Count one run in. `None` is a run with no usage on record at all.
    pub fn add(&mut self, usage: Option<&RunUsage>) {
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
            Some(c) => self.cost_usd += c,
            None => self.runs_cost_unknown += 1,
        }
        for p in &u.pricing_sources {
            if !self.pricing_sources.contains(p) {
                self.pricing_sources.push(p.clone());
            }
        }
    }

    /// Runs whose cost is fully in `cost_usd`.
    pub fn runs_costed(&self) -> u32 {
        self.runs - self.runs_unknown - self.runs_cost_unknown
    }
}

/// `factory cost`'s and `GET /api/costs`' answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostReport {
    pub group_by: CostGroupBy,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Most expensive first.
    pub rows: Vec<CostRow>,
    pub total: CostRow,
}

impl CostReport {
    /// Order rows most expensive first, ties by key, so the list reads the
    /// same on every call.
    pub fn sort_rows(rows: &mut [CostRow]) {
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
    pub usage: RunUsage,
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
    fn group_by_round_trips() {
        for g in [CostGroupBy::Task, CostGroupBy::Issue, CostGroupBy::Scope, CostGroupBy::Agent] {
            assert_eq!(g.as_str().parse::<CostGroupBy>().unwrap(), g);
            assert_eq!(serde_json::to_value(g).unwrap(), serde_json::json!(g.as_str()));
        }
        assert!("provider".parse::<CostGroupBy>().is_err());
    }
}
