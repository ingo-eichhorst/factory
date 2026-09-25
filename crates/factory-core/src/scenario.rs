//! `#100` (L6 Scenarios): play out a what-if -- a goal changes, a regulation
//! tightens, capacity drops -- without ever touching the real config. Same
//! shape as `policy.rs` and `goals.rs`: authored YAML under
//! `<root>/.factory/scenarios/<name>.yaml`, re-read on every request, never
//! written by Factory, a [`Finding`] for anything an author got wrong rather
//! than a hard failure that takes the rest of the directory down with it.
//!
//! This slice ("scenarios-core") is pure `factory-core` only -- the loader,
//! the policy overlay, a seeded Monte Carlo forecast, a small built-in
//! driver tree with a tornado, goal-scenario probabilities and signposts.
//! No engine, protocol, access, CLI or HTTP wiring; no I/O anywhere in this
//! module except [`load`] itself (and [`load_drafts`], a thin wrapper around
//! `policy::load_all`).
//!
//! ## Layering: never mix an exact answer with a probabilistic one
//!
//! Three different kinds of answer live in this module, and they are never
//! combined into one number (the issue's own guardrail):
//!
//! - **Exact:** [`overlay_chain`] and [`policy_delta`] reuse `policy.rs`'s
//!   own pure evaluator -- a control is open or it is not, deterministically,
//!   the same as the real report.
//! - **Probabilistic:** [`forecast_completion`], [`tornado`] and
//!   [`goal_probability`] are all seeded Monte Carlo -- always a band
//!   (p10/p50/p90), never a single number, and always the same band for the
//!   same seed and inputs (see "Determinism" below).
//! - **Qualitative:** [`Narrative`] carries the workshop layer's own free
//!   text (a 2x2's axes, PESTLE drivers, a pre-mortem) verbatim. Nothing
//!   here scores or evaluates it; turning a pre-mortem point into a real
//!   signpost or driver override is an author's job, done by editing the
//!   YAML, not a computation.
//!
//! ## Determinism
//!
//! Every function that reads a clock takes `now` as a parameter, exactly
//! like `goals::evaluate`. Every function that samples takes an explicit
//! `seed: u64`; [`seed_from`] turns a scenario's own identity (its name,
//! plus whatever it forecasts) into one, so the same file and the same
//! history always produce the same bands -- the issue's own requirement.
//! The sampler itself ([`forecast_completion`], [`forecast_metric`]) is a
//! from-scratch splitmix64 PRNG, not `std`'s `DefaultHasher` (its algorithm
//! is explicitly unspecified and may change between Rust releases, which
//! would silently break "same seed forever") and not a new crate dependency
//! -- splitmix64 is a dozen lines with a well-known reference
//! implementation, which is all determinism needs here.
//!
//! ## Storage
//!
//! `<root>/.factory/scenarios/<name>.yaml`, stem must equal the scenario's
//! own `name` -- the same rule `policy::Catalogue.framework` and
//! `goals::Cycle.id` hold against their file names. [`load`] reads the whole
//! directory in the same two-pass shape `policy::load_all`/`goals::load`
//! use: a file that fails to parse, or whose name does not match its own
//! `name`, is a [`Finding`] naming it, and every other file still loads. A
//! missing directory is empty, not an error.
//!
//! Draft policy catalogues -- a framework not yet real enough for
//! `.factory/policies/`, only for playing out a what-if -- live at
//! `<root>/.factory/policies/drafts/*.yaml`. [`load_drafts`] reads them with
//! `policy::load_all`'s own semantics; a test in this module pins that
//! `policy::load_all` on the real `policies/` directory never recurses into
//! `drafts/`, so a draft can never leak into the real compliance report by
//! accident.
//!
//! ## Deviations from the issue's sketch (read before assuming a shape)
//!
//! The issue text is a sketch, not a spec down to the byte -- `goals.rs`'s
//! own doc comment sets that precedent (its `MetricId` charset, its
//! `goal_tasks_done` binding scheme). This module makes a few more calls
//! the issue leaves open, each documented again at the item it affects:
//!
//! 1. **`from: Option<NaiveDate>` is a new top-level field**, not in the
//!    issue's field list. "Stale (horizon passed)" needs an anchor date to
//!    measure the horizon from, and nothing else in the authored shape gives
//!    one -- a file's own mtime was considered and rejected: this very repo
//!    hands every task a fresh git worktree, which resets every file's mtime
//!    on checkout, so a mtime-based staleness check would silently un-stale
//!    every scenario the moment a worktree is cut. `from` unset means
//!    staleness is simply never checked for that scenario (see
//!    [`stale_findings`]).
//! 2. **`overlay_chain` takes `&Scenario`, not `&PolicyOverlay`, and returns
//!    findings.** The issue's own sketch is
//!    `overlay_chain(chain, overlay) -> Vec<PolicyLayer>`. A scenario's
//!    `policy:` section is optional (only a `kind: [policy, ...]` scenario
//!    authors one), so taking the whole `Scenario` lets "no policy overlay"
//!    be a clean no-op instead of forcing every caller to unwrap
//!    `scenario.policy` first. The synthetic layer's scope is
//!    `scenario:<name>` -- the issue offers "or the leaf scope's" as an
//!    alternative, rejected because `policy::applicable`'s own findings
//!    (`UnknownFramework`, `LooseningHasNoEffect`, ...) name `layer.scope`
//!    as their subject, and attributing a scenario author's own mistake to
//!    the real scope it was evaluated against would point a person at the
//!    wrong file. Findings are added because `drop_not_applicable` can name
//!    a control that was never marked not-applicable anywhere in the chain
//!    -- a no-op the issue does not mention but `policy.rs`'s own
//!    `LooseningHasNoEffect` establishes the house answer for: report it,
//!    don't silently ignore it.
//! 3. **`evaluate_outcomes` stays `BTreeMap<OutcomeId, f64>` exactly as
//!    written, and `weeks_to_clear` is its own function, not a map entry.**
//!    A weeks-to-clear-the-backlog figure is `f64::INFINITY` at zero
//!    effective throughput, which `serde_json` serializes as `null`,
//!    indistinguishable on the wire from "missing" -- keeping it a
//!    dedicated `Option<f64>`-returning function sidesteps that without
//!    touching the outcomes map's own type.
//! 4. **An unquoted number in `drivers:` (`review_time_per_task: +5` with no
//!    quotes) is refused, not guessed.** YAML's own grammar reads a bare
//!    `+5` or `-5` as an integer scalar, not the string `"+5"` -- by the
//!    time this module sees it, the sign survived but the *meaning* (delta
//!    vs. an absolute assumption) did not, so it is a [`FindingKind::BadOverride`]
//!    asking the author to quote it (`"+5"`), never a silent guess. See
//!    [`RawOverride`].
//! 5. **The built-in driver set is this module's own invention.** The issue
//!    gives two different example driver names in two places (design's
//!    `review_time_per_task`/`runs_per_day` in the YAML sketch, `#100`'s own
//!    task text's `throughput_week`/`first_pass_yield`/`scrap_rate`/
//!    `rework_rate`/`capacity_factor` for the driver *tree*) -- this module
//!    ships the second set, since those are the ones with formulas attached
//!    and registry metrics to tie to. See [`driver_defs`].
//! 6. **`forecast_metric` is a smaller cousin of [`Forecast`], not the same
//!    type.** It has no backlog to clear and thus no `completion_week` or
//!    `probability_done_by` -- see [`MetricForecast`].
//!
//! ## Dependencies run one way
//!
//! This module imports `crate::metrics` (the registry) and `crate::policy`
//! and `crate::goals` (to overlay a policy chain and to name a key result by
//! [`goals::KrRef`]) -- never the other way around. `goals.rs`'s own doc
//! comment already promises this module "can sit on the registry without
//! ever picking up a goals-shaped dependency by accident"; the policy and
//! goals dependencies here are additive to that, not a contradiction of it,
//! since neither `policy.rs` nor `goals.rs` imports anything from this one.

use crate::goals::KrRef;
use crate::metrics::{self, MetricError, MetricId, MetricSeries, MetricValue};
use crate::policy::{self, ControlRef, ControlStatus, PolicyLayer};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

// ================================================================ horizon

/// How far out a scenario projects, authored as `26w`, `182d`, or `4380h` --
/// exactly `policy::Duration`'s own grammar, reused rather than
/// reimplemented, and then rounded up to whole weeks (a forecast's own unit
/// throughout this module). Defaults to 26 weeks when a scenario names none
/// (the issue's own default). A duration that rounds to zero weeks floors to
/// one -- a scenario projecting nothing has no meaning, and every consumer
/// of a [`Horizon`] (`forecast_completion`'s `horizon_weeks`, staleness's
/// own arithmetic) needs at least one week to mean anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Horizon {
    weeks: u32,
}

impl Horizon {
    pub fn from_weeks(weeks: u32) -> Self {
        Self { weeks: weeks.max(1) }
    }

    pub fn weeks(&self) -> u32 {
        self.weeks
    }
}

impl Default for Horizon {
    fn default() -> Self {
        Horizon { weeks: 26 }
    }
}

impl std::str::FromStr for Horizon {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let duration: policy::Duration = s
            .parse()
            .map_err(|e| format!("{s:?} is not a horizon like 26w, 182d, or 4380h: {e}"))?;
        let weeks = duration.as_hours().div_ceil(24 * 7).max(1) as u32;
        Ok(Horizon { weeks })
    }
}

impl std::fmt::Display for Horizon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}w", self.weeks)
    }
}

impl TryFrom<String> for Horizon {
    type Error = String;

    fn try_from(s: String) -> std::result::Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<Horizon> for String {
    fn from(h: Horizon) -> String {
        h.to_string()
    }
}

// =============================================================== scenario

/// One authored what-if, exactly as written at
/// `<root>/.factory/scenarios/<name>.yaml`. See the module doc comment for
/// the storage rule and deviation 1 (`from`) from the issue's own field
/// list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub name: String,
    pub title: String,
    /// `policy`, `goals`, `drivers`, `narrative` -- kept as raw strings
    /// rather than a closed enum so an unrecognised entry is
    /// [`FindingKind::UnknownKind`], a finding that leaves the rest of the
    /// file loaded, not a parse failure that drops it (the issue lists
    /// "unknown kind" as its own finding, distinct from a parse error).
    #[serde(default)]
    pub kind: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assumptions: Option<String>,
    #[serde(default)]
    pub horizon: Horizon,
    /// When this scenario's horizon is measured from -- see deviation 1 in
    /// the module doc comment. `None` means [`stale_findings`] never flags
    /// it: there is nothing to measure "past the horizon" against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<NaiveDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<PolicyOverlay>,
    #[serde(default)]
    pub goals: Vec<GoalChange>,
    /// Driver id to its raw, as-authored override -- see [`RawOverride`]
    /// for why the value is not simply `String`, and
    /// [`Scenario::driver_overrides`] for turning this into something
    /// [`apply_overrides`] can use.
    #[serde(default)]
    pub drivers: BTreeMap<DriverId, RawOverride>,
    #[serde(default)]
    pub signposts: Vec<Signpost>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub narrative: Option<Narrative>,
}

impl Scenario {
    /// Every `drivers:` entry that is a validly-parsing override, keyed by
    /// driver id. An entry with bad syntax, or an unquoted number, is
    /// already a [`Finding`] from [`load`] -- this simply omits it rather
    /// than erroring a second time; a caller that wants to know why an
    /// entry is missing re-reads [`load`]'s own findings.
    pub fn driver_overrides(&self) -> BTreeMap<DriverId, Override> {
        self.drivers
            .iter()
            .filter_map(|(id, raw)| match raw {
                RawOverride::Text(s) => parse_override(s).ok().map(|ov| (id.clone(), ov)),
                RawOverride::Number(_) => None,
            })
            .collect()
    }
}

/// An overlay on the real policy chain -- add frameworks, tighten a
/// control's freshness window, or say a control marked not-applicable no
/// longer is. Never loosens anything: see [`overlay_chain`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyOverlay {
    #[serde(default)]
    pub add_frameworks: Vec<String>,
    /// Reuses `policy::Tighten` verbatim -- the same `{max_age}` shape a
    /// real scope's own `tighten:` config already writes, so
    /// `policy::applicable`'s add-or-tighten-only arithmetic applies to a
    /// scenario's tighten exactly the way it applies to a real one.
    #[serde(default)]
    pub tighten: BTreeMap<ControlRef, policy::Tighten>,
    /// Controls to strip a `not_applicable` declaration from, in the chain
    /// copy [`overlay_chain`] builds -- "what if this control's n/a
    /// rationale no longer holds".
    #[serde(default)]
    pub drop_not_applicable: Vec<ControlRef>,
}

/// One key result's changed target and/or deadline. At least one of
/// `target`/`by` must be set, or the change does nothing --
/// [`FindingKind::NoOpGoalChange`] says so at load time rather than a
/// caller discovering it does nothing once it tries to score it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalChange {
    pub kr: KrRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<NaiveDate>,
}

/// A `drivers:` map value, exactly as YAML hands it back -- see deviation 4
/// in the module doc comment. `Text` is the only variant [`Scenario::driver_overrides`]
/// (and [`parse_override`]) ever accepts; `Number` exists only so [`load`]
/// can tell the two apart and raise [`FindingKind::BadOverride`] rather than
/// have serde refuse the whole file with a type error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RawOverride {
    Text(String),
    Number(f64),
}

/// A threshold on a registry metric, watched on every read -- "is reality
/// moving towards this scenario". Never itself starts, stops, or gates
/// anything (design §8, same as everywhere else in this module); see
/// [`evaluate_signposts`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signpost {
    pub metric: MetricId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub below: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub above: Option<f64>,
    /// Inactive before this date -- `None` means active immediately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<NaiveDate>,
}

/// The qualitative workshop layer -- a 2x2's axes and chosen quadrant,
/// PESTLE drivers, a pre-mortem's list of things that went wrong. Free text,
/// authored and re-read verbatim; nothing here scores or validates any of
/// it (see the module doc comment's "Layering" section). Turning a
/// pre-mortem point into a real driver override or signpost is an edit to
/// this same YAML file, done by a person, not a computation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Narrative {
    /// The two axes of critical uncertainty a 2x2 is drawn against, e.g.
    /// `["regulatory pressure", "market demand"]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axes: Option<Vec<String>>,
    /// Which of the four quadrants this scenario names, in the author's own
    /// words -- not a closed enum, since a 2x2's quadrant names are chosen
    /// per workshop, not fixed across every scenario file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quadrant: Option<String>,
    /// PESTLE-style driver descriptions -- free text, distinct from the
    /// built-in [`DriverId`] vocabulary [`driver_defs`] names.
    #[serde(default)]
    pub drivers: Vec<String>,
    #[serde(default)]
    pub premortem: Vec<String>,
}

// ================================================================ findings

/// One of the things [`load`] (or [`stale_findings`], or [`overlay_chain`])
/// checks for. Never stops another file, or another part of the same file,
/// from loading -- exactly `policy::Finding`'s and `goals::Finding`'s own
/// promise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    ParseFailed,
    /// A scenario file's `name` does not match the file's own stem.
    StemMismatch,
    /// A `kind` entry is none of `policy`, `goals`, `drivers`, `narrative`.
    UnknownKind,
    /// A signpost names a metric [`metrics::resolve`] has never heard of.
    UnknownMetric,
    /// A signpost names a metric `metrics::resolve` knows but cannot
    /// compute yet (e.g. a cost metric -- design §12.6).
    UnavailableMetric,
    /// A signpost names neither `below` nor `above`.
    SignpostMissingThreshold,
    /// A `drivers:` entry's key is not one of [`driver_defs`]'s built-in
    /// ids.
    UnknownDriver,
    /// A `drivers:` entry's value does not parse as an override -- either
    /// bad syntax, or an unquoted number (deviation 4, module doc comment).
    BadOverride,
    /// A `goals:` entry names neither `target` nor `by`.
    NoOpGoalChange,
    /// More scenarios are loaded than the cap the issue sets.
    TooManyScenarios,
    /// A scenario's horizon has passed -- see [`stale_findings`].
    Stale,
    /// A policy overlay's `drop_not_applicable` names a control that was
    /// never marked not-applicable anywhere in the chain it was applied to
    /// -- see [`overlay_chain`], deviation 2 in the module doc comment.
    DropNotApplicableHasNoEffect,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub kind: FindingKind,
    /// The file (`<name>.yaml`) or, for [`FindingKind::TooManyScenarios`],
    /// the literal `"scenarios"` -- "where to go look", the same role it
    /// plays in `policy::Finding`/`goals::Finding`.
    pub subject: String,
    pub detail: String,
}

fn finding(kind: FindingKind, subject: impl Into<String>, detail: impl Into<String>) -> Finding {
    Finding { kind, subject: subject.into(), detail: detail.into() }
}

fn sort_findings(findings: &mut [Finding]) {
    findings.sort_by(|a, b| a.subject.cmp(&b.subject).then(a.kind.cmp(&b.kind)).then(a.detail.cmp(&b.detail)));
}

// ================================================================= loader

/// `<root>/.factory/scenarios`, the authored-content directory.
pub fn scenarios_dir(root: &Path) -> PathBuf {
    root.join(".factory").join("scenarios")
}

/// The issue's own cap on active scenarios ("3-4").
pub const MAX_ACTIVE_SCENARIOS: usize = 4;

/// Load every `<name>.yaml` in `dir`. A missing directory is empty, not an
/// error; one bad file never stops the others from loading. See the module
/// doc comment for the two-pass shape this shares with
/// `policy::load_all`/`goals::load`. Does not check staleness -- that needs
/// `now`, which this deliberately never reads; see [`stale_findings`].
pub fn load(dir: &Path) -> (Vec<Scenario>, Vec<Finding>) {
    let mut findings = Vec::new();
    let mut scenarios: Vec<Scenario> = Vec::new();

    let mut paths: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(read) => read
            .flatten()
            .map(|entry| entry.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("yaml"))
            .collect(),
        Err(_) => Vec::new(),
    };
    paths.sort();

    for path in &paths {
        let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();

        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                findings.push(finding(FindingKind::ParseFailed, &file_name, format!("reading: {e}")));
                continue;
            }
        };

        match serde_yaml_ng::from_str::<Scenario>(&text) {
            Ok(scenario) => {
                if scenario.name != stem {
                    findings.push(finding(
                        FindingKind::StemMismatch,
                        &file_name,
                        format!("scenario name {:?} does not match the file name {:?}", scenario.name, stem),
                    ));
                    continue;
                }
                scenarios.push(scenario);
            }
            Err(e) => findings.push(finding(FindingKind::ParseFailed, &file_name, format!("parsing: {e}"))),
        }
    }

    scenarios.sort_by(|a, b| a.name.cmp(&b.name));

    for scenario in &scenarios {
        validate_scenario(scenario, &mut findings);
    }

    if scenarios.len() > MAX_ACTIVE_SCENARIOS {
        let names: Vec<&str> = scenarios.iter().map(|s| s.name.as_str()).collect();
        findings.push(finding(
            FindingKind::TooManyScenarios,
            "scenarios",
            format!(
                "{} scenarios loaded, more than the cap of {MAX_ACTIVE_SCENARIOS}: {}",
                scenarios.len(),
                names.join(", ")
            ),
        ));
    }

    sort_findings(&mut findings);
    (scenarios, findings)
}

fn validate_scenario(scenario: &Scenario, findings: &mut Vec<Finding>) {
    let subject = format!("{}.yaml", scenario.name);

    for k in &scenario.kind {
        if !matches!(k.as_str(), "policy" | "goals" | "drivers" | "narrative") {
            findings.push(finding(
                FindingKind::UnknownKind,
                &subject,
                format!("kind {k:?} is not one of policy, goals, drivers, narrative"),
            ));
        }
    }

    for change in &scenario.goals {
        if change.target.is_none() && change.by.is_none() {
            findings.push(finding(
                FindingKind::NoOpGoalChange,
                &subject,
                format!("{} names neither target nor by; changes nothing", change.kr),
            ));
        }
    }

    for (driver_id, raw) in &scenario.drivers {
        if !driver_defs().iter().any(|d| d.id == driver_id) {
            findings.push(finding(
                FindingKind::UnknownDriver,
                &subject,
                format!("driver {driver_id:?} is not one of the built-in drivers"),
            ));
        }
        match raw {
            RawOverride::Text(s) => {
                if let Err(e) = parse_override(s) {
                    findings.push(finding(FindingKind::BadOverride, &subject, format!("driver {driver_id:?}: {e}")));
                }
            }
            RawOverride::Number(_) => {
                findings.push(finding(
                    FindingKind::BadOverride,
                    &subject,
                    format!(
                        "driver {driver_id:?} is an unquoted number; YAML reads +5/-5 as an integer, \
                         losing whether it means a delta or an absolute assumption -- quote it, \
                         e.g. \"+5\" or \"=5\""
                    ),
                ));
            }
        }
    }

    for sp in &scenario.signposts {
        match metrics::resolve(&sp.metric) {
            Ok(_) => {}
            Err(MetricError::Unknown(_)) => {
                findings.push(finding(FindingKind::UnknownMetric, &subject, format!("signpost names unknown metric {}", sp.metric)));
            }
            Err(MetricError::Unavailable { reason, .. }) => {
                findings.push(finding(
                    FindingKind::UnavailableMetric,
                    &subject,
                    format!("signpost names metric {} which is not available yet: {reason}", sp.metric),
                ));
            }
        }
        if sp.below.is_none() && sp.above.is_none() {
            findings.push(finding(
                FindingKind::SignpostMissingThreshold,
                &subject,
                format!("signpost on {} names neither below nor above", sp.metric),
            ));
        }
    }
}

/// Which scenarios have run past their own horizon -- a separate, pure
/// function from [`load`] because it needs `now` and `load` deliberately
/// never reads one (see the module doc comment). Only checks scenarios with
/// an explicit `from` (deviation 1); a scenario that never says when its
/// clock started is never flagged. "Never read" -- the other half of the
/// issue's own "past their horizon, never read" -- is a fact about who has
/// looked at the scenario, which this module has no state to track; that is
/// the daemon's to add once it has read telemetry.
pub fn stale_findings(scenarios: &[Scenario], now: DateTime<Utc>) -> Vec<Finding> {
    let mut findings = Vec::new();
    let today = now.date_naive();
    for scenario in scenarios {
        let Some(from) = scenario.from else { continue };
        let horizon_ends = from + chrono::Duration::weeks(scenario.horizon.weeks() as i64);
        if today > horizon_ends {
            findings.push(finding(
                FindingKind::Stale,
                format!("{}.yaml", scenario.name),
                format!(
                    "horizon ({from} + {}) ended {horizon_ends}, before today {today}",
                    scenario.horizon
                ),
            ));
        }
    }
    sort_findings(&mut findings);
    findings
}

// ============================================================ policy delta

/// Build the chain [`policy::applicable`] should evaluate for `scenario`'s
/// what-if, from the real chain a caller already resolved for some scope.
/// `scenario.policy` unset is a no-op (`chain` returned unchanged). See
/// deviation 2 in the module doc comment for why this takes `&Scenario`
/// rather than the issue's own `&PolicyOverlay`, and why the synthetic
/// layer's scope is `scenario:<name>`.
///
/// `drop_not_applicable` strips a `not_applicable` declaration for the named
/// control from every layer of the chain copy (layers otherwise keep their
/// own original scope names) -- "what if this control's rationale no longer
/// holds". A control named that was not marked not-applicable anywhere in
/// the chain is a [`FindingKind::DropNotApplicableHasNoEffect`] finding,
/// parallel to `policy::FindingKind::LooseningHasNoEffect`.
///
/// Never loosens anything, by construction: the synthetic layer can only
/// add frameworks (a union, same as every other layer) and tighten (whose
/// own add-or-tighten-only arithmetic is `policy::applicable`'s, reused
/// unchanged here, not reimplemented) -- see the "overlay never loosens"
/// tests below, which feed this straight into `policy::applicable` and
/// check the result, not this function's own bookkeeping.
pub fn overlay_chain(chain: &[PolicyLayer], scenario: &Scenario) -> (Vec<PolicyLayer>, Vec<Finding>) {
    let mut findings = Vec::new();
    let Some(overlay) = &scenario.policy else {
        return (chain.to_vec(), findings);
    };

    let mut dropped: BTreeSet<ControlRef> = BTreeSet::new();
    let mut new_chain: Vec<PolicyLayer> = chain
        .iter()
        .map(|layer| {
            let mut layer = layer.clone();
            layer.not_applicable.retain(|na| {
                if overlay.drop_not_applicable.contains(&na.control) {
                    dropped.insert(na.control.clone());
                    false
                } else {
                    true
                }
            });
            layer
        })
        .collect();

    let subject = format!("{}.yaml", scenario.name);
    for control in &overlay.drop_not_applicable {
        if !dropped.contains(control) {
            findings.push(finding(
                FindingKind::DropNotApplicableHasNoEffect,
                &subject,
                format!("drop_not_applicable names {control}, which is not marked not applicable anywhere in the chain"),
            ));
        }
    }

    new_chain.push(PolicyLayer {
        scope: format!("scenario:{}", scenario.name),
        frameworks: overlay.add_frameworks.clone(),
        tighten: overlay.tighten.clone(),
        not_applicable: Vec::new(),
    });

    (new_chain, findings)
}

/// One framework's rollup before and after a scenario -- `None` on either
/// side means the framework did not appear in that side's statuses at all
/// (e.g. a framework the scenario's `add_frameworks` introduced has no
/// `before`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FrameworkDelta {
    pub framework: String,
    pub before: Option<policy::FrameworkRollup>,
    pub after: Option<policy::FrameworkRollup>,
}

/// What a policy what-if changes, control by control, plus the same
/// before/after rollup the real L6 tab already shows. See
/// [`policy_delta`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PolicyDelta {
    /// A control absent, not-applicable, or otherwise not open in `baseline`
    /// that is `open` in `scenario`.
    pub newly_open: Vec<ControlRef>,
    /// Same, for `stale`.
    pub newly_stale: Vec<ControlRef>,
    /// A control that was not part of `baseline` at all (typically one an
    /// `add_frameworks` overlay introduced) but is already `satisfied` or
    /// `attested` in `scenario` -- existing evidence already covers it, so
    /// the what-if costs nothing here.
    pub newly_applicable_but_covered: Vec<ControlRef>,
    /// Every other control present in `scenario`: identical status to
    /// `baseline`, or a change that is neither a regression into
    /// open/stale nor a brand-new control -- an improvement, for instance.
    /// The full before/after for any one control is always in `baseline`
    /// and `scenario` themselves; this is a count, not a list, because nothing
    /// here needs a person's attention the way the other three fields do.
    pub unchanged: usize,
    /// A control `baseline` had that `scenario` does not -- should always
    /// be empty, since an overlay only ever adds or tightens, never drops a
    /// control the baseline already applied (`policy::applicable`'s own
    /// invariant). Surfaced rather than silently dropped if it ever is not.
    pub missing_from_scenario: Vec<ControlRef>,
    /// Sorted by framework name.
    pub per_framework: Vec<FrameworkDelta>,
}

/// Compare two already-evaluated status lists -- `baseline` from the real
/// chain, `scenario` from [`overlay_chain`]'s output fed through
/// `policy::applicable`/`policy::evaluate` the same way -- and classify
/// every control in `scenario` against its `baseline` counterpart, if any.
/// Pure: takes both sides already evaluated, exactly like `policy::rollup`
/// takes statuses already evaluated.
pub fn policy_delta(baseline: &[ControlStatus], scenario: &[ControlStatus]) -> PolicyDelta {
    use policy::StatusKind::{Attested, NotApplicable, Open, Satisfied, Stale};

    let baseline_by: BTreeMap<ControlRef, &ControlStatus> = baseline.iter().map(|s| (s.control.clone(), s)).collect();
    let scenario_by: BTreeMap<ControlRef, &ControlStatus> = scenario.iter().map(|s| (s.control.clone(), s)).collect();

    let mut newly_open = Vec::new();
    let mut newly_stale = Vec::new();
    let mut newly_applicable_but_covered = Vec::new();
    let mut unchanged = 0usize;

    for (control, after) in &scenario_by {
        let after_kind = after.status.kind();
        match baseline_by.get(control) {
            Some(before) => {
                let before_kind = before.status.kind();
                if before_kind == after_kind {
                    unchanged += 1;
                } else {
                    match after_kind {
                        Open => newly_open.push(control.clone()),
                        Stale => newly_stale.push(control.clone()),
                        Satisfied | Attested | NotApplicable => unchanged += 1,
                    }
                }
            }
            None => match after_kind {
                Open => newly_open.push(control.clone()),
                Stale => newly_stale.push(control.clone()),
                Satisfied | Attested => newly_applicable_but_covered.push(control.clone()),
                NotApplicable => {}
            },
        }
    }

    let mut missing_from_scenario: Vec<ControlRef> =
        baseline_by.keys().filter(|c| !scenario_by.contains_key(*c)).cloned().collect();

    newly_open.sort();
    newly_stale.sort();
    newly_applicable_but_covered.sort();
    missing_from_scenario.sort();

    let before_rollup: BTreeMap<String, policy::FrameworkRollup> =
        policy::rollup(baseline).into_iter().map(|r| (r.framework.clone(), r)).collect();
    let after_rollup: BTreeMap<String, policy::FrameworkRollup> =
        policy::rollup(scenario).into_iter().map(|r| (r.framework.clone(), r)).collect();
    let frameworks: BTreeSet<String> = before_rollup.keys().chain(after_rollup.keys()).cloned().collect();
    let per_framework = frameworks
        .into_iter()
        .map(|fw| FrameworkDelta { before: before_rollup.get(&fw).cloned(), after: after_rollup.get(&fw).cloned(), framework: fw })
        .collect();

    PolicyDelta { newly_open, newly_stale, newly_applicable_but_covered, unchanged, missing_from_scenario, per_framework }
}

/// `<root>/.factory/policies/drafts`, alongside `policy::policies_dir`.
pub fn drafts_dir(root: &Path) -> PathBuf {
    policy::policies_dir(root).join("drafts")
}

/// Read draft catalogues with `policy::load_all`'s own semantics -- a draft
/// is a framework not yet real enough for `.factory/policies/`, only for a
/// scenario's `add_frameworks` to name. See the module doc comment: a test
/// below pins that `policy::load_all` on the real `policies/` directory
/// never recurses into `drafts/`, so this is the only way a draft is ever
/// read.
pub fn load_drafts(dir: &Path) -> (Vec<policy::Catalogue>, Vec<policy::Finding>) {
    policy::load_all(dir)
}

// ============================================================== forecast

/// A tiny, deterministic, non-cryptographic PRNG -- splitmix64 (Vigna &
/// Blackman, the generator `java.util.SplitableRandom` is built on). Chosen
/// over a `rand` crate dependency: every caller here needs exactly "the
/// same seed produces the same sequence, on every platform, forever", never
/// external entropy or cryptographic strength, and splitmix64 is a dozen
/// lines with a public-domain reference implementation, which is the whole
/// requirement.
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A uniformly distributed index in `0..n`. Callers never invoke this
    /// against an empty slice -- both [`forecast_completion`] and
    /// [`forecast_metric`] check that before sampling begins and return a
    /// `reason` instead.
    fn next_index(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

/// A stable, non-cryptographic 64-bit seed from a scenario's own identity
/// plus whatever it is forecasting (its name, and e.g. the key result or
/// metric being projected) -- the first 8 bytes of SHA-256 over the parts
/// joined with a NUL separator. Not `std::hash::DefaultHasher`: its
/// algorithm is explicitly unspecified and may change between Rust
/// releases, which would silently break "same seed forever" on a toolchain
/// upgrade; `sha2` is already a `factory-core` dependency (used elsewhere in
/// the crate), so this adds no new one.
pub fn seed_from(parts: &[&str]) -> u64 {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for p in parts {
        hasher.update(p.as_bytes());
        hasher.update([0u8]);
    }
    let digest = hasher.finalize();
    u64::from_le_bytes(digest[0..8].try_into().unwrap())
}

/// Nearest-rank percentile over an already-sorted, non-empty slice:
/// `index = round(p * (len - 1))`, clamped into range. Monotone in `p` --
/// `p * (len - 1)` is monotone in `p`, and rounding preserves monotonicity
/// -- which is what keeps p10 <= p50 <= p90 a guarantee rather than a
/// coincidence of the data (pinned directly by a test below).
fn nearest_rank(len: usize, p: f64) -> usize {
    if len == 0 {
        return 0;
    }
    ((p * (len - 1) as f64).round() as usize).min(len - 1)
}

fn percentile_f64(sorted: &[f64], p: f64) -> f64 {
    sorted[nearest_rank(sorted.len(), p)]
}

/// Same idea, over completion weeks sorted ascending with every `None`
/// ("never completed within the horizon") placed after every `Some` --
/// "never" is the worst outcome a percentile can name, so it sorts last.
fn percentile_week(raw: &[Option<u32>], p: f64) -> Option<u32> {
    let mut sorted: Vec<Option<u32>> = raw.to_vec();
    sorted.sort_by(|a, b| match (a, b) {
        (Some(x), Some(y)) => x.cmp(y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    sorted[nearest_rank(sorted.len(), p)]
}

/// p10/p50/p90 of some quantity -- never a single number, the issue's own
/// guardrail against a forecast reading as false precision.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Percentiles<T> {
    pub p10: T,
    pub p50: T,
    pub p90: T,
}

/// A Monte Carlo forecast of when `backlog` clears, bootstrap-sampled from
/// `weekly_throughput_history` (Magennis's own method: draw one historical
/// week's throughput, with replacement, per simulated week) -- see
/// [`forecast_completion`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Forecast {
    /// One entry per week of the horizon (`per_week[0]` is week 1): the
    /// cumulative amount of backlog done by that week, as percentiles
    /// across every sample -- a fan chart's raw material. Empty when
    /// `reason` is set.
    pub per_week: Vec<Percentiles<f64>>,
    /// Which week `backlog` is cleared. `None` for a sample that never
    /// clears it within the horizon; see [`percentile_week`] for how that
    /// sorts.
    pub completion_week: Percentiles<Option<u32>>,
    pub samples: u32,
    pub seed: u64,
    /// Set, with `per_week` empty and `completion_week` all `None`, when
    /// there is nothing to sample from -- see [`forecast_completion`]. Never
    /// a `NaN` or an empty-but-unexplained result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The fraction of samples that had cleared `backlog` by week `w`, for
    /// `w` in `0..=horizon_weeks` -- [`Forecast::probability_done_by`]
    /// indexes into this rather than closing over anything, so the whole
    /// struct stays a plain, serializable value with no function pointers
    /// or closures hiding in it. Not itself part of the wire shape: a
    /// caller wanting the curve calls `probability_done_by` at each week
    /// instead of reading this vector's internal indexing.
    #[serde(skip)]
    done_by_week: Vec<f64>,
}

impl Forecast {
    /// The fraction of samples that had cleared the backlog by `week`.
    /// `None` only when there is no data at all (`reason` is set). A `week`
    /// beyond the horizon reads as the horizon's own last value -- a floor,
    /// since completing by a later week can only be as-or-more likely than
    /// completing by the last week actually sampled, never less.
    pub fn probability_done_by(&self, week: u32) -> Option<f64> {
        if self.done_by_week.is_empty() {
            return None;
        }
        let idx = (week as usize).min(self.done_by_week.len() - 1);
        Some(self.done_by_week[idx])
    }
}

/// Bootstrap-sample `weekly_throughput_history` `samples` times over
/// `horizon_weeks`, seeded so the same inputs always produce the same
/// [`Forecast`] (see the module doc comment's "Determinism" section).
///
/// `backlog <= 0.0` is already done -- every sample's `completion_week` is
/// `Some(0)`. An empty history, zero `samples`, or a zero-week horizon is a
/// `reason`, not a computed-but-empty result and never a `NaN`; a history
/// entirely of zeros is *not* one of those cases (it is valid input meaning
/// "no throughput at all recently") and instead naturally produces
/// `completion_week: None` throughout whenever `backlog > 0.0` -- nothing
/// ever gets done, which is the honest answer, not an error.
pub fn forecast_completion(weekly_throughput_history: &[f64], backlog: f64, horizon_weeks: u32, samples: u32, seed: u64) -> Forecast {
    let bail = |reason: &str| Forecast {
        per_week: Vec::new(),
        completion_week: Percentiles { p10: None, p50: None, p90: None },
        samples,
        seed,
        reason: Some(reason.to_string()),
        done_by_week: Vec::new(),
    };

    if samples == 0 {
        return bail("zero samples requested");
    }
    if horizon_weeks == 0 {
        return bail("zero-week horizon");
    }
    if weekly_throughput_history.is_empty() {
        return bail("no throughput history to forecast from");
    }
    if weekly_throughput_history.iter().any(|v| !v.is_finite() || *v < 0.0) {
        return bail("throughput history contains a negative or non-finite value");
    }

    let mut rng = SplitMix64::new(seed);
    let horizon = horizon_weeks as usize;
    let mut cumulative: Vec<Vec<f64>> = Vec::with_capacity(samples as usize);
    let mut completion_weeks: Vec<Option<u32>> = Vec::with_capacity(samples as usize);

    for _ in 0..samples {
        let mut done = 0.0;
        let mut row = Vec::with_capacity(horizon);
        let mut completed_at = if backlog <= 0.0 { Some(0) } else { None };
        for week in 1..=horizon_weeks {
            let draw = weekly_throughput_history[rng.next_index(weekly_throughput_history.len())];
            done += draw;
            row.push(done);
            if completed_at.is_none() && done >= backlog {
                completed_at = Some(week);
            }
        }
        cumulative.push(row);
        completion_weeks.push(completed_at);
    }

    let mut per_week = Vec::with_capacity(horizon);
    for w in 0..horizon {
        let mut vals: Vec<f64> = cumulative.iter().map(|row| row[w]).collect();
        vals.sort_by(f64::total_cmp);
        per_week.push(Percentiles {
            p10: percentile_f64(&vals, 0.10),
            p50: percentile_f64(&vals, 0.50),
            p90: percentile_f64(&vals, 0.90),
        });
    }

    let completion_week = Percentiles {
        p10: percentile_week(&completion_weeks, 0.10),
        p50: percentile_week(&completion_weeks, 0.50),
        p90: percentile_week(&completion_weeks, 0.90),
    };

    // `done_by_week[w]` = the fraction of samples with `completion_week <=
    // w`, built as a histogram (one bump at each sample's own completion
    // week) then a running cumulative sum, rather than an O(samples *
    // horizon) scan.
    let mut done_by_week = vec![0.0; horizon + 1];
    for w in completion_weeks.iter().flatten() {
        done_by_week[*w as usize] += 1.0;
    }
    let mut running = 0.0;
    for slot in &mut done_by_week {
        running += *slot;
        *slot = running / samples as f64;
    }

    Forecast { per_week, completion_week, samples, seed, reason: None, done_by_week }
}

/// A smaller cousin of [`Forecast`] for a metric's own trend -- no backlog
/// to clear, so no `completion_week` or `probability_done_by`, just a fan
/// chart of the projected value. See [`forecast_metric`] and deviation 6 in
/// the module doc comment.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MetricForecast {
    pub metric: MetricId,
    /// One entry per week of the horizon, the projected value's percentiles
    /// across every sample. Empty when `reason` is set.
    pub per_week: Vec<Percentiles<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Project `series` forward by bootstrap-resampling its own historical
/// day-over-day deltas (the same Magennis-style method [`forecast_completion`]
/// uses on weekly throughput, applied to a metric's daily points instead) --
/// optional and deliberately cheap, per the issue's own "if cheap" framing.
/// Needs at least two points to have a single delta to sample from;
/// fewer -- or `samples`/`horizon_weeks` of zero -- is a `reason`, the same
/// discipline `forecast_completion` holds.
pub fn forecast_metric(series: &MetricSeries, horizon_weeks: u32, samples: u32, seed: u64) -> MetricForecast {
    if samples == 0 || horizon_weeks == 0 || series.points.len() < 2 {
        return MetricForecast {
            metric: series.id.clone(),
            per_week: Vec::new(),
            reason: Some("not enough history to forecast a trend from".to_string()),
        };
    }

    let deltas: Vec<f64> = series.points.windows(2).map(|w| w[1].1 - w[0].1).collect();
    let last_value = series.points.last().unwrap().1;
    let mut rng = SplitMix64::new(seed);
    let horizon = horizon_weeks as usize;
    let mut per_sample: Vec<Vec<f64>> = Vec::with_capacity(samples as usize);

    for _ in 0..samples {
        let mut value = last_value;
        let mut row = Vec::with_capacity(horizon);
        for _week in 0..horizon_weeks {
            for _day in 0..7 {
                value += deltas[rng.next_index(deltas.len())];
            }
            row.push(value);
        }
        per_sample.push(row);
    }

    let mut per_week = Vec::with_capacity(horizon);
    for w in 0..horizon {
        let mut vals: Vec<f64> = per_sample.iter().map(|r| r[w]).collect();
        vals.sort_by(f64::total_cmp);
        per_week.push(Percentiles {
            p10: percentile_f64(&vals, 0.10),
            p50: percentile_f64(&vals, 0.50),
            p90: percentile_f64(&vals, 0.90),
        });
    }

    MetricForecast { metric: series.id.clone(), per_week, reason: None }
}

// ================================================================= drivers

pub type DriverId = String;
pub type OutcomeId = String;

/// One built-in driver's definition -- what it means, its unit, and whether
/// it has a registry metric behind it or is purely a person's assumption.
/// See deviation 5 in the module doc comment for why this exact set.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct DriverDef {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub unit: &'static str,
    /// `true` when nothing computes this driver's baseline -- it is always
    /// a person's own number, never a registry metric's value. Marked
    /// exactly the way `metrics::MetricDef::available` marks a metric
    /// `factory-daemon` cannot compute yet, for the same reason: so a
    /// caller never confuses "the company's own history says so" with
    /// "someone typed a guess".
    pub assumption: bool,
    /// The registry metric this driver's baseline would come from, when it
    /// has one at all (`None` for every `assumption: true` driver).
    pub metric: Option<&'static str>,
}

/// The v1 built-in driver set, tied to `crate::metrics`'s registry where one
/// exists.
pub fn driver_defs() -> Vec<DriverDef> {
    vec![
        DriverDef {
            id: "throughput_week",
            title: "Throughput per week",
            description: "Finished runs in the trailing 7 days -- the same registry metric of the same name.",
            unit: "per_week",
            assumption: false,
            metric: Some("throughput_week"),
        },
        DriverDef {
            id: "first_pass_yield",
            title: "First-pass yield",
            description: "1 minus reworked/finished, over the trailing 28 days -- the same registry metric of the same name.",
            unit: "ratio",
            assumption: false,
            metric: Some("first_pass_yield"),
        },
        DriverDef {
            id: "scrap_rate",
            title: "Scrap rate",
            description: "scrapped/finished, over the trailing 28 days -- the same registry metric of the same name.",
            unit: "ratio",
            assumption: false,
            metric: Some("scrap_rate"),
        },
        DriverDef {
            id: "rework_rate",
            title: "Rework rate",
            description: "No registry metric backs this one directly: `first_pass_yield` is already \
                           `1 - reworked/finished`, so a `rework_rate` driver is a deliberately separate, \
                           author-supplied assumption rather than a second computation of the same \
                           underlying fraction -- never confused with a registry-derived number.",
            unit: "ratio",
            assumption: true,
            metric: None,
        },
        DriverDef {
            id: "capacity_factor",
            title: "Capacity factor",
            description: "A multiplier on effective throughput with no data source at all -- a person's \
                           own what-if (e.g. 'a reviewer goes part-time'), the issue's own example of an \
                           assumption driver.",
            unit: "multiplier",
            assumption: true,
            metric: None,
        },
    ]
}

/// A parsed `drivers:` override. See [`parse_override`] for the authored
/// syntax and [`apply_overrides`] for how each variant combines with a
/// baseline value.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Override {
    /// `×2` / `x2` / `X2` -- multiply the baseline by this factor.
    Multiply(f64),
    /// `+20%` / `-20%` -- a fractional change (`0.20`/`-0.20`) applied to
    /// the baseline: `baseline * (1.0 + change)`.
    PercentChange(f64),
    /// `+5` / `-5` (quoted, see deviation 4) -- add this amount to the
    /// baseline.
    Delta(f64),
    /// `=0.9` -- replace the baseline outright, the only variant that
    /// stands alone without one (see [`apply_overrides`]).
    Set(f64),
}

/// Parse one authored override string. Recognises, in this order: `=N`
/// (`Set`), `×N`/`xN`/`XN` (`Multiply`), `+N%`/`-N%` (`PercentChange`, `N`
/// itself signed), then a bare `+N`/`-N` (`Delta`). Anything else --
/// including a bare number with no sign, which is ambiguous between a
/// `Delta` and a `Set` -- is refused.
pub fn parse_override(raw: &str) -> std::result::Result<Override, String> {
    let s = raw.trim();
    let bad = || format!("{raw:?} is not a valid override: expected ×N, xN, +N%, -N%, +N, -N, or =N");

    if let Some(rest) = s.strip_prefix('=') {
        let v: f64 = rest.trim().parse().map_err(|_| bad())?;
        return Ok(Override::Set(v));
    }
    if let Some(rest) = s.strip_prefix('×').or_else(|| s.strip_prefix(['x', 'X'])) {
        let v: f64 = rest.trim().parse().map_err(|_| bad())?;
        return Ok(Override::Multiply(v));
    }
    if let Some(rest) = s.strip_suffix('%') {
        if !(rest.starts_with('+') || rest.starts_with('-')) {
            return Err(format!("{raw:?} is not a valid override: a percent change must start with + or -"));
        }
        let v: f64 = rest.parse().map_err(|_| bad())?;
        return Ok(Override::PercentChange(v / 100.0));
    }
    if s.starts_with('+') || s.starts_with('-') {
        let v: f64 = s.parse().map_err(|_| bad())?;
        return Ok(Override::Delta(v));
    }
    Err(bad())
}

/// Apply every override in `overrides` to `baseline`, driver by driver.
/// `Set` always applies, even for a driver absent from `baseline` -- it
/// needs no baseline to stand as a pure assumption. Every other variant is a
/// *relative* change and is silently skipped for a driver `baseline` does
/// not carry a value for (there is nothing to be relative to); `load`'s own
/// [`FindingKind::UnknownDriver`] is what catches an unknown driver id in
/// the first place, so this does not re-report it.
pub fn apply_overrides(baseline: &BTreeMap<DriverId, f64>, overrides: &BTreeMap<DriverId, Override>) -> BTreeMap<DriverId, f64> {
    let mut out = baseline.clone();
    for (id, ov) in overrides {
        match ov {
            Override::Set(v) => {
                out.insert(id.clone(), *v);
            }
            Override::Multiply(m) => {
                if let Some(b) = baseline.get(id) {
                    out.insert(id.clone(), b * m);
                }
            }
            Override::PercentChange(p) => {
                if let Some(b) = baseline.get(id) {
                    out.insert(id.clone(), b * (1.0 + p));
                }
            }
            Override::Delta(d) => {
                if let Some(b) = baseline.get(id) {
                    out.insert(id.clone(), b + d);
                }
            }
        }
    }
    out
}

/// `effective_throughput = throughput_week × capacity_factor × first_pass_yield`
/// -- the one outcome formula the issue names. Missing drivers default to
/// neutral: `throughput_week` absent reads as `0.0` (no data, no output);
/// `capacity_factor`/`first_pass_yield` absent read as `1.0` (no adjustment)
/// -- documented here rather than silently varying with whatever `values`
/// happens to carry.
pub fn evaluate_outcomes(values: &BTreeMap<DriverId, f64>) -> BTreeMap<OutcomeId, f64> {
    let get = |id: &str, default: f64| values.get(id).copied().unwrap_or(default);
    let effective_throughput = get("throughput_week", 0.0) * get("capacity_factor", 1.0) * get("first_pass_yield", 1.0);
    let mut out = BTreeMap::new();
    out.insert("effective_throughput".to_string(), effective_throughput);
    out
}

/// Weeks to clear `backlog` at `effective_throughput` -- its own function
/// rather than a key in [`evaluate_outcomes`]'s map; see deviation 3 in the
/// module doc comment. `backlog <= 0.0` is already clear (`Some(0.0)`);
/// `effective_throughput <= 0.0` never clears it (`None`, not `f64::INFINITY`,
/// which `serde_json` would render indistinguishably from "missing").
pub fn weeks_to_clear(effective_throughput: f64, backlog: f64) -> Option<f64> {
    if backlog <= 0.0 {
        return Some(0.0);
    }
    if effective_throughput <= 0.0 {
        return None;
    }
    Some(backlog / effective_throughput)
}

/// One driver's swing in a tornado chart: the outcome at `low`/`high` and
/// the span between them.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TornadoBar {
    pub driver: DriverId,
    pub low_outcome: f64,
    pub high_outcome: f64,
    pub span: f64,
}

/// Vary each driver in `baseline` by `±swing` (e.g. `0.2` for ±20%) in
/// turn, holding every other driver fixed, and rank by the effect on
/// `outcome_id` -- the standard one-at-a-time tornado sensitivity method.
/// Sorted by `span` descending; ties break on `driver` ascending, so the
/// order is deterministic even when two drivers swing an outcome by exactly
/// the same amount.
pub fn tornado(baseline: &BTreeMap<DriverId, f64>, outcome_id: &str, swing: f64) -> Vec<TornadoBar> {
    let mut bars: Vec<TornadoBar> = baseline
        .iter()
        .map(|(driver, &value)| {
            let mut low_map = baseline.clone();
            let mut high_map = baseline.clone();
            low_map.insert(driver.clone(), value * (1.0 - swing));
            high_map.insert(driver.clone(), value * (1.0 + swing));
            let a = evaluate_outcomes(&low_map).get(outcome_id).copied().unwrap_or(0.0);
            let b = evaluate_outcomes(&high_map).get(outcome_id).copied().unwrap_or(0.0);
            let (low, high) = (a.min(b), a.max(b));
            TornadoBar { driver: driver.clone(), low_outcome: low, high_outcome: high, span: high - low }
        })
        .collect();
    bars.sort_by(|a, b| b.span.partial_cmp(&a.span).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.driver.cmp(&b.driver)));
    bars
}

// ========================================================= goal scenarios

/// Whether a key result's `value` behaves like a backlog to clear (a count
/// that only grows, e.g. `goal_tasks_done.*`) or not. Only `CountLike` ever
/// forecasts a probability -- see [`goal_probability`]. A caller picks this
/// from the key result's own metric: `CountLike` only when
/// `metrics::MetricDef::unit` is `Unit::Count` *and* the changed `target` is
/// greater than the current value (a lower-is-better count, e.g.
/// `open_controls.<framework>`, is not a backlog either, and gets no rate
/// model here); every ratio metric (`compliance.*`, `first_pass_yield`, ...)
/// is `Ratio`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KrShape {
    CountLike,
    Ratio,
}

/// A key result's probability of reaching a changed target by a changed
/// deadline -- see [`goal_probability`]. `probability: None` is always
/// paired with a `reason`; the issue's own rule ("no fake numbers") means
/// this is never guessed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GoalProbability {
    pub probability: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Re-score a key result against a changed `target`/`by`, using a Monte
/// Carlo forecast of `weekly_throughput_history` for `shape: CountLike` --
/// there is no rate model for a `Ratio` key result (no registry metric
/// projects a ratio's future value from a throughput history the way a
/// count's remaining distance can be), so that case always returns `None`
/// with a reason. `now` and `by` are both read as start-of-day UTC, the
/// same convention `goals::Cycle::bounds` and [`evaluate_signposts`] use.
#[allow(clippy::too_many_arguments)]
pub fn goal_probability(
    shape: KrShape,
    current_value: f64,
    target: f64,
    by: NaiveDate,
    now: DateTime<Utc>,
    weekly_throughput_history: &[f64],
    samples: u32,
    seed: u64,
) -> GoalProbability {
    if shape == KrShape::Ratio {
        return GoalProbability {
            probability: None,
            reason: Some(
                "ratio key results have no rate model to forecast a completion probability from; \
                 record a check-in, or use a driver what-if instead"
                    .to_string(),
            ),
        };
    }

    if target <= current_value {
        return GoalProbability { probability: Some(1.0), reason: Some("target already reached".to_string()) };
    }

    let today = now.date_naive();
    if by <= today {
        return GoalProbability {
            probability: Some(0.0),
            reason: Some(format!("deadline {by} has already passed without reaching the target")),
        };
    }

    let days = (by - today).num_days();
    let weeks = (days as f64 / 7.0).ceil() as u32;
    let needed = target - current_value;

    let forecast = forecast_completion(weekly_throughput_history, needed, weeks, samples, seed);
    match forecast.probability_done_by(weeks) {
        Some(p) => GoalProbability { probability: Some(p), reason: None },
        None => GoalProbability { probability: None, reason: forecast.reason },
    }
}

// ================================================================ signposts

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignpostState {
    /// Active and within bounds.
    Quiet,
    /// Active and past a threshold.
    Triggered,
    /// Before its own `from` date.
    NotYetActive,
    /// Active, but there is no metric value to check it against.
    NoData,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SignpostStatus {
    pub metric: MetricId,
    pub state: SignpostState,
    pub reason: String,
}

/// Evaluate every signpost against `values` (already-computed metric
/// values, `factory-daemon`'s job to gather, exactly like `goals::evaluate`
/// takes them) as of `now`. Preserves `signposts`' own order -- like an
/// objective's key results in `goals.rs`, that order is itself information
/// (an author's own priority), not something to sort away.
///
/// `below`/`above` are both checked when both are set -- `Triggered` if
/// either is breached -- rather than one refusing the other, since a
/// two-sided band (e.g. "watch if this share leaves 0.4..0.8") is a
/// legitimate signpost, not an authoring mistake; [`load`] never requires
/// exactly one, only that at least one is set
/// ([`FindingKind::SignpostMissingThreshold`]). Both comparisons are
/// strict (`<`/`>`): a value sitting exactly on a threshold reads `Quiet`,
/// the same "not yet past it" reading `within_max_age`'s own `<=` gives a
/// control right at its freshness window in `policy.rs`.
pub fn evaluate_signposts(signposts: &[Signpost], values: &BTreeMap<MetricId, MetricValue>, now: DateTime<Utc>) -> Vec<SignpostStatus> {
    signposts.iter().map(|sp| evaluate_signpost(sp, values, now)).collect()
}

fn evaluate_signpost(sp: &Signpost, values: &BTreeMap<MetricId, MetricValue>, now: DateTime<Utc>) -> SignpostStatus {
    if let Some(from) = sp.from {
        let start = from.and_hms_opt(0, 0, 0).unwrap().and_utc();
        if now < start {
            return SignpostStatus { metric: sp.metric.clone(), state: SignpostState::NotYetActive, reason: format!("active from {from}") };
        }
    }

    let Some(mv) = values.get(&sp.metric) else {
        return SignpostStatus { metric: sp.metric.clone(), state: SignpostState::NoData, reason: "no metric value supplied".to_string() };
    };
    let Some(value) = mv.value else {
        let reason = mv.reason.clone().unwrap_or_else(|| "no reason given".to_string());
        return SignpostStatus { metric: sp.metric.clone(), state: SignpostState::NoData, reason };
    };

    let below_hit = sp.below.is_some_and(|t| value < t);
    let above_hit = sp.above.is_some_and(|t| value > t);
    if below_hit || above_hit {
        let mut parts = Vec::new();
        if below_hit {
            parts.push(format!("{value} is below {}", sp.below.unwrap()));
        }
        if above_hit {
            parts.push(format!("{value} is above {}", sp.above.unwrap()));
        }
        SignpostStatus { metric: sp.metric.clone(), state: SignpostState::Triggered, reason: parts.join("; ") }
    } else {
        SignpostStatus { metric: sp.metric.clone(), state: SignpostState::Quiet, reason: format!("{value} within bounds") }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{Catalogue, Check, Control, Duration as PDuration, Evidence, Kind, NotApplicable, Tighten};

    fn write(dir: &Path, name: &str, text: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), text).unwrap();
    }

    fn tempdir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("factory-scenario-test-{label}-{}", uuid::Uuid::new_v4()))
    }

    fn dt(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(0, 0, 0).unwrap().and_utc()
    }

    fn nd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    // -- Horizon -----------------------------------------------------------

    #[test]
    fn horizon_parses_weeks_days_and_hours_and_defaults_to_26w() {
        assert_eq!("26w".parse::<Horizon>().unwrap().weeks(), 26);
        assert_eq!("182d".parse::<Horizon>().unwrap().weeks(), 26);
        assert_eq!(Horizon::default().weeks(), 26);
        assert_eq!(Horizon::default().to_string(), "26w");
        // A duration that doesn't land on a whole week rounds up.
        assert_eq!("8d".parse::<Horizon>().unwrap().weeks(), 2);
    }

    #[test]
    fn horizon_round_trips_through_serde() {
        let h: Horizon = "13w".parse().unwrap();
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(json, "\"13w\"");
    }

    // -- RawOverride: the YAML quoting trap ---------------------------------

    #[test]
    fn an_unquoted_signed_number_deserializes_as_a_number_not_text() {
        // Deviation 4 in the module doc comment, verified empirically rather
        // than assumed: YAML reads a bare +5/-5 as an integer scalar.
        let v: RawOverride = serde_yaml_ng::from_str("+5").unwrap();
        assert_eq!(v, RawOverride::Number(5.0));
        let v: RawOverride = serde_yaml_ng::from_str("-5").unwrap();
        assert_eq!(v, RawOverride::Number(-5.0));
    }

    #[test]
    fn a_quoted_override_deserializes_as_text() {
        for raw in ["\"+5\"", "\"-20%\"", "\"\u{d7}2\"", "\"=0.9\"", "\"x2\""] {
            let v: RawOverride = serde_yaml_ng::from_str(raw).unwrap();
            assert!(matches!(v, RawOverride::Text(_)), "{raw} did not deserialize as Text: {v:?}");
        }
    }

    // -- parse_override ------------------------------------------------------

    #[test]
    fn parse_override_reads_multiply_in_every_spelling() {
        assert_eq!(parse_override("\u{d7}2").unwrap(), Override::Multiply(2.0));
        assert_eq!(parse_override("x2").unwrap(), Override::Multiply(2.0));
        assert_eq!(parse_override("X3.5").unwrap(), Override::Multiply(3.5));
    }

    #[test]
    fn parse_override_reads_percent_change_signed() {
        assert_eq!(parse_override("+20%").unwrap(), Override::PercentChange(0.20));
        assert_eq!(parse_override("-20%").unwrap(), Override::PercentChange(-0.20));
    }

    #[test]
    fn parse_override_reads_delta() {
        assert_eq!(parse_override("+5").unwrap(), Override::Delta(5.0));
        assert_eq!(parse_override("-5").unwrap(), Override::Delta(-5.0));
    }

    #[test]
    fn parse_override_reads_set() {
        assert_eq!(parse_override("=0.9").unwrap(), Override::Set(0.9));
    }

    #[test]
    fn parse_override_refuses_bad_syntax() {
        for bad in ["", "5", "20%", "x", "=", "+", "banana", "+5%%"] {
            assert!(parse_override(bad).is_err(), "{bad:?} should not parse");
        }
    }

    // -- apply_overrides ------------------------------------------------------

    #[test]
    fn apply_overrides_combines_every_variant_with_a_baseline() {
        let baseline: BTreeMap<DriverId, f64> =
            [("throughput_week".to_string(), 10.0), ("capacity_factor".to_string(), 1.0), ("scrap_rate".to_string(), 0.1)]
                .into_iter()
                .collect();
        let overrides: BTreeMap<DriverId, Override> = [
            ("throughput_week".to_string(), Override::Multiply(2.0)),
            ("capacity_factor".to_string(), Override::Delta(-0.2)),
            ("scrap_rate".to_string(), Override::PercentChange(0.5)),
        ]
        .into_iter()
        .collect();
        let out = apply_overrides(&baseline, &overrides);
        assert_eq!(out["throughput_week"], 20.0);
        assert_eq!(out["capacity_factor"], 0.8);
        assert!((out["scrap_rate"] - 0.15).abs() < 1e-9);
    }

    #[test]
    fn set_applies_even_without_a_baseline_but_relative_overrides_are_skipped() {
        let baseline: BTreeMap<DriverId, f64> = BTreeMap::new();
        let overrides: BTreeMap<DriverId, Override> =
            [("unit_cost".to_string(), Override::Set(3.0)), ("throughput_week".to_string(), Override::Multiply(2.0))]
                .into_iter()
                .collect();
        let out = apply_overrides(&baseline, &overrides);
        assert_eq!(out.get("unit_cost"), Some(&3.0));
        assert_eq!(out.get("throughput_week"), None);
    }

    // -- evaluate_outcomes / weeks_to_clear ------------------------------------

    #[test]
    fn evaluate_outcomes_multiplies_throughput_by_capacity_and_yield() {
        let values: BTreeMap<DriverId, f64> = [
            ("throughput_week".to_string(), 10.0),
            ("capacity_factor".to_string(), 0.8),
            ("first_pass_yield".to_string(), 0.9),
        ]
        .into_iter()
        .collect();
        let out = evaluate_outcomes(&values);
        assert!((out["effective_throughput"] - 7.2).abs() < 1e-9);
    }

    #[test]
    fn evaluate_outcomes_defaults_missing_drivers_neutrally() {
        let out = evaluate_outcomes(&BTreeMap::new());
        assert_eq!(out["effective_throughput"], 0.0); // no throughput_week -> zero output
    }

    #[test]
    fn weeks_to_clear_handles_zero_throughput_and_zero_backlog() {
        assert_eq!(weeks_to_clear(0.0, 10.0), None);
        assert_eq!(weeks_to_clear(5.0, 0.0), Some(0.0));
        assert_eq!(weeks_to_clear(5.0, -3.0), Some(0.0));
        assert_eq!(weeks_to_clear(5.0, 10.0), Some(2.0));
    }

    // -- tornado ---------------------------------------------------------------

    #[test]
    fn tornado_sorts_by_span_descending_with_deterministic_ties() {
        let baseline: BTreeMap<DriverId, f64> = [
            ("throughput_week".to_string(), 10.0),
            ("capacity_factor".to_string(), 1.0),
            ("first_pass_yield".to_string(), 1.0),
        ]
        .into_iter()
        .collect();
        let bars = tornado(&baseline, "effective_throughput", 0.2);
        assert_eq!(bars.len(), 3);
        for w in bars.windows(2) {
            assert!(w[0].span >= w[1].span, "{bars:?} is not sorted by span descending");
        }
        // throughput_week and first_pass_yield both swing the outcome by the
        // same ±20% off the same baseline (10.0 * 1.0 * 1.0) -- a tie, broken
        // by driver id ascending.
        let tied: Vec<&str> = bars.iter().filter(|b| (b.span - bars[0].span).abs() < 1e-9).map(|b| b.driver.as_str()).collect();
        assert!(tied.contains(&"first_pass_yield") && tied.contains(&"throughput_week"));
        let names: Vec<&str> = bars.iter().map(|b| b.driver.as_str()).collect();
        assert!(names.iter().position(|n| *n == "first_pass_yield") < names.iter().position(|n| *n == "throughput_week"));
    }

    // -- signposts ---------------------------------------------------------------

    fn mv(id: &str, value: Option<f64>, reason: Option<&str>) -> (MetricId, MetricValue) {
        let mid = MetricId::new(id).unwrap();
        (mid.clone(), MetricValue { id: mid, value, as_of: dt(2026, 9, 25), reason: reason.map(str::to_string) })
    }

    #[test]
    fn signpost_states_cover_quiet_triggered_not_yet_active_and_no_data() {
        let below = Signpost { metric: MetricId::new("throughput_week").unwrap(), below: Some(8.0), above: None, from: None };
        let values: BTreeMap<MetricId, MetricValue> = [mv("throughput_week", Some(10.0), None)].into_iter().collect();
        assert_eq!(evaluate_signposts(std::slice::from_ref(&below), &values, dt(2026, 9, 25))[0].state, SignpostState::Quiet);

        let values: BTreeMap<MetricId, MetricValue> = [mv("throughput_week", Some(5.0), None)].into_iter().collect();
        assert_eq!(evaluate_signposts(std::slice::from_ref(&below), &values, dt(2026, 9, 25))[0].state, SignpostState::Triggered);

        let future = Signpost { metric: MetricId::new("throughput_week").unwrap(), below: Some(8.0), above: None, from: Some(nd(2027, 1, 1)) };
        assert_eq!(evaluate_signposts(&[future], &values, dt(2026, 9, 25))[0].state, SignpostState::NotYetActive);

        let no_data = Signpost { metric: MetricId::new("scrap_rate").unwrap(), below: Some(0.1), above: None, from: None };
        assert_eq!(evaluate_signposts(&[no_data], &BTreeMap::new(), dt(2026, 9, 25))[0].state, SignpostState::NoData);

        let unavailable_values: BTreeMap<MetricId, MetricValue> = [mv("throughput_week", None, Some("no data this week"))].into_iter().collect();
        let status = &evaluate_signposts(&[below], &unavailable_values, dt(2026, 9, 25))[0];
        assert_eq!(status.state, SignpostState::NoData);
        assert_eq!(status.reason, "no data this week");
    }

    #[test]
    fn a_two_sided_signpost_triggers_on_either_bound() {
        let sp = Signpost { metric: MetricId::new("first_pass_yield").unwrap(), below: Some(0.4), above: Some(0.8), from: None };
        let low: BTreeMap<MetricId, MetricValue> = [mv("first_pass_yield", Some(0.2), None)].into_iter().collect();
        let high: BTreeMap<MetricId, MetricValue> = [mv("first_pass_yield", Some(0.95), None)].into_iter().collect();
        let mid: BTreeMap<MetricId, MetricValue> = [mv("first_pass_yield", Some(0.6), None)].into_iter().collect();
        assert_eq!(evaluate_signposts(std::slice::from_ref(&sp), &low, dt(2026, 9, 25))[0].state, SignpostState::Triggered);
        assert_eq!(evaluate_signposts(std::slice::from_ref(&sp), &high, dt(2026, 9, 25))[0].state, SignpostState::Triggered);
        assert_eq!(evaluate_signposts(&[sp], &mid, dt(2026, 9, 25))[0].state, SignpostState::Quiet);
    }

    #[test]
    fn signpost_evaluation_preserves_authored_order() {
        let sps = vec![
            Signpost { metric: MetricId::new("scrap_rate").unwrap(), below: None, above: Some(0.5), from: None },
            Signpost { metric: MetricId::new("throughput_week").unwrap(), below: Some(1.0), above: None, from: None },
        ];
        let out = evaluate_signposts(&sps, &BTreeMap::new(), dt(2026, 9, 25));
        assert_eq!(out[0].metric.as_str(), "scrap_rate");
        assert_eq!(out[1].metric.as_str(), "throughput_week");
    }

    // -- goal_probability ---------------------------------------------------

    #[test]
    fn ratio_key_results_return_none_with_a_reason_never_a_fake_number() {
        let result = goal_probability(KrShape::Ratio, 0.5, 1.0, nd(2027, 1, 1), dt(2026, 9, 25), &[5.0, 6.0, 7.0], 200, 1);
        assert_eq!(result.probability, None);
        assert!(result.reason.is_some());
    }

    #[test]
    fn a_count_like_kr_already_at_target_is_certain() {
        let result = goal_probability(KrShape::CountLike, 10.0, 10.0, nd(2027, 1, 1), dt(2026, 9, 25), &[5.0], 200, 1);
        assert_eq!(result.probability, Some(1.0));
    }

    #[test]
    fn a_count_like_kr_past_its_deadline_and_short_is_zero() {
        let result = goal_probability(KrShape::CountLike, 2.0, 10.0, nd(2026, 1, 1), dt(2026, 9, 25), &[5.0], 200, 1);
        assert_eq!(result.probability, Some(0.0));
        assert!(result.reason.unwrap().contains("passed"));
    }

    #[test]
    fn a_count_like_kr_forecasts_a_probability_in_bounds_and_is_deterministic() {
        let a = goal_probability(KrShape::CountLike, 0.0, 20.0, nd(2027, 3, 1), dt(2026, 9, 25), &[3.0, 4.0, 5.0, 4.0], 500, 42);
        let b = goal_probability(KrShape::CountLike, 0.0, 20.0, nd(2027, 3, 1), dt(2026, 9, 25), &[3.0, 4.0, 5.0, 4.0], 500, 42);
        assert_eq!(a, b);
        let p = a.probability.unwrap();
        assert!((0.0..=1.0).contains(&p), "{p} out of bounds");
    }

    // -- forecast_completion: determinism and monotone percentiles -------------

    #[test]
    fn same_seed_gives_identical_percentiles() {
        let a = forecast_completion(&[3.0, 5.0, 4.0, 6.0, 2.0], 40.0, 12, 500, 7);
        let b = forecast_completion(&[3.0, 5.0, 4.0, 6.0, 2.0], 40.0, 12, 500, 7);
        assert_eq!(a, b);
    }

    #[test]
    fn a_different_seed_can_give_a_different_forecast() {
        let a = forecast_completion(&[3.0, 5.0, 4.0, 6.0, 2.0], 40.0, 12, 500, 1);
        let b = forecast_completion(&[3.0, 5.0, 4.0, 6.0, 2.0], 40.0, 12, 500, 2);
        assert_ne!(a.per_week, b.per_week);
    }

    #[test]
    fn per_week_percentiles_are_p10_le_p50_le_p90_and_non_decreasing_across_weeks() {
        let f = forecast_completion(&[1.0, 2.0, 3.0, 4.0, 5.0], 1000.0, 20, 400, 99);
        for wk in &f.per_week {
            assert!(wk.p10 <= wk.p50, "{wk:?}");
            assert!(wk.p50 <= wk.p90, "{wk:?}");
        }
        for w in f.per_week.windows(2) {
            assert!(w[1].p10 >= w[0].p10);
            assert!(w[1].p50 >= w[0].p50);
            assert!(w[1].p90 >= w[0].p90);
        }
    }

    #[test]
    fn completion_week_percentiles_are_monotone_with_none_sorting_last() {
        // A short horizon against a large backlog: many samples never clear
        // it, so `p90` (and maybe `p50`) should read `None` while `p10` may
        // still be `Some`.
        let f = forecast_completion(&[1.0, 2.0], 1000.0, 4, 500, 3);
        let rank = |w: Option<u32>| w.map(i64::from).unwrap_or(i64::MAX);
        assert!(rank(f.completion_week.p10) <= rank(f.completion_week.p50));
        assert!(rank(f.completion_week.p50) <= rank(f.completion_week.p90));
    }

    #[test]
    fn empty_or_invalid_history_is_a_reason_never_a_nan() {
        let f = forecast_completion(&[], 10.0, 10, 100, 1);
        assert!(f.reason.is_some());
        assert!(f.per_week.is_empty());
        assert_eq!(f.probability_done_by(5), None);

        let f = forecast_completion(&[1.0, -2.0], 10.0, 10, 100, 1);
        assert!(f.reason.is_some());

        let f = forecast_completion(&[1.0, f64::NAN], 10.0, 10, 100, 1);
        assert!(f.reason.is_some());

        let f = forecast_completion(&[1.0], 10.0, 10, 0, 1);
        assert!(f.reason.is_some());

        let f = forecast_completion(&[1.0], 10.0, 0, 100, 1);
        assert!(f.reason.is_some());
    }

    #[test]
    fn an_all_zero_history_never_completes_a_positive_backlog() {
        let f = forecast_completion(&[0.0, 0.0, 0.0], 10.0, 8, 200, 5);
        assert!(f.reason.is_none());
        assert_eq!(f.completion_week, Percentiles { p10: None, p50: None, p90: None });
        assert_eq!(f.probability_done_by(8), Some(0.0));
        for wk in &f.per_week {
            assert_eq!(wk.p50, 0.0);
        }
    }

    #[test]
    fn a_backlog_already_cleared_completes_at_week_zero_for_every_sample() {
        let f = forecast_completion(&[3.0, 4.0], 0.0, 6, 100, 5);
        assert_eq!(f.completion_week, Percentiles { p10: Some(0), p50: Some(0), p90: Some(0) });
        assert_eq!(f.probability_done_by(0), Some(1.0));
    }

    #[test]
    fn probability_done_by_floors_at_the_horizon_for_a_later_week() {
        let f = forecast_completion(&[5.0, 5.0], 5.0, 3, 100, 5);
        // Every sample clears a backlog of 5 in week 1, deterministically
        // (constant history) -- probability at the horizon and well beyond
        // it should read the same.
        assert_eq!(f.probability_done_by(3), Some(1.0));
        assert_eq!(f.probability_done_by(300), f.probability_done_by(3));
    }

    // -- forecast_metric -----------------------------------------------------

    fn metric_series(values: &[f64]) -> MetricSeries {
        let start = nd(2026, 1, 1);
        MetricSeries {
            id: MetricId::new("throughput_week").unwrap(),
            points: values.iter().enumerate().map(|(i, v)| (start + chrono::Duration::days(i as i64), *v)).collect(),
        }
    }

    #[test]
    fn forecast_metric_is_deterministic_and_reasons_on_too_little_history() {
        let short = forecast_metric(&metric_series(&[1.0]), 4, 100, 1);
        assert!(short.reason.is_some());

        let series = metric_series(&[1.0, 1.2, 1.1, 1.3, 1.25, 1.4]);
        let a = forecast_metric(&series, 6, 300, 11);
        let b = forecast_metric(&series, 6, 300, 11);
        assert_eq!(a, b);
        assert!(a.reason.is_none());
        assert_eq!(a.per_week.len(), 6);
    }

    // -- seed_from -------------------------------------------------------------

    #[test]
    fn seed_from_is_deterministic_and_sensitive_to_its_parts() {
        assert_eq!(seed_from(&["ai-act-2027", "throughput_week"]), seed_from(&["ai-act-2027", "throughput_week"]));
        assert_ne!(seed_from(&["ai-act-2027", "throughput_week"]), seed_from(&["capacity-drop", "throughput_week"]));
    }

    // -- loader: parse/stem/kind/metric/signpost/driver/override findings ------

    #[test]
    fn a_file_that_fails_to_parse_is_a_finding_and_does_not_stop_the_others() {
        let dir = tempdir("bad-parse");
        write(&dir, "broken.yaml", "not: [valid, scenario\n");
        write(&dir, "ok.yaml", "name: ok\ntitle: Fine\n");
        let (scenarios, findings) = load(&dir);
        assert_eq!(scenarios.len(), 1);
        assert_eq!(scenarios[0].name, "ok");
        assert!(findings.iter().any(|f| f.kind == FindingKind::ParseFailed && f.subject == "broken.yaml"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_stem_mismatch_is_a_finding_and_the_file_is_dropped() {
        let dir = tempdir("stem-mismatch");
        write(&dir, "wrong-name.yaml", "name: right-name\ntitle: X\n");
        let (scenarios, findings) = load(&dir);
        assert!(scenarios.is_empty());
        assert!(findings.iter().any(|f| f.kind == FindingKind::StemMismatch));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_directory_is_empty_not_an_error() {
        let (scenarios, findings) = load(&tempdir("never-created"));
        assert!(scenarios.is_empty());
        assert!(findings.is_empty());
    }

    #[test]
    fn unknown_kind_is_a_finding_not_a_parse_failure() {
        let dir = tempdir("unknown-kind");
        write(&dir, "s.yaml", "name: s\ntitle: X\nkind: [polic]\n");
        let (scenarios, findings) = load(&dir);
        assert_eq!(scenarios.len(), 1, "the file still loads despite the bad kind entry");
        assert!(findings.iter().any(|f| f.kind == FindingKind::UnknownKind));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn signpost_findings_cover_unknown_unavailable_and_missing_threshold() {
        let dir = tempdir("signpost-findings");
        write(
            &dir,
            "s.yaml",
            "name: s\ntitle: X\nsignposts:\n  - { metric: not_a_real_metric }\n  - { metric: unit_cost, below: 1 }\n  - { metric: throughput_week }\n",
        );
        let (_scenarios, findings) = load(&dir);
        assert!(findings.iter().any(|f| f.kind == FindingKind::UnknownMetric));
        assert!(findings.iter().any(|f| f.kind == FindingKind::UnavailableMetric));
        assert!(findings.iter().any(|f| f.kind == FindingKind::SignpostMissingThreshold));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn driver_findings_cover_unknown_driver_and_bad_override_syntax() {
        let dir = tempdir("driver-findings");
        write(
            &dir,
            "s.yaml",
            "name: s\ntitle: X\ndrivers:\n  not_a_real_driver: \"=1\"\n  capacity_factor: \"banana\"\n  scrap_rate: +5\n",
        );
        let (_scenarios, findings) = load(&dir);
        assert!(findings.iter().any(|f| f.kind == FindingKind::UnknownDriver));
        assert_eq!(findings.iter().filter(|f| f.kind == FindingKind::BadOverride).count(), 2, "{findings:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_goal_change_naming_neither_target_nor_by_is_a_finding() {
        let dir = tempdir("no-op-goal");
        write(&dir, "s.yaml", "name: s\ntitle: X\ngoals:\n  - { kr: ship-compliant/cra-open-zero }\n");
        let (_scenarios, findings) = load(&dir);
        assert!(findings.iter().any(|f| f.kind == FindingKind::NoOpGoalChange));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn more_than_the_cap_is_a_single_finding_naming_every_scenario() {
        let dir = tempdir("too-many");
        for i in 0..5 {
            write(&dir, &format!("s{i}.yaml"), &format!("name: s{i}\ntitle: X\n"));
        }
        let (scenarios, findings) = load(&dir);
        assert_eq!(scenarios.len(), 5, "every file still loads even over the cap");
        let hits: Vec<&Finding> = findings.iter().filter(|f| f.kind == FindingKind::TooManyScenarios).collect();
        assert_eq!(hits.len(), 1);
        for i in 0..5 {
            assert!(hits[0].detail.contains(&format!("s{i}")), "{hits:?}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn findings_are_sorted_deterministically() {
        let dir = tempdir("finding-order");
        write(&dir, "a.yaml", "name: a\ntitle: X\nkind: [bogus]\n");
        write(&dir, "b.yaml", "name: b\ntitle: X\nkind: [bogus]\n");
        let (_scenarios, findings) = load(&dir);
        let mut sorted = findings.clone();
        sort_findings(&mut sorted);
        assert_eq!(findings, sorted);
        std::fs::remove_dir_all(&dir).ok();
    }

    // -- stale_findings ----------------------------------------------------

    fn scenario_with(name: &str, from: Option<NaiveDate>, weeks: u32) -> Scenario {
        Scenario {
            name: name.to_string(),
            title: "X".to_string(),
            kind: Vec::new(),
            assumptions: None,
            horizon: Horizon::from_weeks(weeks),
            from,
            policy: None,
            goals: Vec::new(),
            drivers: BTreeMap::new(),
            signposts: Vec::new(),
            narrative: None,
        }
    }

    #[test]
    fn a_scenario_past_its_horizon_is_stale_one_without_a_from_never_is() {
        let scenarios = vec![
            scenario_with("stale-one", Some(nd(2026, 1, 1)), 4), // ends ~4 weeks later, long past `now`
            scenario_with("fresh-one", Some(nd(2026, 9, 20)), 26),
            scenario_with("no-anchor", None, 1),
        ];
        let findings = stale_findings(&scenarios, dt(2026, 9, 25));
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].subject, "stale-one.yaml");
    }

    // -- examples ------------------------------------------------------------

    #[test]
    fn examples_scenarios_load_with_zero_findings() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/scenarios");
        let (scenarios, findings) = load(&dir);
        assert_eq!(findings, Vec::new(), "{findings:?}");
        assert_eq!(scenarios.len(), 2);
        let names: Vec<&str> = scenarios.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"ai-act-2027"));
        assert!(names.contains(&"capacity-drop"));
    }

    #[test]
    fn policy_load_all_does_not_recurse_into_drafts_so_a_draft_never_affects_the_real_report() {
        let policies = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/policies");
        let (catalogues, _findings) = policy::load_all(&policies);
        assert_eq!(catalogues.len(), 1, "{:?}", catalogues.iter().map(|c| &c.framework).collect::<Vec<_>>());
        assert_eq!(catalogues[0].framework, "cra");

        let (drafts, findings) = load_drafts(&policies.join("drafts"));
        assert_eq!(findings, Vec::new(), "{findings:?}");
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].framework, "ai-act");
    }

    // -- overlay_chain / policy_delta ---------------------------------------

    fn cra_catalogue() -> Catalogue {
        Catalogue {
            framework: "cra".to_string(),
            title: "CRA".to_string(),
            kind: Kind::Regulation,
            controls: vec![Control {
                id: "sbom".to_string(),
                title: "SBOM".to_string(),
                kind: None,
                max_age: Some(PDuration::from_hours(24 * 30)),
                maps_to: Vec::new(),
                remediation: None,
                evidence: vec![Check::Knowledge { tag: None }],
            }],
        }
    }

    fn ai_act_catalogue() -> Catalogue {
        Catalogue {
            framework: "ai-act".to_string(),
            title: "AI Act (draft)".to_string(),
            kind: Kind::Regulation,
            controls: vec![Control {
                id: "oversight".to_string(),
                title: "Human oversight".to_string(),
                kind: None,
                max_age: None,
                maps_to: Vec::new(),
                remediation: None,
                evidence: vec![Check::Attestation],
            }],
        }
    }

    fn scenario_with_policy(name: &str, overlay: PolicyOverlay) -> Scenario {
        let mut s = scenario_with(name, None, 26);
        s.policy = Some(overlay);
        s
    }

    #[test]
    fn overlay_chain_is_a_no_op_without_a_policy_section() {
        let chain = vec![PolicyLayer { scope: "root".to_string(), frameworks: vec!["cra".to_string()], tighten: BTreeMap::new(), not_applicable: Vec::new() }];
        let scenario = scenario_with("no-policy", None, 26);
        let (out, findings) = overlay_chain(&chain, &scenario);
        assert_eq!(out, chain);
        assert!(findings.is_empty());
    }

    #[test]
    fn overlay_chain_adds_a_synthetic_scenario_scoped_layer() {
        let chain = vec![PolicyLayer { scope: "root".to_string(), frameworks: vec!["cra".to_string()], tighten: BTreeMap::new(), not_applicable: Vec::new() }];
        let overlay = PolicyOverlay { add_frameworks: vec!["ai-act".to_string()], tighten: BTreeMap::new(), drop_not_applicable: Vec::new() };
        let scenario = scenario_with_policy("ai-act-2027", overlay);
        let (out, findings) = overlay_chain(&chain, &scenario);
        assert!(findings.is_empty());
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].scope, "scenario:ai-act-2027");
        assert_eq!(out[1].frameworks, vec!["ai-act".to_string()]);
    }

    #[test]
    fn overlay_chain_never_loosens_a_tighten_even_if_the_scenario_asks_for_a_longer_max_age() {
        let chain = vec![PolicyLayer { scope: "root".to_string(), frameworks: vec!["cra".to_string()], tighten: BTreeMap::new(), not_applicable: Vec::new() }];
        let catalogues = vec![cra_catalogue()];

        // A genuine tighten: 30d -> 14d.
        let tightened = PolicyOverlay {
            add_frameworks: Vec::new(),
            tighten: [(ControlRef::new("cra", "sbom"), Tighten { max_age: Some(PDuration::from_hours(24 * 14)) })].into_iter().collect(),
            drop_not_applicable: Vec::new(),
        };
        let scenario = scenario_with_policy("tighten", tightened);
        let (new_chain, ov_findings) = overlay_chain(&chain, &scenario);
        assert!(ov_findings.is_empty());
        let (applied, applic_findings) = policy::applicable(&catalogues, &new_chain);
        assert!(applic_findings.is_empty());
        assert_eq!(applied[0].max_age, Some(PDuration::from_hours(24 * 14)));

        // An attempted loosening: 30d -> 60d must not win, and must be
        // reported by policy::applicable's own LooseningHasNoEffect finding
        // -- this scenario module reuses that machinery rather than
        // reimplementing it.
        let loosened = PolicyOverlay {
            add_frameworks: Vec::new(),
            tighten: [(ControlRef::new("cra", "sbom"), Tighten { max_age: Some(PDuration::from_hours(24 * 60)) })].into_iter().collect(),
            drop_not_applicable: Vec::new(),
        };
        let scenario = scenario_with_policy("loosen", loosened);
        let (new_chain, _) = overlay_chain(&chain, &scenario);
        let (applied, applic_findings) = policy::applicable(&catalogues, &new_chain);
        assert_eq!(applied[0].max_age, Some(PDuration::from_hours(24 * 30)), "the original 30d must survive");
        assert!(applic_findings.iter().any(|f| f.kind == policy::FindingKind::LooseningHasNoEffect));
    }

    #[test]
    fn drop_not_applicable_clears_an_existing_na_and_flags_a_no_op() {
        let chain = vec![PolicyLayer {
            scope: "root".to_string(),
            frameworks: vec!["cra".to_string()],
            tighten: BTreeMap::new(),
            not_applicable: vec![NotApplicable { control: ControlRef::new("cra", "sbom"), rationale: "not built yet".to_string() }],
        }];
        let catalogues = vec![cra_catalogue()];

        // Baseline: n/a.
        let (baseline_applied, _) = policy::applicable(&catalogues, &chain);
        assert!(baseline_applied[0].not_applicable.is_some());

        // Drop it: applicable again.
        let overlay = PolicyOverlay { add_frameworks: Vec::new(), tighten: BTreeMap::new(), drop_not_applicable: vec![ControlRef::new("cra", "sbom")] };
        let scenario = scenario_with_policy("drop-na", overlay);
        let (new_chain, findings) = overlay_chain(&chain, &scenario);
        assert!(findings.is_empty());
        let (applied, _) = policy::applicable(&catalogues, &new_chain);
        assert!(applied[0].not_applicable.is_none());

        // Dropping a control that was never n/a anywhere is a no-op finding.
        let overlay = PolicyOverlay { add_frameworks: Vec::new(), tighten: BTreeMap::new(), drop_not_applicable: vec![ControlRef::new("cra", "does-not-exist")] };
        let scenario = scenario_with_policy("drop-nothing", overlay);
        let (_, findings) = overlay_chain(&chain, &scenario);
        assert!(findings.iter().any(|f| f.kind == FindingKind::DropNotApplicableHasNoEffect));
    }

    #[test]
    fn policy_delta_classifies_every_kind_of_change() {
        let sbom = ControlRef::new("cra", "sbom");
        let oversight = ControlRef::new("ai-act", "oversight");
        let already_covered = ControlRef::new("ai-act", "already-covered");
        let stayed_open = ControlRef::new("cra", "stayed-open");
        let vanished = ControlRef::new("cra", "vanished");

        let baseline = vec![
            ControlStatus { control: sbom.clone(), title: "SBOM".to_string(), kind: Kind::Regulation, refs: Vec::new(), status: policy::Status::Open { reasons: vec![] } },
            ControlStatus {
                control: stayed_open.clone(),
                title: "Stayed open".to_string(),
                kind: Kind::Regulation,
                refs: Vec::new(),
                status: policy::Status::Open { reasons: vec![] },
            },
            ControlStatus { control: vanished.clone(), title: "Vanished".to_string(), kind: Kind::Regulation, refs: Vec::new(), status: policy::Status::Open { reasons: vec![] } },
        ];
        let scenario = vec![
            // sbom regresses open -> stale
            ControlStatus { control: sbom.clone(), title: "SBOM".to_string(), kind: Kind::Regulation, refs: Vec::new(), status: policy::Status::Stale { reasons: vec![] } },
            // stayed_open stays open -> unchanged
            ControlStatus {
                control: stayed_open.clone(),
                title: "Stayed open".to_string(),
                kind: Kind::Regulation,
                refs: Vec::new(),
                status: policy::Status::Open { reasons: vec![] },
            },
            // brand new, open
            ControlStatus {
                control: oversight.clone(),
                title: "Oversight".to_string(),
                kind: Kind::Regulation,
                refs: Vec::new(),
                status: policy::Status::Open { reasons: vec![] },
            },
            // brand new, already satisfied by existing evidence (maps_to)
            ControlStatus {
                control: already_covered.clone(),
                title: "Already covered".to_string(),
                kind: Kind::Regulation,
                refs: Vec::new(),
                status: policy::Status::Satisfied { reasons: vec![] },
            },
            // vanished is absent from scenario entirely -- an invariant
            // violation, surfaced rather than silently dropped.
        ];

        let delta = policy_delta(&baseline, &scenario);
        assert_eq!(delta.newly_open, vec![oversight]);
        assert_eq!(delta.newly_stale, vec![sbom]);
        assert_eq!(delta.newly_applicable_but_covered, vec![already_covered]);
        assert_eq!(delta.unchanged, 1);
        assert_eq!(delta.missing_from_scenario, vec![vanished]);
        let cra_rollup = delta.per_framework.iter().find(|f| f.framework == "cra").unwrap();
        assert!(cra_rollup.before.is_some());
        assert!(cra_rollup.after.is_some());
        let ai_act_rollup = delta.per_framework.iter().find(|f| f.framework == "ai-act").unwrap();
        assert!(ai_act_rollup.before.is_none(), "ai-act had no baseline statuses at all");
        assert!(ai_act_rollup.after.is_some());
    }

    #[test]
    fn ai_act_catalogue_helper_is_a_valid_catalogue() {
        // A cheap smoke test on the fixture itself -- `applicable` refuses
        // nothing about it.
        let catalogues = vec![ai_act_catalogue()];
        let chain = vec![PolicyLayer { scope: "root".to_string(), frameworks: vec!["ai-act".to_string()], tighten: BTreeMap::new(), not_applicable: Vec::new() }];
        let (applied, findings) = policy::applicable(&catalogues, &chain);
        assert!(findings.is_empty());
        assert_eq!(applied.len(), 1);
        let statuses = policy::evaluate(&applied, &Evidence::default(), dt(2026, 9, 25));
        assert_eq!(statuses[0].status.kind(), policy::StatusKind::Open);
    }

    // -- Everything Serialize: a smoke test over the computed report types --

    #[test]
    fn key_output_types_serialize() {
        let forecast = forecast_completion(&[3.0, 4.0, 5.0], 30.0, 8, 100, 1);
        serde_json::to_string(&forecast).unwrap();

        let metric_forecast = forecast_metric(&metric_series(&[1.0, 1.1, 1.2]), 4, 50, 1);
        serde_json::to_string(&metric_forecast).unwrap();

        let bars = tornado(&BTreeMap::from([("throughput_week".to_string(), 10.0)]), "effective_throughput", 0.2);
        serde_json::to_string(&bars).unwrap();

        let gp = goal_probability(KrShape::CountLike, 0.0, 5.0, nd(2027, 1, 1), dt(2026, 9, 25), &[1.0, 2.0], 50, 1);
        serde_json::to_string(&gp).unwrap();

        let sp = SignpostStatus { metric: MetricId::new("throughput_week").unwrap(), state: SignpostState::Quiet, reason: "ok".to_string() };
        serde_json::to_string(&sp).unwrap();

        let delta = policy_delta(&[], &[]);
        serde_json::to_string(&delta).unwrap();

        let scenario = scenario_with("s", Some(nd(2026, 1, 1)), 4);
        serde_json::to_string(&scenario).unwrap();
    }
}
