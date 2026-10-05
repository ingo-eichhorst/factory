//! L5 owns suggestions (`#275`): an agent's own account of friction during a
//! run -- a blocked capability, stale docs, a confusing spec, a pointless
//! process step or a badly modelled entity -- collected for a person to act
//! on. A suggestion is evidence from an agent, never a status: filing one
//! changes nothing about the run it came from, and nothing here ever derives
//! one on its own or turns one into work by itself (`AGENTS.md`'s "Factory
//! never creates a task from a suggestion on its own").
//!
//! This module is the pure domain model, its state machine and grouping;
//! `suggestion_store.rs` is where a suggestion is kept. `run_id`, `task_id`,
//! `scope`, `agent`, `harness`, `session_id` and `usage` are always filled in
//! by whoever calls `Filing::file` from what Factory itself knows about the
//! run -- an agent never states them, and there is nothing here for it to.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The small fixed vocabulary of what changed. Exhaustive on purpose -- an
/// agent names the closest fit rather than writing free text that nothing
/// can group on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionKind {
    /// A blocked web call, a missing grant, secret, tool or dependency.
    Capability,
    /// Documentation that was missing, stale or misleading.
    Docs,
    /// Task instructions, a spec or acceptance criteria that sent the agent
    /// down the wrong path.
    Spec,
    /// A workflow, gate or intake step that asked for the impossible or the
    /// pointless.
    Process,
    /// A confusing or wrong entity -- a model, a name, a scope.
    Entity,
    Other,
}

impl SuggestionKind {
    pub const ALL: [SuggestionKind; 6] = [
        Self::Capability,
        Self::Docs,
        Self::Spec,
        Self::Process,
        Self::Entity,
        Self::Other,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Capability => "capability",
            Self::Docs => "docs",
            Self::Spec => "spec",
            Self::Process => "process",
            Self::Entity => "entity",
            Self::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s.trim())
    }
}

impl std::fmt::Display for SuggestionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a suggestion stands. A state only ever moves forward -- see
/// [`Suggestion::task`], [`Suggestion::dismiss`] and [`Suggestion::done`] --
/// and nothing is ever deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionState {
    Open,
    /// An improvement task was created for it (or the group it belongs to).
    /// Still not final: it may yet be dismissed or marked done.
    Tasked,
    Dismissed,
    Done,
}

impl SuggestionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Tasked => "tasked",
            Self::Dismissed => "dismissed",
            Self::Done => "done",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim() {
            "open" => Self::Open,
            "tasked" => Self::Tasked,
            "dismissed" => Self::Dismissed,
            "done" => Self::Done,
            _ => return None,
        })
    }

    /// Dismissed and done are both terminal -- a suggestion that reached
    /// either stays there.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Dismissed | Self::Done)
    }
}

/// The run's usage at the moment of filing, so the cost of the problem is
/// evidence rather than a claim. `None` fields mean Factory could not work
/// that figure out yet, the same "unknown is not zero" rule
/// `factory_process::usage::RunUsage` follows -- this is a flattened,
/// L5-local copy of the parts worth keeping, never that L4 type itself.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

/// One entry in a suggestion's own append-only history: every state change,
/// and every question asked of it, journaled with who.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuggestionEvent {
    pub at: DateTime<Utc>,
    /// Who made it happen: the filing agent (`scope/agent`), or the person
    /// or agent who asked for the state change.
    pub by: String,
    /// `filed`, `tasked`, `dismissed`, `done`, `asked` or `answered`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A question asked of the agent that filed the suggestion, and its answer
/// once the resumed run reports (`#178`, `#275`'s "Ask the agent").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AskExchange {
    pub question: String,
    pub asked_by: String,
    pub asked_at: DateTime<Utc>,
    /// The continuation run started to ask it, so the answer can be matched
    /// back once that run reports.
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_at: Option<DateTime<Utc>>,
}

/// The wire shape of `factory task suggest`: exactly what the agent states,
/// nothing Factory already knows about the run. Mirrors
/// `factory_process::task::TaskReport` carrying its own `token`, checked the
/// same way (`Engine::check_run_token`) so this needs no grant an agent
/// could lack -- the run token is the whole authorization, like `report`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestionReport {
    pub kind: SuggestionKind,
    pub target: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasted_tokens: Option<u64>,
    /// Presented by the agent, checked against the run's own token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// What `factory task suggest` carries in, already resolved by whoever calls
/// [`Filing::file`] -- the daemon, from the run and task it already holds.
/// Nothing here is read back from the agent's own words.
#[derive(Debug, Clone)]
pub struct Filing {
    pub run_id: String,
    pub task_id: String,
    pub scope: String,
    pub agent: String,
    pub harness: String,
    pub session_id: Option<String>,
    pub usage: Option<UsageSnapshot>,
    pub kind: SuggestionKind,
    pub target: String,
    pub summary: String,
    pub detail: Option<String>,
    pub wasted_tokens: Option<u64>,
}

impl Filing {
    pub fn file(self, id: String, filed_at: DateTime<Utc>) -> Suggestion {
        let by = format!("{}/{}", self.scope, self.agent);
        Suggestion {
            id,
            filed_at,
            run_id: self.run_id,
            task_id: self.task_id,
            scope: self.scope,
            agent: self.agent,
            harness: self.harness,
            session_id: self.session_id,
            usage: self.usage,
            kind: self.kind,
            target: self.target,
            summary: self.summary,
            detail: self.detail,
            wasted_tokens: self.wasted_tokens,
            state: SuggestionState::Open,
            improvement_task_id: None,
            dismiss_reason: None,
            ask: None,
            history: vec![SuggestionEvent {
                at: filed_at,
                by,
                kind: "filed".into(),
                note: None,
            }],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Suggestion {
    pub id: String,
    pub filed_at: DateTime<Utc>,
    // Recorded by Factory from the run; never stated by the agent.
    pub run_id: String,
    pub task_id: String,
    pub scope: String,
    pub agent: String,
    pub harness: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageSnapshot>,
    // What the agent said.
    pub kind: SuggestionKind,
    pub target: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasted_tokens: Option<u64>,
    // State.
    pub state: SuggestionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub improvement_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismiss_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<AskExchange>,
    #[serde(default)]
    pub history: Vec<SuggestionEvent>,
}

impl Suggestion {
    /// Open, or already tasked, may be tasked (again, for a group that was
    /// partly tasked already, or a second improvement task over the same
    /// target) -- but never once dismissed or done.
    pub fn task(&mut self, improvement_task_id: &str, by: &str, at: DateTime<Utc>) -> Result<(), String> {
        if self.state.is_terminal() {
            return Err(format!(
                "suggestion {} is already {}; nothing can tie a new task to it",
                self.id,
                self.state.as_str()
            ));
        }
        self.state = SuggestionState::Tasked;
        self.improvement_task_id = Some(improvement_task_id.to_string());
        self.history.push(SuggestionEvent {
            at,
            by: by.to_string(),
            kind: "tasked".into(),
            note: Some(improvement_task_id.to_string()),
        });
        Ok(())
    }

    pub fn dismiss(&mut self, reason: String, by: &str, at: DateTime<Utc>) -> Result<(), String> {
        if self.state.is_terminal() {
            return Err(format!(
                "suggestion {} is already {}; it cannot be dismissed",
                self.id,
                self.state.as_str()
            ));
        }
        self.state = SuggestionState::Dismissed;
        self.dismiss_reason = Some(reason.clone());
        self.history.push(SuggestionEvent {
            at,
            by: by.to_string(),
            kind: "dismissed".into(),
            note: Some(reason),
        });
        Ok(())
    }

    pub fn done(&mut self, by: &str, at: DateTime<Utc>) -> Result<(), String> {
        if self.state.is_terminal() {
            return Err(format!(
                "suggestion {} is already {}; it cannot be marked done again",
                self.id,
                self.state.as_str()
            ));
        }
        self.state = SuggestionState::Done;
        self.history.push(SuggestionEvent {
            at,
            by: by.to_string(),
            kind: "done".into(),
            note: None,
        });
        Ok(())
    }

    /// Record a question asked of the agent that filed this suggestion.
    /// Overwrites a previous exchange -- only the latest question and answer
    /// are kept live on the suggestion; every ask still leaves a line in
    /// `history`.
    pub fn ask(&mut self, question: String, by: &str, run_id: &str, at: DateTime<Utc>) {
        self.history.push(SuggestionEvent {
            at,
            by: by.to_string(),
            kind: "asked".into(),
            note: Some(question.clone()),
        });
        self.ask = Some(AskExchange {
            question,
            asked_by: by.to_string(),
            asked_at: at,
            run_id: run_id.to_string(),
            answer: None,
            answered_at: None,
        });
    }

    /// Record the answer once the continuation run that was asked reports.
    /// A no-op (returns `false`) when this suggestion's pending ask is not
    /// the one that run answers -- already answered, superseded by a newer
    /// ask, or simply a different suggestion.
    pub fn answer(&mut self, run_id: &str, answer: String, at: DateTime<Utc>) -> bool {
        let Some(exchange) = &mut self.ask else { return false };
        if exchange.run_id != run_id || exchange.answer.is_some() {
            return false;
        }
        exchange.answer = Some(answer.clone());
        exchange.answered_at = Some(at);
        self.history.push(SuggestionEvent {
            at,
            by: format!("{}/{}", self.scope, self.agent),
            kind: "answered".into(),
            note: Some(answer),
        });
        true
    }
}

/// What `factory suggestion list` and `GET /api/suggestions` both narrow by.
/// `scopes: None` is every scope; `Some` is a resolved subtree (that scope
/// and its descendants), the same "roll up the subtree" rule
/// `Request::Operations`/`Request::Quality` already follow -- resolving a
/// name into that set is the caller's job, using the live config, not this
/// module's (this module never reads config).
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub scopes: Option<BTreeSet<String>>,
    pub kind: Option<SuggestionKind>,
    pub target: Option<String>,
    pub state: Option<SuggestionState>,
}

impl Filter {
    pub fn matches(&self, s: &Suggestion) -> bool {
        self.scopes.as_ref().is_none_or(|scopes| scopes.contains(&s.scope))
            && self.kind.is_none_or(|k| k == s.kind)
            && self.target.as_deref().is_none_or(|t| s.target == t)
            && self.state.is_none_or(|st| st == s.state)
    }

    pub fn apply<'a>(&self, suggestions: &'a [Suggestion]) -> Vec<&'a Suggestion> {
        suggestions.iter().filter(|s| self.matches(s)).collect()
    }
}

/// One target's suggestions folded together: the prioritisation signal --
/// "web access to X blocked" reported by ten runs reads as one item with a
/// count and the summed cost, newest first.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestionGroup {
    pub target: String,
    /// The newest suggestion's kind -- suggestions sharing a target
    /// overwhelmingly share a kind too, and this is a display label, not a
    /// second filter axis.
    pub kind: SuggestionKind,
    pub count: usize,
    pub open_count: usize,
    pub total_wasted_tokens: u64,
    pub total_cost_usd: f64,
    pub newest_filed_at: DateTime<Utc>,
    /// Every suggestion in the group, newest first -- what "create
    /// improvement task" on a group links to.
    pub ids: Vec<String>,
}

/// Fold a (already filtered) list of suggestions into one group per target,
/// newest group first. Pure: the caller decides what to pass in.
pub fn group_by_target(suggestions: &[&Suggestion]) -> Vec<SuggestionGroup> {
    use std::collections::BTreeMap;
    let mut by_target: BTreeMap<&str, Vec<&Suggestion>> = BTreeMap::new();
    for s in suggestions {
        by_target.entry(s.target.as_str()).or_default().push(s);
    }
    let mut groups: Vec<SuggestionGroup> = by_target
        .into_values()
        .map(|mut items| {
            items.sort_by(|a, b| b.filed_at.cmp(&a.filed_at));
            SuggestionGroup {
                target: items[0].target.clone(),
                kind: items[0].kind,
                count: items.len(),
                open_count: items.iter().filter(|s| s.state == SuggestionState::Open).count(),
                total_wasted_tokens: items.iter().filter_map(|s| s.wasted_tokens).sum(),
                total_cost_usd: items
                    .iter()
                    .filter_map(|s| s.usage.as_ref().and_then(|u| u.cost_usd))
                    .sum(),
                newest_filed_at: items[0].filed_at,
                ids: items.iter().map(|s| s.id.clone()).collect(),
            }
        })
        .collect();
    groups.sort_by(|a, b| b.newest_filed_at.cmp(&a.newest_filed_at));
    groups
}

/// `GET /api/suggestions`' and `factory suggestion list`'s shared answer:
/// the filtered suggestions, newest first, and the same set folded by
/// target. Computed fresh on every call, like `QualityReport` and the other
/// read projections -- nothing here is itself stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub suggestions: Vec<Suggestion>,
    pub groups: Vec<SuggestionGroup>,
}

pub fn report(suggestions: Vec<Suggestion>) -> Report {
    let groups = group_by_target(&suggestions.iter().collect::<Vec<_>>());
    Report { suggestions, groups }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filing(target: &str) -> Filing {
        Filing {
            run_id: "run-1".into(),
            task_id: "task-1".into(),
            scope: "demo".into(),
            agent: "worker".into(),
            harness: "claude-code".into(),
            session_id: Some("sess-1".into()),
            usage: Some(UsageSnapshot { total_tokens: Some(1000), cost_usd: Some(0.5) }),
            kind: SuggestionKind::Capability,
            target: target.into(),
            summary: "blocked web call".into(),
            detail: None,
            wasted_tokens: Some(2000),
        }
    }

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    #[test]
    fn filing_records_what_the_agent_never_states_and_starts_open() {
        let s = filing("L2 secret stripe_key").file("s1".into(), at(0));
        assert_eq!(s.state, SuggestionState::Open);
        assert_eq!(s.run_id, "run-1");
        assert_eq!(s.scope, "demo");
        assert_eq!(s.history.len(), 1);
        assert_eq!(s.history[0].kind, "filed");
        assert_eq!(s.history[0].by, "demo/worker");
    }

    #[test]
    fn kind_round_trips_through_its_wire_spelling() {
        for kind in SuggestionKind::ALL {
            assert_eq!(SuggestionKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(SuggestionKind::parse("bogus"), None);
    }

    #[test]
    fn open_may_be_tasked_dismissed_or_done_but_never_twice() {
        let s = filing("x").file("s1".into(), at(0));
        s.clone().task("t1", "foreman", at(1)).unwrap();

        let mut dismissed = s.clone();
        dismissed.dismiss("not worth it".into(), "owner", at(1)).unwrap();
        assert_eq!(dismissed.state, SuggestionState::Dismissed);
        assert_eq!(dismissed.dismiss_reason.as_deref(), Some("not worth it"));
        assert!(dismissed.dismiss("again".into(), "owner", at(2)).is_err());
        assert!(dismissed.done("owner", at(2)).is_err());
        assert!(dismissed.task("t2", "owner", at(2)).is_err());

        let mut done = s.clone();
        done.done("owner", at(1)).unwrap();
        assert_eq!(done.state, SuggestionState::Done);
        assert!(done.done("owner", at(2)).is_err());
    }

    #[test]
    fn tasked_may_still_be_dismissed_or_done() {
        let mut s = filing("x").file("s1".into(), at(0));
        s.task("t1", "foreman", at(1)).unwrap();
        assert_eq!(s.state, SuggestionState::Tasked);
        assert_eq!(s.improvement_task_id.as_deref(), Some("t1"));
        s.done("foreman", at(2)).unwrap();
        assert_eq!(s.state, SuggestionState::Done);
        assert_eq!(s.history.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(), vec!["filed", "tasked", "done"]);
    }

    #[test]
    fn ask_then_answer_matches_the_run_and_refuses_a_stale_or_repeat_answer() {
        let mut s = filing("x").file("s1".into(), at(0));
        s.ask("why did this fail?".into(), "owner", "ask-run-1", at(1));
        assert_eq!(s.ask.as_ref().unwrap().question, "why did this fail?");
        assert!(s.ask.as_ref().unwrap().answer.is_none());

        assert!(!s.answer("some-other-run", "irrelevant".into(), at(2)));
        assert!(s.answer("ask-run-1", "it needed a different grant".into(), at(2)));
        assert_eq!(s.ask.as_ref().unwrap().answer.as_deref(), Some("it needed a different grant"));
        // Already answered: a second answer for the same run is refused.
        assert!(!s.answer("ask-run-1", "again".into(), at(3)));
        assert_eq!(
            s.history.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
            vec!["filed", "asked", "answered"]
        );
    }

    #[test]
    fn filter_narrows_by_scope_subtree_kind_target_and_state() {
        let mut a = filing("L2 secret x").file("a".into(), at(0));
        a.scope = "projects/one".into();
        let mut b = filing("L2 secret x").file("b".into(), at(1));
        b.scope = "projects/two".into();
        b.kind = SuggestionKind::Docs;
        let all = vec![a.clone(), b.clone()];

        let by_scope = Filter { scopes: Some(["projects/one".to_string()].into_iter().collect()), ..Default::default() };
        assert_eq!(by_scope.apply(&all).len(), 1);

        let by_kind = Filter { kind: Some(SuggestionKind::Docs), ..Default::default() };
        assert_eq!(by_kind.apply(&all), vec![&b]);

        let by_target = Filter { target: Some("L2 secret x".into()), ..Default::default() };
        assert_eq!(by_target.apply(&all).len(), 2);

        let by_state = Filter { state: Some(SuggestionState::Tasked), ..Default::default() };
        assert!(by_state.apply(&all).is_empty());
    }

    #[test]
    fn grouping_sums_cost_and_wasted_tokens_and_counts_open_newest_first() {
        let mut older = filing("L2 secret x").file("a".into(), at(0));
        older.usage = Some(UsageSnapshot { total_tokens: Some(100), cost_usd: Some(0.1) });
        older.wasted_tokens = Some(10);
        let mut newer = filing("L2 secret x").file("b".into(), at(10));
        newer.usage = Some(UsageSnapshot { total_tokens: Some(200), cost_usd: Some(0.2) });
        newer.wasted_tokens = Some(20);
        newer.task("t1", "foreman", at(11)).unwrap();
        let other = filing("L3 role critic").file("c".into(), at(5));

        let refs = vec![&older, &newer, &other];
        let groups = group_by_target(&refs);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].target, "L2 secret x", "the group with the newest suggestion sorts first");
        assert_eq!(groups[0].count, 2);
        assert_eq!(groups[0].open_count, 1, "the tasked one no longer counts as open");
        assert_eq!(groups[0].total_wasted_tokens, 30);
        assert!((groups[0].total_cost_usd - 0.3).abs() < 1e-9);
        assert_eq!(groups[0].ids, vec!["b".to_string(), "a".to_string()]);
        assert_eq!(groups[1].target, "L3 role critic");
    }

    #[test]
    fn report_folds_the_same_list_it_carries_into_groups() {
        let a = filing("x").file("a".into(), at(0));
        let b = filing("x").file("b".into(), at(1));
        let out = report(vec![a, b]);
        assert_eq!(out.suggestions.len(), 2);
        assert_eq!(out.groups.len(), 1);
        assert_eq!(out.groups[0].count, 2);
    }
}
