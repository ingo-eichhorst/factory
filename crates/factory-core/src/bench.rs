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

pub use factory_kernel::BenchVerdict as Verdict;

/// What ties a task to the bench attempt that spawned it, following
/// `WorkflowOrigin`'s own shape. `#[serde(default)]` on `Task::bench_origin`
/// reads a task written before this field existed as `None`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BenchOrigin {
    pub bench_run_id: String,
    pub case_id: String,
    pub agent: String,
    pub attempt: u32,
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
        format!("{} · {} · {}", config.harness, model, config.flags.join(" "))
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
    /// `done` or `failed` -- set only for `Verdict::Unverified`, the agent's
    /// own word about how it ended.
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
}

impl BenchRun {
    /// Every attempt still to be started or judged.
    pub fn settled(&self) -> bool {
        self.attempts.iter().all(|a| a.verdict.is_some())
    }
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
            let hash = attempt.config.as_ref().map(|c| c.config_hash.clone()).unwrap_or_default();
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
    out.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.config_hash.cmp(&b.config_hash)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(harness: &str, args: &[&str]) -> ConfigSnapshot {
        derive_config(harness, &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(), "none")
    }

    fn attempt(config: Option<ConfigSnapshot>, verdict: Option<Verdict>, wall: Option<i64>) -> BenchAttempt {
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
        let different_sandbox = config_hash("claude-code", &["--model".into(), "opus".into()], "docker");
        assert_ne!(a, different_sandbox);
        let different_args = config_hash("claude-code", &["--model".into(), "sonnet".into()], "none");
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
        let a = attempt(Some(config("claude-code", &["--model", "opus"])), Some(Verdict::Pass), Some(1));
        let b = attempt(Some(config("claude-code", &["--model", "sonnet"])), Some(Verdict::Pass), Some(1));
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

    #[test]
    fn config_label_reads_naturally_with_and_without_flags() {
        let bare = derive_config("shell", &[], "none");
        assert_eq!(config_label(&bare), "shell · harness default, not recorded");
        let flagged = config("claude-code", &["--model", "opus", "--yolo"]);
        assert_eq!(config_label(&flagged), "claude-code · opus · --yolo");
    }
}
