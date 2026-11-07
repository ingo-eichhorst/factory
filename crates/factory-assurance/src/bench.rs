//! Bench runs: dataset@revision × agents × attempts, and the pure result
//! aggregation that turns settled attempts into rows a person can read.
//!
//! Everything here is a type or a pure function. Starting, advancing,
//! judging, cancelling and cleaning a run is `factory-daemon`'s job
//! (`src/bench/{store,engine}.rs`) -- it has the git worktrees, the task
//! store and the subprocesses this module never touches.
//!
//! **`config_hash` is a sha256 over harness, full arguments, and sandbox --
//! computed here, from the whole `args`, and never serialized itself.**
//! `ConfigSnapshot` carries only the hash and the same elided `flags`
//! `benchmark::Configuration` already shows; no argument value but the model
//! ever reaches a `BenchAttempt`, the same rule `benchmark.rs` lives by.

use crate::benchmark::{analyze_args, missing_fields};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub use factory_kernel::BenchVerdict as Verdict;

/// What ties a task to the bench attempt that spawned it, following
/// `WorkflowOrigin`'s own shape. `#[serde(default)]` on `Task::bench_origin`
/// reads a task written before this field existed as `None`.
///
/// `base` and `reset` are the placement the attempt's workspace needs, decided by L5 when it creates the task and
/// carried on the origin so L4 can place the run without calling back up: the commit the worktree branches from, and a
/// command that runs in the fresh worktree before the agent starts. Both are absent in an origin written before they
/// existed (`#[serde(default)]`) and are not written when absent, so an old origin round-trips byte for byte.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BenchOrigin {
    pub bench_run_id: String,
    pub case_id: String,
    pub agent: String,
    pub attempt: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset: Option<String>,
}

// Only the benchmark owner interprets its legacy reference. L4 forwards it
// without depending on this type or gaining an accessor for its fields.
impl From<BenchOrigin> for factory_process::origin::OriginRef {
    fn from(origin: BenchOrigin) -> Self {
        serde_json::from_value(serde_json::to_value(&origin).expect("an origin serializes"))
            .expect("an opaque reference accepts plain JSON")
    }
}

impl TryFrom<&factory_process::origin::OriginRef> for BenchOrigin {
    type Error = serde_json::Error;
    fn try_from(reference: &factory_process::origin::OriginRef) -> Result<Self, Self::Error> {
        serde_json::from_value(serde_json::to_value(reference)?)
    }
}

/// The configuration tuple an attempt actually dispatched with, snapshotted
/// at the moment its task was created -- reusing `benchmark.rs`'s own
/// derivation so a bench result and the Configurations tab never disagree
/// about what a harness's `args` say. `flags` are already elided; the full
/// `args` behind `config_hash` never leave this snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigSnapshot {
    pub harness: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub model_source: Option<String>,
    pub flags: Vec<String>,
    pub sandbox: String,
    pub missing: Vec<String>,
    pub config_hash: String,
}

/// sha256 over harness, the full (unelided) arguments, and sandbox. A `\0`
/// separator between every field keeps `("a", ["b"], "c")` from hashing the
/// same as `("ab", [], "c")` -- string concatenation alone cannot tell those
/// apart.
pub fn config_hash(harness: &str, full_args: &[String], sandbox: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(harness.as_bytes());
    hasher.update([0u8]);
    for arg in full_args {
        hasher.update(arg.as_bytes());
        hasher.update([0u8]);
    }
    hasher.update(sandbox.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Derive the snapshot an attempt dispatches with. `full_args` never leaves
/// this function except folded into `config_hash`.
pub fn derive_config(harness: &str, full_args: &[String], sandbox: &str) -> ConfigSnapshot {
    let (model, flags) = analyze_args(full_args);
    let model_source = model.as_ref().map(|_| "args".to_string());
    let missing = missing_fields(&model);
    let hash = config_hash(harness, full_args, sandbox);
    ConfigSnapshot {
        harness: harness.to_string(),
        model,
        model_source,
        flags,
        sandbox: sandbox.to_string(),
        missing,
        config_hash: hash,
    }
}

/// A label fit for a person: `claude-code · opus · --permission-mode …`, or
/// `claude-code · no model recorded` with no flags at all.
pub fn config_label(config: &ConfigSnapshot) -> String {
    let model = config
        .model
        .as_deref()
        .unwrap_or("harness default, not recorded");
    if config.flags.is_empty() {
        format!("{} · {}", config.harness, model)
    } else {
        format!(
            "{} · {} · {}",
            config.harness,
            model,
            config.flags.join(" ")
        )
    }
}

/// One case × agent × attempt. Starts with nothing but its identity; every
/// other field fills in as the attempt actually runs. `verdict` is the one
/// thing this attempt is ever settled by -- once it is `Some`, nothing
/// touches this attempt again.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BenchAttempt {
    pub id: String,
    pub case_id: String,
    /// The agent name requested, resolved in the case's own scope -- never
    /// necessarily the scope the roster first showed it in.
    pub agent: String,
    /// 1-based; `--attempts N` means N independent tries, not "retry until
    /// it passes".
    pub attempt: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    /// `done`, `failed` or `blocked` -- the agent's own word about how it
    /// ended, set whenever the verdict came from the case's gate (or its
    /// absence, `Verdict::Unverified`) rather than from the daemon giving up.
    /// `blocked` (#381) means the attempt was judged on a blocked report: a
    /// bench run never answers, so the gate ran then and the run was ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported: Option<String>,
    /// Free text: why this attempt was skipped, or the daemon's reason for
    /// giving up on it before any report came back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_exit_code: Option<i32>,
    /// The last 4 KiB of the gate command's combined stdout and stderr.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_clock_seconds: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<ConfigSnapshot>,
    /// #388: the bench run `bench compose` copied this attempt from. `None`
    /// on an attempt its own run dispatched -- and on every attempt written
    /// before the field existed, which is the same thing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_run: Option<String>,
}

impl BenchAttempt {
    pub fn pending(id: String, case_id: String, agent: String, attempt: u32) -> Self {
        Self {
            id,
            case_id,
            agent,
            attempt,
            task_id: None,
            run_id: None,
            verdict: None,
            reported: None,
            reason: None,
            gate_exit_code: None,
            gate_output: None,
            started_at: None,
            ended_at: None,
            wall_clock_seconds: None,
            config: None,
            source_run: None,
        }
    }

    /// In flight: a task exists and nothing has settled it yet.
    pub fn is_in_flight(&self) -> bool {
        self.task_id.is_some() && self.verdict.is_none()
    }

    pub fn is_pending(&self) -> bool {
        self.task_id.is_none() && self.verdict.is_none()
    }
}

/// The last 4 KiB of `text`, on a UTF-8 boundary -- never a byte slice that
/// would panic splitting a multi-byte character in half.
pub fn tail_4kib(text: &str) -> String {
    const CAP: usize = 4096;
    if text.len() <= CAP {
        return text.to_string();
    }
    let mut start = text.len() - CAP;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_string()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BenchRunStatus {
    Running,
    Done,
    Cancelled,
}

impl BenchRunStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }
}

/// A bench run: an immutable snapshot of the dataset's cases at the moment it
/// started, plus every attempt case × agent × attempt makes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BenchRun {
    pub id: String,
    pub dataset: String,
    pub dataset_revision: u64,
    /// The cases this run attempts, snapshotted at start -- a later edit to
    /// the dataset never changes a run already going.
    pub cases: Vec<crate::dataset::Case>,
    /// Each case's resolved base commit, resolved once when the run starts:
    /// the case's own `base`, or the scope's HEAD at that moment. Every
    /// agent's attempt at a case therefore starts from the same commit.
    pub case_bases: std::collections::BTreeMap<String, String>,
    pub agents: Vec<String>,
    pub attempts_per_case: u32,
    pub concurrency: u32,
    pub status: BenchRunStatus,
    pub attempts: Vec<BenchAttempt>,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    /// #388: the runs `bench compose` made this one from, in the order they
    /// were given. Empty for a run that dispatched its own attempts, and for
    /// every run written before the field existed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub composed_from: Vec<String>,
    /// Each source's own `dataset_revision`, by source run id, recorded when
    /// the run was composed -- what lets `bench show` say the sources
    /// disagree about the dataset without reading them again.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub source_revisions: BTreeMap<String, u64>,
    /// Free text a person gave `bench compose --note`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl BenchRun {
    /// Every attempt still to be started or judged.
    pub fn settled(&self) -> bool {
        self.attempts.iter().all(|a| a.verdict.is_some())
    }

    /// Made by `bench compose` from other runs' attempts, rather than by
    /// dispatching its own. Its attempts' worktrees belong to those runs.
    pub fn is_composed(&self) -> bool {
        !self.composed_from.is_empty()
    }
}

/// #388: one finished bench run made from the settled attempts of existing
/// runs of `dataset`, so configurations that could only be run one at a time
/// read side by side. Pure: the caller loads `sources` (in `--from` order),
/// supplies fresh ids, and writes the result. Nothing is dispatched and no
/// attempt is judged again -- each settled attempt is copied as it is, under
/// a new id (`bench_attempts.id` is the primary key) and with `source_run`
/// naming the run it came from. Its `attempt` number stays the source's, so
/// two sources' first attempts at a case are both `#1`; `source_run` tells
/// them apart.
///
/// Refuses: no source, a source named twice, a source of another dataset, a
/// source still running, a source that is itself composed (compose from its
/// sources instead -- every `source_run` then names the run whose worktree it
/// is), an `exclude` id that is no attempt of any source, and a result with
/// no attempt left at all.
///
/// The composed run is `done`, with `concurrency` 0. It spans the earliest
/// start to the latest end of its sources; `agents` is their union in order
/// of first appearance; `dataset_revision` the highest among them;
/// `attempts_per_case` the most attempts any agent × case now has. `cases`
/// and `case_bases` are the newest source's (latest `started_at`), plus any
/// case only an older source has, with that source's base.
pub fn compose(
    dataset: &str,
    sources: &[BenchRun],
    exclude: &[String],
    note: Option<String>,
    mut new_id: impl FnMut() -> String,
) -> std::result::Result<BenchRun, String> {
    if sources.is_empty() {
        return Err("bench compose needs at least one --from run".into());
    }
    let mut seen = BTreeSet::new();
    for source in sources {
        if !seen.insert(source.id.as_str()) {
            return Err(format!("bench run {} is named twice in --from", source.id));
        }
        if source.dataset != dataset {
            return Err(format!(
                "bench run {} is a run of dataset {:?}, not {dataset:?}",
                source.id, source.dataset
            ));
        }
        if source.status == BenchRunStatus::Running {
            return Err(format!(
                "bench run {} is still going; wait for it to finish or cancel it first",
                source.id
            ));
        }
        if source.is_composed() {
            return Err(format!(
                "bench run {} is itself composed; compose from its sources instead: {}",
                source.id,
                source.composed_from.join(", ")
            ));
        }
    }
    let excluded: BTreeSet<&str> = exclude.iter().map(String::as_str).collect();
    for id in &excluded {
        if !sources.iter().any(|s| s.attempts.iter().any(|a| a.id == *id)) {
            return Err(format!("--exclude {id} is not an attempt of any --from run"));
        }
    }

    let mut attempts = Vec::new();
    for source in sources {
        for attempt in &source.attempts {
            if attempt.verdict.is_none() || excluded.contains(attempt.id.as_str()) {
                continue;
            }
            let mut copy = attempt.clone();
            copy.id = new_id();
            copy.source_run = Some(source.id.clone());
            attempts.push(copy);
        }
    }
    if attempts.is_empty() {
        return Err("nothing to compose: every settled attempt of those runs is excluded".into());
    }

    let mut agents: Vec<String> = Vec::new();
    for agent in sources.iter().flat_map(|s| &s.agents) {
        if !agents.contains(agent) {
            agents.push(agent.clone());
        }
    }
    let mut per_cell: BTreeMap<(&str, &str), u32> = BTreeMap::new();
    for attempt in &attempts {
        *per_cell.entry((&attempt.agent, &attempt.case_id)).or_default() += 1;
    }
    let attempts_per_case = per_cell.values().copied().max().unwrap_or(0);

    // Newest first; a stable sort keeps `--from` order among equal starts.
    let mut by_age: Vec<&BenchRun> = sources.iter().collect();
    by_age.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    let mut cases = Vec::new();
    let mut case_bases = BTreeMap::new();
    for source in by_age {
        for case in &source.cases {
            if cases.iter().any(|c: &crate::dataset::Case| c.id == case.id) {
                continue;
            }
            if let Some(base) = source.case_bases.get(&case.id) {
                case_bases.insert(case.id.clone(), base.clone());
            }
            cases.push(case.clone());
        }
    }

    let note = note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    Ok(BenchRun {
        id: new_id(),
        dataset: dataset.to_string(),
        dataset_revision: sources.iter().map(|s| s.dataset_revision).max().unwrap_or(0),
        cases,
        case_bases,
        agents,
        attempts_per_case,
        concurrency: 0,
        status: BenchRunStatus::Done,
        attempts,
        started_at: sources.iter().map(|s| s.started_at).min().expect("at least one source"),
        ended_at: sources.iter().filter_map(|s| s.ended_at).max(),
        composed_from: sources.iter().map(|s| s.id.clone()).collect(),
        source_revisions: sources.iter().map(|s| (s.id.clone(), s.dataset_revision)).collect(),
        note,
    })
}

/// One row of `bench show`'s results table, one per distinct `config_hash`
/// among the run's attempts. Attempts that never resolved a configuration at
/// all (the agent did not exist in the case's scope) land in their own row,
/// `config_hash: ""`, so nothing here is silently dropped.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BenchResult {
    pub config_hash: String,
    pub label: String,
    pub agents: Vec<String>,
    pub attempts: u32,
    pub pass: u32,
    pub fail: u32,
    pub unverified: u32,
    pub skipped: u32,
    pub cancelled: u32,
    pub error: u32,
    /// `pass / (pass + fail)`; `None` when nothing was gated at all.
    pub resolve_rate: Option<f64>,
    pub mean_wall_clock_seconds: Option<f64>,
    /// Always `null` on the wire -- never recorded, and the payload says so
    /// rather than showing a `0`.
    pub cost: Option<f64>,
    pub missing: Vec<String>,
}

/// Turn a run's attempts into one row per configuration. Pure, and total: an
/// attempt with no configuration at all (an agent that never resolved) still
/// gets a row rather than disappearing from the count.
pub fn aggregate(attempts: &[BenchAttempt]) -> Vec<BenchResult> {
    struct Group {
        label: String,
        agents: std::collections::BTreeSet<String>,
        pass: u32,
        fail: u32,
        unverified: u32,
        skipped: u32,
        cancelled: u32,
        error: u32,
        wall_clocks: Vec<i64>,
        missing: Vec<String>,
    }

    let mut groups: std::collections::BTreeMap<String, Group> = std::collections::BTreeMap::new();

    for attempt in attempts {
        let (hash, label, missing) = match &attempt.config {
            Some(c) => (c.config_hash.clone(), config_label(c), c.missing.clone()),
            None => (
                String::new(),
                "(no configuration -- the agent never resolved)".to_string(),
                Vec::new(),
            ),
        };
        let group = groups.entry(hash).or_insert_with(|| Group {
            label,
            agents: Default::default(),
            pass: 0,
            fail: 0,
            unverified: 0,
            skipped: 0,
            cancelled: 0,
            error: 0,
            wall_clocks: Vec::new(),
            missing,
        });
        group.agents.insert(attempt.agent.clone());
        match attempt.verdict {
            Some(Verdict::Pass) => group.pass += 1,
            Some(Verdict::Fail) => group.fail += 1,
            Some(Verdict::Unverified) => group.unverified += 1,
            Some(Verdict::Skipped) => group.skipped += 1,
            Some(Verdict::Cancelled) => group.cancelled += 1,
            Some(Verdict::Error) => group.error += 1,
            None => {} // still running; counted in `attempts` only, via len()
        }
        if let Some(secs) = attempt.wall_clock_seconds {
            group.wall_clocks.push(secs);
        }
    }

    // Attempts sharing a config_hash all share this same `config`, since the
    // hash is derived from exactly the fields that would otherwise differ.
    let counts: std::collections::BTreeMap<String, u32> = {
        let mut m = std::collections::BTreeMap::new();
        for attempt in attempts {
            let hash = attempt
                .config
                .as_ref()
                .map(|c| c.config_hash.clone())
                .unwrap_or_default();
            *m.entry(hash).or_default() += 1;
        }
        m
    };

    let mut out: Vec<BenchResult> = groups
        .into_iter()
        .map(|(hash, g)| {
            let resolve_rate = if g.pass + g.fail == 0 {
                None
            } else {
                Some(f64::from(g.pass) / f64::from(g.pass + g.fail))
            };
            let mean_wall_clock_seconds = if g.wall_clocks.is_empty() {
                None
            } else {
                Some(g.wall_clocks.iter().sum::<i64>() as f64 / g.wall_clocks.len() as f64)
            };
            BenchResult {
                attempts: *counts.get(&hash).unwrap_or(&0),
                config_hash: hash,
                label: g.label,
                agents: g.agents.into_iter().collect(),
                pass: g.pass,
                fail: g.fail,
                unverified: g.unverified,
                skipped: g.skipped,
                cancelled: g.cancelled,
                error: g.error,
                resolve_rate,
                mean_wall_clock_seconds,
                cost: None,
                missing: g.missing,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        a.label
            .cmp(&b.label)
            .then_with(|| a.config_hash.cmp(&b.config_hash))
    });
    out
}

#[cfg(test)]
mod tests {
    /// An origin written before `base` and `reset` existed still parses (both absent) and round-trips unchanged;
    /// one that carries them round-trips with them, and L4 sees them as a placement.
    #[test]
    fn a_pre_change_origin_parses_without_placement_and_round_trips_unchanged() {
        let legacy = serde_json::json!({"bench_run_id": "b", "case_id": "c", "agent": "shell", "attempt": 2});
        let origin: BenchOrigin = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!((origin.base.as_deref(), origin.reset.as_deref()), (None, None));
        assert_eq!(serde_json::to_value(&origin).unwrap(), legacy, "no new keys are written for an old origin");
        let reference: factory_process::origin::OriginRef = origin.clone().into();
        assert_eq!(serde_json::to_value(&reference).unwrap(), legacy);
        assert_eq!(BenchOrigin::try_from(&reference).unwrap(), origin);
        assert_eq!(reference.placement(), Ok(Default::default()));

        let placed = BenchOrigin { base: Some("0123abc".into()), reset: Some("make clean".into()), ..origin };
        let reference: factory_process::origin::OriginRef = placed.clone().into();
        assert_eq!(BenchOrigin::try_from(&reference).unwrap(), placed);
        let placement = reference.placement().unwrap();
        assert_eq!((placement.base.as_deref(), placement.reset.as_deref()), (Some("0123abc"), Some("make clean")));
    }

    use super::*;

    fn config(harness: &str, args: &[&str]) -> ConfigSnapshot {
        derive_config(
            harness,
            &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            "none",
        )
    }

    fn attempt(
        config: Option<ConfigSnapshot>,
        verdict: Option<Verdict>,
        wall: Option<i64>,
    ) -> BenchAttempt {
        let mut a = BenchAttempt::pending("id".into(), "case".into(), "agent".into(), 1);
        a.config = config;
        a.verdict = verdict;
        a.wall_clock_seconds = wall;
        a
    }

    #[test]
    fn config_hash_is_stable_and_sensitive_to_every_field() {
        let a = config_hash("claude-code", &["--model".into(), "opus".into()], "none");
        let b = config_hash("claude-code", &["--model".into(), "opus".into()], "none");
        assert_eq!(a, b);
        let different_sandbox =
            config_hash("claude-code", &["--model".into(), "opus".into()], "docker");
        assert_ne!(a, different_sandbox);
        let different_args =
            config_hash("claude-code", &["--model".into(), "sonnet".into()], "none");
        assert_ne!(a, different_args);
        let different_harness = config_hash("pi", &["--model".into(), "opus".into()], "none");
        assert_ne!(a, different_harness);
    }

    #[test]
    fn no_argument_value_but_the_model_ever_appears_in_a_config_snapshot_or_an_attempt() {
        let snapshot = config("claude-code", &["--model", "opus", "--api-key", "s3cret"]);
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(!json.contains("s3cret"), "{json}");
        assert!(json.contains("opus"));

        let mut a = BenchAttempt::pending("a1".into(), "case-a".into(), "builder".into(), 1);
        a.config = Some(snapshot);
        let json = serde_json::to_string(&a).unwrap();
        assert!(!json.contains("s3cret"), "{json}");

        let run = BenchRun {
            id: "r1".into(),
            dataset: "demo".into(),
            dataset_revision: 1,
            cases: Vec::new(),
            case_bases: Default::default(),
            agents: vec!["builder".into()],
            attempts_per_case: 1,
            concurrency: 1,
            status: BenchRunStatus::Running,
            attempts: vec![a],
            started_at: Utc::now(),
            ended_at: None,
            composed_from: Vec::new(),
            source_revisions: Default::default(),
            note: None,
        };
        let json = serde_json::to_string(&run).unwrap();
        assert!(!json.contains("s3cret"), "{json}");
    }

    #[test]
    fn resolve_rate_excludes_unverified_from_both_halves_of_the_fraction() {
        // One pass, one fail, two unverified: 1/(1+1) = 0.5, not 1/4 = 0.25.
        let cfg = config("claude-code", &["--model", "opus"]);
        let attempts = vec![
            attempt(Some(cfg.clone()), Some(Verdict::Pass), Some(10)),
            attempt(Some(cfg.clone()), Some(Verdict::Fail), Some(20)),
            attempt(Some(cfg.clone()), Some(Verdict::Unverified), Some(30)),
            attempt(Some(cfg), Some(Verdict::Unverified), Some(40)),
        ];
        let results = aggregate(&attempts);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].resolve_rate, Some(0.5));
        assert_eq!(results[0].unverified, 2);
        assert_eq!(results[0].attempts, 4);
        assert_eq!(results[0].mean_wall_clock_seconds, Some(25.0));
        assert_eq!(results[0].cost, None);
    }

    #[test]
    fn a_configuration_with_nothing_gated_has_no_resolve_rate() {
        let cfg = config("claude-code", &[]);
        let attempts = vec![attempt(Some(cfg), Some(Verdict::Unverified), None)];
        let results = aggregate(&attempts);
        assert_eq!(results[0].resolve_rate, None);
    }

    #[test]
    fn attempts_with_different_args_land_in_different_rows() {
        let a = attempt(
            Some(config("claude-code", &["--model", "opus"])),
            Some(Verdict::Pass),
            Some(1),
        );
        let b = attempt(
            Some(config("claude-code", &["--model", "sonnet"])),
            Some(Verdict::Pass),
            Some(1),
        );
        let results = aggregate(&[a, b]);
        assert_eq!(results.len(), 2, "{results:?}");
    }

    #[test]
    fn an_attempt_with_no_configuration_gets_its_own_row_and_is_never_dropped() {
        let never_resolved = attempt(None, Some(Verdict::Skipped), None);
        let results = aggregate(&[never_resolved]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].config_hash, "");
        assert_eq!(results[0].skipped, 1);
    }

    #[test]
    fn tail_4kib_never_panics_on_a_multibyte_boundary() {
        let text = "é".repeat(3000); // 2 bytes each, ~6000 bytes total
        let tail = tail_4kib(&text);
        assert!(tail.len() <= 4096);
        assert!(text.ends_with(&tail));
    }

    #[test]
    fn tail_4kib_is_a_no_op_under_the_cap() {
        assert_eq!(tail_4kib("short"), "short");
    }

    // -- #388: bench compose ----------------------------------------------

    fn case(id: &str) -> crate::dataset::Case {
        serde_json::from_value(serde_json::json!({"id": id, "title": id, "scope": "demo"})).unwrap()
    }

    fn settled(id: &str, case: &str, agent: &str, n: u32, cfg: &ConfigSnapshot, verdict: Verdict, gate: &str) -> BenchAttempt {
        let mut a = BenchAttempt::pending(id.into(), case.into(), agent.into(), n);
        a.task_id = Some(format!("task-{id}"));
        a.run_id = Some(format!("run-{id}"));
        a.verdict = Some(verdict);
        a.reported = Some("done".into());
        a.gate_exit_code = Some(if verdict == Verdict::Pass { 0 } else { 1 });
        a.gate_output = Some(gate.into());
        a.started_at = Some("2026-11-01T10:00:00Z".parse().unwrap());
        a.ended_at = Some("2026-11-01T10:05:00Z".parse().unwrap());
        a.wall_clock_seconds = Some(300);
        a.config = Some(cfg.clone());
        a
    }

    #[allow(clippy::too_many_arguments)]
    fn source(id: &str, revision: u64, start: &str, end: &str, cases: &[(&str, &str)], agents: &[&str], attempts: Vec<BenchAttempt>) -> BenchRun {
        BenchRun {
            id: id.into(),
            dataset: "finance".into(),
            dataset_revision: revision,
            cases: cases.iter().map(|(c, _)| case(c)).collect(),
            case_bases: cases.iter().map(|(c, b)| (c.to_string(), b.to_string())).collect(),
            agents: agents.iter().map(|a| a.to_string()).collect(),
            attempts_per_case: 1,
            concurrency: 1,
            status: BenchRunStatus::Done,
            attempts,
            started_at: start.parse().unwrap(),
            ended_at: Some(end.parse().unwrap()),
            composed_from: Vec::new(),
            source_revisions: Default::default(),
            note: None,
        }
    }

    /// Two runs of `finance`, the older at revision 3 and the newer at 4.
    /// Sonnet ran in both; Qwen only in the newer one.
    fn two_sources() -> (BenchRun, BenchRun) {
        let sonnet = config("claude-code", &["--model", "sonnet"]);
        let qwen = config("pi", &["--model", "qwen"]);
        let older = source(
            "older",
            3,
            "2026-11-01T10:00:00Z",
            "2026-11-01T11:00:00Z",
            &[("c1", "base-old-c1"), ("c2", "base-old-c2")],
            &["sonnet"],
            vec![
                settled("o-1", "c1", "sonnet", 1, &sonnet, Verdict::Pass, "all good"),
                settled("o-2", "c2", "sonnet", 1, &sonnet, Verdict::Fail, "assertion failed: 1 == 2"),
                settled("o-3", "c2", "sonnet", 2, &sonnet, Verdict::Pass, "flaky pass"),
            ],
        );
        let newer = source(
            "newer",
            4,
            "2026-11-02T10:00:00Z",
            "2026-11-02T12:00:00Z",
            &[("c1", "base-new-c1"), ("c3", "base-new-c3")],
            &["qwen", "sonnet"],
            vec![
                settled("n-1", "c1", "qwen", 1, &qwen, Verdict::Pass, "qwen ok"),
                settled("n-2", "c3", "qwen", 1, &qwen, Verdict::Unverified, ""),
                settled("n-3", "c1", "sonnet", 1, &sonnet, Verdict::Fail, "sonnet broke c1"),
            ],
        );
        (older, newer)
    }

    fn ids() -> impl FnMut() -> String {
        let mut n = 0;
        move || {
            n += 1;
            format!("new-{n}")
        }
    }

    #[test]
    fn composing_two_runs_with_an_exclude_copies_every_other_settled_attempt_unchanged_under_a_new_id() {
        let (older, newer) = two_sources();
        let sources = [older.clone(), newer.clone()];
        let run = compose("finance", &sources, &["o-3".into()], Some("  side by side \n".into()), ids()).unwrap();

        assert_eq!(run.attempts.len(), 5, "three and three, less the one excluded");
        let originals: Vec<&BenchAttempt> = sources.iter().flat_map(|s| &s.attempts).collect();
        let new_ids: BTreeSet<&str> = run.attempts.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(new_ids.len(), 5, "every copy has an id of its own");
        assert!(originals.iter().all(|o| !new_ids.contains(o.id.as_str())), "no source id is reused");
        assert!(!new_ids.contains(run.id.as_str()));

        // Everything but the id and `source_run` is the source attempt's own.
        for copy in &run.attempts {
            let source_id = copy.source_run.as_deref().expect("source_run is set");
            let source = sources.iter().find(|s| s.id == source_id).unwrap();
            let original = source
                .attempts
                .iter()
                .find(|o| o.task_id == copy.task_id)
                .expect("copied from an attempt of the run it names");
            let mut back = copy.clone();
            back.id = original.id.clone();
            back.source_run = None;
            assert_eq!(&back, original, "verdict, gate output and the rest are unchanged");
        }
        assert!(run.attempts.iter().all(|a| a.task_id.as_deref() != Some("task-o-3")), "the excluded attempt is left out");

        let results = aggregate(&run.attempts);
        assert_eq!(results.len(), 2, "one row per configuration: {results:?}");
        let sonnet = results.iter().find(|r| r.label.contains("sonnet")).unwrap();
        let qwen = results.iter().find(|r| r.label.contains("qwen")).unwrap();
        assert_eq!((sonnet.attempts, sonnet.pass, sonnet.fail), (3, 1, 2));
        assert_eq!((qwen.attempts, qwen.pass, qwen.unverified), (2, 1, 1));

        assert_eq!(run.status, BenchRunStatus::Done);
        assert_eq!(run.dataset, "finance");
        assert_eq!(run.dataset_revision, 4, "the highest revision among the sources");
        assert_eq!(run.started_at, older.started_at, "the earliest start");
        assert_eq!(run.ended_at, newer.ended_at, "the latest end");
        assert_eq!(run.agents, ["sonnet", "qwen"], "the union, in order of first appearance");
        assert_eq!(run.attempts_per_case, 2, "sonnet attempted c1 once in each source");
        assert_eq!(run.concurrency, 0);
        assert_eq!(run.composed_from, ["older", "newer"]);
        assert_eq!(run.source_revisions, BTreeMap::from([("older".into(), 3), ("newer".into(), 4)]));
        assert_eq!(run.note.as_deref(), Some("side by side"));
        assert!(run.is_composed() && run.settled());

        // The newest source's cases and bases, then the case only the older one has.
        assert_eq!(run.cases.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["c1", "c3", "c2"]);
        assert_eq!(
            run.case_bases,
            BTreeMap::from([
                ("c1".into(), "base-new-c1".into()),
                ("c2".into(), "base-old-c2".into()),
                ("c3".into(), "base-new-c3".into()),
            ])
        );
    }

    #[test]
    fn compose_takes_only_settled_attempts_and_drops_an_empty_note() {
        let (mut older, _) = two_sources();
        older.status = BenchRunStatus::Cancelled;
        older.attempts[2].verdict = None;
        let run = compose("finance", &[older], &[], Some("   ".into()), ids()).unwrap();
        assert_eq!(run.attempts.len(), 2);
        assert_eq!(run.note, None);
        assert_eq!(run.attempts_per_case, 1);
    }

    #[test]
    fn compose_refuses_what_it_cannot_honestly_combine() {
        let (older, newer) = two_sources();
        let refused = |sources: &[BenchRun], exclude: &[&str]| {
            let exclude: Vec<String> = exclude.iter().map(|s| s.to_string()).collect();
            compose("finance", sources, &exclude, None, ids()).unwrap_err()
        };

        assert!(refused(&[], &[]).contains("at least one --from"));

        let mut running = newer.clone();
        running.status = BenchRunStatus::Running;
        let e = refused(&[older.clone(), running], &[]);
        assert!(e.contains("newer") && e.contains("still going"), "{e}");

        let mut other = newer.clone();
        other.dataset = "legal".into();
        let e = refused(&[older.clone(), other], &[]);
        assert!(e.contains("\"legal\"") && e.contains("not \"finance\""), "{e}");

        let e = refused(&[older.clone(), newer.clone()], &["o-1", "nope"]);
        assert!(e.contains("--exclude nope") && e.contains("not an attempt"), "{e}");

        let e = refused(&[older.clone(), older.clone()], &[]);
        assert!(e.contains("named twice"), "{e}");

        let composed = compose("finance", &[older.clone(), newer.clone()], &[], None, ids()).unwrap();
        let e = refused(&[composed], &[]);
        assert!(e.contains("itself composed") && e.contains("older, newer"), "{e}");

        let e = refused(&[older.clone()], &["o-1", "o-2", "o-3"]);
        assert!(e.contains("nothing to compose"), "{e}");
    }

    /// A run and an attempt written before #388 decode with the new fields
    /// empty, and one that never had them writes none of them back.
    #[test]
    fn a_run_and_an_attempt_without_the_compose_fields_decode_and_round_trip_unchanged() {
        let attempt = serde_json::json!({"id": "a1", "case_id": "c1", "agent": "shell", "attempt": 1, "verdict": "pass"});
        let decoded: BenchAttempt = serde_json::from_value(attempt.clone()).unwrap();
        assert_eq!(decoded.source_run, None);
        assert_eq!(serde_json::to_value(&decoded).unwrap(), attempt);

        let run = serde_json::json!({
            "id": "r1", "dataset": "finance", "dataset_revision": 3, "cases": [], "case_bases": {},
            "agents": ["shell"], "attempts_per_case": 1, "concurrency": 1, "status": "done",
            "attempts": [], "started_at": "2026-11-01T10:00:00Z"
        });
        let decoded: BenchRun = serde_json::from_value(run.clone()).unwrap();
        assert!(decoded.composed_from.is_empty() && decoded.source_revisions.is_empty() && decoded.note.is_none());
        assert!(!decoded.is_composed());
        assert_eq!(serde_json::to_value(&decoded).unwrap(), run);
    }

    #[test]
    fn config_label_reads_naturally_with_and_without_flags() {
        let bare = derive_config("shell", &[], "none");
        assert_eq!(config_label(&bare), "shell · harness default, not recorded");
        let flagged = config("claude-code", &["--model", "opus", "--yolo"]);
        assert_eq!(config_label(&flagged), "claude-code · opus · --yolo");
    }
}
