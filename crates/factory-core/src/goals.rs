//! Vision, mission and objectives, the same shape as `policy.rs`'s
//! catalogue: authored YAML, re-read on every request, never written by
//! Factory, with a [`Finding`] for anything an author got wrong rather
//! than a hard failure that takes the rest of the file (or the other
//! files) down with it. There is no ADR for this one -- GitHub issue `#99`
//! (L6 Goals tab) is the spec, and `#100` (Scenarios) reuses the metric
//! registry this module sits on top of (`crate::metrics`).
//!
//! ## Storage
//!
//! `<root>/.factory/goals/` -- authored content, like `.factory/knowledge/`,
//! `.factory/datasets/` and `.factory/policies/`: `direction.yaml`, the
//! long-term frame ([`Direction`]), plus one file per planning cycle,
//! `<cycle-id>.yaml` ([`Cycle`]), whose stem must equal the cycle's own
//! `id` -- exactly the rule `policy.rs`'s `Catalogue.framework` holds
//! against its file name. [`load`] reads the whole directory at once, the
//! same two-pass shape `policy::load_all` uses: a file that fails to
//! parse, or whose id does not match its name, is a [`Finding`] naming it,
//! and every other file still loads; a missing directory is empty, not an
//! error.
//!
//! ## Goals enforce nothing
//!
//! Nothing here starts, stops, or gates any work -- design §8's deferral
//! of a rule language holds exactly as it does for policies. A key result
//! is either backed by a metric this module can look up, or explicitly
//! `manual` -- recorded by an append-only [`CheckIn`], never asserted.
//!
//! ## The metric registry lives one module over
//!
//! `crate::metrics` is the shared vocabulary `#99` and `#100` both read
//! from -- what a metric means, its unit, which way is better, and whether
//! `factory-daemon` can compute it yet. This module never computes a
//! metric's value itself; [`evaluate`] is handed already-computed
//! [`crate::metrics::MetricValue`]s (`factory-daemon`'s job, a later
//! ticket) and a bundle of check-ins, and turns them into a graded report
//! -- pure, like `policy::evaluate`, with `now` passed in rather than read
//! off the clock. The dependency runs one way only: this module imports
//! `crate::metrics`, and `crate::metrics` imports nothing of this one, so
//! `#100`'s scenario module can sit on the registry without ever picking
//! up a goals-shaped dependency by accident.
//!
//! `goal_tasks_done`, the one family a key result names *unbound*
//! (`metric: goal_tasks_done`, no objective or key-result id spelled out)
//! is bound by [`KeyResult::bound_metric`] to that key result's own ids --
//! `goal_tasks_done.<objective>.<kr>` -- the scheme the issue leaves this
//! module to invent, chosen so an author never repeats an id already
//! written two lines above.
//!
//! ## Scoring
//!
//! [`score`] is the one formula for both directions a key result can run:
//! higher-is-better (`target > baseline`) and lower-is-better
//! (`target < baseline`) read the same linear fraction of `baseline` to
//! `target`, clamped to `0.0..=1.0`. [`band`] then grades that fraction on
//! one of two curves -- *Measure What Matters*'s own rule that a
//! *committed* key result is a promise (green only once fully met) and an
//! *aspirational* one is a stretch (green at "clearly winning"). A key
//! result [`evaluate`] could not compute at all -- no metric value yet, no
//! check-in yet -- carries `score: None`, never a manufactured `0.0`:
//! "unscored" and "scored red" are different facts, and only the first is
//! true when there is simply no evidence yet.
//!
//! ## Ordering
//!
//! [`load`] sorts cycles by `(from, id)` and findings by
//! `(subject, kind, detail)`, the same determinism `policy::load_all`
//! promises. Objectives, key results and roadmap items keep the order
//! they were authored in -- that order is itself information (priority),
//! not something to alphabetise away.

use crate::dataset::is_slug;
use crate::metrics::{self, MetricError, MetricId, MetricValue};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

// ============================================================== direction

/// The long-term frame, authored once at `<root>/.factory/goals/direction.yaml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Direction {
    pub vision: String,
    pub mission: String,
    #[serde(default)]
    pub values: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub north_star: Option<NorthStar>,
    #[serde(default)]
    pub inputs: Vec<MetricId>,
    #[serde(default)]
    pub obstacles: Vec<String>,
}

/// The one leading metric, and why it was chosen over any other (Amplitude's
/// North Star framework).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NorthStar {
    pub metric: MetricId,
    pub why: String,
}

// ================================================================== cycle

/// `cycle: { id, from, to }` -- the header every cycle file carries
/// alongside its `objectives` and `roadmap`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CycleHeader {
    pub id: String,
    pub from: NaiveDate,
    pub to: NaiveDate,
}

/// One planning cycle, exactly as authored at
/// `<root>/.factory/goals/<cycle-id>.yaml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cycle {
    pub cycle: CycleHeader,
    #[serde(default)]
    pub objectives: Vec<Objective>,
    #[serde(default)]
    pub roadmap: Vec<RoadmapItem>,
}

impl Cycle {
    pub fn id(&self) -> &str {
        &self.cycle.id
    }

    pub fn starts_on(&self) -> NaiveDate {
        self.cycle.from
    }

    pub fn ends_on(&self) -> NaiveDate {
        self.cycle.to
    }

    /// `[start, end)`, `end` one day past `ends_on` so a cycle whose `to`
    /// is `2026-12-31` still covers every moment of December 31st --
    /// authored dates are calendar days, not midnight cutoffs.
    fn bounds(&self) -> (DateTime<Utc>, DateTime<Utc>) {
        let start = self.starts_on().and_hms_opt(0, 0, 0).unwrap().and_utc();
        let end = (self.ends_on() + chrono::Duration::days(1))
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc();
        (start, end)
    }
}

/// One objective within a cycle: a small number of key results, and
/// optionally a parent objective it cascades from ([`Objective::aligns_to`],
/// same cycle only -- see the module doc comment).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Objective {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aligns_to: Option<String>,
    #[serde(default)]
    pub key_results: Vec<KeyResult>,
}

/// Whether a key result is a promise or a stretch -- *Measure What
/// Matters*'s own vocabulary. [`band`] grades the two on different curves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KrKind {
    Committed,
    Aspirational,
}

/// One key result. Exactly one of `metric` or `manual: true` must be set --
/// neither is a *vanity* key result ([`FindingKind::VanityKeyResult`]),
/// both is a [`FindingKind::ConflictingKeyResult`] -- [`load`] checks both,
/// never a hard parse failure, since the rest of the objective is still
/// worth loading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyResult {
    pub id: String,
    pub title: String,
    pub kind: KrKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric: Option<MetricId>,
    #[serde(default)]
    pub manual: bool,
    pub baseline: f64,
    pub target: f64,
}

impl KeyResult {
    /// The concrete metric id this key result reads from, with the
    /// `goal_tasks_done` family's placeholder bound to this key result's
    /// own `objective_id`/`id` -- see the module doc comment. `None` if
    /// `metric` is unset, or -- defensively -- if binding would produce
    /// something [`MetricId`] itself refuses (an objective or key-result
    /// id with a character outside a metric segment's charset); `load`'s
    /// own validation turns that into a [`FindingKind::UnknownMetric`]
    /// rather than this ever panicking.
    pub fn bound_metric(&self, objective_id: &str) -> Option<MetricId> {
        let raw = self.metric.as_ref()?;
        if raw.as_str() == "goal_tasks_done" {
            MetricId::new(format!("goal_tasks_done.{objective_id}.{}", self.id)).ok()
        } else {
            Some(raw.clone())
        }
    }
}

/// Now / Next / Later -- ProdPad's confidence lanes, not promised dates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    Now,
    Next,
    Later,
}

/// One roadmap item. `objectives` empty is an orphan
/// ([`FindingKind::OrphanRoadmapItem`]); naming an id this cycle does not
/// have is [`FindingKind::UnknownRoadmapObjective`] -- both reported, both
/// still loaded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoadmapItem {
    pub id: String,
    pub title: String,
    pub lane: Lane,
    #[serde(default)]
    pub objectives: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

// ============================================================== findings

/// One of the things [`load`] checks for. Never stops another file, or
/// another part of the same file, from loading -- see the module doc
/// comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    ParseFailed,
    /// A cycle file's `cycle.id` does not match the file's own stem.
    StemMismatch,
    /// An objective, key-result, or roadmap-item id repeated within the
    /// scope it must be unique in -- the detail says which.
    DuplicateId,
    /// A cycle's `from` is after its `to`.
    InvertedDates,
    /// Two cycles' `[from, to]` windows overlap.
    OverlappingCycles,
    /// A key result names neither `metric` nor `manual: true`.
    VanityKeyResult,
    /// A key result names both `metric` and `manual: true`.
    ConflictingKeyResult,
    /// A `metric`/`north_star`/`inputs` field names something
    /// [`metrics::resolve`] has never heard of.
    UnknownMetric,
    /// A `metric`/`north_star`/`inputs` field names a metric
    /// `metrics::resolve` knows but cannot compute yet.
    UnavailableMetric,
    /// An objective with no key results.
    ObjectiveWithoutKeyResults,
    /// A cycle with more than five objectives.
    TooManyObjectives,
    /// A roadmap item names no objective.
    OrphanRoadmapItem,
    /// A roadmap item names an objective id this cycle does not have.
    UnknownRoadmapObjective,
    /// An `aligns_to` names an objective id this cycle does not have.
    UnknownAlignsTo,
    /// An `aligns_to` chain loops back on itself (directly, or through
    /// other objectives) instead of terminating.
    CyclicAlignsTo,
    /// A cycle, objective, key-result, or roadmap-item id is not
    /// `dataset::is_slug`-shaped (`[a-z0-9][a-z0-9-]*`). Every one of
    /// these ids is later named from outside its own file -- a check-in's
    /// [`KrRef`], a task's `goal=<objective>/<kr>` label, `aligns_to`, a
    /// roadmap item's `objectives` -- and a badly-shaped one can never be
    /// matched by any of them, so it never becomes anything but a vanity
    /// entry no evidence can ever reach.
    BadIdShape,
    /// A key result's `baseline`/`target` move the opposite way from its
    /// metric's [`metrics::Better`] -- e.g. a `scrap_rate` (lower is
    /// better) key result with `baseline: 0.05, target: 0.11`, which asks
    /// to make the number worse. Only checked for a metric-backed key
    /// result: a `manual` one has no `Better` to contradict.
    WrongDirection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub kind: FindingKind,
    /// The file (or, for a cross-cycle check, one of the two files) the
    /// finding is about -- "where to go look", the same role it plays in
    /// `policy::Finding`.
    pub subject: String,
    pub detail: String,
}

fn finding(kind: FindingKind, subject: impl Into<String>, detail: impl Into<String>) -> Finding {
    Finding {
        kind,
        subject: subject.into(),
        detail: detail.into(),
    }
}

// ================================================================= loader

/// `<root>/.factory/goals`, the authored-content directory.
pub fn goals_dir(root: &Path) -> PathBuf {
    root.join(".factory").join("goals")
}

/// Everything [`load`] found in `<root>/.factory/goals/`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct GoalsCatalogue {
    pub direction: Option<Direction>,
    /// Sorted by `(from, id)`.
    pub cycles: Vec<Cycle>,
    /// Sorted by `(subject, kind, detail)`.
    pub findings: Vec<Finding>,
}

/// Load `direction.yaml` and every `<cycle-id>.yaml` in `dir`. A missing
/// directory is empty, not an error. See the module doc comment for the
/// two-pass shape (parse, then cross-cutting checks) this shares with
/// `policy::load_all`.
pub fn load(dir: &Path) -> GoalsCatalogue {
    let mut findings = Vec::new();
    let mut direction: Option<Direction> = None;
    let mut cycles: Vec<Cycle> = Vec::new();

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
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();

        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                findings.push(finding(FindingKind::ParseFailed, &file_name, format!("reading: {e}")));
                continue;
            }
        };

        if stem == "direction" {
            match serde_yaml_ng::from_str::<Direction>(&text) {
                Ok(d) => direction = Some(d),
                Err(e) => findings.push(finding(FindingKind::ParseFailed, &file_name, format!("parsing: {e}"))),
            }
            continue;
        }

        match serde_yaml_ng::from_str::<Cycle>(&text) {
            Ok(cycle) => {
                if cycle.id() != stem {
                    findings.push(finding(
                        FindingKind::StemMismatch,
                        &file_name,
                        format!("cycle id {:?} does not match the file name {:?}", cycle.id(), stem),
                    ));
                    continue;
                }
                cycles.push(cycle);
            }
            Err(e) => findings.push(finding(FindingKind::ParseFailed, &file_name, format!("parsing: {e}"))),
        }
    }

    cycles.sort_by(|a, b| (a.starts_on(), a.id()).cmp(&(b.starts_on(), b.id())));

    if let Some(d) = &direction {
        validate_direction(d, &mut findings);
    }
    for cycle in &cycles {
        validate_cycle(cycle, &mut findings);
    }
    validate_no_overlap(&cycles, &mut findings);

    findings.sort_by(|a, b| a.subject.cmp(&b.subject).then(a.kind.cmp(&b.kind)).then(a.detail.cmp(&b.detail)));

    GoalsCatalogue { direction, cycles, findings }
}

/// Resolves `id`, pushing an `UnknownMetric`/`UnavailableMetric` finding and
/// returning `None` if it can't be; `Some` the metric's own definition
/// otherwise, for a caller (the key-result direction check below) that
/// needs more than just whether it resolved.
fn check_metric(id: &MetricId, subject: &str, what: &str, findings: &mut Vec<Finding>) -> Option<metrics::MetricDef> {
    match metrics::resolve(id) {
        Ok(def) => Some(def),
        Err(MetricError::Unknown(_)) => {
            findings.push(finding(FindingKind::UnknownMetric, subject, format!("{what} names unknown metric {id}")));
            None
        }
        Err(MetricError::Unavailable { reason, .. }) => {
            findings.push(finding(
                FindingKind::UnavailableMetric,
                subject,
                format!("{what} names metric {id} which is not available yet: {reason}"),
            ));
            None
        }
    }
}

/// `dataset::is_slug`-shaped, or a [`FindingKind::BadIdShape`] naming
/// `what` and the offending `id`. See that variant's own doc comment for
/// why: every one of these ids is later matched from outside its own
/// file.
fn check_id_shape(id: &str, subject: &str, what: &str, findings: &mut Vec<Finding>) {
    if !is_slug(id) {
        findings.push(finding(
            FindingKind::BadIdShape,
            subject,
            format!("{what} {id:?} is not shaped like dataset::is_slug ([a-z0-9][a-z0-9-]*)"),
        ));
    }
}

fn validate_direction(direction: &Direction, findings: &mut Vec<Finding>) {
    const SUBJECT: &str = "direction.yaml";
    if let Some(ns) = &direction.north_star {
        check_metric(&ns.metric, SUBJECT, "north_star", findings);
    }
    for input in &direction.inputs {
        check_metric(input, SUBJECT, "inputs", findings);
    }
}

fn validate_cycle(cycle: &Cycle, findings: &mut Vec<Finding>) {
    let subject = format!("{}.yaml", cycle.id());

    check_id_shape(cycle.id(), &subject, "cycle id", findings);

    if cycle.starts_on() > cycle.ends_on() {
        findings.push(finding(
            FindingKind::InvertedDates,
            &subject,
            format!("from {} is after to {}", cycle.starts_on(), cycle.ends_on()),
        ));
    }

    if cycle.objectives.len() > 5 {
        findings.push(finding(
            FindingKind::TooManyObjectives,
            &subject,
            format!("{} objectives, more than the cap of 5", cycle.objectives.len()),
        ));
    }

    let objective_ids: BTreeSet<&str> = cycle.objectives.iter().map(|o| o.id.as_str()).collect();
    let mut seen_objective_ids: BTreeSet<&str> = BTreeSet::new();

    for objective in &cycle.objectives {
        check_id_shape(&objective.id, &subject, "objective id", findings);

        if !seen_objective_ids.insert(objective.id.as_str()) {
            findings.push(finding(
                FindingKind::DuplicateId,
                &subject,
                format!("objective id {:?} repeated", objective.id),
            ));
        }

        if objective.key_results.is_empty() {
            findings.push(finding(
                FindingKind::ObjectiveWithoutKeyResults,
                &subject,
                format!("objective {:?} has no key results", objective.id),
            ));
        }

        if let Some(parent) = &objective.aligns_to {
            if !objective_ids.contains(parent.as_str()) {
                findings.push(finding(
                    FindingKind::UnknownAlignsTo,
                    &subject,
                    format!("objective {:?} aligns_to unknown objective {:?}", objective.id, parent),
                ));
            }
        }

        let mut seen_kr_ids: BTreeSet<&str> = BTreeSet::new();
        for kr in &objective.key_results {
            check_id_shape(&kr.id, &subject, "key result id", findings);

            if !seen_kr_ids.insert(kr.id.as_str()) {
                findings.push(finding(
                    FindingKind::DuplicateId,
                    &subject,
                    format!("key result id {:?} repeated in objective {:?}", kr.id, objective.id),
                ));
            }

            let has_metric = kr.metric.is_some();
            if has_metric && kr.manual {
                findings.push(finding(
                    FindingKind::ConflictingKeyResult,
                    &subject,
                    format!("{}/{} names both a metric and manual: true", objective.id, kr.id),
                ));
            }
            if !has_metric && !kr.manual {
                findings.push(finding(
                    FindingKind::VanityKeyResult,
                    &subject,
                    format!("{}/{} names neither a metric nor manual: true", objective.id, kr.id),
                ));
            }
            if kr.metric.is_some() {
                let what = format!("{}/{}", objective.id, kr.id);
                match kr.bound_metric(&objective.id) {
                    Some(bound) => {
                        if let Some(def) = check_metric(&bound, &subject, &what, findings) {
                            // Only a computed metric has a `Better` to
                            // contradict -- a manual key result (excluded
                            // by the `kr.metric.is_some()` guard above) has
                            // none, so it is never checked here.
                            let wrong_direction = match def.better {
                                metrics::Better::Higher => kr.target < kr.baseline,
                                metrics::Better::Lower => kr.target > kr.baseline,
                            };
                            if wrong_direction {
                                findings.push(finding(
                                    FindingKind::WrongDirection,
                                    &subject,
                                    format!(
                                        "{what}'s baseline {} -> target {} moves the wrong way for {bound} (better: {:?})",
                                        kr.baseline, kr.target, def.better
                                    ),
                                ));
                            }
                        }
                    }
                    None => findings.push(finding(
                        FindingKind::UnknownMetric,
                        &subject,
                        format!("{what}'s metric {:?} could not be bound to a metric id", kr.metric),
                    )),
                }
            }
        }
    }

    for id in cyclic_objectives(cycle) {
        findings.push(finding(FindingKind::CyclicAlignsTo, &subject, format!("objective {id:?} is in an aligns_to cycle")));
    }

    let mut seen_roadmap_ids: BTreeSet<&str> = BTreeSet::new();
    for item in &cycle.roadmap {
        check_id_shape(&item.id, &subject, "roadmap item id", findings);

        if !seen_roadmap_ids.insert(item.id.as_str()) {
            findings.push(finding(
                FindingKind::DuplicateId,
                &subject,
                format!("roadmap item id {:?} repeated", item.id),
            ));
        }
        if item.objectives.is_empty() {
            findings.push(finding(
                FindingKind::OrphanRoadmapItem,
                &subject,
                format!("roadmap item {:?} links no objective", item.id),
            ));
        }
        for obj_id in &item.objectives {
            if !objective_ids.contains(obj_id.as_str()) {
                findings.push(finding(
                    FindingKind::UnknownRoadmapObjective,
                    &subject,
                    format!("roadmap item {:?} links unknown objective {:?}", item.id, obj_id),
                ));
            }
        }
    }
}

/// Every objective id whose `aligns_to` chain fails to terminate within
/// the number of objectives in the cycle -- by the pigeonhole principle,
/// that many hops without a `None` means some id was visited twice, so a
/// cycle exists somewhere along the chain. Catches a direct
/// self-reference (`aligns_to` naming its own id, a one-hop loop) the same
/// way it catches a longer chain; deliberately also flags an objective
/// that merely *feeds into* someone else's cycle without being part of it
/// itself -- its own chain never reaches a root either, which is the fact
/// worth reporting.
fn cyclic_objectives(cycle: &Cycle) -> BTreeSet<String> {
    let parent_of: BTreeMap<&str, &str> = cycle
        .objectives
        .iter()
        .filter_map(|o| o.aligns_to.as_deref().map(|parent| (o.id.as_str(), parent)))
        .collect();
    let limit = cycle.objectives.len();

    let mut cyclic = BTreeSet::new();
    for objective in &cycle.objectives {
        let mut cur = objective.id.as_str();
        let mut hops = 0usize;
        while let Some(&next) = parent_of.get(cur) {
            hops += 1;
            if hops > limit {
                cyclic.insert(objective.id.clone());
                break;
            }
            cur = next;
        }
    }
    cyclic
}

/// `[from, to]` windows treated as half-open `[start, end)` (see
/// `Cycle::bounds`), so two cycles that merely touch at a day boundary do
/// not count as overlapping.
fn validate_no_overlap(cycles: &[Cycle], findings: &mut Vec<Finding>) {
    for i in 0..cycles.len() {
        for j in (i + 1)..cycles.len() {
            let (s1, e1) = cycles[i].bounds();
            let (s2, e2) = cycles[j].bounds();
            if s1 < e2 && s2 < e1 {
                findings.push(finding(
                    FindingKind::OverlappingCycles,
                    format!("{}.yaml", cycles[i].id()),
                    format!(
                        "overlaps cycle {} ({}..{})",
                        cycles[j].id(),
                        cycles[j].starts_on(),
                        cycles[j].ends_on()
                    ),
                ));
            }
        }
    }
}

// ================================================================ scoring

/// Linear score from `baseline` to `target`, clamped to `0.0..=1.0`. One
/// formula reads both directions: for a higher-is-better key result
/// (`target > baseline`) and a lower-is-better one (`target < baseline`)
/// alike, `value` moving from `baseline` towards `target` always raises
/// the result, whichever way that motion runs on the number line. A key
/// result with no distance to travel (`baseline == target`) is trivially
/// fully scored -- there is nothing `value` could do to change how far
/// along it is. A `value` (or `baseline`/`target`) that makes the fraction
/// NaN -- should never happen; a computed metric or a check-in is always a
/// real number -- scores `0.0` rather than propagating a NaN into a
/// report. An infinite overshoot or undershoot still clamps normally, the
/// same as a large finite one.
pub fn score(baseline: f64, target: f64, value: f64) -> f64 {
    let span = target - baseline;
    if span == 0.0 {
        return 1.0;
    }
    let raw = (value - baseline) / span;
    if raw.is_nan() {
        return 0.0;
    }
    // `clamp` itself handles a non-NaN infinity correctly -- a `value` that
    // overshoots the target without bound still scores the same `1.0` a
    // merely-large overshoot does.
    raw.clamp(0.0, 1.0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Band {
    Green,
    Yellow,
    Red,
}

/// Committed and aspirational key results are graded on different curves
/// -- *Measure What Matters*'s own rule: a committed key result is a
/// promise, graded green only once fully met (`1.0`); an aspirational one
/// is a stretch, green once it is clearly winning (`>= 0.7`) rather than
/// finished.
pub fn band(kind: KrKind, score: f64) -> Band {
    match kind {
        KrKind::Committed => {
            if score >= 1.0 {
                Band::Green
            } else if score >= 0.7 {
                Band::Yellow
            } else {
                Band::Red
            }
        }
        KrKind::Aspirational => {
            if score >= 0.7 {
                Band::Green
            } else if score >= 0.4 {
                Band::Yellow
            } else {
                Band::Red
            }
        }
    }
}

// ================================================================ check-ins

/// A key result's stable identity outside the file that defines it:
/// `objective/kr`. Serializes as exactly that string -- the same shape,
/// for the same reason, as `policy::ControlRef`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct KrRef {
    pub objective: String,
    pub kr: String,
}

impl KrRef {
    pub fn new(objective: impl Into<String>, kr: impl Into<String>) -> Self {
        Self { objective: objective.into(), kr: kr.into() }
    }
}

impl std::fmt::Display for KrRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.objective, self.kr)
    }
}

impl std::str::FromStr for KrRef {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let (objective, kr) = s.split_once('/').ok_or_else(|| format!("{s:?} is not objective/kr"))?;
        if !is_slug(objective) || !is_slug(kr) {
            return Err(format!("{s:?} is not objective/kr, each matching [a-z0-9][a-z0-9-]*"));
        }
        Ok(KrRef { objective: objective.to_string(), kr: kr.to_string() })
    }
}

impl TryFrom<String> for KrRef {
    type Error = String;

    fn try_from(s: String) -> std::result::Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<KrRef> for String {
    fn from(r: KrRef) -> String {
        r.to_string()
    }
}

/// One append-only check-in against a manual key result --
/// `factory-daemon` stores these (a later ticket), the same pattern as
/// `policy::Attestation`; this module only carries the type and reads the
/// latest one per key result in [`evaluate`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckIn {
    pub id: String,
    pub kr: KrRef,
    pub value: f64,
    /// 0..=10. Not enforced by this type -- same restraint `policy.rs`
    /// shows toward ranges it does not own the write path for; the
    /// daemon's write path (`#99` slice 2) is where a bad value is
    /// refused.
    pub confidence: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub by: String,
    pub at: DateTime<Utc>,
}

// ================================================================ evaluate

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleStatus {
    Future,
    Current,
    Past,
}

/// Which side of `now` a cycle's `[from, to]` window falls on -- half-open
/// bounds, see `Cycle::bounds`.
pub fn cycle_status(cycle: &Cycle, now: DateTime<Utc>) -> CycleStatus {
    let (start, end) = cycle.bounds();
    if now < start {
        CycleStatus::Future
    } else if now >= end {
        CycleStatus::Past
    } else {
        CycleStatus::Current
    }
}

/// How far `now` sits within a cycle's window, `0.0..=1.0` -- `0.0` at or
/// before `from`, `1.0` at or after the end of `to`. `Cycle::bounds`'s end
/// is always at least a day past `start`, so this never divides by zero
/// for a well-formed cycle; an inverted one (`from` after `to`, already an
/// [`FindingKind::InvertedDates`] finding) can still make `total`
/// negative, which reads as fully elapsed rather than a negative fraction.
pub fn elapsed_fraction(cycle: &Cycle, now: DateTime<Utc>) -> f64 {
    let (start, end) = cycle.bounds();
    let total = (end - start).num_seconds();
    if total <= 0 {
        return 1.0;
    }
    let elapsed = (now - start).num_seconds();
    (elapsed as f64 / total as f64).clamp(0.0, 1.0)
}

/// The cycle whose window contains `now`, choosing the earliest-starting
/// one (then lowest id) if more than one does -- which only happens when
/// [`load`] has already reported an [`FindingKind::OverlappingCycles`]
/// finding, so this still needs a deterministic answer rather than an
/// error.
pub fn current_cycle(cycles: &[Cycle], now: DateTime<Utc>) -> Option<&Cycle> {
    cycles
        .iter()
        .filter(|c| cycle_status(c, now) == CycleStatus::Current)
        .min_by(|a, b| (a.starts_on(), a.id()).cmp(&(b.starts_on(), b.id())))
}

/// One key result, evaluated: its current value, score and band if it has
/// one, and why not if it does not.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KrResult {
    pub kr: KrRef,
    pub title: String,
    pub kind: KrKind,
    pub value: Option<f64>,
    /// `None` means unscored -- no metric value or check-in yet -- never a
    /// manufactured `0.0`; see the module doc comment.
    pub score: Option<f64>,
    pub band: Option<Band>,
    /// Committed only: whether `score` has kept pace with the cycle's own
    /// `elapsed` fraction -- a committed key result is meant to close
    /// linearly over the cycle, so `score < elapsed` reads as behind.
    /// `None` for an aspirational key result (no such pace is implied) or
    /// an unscored one.
    pub on_pace: Option<bool>,
    /// `"metric compliance.cra (as of 2026-09-24)"` or
    /// `"check-in by alice on 2026-09-24"` -- one line naming where
    /// `value` came from.
    pub source: String,
    /// e.g. `"metric unavailable: …"`, `"no check-in yet"`.
    pub reasons: Vec<String>,
    /// The latest check-in's own confidence, for a manual key result.
    pub confidence: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ObjectiveResult {
    pub objective: String,
    pub title: String,
    /// The mean of every scored key result's `score`; `None` if none of
    /// them are.
    pub score: Option<f64>,
    pub key_results: Vec<KrResult>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CycleReport {
    pub cycle_id: String,
    pub status: CycleStatus,
    /// `0.0..=1.0` -- also the pace benchmark `KrResult::on_pace` compares
    /// a committed key result's `score` against.
    pub elapsed: f64,
    pub objectives: Vec<ObjectiveResult>,
}

/// Turn a cycle's authored key results into a graded report: each key
/// result's current value (a computed metric from `values`, or the latest
/// [`CheckIn`] for a manual one), its score and band, and why one is
/// missing when it is. Pure -- `now` only ever reaches this through
/// `elapsed_fraction`/`cycle_status`, never read off the clock directly.
pub fn evaluate(cycle: &Cycle, values: &BTreeMap<MetricId, MetricValue>, checkins: &[CheckIn], now: DateTime<Utc>) -> CycleReport {
    let elapsed = elapsed_fraction(cycle, now);
    let status = cycle_status(cycle, now);

    let objectives = cycle
        .objectives
        .iter()
        .map(|objective| {
            let key_results: Vec<KrResult> = objective
                .key_results
                .iter()
                .map(|kr| evaluate_kr(objective, kr, values, checkins, elapsed))
                .collect();
            let scored: Vec<f64> = key_results.iter().filter_map(|r| r.score).collect();
            let score = if scored.is_empty() { None } else { Some(scored.iter().sum::<f64>() / scored.len() as f64) };
            ObjectiveResult {
                objective: objective.id.clone(),
                title: objective.title.clone(),
                score,
                key_results,
            }
        })
        .collect();

    CycleReport { cycle_id: cycle.id().to_string(), status, elapsed, objectives }
}

fn evaluate_kr(
    objective: &Objective,
    kr: &KeyResult,
    values: &BTreeMap<MetricId, MetricValue>,
    checkins: &[CheckIn],
    elapsed: f64,
) -> KrResult {
    let kr_ref = KrRef::new(objective.id.clone(), kr.id.clone());
    let mut reasons = Vec::new();

    // A key result naming both `metric` and `manual: true` is already a
    // load-time `ConflictingKeyResult` finding; a report still has to pick
    // one deterministically rather than panic, and `manual` wins -- a
    // person's own check-in is the more deliberate of the two sources.
    let (value, confidence, source) = if kr.manual {
        match checkins.iter().filter(|c| c.kr == kr_ref).max_by_key(|c| c.at) {
            Some(c) => (Some(c.value), Some(c.confidence), format!("check-in by {} on {}", c.by, c.at.date_naive())),
            None => {
                reasons.push("no check-in yet".to_string());
                (None, None, "manual (no check-in yet)".to_string())
            }
        }
    } else if let Some(bound) = kr.bound_metric(&objective.id) {
        match values.get(&bound) {
            Some(mv) => match mv.value {
                Some(v) => (Some(v), None, format!("metric {bound} (as of {})", mv.as_of.date_naive())),
                None => {
                    let reason = mv.reason.clone().unwrap_or_else(|| "no reason given".to_string());
                    reasons.push(format!("metric unavailable: {reason}"));
                    (None, None, format!("metric {bound}"))
                }
            },
            None => {
                reasons.push(format!("no data for metric {bound} yet"));
                (None, None, format!("metric {bound}"))
            }
        }
    } else {
        reasons.push("no metric or check-in source".to_string());
        (None, None, "unscored".to_string())
    };

    let score = value.map(|v| score(kr.baseline, kr.target, v));
    let band = score.map(|s| band(kr.kind, s));
    let on_pace = match kr.kind {
        KrKind::Committed => score.map(|s| s >= elapsed),
        KrKind::Aspirational => None,
    };

    KrResult {
        kr: kr_ref,
        title: kr.title.clone(),
        kind: kr.kind,
        value,
        score,
        band,
        on_pace,
        source,
        reasons,
        confidence,
    }
}

// ============================================================== summary

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct BandCounts {
    pub green: usize,
    pub yellow: usize,
    pub red: usize,
    pub unscored: usize,
}

impl BandCounts {
    fn add(&mut self, band: Option<Band>) {
        match band {
            Some(Band::Green) => self.green += 1,
            Some(Band::Yellow) => self.yellow += 1,
            Some(Band::Red) => self.red += 1,
            None => self.unscored += 1,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ReportSummary {
    pub committed: BandCounts,
    pub aspirational: BandCounts,
}

/// Every key result in `report`, counted into its band -- committed and
/// aspirational kept apart, the split the L6 tab must keep visually
/// distinct (the issue's own guardrail).
pub fn report_summary(report: &CycleReport) -> ReportSummary {
    let mut summary = ReportSummary::default();
    for objective in &report.objectives {
        for kr in &objective.key_results {
            let bucket = match kr.kind {
                KrKind::Committed => &mut summary.committed,
                KrKind::Aspirational => &mut summary.aspirational,
            };
            bucket.add(kr.band);
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), text).unwrap();
    }

    fn tempdir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("factory-goals-test-{label}-{}", uuid::Uuid::new_v4()))
    }

    fn cleanup(dir: &Path) {
        std::fs::remove_dir_all(dir).ok();
    }

    /// `0.6 - 0.2`, `0.8`, and their kin are not exactly representable in
    /// binary floating point, so a couple of the `score` tests below
    /// compare within a tolerance rather than bit for bit.
    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    const MINIMAL_CYCLE: &str = "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
         objectives:\n\
         \x20\x20- id: obj\n\x20\x20\x20\x20title: Objective\n\x20\x20\x20\x20key_results:\n\
         \x20\x20\x20\x20\x20\x20- id: kr\n\x20\x20\x20\x20\x20\x20\x20\x20title: KR\n\x20\x20\x20\x20\x20\x20\x20\x20kind: committed\n\
         \x20\x20\x20\x20\x20\x20\x20\x20metric: first_pass_yield\n\x20\x20\x20\x20\x20\x20\x20\x20baseline: 0\n\x20\x20\x20\x20\x20\x20\x20\x20target: 1\n";

    // -- load: missing dir, parse, stem -----------------------------------

    #[test]
    fn a_missing_directory_loads_as_empty_with_no_findings() {
        let dir = tempdir("missing");
        let catalogue = load(&dir);
        assert!(catalogue.direction.is_none());
        assert!(catalogue.cycles.is_empty());
        assert!(catalogue.findings.is_empty());
    }

    #[test]
    fn a_valid_cycle_and_direction_load_with_no_findings() {
        let dir = tempdir("valid");
        write(&dir, "direction.yaml", "vision: See far\nmission: Do the work\n");
        write(&dir, "2026-q4.yaml", MINIMAL_CYCLE);
        let catalogue = load(&dir);
        assert_eq!(catalogue.findings, Vec::new(), "{:?}", catalogue.findings);
        assert!(catalogue.direction.is_some());
        assert_eq!(catalogue.cycles.len(), 1);
        cleanup(&dir);
    }

    #[test]
    fn a_file_that_fails_to_parse_is_a_finding_and_does_not_stop_the_others() {
        let dir = tempdir("bad-parse");
        write(&dir, "broken.yaml", "not: [valid, cycle\n");
        write(&dir, "2026-q4.yaml", MINIMAL_CYCLE);
        let catalogue = load(&dir);
        assert_eq!(catalogue.cycles.len(), 1);
        assert_eq!(catalogue.findings.len(), 1);
        assert_eq!(catalogue.findings[0].kind, FindingKind::ParseFailed);
        assert_eq!(catalogue.findings[0].subject, "broken.yaml");
        cleanup(&dir);
    }

    #[test]
    fn a_cycle_id_that_differs_from_the_file_stem_is_a_finding() {
        let dir = tempdir("stem-mismatch");
        write(&dir, "not-the-id.yaml", MINIMAL_CYCLE);
        let catalogue = load(&dir);
        assert!(catalogue.cycles.is_empty());
        assert_eq!(catalogue.findings.len(), 1);
        assert_eq!(catalogue.findings[0].kind, FindingKind::StemMismatch);
        cleanup(&dir);
    }

    // -- duplicate ids -----------------------------------------------------

    #[test]
    fn a_duplicate_objective_id_is_a_finding() {
        let dir = tempdir("dup-objective");
        write(
            &dir,
            "2026-q4.yaml",
            "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
             objectives:\n\
             \x20\x20- id: obj\n\x20\x20\x20\x20title: A\n\x20\x20\x20\x20key_results: [{id: kr, title: K, kind: committed, manual: true, baseline: 0, target: 1}]\n\
             \x20\x20- id: obj\n\x20\x20\x20\x20title: B\n\x20\x20\x20\x20key_results: [{id: kr2, title: K, kind: committed, manual: true, baseline: 0, target: 1}]\n",
        );
        let catalogue = load(&dir);
        assert!(catalogue.findings.iter().any(|f| f.kind == FindingKind::DuplicateId && f.detail.contains("objective")));
        cleanup(&dir);
    }

    #[test]
    fn a_duplicate_key_result_id_is_a_finding() {
        let dir = tempdir("dup-kr");
        write(
            &dir,
            "2026-q4.yaml",
            "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
             objectives:\n\
             \x20\x20- id: obj\n\x20\x20\x20\x20title: A\n\x20\x20\x20\x20key_results:\n\
             \x20\x20\x20\x20\x20\x20- {id: kr, title: K1, kind: committed, manual: true, baseline: 0, target: 1}\n\
             \x20\x20\x20\x20\x20\x20- {id: kr, title: K2, kind: committed, manual: true, baseline: 0, target: 1}\n",
        );
        let catalogue = load(&dir);
        assert!(catalogue.findings.iter().any(|f| f.kind == FindingKind::DuplicateId && f.detail.contains("key result")));
        cleanup(&dir);
    }

    #[test]
    fn a_duplicate_roadmap_item_id_is_a_finding() {
        let dir = tempdir("dup-roadmap");
        write(
            &dir,
            "2026-q4.yaml",
            &format!(
                "{MINIMAL_CYCLE}roadmap:\n\
                 \x20\x20- {{id: item, title: A, lane: now, objectives: [obj]}}\n\
                 \x20\x20- {{id: item, title: B, lane: next, objectives: [obj]}}\n"
            ),
        );
        let catalogue = load(&dir);
        assert!(catalogue.findings.iter().any(|f| f.kind == FindingKind::DuplicateId && f.detail.contains("roadmap")));
        cleanup(&dir);
    }

    // -- dates ---------------------------------------------------------

    #[test]
    fn inverted_dates_are_a_finding() {
        let dir = tempdir("inverted");
        write(
            &dir,
            "2026-q4.yaml",
            "cycle: { id: 2026-q4, from: 2026-12-31, to: 2026-10-01 }\nobjectives: []\n",
        );
        let catalogue = load(&dir);
        assert!(catalogue.findings.iter().any(|f| f.kind == FindingKind::InvertedDates));
        cleanup(&dir);
    }

    #[test]
    fn overlapping_cycles_are_a_finding_naming_both() {
        let dir = tempdir("overlap");
        write(&dir, "2026-q4.yaml", "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\nobjectives: []\n");
        write(&dir, "2026-q4b.yaml", "cycle: { id: 2026-q4b, from: 2026-12-01, to: 2027-01-31 }\nobjectives: []\n");
        let catalogue = load(&dir);
        assert!(catalogue.findings.iter().any(|f| f.kind == FindingKind::OverlappingCycles));
        cleanup(&dir);
    }

    #[test]
    fn adjacent_cycles_that_only_touch_at_a_day_boundary_do_not_overlap() {
        let dir = tempdir("adjacent");
        write(&dir, "2026-q3.yaml", "cycle: { id: 2026-q3, from: 2026-07-01, to: 2026-09-30 }\nobjectives: []\n");
        write(&dir, "2026-q4.yaml", "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\nobjectives: []\n");
        let catalogue = load(&dir);
        assert!(!catalogue.findings.iter().any(|f| f.kind == FindingKind::OverlappingCycles), "{:?}", catalogue.findings);
        cleanup(&dir);
    }

    // -- metrics on key results, north_star, inputs -----------------------

    #[test]
    fn an_unknown_metric_on_a_key_result_is_a_finding() {
        let dir = tempdir("unknown-metric");
        write(
            &dir,
            "2026-q4.yaml",
            "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
             objectives:\n\
             \x20\x20- id: obj\n\x20\x20\x20\x20title: A\n\x20\x20\x20\x20key_results:\n\
             \x20\x20\x20\x20\x20\x20- {id: kr, title: K, kind: committed, metric: bogus_metric, baseline: 0, target: 1}\n",
        );
        let catalogue = load(&dir);
        assert!(catalogue.findings.iter().any(|f| f.kind == FindingKind::UnknownMetric));
        cleanup(&dir);
    }

    #[test]
    fn an_unavailable_metric_on_a_key_result_is_a_finding_naming_the_reason() {
        let dir = tempdir("unavailable-metric");
        write(
            &dir,
            "2026-q4.yaml",
            "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
             objectives:\n\
             \x20\x20- id: obj\n\x20\x20\x20\x20title: A\n\x20\x20\x20\x20key_results:\n\
             \x20\x20\x20\x20\x20\x20- {id: kr, title: K, kind: committed, metric: unit_cost, baseline: 0, target: 1}\n",
        );
        let catalogue = load(&dir);
        let f = catalogue.findings.iter().find(|f| f.kind == FindingKind::UnavailableMetric).unwrap();
        assert!(f.detail.contains("design §12.6"), "{}", f.detail);
        cleanup(&dir);
    }

    #[test]
    fn north_star_and_inputs_naming_an_unknown_metric_are_findings() {
        let dir = tempdir("direction-unknown");
        write(&dir, "direction.yaml", "vision: V\nmission: M\nnorth_star: {metric: bogus, why: because}\ninputs: [also_bogus]\n");
        let catalogue = load(&dir);
        assert_eq!(catalogue.findings.iter().filter(|f| f.kind == FindingKind::UnknownMetric).count(), 2);
        cleanup(&dir);
    }

    // -- WrongDirection ----------------------------------------------------

    #[test]
    fn a_lower_is_better_metric_with_a_rising_target_is_a_wrong_direction_finding() {
        // scrap_rate is lower-is-better; baseline 0.05 -> target 0.11 asks
        // to make it worse.
        let dir = tempdir("wrong-direction-lower");
        write(
            &dir,
            "2026-q4.yaml",
            "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
             objectives:\n\
             \x20\x20- id: obj\n\x20\x20\x20\x20title: A\n\x20\x20\x20\x20key_results:\n\
             \x20\x20\x20\x20\x20\x20- {id: kr, title: K, kind: aspirational, metric: scrap_rate, baseline: 0.05, target: 0.11}\n",
        );
        let catalogue = load(&dir);
        let f = catalogue.findings.iter().find(|f| f.kind == FindingKind::WrongDirection).unwrap();
        assert!(f.detail.contains("obj/kr"), "{}", f.detail);
        cleanup(&dir);
    }

    #[test]
    fn a_higher_is_better_metric_with_a_falling_target_is_a_wrong_direction_finding() {
        // first_pass_yield is higher-is-better; baseline 0.9 -> target 0.5
        // asks to make it worse.
        let dir = tempdir("wrong-direction-higher");
        write(
            &dir,
            "2026-q4.yaml",
            "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
             objectives:\n\
             \x20\x20- id: obj\n\x20\x20\x20\x20title: A\n\x20\x20\x20\x20key_results:\n\
             \x20\x20\x20\x20\x20\x20- {id: kr, title: K, kind: aspirational, metric: first_pass_yield, baseline: 0.9, target: 0.5}\n",
        );
        let catalogue = load(&dir);
        assert!(catalogue.findings.iter().any(|f| f.kind == FindingKind::WrongDirection));
        cleanup(&dir);
    }

    #[test]
    fn the_right_direction_for_either_kind_of_metric_is_no_finding() {
        let dir = tempdir("right-direction");
        write(
            &dir,
            "2026-q4.yaml",
            "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
             objectives:\n\
             \x20\x20- id: obj\n\x20\x20\x20\x20title: A\n\x20\x20\x20\x20key_results:\n\
             \x20\x20\x20\x20\x20\x20- {id: higher, title: H, kind: committed, metric: first_pass_yield, baseline: 0.7, target: 0.9}\n\
             \x20\x20\x20\x20\x20\x20- {id: lower, title: L, kind: aspirational, metric: scrap_rate, baseline: 0.11, target: 0.05}\n",
        );
        let catalogue = load(&dir);
        assert!(!catalogue.findings.iter().any(|f| f.kind == FindingKind::WrongDirection), "{:?}", catalogue.findings);
        cleanup(&dir);
    }

    #[test]
    fn a_manual_key_result_is_never_checked_for_direction() {
        // No metric means no Better to contradict, however baseline and
        // target are ordered.
        let dir = tempdir("wrong-direction-manual");
        write(
            &dir,
            "2026-q4.yaml",
            "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
             objectives:\n\
             \x20\x20- id: obj\n\x20\x20\x20\x20title: A\n\x20\x20\x20\x20key_results:\n\
             \x20\x20\x20\x20\x20\x20- {id: kr, title: K, kind: aspirational, manual: true, baseline: 12, target: 0}\n",
        );
        let catalogue = load(&dir);
        assert!(!catalogue.findings.iter().any(|f| f.kind == FindingKind::WrongDirection), "{:?}", catalogue.findings);
        cleanup(&dir);
    }

    // -- vanity / conflicting key results -----------------------------------

    #[test]
    fn a_key_result_with_neither_metric_nor_manual_is_a_vanity_finding() {
        let dir = tempdir("vanity");
        write(
            &dir,
            "2026-q4.yaml",
            "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
             objectives:\n\
             \x20\x20- id: obj\n\x20\x20\x20\x20title: A\n\x20\x20\x20\x20key_results:\n\
             \x20\x20\x20\x20\x20\x20- {id: kr, title: K, kind: committed, baseline: 0, target: 1}\n",
        );
        let catalogue = load(&dir);
        assert!(catalogue.findings.iter().any(|f| f.kind == FindingKind::VanityKeyResult));
        cleanup(&dir);
    }

    #[test]
    fn a_key_result_with_both_metric_and_manual_is_a_conflicting_finding() {
        let dir = tempdir("conflicting");
        write(
            &dir,
            "2026-q4.yaml",
            "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
             objectives:\n\
             \x20\x20- id: obj\n\x20\x20\x20\x20title: A\n\x20\x20\x20\x20key_results:\n\
             \x20\x20\x20\x20\x20\x20- {id: kr, title: K, kind: committed, metric: first_pass_yield, manual: true, baseline: 0, target: 1}\n",
        );
        let catalogue = load(&dir);
        assert!(catalogue.findings.iter().any(|f| f.kind == FindingKind::ConflictingKeyResult));
        cleanup(&dir);
    }

    // -- objective / cycle shape -----------------------------------------

    #[test]
    fn an_objective_with_no_key_results_is_a_finding() {
        let dir = tempdir("no-krs");
        write(&dir, "2026-q4.yaml", "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\nobjectives: [{id: obj, title: A, key_results: []}]\n");
        let catalogue = load(&dir);
        assert!(catalogue.findings.iter().any(|f| f.kind == FindingKind::ObjectiveWithoutKeyResults));
        cleanup(&dir);
    }

    #[test]
    fn more_than_five_objectives_is_a_finding_but_all_still_load() {
        let dir = tempdir("too-many");
        let mut text = "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\nobjectives:\n".to_string();
        for i in 0..6 {
            text.push_str(&format!(
                "\x20\x20- {{id: obj{i}, title: Objective {i}, key_results: [{{id: kr, title: K, kind: committed, manual: true, baseline: 0, target: 1}}]}}\n"
            ));
        }
        write(&dir, "2026-q4.yaml", &text);
        let catalogue = load(&dir);
        assert!(catalogue.findings.iter().any(|f| f.kind == FindingKind::TooManyObjectives));
        assert_eq!(catalogue.cycles[0].objectives.len(), 6);
        cleanup(&dir);
    }

    // -- roadmap ---------------------------------------------------------

    #[test]
    fn a_roadmap_item_with_no_objectives_is_an_orphan_finding() {
        let dir = tempdir("orphan");
        write(&dir, "2026-q4.yaml", &format!("{MINIMAL_CYCLE}roadmap:\n\x20\x20- {{id: item, title: I, lane: now, objectives: []}}\n"));
        let catalogue = load(&dir);
        assert!(catalogue.findings.iter().any(|f| f.kind == FindingKind::OrphanRoadmapItem));
        cleanup(&dir);
    }

    #[test]
    fn a_roadmap_item_linking_an_unknown_objective_is_a_finding() {
        let dir = tempdir("unknown-roadmap-obj");
        write(&dir, "2026-q4.yaml", &format!("{MINIMAL_CYCLE}roadmap:\n\x20\x20- {{id: item, title: I, lane: now, objectives: [nope]}}\n"));
        let catalogue = load(&dir);
        assert!(catalogue.findings.iter().any(|f| f.kind == FindingKind::UnknownRoadmapObjective));
        cleanup(&dir);
    }

    // -- BadIdShape ----------------------------------------------------

    #[test]
    fn a_non_slug_cycle_id_is_a_bad_id_shape_finding() {
        let dir = tempdir("bad-cycle-id");
        // The stem must still match `cycle.id`, so the file name is the
        // same not-a-slug string -- `is_slug` forbids the underscore.
        write(&dir, "2026_q4.yaml", "cycle: { id: 2026_q4, from: 2026-10-01, to: 2026-12-31 }\nobjectives: []\n");
        let catalogue = load(&dir);
        let f = catalogue.findings.iter().find(|f| f.kind == FindingKind::BadIdShape).unwrap();
        assert!(f.detail.contains("cycle id"), "{}", f.detail);
        cleanup(&dir);
    }

    #[test]
    fn a_non_slug_objective_id_is_a_bad_id_shape_finding() {
        let dir = tempdir("bad-objective-id");
        write(
            &dir,
            "2026-q4.yaml",
            "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
             objectives: [{id: Ship_Compliant, title: A, key_results: [{id: kr, title: K, kind: committed, manual: true, baseline: 0, target: 1}]}]\n",
        );
        let catalogue = load(&dir);
        let f = catalogue.findings.iter().find(|f| f.kind == FindingKind::BadIdShape).unwrap();
        assert!(f.detail.contains("objective id"), "{}", f.detail);
        cleanup(&dir);
    }

    #[test]
    fn a_non_slug_key_result_id_is_a_bad_id_shape_finding() {
        let dir = tempdir("bad-kr-id");
        write(
            &dir,
            "2026-q4.yaml",
            "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
             objectives: [{id: obj, title: A, key_results: [{id: Bad_Id, title: K, kind: committed, manual: true, baseline: 0, target: 1}]}]\n",
        );
        let catalogue = load(&dir);
        let f = catalogue.findings.iter().find(|f| f.kind == FindingKind::BadIdShape).unwrap();
        assert!(f.detail.contains("key result id"), "{}", f.detail);
        cleanup(&dir);
    }

    #[test]
    fn a_non_slug_roadmap_item_id_is_a_bad_id_shape_finding() {
        let dir = tempdir("bad-roadmap-id");
        write(&dir, "2026-q4.yaml", &format!("{MINIMAL_CYCLE}roadmap:\n\x20\x20- {{id: Bad_Item, title: I, lane: now, objectives: [obj]}}\n"));
        let catalogue = load(&dir);
        let f = catalogue.findings.iter().find(|f| f.kind == FindingKind::BadIdShape).unwrap();
        assert!(f.detail.contains("roadmap item id"), "{}", f.detail);
        cleanup(&dir);
    }

    #[test]
    fn a_bad_id_shape_makes_a_bare_goal_tasks_done_metric_unbindable_but_never_panics() {
        // The exact failure mode `BadIdShape` is meant to catch early: a
        // badly-shaped key-result id can never be matched by a check-in or
        // a `goal=` label, and `goal_tasks_done`'s auto-binding can't even
        // construct a valid metric id from it.
        let dir = tempdir("bad-id-goal-tasks-done");
        write(
            &dir,
            "2026-q4.yaml",
            "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
             objectives: [{id: obj, title: A, key_results: [{id: Bad_Id, title: K, kind: committed, metric: goal_tasks_done, baseline: 0, target: 1}]}]\n",
        );
        let catalogue = load(&dir);
        assert!(catalogue.findings.iter().any(|f| f.kind == FindingKind::BadIdShape));
        assert!(catalogue.findings.iter().any(|f| f.kind == FindingKind::UnknownMetric));
        cleanup(&dir);
    }

    #[test]
    fn slug_shaped_ids_throughout_are_no_bad_id_shape_finding() {
        let dir = tempdir("good-id-shapes");
        write(&dir, "2026-q4.yaml", &format!("{MINIMAL_CYCLE}roadmap:\n\x20\x20- {{id: item-1, title: I, lane: now, objectives: [obj]}}\n"));
        let catalogue = load(&dir);
        assert!(!catalogue.findings.iter().any(|f| f.kind == FindingKind::BadIdShape), "{:?}", catalogue.findings);
        cleanup(&dir);
    }

    // -- aligns_to ---------------------------------------------------------

    #[test]
    fn aligns_to_an_unknown_objective_is_a_finding() {
        let dir = tempdir("unknown-aligns");
        write(
            &dir,
            "2026-q4.yaml",
            "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
             objectives: [{id: obj, title: A, aligns_to: nope, key_results: [{id: kr, title: K, kind: committed, manual: true, baseline: 0, target: 1}]}]\n",
        );
        let catalogue = load(&dir);
        assert!(catalogue.findings.iter().any(|f| f.kind == FindingKind::UnknownAlignsTo));
        cleanup(&dir);
    }

    #[test]
    fn aligns_to_itself_is_a_cyclic_finding() {
        let dir = tempdir("self-cycle");
        write(
            &dir,
            "2026-q4.yaml",
            "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
             objectives: [{id: obj, title: A, aligns_to: obj, key_results: [{id: kr, title: K, kind: committed, manual: true, baseline: 0, target: 1}]}]\n",
        );
        let catalogue = load(&dir);
        assert!(catalogue.findings.iter().any(|f| f.kind == FindingKind::CyclicAlignsTo));
        cleanup(&dir);
    }

    #[test]
    fn a_longer_aligns_to_cycle_is_a_finding_for_every_objective_in_it() {
        let dir = tempdir("long-cycle");
        write(
            &dir,
            "2026-q4.yaml",
            "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
             objectives:\n\
             \x20\x20- {id: a, title: A, aligns_to: b, key_results: [{id: kr, title: K, kind: committed, manual: true, baseline: 0, target: 1}]}\n\
             \x20\x20- {id: b, title: B, aligns_to: a, key_results: [{id: kr, title: K, kind: committed, manual: true, baseline: 0, target: 1}]}\n",
        );
        let catalogue = load(&dir);
        let cyclic: Vec<&str> = catalogue
            .findings
            .iter()
            .filter(|f| f.kind == FindingKind::CyclicAlignsTo)
            .map(|f| f.detail.as_str())
            .collect();
        assert_eq!(cyclic.len(), 2, "{cyclic:?}");
        cleanup(&dir);
    }

    #[test]
    fn aligning_to_a_valid_parent_in_the_same_cycle_is_no_finding() {
        let dir = tempdir("valid-aligns");
        write(
            &dir,
            "2026-q4.yaml",
            "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
             objectives:\n\
             \x20\x20- {id: parent, title: P, key_results: [{id: kr, title: K, kind: committed, manual: true, baseline: 0, target: 1}]}\n\
             \x20\x20- {id: child, title: C, aligns_to: parent, key_results: [{id: kr, title: K, kind: committed, manual: true, baseline: 0, target: 1}]}\n",
        );
        let catalogue = load(&dir);
        assert_eq!(catalogue.findings, Vec::new(), "{:?}", catalogue.findings);
        cleanup(&dir);
    }

    // -- KrRef -----------------------------------------------------------

    #[test]
    fn a_kr_ref_round_trips_through_its_string_form() {
        let r = KrRef::new("ship-compliant", "cra-open-zero");
        assert_eq!(r.to_string(), "ship-compliant/cra-open-zero");
        assert_eq!("ship-compliant/cra-open-zero".parse::<KrRef>().unwrap(), r);
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, "\"ship-compliant/cra-open-zero\"");
        assert_eq!(serde_json::from_str::<KrRef>(&json).unwrap(), r);
    }

    #[test]
    fn a_kr_ref_refuses_the_wrong_shape() {
        assert!("no-slash".parse::<KrRef>().is_err());
        assert!("Upper/case".parse::<KrRef>().is_err());
        assert!("a/b/c".parse::<KrRef>().is_err());
        assert!("/kr".parse::<KrRef>().is_err());
        assert!("obj/".parse::<KrRef>().is_err());
    }

    // -- bound_metric --------------------------------------------------

    #[test]
    fn a_bare_goal_tasks_done_metric_binds_to_this_key_results_own_ids() {
        let kr = KeyResult {
            id: "cra-open-zero".into(),
            title: "T".into(),
            kind: KrKind::Committed,
            metric: Some(MetricId::new("goal_tasks_done").unwrap()),
            manual: false,
            baseline: 0.0,
            target: 1.0,
        };
        let bound = kr.bound_metric("ship-compliant").unwrap();
        assert_eq!(bound.as_str(), "goal_tasks_done.ship-compliant.cra-open-zero");
        assert!(metrics::resolve(&bound).is_ok());
    }

    #[test]
    fn an_already_bound_metric_passes_through_unchanged() {
        let kr = KeyResult {
            id: "kr".into(),
            title: "T".into(),
            kind: KrKind::Committed,
            metric: Some(MetricId::new("compliance.cra").unwrap()),
            manual: false,
            baseline: 0.0,
            target: 1.0,
        };
        assert_eq!(kr.bound_metric("obj").unwrap().as_str(), "compliance.cra");
    }

    // -- score ---------------------------------------------------------

    #[test]
    fn score_reads_a_higher_is_better_key_result() {
        assert_eq!(score(0.2, 1.0, 0.2), 0.0);
        assert!(approx(score(0.2, 1.0, 0.6), 0.5));
        assert_eq!(score(0.2, 1.0, 1.0), 1.0);
        assert_eq!(score(0.2, 1.0, 1.5), 1.0, "overshoot clamps to 1.0");
        assert_eq!(score(0.2, 1.0, 0.0), 0.0, "undershoot clamps to 0.0");
    }

    #[test]
    fn score_reads_a_lower_is_better_key_result() {
        assert_eq!(score(0.11, 0.05, 0.11), 0.0);
        assert!(approx(score(0.11, 0.05, 0.08), 0.5));
        assert_eq!(score(0.11, 0.05, 0.05), 1.0);
        assert_eq!(score(0.11, 0.05, 0.0), 1.0, "beating the target clamps to 1.0");
        assert_eq!(score(0.11, 0.05, 0.20), 0.0, "worse than baseline clamps to 0.0");
    }

    #[test]
    fn score_of_a_baseline_equal_to_target_is_trivially_one() {
        assert_eq!(score(1.0, 1.0, 0.0), 1.0);
        assert_eq!(score(0.0, 0.0, 999.0), 1.0);
    }

    #[test]
    fn score_never_returns_nan_for_a_non_finite_value() {
        assert_eq!(score(0.0, 1.0, f64::NAN), 0.0);
        assert_eq!(score(0.0, 1.0, f64::INFINITY), 1.0);
    }

    // -- band ------------------------------------------------------------

    #[test]
    fn a_committed_key_result_is_green_only_at_a_perfect_score() {
        assert_eq!(band(KrKind::Committed, 1.0), Band::Green);
        assert_eq!(band(KrKind::Committed, 0.99), Band::Yellow);
        assert_eq!(band(KrKind::Committed, 0.7), Band::Yellow);
        assert_eq!(band(KrKind::Committed, 0.69), Band::Red);
        assert_eq!(band(KrKind::Committed, 0.0), Band::Red);
    }

    #[test]
    fn an_aspirational_key_result_is_green_once_clearly_winning() {
        assert_eq!(band(KrKind::Aspirational, 1.0), Band::Green);
        assert_eq!(band(KrKind::Aspirational, 0.7), Band::Green);
        assert_eq!(band(KrKind::Aspirational, 0.69), Band::Yellow);
        assert_eq!(band(KrKind::Aspirational, 0.4), Band::Yellow);
        assert_eq!(band(KrKind::Aspirational, 0.39), Band::Red);
    }

    // -- cycle status / elapsed / current_cycle -----------------------------

    fn q4_2026() -> Cycle {
        Cycle {
            cycle: CycleHeader {
                id: "2026-q4".into(),
                from: NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
                to: NaiveDate::from_ymd_opt(2026, 12, 31).unwrap(),
            },
            objectives: Vec::new(),
            roadmap: Vec::new(),
        }
    }

    fn at(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    #[test]
    fn cycle_status_reads_future_current_and_past() {
        let cycle = q4_2026();
        assert_eq!(cycle_status(&cycle, at("2026-09-30T23:59:59Z")), CycleStatus::Future);
        assert_eq!(cycle_status(&cycle, at("2026-10-01T00:00:00Z")), CycleStatus::Current);
        assert_eq!(cycle_status(&cycle, at("2026-12-31T23:59:59Z")), CycleStatus::Current);
        assert_eq!(cycle_status(&cycle, at("2027-01-01T00:00:00Z")), CycleStatus::Past);
    }

    #[test]
    fn elapsed_fraction_reads_zero_at_the_start_one_at_the_end_and_a_midpoint() {
        let cycle = q4_2026();
        assert_eq!(elapsed_fraction(&cycle, at("2026-09-01T00:00:00Z")), 0.0);
        assert_eq!(elapsed_fraction(&cycle, at("2026-10-01T00:00:00Z")), 0.0);
        assert_eq!(elapsed_fraction(&cycle, at("2027-06-01T00:00:00Z")), 1.0);
        let mid = elapsed_fraction(&cycle, at("2026-11-15T12:00:00Z"));
        assert!((0.45..=0.55).contains(&mid), "{mid}");
    }

    #[test]
    fn current_cycle_picks_the_one_containing_now_and_none_otherwise() {
        let q3 = Cycle {
            cycle: CycleHeader {
                id: "2026-q3".into(),
                from: NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(),
                to: NaiveDate::from_ymd_opt(2026, 9, 30).unwrap(),
            },
            objectives: Vec::new(),
            roadmap: Vec::new(),
        };
        let q4 = q4_2026();
        let cycles = vec![q3.clone(), q4.clone()];
        assert_eq!(current_cycle(&cycles, at("2026-11-01T00:00:00Z")).unwrap().id(), "2026-q4");
        assert_eq!(current_cycle(&cycles, at("2026-08-01T00:00:00Z")).unwrap().id(), "2026-q3");
        assert!(current_cycle(&cycles, at("2030-01-01T00:00:00Z")).is_none());
    }

    // -- evaluate: manual key result -----------------------------------

    #[test]
    fn a_manual_key_result_takes_its_value_from_the_latest_check_in() {
        let objective = Objective {
            id: "obj".into(),
            title: "Objective".into(),
            scope: None,
            aligns_to: None,
            key_results: vec![KeyResult {
                id: "kr".into(),
                title: "K".into(),
                kind: KrKind::Aspirational,
                metric: None,
                manual: true,
                baseline: 0.0,
                target: 12.0,
            }],
        };
        let cycle = Cycle { cycle: q4_2026().cycle, objectives: vec![objective], roadmap: Vec::new() };
        let checkins = vec![
            CheckIn {
                id: "c1".into(),
                kr: KrRef::new("obj", "kr"),
                value: 3.0,
                confidence: 5,
                note: None,
                by: "alice".into(),
                at: at("2026-10-15T00:00:00Z"),
            },
            CheckIn {
                id: "c2".into(),
                kr: KrRef::new("obj", "kr"),
                value: 9.0,
                confidence: 8,
                note: None,
                by: "alice".into(),
                at: at("2026-11-20T00:00:00Z"),
            },
        ];
        let report = evaluate(&cycle, &BTreeMap::new(), &checkins, at("2026-12-01T00:00:00Z"));
        let kr = &report.objectives[0].key_results[0];
        assert_eq!(kr.value, Some(9.0));
        assert_eq!(kr.confidence, Some(8));
        assert!(kr.source.contains("alice"));
        assert!(kr.reasons.is_empty());
    }

    #[test]
    fn a_manual_key_result_with_no_check_in_is_unscored_with_a_reason() {
        let objective = Objective {
            id: "obj".into(),
            title: "Objective".into(),
            scope: None,
            aligns_to: None,
            key_results: vec![KeyResult {
                id: "kr".into(),
                title: "K".into(),
                kind: KrKind::Aspirational,
                metric: None,
                manual: true,
                baseline: 0.0,
                target: 12.0,
            }],
        };
        let cycle = Cycle { cycle: q4_2026().cycle, objectives: vec![objective], roadmap: Vec::new() };
        let report = evaluate(&cycle, &BTreeMap::new(), &[], at("2026-12-01T00:00:00Z"));
        let kr = &report.objectives[0].key_results[0];
        assert_eq!(kr.value, None);
        assert_eq!(kr.score, None);
        assert_eq!(kr.band, None);
        assert_eq!(kr.reasons, vec!["no check-in yet".to_string()]);
        assert_eq!(report.objectives[0].score, None);
    }

    // -- evaluate: metric-backed key result -----------------------------

    #[test]
    fn a_metric_backed_key_result_scores_from_its_computed_value() {
        let objective = Objective {
            id: "ship-compliant".into(),
            title: "Objective".into(),
            scope: None,
            aligns_to: None,
            key_results: vec![KeyResult {
                id: "cra-open-zero".into(),
                title: "K".into(),
                kind: KrKind::Committed,
                metric: Some(MetricId::new("compliance.cra").unwrap()),
                manual: false,
                baseline: 0.2,
                target: 1.0,
            }],
        };
        let cycle = Cycle { cycle: q4_2026().cycle, objectives: vec![objective], roadmap: Vec::new() };
        let mut values = BTreeMap::new();
        values.insert(
            MetricId::new("compliance.cra").unwrap(),
            MetricValue { id: MetricId::new("compliance.cra").unwrap(), value: Some(0.6), as_of: at("2026-11-01T00:00:00Z"), reason: None },
        );
        let report = evaluate(&cycle, &values, &[], at("2026-11-15T00:00:00Z"));
        let kr = &report.objectives[0].key_results[0];
        assert_eq!(kr.value, Some(0.6));
        assert!(approx(kr.score.unwrap(), 0.5), "{:?}", kr.score);
        // Committed key results are graded on the strict curve: yellow
        // only starts at 0.7, so a score of 0.5 is still red.
        assert_eq!(kr.band, Some(Band::Red));
    }

    #[test]
    fn a_metric_marked_unavailable_this_time_carries_the_reason_and_stays_unscored() {
        let objective = Objective {
            id: "obj".into(),
            title: "Objective".into(),
            scope: None,
            aligns_to: None,
            key_results: vec![KeyResult {
                id: "kr".into(),
                title: "K".into(),
                kind: KrKind::Committed,
                metric: Some(MetricId::new("compliance.dsgvo").unwrap()),
                manual: false,
                baseline: 0.0,
                target: 1.0,
            }],
        };
        let cycle = Cycle { cycle: q4_2026().cycle, objectives: vec![objective], roadmap: Vec::new() };
        let mut values = BTreeMap::new();
        values.insert(
            MetricId::new("compliance.dsgvo").unwrap(),
            MetricValue {
                id: MetricId::new("compliance.dsgvo").unwrap(),
                value: None,
                as_of: at("2026-11-01T00:00:00Z"),
                reason: Some("no dsgvo.yaml catalogue".into()),
            },
        );
        let report = evaluate(&cycle, &values, &[], at("2026-11-15T00:00:00Z"));
        let kr = &report.objectives[0].key_results[0];
        assert_eq!(kr.value, None);
        assert_eq!(kr.score, None);
        assert!(kr.reasons[0].contains("no dsgvo.yaml catalogue"), "{:?}", kr.reasons);
    }

    #[test]
    fn a_goal_tasks_done_key_result_binds_automatically_and_is_looked_up_by_the_bound_id() {
        let objective = Objective {
            id: "ship-compliant".into(),
            title: "Objective".into(),
            scope: None,
            aligns_to: None,
            key_results: vec![KeyResult {
                id: "sbom-done".into(),
                title: "K".into(),
                kind: KrKind::Committed,
                metric: Some(MetricId::new("goal_tasks_done").unwrap()),
                manual: false,
                baseline: 0.0,
                target: 1.0,
            }],
        };
        let cycle = Cycle { cycle: q4_2026().cycle, objectives: vec![objective], roadmap: Vec::new() };
        let bound = MetricId::new("goal_tasks_done.ship-compliant.sbom-done").unwrap();
        let mut values = BTreeMap::new();
        values.insert(bound.clone(), MetricValue { id: bound, value: Some(1.0), as_of: at("2026-11-01T00:00:00Z"), reason: None });
        let report = evaluate(&cycle, &values, &[], at("2026-11-15T00:00:00Z"));
        assert_eq!(report.objectives[0].key_results[0].score, Some(1.0));
    }

    // -- on_pace -----------------------------------------------------------

    #[test]
    fn on_pace_is_set_for_committed_and_never_for_aspirational() {
        let objective = Objective {
            id: "obj".into(),
            title: "Objective".into(),
            scope: None,
            aligns_to: None,
            key_results: vec![
                KeyResult {
                    id: "committed".into(),
                    title: "C".into(),
                    kind: KrKind::Committed,
                    metric: Some(MetricId::new("first_pass_yield").unwrap()),
                    manual: false,
                    baseline: 0.0,
                    target: 1.0,
                },
                KeyResult {
                    id: "aspirational".into(),
                    title: "A".into(),
                    kind: KrKind::Aspirational,
                    metric: Some(MetricId::new("first_pass_yield").unwrap()),
                    manual: false,
                    baseline: 0.0,
                    target: 1.0,
                },
            ],
        };
        let cycle = Cycle { cycle: q4_2026().cycle, objectives: vec![objective], roadmap: Vec::new() };
        let mut values = BTreeMap::new();
        values.insert(
            MetricId::new("first_pass_yield").unwrap(),
            MetricValue { id: MetricId::new("first_pass_yield").unwrap(), value: Some(0.1), as_of: at("2026-11-15T00:00:00Z"), reason: None },
        );
        // Halfway through the cycle, a committed KR scoring 0.1 is behind.
        let report = evaluate(&cycle, &values, &[], at("2026-11-15T12:00:00Z"));
        assert_eq!(report.objectives[0].key_results[0].on_pace, Some(false));
        assert_eq!(report.objectives[0].key_results[1].on_pace, None);
    }

    // -- report_summary ------------------------------------------------

    #[test]
    fn report_summary_counts_bands_apart_for_committed_and_aspirational() {
        let report = CycleReport {
            cycle_id: "2026-q4".into(),
            status: CycleStatus::Current,
            elapsed: 0.5,
            objectives: vec![ObjectiveResult {
                objective: "obj".into(),
                title: "Objective".into(),
                score: Some(0.6),
                key_results: vec![
                    KrResult {
                        kr: KrRef::new("obj", "a"),
                        title: "A".into(),
                        kind: KrKind::Committed,
                        value: Some(1.0),
                        score: Some(1.0),
                        band: Some(Band::Green),
                        on_pace: Some(true),
                        source: "metric a".into(),
                        reasons: Vec::new(),
                        confidence: None,
                    },
                    KrResult {
                        kr: KrRef::new("obj", "b"),
                        title: "B".into(),
                        kind: KrKind::Aspirational,
                        value: None,
                        score: None,
                        band: None,
                        on_pace: None,
                        source: "manual (no check-in yet)".into(),
                        reasons: vec!["no check-in yet".into()],
                        confidence: None,
                    },
                ],
            }],
        };
        let summary = report_summary(&report);
        assert_eq!(summary.committed, BandCounts { green: 1, yellow: 0, red: 0, unscored: 0 });
        assert_eq!(summary.aspirational, BandCounts { green: 0, yellow: 0, red: 0, unscored: 1 });
    }

    // -- examples --------------------------------------------------------

    #[test]
    fn examples_goals_load_with_zero_findings() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/goals");
        let catalogue = load(&dir);
        assert_eq!(catalogue.findings, Vec::new(), "{:?}", catalogue.findings);
        assert!(catalogue.direction.is_some());
        assert_eq!(catalogue.cycles.len(), 1);
        assert_eq!(catalogue.cycles[0].id(), "2026-q4");
        assert!(catalogue.cycles[0].objectives.len() >= 2, "{:?}", catalogue.cycles[0].objectives);
    }
}
