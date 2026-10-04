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
//! parsing that chain out of the live config is `Config::policy_chain_for_scope`,
//! `Factory::policy_chain` and `Engine::policy_chain`'s job (`#76`), not this
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
//! than this module reading the clock. `evaluate` now understands all
//! eleven of `Check`'s kinds: `knowledge` and `attestation` (v1), `task`,
//! `workflow` and `gate` (`#81`), and `roles`, `sandbox`, `secrets` and
//! `daemon` (`#82`), plus `dependencies` (`#123`) and `attested` (`#158`).
//! The four config checks read facts the engine resolves once,
//! synchronously, from the live config snapshot rather than a store --
//! `Evidence::agents` (`Engine::agent_facts_for`, `Scope::agents_with` and
//! `Engine::roles_for`), `Evidence::secrets` (`Engine::credential_inventory`,
//! the same inventory the L2 Secrets tab reads) and `Evidence::daemon`
//! (`Engine::daemon_facts`, `DaemonConfig` and the mounted `http` interface's
//! `bind`) -- but the shape is the same as `task`/`workflow`/`gate`: this
//! module only ever reads a fact somebody else resolved, never a store or
//! the config tree itself. A `roles`/`daemon`/`secrets` field left `None` (or,
//! for `secrets`, a location missing from the map) means "never gathered",
//! not "gathered and empty" -- `direct_status` reports that `open` by name
//! rather than guessing it away, the same restraint an ambiguous
//! `task`/`workflow` name gets.
//!
//! `task`, `workflow` and `gate` never touch a store themselves -- this
//! module stays pure. The engine (`Engine::policy_report`/`policy_control`
//! in `factory-daemon`) resolves whatever a check names -- a task, a
//! workflow, a dataset's bench runs -- into a [`TaskFact`], [`WorkflowFact`]
//! or [`GateFact`] first, and hands the result in on [`Evidence`]. Resolution
//! happens once per evaluated scope, and only for the names an applicable
//! check actually references -- never every task or workflow a scope has.
//! `task` and `workflow` carry a bounded window of recent runs, not just the
//! newest one, so `evaluate` can skip past a run still in progress to the
//! newest *finished* one -- a scheduled evidence task must not flip its own
//! control `open` for as long as it happens to be running.
//!
//! `Evidence` grows a field per check kind as each ticket teaches `evaluate`
//! to read it; every field it has is `#[serde(default)]` so an older caller
//! building one is still a valid, if incomplete, bundle.
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
//!
//! A second spelling amends the ADR: its evidence table says `secrets`
//! holds when "secrets reach agents only through the Secrets seam", but
//! there is no such seam -- the README's own "Secrets" section is explicit
//! that Factory injects no credentials and gates nothing, because every
//! agent runs as the daemon's owner and reads whatever that user can read.
//! The L2 Secrets tab records one fact only, presence: whether a file sits
//! at each of a handful of well-known locations, never a value. That is
//! what `secrets` actually checks -- a named location's *absence*, not a
//! seam that does not exist -- see [`Check::Secrets`] and
//! [`KNOWN_SECRETS_LOCATIONS`].

#[cfg(test)]
use crate::bench::Verdict;
#[cfg(test)]
use crate::conformance::AttestedRun;
#[cfg(test)]
use crate::dependencies::{DependenciesFact, Severity};
#[cfg(test)]
use crate::role::Grant;
#[cfg(test)]
use crate::run::RunStatus;
#[cfg(test)]
use crate::workflow::WorkflowRunStatus;
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
/// (days, hours, weeks). Moved to the L0 kernel (#193, phase 1, F7): it is
/// imported below L6 (`dependencies.rs`, `quality.rs`, `config.rs`, and the
/// daemon's own `dependencies.rs` tests), so it belongs where nothing above
/// it can accidentally deepen the dependency the wrong way. Re-exported
/// here unchanged, so `policy.rs` and everything L6 and up keeps naming it
/// `policy::Duration` and the serialized form -- `"30d"`, `"12h"`, `"2w"` --
/// stays exactly what it was.
pub use factory_kernel::Duration;

pub use factory_kernel::{ControlRef, Attestation, Withdrawal};
pub use factory_assurance::checks::{Check, Evidence, StatusKind, Status, EvidenceRefKind, EvidenceRef};
pub type ControlStatus = factory_assurance::checks::EvaluationResult<Kind>;
pub use factory_kernel::{KNOWN_DAEMON_FACTS, KNOWN_SECRETS_LOCATIONS};

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
    /// The steps work of a given category must pass through for this
    /// control to hold (`#118`): folded into a scope's control plan by
    /// `control_plan::resolve`, injected at dispatch, and proved by an
    /// attestation before a run counts as `done`. Evidence says the plant
    /// is compliant; `requires` says what every run on it must carry.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<crate::control_plan::Requirement>,
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
    /// A `task` or `workflow` check named something that matched more than
    /// one task's title, or more than one workflow's name, in the evaluated
    /// scope -- see [`evidence_findings`]. `evaluate` also reports the same
    /// control `open` over it, but the mistake is in the catalogue (or the
    /// scope's tasks), not in the evidence, so it is a finding too.
    AmbiguousCheckTarget,
    /// A `daemon` check named a `fact` outside [`KNOWN_DAEMON_FACTS`].
    UnknownDaemonFact,
    /// A `secrets` check's `absent` named a location outside
    /// [`KNOWN_SECRETS_LOCATIONS`].
    UnknownSecretsLocation,
    /// A `requires:` entry that cannot do what it says -- a gate with no
    /// command, a category that is not a name. Kept, never dropped: see
    /// `control_plan::Requirement::problems`.
    BadRequirement,
    /// An `attested` check's `category` or `step` is not a name
    /// (`control_plan::is_name`) -- it could never match a run's own
    /// `category`/`RequiredStep::step`, so the check can never be
    /// satisfied. `#158`.
    BadCheckTarget,
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

/// Whatever in `check` names something outside a fixed vocabulary -- a
/// `daemon` fact not in [`KNOWN_DAEMON_FACTS`], a `secrets` location not in
/// [`KNOWN_SECRETS_LOCATIONS`] -- as a finding kind and a detail that reads
/// on after the name of whatever holds the check ("<control> names daemon
/// fact ..."). Neither vocabulary depends on live evidence, so a caller can
/// catch the mistake when the file loads rather than only once a report is
/// evaluated: [`load_all`] for a catalogue, `quality::load` for a quality
/// profile's check measures.
pub fn check_vocabulary(check: &Check) -> Vec<(FindingKind, String)> {
    use factory_assurance::checks::VocabularyFinding;
    factory_assurance::checks::check_vocabulary(check).into_iter().map(|(kind, detail)| {
        let kind = match kind {
            VocabularyFinding::UnknownDaemonFact => FindingKind::UnknownDaemonFact,
            VocabularyFinding::UnknownSecretsLocation => FindingKind::UnknownSecretsLocation,
            VocabularyFinding::BadCheckTarget => FindingKind::BadCheckTarget,
        };
        (kind, detail)
    }).collect()
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
            // `daemon`'s `fact` and `secrets`' `absent` are each a fixed,
            // known vocabulary (`KNOWN_DAEMON_FACTS`/`KNOWN_SECRETS_LOCATIONS`)
            // that never depends on live evidence, so an unknown name is
            // caught here, at parse time, rather than only once a report is
            // evaluated.
            for check in &control.evidence {
                for (kind, detail) in check_vocabulary(check) {
                    findings.push(Finding {
                        kind,
                        subject: file_name.clone(),
                        detail: format!("{}/{} {detail}", catalogue.framework, control.id),
                    });
                }
            }
            for requirement in &control.requires {
                for detail in requirement.problems() {
                    findings.push(Finding {
                        kind: FindingKind::BadRequirement,
                        subject: file_name.clone(),
                        detail: format!("{}/{} {detail}", catalogue.framework, control.id),
                    });
                }
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
/// Built from the live config by `Config::policy_chain_for_scope`,
/// `Factory::policy_chain` and `Engine::policy_chain` (`#76`); the config-side
/// declaration this is converted from (`PolicyDeclaration`, minus `scope`,
/// which the chain builder fills in) lives in `factory-core::config`.
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
    /// Whatever reads freshness for a check reads this control's own
    /// `max_age` field below, not a check's -- `direct_status` does exactly
    /// this for `task`/`workflow`/`gate`; reading a check's own field
    /// instead would silently ignore a scope's `tighten`.
    pub evidence: Vec<Check>,
    /// The minimum of the control's own `max_age`, every one of its checks'
    /// own `max_age`, and every layer's `tighten` for it -- `None` when
    /// nothing in any of those ever set one. This, not a `Check` variant's
    /// own `max_age` field, is the effective freshness window to check
    /// evidence against.
    pub max_age: Option<Duration>,
    pub not_applicable: Option<AppliedNotApplicable>,
    /// The catalogue's own `remediation:` text for this control, carried
    /// through unchanged -- no layer tightens or overrides it, the same as
    /// `title`. `Engine::policy_remediate` (`#83`) is the one reader; kept
    /// here rather than looked up separately so a caller that already has
    /// an `Applied` (or a `PolicyControlDetail` built from one) never needs
    /// a second pass over the catalogue just for this field.
    pub remediation: Option<String>,
    /// The catalogue's own `requires:` for this control, unchanged -- like
    /// `remediation`, no layer edits it. `control_plan::resolve` reads it,
    /// and a control `n/a` here waives it there, by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<crate::control_plan::Requirement>,
}

fn min_duration(a: Option<Duration>, b: Option<Duration>) -> Option<Duration> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

/// The framework names named anywhere in `chain`, deduplicated and sorted --
/// what `AgentContext::factory_guide` (`factory-core::adapter::agent`) names
/// to an agent as the frameworks its scope is committed to. Deliberately
/// cheaper than [`applicable`]: it reads only the chain a dispatch already
/// resolved (`Engine::policy_chain`), never touches disk to load a
/// catalogue, and never checks a name against one -- a framework with no
/// loaded catalogue is still named here, the same way it is still a
/// [`Finding`] `applicable` reports rather than silently drops. The guide is
/// a plain restatement of what the config commits the scope to, not a report
/// on whether that commitment resolved to something real.
pub fn frameworks_in_chain(chain: &[PolicyLayer]) -> Vec<String> {
    chain
        .iter()
        .flat_map(|layer| layer.frameworks.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
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
                remediation: control.remediation.clone(),
                requires: control.requires.clone(),
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

/// Turn what a person typed for an attestation's expiry into the absolute
/// `expires_at` `Request::PolicyAttest` carries on the wire: `30d`/`12w`
/// (this module's own [`Duration`] grammar, relative to `now`), a bare date
/// (`2027-01-01`, midnight UTC), or a full RFC3339 timestamp. `#78`'s CLI
/// and HTTP interface both accept the same three forms, so this is the one
/// place the grammar is written down rather than two -- pure, like the rest
/// of this module: `now` is a parameter, never read off the clock.
pub fn parse_expiry(s: &str, now: DateTime<Utc>) -> std::result::Result<DateTime<Utc>, String> {
    let bad = || {
        format!(
            "{s:?} is not a duration like 30d or 12w, a date like 2027-01-01, or an RFC3339 timestamp"
        )
    };
    if let Ok(d) = s.parse::<Duration>() {
        return now
            .checked_add_signed(d.as_time_delta())
            .ok_or_else(|| format!("{s:?} is too far in the future to be an expiry"));
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let midnight = date.and_hms_opt(0, 0, 0).ok_or_else(bad)?;
        return Ok(midnight.and_utc());
    }
    Err(bad())
}

pub use factory_kernel::RunFact;

pub use factory_kernel::TaskFact;

pub use factory_kernel::WorkflowRunFact;

pub use factory_kernel::WorkflowFact;

pub use factory_kernel::GateCase;

pub use factory_kernel::GateFact;

pub use factory_kernel::AgentFact;

/// The `daemon` check's own configuration facts, resolved once per report by
/// the engine (`Engine::daemon_facts`) rather than per scope -- the daemon's
/// own configuration is the same wherever it is asked from, the same
/// reasoning `Evidence::gates` already uses for datasets. Moved to the L0
/// kernel as `DaemonConfigFact` (#193, phase 2: no field's type is owned by
/// another level's module) and re-exported here unchanged under its old
/// name, so nothing below still calls it `DaemonFact` has to change --
/// `factory_kernel::facts`'s own doc comment explains the rename (it
/// disambiguates from `protocol::DaemonFacts`, the unrelated L1 wire
/// payload) and carries its `impl Fact` (`Producer = L1`).
pub use factory_kernel::DaemonConfigFact as DaemonFact;

pub fn evaluation_subject(applied: &Applied) -> factory_assurance::checks::EvaluationSubject<Kind> {
    factory_assurance::checks::EvaluationSubject {
        control: applied.control.clone(),
        title: applied.title.clone(),
        kind: applied.kind,
        maps_to: applied.maps_to.clone(),
        evidence: applied.evidence.clone(),
        max_age: applied.max_age,
        not_applicable: applied.not_applicable.as_ref().map(|na| {
            factory_assurance::checks::Exemption {
                scope: na.scope.clone(),
                rationale: na.rationale.clone(),
            }
        }),
    }
}

impl factory_assurance::checks::CheckSource for Applied {
    fn checks(&self) -> &[Check] {
        &self.evidence
    }
    fn max_age(&self) -> Option<Duration> {
        self.max_age
    }
}

pub fn evaluate(
    applied: &[Applied],
    evidence: &Evidence,
    now: DateTime<Utc>,
) -> Vec<ControlStatus> {
    let subjects: Vec<_> = applied.iter().map(evaluation_subject).collect();
    factory_assurance::checks::evaluate(&subjects, evidence, now)
}

pub fn evidence_findings(evidence: &Evidence, scope: &str) -> Vec<Finding> {
    factory_assurance::checks::evidence_findings(evidence, scope)
        .into_iter()
        .map(|f| Finding {
            kind: FindingKind::AmbiguousCheckTarget,
            subject: f.subject,
            detail: f.detail,
        })
        .collect()
}

// ============================================================ remediation

/// The instructions `Engine::policy_remediate` (`#83`) writes into the task
/// it creates to close a gap -- three parts, in the order the issue asks
/// for: the catalogue's own `remediation:` guidance, if it wrote one; the
/// reasons the control's current `status` carries, verbatim -- exactly what
/// a person reads on the L6 tab or `factory policy show`, so the task never
/// says something the control's own evaluation does not; and a closing line
/// naming every check that could satisfy it, so the task also says what
/// would close it, not only what has not. Pure -- like the rest of this
/// module, `Engine::policy_remediate` is the one caller, and it already has
/// every argument from a `PolicyControlDetail`/`Applied` it already fetched.
pub fn remediation_instructions(control: &ControlRef, remediation: Option<&str>, status: &Status, checks: &[Check]) -> String {
    let mut out = String::new();
    if let Some(r) = remediation.map(str::trim).filter(|r| !r.is_empty()) {
        out.push_str(r);
        out.push_str("\n\n");
    }
    out.push_str("Missing evidence:\n");
    for reason in status.reasons() {
        out.push_str(&format!("- {reason}\n"));
    }
    out.push_str(&format!("\nAny one of these checks passing closes {control}:\n"));
    for check in checks {
        out.push_str(&format!("- {}\n", check.describe()));
    }
    out.trim_end().to_string()
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

/// How bad a control's status is for cross-scope aggregation, lowest is
/// worst -- deliberately not `StatusKind::rank`, which orders `NotApplicable`
/// *below* `Open` for a different purpose (whether a `maps_to` neighbour's
/// status should win out over this control's own). Here `NotApplicable`
/// means "this scope has nothing to say", which is not a status to compare
/// against the real ones at all -- see [`worst_across_scopes`], which never
/// calls this on one.
fn compliance_severity(kind: StatusKind) -> u8 {
    match kind {
        StatusKind::Open => 0,
        StatusKind::Stale => 1,
        StatusKind::Attested => 2,
        StatusKind::Satisfied => 3,
        StatusKind::NotApplicable => 4,
    }
}

/// One status per control, folding many scopes' own [`evaluate`] output
/// into the single view a subtree-wide [`rollup`] needs: "a control counts
/// compliant only if it is compliant in every scope it applies to" (ADR
/// 0004). For each control, every scope where it is `not_applicable` is
/// ignored -- that scope has nothing to say about whether it is met -- and
/// the worst of whatever real statuses remain wins (`Open` beats `Stale`
/// beats `Attested` beats `Satisfied`). A control that is `not_applicable`
/// in every scope it appears in keeps that status; a control absent from
/// every scope in `per_scope` never appears in the result at all.
///
/// Pure: no scope tree, no store, no clock -- just what each scope's own
/// `evaluate` already produced. The caller (`Engine::policy_report`) is the
/// one that knows which scopes are in the subtree being asked about.
pub fn worst_across_scopes(per_scope: &[Vec<ControlStatus>]) -> Vec<ControlStatus> {
    let mut worst: BTreeMap<ControlRef, ControlStatus> = BTreeMap::new();
    let mut fallback_na: BTreeMap<ControlRef, ControlStatus> = BTreeMap::new();

    for statuses in per_scope {
        for status in statuses {
            if status.status.kind() == StatusKind::NotApplicable {
                fallback_na.entry(status.control.clone()).or_insert_with(|| status.clone());
                continue;
            }
            match worst.get(&status.control) {
                Some(current) if compliance_severity(current.status.kind()) <= compliance_severity(status.status.kind()) => {
                    // The status already kept is at least as bad; nothing to do.
                }
                _ => {
                    worst.insert(status.control.clone(), status.clone());
                }
            }
        }
    }

    // A control that never had a real status anywhere it appeared is
    // `not_applicable` everywhere -- keep exactly one of those entries.
    for (control, status) in fallback_na {
        worst.entry(control).or_insert(status);
    }

    worst.into_values().collect()
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

    // -- Duration ------------------------------------------------------------
    //
    // `Duration` itself -- parsing, `Display`, and the overflow cap -- moved
    // to `factory-kernel` with the type (#193, phase 1, F7); see
    // `factory_kernel::duration::tests`. What is left here is policy-level:
    // `within_max_age` and `parse_expiry` are this module's own functions,
    // not the kernel's, so their tests stay, using the re-exported type.

    #[test]
    fn an_absurdly_long_max_age_never_looks_stale_and_refuses_as_an_expiry() {
        let huge: Duration = "9999999999999999h".parse().unwrap();
        let now = Utc::now();
        let check = Applied {
            control: ControlRef::new("test", "huge"),
            title: "huge".into(),
            kind: Kind::Standard,
            maps_to: vec![],
            evidence: vec![Check::Task {
                task: "old".into(),
                max_age: Some(huge),
            }],
            max_age: Some(huge),
            not_applicable: None,
            remediation: None,
            requires: vec![],
        };
        let evidence = Evidence {
            tasks: BTreeMap::from([(
                "old".into(),
                vec![TaskFact {
                    id: "t".into(),
                    title: "old".into(),
                    runs: vec![RunFact {
                        id: "r".into(),
                        status: RunStatus::Done,
                        started_at: now - chrono::TimeDelta::days(10_001),
                        ended_at: Some(now - chrono::TimeDelta::days(10_000)),
                    }],
                }],
            )]),
            ..Default::default()
        };
        assert_eq!(
            evaluate(&[check], &evidence, now)[0].status.kind(),
            StatusKind::Satisfied
        );
        let e = parse_expiry("9999999999999999h", now).unwrap_err();
        assert!(e.contains("too far"), "{e}");
    }

    #[test]
    fn check_vocabulary_names_an_unknown_daemon_fact_or_secrets_location() {
        assert!(check_vocabulary(&Check::Sandbox).is_empty());
        assert!(check_vocabulary(&Check::Daemon { fact: "power_assertion".into() }).is_empty());
        let found = check_vocabulary(&Check::Daemon { fact: "power_asertion".into() });
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, FindingKind::UnknownDaemonFact);
        // #154: the three backup facts are known, and a misspelling of one
        // is caught the same way, at load time rather than only once
        // evaluated.
        for fact in ["backup_recent", "backup_offsite", "backup_verified"] {
            assert!(check_vocabulary(&Check::Daemon { fact: fact.into() }).is_empty(), "{fact}");
        }
        let found = check_vocabulary(&Check::Daemon { fact: "backup_verfied".into() });
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, FindingKind::UnknownDaemonFact);
        let found = check_vocabulary(&Check::Secrets { absent: vec!["github".into(), "gitlab".into(), "nope".into()] });
        assert_eq!(
            found.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
            vec![FindingKind::UnknownSecretsLocation, FindingKind::UnknownSecretsLocation]
        );
        assert!(found[0].1.contains("gitlab"), "{}", found[0].1);
    }

    // -- parse_expiry --------------------------------------------------------

    #[test]
    fn parse_expiry_accepts_a_relative_duration() {
        let now = "2026-09-24T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(
            parse_expiry("30d", now).unwrap(),
            now + chrono::Duration::days(30)
        );
        assert_eq!(
            parse_expiry("12w", now).unwrap(),
            now + chrono::Duration::weeks(12)
        );
    }

    #[test]
    fn parse_expiry_accepts_a_bare_date_as_midnight_utc() {
        let now = "2026-09-24T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(
            parse_expiry("2027-01-01", now).unwrap(),
            "2027-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
    }

    #[test]
    fn parse_expiry_accepts_an_rfc3339_timestamp() {
        let now = "2026-09-24T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(
            parse_expiry("2027-01-01T08:30:00Z", now).unwrap(),
            "2027-01-01T08:30:00Z".parse::<DateTime<Utc>>().unwrap()
        );
    }

    #[test]
    fn parse_expiry_refuses_anything_else() {
        let now = Utc::now();
        let err = parse_expiry("soon", now).unwrap_err();
        assert!(err.contains("soon"), "{err}");
        assert!(parse_expiry("", now).is_err());
        assert!(parse_expiry("2027-13-40", now).is_err());
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

    #[test]
    fn a_daemon_check_naming_an_unknown_fact_is_a_finding_at_parse_time() {
        let dir = tempdir("unknown-daemon-fact");
        write(
            &dir,
            "cra.yaml",
            "framework: cra\ntitle: CRA\nkind: regulation\ncontrols:\n\
             \x20\x20- id: a\n\x20\x20\x20\x20title: A\n\x20\x20\x20\x20evidence:\n\
             \x20\x20\x20\x20\x20\x20- check: daemon\n\x20\x20\x20\x20\x20\x20\x20\x20fact: launches_rockets\n",
        );
        let (catalogues, findings) = load_all(&dir);
        assert_eq!(catalogues[0].controls.len(), 1, "the control still loads");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, FindingKind::UnknownDaemonFact);
        assert_eq!(findings[0].subject, "cra.yaml");
        assert!(findings[0].detail.contains("launches_rockets"), "{}", findings[0].detail);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_secrets_check_naming_an_unknown_location_is_a_finding_at_parse_time() {
        let dir = tempdir("unknown-secrets-location");
        write(
            &dir,
            "cra.yaml",
            "framework: cra\ntitle: CRA\nkind: regulation\ncontrols:\n\
             \x20\x20- id: a\n\x20\x20\x20\x20title: A\n\x20\x20\x20\x20evidence:\n\
             \x20\x20\x20\x20\x20\x20- check: secrets\n\x20\x20\x20\x20\x20\x20\x20\x20absent: [anthropic, under-the-mat]\n",
        );
        let (catalogues, findings) = load_all(&dir);
        assert_eq!(catalogues[0].controls.len(), 1, "the control still loads");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].kind, FindingKind::UnknownSecretsLocation);
        assert!(findings[0].detail.contains("under-the-mat"), "{}", findings[0].detail);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_bare_check_secrets_still_parses_and_round_trips_with_no_absent_field() {
        let check: Check = serde_yaml_ng::from_str("check: secrets\n").unwrap();
        assert_eq!(check, Check::Secrets { absent: Vec::new() });
        let json = serde_json::to_string(&check).unwrap();
        assert_eq!(json, "{\"check\":\"secrets\"}", "an empty `absent` is not written out");
    }

    #[test]
    fn dependencies_built_sbom_requirement_parses_and_defaults_off() {
        let required: Check =
            serde_yaml_ng::from_str("check: dependencies\nbuilt_sbom: true\n").unwrap();
        assert_eq!(
            required,
            Check::Dependencies {
                sbom_max_age: None,
                built_sbom: true,
                max_open: BTreeMap::new(),
                exploited_open: None,
            }
        );
        assert_eq!(required.describe(), "dependencies: built SBOM required");

        let optional: Check = serde_yaml_ng::from_str("check: dependencies\n").unwrap();
        assert_eq!(
            optional,
            Check::Dependencies {
                sbom_max_age: None,
                built_sbom: false,
                max_open: BTreeMap::new(),
                exploited_open: None,
            }
        );
        assert_eq!(
            serde_json::to_string(&optional).unwrap(),
            "{\"check\":\"dependencies\"}"
        );
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
            requires: Vec::new(),
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

    // -- frameworks_in_chain --------------------------------------------------

    #[test]
    fn frameworks_in_chain_is_the_deduplicated_sorted_union() {
        let chain = vec![
            layer("root", &["cra", "gdpr"]),
            layer("root/demo", &["gdpr", "iso27001"]),
        ];
        assert_eq!(frameworks_in_chain(&chain), vec!["cra", "gdpr", "iso27001"]);
    }

    #[test]
    fn frameworks_in_chain_is_empty_for_an_empty_chain_or_one_with_nothing_declared() {
        assert_eq!(frameworks_in_chain(&[]), Vec::<String>::new());
        assert_eq!(frameworks_in_chain(&[layer("root", &[])]), Vec::<String>::new());
    }

    #[test]
    fn frameworks_in_chain_names_a_framework_with_no_loaded_catalogue() {
        // Unlike `applicable`, this never checks a name against a loaded
        // catalogue -- it is a plain restatement of the config, findings are
        // `applicable`'s job.
        let chain = vec![layer("root", &["not-a-real-framework"])];
        assert_eq!(frameworks_in_chain(&chain), vec!["not-a-real-framework"]);
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
            remediation: None,
            requires: Vec::new(),
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

    // -- evaluate: roles ----------------------------------------------------

    fn agent(name: &str, role: &str, grants: &[Grant]) -> AgentFact {
        AgentFact {
            name: name.to_string(),
            role: role.to_string(),
            grants: Some(grants.iter().copied().collect()),
            has_sandbox: true,
            sandbox_enforced: false,
        }
    }

    #[test]
    fn roles_with_agents_never_resolved_stays_open() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Roles { forbid: vec![Grant::KnowledgeWrite] }],
            Vec::new(),
        )];
        let statuses = evaluate(&applied, &Evidence::default(), Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(statuses[0].status.reasons().iter().any(|r| r.contains("not resolved")), "{:?}", statuses[0].status);
    }

    #[test]
    fn roles_with_no_agent_declared_is_satisfied() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Roles { forbid: vec![Grant::KnowledgeWrite] }],
            Vec::new(),
        )];
        let evidence = Evidence { agents: Some(Vec::new()), ..Default::default() };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Satisfied);
    }

    #[test]
    fn roles_open_when_an_agents_role_holds_a_forbidden_grant() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Roles { forbid: vec![Grant::KnowledgeWrite] }],
            Vec::new(),
        )];
        let evidence = Evidence {
            agents: Some(vec![
                agent("worker", "worker", &[Grant::TaskEdit]),
                agent("foreman", "foreman", &[Grant::KnowledgeWrite, Grant::TaskEdit]),
            ]),
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(
            statuses[0].status.reasons().iter().any(|r| r.contains("foreman") && r.contains("knowledge.write")),
            "{:?}",
            statuses[0].status
        );
    }

    #[test]
    fn roles_satisfied_when_no_agent_holds_a_forbidden_grant() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Roles { forbid: vec![Grant::PolicyAttest] }],
            Vec::new(),
        )];
        let evidence = Evidence {
            agents: Some(vec![agent("worker", "worker", &[Grant::TaskEdit, Grant::RunInput])]),
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Satisfied);
    }

    #[test]
    fn roles_open_when_an_agents_role_does_not_resolve() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Roles { forbid: vec![Grant::PolicyAttest] }],
            Vec::new(),
        )];
        let evidence = Evidence {
            agents: Some(vec![AgentFact {
                name: "ghost".to_string(),
                role: "vanished".to_string(),
                grants: None,
                has_sandbox: true,
                sandbox_enforced: false,
            }]),
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(
            statuses[0].status.reasons().iter().any(|r| r.contains("ghost") && r.contains("vanished")),
            "{:?}",
            statuses[0].status
        );
    }

    // -- evaluate: sandbox ----------------------------------------------------

    #[test]
    fn sandbox_with_agents_never_resolved_stays_open() {
        let applied = vec![applied_control("a", vec![Check::Sandbox], Vec::new())];
        let statuses = evaluate(&applied, &Evidence::default(), Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(statuses[0].status.reasons().iter().any(|r| r.contains("not resolved")), "{:?}", statuses[0].status);
    }

    #[test]
    fn sandbox_with_no_agent_declared_is_satisfied() {
        let applied = vec![applied_control("a", vec![Check::Sandbox], Vec::new())];
        let evidence = Evidence { agents: Some(Vec::new()), ..Default::default() };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Satisfied);
    }

    #[test]
    fn sandbox_open_when_an_agent_declares_none() {
        let applied = vec![applied_control("a", vec![Check::Sandbox], Vec::new())];
        let evidence = Evidence {
            agents: Some(vec![
                agent("worker", "worker", &[]),
                AgentFact { name: "foreman".to_string(), role: "foreman".to_string(), grants: Some(BTreeSet::new()), has_sandbox: false, sandbox_enforced: false },
            ]),
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(statuses[0].status.reasons().iter().any(|r| r.contains("foreman")), "{:?}", statuses[0].status);
    }

    #[test]
    fn sandbox_satisfied_when_every_agent_declares_one() {
        let applied = vec![applied_control("a", vec![Check::Sandbox], Vec::new())];
        let evidence = Evidence {
            agents: Some(vec![agent("worker", "worker", &[]), agent("foreman", "foreman", &[])]),
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Satisfied);
    }

    /// `#218`: the evidence tells a declared-only sandbox from an enforced
    /// one, so a satisfied control never reads as more than it is.
    #[test]
    fn sandbox_evidence_says_which_declarations_are_enforced() {
        let applied = vec![applied_control("a", vec![Check::Sandbox], Vec::new())];
        let mut boxed = agent("curator", "worker", &[]);
        boxed.sandbox_enforced = true;
        let evidence = Evidence {
            agents: Some(vec![boxed, agent("builder", "worker", &[])]),
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Satisfied);
        let reasons = statuses[0].status.reasons().join(" | ");
        assert!(reasons.contains("2 checked, 1 enforced"), "{reasons}");
        assert!(reasons.contains("declared but not enforced: builder"), "{reasons}");
    }

    // -- evaluate: secrets ----------------------------------------------------

    #[test]
    fn secrets_with_the_default_location_never_resolved_stays_open() {
        let applied = vec![applied_control("a", vec![Check::Secrets { absent: Vec::new() }], Vec::new())];
        let statuses = evaluate(&applied, &Evidence::default(), Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(statuses[0].status.reasons().iter().any(|r| r.contains("not resolved")), "{:?}", statuses[0].status);
    }

    #[test]
    fn secrets_satisfied_when_the_default_location_is_absent() {
        let applied = vec![applied_control("a", vec![Check::Secrets { absent: Vec::new() }], Vec::new())];
        let evidence = Evidence {
            secrets: BTreeMap::from([("scope_env".to_string(), false)]).into(),
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Satisfied);
    }

    #[test]
    fn secrets_open_when_a_named_location_is_present() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Secrets { absent: vec!["anthropic".to_string(), "ssh".to_string()] }],
            Vec::new(),
        )];
        let evidence = Evidence {
            secrets: BTreeMap::from([("anthropic".to_string(), true), ("ssh".to_string(), false)]).into(),
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(statuses[0].status.reasons().iter().any(|r| r.contains("anthropic")), "{:?}", statuses[0].status);
    }

    // -- evaluate: daemon -----------------------------------------------------

    #[test]
    fn daemon_check_with_no_facts_gathered_stays_open() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Daemon { fact: "power_assertion".to_string() }],
            Vec::new(),
        )];
        let statuses = evaluate(&applied, &Evidence::default(), Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(statuses[0].status.reasons().iter().any(|r| r.contains("not resolved")), "{:?}", statuses[0].status);
    }

    #[test]
    fn daemon_check_satisfied_when_the_named_fact_holds() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Daemon { fact: "power_assertion".to_string() }],
            Vec::new(),
        )];
        let evidence = Evidence {
            daemon: Some(DaemonFact { foreman_enabled: false, http_loopback_only: None, power_assertion: true }),
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Satisfied);
    }

    #[test]
    fn daemon_check_open_when_the_named_fact_does_not_hold() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Daemon { fact: "foreman_enabled".to_string() }],
            Vec::new(),
        )];
        let evidence = Evidence {
            daemon: Some(DaemonFact { foreman_enabled: false, http_loopback_only: None, power_assertion: true }),
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
    }

    #[test]
    fn daemon_check_naming_an_unknown_fact_stays_open_with_its_own_reason() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Daemon { fact: "launches_rockets".to_string() }],
            Vec::new(),
        )];
        let evidence = Evidence {
            daemon: Some(DaemonFact::default()),
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(
            statuses[0].status.reasons().iter().any(|r| r.contains("not a fact this build knows")),
            "{:?}",
            statuses[0].status
        );
    }

    #[test]
    fn daemon_check_open_when_http_loopback_only_cannot_be_determined() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Daemon { fact: "http_loopback_only".to_string() }],
            Vec::new(),
        )];
        let evidence = Evidence {
            daemon: Some(DaemonFact { foreman_enabled: false, http_loopback_only: None, power_assertion: true }),
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(
            statuses[0].status.reasons().iter().any(|r| r.contains("could not be determined")),
            "{:?}",
            statuses[0].status
        );
    }

    /// `#154`: `evidence.backup` missing (never gathered) is "not resolved",
    /// distinct from gathered-but-indeterminate ("could not be determined")
    /// -- the same two-level reading `daemon_fact_value` now gives every
    /// `daemon` name, backup or not.
    #[test]
    fn a_backup_fact_check_reads_not_resolved_when_never_gathered_and_could_not_be_determined_when_indeterminate() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Daemon { fact: "backup_verified".to_string() }],
            Vec::new(),
        )];
        let statuses = evaluate(&applied, &Evidence::default(), Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(statuses[0].status.reasons().iter().any(|r| r.contains("not resolved")), "{:?}", statuses[0].status);

        let backup_fact = crate::backup::BackupFact {
            at: Utc::now(),
            configured: true,
            newest: None,
            recent: None,
            offsite: None,
            verified: None,
            last_verified: None,
        };
        let evidence = Evidence { backup: Some(backup_fact), ..Default::default() };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(
            statuses[0].status.reasons().iter().any(|r| r.contains("could not be determined")),
            "{:?}",
            statuses[0].status
        );
    }

    #[test]
    fn backup_daemon_checks_read_satisfied_or_open_off_the_backup_fact() {
        let fact_with = |recent: Option<bool>, offsite: Option<bool>, verified: Option<bool>| crate::backup::BackupFact {
            at: Utc::now(),
            configured: true,
            newest: Some(Utc::now()),
            recent,
            offsite,
            verified,
            last_verified: None,
        };

        let recent_applied = vec![applied_control("a", vec![Check::Daemon { fact: "backup_recent".to_string() }], Vec::new())];
        let evidence = Evidence { backup: Some(fact_with(Some(true), Some(false), Some(false))), ..Default::default() };
        assert_eq!(evaluate(&recent_applied, &evidence, Utc::now())[0].status.kind(), StatusKind::Satisfied);

        let offsite_applied = vec![applied_control("a", vec![Check::Daemon { fact: "backup_offsite".to_string() }], Vec::new())];
        let evidence = Evidence { backup: Some(fact_with(Some(true), Some(false), Some(false))), ..Default::default() };
        let statuses = evaluate(&offsite_applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open, "the temp destination is the same device");
        assert!(statuses[0].status.reasons().iter().any(|r| r.contains("does not hold")), "{:?}", statuses[0].status);
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
                corrective: None,
                clock: None,
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
                corrective: None,
                clock: None,
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
                corrective: None,
                clock: None,
            }],
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, now);
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
    }

    /// A CRA Art. 14 clock submission (`#157`, phase 1) is evidence for one
    /// deadline, not for the whole control -- `direct_status` must skip it
    /// exactly like a withdrawn row, leaving the control `open`.
    #[test]
    fn an_attestation_carrying_a_clock_mark_does_not_satisfy_the_control() {
        let now = Utc::now();
        let applied = vec![applied_control("a", vec![Check::Attestation], Vec::new())];
        let evidence = Evidence {
            attestations: vec![Attestation {
                id: "att-1".to_string(),
                control: ControlRef::new("cra", "a"),
                scope: "root".to_string(),
                evidence: "https://example.com/notice".to_string(),
                note: None,
                attested_by: "owner".to_string(),
                attested_at: now,
                expires_at: now + chrono::Duration::days(30),
                withdrawn: None,
                corrective: None,
                clock: Some(crate::reporting_clock::ClockMark {
                    item: crate::reporting_clock::ClockItemRef::Finding {
                        scope: "demo".to_string(),
                        vulnerability: "CVE-2026-1234".to_string(),
                    },
                    deadline: crate::reporting_clock::ClockDeadlineKind::EarlyWarning,
                }),
            }],
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, now);
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
    }

    /// A row recorded before `#157` carries no `clock` field at all -- it
    /// must still deserialize, and still count as an ordinary attestation.
    #[test]
    fn an_attestation_with_no_clock_field_still_deserializes_and_counts() {
        let now = Utc::now();
        let json = serde_json::json!({
            "id": "att-1",
            "control": "cra/a",
            "scope": "root",
            "evidence": "https://example.com/policy",
            "attested_by": "owner",
            "attested_at": now,
            "expires_at": now + chrono::Duration::days(30),
        });
        let attestation: Attestation = serde_json::from_value(json).unwrap();
        assert_eq!(attestation.clock, None);

        let applied = vec![applied_control("a", vec![Check::Attestation], Vec::new())];
        let evidence = Evidence { attestations: vec![attestation], ..Default::default() };
        let statuses = evaluate(&applied, &evidence, now);
        assert_eq!(statuses[0].status.kind(), StatusKind::Attested);
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
                corrective: None,
                clock: None,
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
                corrective: None,
                clock: None,
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
    /// literal JSON shape, not just that it round-trips -- and that `refs`
    /// is an ordinary sibling field, present when non-empty and absent (not
    /// `null` or `[]`) when it is.
    #[test]
    fn control_status_serializes_flat_with_status_and_reasons_alongside_control() {
        let cs = ControlStatus {
            control: ControlRef::new("cra", "a"),
            title: "A".to_string(),
            kind: Kind::Regulation,
            refs: vec![EvidenceRef::task("task-1")],
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
                "refs": [{"kind": "task", "id": "task-1"}],
                "status": "satisfied",
                "reasons": ["knowledge: tag `control/cra/a` is present"],
            })
        );
        let back: ControlStatus = serde_json::from_value(json).unwrap();
        assert_eq!(back, cs);

        let empty = ControlStatus { refs: Vec::new(), ..cs };
        let json = serde_json::to_value(&empty).unwrap();
        assert!(json.get("refs").is_none(), "{json:?}");
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

    // -- evaluate: task ----------------------------------------------------

    fn with_max_age(mut a: Applied, max_age: &str) -> Applied {
        a.max_age = Some(max_age.parse().unwrap());
        a
    }

    fn done_run(id: &str, ended_at: DateTime<Utc>) -> RunFact {
        RunFact {
            id: id.to_string(),
            status: RunStatus::Done,
            started_at: ended_at - chrono::Duration::hours(1),
            ended_at: Some(ended_at),
        }
    }

    #[test]
    fn a_task_check_is_satisfied_by_a_recent_done_run() {
        let now = Utc::now();
        let applied = with_max_age(
            applied_control(
                "a",
                vec![Check::Task {
                    task: "sbom-export".to_string(),
                    max_age: None,
                }],
                Vec::new(),
            ),
            "7d",
        );
        let mut evidence = Evidence::default();
        evidence.tasks.insert(
            "sbom-export".to_string(),
            vec![TaskFact {
                id: "task-1".to_string(),
                title: "sbom-export".to_string(),
                runs: vec![done_run("run-1", now - chrono::Duration::hours(1))],
            }],
        );
        let statuses = evaluate(&[applied], &evidence, now);
        assert_eq!(statuses[0].status.kind(), StatusKind::Satisfied);
        assert!(statuses[0].refs.contains(&EvidenceRef::task("task-1")));
        assert!(statuses[0].refs.contains(&EvidenceRef::run("run-1")));
    }

    #[test]
    fn a_task_check_with_no_max_age_treats_any_done_run_as_current() {
        let now = Utc::now();
        let applied = applied_control(
            "a",
            vec![Check::Task {
                task: "sbom-export".to_string(),
                max_age: None,
            }],
            Vec::new(),
        );
        let mut evidence = Evidence::default();
        evidence.tasks.insert(
            "sbom-export".to_string(),
            vec![TaskFact {
                id: "task-1".to_string(),
                title: "sbom-export".to_string(),
                runs: vec![done_run("run-1", now - chrono::Duration::days(400))],
            }],
        );
        let statuses = evaluate(&[applied], &evidence, now);
        assert_eq!(statuses[0].status.kind(), StatusKind::Satisfied);
    }

    /// The whole point of the in-flight rule: a nightly scan's own run must
    /// never flip its control `open` for as long as it is running -- the
    /// older, already-finished run underneath it still decides the status.
    #[test]
    fn a_task_check_is_satisfied_by_an_older_done_run_even_with_a_newer_one_in_progress() {
        let now = Utc::now();
        let applied = with_max_age(
            applied_control(
                "a",
                vec![Check::Task {
                    task: "sbom-export".to_string(),
                    max_age: None,
                }],
                Vec::new(),
            ),
            "7d",
        );
        let mut evidence = Evidence::default();
        evidence.tasks.insert(
            "sbom-export".to_string(),
            vec![TaskFact {
                id: "task-1".to_string(),
                title: "sbom-export".to_string(),
                // Newest first, exactly as `TaskStore::runs` returns them:
                // the in-progress run is ahead of the finished one.
                runs: vec![
                    RunFact {
                        id: "run-2".to_string(),
                        status: RunStatus::Running,
                        started_at: now - chrono::Duration::minutes(5),
                        ended_at: None,
                    },
                    done_run("run-1", now - chrono::Duration::hours(1)),
                ],
            }],
        );
        let statuses = evaluate(&[applied], &evidence, now);
        assert_eq!(statuses[0].status.kind(), StatusKind::Satisfied, "{:?}", statuses[0].status);
        assert!(statuses[0].refs.contains(&EvidenceRef::run("run-1")));
        assert!(!statuses[0].refs.contains(&EvidenceRef::run("run-2")), "the in-progress run is never cited as evidence");
    }

    #[test]
    fn a_task_check_is_open_with_only_an_in_progress_run_and_says_so() {
        let now = Utc::now();
        let applied = applied_control(
            "a",
            vec![Check::Task {
                task: "sbom-export".to_string(),
                max_age: None,
            }],
            Vec::new(),
        );
        let mut evidence = Evidence::default();
        evidence.tasks.insert(
            "sbom-export".to_string(),
            vec![TaskFact {
                id: "task-1".to_string(),
                title: "sbom-export".to_string(),
                runs: vec![RunFact {
                    id: "run-1".to_string(),
                    status: RunStatus::Running,
                    started_at: now,
                    ended_at: None,
                }],
            }],
        );
        let statuses = evaluate(&[applied], &evidence, now);
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(statuses[0].status.reasons()[0].contains("in progress"), "{:?}", statuses[0].status);
        assert!(statuses[0].refs.contains(&EvidenceRef::run("run-1")), "still worth linking to, even though it cannot satisfy the check");
    }

    #[test]
    fn a_task_check_is_stale_once_its_newest_run_is_older_than_max_age() {
        let now = Utc::now();
        let applied = with_max_age(
            applied_control(
                "a",
                vec![Check::Task {
                    task: "sbom-export".to_string(),
                    max_age: None,
                }],
                Vec::new(),
            ),
            "7d",
        );
        let mut evidence = Evidence::default();
        evidence.tasks.insert(
            "sbom-export".to_string(),
            vec![TaskFact {
                id: "task-1".to_string(),
                title: "sbom-export".to_string(),
                runs: vec![done_run("run-1", now - chrono::Duration::days(30))],
            }],
        );
        let statuses = evaluate(&[applied], &evidence, now);
        assert_eq!(statuses[0].status.kind(), StatusKind::Stale);
        assert!(statuses[0].refs.contains(&EvidenceRef::run("run-1")));
    }

    #[test]
    fn a_task_check_is_open_with_no_matching_task() {
        let applied = applied_control(
            "a",
            vec![Check::Task {
                task: "sbom-export".to_string(),
                max_age: None,
            }],
            Vec::new(),
        );
        let statuses = evaluate(&[applied], &Evidence::default(), Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(statuses[0].status.reasons()[0].contains("no task named"), "{:?}", statuses[0].status);
        assert!(statuses[0].refs.is_empty());
    }

    #[test]
    fn a_task_check_is_open_when_the_task_has_never_run() {
        let applied = applied_control(
            "a",
            vec![Check::Task {
                task: "sbom-export".to_string(),
                max_age: None,
            }],
            Vec::new(),
        );
        let mut evidence = Evidence::default();
        evidence.tasks.insert(
            "sbom-export".to_string(),
            vec![TaskFact {
                id: "task-1".to_string(),
                title: "sbom-export".to_string(),
                runs: Vec::new(),
            }],
        );
        let statuses = evaluate(&[applied], &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(statuses[0].status.reasons()[0].contains("never run"), "{:?}", statuses[0].status);
    }

    #[test]
    fn a_task_check_is_open_when_the_newest_finished_run_is_not_done() {
        let now = Utc::now();
        let applied = applied_control(
            "a",
            vec![Check::Task {
                task: "sbom-export".to_string(),
                max_age: None,
            }],
            Vec::new(),
        );
        let mut evidence = Evidence::default();
        evidence.tasks.insert(
            "sbom-export".to_string(),
            vec![TaskFact {
                id: "task-1".to_string(),
                title: "sbom-export".to_string(),
                runs: vec![RunFact {
                    id: "run-1".to_string(),
                    status: RunStatus::Failed,
                    started_at: now - chrono::Duration::hours(2),
                    ended_at: Some(now - chrono::Duration::hours(1)),
                }],
            }],
        );
        let statuses = evaluate(&[applied], &evidence, now);
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(statuses[0].status.reasons()[0].contains("failed"), "{:?}", statuses[0].status);
    }

    #[test]
    fn a_task_check_naming_an_ambiguous_title_is_open_and_a_finding() {
        let applied = applied_control(
            "a",
            vec![Check::Task {
                task: "nightly sweep".to_string(),
                max_age: None,
            }],
            Vec::new(),
        );
        let mut evidence = Evidence::default();
        evidence.tasks.insert(
            "nightly sweep".to_string(),
            vec![
                TaskFact {
                    id: "task-1".to_string(),
                    title: "nightly sweep".to_string(),
                    runs: Vec::new(),
                },
                TaskFact {
                    id: "task-2".to_string(),
                    title: "nightly sweep".to_string(),
                    runs: Vec::new(),
                },
            ],
        );
        let statuses = evaluate(&[applied], &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(
            statuses[0].status.reasons()[0].contains("matches more than one task's title"),
            "{:?}",
            statuses[0].status
        );

        let findings = evidence_findings(&evidence, "demo");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, FindingKind::AmbiguousCheckTarget);
        assert_eq!(findings[0].subject, "demo");
        assert!(findings[0].detail.contains("task-1") && findings[0].detail.contains("task-2"));
    }

    // -- evaluate: workflow --------------------------------------------------

    fn done_workflow_run(id: &str, updated_at: DateTime<Utc>) -> WorkflowRunFact {
        WorkflowRunFact {
            id: id.to_string(),
            status: WorkflowRunStatus::Done,
            updated_at,
        }
    }

    #[test]
    fn a_workflow_check_is_satisfied_by_a_recent_done_run() {
        let now = Utc::now();
        let applied = with_max_age(
            applied_control(
                "a",
                vec![Check::Workflow {
                    workflow: "release train".to_string(),
                    max_age: None,
                }],
                Vec::new(),
            ),
            "7d",
        );
        let mut evidence = Evidence::default();
        evidence.workflows.insert(
            "release train".to_string(),
            vec![WorkflowFact {
                id: "wf-1".to_string(),
                name: "release train".to_string(),
                runs: vec![done_workflow_run("wfr-1", now - chrono::Duration::hours(1))],
            }],
        );
        let statuses = evaluate(&[applied], &evidence, now);
        assert_eq!(statuses[0].status.kind(), StatusKind::Satisfied);
        assert!(statuses[0].refs.contains(&EvidenceRef::workflow_run("wfr-1")));
    }

    /// The workflow-side twin of the task in-flight test: a run still
    /// `Running` ahead of a finished one in `runs` must not stop the older,
    /// finished run from deciding the status.
    #[test]
    fn a_workflow_check_is_satisfied_by_an_older_done_run_even_with_a_newer_one_in_progress() {
        let now = Utc::now();
        let applied = with_max_age(
            applied_control(
                "a",
                vec![Check::Workflow {
                    workflow: "release train".to_string(),
                    max_age: None,
                }],
                Vec::new(),
            ),
            "7d",
        );
        let mut evidence = Evidence::default();
        evidence.workflows.insert(
            "release train".to_string(),
            vec![WorkflowFact {
                id: "wf-1".to_string(),
                name: "release train".to_string(),
                runs: vec![
                    WorkflowRunFact {
                        id: "wfr-2".to_string(),
                        status: WorkflowRunStatus::Running,
                        updated_at: now - chrono::Duration::minutes(5),
                    },
                    done_workflow_run("wfr-1", now - chrono::Duration::hours(1)),
                ],
            }],
        );
        let statuses = evaluate(&[applied], &evidence, now);
        assert_eq!(statuses[0].status.kind(), StatusKind::Satisfied, "{:?}", statuses[0].status);
        assert!(statuses[0].refs.contains(&EvidenceRef::workflow_run("wfr-1")));
    }

    #[test]
    fn a_workflow_check_is_open_with_only_an_in_progress_run_and_says_so() {
        let now = Utc::now();
        let applied = applied_control(
            "a",
            vec![Check::Workflow {
                workflow: "release train".to_string(),
                max_age: None,
            }],
            Vec::new(),
        );
        let mut evidence = Evidence::default();
        evidence.workflows.insert(
            "release train".to_string(),
            vec![WorkflowFact {
                id: "wf-1".to_string(),
                name: "release train".to_string(),
                runs: vec![WorkflowRunFact {
                    id: "wfr-1".to_string(),
                    status: WorkflowRunStatus::Running,
                    updated_at: now,
                }],
            }],
        );
        let statuses = evaluate(&[applied], &evidence, now);
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(statuses[0].status.reasons()[0].contains("in progress"), "{:?}", statuses[0].status);
    }

    #[test]
    fn a_workflow_check_is_stale_once_its_newest_run_is_older_than_max_age() {
        let now = Utc::now();
        let applied = with_max_age(
            applied_control(
                "a",
                vec![Check::Workflow {
                    workflow: "release train".to_string(),
                    max_age: None,
                }],
                Vec::new(),
            ),
            "7d",
        );
        let mut evidence = Evidence::default();
        evidence.workflows.insert(
            "release train".to_string(),
            vec![WorkflowFact {
                id: "wf-1".to_string(),
                name: "release train".to_string(),
                runs: vec![done_workflow_run("wfr-1", now - chrono::Duration::days(30))],
            }],
        );
        let statuses = evaluate(&[applied], &evidence, now);
        assert_eq!(statuses[0].status.kind(), StatusKind::Stale);
    }

    #[test]
    fn a_workflow_check_is_open_with_no_matching_workflow() {
        let applied = applied_control(
            "a",
            vec![Check::Workflow {
                workflow: "release train".to_string(),
                max_age: None,
            }],
            Vec::new(),
        );
        let statuses = evaluate(&[applied], &Evidence::default(), Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(statuses[0].status.reasons()[0].contains("no workflow named"), "{:?}", statuses[0].status);
    }

    #[test]
    fn a_workflow_check_naming_an_ambiguous_name_is_open_and_a_finding() {
        let applied = applied_control(
            "a",
            vec![Check::Workflow {
                workflow: "release train".to_string(),
                max_age: None,
            }],
            Vec::new(),
        );
        let mut evidence = Evidence::default();
        evidence.workflows.insert(
            "release train".to_string(),
            vec![
                WorkflowFact {
                    id: "wf-1".to_string(),
                    name: "release train".to_string(),
                    runs: Vec::new(),
                },
                WorkflowFact {
                    id: "wf-2".to_string(),
                    name: "release train".to_string(),
                    runs: Vec::new(),
                },
            ],
        );
        let statuses = evaluate(&[applied], &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(statuses[0]
            .status
            .reasons()[0]
            .contains("matches more than one workflow's name"));

        let findings = evidence_findings(&evidence, "demo");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, FindingKind::AmbiguousCheckTarget);
    }

    // -- evaluate: gate ------------------------------------------------------

    fn gate_check(dataset: &str, case: Option<&str>) -> Check {
        Check::Gate {
            dataset: dataset.to_string(),
            case: case.map(str::to_string),
            max_age: None,
        }
    }

    #[test]
    fn a_gate_check_is_satisfied_when_every_gated_case_passed_recently() {
        let now = Utc::now();
        let applied = with_max_age(
            applied_control("a", vec![gate_check("smoke", None)], Vec::new()),
            "7d",
        );
        let mut evidence = Evidence::default();
        evidence.gates.insert(
            "smoke".to_string(),
            GateFact {
                run_id: "bench-1".to_string(),
                ended_at: Some(now - chrono::Duration::hours(1)),
                cases: vec![
                    GateCase {
                        id: "case-1".to_string(),
                        gated: true,
                        verdicts: vec![Verdict::Pass, Verdict::Pass],
                    },
                    GateCase {
                        id: "case-2".to_string(),
                        gated: false,
                        verdicts: vec![Verdict::Unverified],
                    },
                ],
            },
        );
        let statuses = evaluate(&[applied], &evidence, now);
        assert_eq!(statuses[0].status.kind(), StatusKind::Satisfied);
        assert!(statuses[0].refs.contains(&EvidenceRef::bench_run("bench-1")));
    }

    #[test]
    fn a_gate_check_for_one_named_case_only_looks_at_that_case() {
        let now = Utc::now();
        let applied = with_max_age(
            applied_control("a", vec![gate_check("smoke", Some("case-1"))], Vec::new()),
            "7d",
        );
        let mut evidence = Evidence::default();
        evidence.gates.insert(
            "smoke".to_string(),
            GateFact {
                run_id: "bench-1".to_string(),
                ended_at: Some(now - chrono::Duration::hours(1)),
                cases: vec![
                    GateCase {
                        id: "case-1".to_string(),
                        gated: true,
                        verdicts: vec![Verdict::Pass],
                    },
                    GateCase {
                        id: "case-2".to_string(),
                        gated: true,
                        verdicts: vec![Verdict::Fail],
                    },
                ],
            },
        );
        let statuses = evaluate(&[applied], &evidence, now);
        assert_eq!(statuses[0].status.kind(), StatusKind::Satisfied, "case-2 failing must not matter");
    }

    #[test]
    fn a_gate_check_is_stale_once_the_settled_run_is_older_than_max_age() {
        let now = Utc::now();
        let applied = with_max_age(
            applied_control("a", vec![gate_check("smoke", None)], Vec::new()),
            "7d",
        );
        let mut evidence = Evidence::default();
        evidence.gates.insert(
            "smoke".to_string(),
            GateFact {
                run_id: "bench-1".to_string(),
                ended_at: Some(now - chrono::Duration::days(30)),
                cases: vec![GateCase {
                    id: "case-1".to_string(),
                    gated: true,
                    verdicts: vec![Verdict::Pass],
                }],
            },
        );
        let statuses = evaluate(&[applied], &evidence, now);
        assert_eq!(statuses[0].status.kind(), StatusKind::Stale);
    }

    #[test]
    fn a_gate_check_is_open_with_no_settled_run() {
        let applied = applied_control("a", vec![gate_check("smoke", None)], Vec::new());
        let statuses = evaluate(&[applied], &Evidence::default(), Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(statuses[0].status.reasons()[0].contains("no settled bench run"), "{:?}", statuses[0].status);
    }

    #[test]
    fn a_gate_check_is_open_when_a_gated_case_did_not_pass() {
        let now = Utc::now();
        let applied = applied_control("a", vec![gate_check("smoke", None)], Vec::new());
        let mut evidence = Evidence::default();
        evidence.gates.insert(
            "smoke".to_string(),
            GateFact {
                run_id: "bench-1".to_string(),
                ended_at: Some(now),
                cases: vec![GateCase {
                    id: "case-1".to_string(),
                    gated: true,
                    verdicts: vec![Verdict::Fail],
                }],
            },
        );
        let statuses = evaluate(&[applied], &evidence, now);
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(statuses[0].status.reasons()[0].contains("case-1"), "{:?}", statuses[0].status);
    }

    #[test]
    fn a_gate_check_naming_a_case_with_no_gate_is_open() {
        let now = Utc::now();
        let applied = applied_control("a", vec![gate_check("smoke", Some("case-1"))], Vec::new());
        let mut evidence = Evidence::default();
        evidence.gates.insert(
            "smoke".to_string(),
            GateFact {
                run_id: "bench-1".to_string(),
                ended_at: Some(now),
                cases: vec![GateCase {
                    id: "case-1".to_string(),
                    gated: false,
                    verdicts: vec![Verdict::Unverified],
                }],
            },
        );
        let statuses = evaluate(&[applied], &evidence, now);
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(statuses[0].status.reasons()[0].contains("no gate configured"), "{:?}", statuses[0].status);
    }

    // -- evidence_findings ---------------------------------------------------

    #[test]
    fn evidence_findings_is_empty_when_nothing_is_ambiguous() {
        let mut evidence = Evidence::default();
        evidence.tasks.insert(
            "sbom-export".to_string(),
            vec![TaskFact {
                id: "task-1".to_string(),
                title: "sbom-export".to_string(),
                runs: Vec::new(),
            }],
        );
        assert_eq!(evidence_findings(&evidence, "demo"), Vec::new());
    }

    // -- rollup ----------------------------------------------------------

    fn status(framework: &str, id: &str, kind: Kind, status: Status) -> ControlStatus {
        ControlStatus {
            control: ControlRef::new(framework, id),
            title: id.to_string(),
            kind,
            refs: Vec::new(),
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

    // -- worst_across_scopes ------------------------------------------------

    #[test]
    fn the_worst_status_anywhere_wins_over_a_better_one_elsewhere() {
        let per_scope = vec![
            vec![status("cra", "a", Kind::Regulation, Status::Satisfied { reasons: vec![] })],
            vec![status("cra", "a", Kind::Regulation, Status::Open { reasons: vec![] })],
        ];
        let worst = worst_across_scopes(&per_scope);
        assert_eq!(worst.len(), 1);
        assert_eq!(worst[0].status.kind(), StatusKind::Open);
    }

    #[test]
    fn stale_outranks_attested_which_outranks_satisfied() {
        let a = ControlRef::new("cra", "a");
        let per_scope = vec![
            vec![ControlStatus {
                control: a.clone(),
                title: "A".into(),
                kind: Kind::Regulation,
                refs: Vec::new(),
                status: Status::Satisfied { reasons: vec![] },
            }],
            vec![ControlStatus {
                control: a.clone(),
                title: "A".into(),
                kind: Kind::Regulation,
                refs: Vec::new(),
                status: Status::Attested { reasons: vec![] },
            }],
            vec![ControlStatus {
                control: a,
                title: "A".into(),
                kind: Kind::Regulation,
                refs: Vec::new(),
                status: Status::Stale { reasons: vec![] },
            }],
        ];
        let worst = worst_across_scopes(&per_scope);
        assert_eq!(worst[0].status.kind(), StatusKind::Stale);
    }

    /// The case the naive `StatusKind::rank()` ordering gets wrong:
    /// `not_applicable` must never be treated as worse than a real `open` --
    /// a scope with nothing to say about a control must not drag down one
    /// that actually has to answer for it.
    #[test]
    fn not_applicable_never_outranks_a_real_status() {
        let per_scope = vec![
            vec![status(
                "cra",
                "a",
                Kind::Regulation,
                Status::NotApplicable { reasons: vec![] },
            )],
            vec![status("cra", "a", Kind::Regulation, Status::Satisfied { reasons: vec![] })],
        ];
        let worst = worst_across_scopes(&per_scope);
        assert_eq!(worst.len(), 1);
        assert_eq!(
            worst[0].status.kind(),
            StatusKind::Satisfied,
            "the scope that actually applies the control decides, not the one that opted out"
        );
    }

    #[test]
    fn not_applicable_everywhere_keeps_that_status() {
        let per_scope = vec![
            vec![status(
                "cra",
                "a",
                Kind::Regulation,
                Status::NotApplicable { reasons: vec![] },
            )],
            vec![status(
                "cra",
                "a",
                Kind::Regulation,
                Status::NotApplicable { reasons: vec![] },
            )],
        ];
        let worst = worst_across_scopes(&per_scope);
        assert_eq!(worst.len(), 1);
        assert_eq!(worst[0].status.kind(), StatusKind::NotApplicable);
    }

    #[test]
    fn a_control_present_in_only_some_scopes_is_still_aggregated() {
        let per_scope = vec![
            vec![status("cra", "a", Kind::Regulation, Status::Open { reasons: vec![] })],
            vec![status("gdpr", "b", Kind::Regulation, Status::Satisfied { reasons: vec![] })],
        ];
        let worst = worst_across_scopes(&per_scope);
        let by_id: BTreeMap<&str, &ControlStatus> =
            worst.iter().map(|s| (s.control.id.as_str(), s)).collect();
        assert_eq!(by_id.len(), 2);
        assert_eq!(by_id["a"].status.kind(), StatusKind::Open);
        assert_eq!(by_id["b"].status.kind(), StatusKind::Satisfied);
    }

    /// Feeding the result straight into `rollup` is the whole point --
    /// `PolicyReport.rollup` is built exactly this way.
    #[test]
    fn worst_across_scopes_feeds_rollup_to_the_subtree_wide_answer() {
        let per_scope = vec![
            vec![status("cra", "a", Kind::Regulation, Status::Satisfied { reasons: vec![] })],
            vec![status("cra", "a", Kind::Regulation, Status::Open { reasons: vec![] })],
        ];
        let rollups = rollup(&worst_across_scopes(&per_scope));
        assert_eq!(rollups.len(), 1);
        assert!(!rollups[0].compliant, "open in even one applicable scope is not compliant");
    }

    // -- remediation (#83) --------------------------------------------------

    #[test]
    fn status_kind_and_evidence_ref_kind_spell_the_wire_form() {
        assert_eq!(StatusKind::NotApplicable.as_str(), "not_applicable");
        assert_eq!(StatusKind::Satisfied.as_str(), "satisfied");
        assert_eq!(EvidenceRefKind::WorkflowRun.as_str(), "workflow_run");
        assert_eq!(EvidenceRefKind::BenchRun.as_str(), "bench_run");
    }

    #[test]
    fn check_describe_names_what_it_checks() {
        assert_eq!(Check::Sandbox.describe(), "sandbox");
        assert_eq!(
            // `Duration`'s own `Display` prefers weeks when a value divides
            // evenly -- 7 days is 1 week, so that is what prints.
            Check::Task { task: "sbom export".into(), max_age: Some("7d".parse().unwrap()) }.describe(),
            "task sbom export (max_age 1w)"
        );
        assert_eq!(
            Check::Knowledge { tag: None }.describe(),
            "knowledge: default tag"
        );
    }

    #[test]
    fn remediation_instructions_carries_all_three_parts_in_order() {
        let control = ControlRef::new("cra", "annex-i-2-1");
        let status = Status::Open {
            reasons: vec!["knowledge: tag `control/cra/annex-i-2-1` not found".to_string()],
        };
        let checks = vec![Check::Knowledge { tag: None }, Check::Attestation];
        let text = remediation_instructions(&control, Some("Publish an SBOM.  \n"), &status, &checks);

        let remediation_at = text.find("Publish an SBOM.").expect("remediation text present");
        let missing_at = text.find("Missing evidence:").expect("missing-evidence heading present");
        let reason_at = text
            .find("knowledge: tag `control/cra/annex-i-2-1` not found")
            .expect("the control's own reason, verbatim");
        let closing_at = text
            .find("Any one of these checks passing closes cra/annex-i-2-1:")
            .expect("closing line names the control");
        assert!(remediation_at < missing_at, "{text}");
        assert!(missing_at < reason_at, "{text}");
        assert!(reason_at < closing_at, "{text}");
        assert!(text.contains("- knowledge: default tag"), "{text}");
        assert!(text.contains("- attestation"), "{text}");
        // Trimmed, including the trailing whitespace `remediation:` carried.
        assert!(!text.ends_with(char::is_whitespace), "{text:?}");
    }

    #[test]
    fn remediation_instructions_omits_the_guidance_paragraph_when_there_is_none() {
        let control = ControlRef::new("cra", "annex-i-2-2");
        let status = Status::Stale { reasons: vec!["attestation: `att-1` expired at 2020-01-01T00:00:00Z".to_string()] };
        let text = remediation_instructions(&control, None, &status, &[Check::Attestation]);
        assert!(text.starts_with("Missing evidence:"), "{text:?}");
    }

    #[test]
    fn dependencies_check_enforces_freshness_severity_and_exploitation_limits() {
        let check = Check::Dependencies {
            sbom_max_age: Some("30d".parse().unwrap()),
            built_sbom: false,
            max_open: BTreeMap::from([(Severity::Critical, 0)]),
            exploited_open: Some(0),
        };
        let applied = vec![applied_control("dependencies", vec![check], Vec::new())];
        let now = "2026-09-25T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let mut evidence = Evidence::default();
        evidence.dependencies = Some(DependenciesFact {
            declared_sbom_at: Some(now - chrono::Duration::days(1)),
            built_sbom_at: None,
            open: BTreeMap::new(),
            exploited_open: 0,
        });
        assert_eq!(evaluate(&applied, &evidence, now)[0].status.kind(), StatusKind::Satisfied);

        evidence.dependencies.as_mut().unwrap().open.insert(Severity::Critical, 1);
        evidence.dependencies.as_mut().unwrap().exploited_open = 1;
        assert_eq!(evaluate(&applied, &evidence, now)[0].status.kind(), StatusKind::Open);
    }

    #[test]
    fn dependencies_check_requires_build_evidence_when_requested() {
        let check = Check::Dependencies {
            sbom_max_age: None,
            built_sbom: true,
            max_open: BTreeMap::new(),
            exploited_open: None,
        };
        let applied = vec![applied_control("built-dependencies", vec![check], Vec::new())];
        let now = "2026-09-25T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let mut evidence = Evidence::default();
        evidence.dependencies = Some(DependenciesFact::default());

        let missing = evaluate(&applied, &evidence, now);
        assert_eq!(missing[0].status.kind(), StatusKind::Open);
        assert!(
            missing[0]
                .status
                .reasons()
                .iter()
                .any(|reason| reason == "dependencies: no built SBOM for the newest release")
        );

        evidence.dependencies.as_mut().unwrap().built_sbom_at = Some(now);
        assert_eq!(
            evaluate(&applied, &evidence, now)[0].status.kind(),
            StatusKind::Satisfied
        );
    }

    // -- evaluate: attested (#158) -------------------------------------------

    fn attested_run(
        run_id: &str,
        status: RunStatus,
        ended_at: DateTime<Utc>,
        required: Vec<crate::control_plan::RequiredStep>,
        attestations: Vec<crate::control_plan::StepAttestation>,
    ) -> AttestedRun {
        AttestedRun {
            run_id: run_id.to_string(),
            task_id: format!("task-{run_id}"),
            scope: "demo".to_string(),
            category: "feature".to_string(),
            agent: "worker".to_string(),
            status,
            ended_at,
            fail_kind: None,
            required_steps: required,
            attestations,
        }
    }

    fn gate_step(name: &str) -> crate::control_plan::RequiredStep {
        crate::control_plan::RequiredStep {
            step: name.to_string(),
            kind: crate::control_plan::StepKind::Gate,
            command: Some("true".to_string()),
            timeout_seconds: None,
            required_by: Vec::new(),
            node_id: None,
            by: None,
            actor: None,
        }
    }

    fn gate_attestation(
        run_id: &str,
        step: &str,
        verdict: crate::control_plan::AttestationVerdict,
        actor: &str,
        at: DateTime<Utc>,
    ) -> crate::control_plan::StepAttestation {
        crate::control_plan::StepAttestation {
            id: uuid::Uuid::new_v4().to_string(),
            run_id: run_id.to_string(),
            task_id: format!("task-{run_id}"),
            scope: "demo".to_string(),
            category: "feature".to_string(),
            step: step.to_string(),
            kind: crate::control_plan::StepKind::Gate,
            actor: actor.to_string(),
            verdict,
            findings: None,
            round: 0,
            required_by: Vec::new(),
            command: Some("true".to_string()),
            exit_code: Some(
                if verdict == crate::control_plan::AttestationVerdict::Pass {
                    0
                } else {
                    1
                },
            ),
            output: None,
            dir: "/tmp".to_string(),
            commit: None,
            dirty: None,
            worktree_digest: None,
            node_id: None,
            at,
        }
    }

    fn attested_check(step: &str, max_age: &str) -> Check {
        Check::Attested {
            category: "feature".to_string(),
            step: step.to_string(),
            max_age: max_age.parse().unwrap(),
        }
    }

    #[test]
    fn attested_check_serde_round_trips_and_refuses_a_missing_max_age() {
        let check = attested_check("tests", "7d");
        let json = serde_json::to_string(&check).unwrap();
        assert_eq!(serde_json::from_str::<Check>(&json).unwrap(), check);

        // A catalogue that leaves `max_age` out fails the whole file --
        // `deny_unknown_fields` and the required field both make this a
        // parse failure, never a silently open control.
        let bad = r#"{"check":"attested","category":"feature","step":"tests"}"#;
        assert!(serde_json::from_str::<Check>(bad).is_err());
        let unknown_field =
            r#"{"check":"attested","category":"feature","step":"tests","max_age":"7d","extra":1}"#;
        assert!(serde_json::from_str::<Check>(unknown_field).is_err());
    }

    #[test]
    fn attested_describes_itself() {
        // `Duration`'s own `Display` prefers the widest exact unit -- 7
        // days is a whole week, so it prints `1w`, the same rule every
        // other check's `max_age` formatting already follows.
        assert_eq!(
            attested_check("tests", "7d").describe(),
            "attested: feature/tests (max_age 1w)"
        );
    }

    #[test]
    fn check_vocabulary_refuses_a_bad_attested_category_or_step() {
        assert!(check_vocabulary(&attested_check("tests", "7d")).is_empty());
        let found = check_vocabulary(&Check::Attested {
            category: "Not A Name".to_string(),
            step: "tests".to_string(),
            max_age: "7d".parse().unwrap(),
        });
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, FindingKind::BadCheckTarget);
        let found = check_vocabulary(&Check::Attested {
            category: "feature".to_string(),
            step: "Not A Name".to_string(),
            max_age: "7d".parse().unwrap(),
        });
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, FindingKind::BadCheckTarget);
    }

    #[test]
    fn gathered_is_true_only_once_attested_evidence_was_read() {
        let check = attested_check("tests", "7d");
        assert!(!crate::quality::gathered(&check, &Evidence::default()));
        let evidence = Evidence {
            attested: Some(Vec::new()),
            ..Default::default()
        };
        assert!(crate::quality::gathered(&check, &evidence));
    }

    #[test]
    fn attested_is_satisfied_when_every_run_within_max_age_passed() {
        let applied = vec![with_max_age(
            applied_control(
                "attested-a",
                vec![attested_check("tests", "7d")],
                Vec::new(),
            ),
            "7d",
        )];
        let now = Utc::now();
        let ended = now - chrono::Duration::days(1);
        let run = attested_run(
            "r1",
            RunStatus::Done,
            ended,
            vec![gate_step("tests")],
            vec![gate_attestation(
                "r1",
                "tests",
                crate::control_plan::AttestationVerdict::Pass,
                "factory-daemon",
                ended,
            )],
        );
        let evidence = Evidence {
            attested: Some(vec![run]),
            ..Default::default()
        };
        let status = evaluate(&applied, &evidence, now)[0].status.clone();
        assert_eq!(status.kind(), StatusKind::Satisfied);
        assert!(
            status.reasons().iter().any(|r| r.starts_with("attested: ")),
            "{status:?}"
        );
    }

    #[test]
    fn attested_is_open_when_an_in_window_run_failed_or_was_never_held() {
        let applied = vec![with_max_age(
            applied_control(
                "attested-b",
                vec![attested_check("tests", "7d")],
                Vec::new(),
            ),
            "7d",
        )];
        let now = Utc::now();
        let ended = now - chrono::Duration::days(1);
        // Failed the gate.
        let failed = attested_run(
            "r1",
            RunStatus::Done,
            ended,
            vec![gate_step("tests")],
            vec![gate_attestation(
                "r1",
                "tests",
                crate::control_plan::AttestationVerdict::Fail,
                "factory-daemon",
                ended,
            )],
        );
        let mut evidence = Evidence {
            attested: Some(vec![failed]),
            ..Default::default()
        };
        let status = evaluate(&applied, &evidence, now)[0].status.clone();
        assert_eq!(status.kind(), StatusKind::Open);

        // Never held to `tests` at all.
        let never_held = attested_run("r2", RunStatus::Done, ended, Vec::new(), Vec::new());
        evidence.attested = Some(vec![never_held]);
        let status = evaluate(&applied, &evidence, now)[0].status.clone();
        assert_eq!(status.kind(), StatusKind::Open);
        assert!(
            status.reasons().iter().any(|r| r.contains("never held to")),
            "{status:?}"
        );
    }

    #[test]
    fn attested_is_stale_when_only_an_older_run_within_2w_passed() {
        let applied = vec![with_max_age(
            applied_control(
                "attested-c",
                vec![attested_check("tests", "7d")],
                Vec::new(),
            ),
            "7d",
        )];
        let now = Utc::now();
        // 10 days ago: outside the 7d window, inside the 14d lookback.
        let ended = now - chrono::Duration::days(10);
        let run = attested_run(
            "r1",
            RunStatus::Done,
            ended,
            vec![gate_step("tests")],
            vec![gate_attestation(
                "r1",
                "tests",
                crate::control_plan::AttestationVerdict::Pass,
                "factory-daemon",
                ended,
            )],
        );
        let evidence = Evidence {
            attested: Some(vec![run]),
            ..Default::default()
        };
        let status = evaluate(&applied, &evidence, now)[0].status.clone();
        assert_eq!(status.kind(), StatusKind::Stale);
    }

    #[test]
    fn attested_is_open_when_nothing_ended_done_within_2w() {
        let applied = vec![with_max_age(
            applied_control(
                "attested-d",
                vec![attested_check("tests", "7d")],
                Vec::new(),
            ),
            "7d",
        )];
        let now = Utc::now();
        // Nothing at all.
        let mut evidence = Evidence {
            attested: Some(Vec::new()),
            ..Default::default()
        };
        assert_eq!(
            evaluate(&applied, &evidence, now)[0].status.kind(),
            StatusKind::Open
        );

        // Something, but older than 2W (14d here).
        let ended = now - chrono::Duration::days(20);
        let run = attested_run(
            "r1",
            RunStatus::Done,
            ended,
            vec![gate_step("tests")],
            vec![gate_attestation(
                "r1",
                "tests",
                crate::control_plan::AttestationVerdict::Pass,
                "factory-daemon",
                ended,
            )],
        );
        evidence.attested = Some(vec![run]);
        assert_eq!(
            evaluate(&applied, &evidence, now)[0].status.kind(),
            StatusKind::Open
        );
    }

    #[test]
    fn attested_is_open_when_never_gathered() {
        let applied = vec![with_max_age(
            applied_control(
                "attested-e",
                vec![attested_check("tests", "7d")],
                Vec::new(),
            ),
            "7d",
        )];
        let status = evaluate(&applied, &Evidence::default(), Utc::now())[0]
            .status
            .clone();
        assert_eq!(status.kind(), StatusKind::Open);
        assert!(
            status.reasons().iter().any(|r| r.contains("not resolved")),
            "{status:?}"
        );
    }

    #[test]
    fn a_self_attested_gate_never_satisfies_attested() {
        let applied = vec![with_max_age(
            applied_control(
                "attested-f",
                vec![attested_check("tests", "7d")],
                Vec::new(),
            ),
            "7d",
        )];
        let now = Utc::now();
        let ended = now - chrono::Duration::days(1);
        // Attested by the run's own agent -- `control_plan::judge`'s own
        // exclusion, so this must not satisfy the check.
        let run = attested_run(
            "r1",
            RunStatus::Done,
            ended,
            vec![gate_step("tests")],
            vec![gate_attestation(
                "r1",
                "tests",
                crate::control_plan::AttestationVerdict::Pass,
                "worker",
                ended,
            )],
        );
        let evidence = Evidence {
            attested: Some(vec![run]),
            ..Default::default()
        };
        assert_eq!(
            evaluate(&applied, &evidence, now)[0].status.kind(),
            StatusKind::Open
        );
    }

    #[test]
    fn a_scope_tighten_on_attested_max_age_is_honoured_by_evaluate() {
        // The catalogue's own `attested` check names 30d; a scope tightens
        // it to 7d. `direct_status` must read `applied.max_age` (7d, the
        // tightened value), never the check's own raw field (30d) --
        // otherwise a run 10 days old would wrongly read `satisfied`.
        let mut ctl = control("a");
        ctl.evidence = vec![Check::Attested {
            category: "feature".to_string(),
            step: "tests".to_string(),
            max_age: "30d".parse().unwrap(),
        }];
        let catalogues = vec![cra_catalogue(vec![ctl])];
        let mut root = layer("root", &["cra"]);
        root.tighten.insert(
            ControlRef::new("cra", "a"),
            Tighten {
                max_age: Some("7d".parse().unwrap()),
            },
        );
        let (applied, findings) = applicable(&catalogues, &[root]);
        assert!(findings.is_empty(), "{findings:?}");
        assert_eq!(applied[0].max_age, Some("7d".parse().unwrap()));

        let now = Utc::now();
        let ended = now - chrono::Duration::days(10);
        let run = attested_run(
            "r1",
            RunStatus::Done,
            ended,
            vec![gate_step("tests")],
            vec![gate_attestation(
                "r1",
                "tests",
                crate::control_plan::AttestationVerdict::Pass,
                "factory-daemon",
                ended,
            )],
        );
        let evidence = Evidence {
            attested: Some(vec![run]),
            ..Default::default()
        };
        // Stale, not satisfied: 10 days is outside the tightened 7d window.
        assert_eq!(
            evaluate(&applied, &evidence, now)[0].status.kind(),
            StatusKind::Stale
        );
    }

    // -- #193 phase 2: the fact vocabulary moved, the wire form did not -----

    /// A lock on `Evidence`'s own JSON shape, touching every field the L0
    /// kernel move (`DaemonFact`'s rename to `DaemonConfigFact`,
    /// `secrets`'s retyping to `factory_kernel::SecretsPresence`, `backup`'s
    /// move) could plausibly have disturbed. `daemon` and `backup` each
    /// cover both the `Some` and the `skip_serializing_if`-omitted arm of
    /// their own `Option` fields, so a field silently reappearing (or
    /// disappearing) on the wire fails this test, not just a type check.
    #[test]
    fn evidence_serializes_exactly_as_it_did_before_the_fact_types_moved() {
        let mut secrets = factory_kernel::SecretsPresence::new();
        secrets.insert("github".to_string(), true);
        secrets.insert("scope_env".to_string(), false);

        let evidence = Evidence {
            tags: BTreeSet::from(["reviewed".to_string()]),
            secrets,
            daemon: Some(DaemonFact { foreman_enabled: true, http_loopback_only: None, power_assertion: true }),
            dependencies: Some(DependenciesFact {
                declared_sbom_at: Some("2026-09-01T00:00:00Z".parse().unwrap()),
                built_sbom_at: None,
                open: BTreeMap::from([(Severity::High, 2)]),
                exploited_open: 1,
            }),
            backup: Some(crate::backup::BackupFact {
                at: "2026-09-29T00:00:00Z".parse().unwrap(),
                configured: true,
                newest: Some("2026-09-28T00:00:00Z".parse().unwrap()),
                recent: Some(true),
                offsite: None,
                verified: Some(false),
                last_verified: None,
            }),
            ..Default::default()
        };

        let json = serde_json::to_value(&evidence).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "tags": ["reviewed"],
                "attestations": [],
                "tasks": {},
                "workflows": {},
                "gates": {},
                "agents": null,
                "secrets": {"github": true, "scope_env": false},
                "daemon": {"foreman_enabled": true, "power_assertion": true},
                "dependencies": {
                    "declared_sbom_at": "2026-09-01T00:00:00Z",
                    "open": {"high": 2},
                    "exploited_open": 1
                },
                "backup": {
                    "at": "2026-09-29T00:00:00Z",
                    "configured": true,
                    "newest": "2026-09-28T00:00:00Z",
                    "recent": true,
                    "offsite": null,
                    "verified": false,
                    "last_verified": null
                },
                "attested": null
            })
        );

        // And it round-trips: the L0 move did not add a field this build's
        // own `Evidence` cannot read back.
        let back: Evidence = serde_json::from_value(json).unwrap();
        assert_eq!(back, evidence);
    }
}
