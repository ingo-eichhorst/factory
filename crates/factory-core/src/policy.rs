//! A policy is a catalogue of **controls** -- a regulation, a standard, or
//! the company's own best practice, broken into checkable items -- plus a
//! pure computation of whether the evidence Factory already has satisfies
//! each one. Nothing here evaluates a policy to *allow* or *forbid*
//! anything: enforcement stays where it already lives, in roles, sandboxes
//! and the Secrets seam. This module only answers "is there current
//! evidence?" -- a picture of the plant, never a rule language evaluated at
//! runtime (ADR 0004, which design §8 explicitly defers). See
//! `.specs/adr/0004-policy-controls.md` in the Business Factory root repo.
//!
//! ## Storage
//!
//! A framework lives as one file, `<root>/.factory/policies/<framework>.yaml`
//! -- authored content, like `.factory/knowledge/` and `.factory/datasets/`:
//! hand-written, re-parsed on every read, never written by Factory.
//! [`load_all`] walks the directory and returns every catalogue that parsed,
//! plus a [`Finding`] for each one that did not. A missing directory is
//! empty, not an error; one bad file never stops the others from loading.
//!
//! ## Applicability
//!
//! Which frameworks apply to a scope, and any per-scope tightening or `n/a`,
//! comes from the same root-to-leaf chain roles use
//! (`Engine::roles_for` in `factory-daemon`), just for policies instead --
//! parsing that chain out of the live config is `#76`'s job, not this
//! module's. [`PolicyLayer`] is what that chain hands to [`applicable`]:
//! one layer per scope, root first. Unlike roles, where the nearest
//! definition wins outright, a layer may only *add* a framework or *tighten*
//! a control -- shorten a `max_age`, mark something `n/a` with a rationale --
//! never drop or relax something an ancestor already committed the company
//! to. A layer that tries to loosen something, or names a framework or
//! control nothing applicable resolves to, is not refused -- the file (or
//! scope) that did it keeps working, but a [`Finding`] says so.
//!
//! ## Evidence and evaluation
//!
//! [`evaluate`] takes the applicable controls, a bundle of [`Evidence`]
//! Factory already has lying around, and `now`, and produces a
//! [`ControlStatus`] per control -- pure, so a caller passes `now` in rather
//! than this module reading the clock. v1 (this ticket) only *evaluates*
//! two of the ADR's seven check kinds, `knowledge` and `attestation`; the
//! catalogue format already parses every kind the ADR names (`task`,
//! `workflow`, `gate`, `roles`, `sandbox`, `secrets`, `daemon`) so later
//! tickets (`#81`) only have to teach `evaluate` what those checks mean, not
//! change what a catalogue can say. A check this module cannot yet evaluate
//! never satisfies anything -- its `unevaluated` reason says so rather than
//! silently counting as met.
//!
//! `Evidence` is deliberately thin in v1 (a set of knowledge tags and a list
//! of attestations) and is expected to grow a field per check kind as later
//! tickets teach `evaluate` to read it; every field it has is `#[serde(default)]`
//! so an older caller building one is still a valid, if incomplete, bundle.
//!
//! `maps_to` lets one piece of evidence satisfy more than one control: two
//! frameworks that both require, say, an SBOM should not need separate proof
//! of the same fact. The link is declared one way in the YAML but read both
//! ways, one hop only -- deliberately not transitive, so a chain of loosely
//! related controls can never bootstrap each other into looking compliant.
//!
//! One spelling amends the ADR: its own example tags evidence
//! `control:<framework>/<id>`, but an inline Obsidian tag cannot contain `:`
//! or `.` (`knowledge::is_tag_char`), so the default tag this module looks
//! for is `control/<framework>/<id>` instead -- a `/` is a character
//! `is_tag_char` already accepts, and every page frontmatter or `#tag` link
//! keeps working unmodified.

use crate::dataset::is_slug;
use crate::role::Grant;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

// ============================================================== catalogue

/// Which of the three the company's commitment to a framework is. Only
/// `regulation` and `standard` controls count towards a rollup's
/// `compliant`; `best_practice` controls are shown but never counted --
/// nobody is out of compliance for skipping a recommendation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Regulation,
    Standard,
    /// Written `best-practice` in the authored YAML (matching the ADR's own
    /// spelling); `best_practice` is also accepted, since every other enum
    /// in this crate is plain snake_case on the wire.
    #[serde(rename = "best-practice", alias = "best_practice")]
    BestPractice,
}

/// A freshness window, written in the catalogue as `Nd`, `Nh`, or `Nw`
/// (days, hours, weeks). Stored as whole hours, so two durations compare
/// and take a minimum exactly, with nothing to round at the edges.
/// `chrono::Duration` has no serde support of its own and no `Ord`, so this
/// is its own small type rather than a wrapper around that one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Duration {
    hours: u64,
}

impl Duration {
    pub fn from_hours(hours: u64) -> Self {
        Self { hours }
    }

    pub fn as_hours(&self) -> u64 {
        self.hours
    }
}

impl std::str::FromStr for Duration {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let bad = || format!("{s:?} is not a duration like 30d, 12h, or 2w");
        if s.len() < 2 {
            return Err(bad());
        }
        let (num, unit) = s.split_at(s.len() - 1);
        let n: u64 = num.parse().map_err(|_| bad())?;
        let hours = match unit {
            "h" => Some(n),
            "d" => n.checked_mul(24),
            "w" => n.checked_mul(24 * 7),
            _ => None,
        }
        .ok_or_else(bad)?;
        Ok(Duration { hours })
    }
}

impl std::fmt::Display for Duration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.hours != 0 && self.hours.is_multiple_of(24 * 7) {
            write!(f, "{}w", self.hours / (24 * 7))
        } else if self.hours != 0 && self.hours.is_multiple_of(24) {
            write!(f, "{}d", self.hours / 24)
        } else {
            write!(f, "{}h", self.hours)
        }
    }
}

impl TryFrom<String> for Duration {
    type Error = String;

    fn try_from(s: String) -> std::result::Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<Duration> for String {
    fn from(d: Duration) -> String {
        d.to_string()
    }
}

/// A control's stable identity, everywhere but inside the file that defines
/// it: `framework/id`, e.g. `cra/annex-i-2-1`. Serializes as exactly that
/// string, never as a two-field object -- it is a name, not a record.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ControlRef {
    pub framework: String,
    pub id: String,
}

impl ControlRef {
    pub fn new(framework: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            framework: framework.into(),
            id: id.into(),
        }
    }

    /// `control/<framework>/<id>` -- the knowledge tag a `knowledge` check
    /// looks for when the control does not name one of its own. Amends the
    /// ADR's `control:<framework>/<id>`; see the module doc comment.
    pub fn default_tag(&self) -> String {
        format!("control/{}/{}", self.framework, self.id)
    }
}

impl std::fmt::Display for ControlRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.framework, self.id)
    }
}

impl std::str::FromStr for ControlRef {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let (framework, id) = s
            .split_once('/')
            .ok_or_else(|| format!("{s:?} is not framework/id"))?;
        if !is_slug(framework) || !is_slug(id) {
            return Err(format!(
                "{s:?} is not framework/id, each matching [a-z0-9][a-z0-9-]*"
            ));
        }
        Ok(ControlRef {
            framework: framework.to_string(),
            id: id.to_string(),
        })
    }
}

impl TryFrom<String> for ControlRef {
    type Error = String;

    fn try_from(s: String) -> std::result::Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<ControlRef> for String {
    fn from(r: ControlRef) -> String {
        r.to_string()
    }
}

/// One thing Factory already records that can stand as evidence for a
/// control. Every kind the ADR names is parsed here, even though `evaluate`
/// only understands `knowledge` and `attestation` in v1 -- a catalogue
/// author can write the whole shape today, and a later ticket only has to
/// teach evaluation, never change the file format underneath an author who
/// already wrote one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "check", rename_all = "snake_case", deny_unknown_fields)]
pub enum Check {
    /// Satisfied when a page in the knowledge vault carries `tag` (default
    /// `control/<framework>/<id>`, see `ControlRef::default_tag`).
    Knowledge {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
    },
    /// Satisfied by an unexpired, unwithdrawn `Attestation` recorded for
    /// this control.
    Attestation,
    /// A scheduled task's newest run is `done` within `max_age`. Parsed now,
    /// evaluated in `#81`.
    Task {
        task: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_age: Option<Duration>,
    },
    /// Same as `task`, for a workflow. Parsed now, evaluated in `#81`.
    Workflow {
        workflow: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_age: Option<Duration>,
    },
    /// A dataset case's gate passed in a bench run within `max_age`. Parsed
    /// now, evaluated in `#81`.
    Gate {
        dataset: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        case: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_age: Option<Duration>,
    },
    /// A stated condition on role grants holds, e.g. no role below the root
    /// holds any of `forbid`. Parsed now, evaluated in `#81`.
    Roles {
        #[serde(default)]
        forbid: Vec<Grant>,
    },
    /// Every agent in the scope declares a sandbox. Parsed now, evaluated in
    /// `#81`.
    Sandbox,
    /// Secrets reach agents only through the Secrets seam. Parsed now,
    /// evaluated in `#81`.
    Secrets,
    /// A stated daemon or backup fact holds. Parsed now, evaluated in `#81`.
    Daemon { fact: String },
}

impl Check {
    /// The `check:` value this variant was written as -- used to build a
    /// reason string without duplicating the wire spelling by hand.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Check::Knowledge { .. } => "knowledge",
            Check::Attestation => "attestation",
            Check::Task { .. } => "task",
            Check::Workflow { .. } => "workflow",
            Check::Gate { .. } => "gate",
            Check::Roles { .. } => "roles",
            Check::Sandbox => "sandbox",
            Check::Secrets => "secrets",
            Check::Daemon { .. } => "daemon",
        }
    }

    /// This check's own `max_age`, for the kinds that carry one.
    fn own_max_age(&self) -> Option<Duration> {
        match self {
            Check::Task { max_age, .. } | Check::Workflow { max_age, .. } | Check::Gate { max_age, .. } => *max_age,
            _ => None,
        }
    }
}

/// One control within a framework: something that is either currently
/// evidenced or is not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Control {
    /// `[a-z0-9][a-z0-9-]*`, referenced elsewhere as `<framework>/<id>`.
    pub id: String,
    pub title: String,
    /// Overrides the catalogue's own `kind` for this control alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Kind>,
    /// The default freshness window for this control's evidence, folded
    /// into the minimum `applicable` computes alongside every check's own
    /// `max_age` and every scope's `tighten`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age: Option<Duration>,
    /// Equivalent controls in other frameworks: evidence that satisfies this
    /// control also satisfies these, one hop, and the other way around.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub maps_to: Vec<ControlRef>,
    /// Free text a later ticket (`#83`) offers when this control is `open`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
    #[serde(default)]
    pub evidence: Vec<Check>,
}

/// One framework's whole catalogue, exactly as authored at
/// `<root>/.factory/policies/<framework>.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalogue {
    /// Must equal the file's stem -- checked by `load_all`, not here, since
    /// only the loader knows the path a value came from.
    pub framework: String,
    pub title: String,
    pub kind: Kind,
    #[serde(default)]
    pub controls: Vec<Control>,
}

/// One of the four things `load_all` checks for. A finding never stops
/// another file, or another control in the same file, from loading --
/// `applicable`'s findings share this type and this same property: a
/// mistake at one scope is reported, not allowed to take the rest down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    ParseFailed,
    DuplicateControl,
    FrameworkMismatch,
    UnknownMapsTo,
    UnknownFramework,
    UnknownControl,
    LooseningHasNoEffect,
    EmptyRationale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub kind: FindingKind,
    /// The file (`load_all`) or the scope (`applicable`) the finding is
    /// about -- the two callers name different kinds of thing here, but
    /// both are "where to go look".
    pub subject: String,
    pub detail: String,
}

/// `<root>/.factory/policies`, the catalogue directory.
pub fn policies_dir(root: &Path) -> PathBuf {
    root.join(".factory").join("policies")
}

/// Load every `<framework>.yaml` in `dir`. A missing directory is empty, not
/// an error. Each file is independent: one that fails to parse, names a
/// `framework` other than its own file stem, or repeats a control `id` is a
/// [`Finding`] naming the file, and every other file still loads. A
/// `maps_to` that names a control found in none of the files that did load
/// is a second-pass finding, checked once every file that could load has.
pub fn load_all(dir: &Path) -> (Vec<Catalogue>, Vec<Finding>) {
    let mut findings = Vec::new();
    let mut by_framework: BTreeMap<String, Catalogue> = BTreeMap::new();

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
                findings.push(Finding {
                    kind: FindingKind::ParseFailed,
                    subject: file_name,
                    detail: format!("reading: {e}"),
                });
                continue;
            }
        };
        let catalogue: Catalogue = match serde_yaml_ng::from_str(&text) {
            Ok(c) => c,
            Err(e) => {
                findings.push(Finding {
                    kind: FindingKind::ParseFailed,
                    subject: file_name,
                    detail: format!("parsing: {e}"),
                });
                continue;
            }
        };
        if catalogue.framework != stem {
            findings.push(Finding {
                kind: FindingKind::FrameworkMismatch,
                subject: file_name,
                detail: format!(
                    "framework {:?} does not match the file name {:?}",
                    catalogue.framework, stem
                ),
            });
            continue;
        }

        let mut seen_ids: BTreeSet<String> = BTreeSet::new();
        let mut controls = Vec::with_capacity(catalogue.controls.len());
        for control in catalogue.controls {
            if !seen_ids.insert(control.id.clone()) {
                findings.push(Finding {
                    kind: FindingKind::DuplicateControl,
                    subject: file_name.clone(),
                    detail: format!("duplicate control id {:?}", control.id),
                });
                continue;
            }
            controls.push(control);
        }

        by_framework.insert(
            catalogue.framework.clone(),
            Catalogue { controls, ..catalogue },
        );
    }

    // Second pass: `maps_to` often points across frameworks, so it can only
    // be checked once every file that could load has -- a target in a
    // sibling file that itself failed to parse is exactly the "naming
    // nothing known" case, not a false positive to special-case around.
    let known: BTreeSet<ControlRef> = by_framework
        .values()
        .flat_map(|cat| {
            cat.controls
                .iter()
                .map(|ctl| ControlRef::new(cat.framework.clone(), ctl.id.clone()))
        })
        .collect();
    for cat in by_framework.values() {
        let file_name = format!("{}.yaml", cat.framework);
        for ctl in &cat.controls {
            for target in &ctl.maps_to {
                if !known.contains(target) {
                    findings.push(Finding {
                        kind: FindingKind::UnknownMapsTo,
                        subject: file_name.clone(),
                        detail: format!(
                            "{}/{} maps_to {target}, which is not a control in any loaded catalogue",
                            cat.framework, ctl.id
                        ),
                    });
                }
            }
        }
    }

    findings.sort_by(|a, b| {
        a.subject
            .cmp(&b.subject)
            .then(a.kind.cmp(&b.kind))
            .then(a.detail.cmp(&b.detail))
    });
    (by_framework.into_values().collect(), findings)
}

// ============================================================ applicability

/// A control's `max_age` narrowed at one scope. Never widens it -- see
/// [`applicable`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tighten {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age: Option<Duration>,
}

/// A control marked as not applying, from the scope that declared it down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotApplicable {
    pub control: ControlRef,
    pub rationale: String,
}

/// One scope's own policy declarations. `applicable` is handed a chain of
/// these, root first -- the same shape `Engine::roles_for` walks for roles,
/// just carrying additive-only data instead of nearest-wins overrides.
/// Building this chain from the live config is `#76`'s job.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyLayer {
    pub scope: String,
    #[serde(default)]
    pub frameworks: Vec<String>,
    #[serde(default)]
    pub tighten: BTreeMap<ControlRef, Tighten>,
    #[serde(default)]
    pub not_applicable: Vec<NotApplicable>,
}

/// Which scope declared a control not applicable, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedNotApplicable {
    pub scope: String,
    pub rationale: String,
}

/// One control as it applies at the scope `applicable` was asked about:
/// its evidence checks exactly as the catalogue wrote them, and the
/// freshness window and `n/a` status after folding in every layer of the
/// chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Applied {
    pub control: ControlRef,
    pub title: String,
    pub kind: Kind,
    pub maps_to: Vec<ControlRef>,
    /// Exactly as the catalogue wrote it -- a `Task`/`Workflow`/`Gate`
    /// check's own `max_age` here has *not* been tightened by any layer.
    /// Whatever reads freshness for a check (`#81`) must read this
    /// control's own `max_age` field below, not a check's, or a scope's
    /// `tighten` is silently ignored.
    pub evidence: Vec<Check>,
    /// The minimum of the control's own `max_age`, every one of its checks'
    /// own `max_age`, and every layer's `tighten` for it -- `None` when
    /// nothing in any of those ever set one. This, not a `Check` variant's
    /// own `max_age` field, is the effective freshness window to check
    /// evidence against.
    pub max_age: Option<Duration>,
    pub not_applicable: Option<AppliedNotApplicable>,
}

fn min_duration(a: Option<Duration>, b: Option<Duration>) -> Option<Duration> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

/// Every control that applies at the scope `chain`'s last layer names,
/// folding in every layer's tightening and `n/a` declarations -- add or
/// tighten only, by construction:
///
/// - Frameworks are the union across the whole chain; nothing in a layer can
///   remove one.
/// - A control's effective `max_age` is the minimum of the catalogue's own
///   value, every one of its checks' own value, and every layer's
///   `tighten` for it, walked root to leaf. A `tighten` that does not lower
///   that running minimum has no effect and is a [`Finding`].
/// - `n/a` requires a non-empty `rationale`; an empty one is a finding and
///   is ignored (the control stays applicable). The first declaration found
///   walking root to leaf is the one that holds.
/// - A framework, or a `tighten`/`not_applicable` control, that names
///   nothing in `catalogues` is a finding; the rest of that layer still
///   applies.
pub fn applicable(catalogues: &[Catalogue], chain: &[PolicyLayer]) -> (Vec<Applied>, Vec<Finding>) {
    let mut findings = Vec::new();
    let by_framework: BTreeMap<&str, &Catalogue> =
        catalogues.iter().map(|c| (c.framework.as_str(), c)).collect();

    let mut framework_names: BTreeSet<String> = BTreeSet::new();
    for layer in chain {
        for fw in &layer.frameworks {
            if by_framework.contains_key(fw.as_str()) {
                framework_names.insert(fw.clone());
            } else {
                findings.push(Finding {
                    kind: FindingKind::UnknownFramework,
                    subject: layer.scope.clone(),
                    detail: format!("names framework {fw:?}, which has no loaded catalogue"),
                });
            }
        }
    }

    let known_controls: BTreeSet<ControlRef> = framework_names
        .iter()
        .filter_map(|fw| by_framework.get(fw.as_str()))
        .flat_map(|cat| {
            cat.controls
                .iter()
                .map(|ctl| ControlRef::new(cat.framework.clone(), ctl.id.clone()))
        })
        .collect();

    for layer in chain {
        for control in layer.tighten.keys() {
            if !known_controls.contains(control) {
                findings.push(Finding {
                    kind: FindingKind::UnknownControl,
                    subject: layer.scope.clone(),
                    detail: format!("tightens {control}, which is not an applicable control"),
                });
            }
        }
        for na in &layer.not_applicable {
            if !known_controls.contains(&na.control) {
                findings.push(Finding {
                    kind: FindingKind::UnknownControl,
                    subject: layer.scope.clone(),
                    detail: format!(
                        "marks {} not applicable, which is not an applicable control",
                        na.control
                    ),
                });
            }
        }
    }

    let mut applied = Vec::new();
    for fw in &framework_names {
        let cat = by_framework[fw.as_str()];
        for control in &cat.controls {
            let control_ref = ControlRef::new(cat.framework.clone(), control.id.clone());

            let mut not_applicable = None;
            for layer in chain {
                let Some(na) = layer.not_applicable.iter().find(|na| na.control == control_ref) else {
                    continue;
                };
                if na.rationale.trim().is_empty() {
                    findings.push(Finding {
                        kind: FindingKind::EmptyRationale,
                        subject: layer.scope.clone(),
                        detail: format!("{control_ref} marked not applicable with no rationale"),
                    });
                    continue;
                }
                not_applicable = Some(AppliedNotApplicable {
                    scope: layer.scope.clone(),
                    rationale: na.rationale.clone(),
                });
                break;
            }

            let base = control
                .evidence
                .iter()
                .fold(control.max_age, |acc, check| min_duration(acc, check.own_max_age()));
            let mut max_age = base;
            for layer in chain {
                let Some(tighten) = layer.tighten.get(&control_ref) else {
                    continue;
                };
                let Some(new_age) = tighten.max_age else {
                    continue;
                };
                match max_age {
                    Some(current) if new_age >= current => {
                        findings.push(Finding {
                            kind: FindingKind::LooseningHasNoEffect,
                            subject: layer.scope.clone(),
                            detail: format!(
                                "tightens {control_ref} to {new_age}, which is not shorter than the existing {current}"
                            ),
                        });
                    }
                    _ => max_age = Some(new_age),
                }
            }

            applied.push(Applied {
                control: control_ref,
                title: control.title.clone(),
                kind: control.kind.unwrap_or(cat.kind),
                maps_to: control.maps_to.clone(),
                evidence: control.evidence.clone(),
                max_age,
                not_applicable,
            });
        }
    }

    applied.sort_by(|a, b| a.control.cmp(&b.control));
    findings.sort_by(|a, b| {
        a.subject
            .cmp(&b.subject)
            .then(a.kind.cmp(&b.kind))
            .then(a.detail.cmp(&b.detail))
    });
    (applied, findings)
}

// ================================================================ evidence

/// One person's word that a control is met, with an expiry -- the only
/// check kind a person satisfies by saying so, and the only new state this
/// slice introduces (everything else is a lookup against what already
/// exists). Recorded through the API into the instance database,
/// append-only, once `policy.attest` exists (`#76` or later); this module
/// only reads them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attestation {
    pub id: String,
    pub control: ControlRef,
    pub scope: String,
    /// A pointer to the evidence -- a document, a ticket, a page -- not the
    /// evidence itself.
    pub evidence: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub attested_by: String,
    pub attested_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withdrawn: Option<Withdrawal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Withdrawal {
    pub at: DateTime<Utc>,
    pub by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Every piece of evidence `evaluate` has to check controls against. `#77`,
/// `#81`, and `#82` each add a field here as they teach `evaluate` another
/// check kind (a task/workflow run, a gate verdict, a role snapshot, a
/// sandbox/secrets/daemon fact) -- every field is `#[serde(default)]`, so an
/// `Evidence` built before a field existed is still a valid, if incomplete,
/// one, and no caller has to be updated the moment a new field is added.
///
/// Not `deny_unknown_fields`, for the same reason: a wire payload from a
/// newer build of Factory naming a field this build does not know about yet
/// should still parse.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    /// Every tag present anywhere in the knowledge vault.
    #[serde(default)]
    pub tags: BTreeSet<String>,
    /// Attestations that could apply to the scope being evaluated. The
    /// caller filters this to attestations recorded at the evaluated scope
    /// or an ancestor of it before calling `evaluate` -- this module has no
    /// scope tree of its own to check that against.
    #[serde(default)]
    pub attestations: Vec<Attestation>,
}

// =============================================================== evaluate

/// A control's status carries its own reasons, one per check that
/// contributed to it (including a check this module cannot yet evaluate,
/// whose reason says so) -- so a caller never has to go re-derive why a
/// control is `open` from the checks alone.
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
    /// Higher outranks lower when a control's own checks, or a control it
    /// is linked to via `maps_to`, disagree about its status.
    fn rank(self) -> u8 {
        match self {
            StatusKind::NotApplicable => 0,
            StatusKind::Open => 1,
            StatusKind::Stale => 2,
            StatusKind::Attested => 3,
            StatusKind::Satisfied => 4,
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
    fn from_kind(kind: StatusKind, reasons: Vec<String>) -> Status {
        match kind {
            StatusKind::Satisfied => Status::Satisfied { reasons },
            StatusKind::Attested => Status::Attested { reasons },
            StatusKind::Stale => Status::Stale { reasons },
            StatusKind::Open => Status::Open { reasons },
            StatusKind::NotApplicable => Status::NotApplicable { reasons },
        }
    }

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlStatus {
    pub control: ControlRef,
    pub title: String,
    pub kind: Kind,
    #[serde(flatten)]
    pub status: Status,
}

/// This control's status from its own checks alone -- `knowledge` and
/// `attestation` evaluated for real; every other kind `evaluate` cannot yet
/// evaluate contributes an `unevaluated` reason and nothing else.
fn direct_status(applied: &Applied, evidence: &Evidence, now: DateTime<Utc>) -> Status {
    let mut satisfied = Vec::new();
    let mut attested = Vec::new();
    let mut stale = Vec::new();
    let mut open = Vec::new();

    for check in &applied.evidence {
        match check {
            Check::Knowledge { tag } => {
                let tag = tag.clone().unwrap_or_else(|| applied.control.default_tag());
                if evidence.tags.contains(&tag) {
                    satisfied.push(format!("knowledge: tag `{tag}` is present"));
                } else {
                    open.push(format!("knowledge: tag `{tag}` not found"));
                }
            }
            Check::Attestation => {
                let mut expired: Option<&Attestation> = None;
                let mut any_valid = false;
                for att in &evidence.attestations {
                    if att.control != applied.control || att.withdrawn.is_some() {
                        continue;
                    }
                    if att.expires_at > now {
                        attested.push(format!(
                            "attestation: `{}` by {}, valid until {}",
                            att.id, att.attested_by, att.expires_at
                        ));
                        any_valid = true;
                    } else if expired.is_none() {
                        expired = Some(att);
                    }
                }
                if !any_valid {
                    if let Some(att) = expired {
                        stale.push(format!(
                            "attestation: `{}` expired at {}",
                            att.id, att.expires_at
                        ));
                    } else {
                        open.push("attestation: none recorded".to_string());
                    }
                }
            }
            other => {
                open.push(format!("{}: unevaluated", other.kind_name()));
            }
        }
    }

    if !satisfied.is_empty() {
        Status::Satisfied { reasons: satisfied }
    } else if !attested.is_empty() {
        Status::Attested { reasons: attested }
    } else if !stale.is_empty() {
        Status::Stale { reasons: stale }
    } else if open.is_empty() {
        Status::Open {
            reasons: vec!["no evidence".to_string()],
        }
    } else {
        Status::Open { reasons: open }
    }
}

/// Every applicable control's status: its own checks, plus one hop of
/// `maps_to` in both directions among controls that are themselves
/// applicable here. Only `satisfied` or `attested` evidence propagates
/// across a link -- stale or open evidence for a mapped control says
/// nothing about this one. Pure: `now` is a parameter, never read off the
/// clock.
pub fn evaluate(applied: &[Applied], evidence: &Evidence, now: DateTime<Utc>) -> Vec<ControlStatus> {
    let direct: BTreeMap<&ControlRef, Status> = applied
        .iter()
        .map(|a| (&a.control, direct_status(a, evidence, now)))
        .collect();

    let present: BTreeSet<&ControlRef> = applied.iter().map(|a| &a.control).collect();
    let mut neighbors: BTreeMap<&ControlRef, BTreeSet<&ControlRef>> = BTreeMap::new();
    for a in applied {
        for target in &a.maps_to {
            if present.contains(target) {
                neighbors.entry(&a.control).or_default().insert(target);
                neighbors.entry(target).or_default().insert(&a.control);
            }
        }
    }

    let mut out = Vec::with_capacity(applied.len());
    for a in applied {
        if let Some(na) = &a.not_applicable {
            out.push(ControlStatus {
                control: a.control.clone(),
                title: a.title.clone(),
                kind: a.kind,
                status: Status::NotApplicable {
                    reasons: vec![format!(
                        "marked not applicable at {}: {}",
                        na.scope, na.rationale
                    )],
                },
            });
            continue;
        }

        let own = &direct[&a.control];
        let mut reasons = own.reasons().to_vec();
        let mut best = own.kind();

        if let Some(ns) = neighbors.get(&a.control) {
            for neighbor in ns {
                let neighbor_status = &direct[neighbor];
                match neighbor_status.kind() {
                    StatusKind::Satisfied => {
                        reasons.push(format!("satisfied via {neighbor} (maps_to)"));
                        if StatusKind::Satisfied.rank() > best.rank() {
                            best = StatusKind::Satisfied;
                        }
                    }
                    StatusKind::Attested => {
                        reasons.push(format!("attested via {neighbor} (maps_to)"));
                        if StatusKind::Attested.rank() > best.rank() {
                            best = StatusKind::Attested;
                        }
                    }
                    _ => {}
                }
            }
        }

        out.push(ControlStatus {
            control: a.control.clone(),
            title: a.title.clone(),
            kind: a.kind,
            status: Status::from_kind(best, reasons),
        });
    }

    out.sort_by(|a, b| a.control.cmp(&b.control));
    out
}

// ================================================================= rollup

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusCounts {
    #[serde(default)]
    pub satisfied: usize,
    #[serde(default)]
    pub attested: usize,
    #[serde(default)]
    pub stale: usize,
    #[serde(default)]
    pub open: usize,
    #[serde(default)]
    pub not_applicable: usize,
}

impl StatusCounts {
    fn add(&mut self, kind: StatusKind) {
        match kind {
            StatusKind::Satisfied => self.satisfied += 1,
            StatusKind::Attested => self.attested += 1,
            StatusKind::Stale => self.stale += 1,
            StatusKind::Open => self.open += 1,
            StatusKind::NotApplicable => self.not_applicable += 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameworkRollup {
    pub framework: String,
    /// `regulation` and `standard` controls -- these decide `compliant`.
    pub counts: StatusCounts,
    /// `best_practice` controls, shown but never counted towards `compliant`.
    pub best_practice: StatusCounts,
    /// Every counted control is `satisfied`, `attested`, or `not_applicable`.
    /// Means "evidence complete", never "certified".
    pub compliant: bool,
}

/// One rollup per framework named in `statuses`, sorted by framework name.
pub fn rollup(statuses: &[ControlStatus]) -> Vec<FrameworkRollup> {
    let mut by_framework: BTreeMap<String, FrameworkRollup> = BTreeMap::new();
    for status in statuses {
        let entry = by_framework
            .entry(status.control.framework.clone())
            .or_insert_with(|| FrameworkRollup {
                framework: status.control.framework.clone(),
                counts: StatusCounts::default(),
                best_practice: StatusCounts::default(),
                compliant: true,
            });
        let bucket = if status.kind == Kind::BestPractice {
            &mut entry.best_practice
        } else {
            &mut entry.counts
        };
        bucket.add(status.status.kind());
    }
    for rollup in by_framework.values_mut() {
        rollup.compliant = rollup.counts.open == 0 && rollup.counts.stale == 0;
    }
    by_framework.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), text).unwrap();
    }

    fn tempdir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("factory-policy-test-{label}-{}", uuid::Uuid::new_v4()))
    }

    // -- ControlRef ------------------------------------------------------

    #[test]
    fn a_control_ref_round_trips_through_its_string_form() {
        let r = ControlRef::new("cra", "annex-i-2-1");
        assert_eq!(r.to_string(), "cra/annex-i-2-1");
        assert_eq!("cra/annex-i-2-1".parse::<ControlRef>().unwrap(), r);
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, "\"cra/annex-i-2-1\"");
        assert_eq!(serde_json::from_str::<ControlRef>(&json).unwrap(), r);
    }

    #[test]
    fn a_control_ref_refuses_the_wrong_shape() {
        assert!("no-slash".parse::<ControlRef>().is_err());
        assert!("Upper/case".parse::<ControlRef>().is_err());
        // A second `/` lands in `id`, which then fails the slug check.
        assert!("a/b/c".parse::<ControlRef>().is_err());
        assert!("/id".parse::<ControlRef>().is_err());
        assert!("framework/".parse::<ControlRef>().is_err());
    }

    // -- Duration ----------------------------------------------------------

    #[test]
    fn a_duration_parses_days_hours_and_weeks() {
        assert_eq!("30d".parse::<Duration>().unwrap().as_hours(), 30 * 24);
        assert_eq!("12h".parse::<Duration>().unwrap().as_hours(), 12);
        assert_eq!("2w".parse::<Duration>().unwrap().as_hours(), 2 * 24 * 7);
    }

    #[test]
    fn a_duration_refuses_an_unknown_unit_or_a_bare_number() {
        assert!("30m".parse::<Duration>().is_err());
        assert!("30".parse::<Duration>().is_err());
        assert!("abc".parse::<Duration>().is_err());
    }

    // -- load_all ----------------------------------------------------------

    #[test]
    fn a_missing_directory_loads_as_empty_with_no_findings() {
        let dir = tempdir("missing");
        let (catalogues, findings) = load_all(&dir);
        assert!(catalogues.is_empty());
        assert!(findings.is_empty());
    }

    #[test]
    fn a_valid_catalogue_loads_with_no_findings() {
        let dir = tempdir("valid");
        write(
            &dir,
            "cra.yaml",
            "framework: cra\n\
             title: Cyber Resilience Act\n\
             kind: regulation\n\
             controls:\n\
             \x20\x20- id: annex-i-2-1\n\
             \x20\x20\x20\x20title: Identify and document components (SBOM)\n\
             \x20\x20\x20\x20evidence:\n\
             \x20\x20\x20\x20\x20\x20- check: knowledge\n",
        );
        let (catalogues, findings) = load_all(&dir);
        assert_eq!(findings, Vec::new(), "{findings:?}");
        assert_eq!(catalogues.len(), 1);
        assert_eq!(catalogues[0].framework, "cra");
        assert_eq!(catalogues[0].controls.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn kind_best_practice_parses_from_its_hyphenated_spelling_and_its_alias() {
        let hyphenated: Catalogue = serde_yaml_ng::from_str(
            "framework: house\ntitle: House rules\nkind: best-practice\ncontrols: []\n",
        )
        .unwrap();
        assert_eq!(hyphenated.kind, Kind::BestPractice);

        let aliased: Catalogue = serde_yaml_ng::from_str(
            "framework: house\ntitle: House rules\nkind: best_practice\ncontrols: []\n",
        )
        .unwrap();
        assert_eq!(aliased.kind, Kind::BestPractice);

        assert_eq!(serde_json::to_string(&Kind::BestPractice).unwrap(), "\"best-practice\"");
    }

    #[test]
    fn examples_policies_cra_loads_with_no_findings() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/policies");
        let (catalogues, findings) = load_all(&dir);
        assert_eq!(findings, Vec::new(), "{findings:?}");
        assert_eq!(catalogues.len(), 1);
        assert_eq!(catalogues[0].framework, "cra");
        assert!(catalogues[0].controls.len() >= 3, "{:?}", catalogues[0].controls);
    }

    #[test]
    fn a_file_that_fails_to_parse_is_a_finding_and_does_not_stop_the_others() {
        let dir = tempdir("bad-parse");
        write(&dir, "broken.yaml", "not: [valid, catalogue\n");
        write(
            &dir,
            "cra.yaml",
            "framework: cra\ntitle: CRA\nkind: regulation\ncontrols: []\n",
        );
        let (catalogues, findings) = load_all(&dir);
        assert_eq!(catalogues.len(), 1);
        assert_eq!(catalogues[0].framework, "cra");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, FindingKind::ParseFailed);
        assert_eq!(findings[0].subject, "broken.yaml");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unknown_check_kind_is_a_finding_never_a_panic() {
        let dir = tempdir("unknown-check");
        write(
            &dir,
            "cra.yaml",
            "framework: cra\ntitle: CRA\nkind: regulation\ncontrols:\n\
             \x20\x20- id: a\n\x20\x20\x20\x20title: A\n\x20\x20\x20\x20evidence:\n\
             \x20\x20\x20\x20\x20\x20- check: bogus\n",
        );
        let (catalogues, findings) = load_all(&dir);
        assert!(catalogues.is_empty());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, FindingKind::ParseFailed);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unknown_field_inside_a_known_check_is_a_finding_never_a_panic() {
        let dir = tempdir("unknown-check-field");
        write(
            &dir,
            "cra.yaml",
            "framework: cra\ntitle: CRA\nkind: regulation\ncontrols:\n\
             \x20\x20- id: a\n\x20\x20\x20\x20title: A\n\x20\x20\x20\x20evidence:\n\
             \x20\x20\x20\x20\x20\x20- check: knowledge\n\x20\x20\x20\x20\x20\x20\x20\x20bogus: 1\n",
        );
        let (catalogues, findings) = load_all(&dir);
        assert!(catalogues.is_empty());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, FindingKind::ParseFailed);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_framework_that_differs_from_the_file_stem_is_a_finding() {
        let dir = tempdir("mismatch");
        write(
            &dir,
            "cra.yaml",
            "framework: not-cra\ntitle: CRA\nkind: regulation\ncontrols: []\n",
        );
        let (catalogues, findings) = load_all(&dir);
        assert!(catalogues.is_empty());
        assert_eq!(findings[0].kind, FindingKind::FrameworkMismatch);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_duplicate_control_id_is_a_finding_and_the_first_one_is_kept() {
        let dir = tempdir("dup-control");
        write(
            &dir,
            "cra.yaml",
            "framework: cra\ntitle: CRA\nkind: regulation\ncontrols:\n\
             \x20\x20- id: a\n\x20\x20\x20\x20title: First\n\
             \x20\x20- id: a\n\x20\x20\x20\x20title: Second\n",
        );
        let (catalogues, findings) = load_all(&dir);
        assert_eq!(catalogues[0].controls.len(), 1);
        assert_eq!(catalogues[0].controls[0].title, "First");
        assert_eq!(findings[0].kind, FindingKind::DuplicateControl);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn maps_to_naming_nothing_known_is_a_finding_naming_the_file() {
        let dir = tempdir("unknown-mapsto");
        write(
            &dir,
            "cra.yaml",
            "framework: cra\ntitle: CRA\nkind: regulation\ncontrols:\n\
             \x20\x20- id: a\n\x20\x20\x20\x20title: A\n\
             \x20\x20\x20\x20maps_to: [iso27001/a-8-8]\n",
        );
        let (catalogues, findings) = load_all(&dir);
        assert_eq!(catalogues[0].controls.len(), 1, "the control still loads");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, FindingKind::UnknownMapsTo);
        assert_eq!(findings[0].subject, "cra.yaml");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn maps_to_across_two_loaded_catalogues_names_nothing_unknown() {
        let dir = tempdir("known-mapsto");
        write(
            &dir,
            "cra.yaml",
            "framework: cra\ntitle: CRA\nkind: regulation\ncontrols:\n\
             \x20\x20- id: a\n\x20\x20\x20\x20title: A\n\
             \x20\x20\x20\x20maps_to: [iso27001/a-8-8]\n",
        );
        write(
            &dir,
            "iso27001.yaml",
            "framework: iso27001\ntitle: ISO/IEC 27001\nkind: standard\ncontrols:\n\
             \x20\x20- id: a-8-8\n\x20\x20\x20\x20title: Management of technical vulnerabilities\n",
        );
        let (catalogues, findings) = load_all(&dir);
        assert_eq!(catalogues.len(), 2);
        assert_eq!(findings, Vec::new(), "{findings:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    // -- applicable ----------------------------------------------------------

    fn cra_catalogue(controls: Vec<Control>) -> Catalogue {
        Catalogue {
            framework: "cra".to_string(),
            title: "Cyber Resilience Act".to_string(),
            kind: Kind::Regulation,
            controls,
        }
    }

    fn control(id: &str) -> Control {
        Control {
            id: id.to_string(),
            title: format!("Control {id}"),
            kind: None,
            max_age: None,
            maps_to: Vec::new(),
            remediation: None,
            evidence: vec![Check::Knowledge { tag: None }],
        }
    }

    fn layer(scope: &str, frameworks: &[&str]) -> PolicyLayer {
        PolicyLayer {
            scope: scope.to_string(),
            frameworks: frameworks.iter().map(|f| f.to_string()).collect(),
            tighten: BTreeMap::new(),
            not_applicable: Vec::new(),
        }
    }

    #[test]
    fn frameworks_are_the_union_down_the_chain_and_never_shrink() {
        let catalogues = vec![
            cra_catalogue(vec![control("a")]),
            Catalogue {
                framework: "gdpr".to_string(),
                title: "GDPR".to_string(),
                kind: Kind::Regulation,
                controls: vec![control("b")],
            },
        ];
        let chain = vec![layer("root", &["cra"]), layer("root/demo", &["gdpr"])];
        let (applied, findings) = applicable(&catalogues, &chain);
        assert!(findings.is_empty(), "{findings:?}");
        let frameworks: BTreeSet<&str> = applied.iter().map(|a| a.control.framework.as_str()).collect();
        assert_eq!(frameworks, BTreeSet::from(["cra", "gdpr"]));
    }

    #[test]
    fn a_framework_named_with_no_catalogue_is_a_finding() {
        let (applied, findings) = applicable(&[], &[layer("root", &["cra"])]);
        assert!(applied.is_empty());
        assert_eq!(findings[0].kind, FindingKind::UnknownFramework);
    }

    #[test]
    fn max_age_is_the_minimum_of_the_catalogue_the_checks_and_every_tighten() {
        let mut ctl = control("a");
        ctl.max_age = Some("30d".parse().unwrap());
        ctl.evidence = vec![Check::Task {
            task: "sbom-export".to_string(),
            max_age: Some("20d".parse().unwrap()),
        }];
        let catalogues = vec![cra_catalogue(vec![ctl])];

        let mut root = layer("root", &["cra"]);
        root.tighten.insert(
            ControlRef::new("cra", "a"),
            Tighten {
                max_age: Some("14d".parse().unwrap()),
            },
        );
        let mut leaf = layer("root/demo", &[]);
        leaf.tighten.insert(
            ControlRef::new("cra", "a"),
            Tighten {
                max_age: Some("7d".parse().unwrap()),
            },
        );
        let chain = vec![root, leaf];

        let (applied, findings) = applicable(&catalogues, &chain);
        assert!(findings.is_empty(), "{findings:?}");
        assert_eq!(applied[0].max_age, Some("7d".parse().unwrap()));
    }

    #[test]
    fn a_tighten_that_does_not_lower_the_running_minimum_has_no_effect_and_is_a_finding() {
        let mut ctl = control("a");
        ctl.max_age = Some("7d".parse().unwrap());
        let catalogues = vec![cra_catalogue(vec![ctl])];

        let mut leaf = layer("root", &["cra"]);
        leaf.tighten.insert(
            ControlRef::new("cra", "a"),
            Tighten {
                max_age: Some("30d".parse().unwrap()),
            },
        );
        let (applied, findings) = applicable(&catalogues, &[leaf]);
        assert_eq!(applied[0].max_age, Some("7d".parse().unwrap()), "the looser value has no effect");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, FindingKind::LooseningHasNoEffect);
    }

    #[test]
    fn not_applicable_requires_a_rationale() {
        let catalogues = vec![cra_catalogue(vec![control("a")])];
        let mut leaf = layer("root", &["cra"]);
        leaf.not_applicable.push(NotApplicable {
            control: ControlRef::new("cra", "a"),
            rationale: "   ".to_string(),
        });
        let (applied, findings) = applicable(&catalogues, &[leaf]);
        assert!(applied[0].not_applicable.is_none(), "empty rationale is ignored");
        assert_eq!(findings[0].kind, FindingKind::EmptyRationale);
    }

    #[test]
    fn not_applicable_with_a_rationale_holds_and_names_the_declaring_scope() {
        let catalogues = vec![cra_catalogue(vec![control("a")])];
        let mut root = layer("root", &["cra"]);
        root.not_applicable.push(NotApplicable {
            control: ControlRef::new("cra", "a"),
            rationale: "we ship no hardware".to_string(),
        });
        let leaf = layer("root/demo", &[]);
        let (applied, findings) = applicable(&catalogues, &[root, leaf]);
        assert!(findings.is_empty(), "{findings:?}");
        let na = applied[0].not_applicable.as_ref().unwrap();
        assert_eq!(na.scope, "root");
        assert_eq!(na.rationale, "we ship no hardware");
    }

    #[test]
    fn a_tighten_or_not_applicable_naming_an_unknown_control_is_a_finding() {
        let catalogues = vec![cra_catalogue(vec![control("a")])];
        let mut leaf = layer("root", &["cra"]);
        leaf.tighten.insert(
            ControlRef::new("cra", "no-such-control"),
            Tighten {
                max_age: Some("1d".parse().unwrap()),
            },
        );
        leaf.not_applicable.push(NotApplicable {
            control: ControlRef::new("gdpr", "x"),
            rationale: "n/a".to_string(),
        });
        let (_, findings) = applicable(&catalogues, &[leaf]);
        assert_eq!(findings.len(), 2);
        assert!(findings.iter().all(|f| f.kind == FindingKind::UnknownControl));
    }

    // -- evaluate ----------------------------------------------------------

    fn applied_control(id: &str, evidence: Vec<Check>, maps_to: Vec<ControlRef>) -> Applied {
        Applied {
            control: ControlRef::new("cra", id),
            title: format!("Control {id}"),
            kind: Kind::Regulation,
            maps_to,
            evidence,
            max_age: None,
            not_applicable: None,
        }
    }

    #[test]
    fn a_knowledge_tag_present_satisfies_the_control() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Knowledge { tag: None }],
            Vec::new(),
        )];
        let mut evidence = Evidence::default();
        evidence.tags.insert("control/cra/a".to_string());
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Satisfied);
    }

    #[test]
    fn no_evidence_leaves_a_control_open() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Knowledge { tag: None }],
            Vec::new(),
        )];
        let statuses = evaluate(&applied, &Evidence::default(), Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
    }

    #[test]
    fn an_unevaluated_check_kind_never_satisfies_and_says_so() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Task {
                task: "sbom-export".to_string(),
                max_age: None,
            }],
            Vec::new(),
        )];
        let statuses = evaluate(&applied, &Evidence::default(), Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(statuses[0].status.reasons().iter().any(|r| r.contains("unevaluated")));
    }

    #[test]
    fn an_unexpired_attestation_gives_attested() {
        let now = Utc::now();
        let applied = vec![applied_control(
            "a",
            vec![Check::Attestation],
            Vec::new(),
        )];
        let evidence = Evidence {
            attestations: vec![Attestation {
                id: "att-1".to_string(),
                control: ControlRef::new("cra", "a"),
                scope: "root".to_string(),
                evidence: "https://example.com/policy".to_string(),
                note: None,
                attested_by: "owner".to_string(),
                attested_at: now,
                expires_at: now + chrono::Duration::days(30),
                withdrawn: None,
            }],
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, now);
        assert_eq!(statuses[0].status.kind(), StatusKind::Attested);
    }

    #[test]
    fn an_expired_attestation_gives_stale() {
        let now = Utc::now();
        let applied = vec![applied_control("a", vec![Check::Attestation], Vec::new())];
        let evidence = Evidence {
            attestations: vec![Attestation {
                id: "att-1".to_string(),
                control: ControlRef::new("cra", "a"),
                scope: "root".to_string(),
                evidence: "https://example.com/policy".to_string(),
                note: None,
                attested_by: "owner".to_string(),
                attested_at: now - chrono::Duration::days(400),
                expires_at: now - chrono::Duration::days(1),
                withdrawn: None,
            }],
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, now);
        assert_eq!(statuses[0].status.kind(), StatusKind::Stale);
    }

    #[test]
    fn a_withdrawn_attestation_does_not_count() {
        let now = Utc::now();
        let applied = vec![applied_control("a", vec![Check::Attestation], Vec::new())];
        let evidence = Evidence {
            attestations: vec![Attestation {
                id: "att-1".to_string(),
                control: ControlRef::new("cra", "a"),
                scope: "root".to_string(),
                evidence: "https://example.com/policy".to_string(),
                note: None,
                attested_by: "owner".to_string(),
                attested_at: now,
                expires_at: now + chrono::Duration::days(30),
                withdrawn: Some(Withdrawal {
                    at: now,
                    by: "owner".to_string(),
                    reason: None,
                }),
            }],
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, now);
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
    }

    #[test]
    fn a_satisfied_knowledge_check_outranks_an_expired_attestation() {
        let now = Utc::now();
        let applied = vec![applied_control(
            "a",
            vec![Check::Knowledge { tag: None }, Check::Attestation],
            Vec::new(),
        )];
        let mut evidence = Evidence {
            attestations: vec![Attestation {
                id: "att-1".to_string(),
                control: ControlRef::new("cra", "a"),
                scope: "root".to_string(),
                evidence: "https://example.com/policy".to_string(),
                note: None,
                attested_by: "owner".to_string(),
                attested_at: now - chrono::Duration::days(400),
                expires_at: now - chrono::Duration::days(1),
                withdrawn: None,
            }],
            ..Default::default()
        };
        evidence.tags.insert("control/cra/a".to_string());
        let statuses = evaluate(&applied, &evidence, now);
        assert_eq!(statuses[0].status.kind(), StatusKind::Satisfied);
    }

    #[test]
    fn not_applicable_short_circuits_every_check() {
        let mut applied = applied_control("a", vec![Check::Knowledge { tag: None }], Vec::new());
        applied.not_applicable = Some(AppliedNotApplicable {
            scope: "root".to_string(),
            rationale: "we ship no hardware".to_string(),
        });
        let statuses = evaluate(&[applied], &Evidence::default(), Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::NotApplicable);
        assert!(statuses[0].status.reasons()[0].contains("we ship no hardware"));
    }

    #[test]
    fn maps_to_is_symmetric_one_hop_and_names_the_control_it_went_through() {
        let a = applied_control(
            "a",
            vec![Check::Knowledge { tag: None }],
            vec![ControlRef::new("cra", "b")],
        );
        // `b` declares no maps_to of its own -- the link still works the
        // other way, because `evaluate` treats it as symmetric.
        let b = applied_control("b", vec![Check::Knowledge { tag: None }], Vec::new());
        let mut evidence = Evidence::default();
        evidence.tags.insert("control/cra/a".to_string());

        let statuses = evaluate(&[a, b], &evidence, Utc::now());
        let by_id: BTreeMap<&str, &ControlStatus> =
            statuses.iter().map(|s| (s.control.id.as_str(), s)).collect();
        assert_eq!(by_id["a"].status.kind(), StatusKind::Satisfied);
        assert_eq!(by_id["b"].status.kind(), StatusKind::Satisfied);
        assert!(by_id["b"].status.reasons().iter().any(|r| r.contains("cra/a")));
    }

    #[test]
    fn maps_to_propagates_an_attestation_and_names_it_attested_via() {
        let now = Utc::now();
        let a = applied_control(
            "a",
            vec![Check::Attestation],
            vec![ControlRef::new("cra", "b")],
        );
        let b = applied_control("b", vec![Check::Knowledge { tag: None }], Vec::new());
        let evidence = Evidence {
            attestations: vec![Attestation {
                id: "att-1".to_string(),
                control: ControlRef::new("cra", "a"),
                scope: "root".to_string(),
                evidence: "https://example.com/policy".to_string(),
                note: None,
                attested_by: "owner".to_string(),
                attested_at: now,
                expires_at: now + chrono::Duration::days(30),
                withdrawn: None,
            }],
            ..Default::default()
        };
        let statuses = evaluate(&[a, b], &evidence, now);
        let by_id: BTreeMap<&str, &ControlStatus> =
            statuses.iter().map(|s| (s.control.id.as_str(), s)).collect();
        assert_eq!(by_id["b"].status.kind(), StatusKind::Attested);
        assert!(by_id["b"].status.reasons().iter().any(|r| r.contains("attested via cra/a")));
    }

    /// The task notes' one hard wire requirement: `ControlStatus` carries
    /// `#[serde(flatten)]` over an internally-tagged `Status`, which is
    /// exactly the serde combination that can silently misbehave (nesting
    /// under a `status` key instead of flattening it). This proves the
    /// literal JSON shape, not just that it round-trips.
    #[test]
    fn control_status_serializes_flat_with_status_and_reasons_alongside_control() {
        let cs = ControlStatus {
            control: ControlRef::new("cra", "a"),
            title: "A".to_string(),
            kind: Kind::Regulation,
            status: Status::Satisfied {
                reasons: vec!["knowledge: tag `control/cra/a` is present".to_string()],
            },
        };
        let json = serde_json::to_value(&cs).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "control": "cra/a",
                "title": "A",
                "kind": "regulation",
                "status": "satisfied",
                "reasons": ["knowledge: tag `control/cra/a` is present"],
            })
        );
        let back: ControlStatus = serde_json::from_value(json).unwrap();
        assert_eq!(back, cs);
    }

    #[test]
    fn maps_to_does_not_propagate_stale_or_open_evidence() {
        let a = applied_control(
            "a",
            vec![Check::Knowledge { tag: None }],
            vec![ControlRef::new("cra", "b")],
        );
        let b = applied_control("b", vec![Check::Knowledge { tag: None }], Vec::new());
        // Neither control has any matching evidence.
        let statuses = evaluate(&[a, b], &Evidence::default(), Utc::now());
        assert!(statuses.iter().all(|s| s.status.kind() == StatusKind::Open));
    }

    // -- rollup ----------------------------------------------------------

    fn status(framework: &str, id: &str, kind: Kind, status: Status) -> ControlStatus {
        ControlStatus {
            control: ControlRef::new(framework, id),
            title: id.to_string(),
            kind,
            status,
        }
    }

    #[test]
    fn compliant_when_every_counted_control_is_satisfied_attested_or_not_applicable() {
        let statuses = vec![
            status("cra", "a", Kind::Regulation, Status::Satisfied { reasons: vec![] }),
            status("cra", "b", Kind::Standard, Status::Attested { reasons: vec![] }),
            status(
                "cra",
                "c",
                Kind::Regulation,
                Status::NotApplicable { reasons: vec![] },
            ),
        ];
        let rollups = rollup(&statuses);
        assert_eq!(rollups.len(), 1);
        assert!(rollups[0].compliant);
        assert_eq!(rollups[0].counts.satisfied, 1);
        assert_eq!(rollups[0].counts.attested, 1);
        assert_eq!(rollups[0].counts.not_applicable, 1);
    }

    #[test]
    fn an_open_or_stale_counted_control_is_not_compliant() {
        let statuses = vec![status("cra", "a", Kind::Regulation, Status::Open { reasons: vec![] })];
        let rollups = rollup(&statuses);
        assert!(!rollups[0].compliant);
    }

    #[test]
    fn best_practice_controls_are_counted_separately_and_never_affect_compliant() {
        let statuses = vec![status(
            "cra",
            "a",
            Kind::BestPractice,
            Status::Open { reasons: vec![] },
        )];
        let rollups = rollup(&statuses);
        assert!(rollups[0].compliant, "no regulation/standard controls to fail");
        assert_eq!(rollups[0].counts.open, 0);
        assert_eq!(rollups[0].best_practice.open, 1);
    }

    #[test]
    fn rollup_groups_by_framework_and_sorts_the_result() {
        let statuses = vec![
            status("gdpr", "a", Kind::Regulation, Status::Open { reasons: vec![] }),
            status(
                "cra",
                "a",
                Kind::Regulation,
                Status::Satisfied { reasons: vec![] },
            ),
        ];
        let rollups = rollup(&statuses);
        let names: Vec<&str> = rollups.iter().map(|r| r.framework.as_str()).collect();
        assert_eq!(names, vec!["cra", "gdpr"]);
    }
}
