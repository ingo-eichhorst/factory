//! `#169` (Intake: per-scope definitions of ready): authored, inheritable
//! checks and limits a scope adds on top of intake's seven built-in
//! readiness axes (`crate::intake::Axis`) -- modelled on `quality.rs`:
//! authored YAML, re-read on every request, add-or-tighten only down the
//! scope chain, a [`Finding`] for anything an author got wrong rather than a
//! hard failure that takes the rest of the chain down with it.
//!
//! This slice is pure `factory-core` only: the schema, the loader, the
//! add-or-tighten fold. No engine, protocol, access, CLI or HTTP wiring, and
//! no I/O anywhere in this module except [`load`] itself. `crate::intake`'s
//! `validate` and `evaluate` are the two places a [`ReadyDefinition`] is
//! actually enforced.
//!
//! ## Files
//!
//! `<root>/.factory/intake/ready.yaml` is the root layer: it applies to
//! every scope without being named anywhere -- the one way this differs
//! from `quality.rs`'s top-level `quality: [profile, ...]` list (see
//! "Deviation from the issue text" below). Every other file,
//! `<root>/.factory/intake/<name>.yaml`, is bound by a nested scope's own
//! `scope.intake: [name, ...]` for itself and every scope below it by path
//! (`Config::intake_chain_for_scope`) -- exactly `scope.quality`'s chain:
//! ancestry by `Scope.path`, never by name, so a sibling never inherits.
//!
//! ## Schema
//!
//! ```yaml
//! checks:
//!   - id: threat-model                 # slug; an addition to the seven built-in axes
//!     pass_condition: "A security-relevant change names its threat model."
//!     categories: [security-report]    # optional; absent means every category
//! max_complexity: 6                    # 1-8, default 8; complexity 9-10 always needs info
//! observability_tolerance: low         # medium (default) | low | none
//! ```
//!
//! The seven built-in axes are never declared here and can never be
//! removed -- a file only ever adds to, or tightens, what intake already
//! checks.
//!
//! ## Add or tighten only
//!
//! Checks are combined by `id`, first-declared order, across the chain
//! (root first, a file bound at two layers folded once, at the higher one).
//! A descendant redeclaring an inherited id may only *widen* its
//! `categories` (absent is already the widest, "every category");
//! `max_complexity` and `observability_tolerance` take the stricter of
//! every declared value. Anything looser -- a narrower `categories`, a
//! higher `max_complexity`, a more permissive `observability_tolerance` --
//! is a [`FindingKind::Loosening`], and the inherited value is kept. A
//! different `pass_condition` on a redeclared id is a
//! [`FindingKind::ConflictingOverride`], the same reason
//! `quality::merge_scenario` keeps one: two sentences for one id would
//! leave a triager guessing which one to satisfy.
//!
//! ## Fail closed
//!
//! A file [`load`] cannot parse, or a name a `scope.intake` binds that has
//! no file, is never a silent fallback to what an ancestor declared:
//! alongside the [`Finding`] naming it, [`effective`] records a human
//! sentence in [`ReadyDefinition::unreadable`], and `crate::intake::evaluate`
//! turns each one into its own blocker -- "definition of ready for `<scope>`
//! could not be read: ..." -- so every assessment routed to that scope is
//! needs-info until it is fixed. The root's own `ready.yaml` is the one
//! exception to "missing is fail-closed": nothing *binds* it, so its
//! absence simply means "this instance declares nothing beyond the seven
//! axes", the same as an empty `.factory/quality/`. A file that exists but
//! fails to parse is always fail-closed, root layer included -- only a
//! file that was never there at all gets that exception.
//!
//! ## Deviation from the issue text
//!
//! The issue writes `.factory/intake/ready.yaml` as if it were one file, a
//! scope's own. Two rules argue against that literal reading: AGENTS.md's
//! "a scope owns only its `.factory/config.yaml`", and `backup::AUTHORED`,
//! which only ever walks paths under the instance root. So, exactly as
//! `quality.rs` and `policy.rs` already do for their own authored content,
//! every definition of ready lives under the *instance root's*
//! `.factory/intake/`, and a nested scope opts in with a name
//! (`scope.intake`) rather than a path. Unlike quality's profiles, though,
//! there is no top-level `intake: [...]` list for the root to bind more of:
//! `ready.yaml` is the root's whole layer, always present if the file is,
//! and the root's own scope entry refuses a `scope.intake` block outright
//! (`Config::refuse_root_scope_intake`) rather than being told to move a
//! binding it has no list to move it to.

use crate::dataset::is_slug;
use crate::intake::ObservabilityCost;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

// ================================================================== schema

/// How much of a failed Observability axis a scope's chain tolerates --
/// declared low to high tolerance so the derived `Ord` makes "stricter"
/// `min`. `Medium` is today's rule (`ObservabilityCost::Low` or `Medium`
/// pass through, `High` never does), and every scope's default with no
/// definition at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tolerance {
    None,
    Low,
    Medium,
}

impl Default for Tolerance {
    fn default() -> Self {
        Tolerance::Medium
    }
}

impl Tolerance {
    pub fn as_str(self) -> &'static str {
        match self {
            Tolerance::None => "none",
            Tolerance::Low => "low",
            Tolerance::Medium => "medium",
        }
    }

    /// Whether a failed Observability axis at `cost` is tolerated under this
    /// setting -- `crate::intake::evaluate`'s one rule, generalised: at
    /// `Medium` (the default) low or medium cost passes through, at `Low`
    /// only low does, and `None` never tolerates a failed Observability axis
    /// at all.
    pub fn allows(self, cost: ObservabilityCost) -> bool {
        match self {
            Tolerance::None => false,
            Tolerance::Low => cost == ObservabilityCost::Low,
            Tolerance::Medium => matches!(cost, ObservabilityCost::Low | ObservabilityCost::Medium),
        }
    }
}

/// `max_complexity`'s own scale -- 1 is the loosest a file could bother
/// stating, 8 is the default (today's ceiling before complexity 9-10's own
/// fixed "always needs info" rule takes over).
pub const MIN_COMPLEXITY: u8 = 1;
pub const MAX_COMPLEXITY: u8 = 8;
pub const DEFAULT_MAX_COMPLEXITY: u8 = MAX_COMPLEXITY;

/// One check declared in a definition file: an id (a slug, an addition to
/// the seven built-in axes), its pass condition, and which categories it
/// applies to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckDef {
    pub id: String,
    pub pass_condition: String,
    /// Absent (or empty) means every category.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
}

/// One `<root>/.factory/intake/<name>.yaml`, exactly as authored. `<name>`
/// is `"ready"` for the root layer; any other name is bound by a
/// `scope.intake` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadyFile {
    #[serde(default)]
    pub checks: Vec<CheckDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_complexity: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observability_tolerance: Option<Tolerance>,
}

// ================================================================ findings

/// One of the things [`load`] or [`effective`] checks for. Never stops
/// another file, or another part of the same file, from loading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    ParseFailed,
    /// A check id is not `dataset::is_slug`-shaped.
    BadIdShape,
    /// A check id repeated within one file. The first is kept.
    DuplicateId,
    /// `max_complexity` outside 1-8. The file's own value is dropped, as if
    /// it had not declared one.
    BadComplexity,
    /// A `scope.intake` binding names a file [`load`] has no entry for --
    /// it either does not exist or failed to parse; either way the bound
    /// scope's whole chain cannot be trusted, and `crate::intake::evaluate`
    /// blocks every assessment routed there until it is fixed.
    Unreadable,
    /// A descendant redeclared an id or a limit with something looser than
    /// what it inherited. The inherited value is kept.
    Loosening,
    /// A descendant redeclared an id's `pass_condition` differently. The
    /// inherited one is kept.
    ConflictingOverride,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub kind: FindingKind,
    /// The file ([`load`]) or the scope ([`effective`]) the finding is
    /// about -- "where to go look", as in `quality::Finding`.
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

/// `<root>/.factory/intake`, the authored-content directory.
pub fn ready_dir(root: &Path) -> PathBuf {
    root.join(".factory").join("intake")
}

/// Everything [`load`] found in `<root>/.factory/intake/`.
#[derive(Debug, Clone, Default)]
pub struct ReadyCatalogue {
    /// Keyed by file stem -- `"ready"` for the root layer.
    pub files: BTreeMap<String, ReadyFile>,
    /// Sorted by `(subject, kind, detail)`.
    pub findings: Vec<Finding>,
    /// File stems that existed on disk but could not be read or parsed --
    /// as opposed to a stem nobody ever wrote a file for. [`effective`]
    /// reads this to fail closed even on the *implicit* root layer, whose
    /// mere absence is not an error (see the module doc's "Fail closed").
    pub failed: BTreeSet<String>,
}

/// Load every `<name>.yaml` in `dir`. A missing directory is empty, not an
/// error; a file that fails to parse is a finding (and its stem is recorded
/// in `failed`) and every other file still loads. See the module doc
/// comment for what is dropped from a file that did parse.
pub fn load(dir: &Path) -> ReadyCatalogue {
    let mut findings = Vec::new();
    let mut files = BTreeMap::new();
    let mut failed = BTreeSet::new();

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
                failed.insert(stem);
                continue;
            }
        };
        match serde_yaml_ng::from_str::<ReadyFile>(&text) {
            Ok(file) => {
                if !is_slug(&stem) {
                    findings.push(finding(
                        FindingKind::BadIdShape,
                        &file_name,
                        format!("definition id {stem:?} is not shaped like dataset::is_slug ([a-z0-9][a-z0-9-]*)"),
                    ));
                }
                files.insert(stem, validate_file(file, &file_name, &mut findings));
            }
            Err(e) => {
                findings.push(finding(FindingKind::ParseFailed, &file_name, format!("parsing: {e}")));
                failed.insert(stem);
            }
        }
    }

    sort_findings(&mut findings);
    ReadyCatalogue { files, findings, failed }
}

/// One parsed file's own checks: every duplicate id dropped (first kept),
/// every check id checked for shape, and `max_complexity` dropped back to
/// "not declared" when it is off the 1-8 scale. Everything else stays,
/// finding or not.
fn validate_file(mut file: ReadyFile, subject: &str, findings: &mut Vec<Finding>) -> ReadyFile {
    let mut seen = BTreeSet::new();
    file.checks.retain(|c| {
        if !seen.insert(c.id.clone()) {
            findings.push(finding(FindingKind::DuplicateId, subject, format!("check {:?} is declared twice", c.id)));
            return false;
        }
        true
    });
    for c in &file.checks {
        if !is_slug(&c.id) {
            findings.push(finding(
                FindingKind::BadIdShape,
                subject,
                format!("check id {:?} is not shaped like dataset::is_slug ([a-z0-9][a-z0-9-]*)", c.id),
            ));
        }
    }
    if let Some(mc) = file.max_complexity {
        if !(MIN_COMPLEXITY..=MAX_COMPLEXITY).contains(&mc) {
            findings.push(finding(
                FindingKind::BadComplexity,
                subject,
                format!("max_complexity {mc} is off the {MIN_COMPLEXITY}-{MAX_COMPLEXITY} scale; the file's own value is dropped"),
            ));
            file.max_complexity = None;
        }
    }
    file
}

// ============================================================ applicability

/// One resolved layer of a scope's readiness chain, as
/// `Config::intake_chain_for_scope` builds it -- `quality::QualityLayer`'s
/// role, for definitions of ready. `required` is false only for the
/// implicit root `"ready"` layer: nothing bound it, so it being missing is
/// not fail-closed (see the module doc's "Fail closed" section); every
/// layer a `scope.intake` names is `required`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IntakeLayer {
    pub scope: String,
    pub files: Vec<String>,
    pub required: bool,
}

/// Where a merged check was first declared: the scope whose layer bound it,
/// and the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    pub scope: String,
    pub file: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedCheck {
    pub id: String,
    pub pass_condition: String,
    /// Empty means every category.
    pub categories: Vec<String>,
    pub declared_at: Origin,
}

/// One scope's effective definition of ready: the seven built-in axes plus
/// whatever its chain adds or tightens. `checks` is empty, `max_complexity`
/// is [`DEFAULT_MAX_COMPLEXITY`] and `observability_tolerance` is
/// [`Tolerance::Medium`] for a scope whose chain binds nothing -- exactly
/// today's seven axes, unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadyDefinition {
    pub scope: String,
    /// Every layer's checks folded in, first-declared order.
    pub checks: Vec<AppliedCheck>,
    pub max_complexity: u8,
    pub observability_tolerance: Tolerance,
    /// One sentence per file in the chain that could not be read -- see the
    /// module doc's "Fail closed" section. Non-empty means
    /// `crate::intake::evaluate` blocks every assessment for this scope.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unreadable: Vec<String>,
}

impl Default for ReadyDefinition {
    fn default() -> Self {
        ReadyDefinition {
            scope: String::new(),
            checks: Vec::new(),
            max_complexity: DEFAULT_MAX_COMPLEXITY,
            observability_tolerance: Tolerance::default(),
            unreadable: Vec::new(),
        }
    }
}

impl ReadyDefinition {
    /// Every check that applies to `category`: none declared for it (the
    /// default, "every category"), or `category` named explicitly.
    pub fn applicable(&self, category: &str) -> Vec<&AppliedCheck> {
        self.checks.iter().filter(|c| c.categories.is_empty() || c.categories.iter().any(|x| x == category)).collect()
    }
}

/// Fold `chain` (root first) into `scope`'s effective definition. Add or
/// tighten only -- see the module doc comment for the exact rules, and for
/// what "fail closed" does to a layer whose file could not be read.
pub fn effective(catalogue: &ReadyCatalogue, scope: &str, chain: &[IntakeLayer]) -> (ReadyDefinition, Vec<Finding>) {
    let mut def = ReadyDefinition { scope: scope.to_string(), ..Default::default() };
    let mut findings = Vec::new();
    let mut applied: Vec<String> = Vec::new();

    for layer in chain {
        for name in &layer.files {
            if applied.contains(name) {
                continue; // bound at two layers -- folded once, at the higher one
            }
            match catalogue.files.get(name) {
                Some(file) => {
                    applied.push(name.clone());
                    merge_file(&mut def, file, &layer.scope, name, &mut findings);
                }
                None => {
                    let known_broken = catalogue.failed.contains(name);
                    if layer.required || known_broken {
                        // Mark it applied only once we have actually committed
                        // to the fail-closed finding and blocker below -- the
                        // implicit root layer's own merely-absent `ready`
                        // (`required` false, nothing in `failed`) falls
                        // through here unmarked, so a nested scope that
                        // later *binds* the same name still gets its own
                        // check rather than being silently skipped as
                        // "already handled".
                        applied.push(name.clone());
                        let reason = if known_broken { format!("{name}.yaml could not be parsed") } else { format!("no {name}.yaml file exists") };
                        findings.push(finding(
                            FindingKind::Unreadable,
                            &layer.scope,
                            format!("binds {name:?}, but {reason}"),
                        ));
                        def.unreadable.push(format!(
                            "definition of ready for {scope} could not be read: {} binds {name:?}, but {reason}",
                            layer.scope
                        ));
                    }
                }
            }
        }
    }

    sort_findings(&mut findings);
    (def, findings)
}

fn merge_file(def: &mut ReadyDefinition, file: &ReadyFile, scope: &str, file_name: &str, findings: &mut Vec<Finding>) {
    let origin = Origin { scope: scope.to_string(), file: file_name.to_string() };
    for check in &file.checks {
        match def.checks.iter_mut().find(|c| c.id == check.id) {
            None => def.checks.push(AppliedCheck {
                id: check.id.clone(),
                pass_condition: check.pass_condition.clone(),
                categories: check.categories.clone(),
                declared_at: origin.clone(),
            }),
            Some(have) => merge_check(have, check, scope, findings),
        }
    }

    if let Some(mc) = file.max_complexity {
        match mc.cmp(&def.max_complexity) {
            std::cmp::Ordering::Less => def.max_complexity = mc,
            std::cmp::Ordering::Equal => {}
            std::cmp::Ordering::Greater => findings.push(finding(
                FindingKind::Loosening,
                scope,
                format!(
                    "max_complexity {mc} loosens the inherited {}; a descendant may only lower it",
                    def.max_complexity
                ),
            )),
        }
    }

    if let Some(tolerance) = file.observability_tolerance {
        match tolerance.cmp(&def.observability_tolerance) {
            std::cmp::Ordering::Less => def.observability_tolerance = tolerance,
            std::cmp::Ordering::Equal => {}
            std::cmp::Ordering::Greater => findings.push(finding(
                FindingKind::Loosening,
                scope,
                format!(
                    "observability_tolerance {:?} loosens the inherited {:?}; a descendant may only lower it",
                    tolerance.as_str(),
                    def.observability_tolerance.as_str()
                ),
            )),
        }
    }
}

fn merge_check(have: &mut AppliedCheck, new: &CheckDef, scope: &str, findings: &mut Vec<Finding>) {
    if new.pass_condition != have.pass_condition {
        findings.push(finding(
            FindingKind::ConflictingOverride,
            scope,
            format!(
                "check {:?} restates its pass_condition differently; the inherited one is kept",
                new.id
            ),
        ));
    }
    if new.categories.is_empty() {
        have.categories = Vec::new(); // widening to "every category" -- always allowed
        return;
    }
    if have.categories.is_empty() {
        findings.push(finding(
            FindingKind::Loosening,
            scope,
            format!(
                "check {:?} narrows its categories to {:?}; categories may only widen, so it still applies to every category",
                new.id, new.categories
            ),
        ));
        return;
    }
    let wider: BTreeSet<&str> = new.categories.iter().map(String::as_str).collect();
    let existing: BTreeSet<&str> = have.categories.iter().map(String::as_str).collect();
    if wider.is_superset(&existing) {
        have.categories = new.categories.clone();
    } else {
        findings.push(finding(
            FindingKind::Loosening,
            scope,
            format!(
                "check {:?} narrows its categories from {:?} to {:?}; categories may only widen",
                new.id, have.categories, new.categories
            ),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    /// Load `files` from a fresh directory, then remove it -- `load` reads
    /// everything up front, so nothing needs the files afterwards.
    fn catalogue_from(files: &[(&str, &str)]) -> ReadyCatalogue {
        let dir = std::env::temp_dir().join(format!("factory-ready-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, body) in files {
            write(&dir, name, body);
        }
        let c = load(&dir);
        std::fs::remove_dir_all(&dir).ok();
        c
    }

    fn kinds(findings: &[Finding]) -> Vec<FindingKind> {
        findings.iter().map(|f| f.kind).collect()
    }

    fn layer(scope: &str, files: &[&str], required: bool) -> IntakeLayer {
        IntakeLayer { scope: scope.to_string(), files: files.iter().map(|f| f.to_string()).collect(), required }
    }

    /// The implicit root layer: always present, missing is never an error.
    fn root_layer() -> IntakeLayer {
        layer("company", &["ready"], false)
    }

    // -- load --------------------------------------------------------------

    #[test]
    fn a_missing_directory_is_empty_not_an_error() {
        let dir = std::env::temp_dir().join(format!("factory-ready-test-gone-{}", uuid::Uuid::new_v4()));
        let c = load(&dir);
        assert!(c.files.is_empty());
        assert!(c.findings.is_empty());
    }

    #[test]
    fn a_file_that_fails_to_parse_is_a_finding_and_everything_else_still_loads() {
        let c = catalogue_from(&[("ready.yaml", "checks: [unclosed"), ("security.yaml", "checks: []\n")]);
        assert_eq!(kinds(&c.findings), vec![FindingKind::ParseFailed]);
        assert!(c.files.contains_key("security"));
        assert!(!c.files.contains_key("ready"));
        assert!(c.failed.contains("ready"));
    }

    #[test]
    fn a_duplicate_check_id_within_one_file_is_a_finding_the_first_is_kept() {
        let c = catalogue_from(&[(
            "ready.yaml",
            "checks:\n  - id: threat-model\n    pass_condition: first\n  - id: threat-model\n    pass_condition: second\n",
        )]);
        assert_eq!(kinds(&c.findings), vec![FindingKind::DuplicateId]);
        let file = &c.files["ready"];
        assert_eq!(file.checks.len(), 1);
        assert_eq!(file.checks[0].pass_condition, "first");
    }

    #[test]
    fn a_check_id_that_is_not_a_slug_is_a_finding_but_kept() {
        let c = catalogue_from(&[("ready.yaml", "checks:\n  - id: Threat_Model\n    pass_condition: x\n")]);
        assert_eq!(kinds(&c.findings), vec![FindingKind::BadIdShape]);
        assert_eq!(c.files["ready"].checks.len(), 1);
    }

    #[test]
    fn max_complexity_off_the_scale_is_a_finding_and_dropped() {
        let c = catalogue_from(&[("ready.yaml", "max_complexity: 9\n")]);
        assert_eq!(kinds(&c.findings), vec![FindingKind::BadComplexity]);
        assert_eq!(c.files["ready"].max_complexity, None);

        let c = catalogue_from(&[("ready.yaml", "max_complexity: 0\n")]);
        assert_eq!(kinds(&c.findings), vec![FindingKind::BadComplexity]);
    }

    #[test]
    fn an_unknown_key_is_refused_not_silently_dropped() {
        let c = catalogue_from(&[("ready.yaml", "checks: []\nextra_field: true\n")]);
        assert_eq!(kinds(&c.findings), vec![FindingKind::ParseFailed]);
        assert!(!c.files.contains_key("ready"));
    }

    #[test]
    fn an_unknown_observability_tolerance_value_is_refused() {
        let c = catalogue_from(&[("ready.yaml", "observability_tolerance: extreme\n")]);
        assert_eq!(kinds(&c.findings), vec![FindingKind::ParseFailed]);
    }

    // -- effective: no files -------------------------------------------------

    #[test]
    fn no_files_and_no_bindings_gives_exactly_todays_seven_axes() {
        let c = ReadyCatalogue::default();
        let (def, findings) = effective(&c, "demo", &[root_layer()]);
        assert_eq!(def, ReadyDefinition { scope: "demo".into(), ..Default::default() });
        assert!(findings.is_empty());
    }

    // -- effective: add -------------------------------------------------------

    #[test]
    fn a_root_check_applies_and_is_declared_at_the_root() {
        let c = catalogue_from(&[("ready.yaml", "checks:\n  - id: threat-model\n    pass_condition: names a threat model\n")]);
        let (def, findings) = effective(&c, "demo", &[root_layer()]);
        assert!(findings.is_empty());
        assert_eq!(def.checks.len(), 1);
        assert_eq!(def.checks[0].id, "threat-model");
        assert_eq!(def.checks[0].declared_at, Origin { scope: "company".into(), file: "ready".into() });
        assert!(def.checks[0].categories.is_empty(), "no categories declared means every category");
    }

    #[test]
    fn a_childs_own_file_adds_a_check_on_top_of_the_root() {
        let c = catalogue_from(&[
            ("ready.yaml", "checks:\n  - id: threat-model\n    pass_condition: names a threat model\n"),
            ("security.yaml", "checks:\n  - id: pen-test\n    pass_condition: a pen test ran\n"),
        ]);
        let chain = [root_layer(), layer("security-team", &["security"], true)];
        let (def, findings) = effective(&c, "security-team", &chain);
        assert!(findings.is_empty());
        let ids: Vec<&str> = def.checks.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["threat-model", "pen-test"]);
    }

    #[test]
    fn a_sibling_never_sees_a_binding_it_does_not_name() {
        // ready::effective itself only ever folds the chain it is handed;
        // never inheriting sideways is `Config::intake_chain_for_scope`'s
        // job (see config.rs), proven there. Here: a scope whose own chain
        // never names `security` gets nothing from it, even when the file
        // exists and loads cleanly.
        let c = catalogue_from(&[("security.yaml", "checks:\n  - id: pen-test\n    pass_condition: x\n")]);
        let (def, findings) = effective(&c, "other-team", &[root_layer()]);
        assert!(findings.is_empty());
        assert!(def.checks.is_empty());
    }

    // -- effective: tighten (widen categories) --------------------------------

    #[test]
    fn a_redeclared_check_may_widen_its_categories_with_no_finding() {
        let c = catalogue_from(&[
            ("ready.yaml", "checks:\n  - id: threat-model\n    pass_condition: names a threat model\n    categories: [security-report]\n"),
            ("security.yaml", "checks:\n  - id: threat-model\n    pass_condition: names a threat model\n    categories: [security-report, feature]\n"),
        ]);
        let chain = [root_layer(), layer("security-team", &["security"], true)];
        let (def, findings) = effective(&c, "security-team", &chain);
        assert!(findings.is_empty(), "{findings:?}");
        assert_eq!(def.checks[0].categories, vec!["security-report".to_string(), "feature".to_string()]);
    }

    #[test]
    fn widening_all_the_way_to_every_category_is_allowed() {
        let c = catalogue_from(&[
            ("ready.yaml", "checks:\n  - id: threat-model\n    pass_condition: x\n    categories: [security-report]\n"),
            ("security.yaml", "checks:\n  - id: threat-model\n    pass_condition: x\n"),
        ]);
        let chain = [root_layer(), layer("security-team", &["security"], true)];
        let (def, findings) = effective(&c, "security-team", &chain);
        assert!(findings.is_empty());
        assert!(def.checks[0].categories.is_empty());
    }

    #[test]
    fn narrowing_categories_is_a_finding_and_the_inherited_ones_stay() {
        let c = catalogue_from(&[
            ("ready.yaml", "checks:\n  - id: threat-model\n    pass_condition: x\n"),
            ("security.yaml", "checks:\n  - id: threat-model\n    pass_condition: x\n    categories: [security-report]\n"),
        ]);
        let chain = [root_layer(), layer("security-team", &["security"], true)];
        let (def, findings) = effective(&c, "security-team", &chain);
        assert_eq!(kinds(&findings), vec![FindingKind::Loosening]);
        assert!(def.checks[0].categories.is_empty(), "the inherited (wider) categories are kept");
    }

    #[test]
    fn a_different_pass_condition_on_redeclare_is_a_conflicting_override() {
        let c = catalogue_from(&[
            ("ready.yaml", "checks:\n  - id: threat-model\n    pass_condition: first wording\n"),
            ("security.yaml", "checks:\n  - id: threat-model\n    pass_condition: different wording\n"),
        ]);
        let chain = [root_layer(), layer("security-team", &["security"], true)];
        let (def, findings) = effective(&c, "security-team", &chain);
        assert_eq!(kinds(&findings), vec![FindingKind::ConflictingOverride]);
        assert_eq!(def.checks[0].pass_condition, "first wording");
    }

    // -- effective: tighten (max_complexity / observability_tolerance) -------

    #[test]
    fn a_lower_max_complexity_tightens_and_a_higher_one_is_a_finding() {
        let c = catalogue_from(&[("ready.yaml", "max_complexity: 6\n"), ("security.yaml", "max_complexity: 4\n")]);
        let chain = [root_layer(), layer("security-team", &["security"], true)];
        let (def, findings) = effective(&c, "security-team", &chain);
        assert!(findings.is_empty());
        assert_eq!(def.max_complexity, 4);

        let c = catalogue_from(&[("ready.yaml", "max_complexity: 4\n"), ("security.yaml", "max_complexity: 6\n")]);
        let (def, findings) = effective(&c, "security-team", &chain);
        assert_eq!(kinds(&findings), vec![FindingKind::Loosening]);
        assert_eq!(def.max_complexity, 4, "the stricter, inherited value is kept");
    }

    #[test]
    fn a_lower_observability_tolerance_tightens_and_a_higher_one_is_a_finding() {
        let c = catalogue_from(&[("ready.yaml", "observability_tolerance: medium\n"), ("security.yaml", "observability_tolerance: none\n")]);
        let chain = [root_layer(), layer("security-team", &["security"], true)];
        let (def, findings) = effective(&c, "security-team", &chain);
        assert!(findings.is_empty());
        assert_eq!(def.observability_tolerance, Tolerance::None);

        let c = catalogue_from(&[("ready.yaml", "observability_tolerance: low\n"), ("security.yaml", "observability_tolerance: medium\n")]);
        let (def, findings) = effective(&c, "security-team", &chain);
        assert_eq!(kinds(&findings), vec![FindingKind::Loosening]);
        assert_eq!(def.observability_tolerance, Tolerance::Low);
    }

    // -- effective: fail closed -----------------------------------------------

    #[test]
    fn a_bound_name_with_no_file_is_a_finding_and_a_fail_closed_blocker() {
        let c = ReadyCatalogue::default();
        let chain = [root_layer(), layer("security-team", &["security"], true)];
        let (def, findings) = effective(&c, "security-team", &chain);
        assert_eq!(kinds(&findings), vec![FindingKind::Unreadable]);
        assert_eq!(def.unreadable.len(), 1);
        assert!(def.unreadable[0].contains("definition of ready for security-team could not be read"), "{}", def.unreadable[0]);
        assert!(def.checks.is_empty(), "the checks that do exist are still empty, not a crash");
    }

    #[test]
    fn a_malformed_nearer_file_is_a_finding_and_a_fail_closed_blocker() {
        let c = catalogue_from(&[("security.yaml", "checks: [unclosed")]);
        assert!(c.failed.contains("security"));
        let chain = [root_layer(), layer("security-team", &["security"], true)];
        let (def, findings) = effective(&c, "security-team", &chain);
        assert_eq!(kinds(&findings), vec![FindingKind::Unreadable]);
        assert_eq!(def.unreadable.len(), 1);
    }

    #[test]
    fn the_root_layer_missing_is_not_an_error_but_malformed_is() {
        // Nothing at all: the implicit root layer's own absence is exactly
        // "this instance declares nothing beyond the seven axes".
        let c = ReadyCatalogue::default();
        let (def, findings) = effective(&c, "demo", &[root_layer()]);
        assert!(findings.is_empty());
        assert!(def.unreadable.is_empty());

        // The file exists but does not parse: fail closed even though
        // nothing *binds* it -- root or not, a file that exists and is
        // broken is never a silent "nothing declared".
        let c = catalogue_from(&[("ready.yaml", "checks: [unclosed")]);
        let (def, findings) = effective(&c, "demo", &[root_layer()]);
        assert_eq!(kinds(&findings), vec![FindingKind::Unreadable]);
        assert_eq!(def.unreadable.len(), 1);
    }

    #[test]
    fn a_later_explicit_binding_of_the_roots_own_missing_name_still_gets_its_own_check() {
        // Regression: the implicit root layer's merely-absent `ready` must
        // not be marked "applied" and swallow a nested scope's own explicit
        // `intake: [ready]` binding of the same name.
        let c = ReadyCatalogue::default();
        let chain = [root_layer(), layer("security-team", &["ready"], true)];
        let (def, findings) = effective(&c, "security-team", &chain);
        assert_eq!(kinds(&findings), vec![FindingKind::Unreadable]);
        assert_eq!(def.unreadable.len(), 1);
    }

    #[test]
    fn a_file_bound_at_two_layers_is_folded_once_at_the_higher_scope() {
        let c = catalogue_from(&[("shared.yaml", "checks:\n  - id: threat-model\n    pass_condition: x\n")]);
        let chain = [
            layer("company", &["shared"], true),
            layer("security-team", &["shared"], true),
        ];
        let (def, findings) = effective(&c, "security-team", &chain);
        assert!(findings.is_empty());
        assert_eq!(def.checks.len(), 1);
        assert_eq!(def.checks[0].declared_at.scope, "company");
    }

    // -- ReadyDefinition::applicable / Tolerance::allows ----------------------

    #[test]
    fn applicable_matches_no_categories_or_a_named_one() {
        let mut def = ReadyDefinition::default();
        def.checks.push(AppliedCheck {
            id: "global".into(),
            pass_condition: "x".into(),
            categories: vec![],
            declared_at: Origin { scope: "s".into(), file: "ready".into() },
        });
        def.checks.push(AppliedCheck {
            id: "security-only".into(),
            pass_condition: "x".into(),
            categories: vec!["security-report".into()],
            declared_at: Origin { scope: "s".into(), file: "ready".into() },
        });
        let for_bugfix: Vec<&str> = def.applicable("bugfix").into_iter().map(|c| c.id.as_str()).collect();
        assert_eq!(for_bugfix, vec!["global"]);
        let for_security: Vec<&str> = def.applicable("security-report").into_iter().map(|c| c.id.as_str()).collect();
        assert_eq!(for_security, vec!["global", "security-only"]);
    }

    #[test]
    fn tolerance_allows_matches_todays_rule_at_medium_and_tightens_below_it() {
        use ObservabilityCost::*;
        assert!(Tolerance::Medium.allows(Low));
        assert!(Tolerance::Medium.allows(Medium));
        assert!(!Tolerance::Medium.allows(High));
        assert!(Tolerance::Low.allows(Low));
        assert!(!Tolerance::Low.allows(Medium));
        assert!(!Tolerance::None.allows(Low));
        assert!(Tolerance::None < Tolerance::Low);
        assert!(Tolerance::Low < Tolerance::Medium);
    }
}
