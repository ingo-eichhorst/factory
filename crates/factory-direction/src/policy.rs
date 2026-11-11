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
//! ## Storage and ownership (#390)
//!
//! A policy belongs to one scope. A scope that needs one writes it in its
//! own directory, `<scope>/.factory/policies/<framework>.yaml`; the root
//! scope's own policies are `<root>/.factory/policies/<framework>.yaml`. A
//! scope without one has none -- nothing is inherited down the tree, and a
//! parent's view is the *aggregate* of its subtree, never a chain folded
//! into its children. Each control states its evidence in that scope's own
//! terms: its metrics, tasks, workflows, datasets.
//!
//! **Templates** (`<root>/.factory/policies/templates/<name>.yaml`) are
//! lists of possible controls -- the CRA, ISO 27001, the GDPR, the AI Act
//! -- with no evidence. A policy that names a `template:` picks controls
//! from it by id, inheriting title, kind, `maps_to` and remediation; a
//! template control it neither picks nor marks `n/a` is *not selected*,
//! listed and never counted. A policy without a template declares its own
//! controls. Neither `templates/` nor `drafts/` is ever read as a policy.
//!
//! Everything here is authored content, like `.factory/knowledge/`:
//! hand-written, re-parsed on every read, never written by Factory. [`load`]
//! reads the whole instance; every mistake is a [`Finding`], and one bad
//! file never stops the others from loading.
//!
//! ## Evidence and evaluation
//!
//! [`evaluate`] takes the applicable controls, a bundle of [`Evidence`]
//! Factory already has lying around, and `now`, and produces a
//! [`ControlStatus`] per control -- pure, so a caller passes `now` in rather
//! than this module reading the clock. `evaluate` now understands all
//! thirteen of `Check`'s kinds: `knowledge` and `attestation` (v1), `task`,
//! `workflow` and `gate` (`#81`), and `roles`, `sandbox`, `secrets` and
//! `daemon` (`#82`), plus `dependencies` (`#123`), `attested` (`#158`) and
//! `metric` (`#278`).
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
//! **`check: metric` (`#278`).** A control can also be judged by a plain
//! metric: `{ check: metric, metric: <id>, above|below: <n>, max_age: <d> }`,
//! judged by the exact same arithmetic as a Quality `MetricMeasure`
//! (inclusive bounds, `stale` beyond `max_age`, a `None`/non-finite value is
//! `no_data`) -- see `factory_assurance::metrics::judge_metric`, the one
//! place both share. Unlike Quality's four-way status, a control has only
//! two: `met` is `satisfied`; `not_met`, `stale` and no value at all are all
//! `open`, with the reason -- the status vocabulary above has no bucket for
//! "used to be current" or "found but wrong" that `open` does not already
//! cover. `compliance.<framework>`, `open_controls.<framework>` and
//! `quality.<characteristic>` are refused as circular -- a control judged by
//! a number this same evaluation produces would never be a measurement --
//! the same reasoning Quality already applies to a `quality.*` measure.
//! Without it, a scope's own reported metrics (`#278`'s first phase) could
//! show progress at L5 but never close an L6 control.
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

use chrono::{DateTime, Utc};
#[cfg(test)]
use factory_assurance::bench::Verdict;
#[cfg(test)]
use factory_kernel::AttestedRun;
#[cfg(test)]
use factory_kernel::Grant;
#[cfg(test)]
use factory_kernel::RunStatus;
#[cfg(test)]
use factory_kernel::WorkflowRunStatus;
#[cfg(test)]
use factory_kernel::{DependenciesFact, Severity};
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

pub use factory_assurance::checks::{
    Check, Evidence, EvidenceRef, EvidenceRefKind, Status, StatusKind,
};
pub use factory_kernel::{Attestation, ControlRef, Withdrawal};
pub type ControlStatus = factory_assurance::checks::EvaluationResult<Kind>;
pub use factory_kernel::{KNOWN_DAEMON_FACTS, KNOWN_SECRETS_LOCATIONS};

// ============================================================== templates

/// A list of possible controls -- the CRA, ISO 27001, the GDPR, the AI Act
/// -- with no evidence (#390). A template only helps a scope pick its
/// controls; the scope's own policy translates each one it picks into that
/// scope's evidence. Authored at `<root>/.factory/policies/templates/<name>.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    /// Must equal the file's stem, like a policy's `framework`.
    pub template: String,
    pub title: String,
    pub kind: Kind,
    #[serde(default)]
    pub controls: Vec<TemplateControl>,
}

/// One possible control: an id, our own title, and what a scope picking it
/// inherits -- never evidence, which only a scope can name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateControl {
    /// `[a-z0-9][a-z0-9-]*`, the id a scope's policy picks it by.
    pub id: String,
    pub title: String,
    /// Overrides the template's own `kind` for this control alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Kind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub maps_to: Vec<ControlRef>,
    /// Generic guidance on what usually closes it; a scope may override it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

/// The template file as written. `evidence:` is accepted on a control only
/// so the loader can name the mistake -- [`FindingKind::TemplateEvidence`]
/// -- instead of failing the whole file over it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTemplate {
    template: String,
    title: String,
    kind: Kind,
    #[serde(default)]
    controls: Vec<RawTemplateControl>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTemplateControl {
    id: String,
    title: String,
    #[serde(default)]
    kind: Option<Kind>,
    #[serde(default)]
    maps_to: Vec<ControlRef>,
    #[serde(default)]
    remediation: Option<String>,
    #[serde(default)]
    evidence: Option<serde::de::IgnoredAny>,
}

// ======================================================== a scope's policy

/// One control in a scope's own policy. Picked from the policy's template by
/// `id`, it inherits the template's title, kind, `maps_to` and remediation
/// and states only this scope's evidence; declared by the policy itself (no
/// template, the finance shape), it carries its own title.
// `#278`: no `Eq` -- `evidence: Vec<Check>` can carry a `Check::Metric`,
// whose `above`/`below` are `Option<f64>`, and `f64` has no `Eq`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Control {
    /// `[a-z0-9][a-z0-9-]*`, referenced elsewhere as `<framework>/<id>`.
    pub id: String,
    /// Required on a control the policy declares itself; a picked template
    /// control takes the template's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Overrides the policy's own `kind` for this control alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Kind>,
    /// The default freshness window for this control's evidence, folded
    /// into the minimum with every check's own `max_age`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age: Option<Duration>,
    /// Equivalent controls in other frameworks of the same scope: evidence
    /// that satisfies this control also satisfies these, one hop, and the
    /// other way around.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub maps_to: Vec<ControlRef>,
    /// What the remediation task (`#83`) says closes it. On a picked
    /// control, this scope's own instructions replace the template's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
    #[serde(default)]
    pub evidence: Vec<Check>,
    /// The steps work of a given category must pass through for this
    /// control to hold (`#118`): folded into the scope's control plan by
    /// `control_plan::resolve`, injected at dispatch, and proved by an
    /// attestation before a run counts as `done`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<factory_assurance::control_plan::Requirement>,
    /// The control does not apply in this scope, and why. An empty
    /// rationale is a finding, and the control stays applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_applicable: Option<String>,
}

/// One scope's policy for one framework, exactly as authored at
/// `<scope>/.factory/policies/<framework>.yaml` (#390). It applies to that
/// scope only; nothing is inherited down the tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyFile {
    /// Must equal the file's stem -- checked by the loader, which alone
    /// knows the path a value came from.
    pub framework: String,
    /// The template this policy picks its controls from, by id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// Required without a template; with one, the template's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Required without a template; with one, the template's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Kind>,
    #[serde(default)]
    pub controls: Vec<Control>,
}

/// What `load_*` and [`resolve`] check for. A finding never stops another
/// file, or another control in the same file, from loading: a mistake in
/// one scope's policy is reported, not allowed to take the rest down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    ParseFailed,
    DuplicateControl,
    FrameworkMismatch,
    UnknownMapsTo,
    /// A scenario overlay's `add_frameworks` names no draft or template.
    UnknownFramework,
    /// A policy picks a control its template does not define, or a
    /// scenario overlay tightens one the scope does not have.
    UnknownControl,
    /// A scenario overlay's `tighten` does not shorten the window.
    LooseningHasNoEffect,
    EmptyRationale,
    /// A `task` or `workflow` check named something that matched more than
    /// one task's title, or more than one workflow's name, in the evaluated
    /// scope -- see [`evidence_findings`].
    AmbiguousCheckTarget,
    /// A `daemon` check named a `fact` outside [`KNOWN_DAEMON_FACTS`].
    UnknownDaemonFact,
    /// A `secrets` check's `absent` named a location outside
    /// [`KNOWN_SECRETS_LOCATIONS`].
    UnknownSecretsLocation,
    /// A `requires:` entry that cannot do what it says -- see
    /// `control_plan::Requirement::problems`.
    BadRequirement,
    /// A check that could never be satisfied as written (`#158`, `#278`).
    BadCheckTarget,
    /// `#278`: a `metric` check names `compliance.*`, `open_controls.*` or
    /// `quality.*` -- computed from this same evaluation's own output.
    CircularMetric,
    /// `#278`: a `metric` check names an id this build has never heard of.
    UnknownMetric,
    /// `#278`: a `metric` check names an id this build cannot compute yet.
    UnavailableMetric,
    /// #390: a policy names a `template:` no template file defines. Its
    /// controls still load, each under its own title (or its id).
    UnknownTemplate,
    /// #390: a template control carries `evidence:`. Only a scope's policy
    /// names evidence; the template's is ignored.
    TemplateEvidence,
    /// #390: a control picked from a template names no evidence. It stays
    /// in the policy, and open.
    NoEvidence,
    /// #390: a picked control sets `title`, `kind` or `maps_to`, which come
    /// from the template. The template's value holds.
    TemplateOverride,
    /// #390: a policy without a template, or one of its own controls, has
    /// no `title`/`kind`. A default is used and the finding says which.
    MissingField,
    /// #390: a root `policies:` block or a nested `scope.policies` -- the
    /// shape Factory no longer reads. The daemon starts; the finding names
    /// the migration.
    LegacyPolicies,
    /// #390: a policy file at the instance root when the root is not a
    /// configured scope, so no scope owns it.
    NoOwningScope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub kind: FindingKind,
    /// Where to go look: the file a loader read, relative to the instance
    /// root, or the scope an evaluation or a config block is about.
    pub subject: String,
    pub detail: String,
}

/// `<root>/.factory/policies`: the root scope's own policies, with the
/// `templates/` and `drafts/` directories beneath it.
pub fn policies_dir(root: &Path) -> PathBuf {
    root.join(".factory").join("policies")
}

/// `<root>/.factory/policies/templates`, the template directory (#390).
pub fn templates_dir(root: &Path) -> PathBuf {
    policies_dir(root).join("templates")
}

/// Whatever in `check` names something outside a fixed vocabulary -- a
/// `daemon` fact not in [`KNOWN_DAEMON_FACTS`], a `secrets` location not in
/// [`KNOWN_SECRETS_LOCATIONS`] -- as a finding kind and a detail that reads
/// on after the name of whatever holds the check (`<control> names daemon
/// fact ...`). Neither vocabulary depends on live evidence, so a caller can
/// catch the mistake when the file loads rather than only once a report is
/// evaluated: the policy loader, and `quality::load` for a quality
/// profile's check measures.
pub fn check_vocabulary(check: &Check) -> Vec<(FindingKind, String)> {
    use factory_assurance::checks::VocabularyFinding;
    factory_assurance::checks::check_vocabulary(check)
        .into_iter()
        .map(|(kind, detail)| {
            let kind = match kind {
                VocabularyFinding::UnknownDaemonFact => FindingKind::UnknownDaemonFact,
                VocabularyFinding::UnknownSecretsLocation => FindingKind::UnknownSecretsLocation,
                VocabularyFinding::BadCheckTarget => FindingKind::BadCheckTarget,
                VocabularyFinding::CircularMetric => FindingKind::CircularMetric,
                VocabularyFinding::UnknownMetric => FindingKind::UnknownMetric,
                VocabularyFinding::UnavailableMetric => FindingKind::UnavailableMetric,
            };
            (kind, detail)
        })
        .collect()
}

/// Every `*.yaml` directly in `dir`, sorted. Never recurses, so
/// `templates/` and `drafts/` beneath a policies directory are never read
/// as policies.
fn yaml_files(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(read) => read
            .flatten()
            .map(|entry| entry.path())
            .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("yaml"))
            .collect(),
        Err(_) => Vec::new(),
    };
    paths.sort();
    paths
}

fn file_name_and_stem(path: &Path) -> (String, String) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    (name, stem)
}

fn read_yaml<T: serde::de::DeserializeOwned>(path: &Path, subject: &str) -> Result<T, Finding> {
    let text = std::fs::read_to_string(path).map_err(|e| Finding {
        kind: FindingKind::ParseFailed,
        subject: subject.to_owned(),
        detail: format!("reading: {e}"),
    })?;
    serde_yaml_ng::from_str(&text).map_err(|e| Finding {
        kind: FindingKind::ParseFailed,
        subject: subject.to_owned(),
        detail: format!("parsing: {e}"),
    })
}

fn sort_findings(findings: &mut Vec<Finding>) {
    findings.sort_by(|a, b| {
        a.subject
            .cmp(&b.subject)
            .then(a.kind.cmp(&b.kind))
            .then(a.detail.cmp(&b.detail))
    });
    findings.dedup();
}

/// Every template in `dir` (`<root>/.factory/policies/templates`). A
/// missing directory is empty. A file that fails to parse, names a
/// `template` other than its own stem, or repeats a control id is a
/// finding; a control carrying `evidence:` is a finding and loads without
/// it. `subject_prefix` is prepended to each file name in a finding.
pub fn load_templates(dir: &Path, subject_prefix: &str) -> (Vec<Template>, Vec<Finding>) {
    let mut findings = Vec::new();
    let mut templates = Vec::new();
    for path in yaml_files(dir) {
        let (file_name, stem) = file_name_and_stem(&path);
        let subject = format!("{subject_prefix}{file_name}");
        let raw: RawTemplate = match read_yaml(&path, &subject) {
            Ok(raw) => raw,
            Err(finding) => {
                findings.push(finding);
                continue;
            }
        };
        if raw.template != stem {
            findings.push(Finding {
                kind: FindingKind::FrameworkMismatch,
                subject,
                detail: format!(
                    "template {:?} does not match the file name {stem:?}",
                    raw.template
                ),
            });
            continue;
        }
        let mut seen = BTreeSet::new();
        let mut controls = Vec::with_capacity(raw.controls.len());
        for control in raw.controls {
            if !seen.insert(control.id.clone()) {
                findings.push(Finding {
                    kind: FindingKind::DuplicateControl,
                    subject: subject.clone(),
                    detail: format!("duplicate control id {:?}", control.id),
                });
                continue;
            }
            if control.evidence.is_some() {
                findings.push(Finding {
                    kind: FindingKind::TemplateEvidence,
                    subject: subject.clone(),
                    detail: format!(
                        "{}/{} carries evidence, which a template never does: a scope's own policy names its \
                         evidence. Move it to the policy of each scope that picks the control",
                        raw.template, control.id
                    ),
                });
            }
            controls.push(TemplateControl {
                id: control.id,
                title: control.title,
                kind: control.kind,
                maps_to: control.maps_to,
                remediation: control.remediation,
            });
        }
        templates.push(Template {
            template: raw.template,
            title: raw.title,
            kind: raw.kind,
            controls,
        });
    }
    sort_findings(&mut findings);
    (templates, findings)
}

/// Read one policy file. A file that fails to parse, or names a `framework`
/// other than its stem, is a finding.
pub fn read_policy_file(path: &Path, subject: &str) -> Result<PolicyFile, Finding> {
    let (_, stem) = file_name_and_stem(path);
    let file: PolicyFile = read_yaml(path, subject)?;
    if file.framework != stem {
        return Err(Finding {
            kind: FindingKind::FrameworkMismatch,
            subject: subject.to_owned(),
            detail: format!(
                "framework {:?} does not match the file name {stem:?}",
                file.framework
            ),
        });
    }
    Ok(file)
}

/// A template control the scope's policy neither picked nor marked `n/a`.
/// Listed, never counted -- so a Statement of Applicability still accounts
/// for every control of the template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotSelected {
    pub control: ControlRef,
    pub title: String,
}

/// One scope's policy for one framework, resolved: every control it picked
/// or declared (`n/a` ones included), and every template control it left
/// unselected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OwnedPolicy {
    /// The scope that owns it -- the only scope it applies to.
    pub scope: String,
    pub framework: String,
    pub title: String,
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// Where it was read from, relative to the instance root.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file: String,
    pub controls: Vec<Applied>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not_selected: Vec<NotSelected>,
}

/// Which scope declared a control not applicable, and why -- always the
/// scope that owns the policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedNotApplicable {
    pub scope: String,
    pub rationale: String,
}

/// One control as it applies in the scope that owns it: its title, kind and
/// `maps_to` (the template's, for a picked control), the scope's own
/// evidence, the freshness window, and its `n/a` state.
// `#278`: no `Eq`, for the same reason as `Control` -- `evidence: Vec<Check>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Applied {
    pub control: ControlRef,
    pub title: String,
    pub kind: Kind,
    pub maps_to: Vec<ControlRef>,
    /// Exactly as the scope's policy wrote it.
    pub evidence: Vec<Check>,
    /// The minimum of the control's own `max_age` and every one of its
    /// checks' own `max_age` -- `None` when none sets one. This, not a
    /// `Check` variant's own field, is the window evidence is checked
    /// against.
    pub max_age: Option<Duration>,
    pub not_applicable: Option<AppliedNotApplicable>,
    /// The scope's own `remediation:`, or the template's for a picked
    /// control that has none. `Engine::policy_remediate` (`#83`) reads it.
    pub remediation: Option<String>,
    /// The scope's own `requires:` for this control. `control_plan::resolve`
    /// reads it, and a control `n/a` here waives it there, by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<factory_assurance::control_plan::Requirement>,
}

fn min_duration(a: Option<Duration>, b: Option<Duration>) -> Option<Duration> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

/// What `file` commits `scope` to (#390). Pure: the template list is handed
/// in, and every mistake is a finding against `subject`, never a refusal.
///
/// - With a `template:`, a control picks the template control of the same
///   id and inherits its title, kind, `maps_to` and remediation; the
///   scope's own `remediation:` replaces the template's. An id the template
///   does not define is a finding and is skipped. A picked control with no
///   evidence is a finding and stays open. Every template control neither
///   picked nor marked `n/a` is listed as not selected.
/// - An unknown template is a finding; the controls still load, each under
///   its own title, or its id when it has none.
/// - Without a template, every control is the policy's own and needs a
///   `title`.
/// - `not_applicable` needs a non-empty rationale; an empty one is a
///   finding and the control stays applicable.
pub fn resolve(
    file: &PolicyFile,
    scope: &str,
    subject: &str,
    templates: &[Template],
) -> (OwnedPolicy, Vec<Finding>) {
    let mut findings = Vec::new();
    let mut finding = |kind, detail: String| {
        findings.push(Finding {
            kind,
            subject: subject.to_owned(),
            detail,
        })
    };
    let framework = &file.framework;
    let template = match &file.template {
        Some(name) => {
            let found = templates.iter().find(|t| &t.template == name);
            if found.is_none() {
                finding(
                    FindingKind::UnknownTemplate,
                    format!(
                        "{framework} names template {name:?}, which no file in .factory/policies/templates/ defines; \
                         its controls load under their own titles"
                    ),
                );
            }
            found
        }
        None => None,
    };
    let title = match (&file.title, template) {
        (Some(title), _) => title.clone(),
        (None, Some(t)) => t.title.clone(),
        (None, None) => {
            // An unknown template already has its own finding.
            if file.template.is_none() {
                finding(
                    FindingKind::MissingField,
                    format!("{framework} has no `title` and no template to take one from; its name is used"),
                );
            }
            framework.clone()
        }
    };
    let kind = match (file.kind, template) {
        (Some(kind), _) => kind,
        (None, Some(t)) => t.kind,
        (None, None) => {
            if file.template.is_none() {
                finding(
                    FindingKind::MissingField,
                    format!(
                        "{framework} has no `kind` and no template to take one from; it counts as a regulation"
                    ),
                );
            }
            Kind::Regulation
        }
    };

    let mut seen = BTreeSet::new();
    let mut controls = Vec::with_capacity(file.controls.len());
    for control in &file.controls {
        let control_ref = ControlRef::new(framework.clone(), control.id.clone());
        if !seen.insert(control.id.clone()) {
            finding(
                FindingKind::DuplicateControl,
                format!("duplicate control id {:?}", control.id),
            );
            continue;
        }
        let picked = match template {
            Some(t) => match t.controls.iter().find(|c| c.id == control.id) {
                Some(picked) => Some((t, picked)),
                None => {
                    finding(
                        FindingKind::UnknownControl,
                        format!(
                            "{control_ref} is not a control of template {:?}; it is not loaded",
                            t.template
                        ),
                    );
                    continue;
                }
            },
            None => None,
        };
        let not_applicable = match control.not_applicable.as_deref() {
            Some(rationale) if rationale.trim().is_empty() => {
                finding(
                    FindingKind::EmptyRationale,
                    format!("{control_ref} marked not applicable with no rationale; it stays applicable"),
                );
                None
            }
            Some(rationale) => Some(AppliedNotApplicable {
                scope: scope.to_owned(),
                rationale: rationale.to_owned(),
            }),
            None => None,
        };
        let (title, kind, maps_to, remediation) = match picked {
            Some((t, picked)) => {
                let overridden: Vec<&str> = [
                    ("title", control.title.is_some()),
                    ("kind", control.kind.is_some()),
                    ("maps_to", !control.maps_to.is_empty()),
                ]
                .into_iter()
                .filter_map(|(field, set)| set.then_some(field))
                .collect();
                if !overridden.is_empty() {
                    finding(
                        FindingKind::TemplateOverride,
                        format!(
                            "{control_ref} sets {}, which come(s) from template {:?}; the template's value holds",
                            overridden.join(", "),
                            t.template
                        ),
                    );
                }
                if control.evidence.is_empty() && not_applicable.is_none() {
                    finding(
                        FindingKind::NoEvidence,
                        format!(
                            "{control_ref} is picked from template {:?} with no evidence; it stays open until \
                             this scope names its evidence",
                            t.template
                        ),
                    );
                }
                (
                    picked.title.clone(),
                    picked.kind.unwrap_or(t.kind),
                    picked.maps_to.clone(),
                    control.remediation.clone().or_else(|| picked.remediation.clone()),
                )
            }
            None => {
                let title = match &control.title {
                    Some(title) => title.clone(),
                    None => {
                        if file.template.is_none() {
                            finding(
                                FindingKind::MissingField,
                                format!("{control_ref} has no `title`; its id is used"),
                            );
                        }
                        control.id.clone()
                    }
                };
                (
                    title,
                    control.kind.unwrap_or(kind),
                    control.maps_to.clone(),
                    control.remediation.clone(),
                )
            }
        };
        for check in &control.evidence {
            for (kind, detail) in check_vocabulary(check) {
                finding(kind, format!("{control_ref} {detail}"));
            }
        }
        for requirement in &control.requires {
            for detail in requirement.problems() {
                finding(
                    FindingKind::BadRequirement,
                    format!("{control_ref} {detail}"),
                );
            }
        }
        let max_age = control.evidence.iter().fold(control.max_age, |acc, check| {
            min_duration(acc, check.own_max_age())
        });
        controls.push(Applied {
            control: control_ref,
            title,
            kind,
            maps_to,
            evidence: control.evidence.clone(),
            max_age,
            not_applicable,
            remediation,
            requires: control.requires.clone(),
        });
    }
    controls.sort_by(|a, b| a.control.cmp(&b.control));
    let not_selected = template
        .map(|t| {
            t.controls
                .iter()
                .filter(|c| !seen.contains(&c.id))
                .map(|c| NotSelected {
                    control: ControlRef::new(framework.clone(), c.id.clone()),
                    title: c.title.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    (
        OwnedPolicy {
            scope: scope.to_owned(),
            framework: framework.clone(),
            title,
            kind,
            template: file.template.clone(),
            file: subject.to_owned(),
            controls,
            not_selected,
        },
        findings,
    )
}

/// Every policy in one directory -- one scope's `.factory/policies/`,
/// owned by `scope` -- resolved against `templates`. A missing directory is
/// empty; one bad file never stops the others. `subject_prefix` is
/// prepended to each file name in a finding and in `OwnedPolicy::file`.
/// `maps_to` is checked by the caller, once every scope has loaded.
pub fn load_dir(
    dir: &Path,
    subject_prefix: &str,
    scope: &str,
    templates: &[Template],
) -> (Vec<OwnedPolicy>, Vec<Finding>) {
    let mut findings = Vec::new();
    let mut policies = Vec::new();
    for path in yaml_files(dir) {
        let (file_name, _) = file_name_and_stem(&path);
        let subject = format!("{subject_prefix}{file_name}");
        match read_policy_file(&path, &subject) {
            Ok(file) => {
                let (policy, own) = resolve(&file, scope, &subject, templates);
                findings.extend(own);
                policies.push(policy);
            }
            Err(finding) => findings.push(finding),
        }
    }
    (policies, findings)
}

/// One directory's policies with no templates and its own `maps_to` checked
/// against itself: the single-directory view a test, the backup verifier
/// and the draft loader read.
pub fn load_all(dir: &Path) -> (Vec<OwnedPolicy>, Vec<Finding>) {
    let (policies, mut findings) = load_dir(dir, "", "", &[]);
    findings.extend(maps_to_findings(&[], &policies));
    sort_findings(&mut findings);
    (policies, findings)
}

/// Every `maps_to` that names a control no loaded template or policy
/// defines. Checked once everything has loaded: a target in a file that
/// failed to parse is exactly the "naming nothing known" case.
pub fn maps_to_findings(templates: &[Template], policies: &[OwnedPolicy]) -> Vec<Finding> {
    let mut known: BTreeSet<ControlRef> = templates
        .iter()
        .flat_map(|t| {
            t.controls
                .iter()
                .map(|c| ControlRef::new(t.template.clone(), c.id.clone()))
        })
        .collect();
    for policy in policies {
        known.extend(policy.controls.iter().map(|c| c.control.clone()));
        known.extend(policy.not_selected.iter().map(|c| c.control.clone()));
    }
    let mut findings = Vec::new();
    for t in templates {
        for c in &t.controls {
            for target in c.maps_to.iter().filter(|target| !known.contains(target)) {
                findings.push(Finding {
                    kind: FindingKind::UnknownMapsTo,
                    subject: format!(".factory/policies/templates/{}.yaml", t.template),
                    detail: format!(
                        "{}/{} maps_to {target}, which is not a control in any loaded template or policy",
                        t.template, c.id
                    ),
                });
            }
        }
    }
    for policy in policies {
        for applied in &policy.controls {
            for target in applied.maps_to.iter().filter(|target| !known.contains(target)) {
                findings.push(Finding {
                    kind: FindingKind::UnknownMapsTo,
                    subject: policy.file.clone(),
                    detail: format!(
                        "{} maps_to {target}, which is not a control in any loaded template or policy",
                        applied.control
                    ),
                });
            }
        }
    }
    findings
}

/// Every policy the instance has, read fresh: the templates, then each
/// scope's own `.factory/policies/` (#390). `scopes` is `(name, path)`, the
/// path relative to `root` (or absolute). The root scope's own directory is
/// `<root>/.factory/policies`; a policy file there when no configured scope
/// sits at the root is owned by nobody and is a finding.
#[derive(Debug, Clone, Default)]
pub struct Policies {
    pub templates: Vec<Template>,
    /// Every scope's policies, each knowing its owner, in scope order then
    /// framework order.
    pub owned: Vec<OwnedPolicy>,
    pub findings: Vec<Finding>,
}

impl Policies {
    /// `scope`'s own policies -- nothing from an ancestor or a descendant.
    pub fn of<'a>(&'a self, scope: &'a str) -> impl Iterator<Item = &'a OwnedPolicy> + 'a {
        self.owned.iter().filter(move |p| p.scope == scope)
    }

    /// Every control `scope`'s own policies commit it to, `n/a` included,
    /// sorted by control.
    pub fn applied(&self, scope: &str) -> Vec<Applied> {
        let mut applied: Vec<Applied> = self
            .of(scope)
            .flat_map(|p| p.controls.iter().cloned())
            .collect();
        applied.sort_by(|a, b| a.control.cmp(&b.control));
        applied
    }

    /// The frameworks `scope` has a policy for, sorted.
    pub fn frameworks(&self, scope: &str) -> Vec<String> {
        self.of(scope)
            .map(|p| p.framework.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn template(&self, name: &str) -> Option<&Template> {
        self.templates.iter().find(|t| t.template == name)
    }
}

/// A scope path relative to the instance root, `/`-separated, with `.`
/// components dropped -- empty for the root itself.
fn relative_label(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative
        .components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// `<scope>/.factory/policies`, for a scope path relative to `root`.
pub fn scope_policies_dir(root: &Path, scope_path: &Path) -> PathBuf {
    policies_dir(&root.join(scope_path))
}

/// The frameworks a scope has a policy file for -- the stems of the
/// `*.yaml` directly in its `.factory/policies/`, sorted. What the agent
/// guide names (`AgentContext::factory_guide`): a plain listing, never a
/// parse, so a dispatch never pays for loading a policy.
pub fn policy_frameworks(root: &Path, scope_path: &Path) -> Vec<String> {
    yaml_files(&scope_policies_dir(root, scope_path))
        .iter()
        .map(|path| file_name_and_stem(path).1)
        .collect()
}

pub fn load(root: &Path, scopes: &[(String, PathBuf)]) -> Policies {
    let (templates, mut findings) = load_templates(
        &templates_dir(root),
        ".factory/policies/templates/",
    );
    let mut owned = Vec::new();
    let mut seen_dirs: BTreeSet<String> = BTreeSet::new();
    for (name, path) in scopes {
        let label = relative_label(root, &root.join(path));
        if !seen_dirs.insert(label.clone()) {
            continue;
        }
        let prefix = if label.is_empty() {
            ".factory/policies/".to_owned()
        } else {
            format!("{label}/.factory/policies/")
        };
        let (policies, own) = load_dir(&scope_policies_dir(root, path), &prefix, name, &templates);
        owned.extend(policies);
        findings.extend(own);
    }
    if !seen_dirs.contains("") {
        for path in yaml_files(&policies_dir(root)) {
            let (file_name, _) = file_name_and_stem(&path);
            findings.push(Finding {
                kind: FindingKind::NoOwningScope,
                subject: format!(".factory/policies/{file_name}"),
                detail: "a policy at the instance root belongs to the root scope, and the root is not a \
                         configured scope: give the root config a `scope:` block, or move the file into \
                         the `.factory/policies/` of the scope it is for"
                    .to_owned(),
            });
        }
    }
    findings.extend(maps_to_findings(&templates, &owned));
    sort_findings(&mut findings);
    Policies {
        templates,
        owned,
        findings,
    }
}

/// A control's `max_age` narrowed by a scenario overlay (`scenario.rs`).
/// Never widens it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tighten {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age: Option<Duration>,
}

/// A skeleton policy for `scope` picking every control of `template`,
/// printed for a person to fill in (`factory policy template skeleton`) --
/// Factory never writes authored content itself. Each control is left with
/// an empty `evidence: []` to translate into the scope's own terms.
pub fn skeleton(template: &Template, framework: Option<&str>) -> String {
    let mut out = format!(
        "# Save as <scope>/.factory/policies/{}.yaml, then give every control this scope's own evidence,\n\
         # mark it `not_applicable: \"<rationale>\"`, or delete it (it is then listed as not selected).\n\
         framework: {}\ntemplate: {}\ncontrols:\n",
        framework.unwrap_or(&template.template),
        framework.unwrap_or(&template.template),
        template.template
    );
    for control in &template.controls {
        out.push_str(&format!(
            "  - id: {}            # {}\n    evidence: []\n",
            control.id, control.title
        ));
    }
    out
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

/// L6 decides which authored classifications count toward compliance. L5
/// receives that plain counting instruction, never an upper-level rollup.
pub fn metric_subject(applied: &Applied) -> factory_assurance::checks::EvaluationSubject<bool> {
    let factory_assurance::checks::EvaluationSubject {
        control,
        title,
        kind,
        maps_to,
        evidence,
        max_age,
        not_applicable,
    } = evaluation_subject(applied);
    factory_assurance::checks::EvaluationSubject {
        control,
        title,
        kind: kind != Kind::BestPractice,
        maps_to,
        evidence,
        max_age,
        not_applicable,
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

/// Lower command input, projected by the declaration owner. L5 remains the
/// sole compiler; no L4 lifecycle or execution-plan API enters L6.
pub fn plan_sources(policy: &[Applied]) -> Vec<factory_assurance::control_plan::PlanSource> {
    use factory_assurance::control_plan::{PlanSource, PlanWaiver};
    policy
        .iter()
        .map(|applied| PlanSource {
            source: applied.control.to_string(),
            requires: applied.requires.clone(),
            not_applicable: applied.not_applicable.as_ref().map(|na| PlanWaiver {
                scope: na.scope.clone(),
                rationale: na.rationale.clone(),
            }),
        })
        .collect()
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
pub fn remediation_instructions(
    control: &ControlRef,
    remediation: Option<&str>,
    status: &Status,
    checks: &[Check],
) -> String {
    let mut out = String::new();
    if let Some(r) = remediation.map(str::trim).filter(|r| !r.is_empty()) {
        out.push_str(r);
        out.push_str("\n\n");
    }
    out.push_str("Missing evidence:\n");
    for reason in status.reasons() {
        out.push_str(&format!("- {reason}\n"));
    }
    out.push_str(&format!(
        "\nAny one of these checks passing closes {control}:\n"
    ));
    for check in checks {
        out.push_str(&format!("- {}\n", check.describe()));
    }
    out.trim_end().to_string()
}

// ================================================================= rollup

pub use factory_assurance::evaluation_rollup::StatusCounts;

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

pub use factory_assurance::evaluation_rollup::worst_across_scopes;

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), text).unwrap();
    }

    fn tempdir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "factory-policy-test-{label}-{}",
            uuid::Uuid::new_v4()
        ))
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
        assert!(check_vocabulary(&Check::Daemon {
            fact: "power_assertion".into()
        })
        .is_empty());
        let found = check_vocabulary(&Check::Daemon {
            fact: "power_asertion".into(),
        });
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, FindingKind::UnknownDaemonFact);
        // #154: the three backup facts are known, and a misspelling of one
        // is caught the same way, at load time rather than only once
        // evaluated.
        for fact in ["backup_recent", "backup_offsite", "backup_verified"] {
            assert!(
                check_vocabulary(&Check::Daemon { fact: fact.into() }).is_empty(),
                "{fact}"
            );
        }
        let found = check_vocabulary(&Check::Daemon {
            fact: "backup_verfied".into(),
        });
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, FindingKind::UnknownDaemonFact);
        let found = check_vocabulary(&Check::Secrets {
            absent: vec!["github".into(), "gitlab".into(), "nope".into()],
        });
        assert_eq!(
            found.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
            vec![
                FindingKind::UnknownSecretsLocation,
                FindingKind::UnknownSecretsLocation
            ]
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
        let hyphenated: PolicyFile = serde_yaml_ng::from_str(
            "framework: house\ntitle: House rules\nkind: best-practice\ncontrols: []\n",
        )
        .unwrap();
        assert_eq!(hyphenated.kind, Some(Kind::BestPractice));

        let aliased: PolicyFile = serde_yaml_ng::from_str(
            "framework: house\ntitle: House rules\nkind: best_practice\ncontrols: []\n",
        )
        .unwrap();
        assert_eq!(aliased.kind, Some(Kind::BestPractice));

        assert_eq!(
            serde_json::to_string(&Kind::BestPractice).unwrap(),
            "\"best-practice\""
        );
    }

    #[test]
    fn the_shipped_templates_and_example_scope_policy_load_with_no_findings() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/policies");
        let (templates, mut findings) = load_templates(&dir.join("templates"), "templates/");
        assert_eq!(
            templates.iter().map(|t| t.template.as_str()).collect::<Vec<_>>(),
            ["ai-act", "cra", "dsgvo", "iso27001"]
        );
        let (policies, own) = load_dir(&dir, "", "demo", &templates);
        findings.extend(own);
        findings.extend(maps_to_findings(&templates, &policies));
        assert_eq!(findings, Vec::new(), "{findings:?}");
        assert_eq!(policies.len(), 1);
        let cra = &policies[0];
        assert_eq!((cra.framework.as_str(), cra.template.as_deref()), ("cra", Some("cra")));
        assert!(cra.controls.len() >= 6, "{:?}", cra.controls);
        assert!(cra.controls.iter().any(|c| c.not_applicable.is_some()));
        assert!(!cra.not_selected.is_empty(), "the example leaves some controls unselected");
        // The draft resolves against the same templates with no finding.
        let draft = read_policy_file(&dir.join("drafts/ai-act.yaml"), "drafts/ai-act.yaml").unwrap();
        let (owned, draft_findings) = resolve(&draft, "demo", "drafts/ai-act.yaml", &templates);
        assert!(draft_findings.is_empty(), "{draft_findings:?}");
        assert_eq!(owned.controls.len(), 5);
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
        assert!(
            findings[0].detail.contains("launches_rockets"),
            "{}",
            findings[0].detail
        );
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
        assert!(
            findings[0].detail.contains("under-the-mat"),
            "{}",
            findings[0].detail
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_bare_check_secrets_still_parses_and_round_trips_with_no_absent_field() {
        let check: Check = serde_yaml_ng::from_str("check: secrets\n").unwrap();
        assert_eq!(check, Check::Secrets { absent: Vec::new() });
        let json = serde_json::to_string(&check).unwrap();
        assert_eq!(
            json, "{\"check\":\"secrets\"}",
            "an empty `absent` is not written out"
        );
    }

    // --------------------------------------------------------- check: metric (#278)

    #[test]
    fn a_check_metric_parses_with_above_below_and_max_age_and_round_trips() {
        let check: Check = serde_yaml_ng::from_str(
            "check: metric\nmetric: reported.finance.beleg_coverage\nabove: 0.98\nmax_age: 30d\n",
        )
        .unwrap();
        assert_eq!(check.kind_name(), "metric");
        assert_eq!(check.own_max_age(), Some("30d".parse().unwrap()));
        let json = serde_json::to_string(&check).unwrap();
        assert_eq!(
            json,
            "{\"check\":\"metric\",\"metric\":\"reported.finance.beleg_coverage\",\"above\":0.98,\"max_age\":\"30d\"}"
        );
    }

    #[test]
    fn a_metric_check_naming_compliance_open_controls_or_quality_is_a_circular_finding_at_parse_time()
    {
        for metric in ["compliance.cra", "open_controls.cra", "quality.reliability"] {
            let dir = tempdir(&format!("circular-metric-{}", metric.replace('.', "-")));
            write(
                &dir,
                "cra.yaml",
                &format!(
                    "framework: cra\ntitle: CRA\nkind: regulation\ncontrols:\n\
                     \x20\x20- id: a\n\x20\x20\x20\x20title: A\n\x20\x20\x20\x20evidence:\n\
                     \x20\x20\x20\x20\x20\x20- check: metric\n\x20\x20\x20\x20\x20\x20\x20\x20metric: {metric}\n\x20\x20\x20\x20\x20\x20\x20\x20above: 0.5\n"
                ),
            );
            let (catalogues, findings) = load_all(&dir);
            assert_eq!(
                catalogues[0].controls.len(),
                1,
                "{metric}: the control still loads"
            );
            assert_eq!(findings.len(), 1, "{metric}: {findings:?}");
            assert_eq!(findings[0].kind, FindingKind::CircularMetric, "{metric}");
            assert!(
                findings[0].detail.contains(metric),
                "{metric}: {}",
                findings[0].detail
            );
            std::fs::remove_dir_all(&dir).ok();
        }
    }

    #[test]
    fn a_metric_check_naming_an_unknown_metric_is_a_finding_at_parse_time() {
        let dir = tempdir("unknown-metric");
        write(
            &dir,
            "cra.yaml",
            "framework: cra\ntitle: CRA\nkind: regulation\ncontrols:\n\
             \x20\x20- id: a\n\x20\x20\x20\x20title: A\n\x20\x20\x20\x20evidence:\n\
             \x20\x20\x20\x20\x20\x20- check: metric\n\x20\x20\x20\x20\x20\x20\x20\x20metric: not_a_real_metric\n\x20\x20\x20\x20\x20\x20\x20\x20above: 0.5\n",
        );
        let (catalogues, findings) = load_all(&dir);
        assert_eq!(catalogues[0].controls.len(), 1, "the control still loads");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].kind, FindingKind::UnknownMetric);
        assert!(
            findings[0].detail.contains("not_a_real_metric"),
            "{}",
            findings[0].detail
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_metric_check_with_neither_above_nor_below_is_a_finding_at_parse_time() {
        let dir = tempdir("bound-less-metric");
        write(
            &dir,
            "cra.yaml",
            "framework: cra\ntitle: CRA\nkind: regulation\ncontrols:\n\
             \x20\x20- id: a\n\x20\x20\x20\x20title: A\n\x20\x20\x20\x20evidence:\n\
             \x20\x20\x20\x20\x20\x20- check: metric\n\x20\x20\x20\x20\x20\x20\x20\x20metric: fail_rate\n",
        );
        let (catalogues, findings) = load_all(&dir);
        assert_eq!(catalogues[0].controls.len(), 1, "the control still loads");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].kind, FindingKind::BadCheckTarget);
        std::fs::remove_dir_all(&dir).ok();
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

    // -- templates and resolve (#390) ---------------------------------------

    const TEMPLATE: &str = "template: cra\ntitle: Cyber Resilience Act\nkind: regulation\ncontrols:\n\
        \x20 - {id: sbom, title: SBOM, maps_to: [iso27001/a-8-8], remediation: Generic guidance}\n\
        \x20 - {id: vulns, title: Vulnerabilities, kind: best-practice}\n\
        \x20 - {id: hw, title: Hardware}\n\
        \x20 - {id: left, title: Left out}\n";

    fn templates() -> Vec<Template> {
        let dir = tempdir("templates");
        write(&dir, "cra.yaml", TEMPLATE);
        let (templates, findings) = load_templates(&dir, "");
        assert!(findings.is_empty(), "{findings:?}");
        std::fs::remove_dir_all(&dir).ok();
        templates
    }

    fn file(raw: &str) -> PolicyFile {
        serde_yaml_ng::from_str(raw).unwrap()
    }

    fn kinds(findings: &[Finding]) -> Vec<FindingKind> {
        findings.iter().map(|f| f.kind).collect()
    }

    #[test]
    fn a_picked_control_inherits_from_the_template_and_its_own_remediation_wins() {
        let policy = file(
            "framework: cra\ntemplate: cra\ncontrols:\n\
             \x20 - {id: sbom, evidence: [{check: task, task: sbom-export, max_age: 30d}]}\n\
             \x20 - {id: vulns, remediation: Run the scanner, evidence: [{check: attestation}]}\n",
        );
        let (owned, findings) = resolve(&policy, "irrlicht", "cra.yaml", &templates());
        assert!(findings.is_empty(), "{findings:?}");
        assert_eq!((owned.title.as_str(), owned.kind), ("Cyber Resilience Act", Kind::Regulation));
        assert_eq!(owned.template.as_deref(), Some("cra"));
        let sbom = &owned.controls[0];
        assert_eq!(sbom.control, ControlRef::new("cra", "sbom"));
        assert_eq!(sbom.title, "SBOM");
        assert_eq!(sbom.kind, Kind::Regulation);
        assert_eq!(sbom.maps_to, [ControlRef::new("iso27001", "a-8-8")]);
        assert_eq!(sbom.remediation.as_deref(), Some("Generic guidance"));
        assert_eq!(sbom.max_age, Some("30d".parse().unwrap()));
        let vulns = &owned.controls[1];
        assert_eq!(vulns.kind, Kind::BestPractice, "the template control's own kind");
        assert_eq!(vulns.remediation.as_deref(), Some("Run the scanner"), "the scope's own wins");
        assert_eq!(
            owned.not_selected.iter().map(|c| c.control.to_string()).collect::<Vec<_>>(),
            ["cra/hw", "cra/left"],
            "every template control neither picked nor n/a, never a control"
        );
        assert_eq!(owned.not_selected[0].title, "Hardware");
    }

    #[test]
    fn two_scopes_bind_the_same_template_control_to_their_own_evidence() {
        let templates = templates();
        let a = resolve(
            &file("framework: cra\ntemplate: cra\ncontrols: [{id: sbom, evidence: [{check: task, task: irrlicht-sbom}]}]\n"),
            "irrlicht",
            "a",
            &templates,
        )
        .0;
        let b = resolve(
            &file("framework: cra\ntemplate: cra\ncontrols: [{id: sbom, evidence: [{check: task, task: factory-sbom}]}]\n"),
            "factory",
            "b",
            &templates,
        )
        .0;
        assert_eq!(a.controls[0].control, b.controls[0].control);
        assert_ne!(a.controls[0].evidence, b.controls[0].evidence);
        assert_eq!((a.scope.as_str(), b.scope.as_str()), ("irrlicht", "factory"));
    }

    #[test]
    fn an_unknown_template_or_template_control_is_a_finding() {
        let unknown = file("framework: cra\ntemplate: crx\ncontrols: [{id: sbom, evidence: [{check: attestation}]}]\n");
        let (owned, findings) = resolve(&unknown, "s", "cra.yaml", &templates());
        assert_eq!(kinds(&findings), [FindingKind::UnknownTemplate]);
        assert_eq!(owned.controls.len(), 1, "its controls still load");
        assert_eq!(owned.controls[0].title, "sbom", "under its id, lacking a title");
        assert_eq!(owned.kind, Kind::Regulation);

        let typo = file("framework: cra\ntemplate: cra\ncontrols: [{id: sbomm, evidence: [{check: attestation}]}]\n");
        let (owned, findings) = resolve(&typo, "s", "cra.yaml", &templates());
        assert_eq!(kinds(&findings), [FindingKind::UnknownControl]);
        assert!(findings[0].detail.contains("cra/sbomm"), "{findings:?}");
        assert!(owned.controls.is_empty());
        assert_eq!(owned.not_selected.len(), 4, "the real control is not selected");
    }

    #[test]
    fn a_picked_control_with_no_evidence_is_a_finding_and_stays_open() {
        let policy = file("framework: cra\ntemplate: cra\ncontrols: [{id: sbom}]\n");
        let (owned, findings) = resolve(&policy, "s", "cra.yaml", &templates());
        assert_eq!(kinds(&findings), [FindingKind::NoEvidence]);
        assert_eq!(owned.controls.len(), 1, "kept, so it is counted");
        let statuses = evaluate(&owned.controls, &Evidence::default(), Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
    }

    #[test]
    fn not_applicable_needs_a_rationale_and_names_the_owning_scope() {
        let policy = file(
            "framework: cra\ntemplate: cra\ncontrols:\n\
             \x20 - {id: hw, not_applicable: No hardware ships}\n\
             \x20 - {id: sbom, not_applicable: '  ', evidence: [{check: attestation}]}\n",
        );
        let (owned, findings) = resolve(&policy, "irrlicht", "cra.yaml", &templates());
        assert_eq!(kinds(&findings), [FindingKind::EmptyRationale]);
        let hw = owned.controls.iter().find(|c| c.control.id == "hw").unwrap();
        assert_eq!(
            hw.not_applicable,
            Some(AppliedNotApplicable {
                scope: "irrlicht".into(),
                rationale: "No hardware ships".into()
            })
        );
        let sbom = owned.controls.iter().find(|c| c.control.id == "sbom").unwrap();
        assert!(sbom.not_applicable.is_none(), "an empty rationale leaves it applicable");
        assert!(
            !owned.not_selected.iter().any(|c| c.control.id == "hw"),
            "an n/a control is accounted for, not unselected"
        );
    }

    #[test]
    fn a_policy_without_a_template_declares_its_own_controls() {
        let finance = file(
            "framework: finance\ntitle: Finance\nkind: best-practice\ncontrols:\n\
             \x20 - id: belege\n    title: Every booking has its receipt\n    kind: regulation\n    max_age: 35d\n\
             \x20   evidence: [{check: metric, metric: reported.finance.bookings_without_beleg, below: 0, max_age: 40d}]\n\
             \x20 - {id: close, title: Monthly close, evidence: [{check: task, task: Monatsabschluss Finanzen}]}\n",
        );
        let (owned, findings) = resolve(&finance, "finance", "finance.yaml", &[]);
        assert!(findings.is_empty(), "{findings:?}");
        assert_eq!((owned.title.as_str(), owned.kind), ("Finance", Kind::BestPractice));
        assert!(owned.not_selected.is_empty());
        let belege = &owned.controls[0];
        assert_eq!(belege.title, "Every booking has its receipt");
        assert_eq!(belege.kind, Kind::Regulation);
        assert_eq!(belege.max_age, Some("35d".parse().unwrap()), "the minimum of control and checks");
        assert_eq!(owned.controls[1].kind, Kind::BestPractice, "the policy's own kind");

        let bare = file("framework: house\ncontrols: [{id: a, evidence: [{check: attestation}]}]\n");
        let (owned, findings) = resolve(&bare, "s", "house.yaml", &[]);
        assert_eq!(kinds(&findings), [FindingKind::MissingField; 3]);
        assert_eq!((owned.title.as_str(), owned.kind), ("house", Kind::Regulation));
        assert_eq!(owned.controls[0].title, "a");
    }

    #[test]
    fn a_picked_control_overriding_template_fields_is_a_finding_and_the_template_holds() {
        let policy = file(
            "framework: cra\ntemplate: cra\ncontrols:\n\
             \x20 - {id: sbom, title: Mine, kind: best-practice, evidence: [{check: attestation}]}\n",
        );
        let (owned, findings) = resolve(&policy, "s", "cra.yaml", &templates());
        assert_eq!(kinds(&findings), [FindingKind::TemplateOverride]);
        assert!(findings[0].detail.contains("title, kind"), "{findings:?}");
        assert_eq!(owned.controls[0].title, "SBOM");
        assert_eq!(owned.controls[0].kind, Kind::Regulation);
    }

    #[test]
    fn a_template_with_evidence_is_a_finding_and_loads_without_it() {
        let dir = tempdir("template-evidence");
        write(
            &dir,
            "cra.yaml",
            "template: cra\ntitle: CRA\nkind: regulation\ncontrols:\n\
             \x20 - {id: sbom, title: SBOM, evidence: [{check: task, task: sbom-export}]}\n",
        );
        write(&dir, "iso.yaml", "template: iso27001\ntitle: ISO\nkind: standard\n");
        let (templates, findings) = load_templates(&dir, ".factory/policies/templates/");
        assert_eq!(
            kinds(&findings),
            [FindingKind::TemplateEvidence, FindingKind::FrameworkMismatch]
        );
        assert_eq!(findings[0].subject, ".factory/policies/templates/cra.yaml");
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].controls[0].id, "sbom");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn templates_and_drafts_beneath_a_policies_dir_are_never_policies() {
        let dir = tempdir("subdirs");
        write(&dir, "house.yaml", "framework: house\ntitle: House\nkind: best-practice\n");
        write(&dir.join("templates"), "cra.yaml", TEMPLATE);
        write(&dir.join("drafts"), "next.yaml", "framework: next\ntitle: Next\nkind: regulation\n");
        let (policies, findings) = load_all(&dir);
        assert!(findings.is_empty(), "{findings:?}");
        assert_eq!(
            policies.iter().map(|p| p.framework.as_str()).collect::<Vec<_>>(),
            ["house"]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_skeleton_picks_every_template_control_and_parses_as_a_policy() {
        let templates = templates();
        let text = skeleton(&templates[0], None);
        let parsed = file(&text);
        assert_eq!(parsed.template.as_deref(), Some("cra"));
        assert_eq!(parsed.controls.len(), 4);
        let (owned, findings) = resolve(&parsed, "s", "cra.yaml", &templates);
        assert_eq!(kinds(&findings), [FindingKind::NoEvidence; 4], "every control still needs its evidence");
        assert!(owned.not_selected.is_empty());
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
            vec![Check::Roles {
                forbid: vec![Grant::KnowledgeWrite],
            }],
            Vec::new(),
        )];
        let statuses = evaluate(&applied, &Evidence::default(), Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(
            statuses[0]
                .status
                .reasons()
                .iter()
                .any(|r| r.contains("not resolved")),
            "{:?}",
            statuses[0].status
        );
    }

    #[test]
    fn roles_with_no_agent_declared_is_satisfied() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Roles {
                forbid: vec![Grant::KnowledgeWrite],
            }],
            Vec::new(),
        )];
        let evidence = Evidence {
            agents: Some(Vec::new()),
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Satisfied);
    }

    #[test]
    fn roles_open_when_an_agents_role_holds_a_forbidden_grant() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Roles {
                forbid: vec![Grant::KnowledgeWrite],
            }],
            Vec::new(),
        )];
        let evidence = Evidence {
            agents: Some(vec![
                agent("worker", "worker", &[Grant::TaskEdit]),
                agent(
                    "foreman",
                    "foreman",
                    &[Grant::KnowledgeWrite, Grant::TaskEdit],
                ),
            ]),
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(
            statuses[0]
                .status
                .reasons()
                .iter()
                .any(|r| r.contains("foreman") && r.contains("knowledge.write")),
            "{:?}",
            statuses[0].status
        );
    }

    #[test]
    fn roles_satisfied_when_no_agent_holds_a_forbidden_grant() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Roles {
                forbid: vec![Grant::PolicyAttest],
            }],
            Vec::new(),
        )];
        let evidence = Evidence {
            agents: Some(vec![agent(
                "worker",
                "worker",
                &[Grant::TaskEdit, Grant::RunInput],
            )]),
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Satisfied);
    }

    #[test]
    fn roles_open_when_an_agents_role_does_not_resolve() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Roles {
                forbid: vec![Grant::PolicyAttest],
            }],
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
            statuses[0]
                .status
                .reasons()
                .iter()
                .any(|r| r.contains("ghost") && r.contains("vanished")),
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
        assert!(
            statuses[0]
                .status
                .reasons()
                .iter()
                .any(|r| r.contains("not resolved")),
            "{:?}",
            statuses[0].status
        );
    }

    #[test]
    fn sandbox_with_no_agent_declared_is_satisfied() {
        let applied = vec![applied_control("a", vec![Check::Sandbox], Vec::new())];
        let evidence = Evidence {
            agents: Some(Vec::new()),
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Satisfied);
    }

    #[test]
    fn sandbox_open_when_an_agent_declares_none() {
        let applied = vec![applied_control("a", vec![Check::Sandbox], Vec::new())];
        let evidence = Evidence {
            agents: Some(vec![
                agent("worker", "worker", &[]),
                AgentFact {
                    name: "foreman".to_string(),
                    role: "foreman".to_string(),
                    grants: Some(BTreeSet::new()),
                    has_sandbox: false,
                    sandbox_enforced: false,
                },
            ]),
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(
            statuses[0]
                .status
                .reasons()
                .iter()
                .any(|r| r.contains("foreman")),
            "{:?}",
            statuses[0].status
        );
    }

    #[test]
    fn sandbox_satisfied_when_every_agent_declares_one() {
        let applied = vec![applied_control("a", vec![Check::Sandbox], Vec::new())];
        let evidence = Evidence {
            agents: Some(vec![
                agent("worker", "worker", &[]),
                agent("foreman", "foreman", &[]),
            ]),
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
        assert!(
            reasons.contains("declared but not enforced: builder"),
            "{reasons}"
        );
    }

    // -- evaluate: secrets ----------------------------------------------------

    #[test]
    fn secrets_with_the_default_location_never_resolved_stays_open() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Secrets { absent: Vec::new() }],
            Vec::new(),
        )];
        let statuses = evaluate(&applied, &Evidence::default(), Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(
            statuses[0]
                .status
                .reasons()
                .iter()
                .any(|r| r.contains("not resolved")),
            "{:?}",
            statuses[0].status
        );
    }

    #[test]
    fn secrets_satisfied_when_the_default_location_is_absent() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Secrets { absent: Vec::new() }],
            Vec::new(),
        )];
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
            vec![Check::Secrets {
                absent: vec!["anthropic".to_string(), "ssh".to_string()],
            }],
            Vec::new(),
        )];
        let evidence = Evidence {
            secrets: BTreeMap::from([("anthropic".to_string(), true), ("ssh".to_string(), false)])
                .into(),
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(
            statuses[0]
                .status
                .reasons()
                .iter()
                .any(|r| r.contains("anthropic")),
            "{:?}",
            statuses[0].status
        );
    }

    // -- evaluate: daemon -----------------------------------------------------

    #[test]
    fn daemon_check_with_no_facts_gathered_stays_open() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Daemon {
                fact: "power_assertion".to_string(),
            }],
            Vec::new(),
        )];
        let statuses = evaluate(&applied, &Evidence::default(), Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(
            statuses[0]
                .status
                .reasons()
                .iter()
                .any(|r| r.contains("not resolved")),
            "{:?}",
            statuses[0].status
        );
    }

    #[test]
    fn daemon_check_satisfied_when_the_named_fact_holds() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Daemon {
                fact: "power_assertion".to_string(),
            }],
            Vec::new(),
        )];
        let evidence = Evidence {
            daemon: Some(DaemonFact {
                foreman_enabled: false,
                http_loopback_only: None,
                power_assertion: true,
            }),
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Satisfied);
    }

    #[test]
    fn daemon_check_open_when_the_named_fact_does_not_hold() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Daemon {
                fact: "foreman_enabled".to_string(),
            }],
            Vec::new(),
        )];
        let evidence = Evidence {
            daemon: Some(DaemonFact {
                foreman_enabled: false,
                http_loopback_only: None,
                power_assertion: true,
            }),
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
    }

    #[test]
    fn daemon_check_naming_an_unknown_fact_stays_open_with_its_own_reason() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Daemon {
                fact: "launches_rockets".to_string(),
            }],
            Vec::new(),
        )];
        let evidence = Evidence {
            daemon: Some(DaemonFact::default()),
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(
            statuses[0]
                .status
                .reasons()
                .iter()
                .any(|r| r.contains("not a fact this build knows")),
            "{:?}",
            statuses[0].status
        );
    }

    #[test]
    fn daemon_check_open_when_http_loopback_only_cannot_be_determined() {
        let applied = vec![applied_control(
            "a",
            vec![Check::Daemon {
                fact: "http_loopback_only".to_string(),
            }],
            Vec::new(),
        )];
        let evidence = Evidence {
            daemon: Some(DaemonFact {
                foreman_enabled: false,
                http_loopback_only: None,
                power_assertion: true,
            }),
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(
            statuses[0]
                .status
                .reasons()
                .iter()
                .any(|r| r.contains("could not be determined")),
            "{:?}",
            statuses[0].status
        );
    }

    /// `#154`: `evidence.backup` missing (never gathered) is "not resolved",
    /// distinct from gathered-but-indeterminate ("could not be determined")
    /// -- the same two-level reading `daemon_fact_value` now gives every
    /// `daemon` name, backup or not.
    #[test]
    fn a_backup_fact_check_reads_not_resolved_when_never_gathered_and_could_not_be_determined_when_indeterminate(
    ) {
        let applied = vec![applied_control(
            "a",
            vec![Check::Daemon {
                fact: "backup_verified".to_string(),
            }],
            Vec::new(),
        )];
        let statuses = evaluate(&applied, &Evidence::default(), Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(
            statuses[0]
                .status
                .reasons()
                .iter()
                .any(|r| r.contains("not resolved")),
            "{:?}",
            statuses[0].status
        );

        let backup_fact = factory_kernel::BackupFact {
            at: Utc::now(),
            configured: true,
            newest: None,
            recent: None,
            offsite: None,
            verified: None,
            last_verified: None,
        };
        let evidence = Evidence {
            backup: Some(backup_fact),
            ..Default::default()
        };
        let statuses = evaluate(&applied, &evidence, Utc::now());
        assert_eq!(statuses[0].status.kind(), StatusKind::Open);
        assert!(
            statuses[0]
                .status
                .reasons()
                .iter()
                .any(|r| r.contains("could not be determined")),
            "{:?}",
            statuses[0].status
        );
    }

    #[test]
    fn backup_daemon_checks_read_satisfied_or_open_off_the_backup_fact() {
        let fact_with = |recent: Option<bool>, offsite: Option<bool>, verified: Option<bool>| {
            factory_kernel::BackupFact {
                at: Utc::now(),
                configured: true,
                newest: Some(Utc::now()),
                recent,
                offsite,
                verified,
                last_verified: None,
            }
        };

        let recent_applied = vec![applied_control(
            "a",
            vec![Check::Daemon {
                fact: "backup_recent".to_string(),
            }],
            Vec::new(),
        )];
        let evidence = Evidence {
            backup: Some(fact_with(Some(true), Some(false), Some(false))),
            ..Default::default()
        };
        assert_eq!(
            evaluate(&recent_applied, &evidence, Utc::now())[0]
                .status
                .kind(),
            StatusKind::Satisfied
        );

        let offsite_applied = vec![applied_control(
            "a",
            vec![Check::Daemon {
                fact: "backup_offsite".to_string(),
            }],
            Vec::new(),
        )];
        let evidence = Evidence {
            backup: Some(fact_with(Some(true), Some(false), Some(false))),
            ..Default::default()
        };
        let statuses = evaluate(&offsite_applied, &evidence, Utc::now());
        assert_eq!(
            statuses[0].status.kind(),
            StatusKind::Open,
            "the temp destination is the same device"
        );
        assert!(
            statuses[0]
                .status
                .reasons()
                .iter()
                .any(|r| r.contains("does not hold")),
            "{:?}",
            statuses[0].status
        );
    }

    #[test]
    fn an_unexpired_attestation_gives_attested() {
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
        let evidence = Evidence {
            attestations: vec![attestation],
            ..Default::default()
        };
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
        let by_id: BTreeMap<&str, &ControlStatus> = statuses
            .iter()
            .map(|s| (s.control.id.as_str(), s))
            .collect();
        assert_eq!(by_id["a"].status.kind(), StatusKind::Satisfied);
        assert_eq!(by_id["b"].status.kind(), StatusKind::Satisfied);
        assert!(by_id["b"]
            .status
            .reasons()
            .iter()
            .any(|r| r.contains("cra/a")));
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
        let by_id: BTreeMap<&str, &ControlStatus> = statuses
            .iter()
            .map(|s| (s.control.id.as_str(), s))
            .collect();
        assert_eq!(by_id["b"].status.kind(), StatusKind::Attested);
        assert!(by_id["b"]
            .status
            .reasons()
            .iter()
            .any(|r| r.contains("attested via cra/a")));
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

        let empty = ControlStatus {
            refs: Vec::new(),
            ..cs
        };
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
        assert_eq!(
            statuses[0].status.kind(),
            StatusKind::Satisfied,
            "{:?}",
            statuses[0].status
        );
        assert!(statuses[0].refs.contains(&EvidenceRef::run("run-1")));
        assert!(
            !statuses[0].refs.contains(&EvidenceRef::run("run-2")),
            "the in-progress run is never cited as evidence"
        );
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
        assert!(
            statuses[0].status.reasons()[0].contains("in progress"),
            "{:?}",
            statuses[0].status
        );
        assert!(
            statuses[0].refs.contains(&EvidenceRef::run("run-1")),
            "still worth linking to, even though it cannot satisfy the check"
        );
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
        assert!(
            statuses[0].status.reasons()[0].contains("no task named"),
            "{:?}",
            statuses[0].status
        );
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
        assert!(
            statuses[0].status.reasons()[0].contains("never run"),
            "{:?}",
            statuses[0].status
        );
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
        assert!(
            statuses[0].status.reasons()[0].contains("failed"),
            "{:?}",
            statuses[0].status
        );
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
        assert!(statuses[0]
            .refs
            .contains(&EvidenceRef::workflow_run("wfr-1")));
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
        assert_eq!(
            statuses[0].status.kind(),
            StatusKind::Satisfied,
            "{:?}",
            statuses[0].status
        );
        assert!(statuses[0]
            .refs
            .contains(&EvidenceRef::workflow_run("wfr-1")));
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
        assert!(
            statuses[0].status.reasons()[0].contains("in progress"),
            "{:?}",
            statuses[0].status
        );
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
        assert!(
            statuses[0].status.reasons()[0].contains("no workflow named"),
            "{:?}",
            statuses[0].status
        );
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
        assert!(statuses[0].status.reasons()[0].contains("matches more than one workflow's name"));

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
        assert!(statuses[0]
            .refs
            .contains(&EvidenceRef::bench_run("bench-1")));
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
        assert_eq!(
            statuses[0].status.kind(),
            StatusKind::Satisfied,
            "case-2 failing must not matter"
        );
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
        assert!(
            statuses[0].status.reasons()[0].contains("no settled bench run"),
            "{:?}",
            statuses[0].status
        );
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
        assert!(
            statuses[0].status.reasons()[0].contains("case-1"),
            "{:?}",
            statuses[0].status
        );
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
        assert!(
            statuses[0].status.reasons()[0].contains("no gate configured"),
            "{:?}",
            statuses[0].status
        );
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
            status(
                "cra",
                "a",
                Kind::Regulation,
                Status::Satisfied { reasons: vec![] },
            ),
            status(
                "cra",
                "b",
                Kind::Standard,
                Status::Attested { reasons: vec![] },
            ),
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
        let statuses = vec![status(
            "cra",
            "a",
            Kind::Regulation,
            Status::Open { reasons: vec![] },
        )];
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
        assert!(
            rollups[0].compliant,
            "no regulation/standard controls to fail"
        );
        assert_eq!(rollups[0].counts.open, 0);
        assert_eq!(rollups[0].best_practice.open, 1);
    }

    #[test]
    fn rollup_groups_by_framework_and_sorts_the_result() {
        let statuses = vec![
            status(
                "gdpr",
                "a",
                Kind::Regulation,
                Status::Open { reasons: vec![] },
            ),
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
            vec![status(
                "cra",
                "a",
                Kind::Regulation,
                Status::Satisfied { reasons: vec![] },
            )],
            vec![status(
                "cra",
                "a",
                Kind::Regulation,
                Status::Open { reasons: vec![] },
            )],
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
            vec![status(
                "cra",
                "a",
                Kind::Regulation,
                Status::Satisfied { reasons: vec![] },
            )],
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
            vec![status(
                "cra",
                "a",
                Kind::Regulation,
                Status::Open { reasons: vec![] },
            )],
            vec![status(
                "gdpr",
                "b",
                Kind::Regulation,
                Status::Satisfied { reasons: vec![] },
            )],
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
            vec![status(
                "cra",
                "a",
                Kind::Regulation,
                Status::Satisfied { reasons: vec![] },
            )],
            vec![status(
                "cra",
                "a",
                Kind::Regulation,
                Status::Open { reasons: vec![] },
            )],
        ];
        let rollups = rollup(&worst_across_scopes(&per_scope));
        assert_eq!(rollups.len(), 1);
        assert!(
            !rollups[0].compliant,
            "open in even one applicable scope is not compliant"
        );
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
            Check::Task {
                task: "sbom export".into(),
                max_age: Some("7d".parse().unwrap())
            }
            .describe(),
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
        let text =
            remediation_instructions(&control, Some("Publish an SBOM.  \n"), &status, &checks);

        let remediation_at = text
            .find("Publish an SBOM.")
            .expect("remediation text present");
        let missing_at = text
            .find("Missing evidence:")
            .expect("missing-evidence heading present");
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
        let status = Status::Stale {
            reasons: vec!["attestation: `att-1` expired at 2020-01-01T00:00:00Z".to_string()],
        };
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
        assert_eq!(
            evaluate(&applied, &evidence, now)[0].status.kind(),
            StatusKind::Satisfied
        );

        evidence
            .dependencies
            .as_mut()
            .unwrap()
            .open
            .insert(Severity::Critical, 1);
        evidence.dependencies.as_mut().unwrap().exploited_open = 1;
        assert_eq!(
            evaluate(&applied, &evidence, now)[0].status.kind(),
            StatusKind::Open
        );
    }

    #[test]
    fn dependencies_check_requires_build_evidence_when_requested() {
        let check = Check::Dependencies {
            sbom_max_age: None,
            built_sbom: true,
            max_open: BTreeMap::new(),
            exploited_open: None,
        };
        let applied = vec![applied_control(
            "built-dependencies",
            vec![check],
            Vec::new(),
        )];
        let now = "2026-09-25T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let mut evidence = Evidence::default();
        evidence.dependencies = Some(DependenciesFact::default());

        let missing = evaluate(&applied, &evidence, now);
        assert_eq!(missing[0].status.kind(), StatusKind::Open);
        assert!(missing[0]
            .status
            .reasons()
            .iter()
            .any(|reason| reason == "dependencies: no built SBOM for the newest release"));

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
        required: Vec<factory_kernel::RequiredStep>,
        attestations: Vec<factory_kernel::StepAttestation>,
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

    fn gate_step(name: &str) -> factory_kernel::RequiredStep {
        factory_kernel::RequiredStep {
            step: name.to_string(),
            kind: factory_kernel::StepKind::Gate,
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
        verdict: factory_kernel::AttestationVerdict,
        actor: &str,
        at: DateTime<Utc>,
    ) -> factory_kernel::StepAttestation {
        factory_kernel::StepAttestation {
            id: uuid::Uuid::new_v4().to_string(),
            run_id: run_id.to_string(),
            task_id: format!("task-{run_id}"),
            scope: "demo".to_string(),
            category: "feature".to_string(),
            step: step.to_string(),
            kind: factory_kernel::StepKind::Gate,
            actor: actor.to_string(),
            verdict,
            findings: None,
            round: 0,
            required_by: Vec::new(),
            command: Some("true".to_string()),
            exit_code: Some(if verdict == factory_kernel::AttestationVerdict::Pass {
                0
            } else {
                1
            }),
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
        assert!(!factory_assurance::quality::gathered(
            &check,
            &Evidence::default()
        ));
        let evidence = Evidence {
            attested: Some(Vec::new()),
            ..Default::default()
        };
        assert!(factory_assurance::quality::gathered(&check, &evidence));
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
                factory_kernel::AttestationVerdict::Pass,
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
                factory_kernel::AttestationVerdict::Fail,
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
                factory_kernel::AttestationVerdict::Pass,
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
                factory_kernel::AttestationVerdict::Pass,
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
                factory_kernel::AttestationVerdict::Pass,
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
    fn a_control_max_age_shorter_than_its_attested_check_is_honoured_by_evaluate() {
        // The `attested` check names 30d; the control itself says 7d.
        // `direct_status` must read `applied.max_age` (7d, the minimum),
        // never the check's own raw field (30d) -- otherwise a run 10 days
        // old would wrongly read `satisfied`.
        let policy = file(
            "framework: cra\ntitle: CRA\nkind: regulation\ncontrols:\n\
             \x20 - {id: a, title: A, max_age: 7d, evidence: [{check: attested, category: feature, step: tests, max_age: 30d}]}\n",
        );
        let (owned, findings) = resolve(&policy, "root", "cra.yaml", &[]);
        assert!(findings.is_empty(), "{findings:?}");
        let applied = owned.controls;
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
                factory_kernel::AttestationVerdict::Pass,
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
            daemon: Some(DaemonFact {
                foreman_enabled: true,
                http_loopback_only: None,
                power_assertion: true,
            }),
            dependencies: Some(DependenciesFact {
                declared_sbom_at: Some("2026-09-01T00:00:00Z".parse().unwrap()),
                built_sbom_at: None,
                open: BTreeMap::from([(Severity::High, 2)]),
                exploited_open: 1,
            }),
            backup: Some(factory_kernel::BackupFact {
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
