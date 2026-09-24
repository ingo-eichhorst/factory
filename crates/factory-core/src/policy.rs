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
//! than this module reading the clock. `evaluate` now understands all nine
//! of `Check`'s kinds: `knowledge` and `attestation` (v1), `task`,
//! `workflow` and `gate` (`#81`), and `roles`, `sandbox`, `secrets` and
//! `daemon` (`#82`). The last four read facts the engine resolves once,
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

use crate::bench::Verdict;
use crate::dataset::is_slug;
use crate::role::Grant;
use crate::run::RunStatus;
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
/// control. Every kind the ADR names is parsed here and `evaluate` now
/// understands all nine -- v1 shipped `knowledge` and `attestation` writable
/// ahead of evaluation, and every ticket since (`#81`, `#82`) taught
/// `evaluate` a few more kinds without ever having to change the file format
/// underneath an author who already wrote one.
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
    /// A scheduled task's newest *finished* run (skipping any still in
    /// progress) is `done` within `max_age`. `task` names a task in the
    /// evaluated scope, by id or by exact title; an ambiguous title is a
    /// [`Finding`], via [`evidence_findings`].
    Task {
        task: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_age: Option<Duration>,
    },
    /// Same as `task`, for a workflow.
    Workflow {
        workflow: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_age: Option<Duration>,
    },
    /// A dataset's gated cases (or, with `case`, one named case) all passed
    /// in the newest *settled* bench run of `dataset`, within `max_age`.
    Gate {
        dataset: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        case: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_age: Option<Duration>,
    },
    /// No agent Factory would dispatch in the scope (`Scope::agents_with`,
    /// which includes a synthesised foreman -- see the README's "Policies"
    /// section for why) is bound to a role holding any of `forbid`. A scope
    /// that declares no agent at all is satisfied: nothing there can hold
    /// the grant.
    Roles {
        #[serde(default)]
        forbid: Vec<Grant>,
    },
    /// Every agent Factory would dispatch in the scope
    /// (`Scope::agents_with`) declares a sandbox other than `none`
    /// (`ScopeAgent.sandbox`). Same empty-scope rule as `roles`.
    Sandbox,
    /// None of `absent` (default: the scope's own `.env`, [`KNOWN_SECRETS_LOCATIONS`]'s
    /// `scope_env`) is present, in the L2 Secrets tab's own inventory
    /// (`Engine::credential_inventory`) -- presence only, never a value.
    Secrets {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        absent: Vec<String>,
    },
    /// A named fact about the daemon's own configuration holds -- see
    /// [`KNOWN_DAEMON_FACTS`] for the fixed vocabulary this evaluates; any
    /// other name is a [`Finding`] ([`FindingKind::UnknownDaemonFact`]) and
    /// stays `open`.
    Daemon { fact: String },
}

/// The `daemon` check's fixed vocabulary -- everything else `DaemonConfig`
/// carries is either not a policy-relevant fact or ambiguous enough that
/// "does it hold" would be a guess (ADR 0004's "facts only, no
/// heuristics"). [`load_all`] checks a `fact` string against this at parse
/// time ([`FindingKind::UnknownDaemonFact`]), so an authoring mistake shows
/// up on the catalogue, not only once a report is evaluated.
///
/// - `foreman_enabled` -- `daemon.foreman.enabled`.
/// - `http_loopback_only` -- every `http` interface the daemon mounts binds
///   to a loopback address, or none is mounted at all.
/// - `power_assertion` -- `daemon.power_assertion`.
pub const KNOWN_DAEMON_FACTS: &[&str] = &["foreman_enabled", "http_loopback_only", "power_assertion"];

/// The `secrets` check's fixed vocabulary -- exactly the locations the L2
/// Secrets tab already reports on (`Engine::credential_inventory`): the
/// five machine-wide locations every scope shares (an agent runs as the
/// daemon's owner, so these are the same regardless of scope) plus a
/// scope's own `.env`. Checked at parse time by [`load_all`]
/// ([`FindingKind::UnknownSecretsLocation`]).
pub const KNOWN_SECRETS_LOCATIONS: &[&str] = &["anthropic", "github", "aws", "netrc", "ssh", "scope_env"];

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
            Check::Secrets { .. } => "secrets",
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
            // `daemon`'s `fact` and `secrets`' `absent` are each a fixed,
            // known vocabulary (`KNOWN_DAEMON_FACTS`/`KNOWN_SECRETS_LOCATIONS`)
            // that never depends on live evidence, so an unknown name is
            // caught here, at parse time, rather than only once a report is
            // evaluated.
            for check in &control.evidence {
                match check {
                    Check::Daemon { fact } if !KNOWN_DAEMON_FACTS.contains(&fact.as_str()) => {
                        findings.push(Finding {
                            kind: FindingKind::UnknownDaemonFact,
                            subject: file_name.clone(),
                            detail: format!(
                                "{}/{} names daemon fact {fact:?}, which is not one of: {}",
                                catalogue.framework,
                                control.id,
                                KNOWN_DAEMON_FACTS.join(", ")
                            ),
                        });
                    }
                    Check::Secrets { absent } => {
                        for loc in absent {
                            if !KNOWN_SECRETS_LOCATIONS.contains(&loc.as_str()) {
                                findings.push(Finding {
                                    kind: FindingKind::UnknownSecretsLocation,
                                    subject: file_name.clone(),
                                    detail: format!(
                                        "{}/{} names secrets location {loc:?}, which is not one of: {}",
                                        catalogue.framework,
                                        control.id,
                                        KNOWN_SECRETS_LOCATIONS.join(", ")
                                    ),
                                });
                            }
                        }
                    }
                    _ => {}
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
        return Ok(now + chrono::Duration::hours(d.as_hours() as i64));
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

/// Enough about one of a task's runs for `evaluate`'s `task` check to judge
/// it without reading a `Run` itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunFact {
    pub id: String,
    pub status: RunStatus,
    pub started_at: DateTime<Utc>,
    /// `None` for a run that has not ended -- a terminal run that somehow
    /// has none is handled the same way, defensively, by `evaluate`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
}

/// One task a `task` check's name could mean, resolved by the engine
/// (`Engine::policy_report`/`policy_control`) against the tasks in the
/// evaluated scope alone. `Evidence::tasks` keys a `Vec` of these by the
/// check's own `task` string, exactly as the catalogue wrote it -- the
/// length says how resolution went: empty means no task matched, by id or by
/// exact title; one means it resolved; more than one means the name is an
/// ambiguous title (ids are unique, so that can only happen for a name that
/// is not one), which [`evidence_findings`] reports as a
/// [`FindingKind::AmbiguousCheckTarget`] and `evaluate` treats as `open`.
///
/// `runs` is only ever resolved for the single unambiguous match --
/// fetching it for every candidate of an ambiguous name would cost a lookup
/// `evaluate` can never use -- and is a bounded lookback (the engine's own
/// `RUN_LOOKBACK`), newest first, not the task's whole history: `evaluate`
/// finds the newest run with a terminal status (`RunStatus::is_terminal`) in
/// it and ignores every run still in progress, however many of those sit
/// ahead of it. An in-flight run must never flip a control `open` for as
/// long as it runs -- a nightly scan's control would otherwise read `open`
/// for the whole scan, every night -- so `evaluate` only ever asks "is there
/// a *finished* run, and how did it end", never "is the newest run done".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskFact {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<RunFact>,
}

/// Enough about one of a workflow's runs for `evaluate`'s `workflow` check
/// to judge it without reading a `WorkflowRun` itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowRunFact {
    pub id: String,
    pub status: WorkflowRunStatus,
    /// A `WorkflowRun` has no `ended_at` of its own -- this is its
    /// `updated_at`, which is exactly its last change and so, once `status`
    /// is terminal, its completion time.
    pub updated_at: DateTime<Utc>,
}

/// The workflow-side twin of [`TaskFact`] -- see it for how
/// `Evidence::workflows`' `Vec` encodes resolution (empty, one, or an
/// ambiguous name) and how `runs` (bounded, newest first) is read: the
/// newest run with a terminal status (`WorkflowRunStatus::is_terminal`),
/// ignoring every one still `Running` ahead of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowFact {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<WorkflowRunFact>,
}

/// One case as it stood in a dataset's newest *settled* bench run
/// (`BenchRun::settled`) -- `gated` is `Case::gate.is_some()` at the moment
/// that run started, and `verdicts` is every attempt's verdict for this case
/// in that run, in no particular order. An ungated case's `verdicts` is
/// still recorded (an attempt's own `Verdict::Unverified` never becomes a
/// `pass`), but `gated: false` is what actually keeps it out of a `gate`
/// check with no `case` named.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateCase {
    pub id: String,
    pub gated: bool,
    pub verdicts: Vec<Verdict>,
}

/// A dataset's newest settled bench run, resolved by the engine for
/// `evaluate`'s `gate` check. `Evidence::gates` keys these by the check's own
/// `dataset` name; a dataset absent from the map has never had one settle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateFact {
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    pub cases: Vec<GateCase>,
}

/// One agent Factory would actually dispatch in the evaluated scope --
/// `Scope::agents_with(&daemon.foreman)`, which folds in a synthesised
/// foreman when the instance turns one on for this scope. It counts: a
/// synthesised foreman is a real agent Factory starts and hands work to,
/// not a hypothetical one, and it happens to be hard-coded `Sandbox::None`
/// (nothing names a sandbox for a foreman nobody wrote), so a scope that
/// enables `daemon.foreman` without giving it one shows up in a `sandbox`
/// check instead of being quietly exempt. See the README's "Policies"
/// section for this written out as the documented choice it is.
///
/// Resolved by the engine (`Engine::agent_facts_for`), never here: `grants`
/// comes from `Engine::roles_for`, a live, in-memory read with no store of
/// its own, but still a lookup only the engine can make.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentFact {
    pub name: String,
    pub role: String,
    /// `None` when `role` names nothing `roles_for` resolves at this scope
    /// -- a role deleted out from under a declared agent, say. Never read
    /// as "holds nothing" (which would silently satisfy `roles`);
    /// `direct_status` reports it `open`, by name, instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grants: Option<BTreeSet<Grant>>,
    /// `false` for `Sandbox::None` -- see the `sandbox` check's own doc.
    pub has_sandbox: bool,
}

/// The `daemon` check's whole fixed vocabulary ([`KNOWN_DAEMON_FACTS`]),
/// resolved once per report by the engine (`Engine::daemon_facts`) rather
/// than per scope -- the daemon's own configuration is the same wherever
/// it is asked from, the same reasoning `Evidence::gates` already uses for
/// datasets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonFact {
    pub foreman_enabled: bool,
    /// `None` when it could not be determined -- a mounted `http`
    /// interface whose `bind` does not parse as a socket address.
    /// `Some(true)` covers both "every mounted `http` interface binds to a
    /// loopback address" and "the daemon mounts no `http` interface at
    /// all": nothing is exposed beyond loopback either way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_loopback_only: Option<bool>,
    pub power_assertion: bool,
}

/// The value [`KNOWN_DAEMON_FACTS`]'s names read off a [`DaemonFact`] --
/// `None` for a name outside that list, which `direct_status` never passes
/// in (it checks membership itself, to give the "not a fact this build
/// knows" reason its own wording), so in practice `None` here only ever
/// means `http_loopback_only`'s own "could not be determined".
fn daemon_fact_value(fact: &str, facts: &DaemonFact) -> Option<bool> {
    match fact {
        "foreman_enabled" => Some(facts.foreman_enabled),
        "http_loopback_only" => facts.http_loopback_only,
        "power_assertion" => Some(facts.power_assertion),
        _ => None,
    }
}

/// Every piece of evidence `evaluate` has to check controls against. `#77`
/// added `tags`/`attestations`; `#81` added `tasks`/`workflows`/`gates`;
/// `#82` adds `agents`, `secrets` and `daemon` -- every field is
/// `#[serde(default)]`, so an `Evidence` built before a field existed is
/// still a valid, if incomplete, one, and no caller has to be updated the
/// moment a new field is added.
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
    /// One entry per distinct `task` name an applicable `task` check names
    /// at the evaluated scope -- see [`TaskFact`].
    #[serde(default)]
    pub tasks: BTreeMap<String, Vec<TaskFact>>,
    /// One entry per distinct `workflow` name an applicable `workflow`
    /// check names at the evaluated scope -- see [`WorkflowFact`].
    #[serde(default)]
    pub workflows: BTreeMap<String, Vec<WorkflowFact>>,
    /// One entry per distinct `dataset` name an applicable `gate` check
    /// names -- see [`GateFact`]. Datasets are instance-wide, not scoped, so
    /// this is the same across every scope a report evaluates.
    #[serde(default)]
    pub gates: BTreeMap<String, GateFact>,
    /// Every agent Factory would dispatch in the evaluated scope -- see
    /// [`AgentFact`]. `None` means "never gathered", distinct from
    /// `Some(vec![])`, an honestly empty scope; `roles`/`sandbox` read it
    /// that way.
    #[serde(default)]
    pub agents: Option<Vec<AgentFact>>,
    /// Whether each of [`KNOWN_SECRETS_LOCATIONS`] a `secrets` check in the
    /// evaluated scope names is present, keyed by that location id -- a
    /// scoped slice of the L2 Secrets tab's own inventory
    /// (`Engine::credential_inventory`). A location absent from this map was
    /// never asked about, not confirmed absent -- see `direct_status`'s
    /// `secrets` arm.
    #[serde(default)]
    pub secrets: BTreeMap<String, bool>,
    /// The daemon-config facts `Check::Daemon` can name -- see
    /// [`DaemonFact`]. `None` means "never gathered".
    #[serde(default)]
    pub daemon: Option<DaemonFact>,
}

// =============================================================== evaluate

/// A control's status carries its own reasons, one per check that
/// contributed to it (including a check whose evidence was never gathered,
/// or whose `daemon`/`secrets` name this build does not recognize, whose
/// reason says so) -- so a caller never has to go re-derive why a control is
/// `open` from the checks alone.
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

/// What kind of thing an [`EvidenceRef`] points at -- exactly the id spaces
/// `evaluate` ever has one for. `knowledge` evidence has no ref yet:
/// `Evidence.tags` only knows a tag is present, never which page carries it,
/// so there is nothing cheap to point at until that changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceRefKind {
    Task,
    Run,
    WorkflowRun,
    BenchRun,
    Attestation,
}

/// A machine-readable pointer alongside a status's human `reasons`, so a UI
/// can link straight to the task, run, workflow run, bench run or
/// attestation that made a control what it is. Additive: only ever added
/// where a check's evidence already carries the id, never invented for the
/// occasion.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub kind: EvidenceRefKind,
    pub id: String,
}

impl EvidenceRef {
    fn task(id: impl Into<String>) -> Self {
        Self { kind: EvidenceRefKind::Task, id: id.into() }
    }
    fn run(id: impl Into<String>) -> Self {
        Self { kind: EvidenceRefKind::Run, id: id.into() }
    }
    fn workflow_run(id: impl Into<String>) -> Self {
        Self { kind: EvidenceRefKind::WorkflowRun, id: id.into() }
    }
    fn bench_run(id: impl Into<String>) -> Self {
        Self { kind: EvidenceRefKind::BenchRun, id: id.into() }
    }
    fn attestation(id: impl Into<String>) -> Self {
        Self { kind: EvidenceRefKind::Attestation, id: id.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlStatus {
    pub control: ControlRef,
    pub title: String,
    pub kind: Kind,
    /// Machine-readable pointers alongside `status`'s reasons -- see
    /// [`EvidenceRef`]. Empty whenever nothing behind the status carries an
    /// id yet (a `knowledge`/`roles`/`sandbox`/`secrets`/`daemon` check,
    /// `n/a`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<EvidenceRef>,
    #[serde(flatten)]
    pub status: Status,
}

/// Whether evidence dated `since` is still current under `max_age` at `now`.
/// `None` is the issue's own rule for `task`/`workflow`/`gate`: "controls
/// without a `max_age` treat any successful run as current" -- there is no
/// window to have fallen outside of. Reads `applied.max_age`, the control's
/// *effective* freshness window after every layer's tightening and every
/// check's own `max_age` are already folded in by [`applicable`] -- never a
/// `Check` variant's own `max_age` field, which would silently ignore a
/// scope's `tighten` (see the doc comment on [`Applied::evidence`]).
fn within_max_age(max_age: Option<Duration>, since: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    match max_age {
        None => true,
        Some(max_age) => now - since <= chrono::Duration::hours(max_age.as_hours() as i64),
    }
}

/// `WorkflowRunStatus` has no `as_str` of its own (unlike `RunStatus`) --
/// this is only for a reason string, so a small local match is cheaper than
/// asking serde to round-trip one.
fn workflow_run_status_str(status: WorkflowRunStatus) -> &'static str {
    match status {
        WorkflowRunStatus::Running => "running",
        WorkflowRunStatus::Done => "done",
        WorkflowRunStatus::Failed => "failed",
        WorkflowRunStatus::Cancelled => "cancelled",
    }
}

/// This control's status from its own checks alone, and the refs those
/// checks can point at -- every one of `Check`'s nine kinds now evaluated
/// for real (`knowledge`/`attestation` in v1, `task`/`workflow`/`gate` in
/// `#81`, `roles`/`sandbox`/`secrets`/`daemon` in `#82`). None of the last
/// four carries a ref: nothing behind them is an id a UI could link to
/// (an agent name is not yet one of `EvidenceRefKind`'s kinds, and a
/// daemon/secrets fact is not tied to any one record at all).
fn direct_status(applied: &Applied, evidence: &Evidence, now: DateTime<Utc>) -> (Status, Vec<EvidenceRef>) {
    let mut satisfied = Vec::new();
    let mut satisfied_refs = Vec::new();
    let mut attested = Vec::new();
    let mut attested_refs = Vec::new();
    let mut stale = Vec::new();
    let mut stale_refs = Vec::new();
    let mut open = Vec::new();
    let mut open_refs = Vec::new();

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
                        attested_refs.push(EvidenceRef::attestation(&att.id));
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
                        stale_refs.push(EvidenceRef::attestation(&att.id));
                    } else {
                        open.push("attestation: none recorded".to_string());
                    }
                }
            }
            Check::Task { task: name, .. } => match evidence.tasks.get(name).map(Vec::as_slice) {
                None | Some([]) => {
                    open.push(format!("task: no task named `{name}` in this scope"));
                }
                Some([fact]) => {
                    // A run still in progress never decides this check --
                    // only the newest *terminal* one does, so a nightly
                    // scan's control never reads `open` for the length of
                    // the scan. `runs` is newest first, so the terminal one
                    // is the first entry whose status says so; anything
                    // ahead of it in the list is still running.
                    let in_progress = fact.runs.first().filter(|r| !r.status.is_terminal());
                    let terminal = fact.runs.iter().find(|r| r.status.is_terminal());
                    match terminal {
                        Some(run) if run.status == RunStatus::Done => match run.ended_at {
                            Some(ended_at) => {
                                let refs = [EvidenceRef::task(&fact.id), EvidenceRef::run(&run.id)];
                                if within_max_age(applied.max_age, ended_at, now) {
                                    satisfied.push(format!(
                                        "task: `{name}` ({}) run {} done at {ended_at}",
                                        fact.id, run.id
                                    ));
                                    satisfied_refs.extend(refs);
                                } else {
                                    stale.push(format!(
                                        "task: `{name}` ({}) run {} done at {ended_at}, older than {}",
                                        fact.id,
                                        run.id,
                                        applied.max_age.expect("stale only ever follows a max_age")
                                    ));
                                    stale_refs.extend(refs);
                                }
                            }
                            None => {
                                open.push(format!(
                                    "task: `{name}` ({}) run {} is done but has no recorded end time",
                                    fact.id, run.id
                                ));
                                open_refs.push(EvidenceRef::task(&fact.id));
                                open_refs.push(EvidenceRef::run(&run.id));
                            }
                        },
                        Some(run) => {
                            open.push(format!(
                                "task: `{name}` ({}) newest finished run {} is {}, not done",
                                fact.id,
                                run.id,
                                run.status.as_str()
                            ));
                            open_refs.push(EvidenceRef::task(&fact.id));
                            open_refs.push(EvidenceRef::run(&run.id));
                        }
                        None => match in_progress {
                            Some(run) => {
                                open.push(format!(
                                    "task: `{name}` ({}) run {} in progress, no finished run yet",
                                    fact.id, run.id
                                ));
                                open_refs.push(EvidenceRef::task(&fact.id));
                                open_refs.push(EvidenceRef::run(&run.id));
                            }
                            None => {
                                open.push(format!("task: `{name}` ({}) has never run", fact.id));
                                open_refs.push(EvidenceRef::task(&fact.id));
                            }
                        },
                    }
                }
                Some(candidates) => {
                    let ids: Vec<&str> = candidates.iter().map(|f| f.id.as_str()).collect();
                    open.push(format!(
                        "task: `{name}` matches more than one task's title ({}); name it by id",
                        ids.join(", ")
                    ));
                }
            },
            Check::Workflow { workflow: name, .. } => match evidence.workflows.get(name).map(Vec::as_slice) {
                None | Some([]) => {
                    open.push(format!("workflow: no workflow named `{name}` in this scope"));
                }
                Some([fact]) => {
                    // Same rule as `task`: a run still `Running` never
                    // decides this check, only the newest terminal one does.
                    let in_progress = fact.runs.first().filter(|r| !r.status.is_terminal());
                    let terminal = fact.runs.iter().find(|r| r.status.is_terminal());
                    match terminal {
                        Some(run) if run.status == WorkflowRunStatus::Done => {
                            let refs = [EvidenceRef::workflow_run(&run.id)];
                            if within_max_age(applied.max_age, run.updated_at, now) {
                                satisfied.push(format!(
                                    "workflow: `{name}` ({}) run {} done at {}",
                                    fact.id, run.id, run.updated_at
                                ));
                                satisfied_refs.extend(refs);
                            } else {
                                stale.push(format!(
                                    "workflow: `{name}` ({}) run {} done at {}, older than {}",
                                    fact.id,
                                    run.id,
                                    run.updated_at,
                                    applied.max_age.expect("stale only ever follows a max_age")
                                ));
                                stale_refs.extend(refs);
                            }
                        }
                        Some(run) => {
                            open.push(format!(
                                "workflow: `{name}` ({}) newest finished run {} is {}, not done",
                                fact.id,
                                run.id,
                                workflow_run_status_str(run.status)
                            ));
                            open_refs.push(EvidenceRef::workflow_run(&run.id));
                        }
                        None => match in_progress {
                            Some(run) => {
                                open.push(format!(
                                    "workflow: `{name}` ({}) run {} in progress, no finished run yet",
                                    fact.id, run.id
                                ));
                                open_refs.push(EvidenceRef::workflow_run(&run.id));
                            }
                            None => {
                                open.push(format!("workflow: `{name}` ({}) has never run", fact.id));
                            }
                        },
                    }
                }
                Some(candidates) => {
                    let ids: Vec<&str> = candidates.iter().map(|f| f.id.as_str()).collect();
                    open.push(format!(
                        "workflow: `{name}` matches more than one workflow's name ({}); name it by id",
                        ids.join(", ")
                    ));
                }
            },
            Check::Gate { dataset, case, .. } => match evidence.gates.get(dataset) {
                None => {
                    open.push(format!("gate: dataset `{dataset}` has no settled bench run"));
                }
                Some(fact) => {
                    let refs = [EvidenceRef::bench_run(&fact.run_id)];
                    let passed: Result<(), String> = match case {
                        Some(case_id) => match fact.cases.iter().find(|c| &c.id == case_id) {
                            None => Err(format!(
                                "gate: dataset `{dataset}` case `{case_id}` is not part of bench run {}",
                                fact.run_id
                            )),
                            Some(c) if !c.gated => Err(format!(
                                "gate: dataset `{dataset}` case `{case_id}` has no gate configured (bench run {})",
                                fact.run_id
                            )),
                            Some(c) if !c.verdicts.is_empty() && c.verdicts.iter().all(|v| *v == Verdict::Pass) => Ok(()),
                            Some(_) => Err(format!(
                                "gate: dataset `{dataset}` case `{case_id}` did not pass in bench run {}",
                                fact.run_id
                            )),
                        },
                        None => {
                            let gated: Vec<&GateCase> = fact.cases.iter().filter(|c| c.gated).collect();
                            if gated.is_empty() {
                                Err(format!(
                                    "gate: dataset `{dataset}` has no gated cases in bench run {}",
                                    fact.run_id
                                ))
                            } else {
                                let failing: Vec<&str> = gated
                                    .iter()
                                    .filter(|c| c.verdicts.is_empty() || !c.verdicts.iter().all(|v| *v == Verdict::Pass))
                                    .map(|c| c.id.as_str())
                                    .collect();
                                if failing.is_empty() {
                                    Ok(())
                                } else {
                                    Err(format!(
                                        "gate: dataset `{dataset}` gated case(s) not passing in bench run {}: {}",
                                        fact.run_id,
                                        failing.join(", ")
                                    ))
                                }
                            }
                        }
                    };
                    match passed {
                        Ok(()) => match fact.ended_at {
                            Some(ended_at) => {
                                if within_max_age(applied.max_age, ended_at, now) {
                                    satisfied.push(format!(
                                        "gate: dataset `{dataset}` run {} passed, ended at {ended_at}",
                                        fact.run_id
                                    ));
                                    satisfied_refs.extend(refs);
                                } else {
                                    stale.push(format!(
                                        "gate: dataset `{dataset}` run {} passed, ended at {ended_at}, older than {}",
                                        fact.run_id,
                                        applied.max_age.expect("stale only ever follows a max_age")
                                    ));
                                    stale_refs.extend(refs);
                                }
                            }
                            None => {
                                open.push(format!(
                                    "gate: dataset `{dataset}` run {} passed but has no recorded end time",
                                    fact.run_id
                                ));
                                open_refs.extend(refs);
                            }
                        },
                        Err(reason) => {
                            open.push(reason);
                            open_refs.extend(refs);
                        }
                    }
                }
            },
            Check::Roles { forbid } => match &evidence.agents {
                None => open.push("roles: agents not resolved for this scope".to_string()),
                Some(agents) if agents.is_empty() => {
                    satisfied.push("roles: no agent declared in this scope -- nothing there can hold a grant".to_string());
                }
                Some(agents) => {
                    let mut unresolved = Vec::new();
                    let mut bad = Vec::new();
                    for agent in agents {
                        match &agent.grants {
                            None => unresolved.push(format!(
                                "roles: agent `{}` has role `{}`, which is not defined at this scope",
                                agent.name, agent.role
                            )),
                            Some(grants) => {
                                for grant in forbid {
                                    if grants.contains(grant) {
                                        bad.push(format!(
                                            "roles: agent `{}` (role `{}`) holds `{}`",
                                            agent.name,
                                            agent.role,
                                            grant.as_str()
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    if !unresolved.is_empty() {
                        open.extend(unresolved);
                    } else if !bad.is_empty() {
                        open.extend(bad);
                    } else {
                        satisfied.push(format!(
                            "roles: none of {} agent(s) holds a forbidden grant",
                            agents.len()
                        ));
                    }
                }
            },
            Check::Sandbox => match &evidence.agents {
                None => open.push("sandbox: agents not resolved for this scope".to_string()),
                Some(agents) if agents.is_empty() => {
                    satisfied.push("sandbox: no agent declared in this scope".to_string());
                }
                Some(agents) => {
                    let missing: Vec<&str> = agents.iter().filter(|a| !a.has_sandbox).map(|a| a.name.as_str()).collect();
                    if missing.is_empty() {
                        satisfied.push(format!("sandbox: every agent declares one ({} checked)", agents.len()));
                    } else {
                        open.push(format!("sandbox: no sandbox declared for {}", missing.join(", ")));
                    }
                }
            },
            Check::Secrets { absent } => {
                let names: Vec<String> = if absent.is_empty() {
                    vec!["scope_env".to_string()]
                } else {
                    absent.clone()
                };
                let mut not_resolved = Vec::new();
                let mut present_at = Vec::new();
                for name in &names {
                    match evidence.secrets.get(name) {
                        None => not_resolved.push(name.clone()),
                        Some(true) => present_at.push(name.clone()),
                        Some(false) => {}
                    }
                }
                if !not_resolved.is_empty() {
                    open.push(format!("secrets: not resolved for {}", not_resolved.join(", ")));
                } else if !present_at.is_empty() {
                    open.push(format!("secrets: present at {}", present_at.join(", ")));
                } else {
                    satisfied.push(format!("secrets: absent at {}", names.join(", ")));
                }
            }
            Check::Daemon { fact } => {
                if !KNOWN_DAEMON_FACTS.contains(&fact.as_str()) {
                    open.push(format!(
                        "daemon: `{fact}` is not a fact this build knows -- see the README's \"Policies\" section for the list"
                    ));
                } else {
                    match evidence.daemon.as_ref().and_then(|facts| daemon_fact_value(fact, facts)) {
                        Some(true) => satisfied.push(format!("daemon: `{fact}` holds")),
                        Some(false) => open.push(format!("daemon: `{fact}` does not hold")),
                        None if evidence.daemon.is_none() => {
                            open.push(format!("daemon: not resolved for `{fact}`"));
                        }
                        None => open.push(format!("daemon: `{fact}` could not be determined")),
                    }
                }
            }
        }
    }

    if !satisfied.is_empty() {
        (Status::Satisfied { reasons: satisfied }, satisfied_refs)
    } else if !attested.is_empty() {
        (Status::Attested { reasons: attested }, attested_refs)
    } else if !stale.is_empty() {
        (Status::Stale { reasons: stale }, stale_refs)
    } else if open.is_empty() {
        (
            Status::Open {
                reasons: vec!["no evidence".to_string()],
            },
            Vec::new(),
        )
    } else {
        (Status::Open { reasons: open }, open_refs)
    }
}

/// Every applicable control's status: its own checks, plus one hop of
/// `maps_to` in both directions among controls that are themselves
/// applicable here. Only `satisfied` or `attested` evidence propagates
/// across a link -- stale or open evidence for a mapped control says
/// nothing about this one. Pure: `now` is a parameter, never read off the
/// clock.
///
/// An ambiguous `task`/`workflow` name never reaches here as anything but an
/// `open` control -- see [`evidence_findings`] for the authoring mistake
/// that produced it, which the engine folds into the same findings list
/// `applicable`'s own findings go into.
pub fn evaluate(applied: &[Applied], evidence: &Evidence, now: DateTime<Utc>) -> Vec<ControlStatus> {
    let direct: BTreeMap<&ControlRef, (Status, Vec<EvidenceRef>)> = applied
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
                refs: Vec::new(),
                status: Status::NotApplicable {
                    reasons: vec![format!(
                        "marked not applicable at {}: {}",
                        na.scope, na.rationale
                    )],
                },
            });
            continue;
        }

        let (own_status, own_refs) = &direct[&a.control];
        let mut reasons = own_status.reasons().to_vec();
        let mut refs: BTreeSet<EvidenceRef> = own_refs.iter().cloned().collect();
        let mut best = own_status.kind();

        if let Some(ns) = neighbors.get(&a.control) {
            for neighbor in ns {
                let (neighbor_status, neighbor_refs) = &direct[neighbor];
                match neighbor_status.kind() {
                    StatusKind::Satisfied => {
                        reasons.push(format!("satisfied via {neighbor} (maps_to)"));
                        refs.extend(neighbor_refs.iter().cloned());
                        if StatusKind::Satisfied.rank() > best.rank() {
                            best = StatusKind::Satisfied;
                        }
                    }
                    StatusKind::Attested => {
                        reasons.push(format!("attested via {neighbor} (maps_to)"));
                        refs.extend(neighbor_refs.iter().cloned());
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
            refs: refs.into_iter().collect(),
            status: Status::from_kind(best, reasons),
        });
    }

    out.sort_by(|a, b| a.control.cmp(&b.control));
    out
}

/// The findings resolving `Evidence` can turn up on its own -- currently
/// just a `task`/`workflow` check whose name matched more than one task's
/// title or workflow's name in the evaluated scope. Kept separate from
/// [`evaluate`], which stays a pure function of applicable controls alone
/// with nothing to say about a name no control's checks reference; the
/// caller folds this into the same `Vec<Finding>` [`applicable`]'s own
/// findings go into (`Engine::policy_report`/`policy_control`).
pub fn evidence_findings(evidence: &Evidence, scope: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (name, candidates) in &evidence.tasks {
        if candidates.len() > 1 {
            let ids: Vec<&str> = candidates.iter().map(|f| f.id.as_str()).collect();
            findings.push(Finding {
                kind: FindingKind::AmbiguousCheckTarget,
                subject: scope.to_string(),
                detail: format!(
                    "task check names {name:?}, which matches more than one task's title: {}",
                    ids.join(", ")
                ),
            });
        }
    }
    for (name, candidates) in &evidence.workflows {
        if candidates.len() > 1 {
            let ids: Vec<&str> = candidates.iter().map(|f| f.id.as_str()).collect();
            findings.push(Finding {
                kind: FindingKind::AmbiguousCheckTarget,
                subject: scope.to_string(),
                detail: format!(
                    "workflow check names {name:?}, which matches more than one workflow's name: {}",
                    ids.join(", ")
                ),
            });
        }
    }
    findings.sort_by(|a, b| a.subject.cmp(&b.subject).then(a.kind.cmp(&b.kind)).then(a.detail.cmp(&b.detail)));
    findings
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
                AgentFact { name: "foreman".to_string(), role: "foreman".to_string(), grants: Some(BTreeSet::new()), has_sandbox: false },
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
            secrets: BTreeMap::from([("scope_env".to_string(), false)]),
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
            secrets: BTreeMap::from([("anthropic".to_string(), true), ("ssh".to_string(), false)]),
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
}
