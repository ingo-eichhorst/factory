//! L5's sole live check evaluator. Inputs are command declarations and
//! current L0 facts; no status table or persisted evidence log is introduced.
use crate::conformance::{AttestedRun, ConformanceEvidence, StepEvidence};
use crate::metrics::{self, Better, MetricError, MetricId, MetricValue};
use chrono::{DateTime, Utc};
use factory_kernel::BenchVerdict as Verdict;
pub use factory_kernel::DaemonConfigFact as DaemonFact;
pub use factory_kernel::{
    AgentFact, ControlRef, Duration, GateCase, GateFact, RunFact, TaskFact, WorkflowFact,
    WorkflowRunFact,
};
use factory_kernel::{
    Attestation, DependenciesFact, Grant, RunStatus, Severity, WorkflowRunStatus,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One thing Factory already records that can stand as evidence for a
/// control. Every kind the ADR names is parsed here and `evaluate` now
/// understands all eleven -- v1 shipped `knowledge` and `attestation`
/// writable ahead of evaluation, and every ticket since (`#81`, `#82`,
/// `#123`, `#158`) taught `evaluate` a few more kinds without ever having to
/// change the file format underneath an author who already wrote one.
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
    /// `catalogue finding`, via [`evidence_findings`].
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
    /// The newest declared and built dependency inventories and their derived
    /// open findings.
    Dependencies {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sbom_max_age: Option<Duration>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        built_sbom: bool,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        max_open: BTreeMap<Severity, u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exploited_open: Option<u32>,
    },
    /// A named fact about the daemon's own configuration holds -- see
    /// [`KNOWN_DAEMON_FACTS`] for the fixed vocabulary this evaluates; any
    /// other name is a `catalogue finding` ([`VocabularyFinding::UnknownDaemonFact`]) and
    /// stays `open`.
    Daemon { fact: String },
    /// Authored monthly caps on this scope and its ancestors, evaluated
    /// against their full subtrees through the L4 spend fact port (#164).
    BudgetWithin,
    /// `#158` phase 1: whether finished runs of `category` actually
    /// conformed to their own control plan's `step`, within `max_age` --
    /// the "did the plant do what it says it does" counterpart to
    /// `attestation`'s "a person said so". Evidence is
    /// `Evidence::attested`, a batch read of finished runs and their
    /// `StepAttestation`s (`Engine::attested_runs`), never re-derived here;
    /// see `direct_status` for the satisfied/stale/open rule and
    /// `factory_core::conformance` for the per-run judging it reuses
    /// (`control_plan::judge`). `max_age` is required -- coverage needs a
    /// window, so leaving it out is a catalogue-wide `ParseFailed` rather
    /// than a silently open control.
    Attested {
        category: String,
        step: String,
        max_age: Duration,
    },
    /// `#278` phase 2: a registry metric (`crate::metrics`, the one Goals,
    /// Quality and Scenarios read) held to one threshold, judged exactly
    /// like a quality `MetricMeasure` -- the bound is inclusive, a value
    /// whose `as_of` is older than the control's effective `max_age` is
    /// `stale`, and a value of `None` is `open` with the metric's own
    /// reason. Exactly one of `above`/`below`, and it has to agree with the
    /// metric's direction: `above` where higher is better, `below` where
    /// lower is -- for a `reported.*` metric, the `better` its scope
    /// declared. Values are read live for the evaluated scope's own subtree
    /// ([`Evidence::metrics`]). A metric computed from L6 verdicts is
    /// refused as circular ([`is_circular_metric`]).
    Metric {
        metric: MetricId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        above: Option<Threshold>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        below: Option<Threshold>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_age: Option<Duration>,
    },
}

/// A `metric` check's bound. `Check` -- and every catalogue type holding
/// one -- derives `Eq`, which a plain `f64` cannot; this compares bit
/// patterns, so equality stays reflexive even for a `.nan` an author wrote
/// (which [`check_vocabulary`] reports and `direct_status` reads as `open`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Threshold(pub f64);

impl PartialEq for Threshold {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}
impl Eq for Threshold {}

impl std::fmt::Display for Threshold {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Whether `metric` is computed from L6 verdicts -- `compliance.<framework>`
/// and `open_controls.<framework>` from policy controls' own statuses,
/// `quality.<characteristic>` from quality scenarios' -- and so can never be
/// a `metric` check's evidence. `compliance.*`/`open_controls.*` would
/// evaluate the very control being judged; `quality.*` is a verdict
/// rollup, not a measurement, and is already refused for quality's own
/// measures (`quality::is_quality_metric`) for the same reason.
pub fn is_circular_metric(metric: &MetricId) -> bool {
    matches!(
        metric.as_str().split('.').next(),
        Some("compliance" | "open_controls" | "quality")
    )
}

/// What a `metric` check found about its own metric: the value read for the
/// evaluated scope, and which way is better -- `None` when nothing
/// declares one (a `reported.*` id no scope declares), so that check never
/// guesses a direction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricEvidence {
    pub value: MetricValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub better: Option<Better>,
}

/// The `daemon` check's fixed vocabulary -- everything else `DaemonConfig`
/// carries is either not a policy-relevant fact or ambiguous enough that
/// "does it hold" would be a guess (ADR 0004's "facts only, no
/// heuristics"). `L6 catalogue loading` checks a `fact` string against this at parse
/// time ([`VocabularyFinding::UnknownDaemonFact`]), so an authoring mistake shows
/// up on the catalogue, not only once a report is evaluated. Moved to the
/// L0 kernel beside [`DaemonFact`] (#193, phase 2) and re-exported here
/// unchanged -- see `factory_kernel::facts`'s own doc comment for the full
/// vocabulary list.
pub use factory_kernel::KNOWN_DAEMON_FACTS;

/// The `secrets` check's fixed vocabulary -- exactly the locations the L2
/// Secrets tab already reports on (`Engine::credential_inventory`): the
/// five machine-wide locations every scope shares (an agent runs as the
/// daemon's owner, so these are the same regardless of scope) plus a
/// scope's own `.env`. Checked at parse time by `L6 catalogue loading`
/// ([`VocabularyFinding::UnknownSecretsLocation`]). Moved to the L0 kernel beside
/// [`SecretsPresence`](factory_kernel::SecretsPresence) (#193, phase 2) and
/// re-exported here unchanged.
pub use factory_kernel::KNOWN_SECRETS_LOCATIONS;

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
            Check::Dependencies { .. } => "dependencies",
            Check::Daemon { .. } => "daemon",
            Check::BudgetWithin => "budget_within",
            Check::Attested { .. } => "attested",
            Check::Metric { .. } => "metric",
        }
    }

    /// This check's own `max_age`, for the kinds that carry one. `attested`
    /// always contributes one -- its field is a required `Duration`, never
    /// `Option`, so `applicable`'s fold always has something to fold in for
    /// a control that carries this check.
    pub fn own_max_age(&self) -> Option<Duration> {
        match self {
            Check::Task { max_age, .. }
            | Check::Workflow { max_age, .. }
            | Check::Gate { max_age, .. }
            | Check::Metric { max_age, .. } => *max_age,
            Check::Dependencies { sbom_max_age, .. } => *sbom_max_age,
            Check::Attested { max_age, .. } => Some(*max_age),
            _ => None,
        }
    }

    /// One line naming what this check asks for -- moved here from
    /// `factory-cli`'s own `describe_check` (`#83`) so the CLI's `policy
    /// show`, a remediation task's own instructions
    /// (`L6 remediation instructions`), and anything else that wants to say
    /// what a check is read the same wording from one place. `policy-model.js`'s
    /// `describeCheck` stays a documented port of this, not an import --
    /// see its own header comment.
    pub fn describe(&self) -> String {
        match self {
            Check::Knowledge { tag: Some(tag) } => format!("knowledge: tag `{tag}`"),
            Check::Knowledge { tag: None } => "knowledge: default tag".to_string(),
            Check::Attestation => "attestation".to_string(),
            Check::Task { task, max_age } => format!(
                "task {task}{}",
                max_age
                    .map(|a| format!(" (max_age {a})"))
                    .unwrap_or_default()
            ),
            Check::Workflow { workflow, max_age } => format!(
                "workflow {workflow}{}",
                max_age
                    .map(|a| format!(" (max_age {a})"))
                    .unwrap_or_default()
            ),
            Check::Gate {
                dataset,
                case,
                max_age,
            } => format!(
                "gate {dataset}{}{}",
                case.as_deref().map(|c| format!("/{c}")).unwrap_or_default(),
                max_age
                    .map(|a| format!(" (max_age {a})"))
                    .unwrap_or_default()
            ),
            Check::Roles { forbid } => format!(
                "roles: forbid {}",
                forbid
                    .iter()
                    .map(|g| g.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Check::Sandbox => "sandbox".to_string(),
            Check::Secrets { absent } => {
                if absent.is_empty() {
                    "secrets".to_string()
                } else {
                    format!("secrets: absent {}", absent.join(", "))
                }
            }
            Check::Daemon { fact } => format!("daemon: {fact}"),
            Check::BudgetWithin => "budget_within: applicable authored monthly USD caps".into(),
            Check::Dependencies {
                sbom_max_age,
                built_sbom,
                max_open,
                exploited_open,
            } => {
                let mut terms = Vec::new();
                if let Some(age) = sbom_max_age {
                    terms.push(format!("SBOM max_age {age}"));
                }
                if *built_sbom {
                    terms.push("built SBOM required".to_string());
                }
                for (severity, limit) in max_open {
                    terms.push(format!("{} <= {limit}", severity.as_str()));
                }
                if let Some(limit) = exploited_open {
                    terms.push(format!("exploited <= {limit}"));
                }
                format!("dependencies: {}", terms.join(", "))
            }
            Check::Attested {
                category,
                step,
                max_age,
            } => {
                format!("attested: {category}/{step} (max_age {max_age})")
            }
            Check::Metric {
                metric,
                above,
                below,
                max_age,
            } => {
                let mut parts = vec![format!("metric {metric}")];
                if let Some(a) = above {
                    parts.push(format!(">= {a}"));
                }
                if let Some(b) = below {
                    parts.push(format!("<= {b}"));
                }
                if let Some(age) = max_age {
                    parts.push(format!("(max_age {age})"));
                }
                parts.join(" ")
            }
        }
    }
}

/// Resolved command input. L6 alone interprets its classification K; L5
/// only returns it unchanged. Remediation and execution requirements are not
/// part of evaluation. The caller has already folded the effective max_age.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvaluationSubject<K = ()> {
    pub control: ControlRef,
    pub title: String,
    pub kind: K,
    pub maps_to: Vec<ControlRef>,
    pub evidence: Vec<Check>,
    pub max_age: Option<Duration>,
    pub not_applicable: Option<Exemption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exemption {
    pub scope: String,
    pub rationale: String,
}

/// Lazy gathering consumes checks and freshness only, not L6 declarations.
pub trait CheckSource {
    fn checks(&self) -> &[Check];
    fn max_age(&self) -> Option<Duration>;
}
impl<K> CheckSource for EvaluationSubject<K> {
    fn checks(&self) -> &[Check] {
        &self.evidence
    }
    fn max_age(&self) -> Option<Duration> {
        self.max_age
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VocabularyFinding {
    UnknownDaemonFact,
    UnknownSecretsLocation,
    BadCheckTarget,
    /// A `metric` check names a metric computed from L6 verdicts
    /// ([`is_circular_metric`]).
    CircularMetric,
    /// A `metric` check names a metric `metrics::resolve` does not know, or
    /// one it knows but cannot compute yet.
    UnknownMetric,
    /// A `metric` check without exactly one of `above`/`below`, or with a
    /// bound that is not a finite number.
    BadThreshold,
    /// A `metric` check's bound runs against its metric's direction:
    /// `above` where lower is better, `below` where higher is.
    WrongDirection,
}

pub use factory_kernel::EvidenceFinding;

pub fn check_vocabulary(check: &Check) -> Vec<(VocabularyFinding, String)> {
    match check {
        Check::Daemon { fact } if !KNOWN_DAEMON_FACTS.contains(&fact.as_str()) => vec![(
            VocabularyFinding::UnknownDaemonFact,
            format!(
                "names daemon fact {fact:?}, which is not one of: {}",
                KNOWN_DAEMON_FACTS.join(", ")
            ),
        )],
        Check::Secrets { absent } => absent
            .iter()
            .filter(|loc| !KNOWN_SECRETS_LOCATIONS.contains(&loc.as_str()))
            .map(|loc| {
                (
                    VocabularyFinding::UnknownSecretsLocation,
                    format!(
                        "names secrets location {loc:?}, which is not one of: {}",
                        KNOWN_SECRETS_LOCATIONS.join(", ")
                    ),
                )
            })
            .collect(),
        Check::Attested { category, step, .. } => {
            let mut findings = Vec::new();
            if !factory_process::control_plan::is_name(category) {
                findings.push((
                    VocabularyFinding::BadCheckTarget,
                    format!("names attested category {category:?}, which is not a name: lowercase letters, digits, '-' and '_', starting with a letter or digit"),
                ));
            }
            if !factory_process::control_plan::is_name(step) {
                findings.push((
                    VocabularyFinding::BadCheckTarget,
                    format!("names attested step {step:?}, which is not a name: lowercase letters, digits, '-' and '_', starting with a letter or digit"),
                ));
            }
            findings
        }
        Check::Metric {
            metric,
            above,
            below,
            ..
        } => metric_check_vocabulary(metric, *above, *below),
        _ => Vec::new(),
    }
}

/// A `metric` check's own load-time findings. The direction is checked
/// here only for a built-in metric: `metrics::resolve`'s `better` for a
/// `reported.*` id is a placeholder with no access to the scope's own
/// declaration, so that one is judged by whoever holds the live
/// declaration (L6's catalogue read) and, always, by `direct_status`.
fn metric_check_vocabulary(
    metric: &MetricId,
    above: Option<Threshold>,
    below: Option<Threshold>,
) -> Vec<(VocabularyFinding, String)> {
    if is_circular_metric(metric) {
        return vec![(
            VocabularyFinding::CircularMetric,
            format!("names metric {metric}, which is computed from policy or quality verdicts and so cannot be evidence for a control"),
        )];
    }
    let mut findings = Vec::new();
    let better = match metrics::resolve(metric) {
        Ok(def) => (!metric.as_str().starts_with("reported.")).then_some(def.better),
        Err(MetricError::Unknown(_)) => {
            findings.push((
                VocabularyFinding::UnknownMetric,
                format!("names unknown metric {metric}"),
            ));
            None
        }
        Err(MetricError::Unavailable { reason, .. }) => {
            findings.push((
                VocabularyFinding::UnknownMetric,
                format!("names metric {metric}, which is not available yet: {reason}"),
            ));
            None
        }
    };
    match threshold_problem(metric, above, below) {
        Some(problem) => findings.push((VocabularyFinding::BadThreshold, problem)),
        None => {
            if let Some(problem) = better.and_then(|b| direction_problem(metric, above, below, b)) {
                findings.push((VocabularyFinding::WrongDirection, problem));
            }
        }
    }
    findings
}

/// Why a `metric` check's bounds cannot judge anything, if they cannot:
/// not exactly one bound, or one that is not a finite number (`.nan`
/// compares false both ways and would read as met every time).
fn threshold_problem(
    metric: &MetricId,
    above: Option<Threshold>,
    below: Option<Threshold>,
) -> Option<String> {
    match (above, below) {
        (None, None) => Some(format!("measures {metric} with neither `above` nor `below`")),
        (Some(_), Some(_)) => Some(format!(
            "measures {metric} with both `above` and `below`; a control's threshold names exactly one"
        )),
        (Some(t), None) | (None, Some(t)) if !t.0.is_finite() => Some(format!(
            "measures {metric} with a bound that is not a finite number"
        )),
        _ => None,
    }
}

/// Why a `metric` check's one bound runs against `better`, if it does.
fn direction_problem(
    metric: &MetricId,
    above: Option<Threshold>,
    below: Option<Threshold>,
    better: Better,
) -> Option<String> {
    match (better, above, below) {
        (Better::Lower, Some(a), _) => Some(format!(
            "holds {metric} `above: {a}`, but lower is better for it -- use `below`"
        )),
        (Better::Higher, _, Some(b)) => Some(format!(
            "holds {metric} `below: {b}`, but higher is better for it -- use `above`"
        )),
        _ => None,
    }
}

/// The value [`KNOWN_DAEMON_FACTS`]'s names read off `evidence` -- outer
/// `None` for a name whose source was never gathered (`evidence.daemon`/
/// `evidence.backup` is `None`), inner `None` for a name that was gathered
/// but could not itself be determined (`http_loopback_only`'s own
/// indeterminate reading, or a `backup_*` fact while the destination is
/// missing or unmounted). `direct_status` never passes in a name outside
/// [`KNOWN_DAEMON_FACTS`] (it checks membership itself, to give the "not a
/// fact this build knows" reason its own wording), so in practice the `_`
/// arm below is unreachable.
fn daemon_fact_value(fact: &str, evidence: &Evidence) -> Option<Option<bool>> {
    match fact {
        "foreman_enabled" => evidence.daemon.map(|facts| Some(facts.foreman_enabled)),
        "http_loopback_only" => evidence.daemon.map(|facts| facts.http_loopback_only),
        "power_assertion" => evidence.daemon.map(|facts| Some(facts.power_assertion)),
        "backup_recent" => evidence.backup.as_ref().map(|facts| facts.recent),
        "backup_offsite" => evidence.backup.as_ref().map(|facts| facts.offsite),
        "backup_verified" => evidence.backup.as_ref().map(|facts| facts.verified),
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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
    /// `secrets` arm. The L0 nominal fact preserves the map's wire shape.
    #[serde(default)]
    pub secrets: factory_kernel::SecretsPresence,
    /// The daemon-config facts `Check::Daemon` can name -- see
    /// [`DaemonFact`]. `None` means "never gathered".
    #[serde(default)]
    pub daemon: Option<DaemonFact>,
    /// Dependency evidence derived from the L2 report. `None` means it was
    /// never gathered, not an empty inventory. Its shared schema is in L0.
    #[serde(default)]
    pub dependencies: Option<DependenciesFact>,
    /// `#154`: the `backup_recent`/`backup_offsite`/`backup_verified` names
    /// `Check::Daemon` can also mean -- see [`factory_kernel::BackupFact`].
    /// `None` means "never gathered", the same as `daemon`; gathered lazily,
    /// only when some applicable control names a `backup_*` fact. Moved to
    /// the L0 kernel and re-exported here unchanged (#193, phase 2).
    #[serde(default)]
    pub backup: Option<factory_kernel::BackupFact>,
    /// `#158`: every finished run `Engine::attested_runs` resolved for the
    /// evaluated scope, across whichever categories some applicable
    /// `attested` check names -- see [`Check::Attested`]. `None` means
    /// "never gathered", the same as `daemon`/`backup`; gathered lazily,
    /// only when some applicable control names one.
    #[serde(default)]
    pub attested: Option<Vec<AttestedRun>>,
    /// Authored configuration goes down; raw spend stays an L4 fact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<crate::budget::PolicyInput>,
    /// `#278` phase 2: every metric an applicable `metric` check names,
    /// read live for the evaluated scope's own subtree by
    /// `metrics_service::Service::check_metrics` -- see [`Check::Metric`].
    /// `None` means "never gathered", the same as `daemon`/`attested`;
    /// gathered lazily, only when some applicable control names one. A
    /// circular or unknown id is never asked for, so it is never a key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<BTreeMap<MetricId, MetricEvidence>>,
}

// =============================================================== evaluate

pub use factory_kernel::{
    EvidenceRef, EvidenceRefKind, EvaluationResult, Status, StatusKind,
};

// Precedence and result construction are private L5 evaluation behaviour.
// Keeping these local traits preserves the evaluator's existing call sites.
trait StatusOrder {
    fn rank(self) -> u8;
}
impl StatusOrder for StatusKind {
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
trait BuildStatus {
    fn from_kind(kind: StatusKind, reasons: Vec<String>) -> Status;
}
impl BuildStatus for Status {
    fn from_kind(kind: StatusKind, reasons: Vec<String>) -> Status {
        match kind {
            StatusKind::Satisfied => Status::Satisfied { reasons },
            StatusKind::Attested => Status::Attested { reasons },
            StatusKind::Stale => Status::Stale { reasons },
            StatusKind::Open => Status::Open { reasons },
            StatusKind::NotApplicable => Status::NotApplicable { reasons },
        }
    }
}

/// Whether evidence dated `since` is still current under `max_age` at `now`.
/// `None` is the issue's own rule for `task`/`workflow`/`gate`: "controls
/// without a `max_age` treat any successful run as current" -- there is no
/// window to have fallen outside of. Reads `applied.max_age`, the control's
/// *effective* freshness window after every layer's tightening and every
/// check's own `max_age` are already folded in by `L6 applicability folding` -- never a
/// `Check` variant's own `max_age` field, which would silently ignore a
/// scope's `tighten` (see the doc comment on [`EvaluationSubject::evidence`]).
fn within_max_age(max_age: Option<Duration>, since: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    match max_age {
        None => true,
        Some(max_age) => now - since <= max_age.as_time_delta(),
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
/// checks can point at -- every one of `Check`'s eleven kinds now evaluated
/// for real (`knowledge`/`attestation` in v1, `task`/`workflow`/`gate` in
/// `#81`, `roles`/`sandbox`/`secrets`/`daemon` in `#82`, `dependencies` in
/// `#123`, `attested` in `#158`). None of `roles`/`sandbox`/`secrets`/`daemon`/
/// `dependencies` carries a ref: nothing behind them is an id a UI could
/// link to (an agent name is not yet one of `EvidenceRefKind`'s kinds, and a
/// daemon/secrets/dependencies fact is not tied to any one record at all).
/// `attested` is the exception among the newer kinds: its evidence is a
/// concrete task and run, so it carries `Task`/`Run` refs the same way
/// `task`/`workflow` do.
fn direct_status<K>(
    applied: &EvaluationSubject<K>,
    evidence: &Evidence,
    now: DateTime<Utc>,
) -> (Status, Vec<EvidenceRef>) {
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
                    // A clock submission (`#157`) is evidence for one
                    // deadline, not for the control as a whole -- skip it
                    // here exactly like a withdrawn row.
                    if att.control != applied.control
                        || att.withdrawn.is_some()
                        || att.clock.is_some()
                        || att.corrective.is_some()
                    {
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
            Check::Workflow { workflow: name, .. } => {
                match evidence.workflows.get(name).map(Vec::as_slice) {
                    None | Some([]) => {
                        open.push(format!(
                            "workflow: no workflow named `{name}` in this scope"
                        ));
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
                                    open.push(format!(
                                        "workflow: `{name}` ({}) has never run",
                                        fact.id
                                    ));
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
                }
            }
            Check::Gate { dataset, case, .. } => match evidence.gates.get(dataset) {
                None => {
                    open.push(format!(
                        "gate: dataset `{dataset}` has no settled bench run"
                    ));
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
            Check::Roles { forbid } => {
                match &evidence.agents {
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
                }
            }
            Check::Sandbox => match &evidence.agents {
                None => open.push("sandbox: agents not resolved for this scope".to_string()),
                Some(agents) if agents.is_empty() => {
                    satisfied.push("sandbox: no agent declared in this scope".to_string());
                }
                Some(agents) => {
                    let missing: Vec<&str> = agents
                        .iter()
                        .filter(|a| !a.has_sandbox)
                        .map(|a| a.name.as_str())
                        .collect();
                    // Declared is what this check has always asked for; the
                    // evidence says which of those declarations dispatch
                    // actually enforces (`#218`), so nobody reads a
                    // declared-only `docker` as a running sandbox.
                    let declared_only: Vec<&str> = agents
                        .iter()
                        .filter(|a| a.has_sandbox && !a.sandbox_enforced)
                        .map(|a| a.name.as_str())
                        .collect();
                    let enforced = agents.iter().filter(|a| a.sandbox_enforced).count();
                    if missing.is_empty() {
                        let mut line = format!(
                            "sandbox: every agent declares one ({} checked, {enforced} enforced",
                            agents.len()
                        );
                        if !declared_only.is_empty() {
                            line.push_str(&format!(
                                "; declared but not enforced: {}",
                                declared_only.join(", ")
                            ));
                        }
                        line.push(')');
                        satisfied.push(line);
                    } else {
                        open.push(format!(
                            "sandbox: no sandbox declared for {}",
                            missing.join(", ")
                        ));
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
                    open.push(format!(
                        "secrets: not resolved for {}",
                        not_resolved.join(", ")
                    ));
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
                    match daemon_fact_value(fact, evidence) {
                        Some(Some(true)) => satisfied.push(format!("daemon: `{fact}` holds")),
                        Some(Some(false)) => open.push(format!("daemon: `{fact}` does not hold")),
                        Some(None) => {
                            open.push(format!("daemon: `{fact}` could not be determined"))
                        }
                        None => open.push(format!("daemon: not resolved for `{fact}`")),
                    }
                }
            }
            Check::BudgetWithin => {
                let (within, reason) = crate::budget::within(evidence.budget.as_ref(), now);
                if within == Some(true) {
                    satisfied.push(reason);
                } else {
                    open.push(reason);
                }
            }
            Check::Dependencies {
                sbom_max_age: _,
                built_sbom,
                max_open,
                exploited_open,
            } => {
                let Some(fact) = &evidence.dependencies else {
                    open.push("dependencies: not resolved for this scope".to_string());
                    continue;
                };
                let mut breaches = Vec::new();
                if let Some(max_age) = applied.max_age {
                    match fact.declared_sbom_at {
                        None => breaches.push("no declared SBOM".to_string()),
                        Some(at) if !within_max_age(Some(max_age), at, now) => breaches
                            .push(format!("declared SBOM from {at} is older than {max_age}")),
                        Some(_) => {}
                    }
                }
                if *built_sbom && fact.built_sbom_at.is_none() {
                    breaches.push("no built SBOM for the newest release".to_string());
                }
                for (severity, limit) in max_open {
                    let actual = fact.open.get(severity).copied().unwrap_or_default();
                    if actual > *limit {
                        breaches.push(format!(
                            "{} open {actual}, maximum {limit}",
                            severity.as_str()
                        ));
                    }
                }
                if let Some(limit) = exploited_open {
                    if fact.exploited_open > *limit {
                        breaches.push(format!(
                            "exploited open {}, maximum {limit}",
                            fact.exploited_open
                        ));
                    }
                }
                if breaches.is_empty() {
                    satisfied
                        .push("dependencies: inventory and findings are within policy".to_string());
                } else {
                    open.extend(
                        breaches
                            .into_iter()
                            .map(|reason| format!("dependencies: {reason}")),
                    );
                }
            }
            Check::Attested { category, step, .. } => {
                let Some(runs) = &evidence.attested else {
                    open.push(format!("attested: not resolved for {category}/{step}"));
                    continue;
                };
                // `own_max_age` always sets one for `attested`, so
                // `applicable`'s fold always leaves `applied.max_age`
                // `Some` for a control that carries this check.
                let w = applied
                    .max_age
                    .expect("`attested`'s own `max_age` always sets one");
                let two_w = Duration::from_hours(w.as_hours().saturating_mul(2));
                let name = |r: &AttestedRun| {
                    if r.holds(step) {
                        format!("run {} (task {})", r.run_id, r.task_id)
                    } else {
                        format!(
                            "run {} (task {}), never held to `{step}`",
                            r.run_id, r.task_id
                        )
                    }
                };
                let relevant: Vec<&AttestedRun> = runs
                    .iter()
                    .filter(|r| &r.category == category && r.status == RunStatus::Done)
                    .collect();
                let recent: Vec<&AttestedRun> = relevant
                    .iter()
                    .copied()
                    .filter(|r| within_max_age(Some(w), r.ended_at, now))
                    .collect();
                if !recent.is_empty() {
                    let bad: Vec<&AttestedRun> = recent
                        .iter()
                        .copied()
                        .filter(|r| r.step_evidence(step) != StepEvidence::Passed)
                        .collect();
                    if bad.is_empty() {
                        satisfied.push(format!(
                            "attested: {category}/{step} -- {} run(s) within {w} all passed",
                            recent.len()
                        ));
                        for r in &recent {
                            satisfied_refs.push(EvidenceRef::task(&r.task_id));
                            satisfied_refs.push(EvidenceRef::run(&r.run_id));
                        }
                    } else {
                        let names: Vec<String> = bad.iter().take(3).map(|r| name(r)).collect();
                        open.push(format!(
                            "attested: {category}/{step} -- {} of {} run(s) within {w} did not pass: {}",
                            bad.len(),
                            recent.len(),
                            names.join("; ")
                        ));
                        for r in &bad {
                            open_refs.push(EvidenceRef::task(&r.task_id));
                            open_refs.push(EvidenceRef::run(&r.run_id));
                        }
                    }
                } else {
                    let stale_candidates: Vec<&AttestedRun> = relevant
                        .iter()
                        .copied()
                        .filter(|r| {
                            let age = now - r.ended_at;
                            age > w.as_time_delta() && age <= two_w.as_time_delta()
                        })
                        .collect();
                    match stale_candidates.iter().copied().max_by_key(|r| r.ended_at) {
                        Some(newest) if newest.step_evidence(step) == StepEvidence::Passed => {
                            stale.push(format!(
                                "attested: {category}/{step} -- newest run {} passed at {}, older than {w}",
                                name(newest),
                                newest.ended_at
                            ));
                            stale_refs.push(EvidenceRef::task(&newest.task_id));
                            stale_refs.push(EvidenceRef::run(&newest.run_id));
                        }
                        Some(newest) => {
                            open.push(format!(
                                "attested: {category}/{step} -- newest run within {two_w} did not pass: {}",
                                name(newest)
                            ));
                            open_refs.push(EvidenceRef::task(&newest.task_id));
                            open_refs.push(EvidenceRef::run(&newest.run_id));
                        }
                        None => {
                            open.push(format!(
                                "attested: {category}/{step} -- no run of this category ended done within {two_w}"
                            ));
                        }
                    }
                }
            }
            Check::Metric {
                metric,
                above,
                below,
                ..
            } => match metric_status(metric, *above, *below, evidence, applied.max_age, now) {
                (StatusKind::Satisfied, reason) => satisfied.push(reason),
                (StatusKind::Stale, reason) => stale.push(reason),
                (_, reason) => open.push(reason),
            },
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

/// One `metric` check's verdict -- `Satisfied`, `Stale` or `Open` -- and
/// its reason, in the order a quality `MetricMeasure` is judged in
/// (`quality::evaluate`), with a control's own refusals ahead of it: a
/// circular metric, a bound that cannot judge, an unknown metric, and a
/// bound against the metric's direction are each `open` whatever the value
/// says. Then `value: None` is `open` with the metric's own reason; an
/// `as_of` older than `max_age` -- the control's effective window -- is
/// `stale` before the bound is even looked at; and the bound is inclusive.
fn metric_status(
    metric: &MetricId,
    above: Option<Threshold>,
    below: Option<Threshold>,
    evidence: &Evidence,
    max_age: Option<Duration>,
    now: DateTime<Utc>,
) -> (StatusKind, String) {
    let open = |reason: String| (StatusKind::Open, format!("metric: {reason}"));
    if is_circular_metric(metric) {
        return open(format!(
            "{metric} is computed from policy or quality verdicts, so it cannot be evidence for a control"
        ));
    }
    if let Some(problem) = threshold_problem(metric, above, below) {
        return open(problem);
    }
    match metrics::resolve(metric) {
        Err(MetricError::Unknown(_)) => return open(format!("{metric} is not a known metric")),
        Err(MetricError::Unavailable { reason, .. }) => {
            return open(format!("{metric} is not available yet: {reason}"))
        }
        Ok(_) => {}
    }
    let Some(read) = evidence.metrics.as_ref() else {
        return open(format!("not resolved for {metric}"));
    };
    let Some(found) = read.get(metric) else {
        return open(format!("no value for {metric}"));
    };
    if let Some(problem) = found
        .better
        .and_then(|better| direction_problem(metric, above, below, better))
    {
        return open(problem);
    }
    let as_of = found.value.as_of;
    let v = match found.value.value {
        Some(v) if v.is_finite() => v,
        Some(v) => {
            return open(format!(
                "{metric} could not be computed: {v} is not a finite number"
            ))
        }
        None => {
            return open(format!(
                "{metric} could not be computed: {}",
                found.value.reason.as_deref().unwrap_or("no reason given")
            ))
        }
    };
    if !within_max_age(max_age, as_of, now) {
        return (
            StatusKind::Stale,
            format!(
                "metric: {metric} = {v} as of {as_of}, older than {}",
                max_age.expect("stale only ever follows a max_age")
            ),
        );
    }
    match (above, below) {
        (Some(a), _) if v < a.0 => open(format!(
            "{metric} = {v} as of {as_of}, below the required {a}"
        )),
        (_, Some(b)) if v > b.0 => open(format!(
            "{metric} = {v} as of {as_of}, above the allowed {b}"
        )),
        (Some(a), _) => (
            StatusKind::Satisfied,
            format!("metric: {metric} = {v} as of {as_of}, at or above {a}"),
        ),
        (_, Some(b)) => (
            StatusKind::Satisfied,
            format!("metric: {metric} = {v} as of {as_of}, at or below {b}"),
        ),
        (None, None) => unreachable!("threshold_problem refuses a check with no bound"),
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
pub fn evaluate<K: Copy>(
    applied: &[EvaluationSubject<K>],
    evidence: &Evidence,
    now: DateTime<Utc>,
) -> Vec<EvaluationResult<K>> {
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
            out.push(EvaluationResult {
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

        out.push(EvaluationResult {
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
/// caller folds this into the same `Vec<Finding>` `L6 applicability folding`'s own
/// findings go into (`Engine::policy_report`/`policy_control`).
pub fn evidence_findings(evidence: &Evidence, scope: &str) -> Vec<EvidenceFinding> {
    let mut findings = Vec::new();
    for (name, candidates) in &evidence.tasks {
        if candidates.len() > 1 {
            let ids: Vec<&str> = candidates.iter().map(|f| f.id.as_str()).collect();
            findings.push(EvidenceFinding {
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
            findings.push(EvidenceFinding {
                subject: scope.to_string(),
                detail: format!(
                    "workflow check names {name:?}, which matches more than one workflow's name: {}",
                    ids.join(", ")
                ),
            });
        }
    }
    findings.sort_by(|a, b| a.subject.cmp(&b.subject).then(a.detail.cmp(&b.detail)));
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        "2026-10-16T12:00:00Z".parse().unwrap()
    }

    fn id(metric: &str) -> MetricId {
        MetricId::new(metric).unwrap()
    }

    fn check(metric: &str, above: Option<f64>, below: Option<f64>) -> Check {
        Check::Metric {
            metric: id(metric),
            above: above.map(Threshold),
            below: below.map(Threshold),
            max_age: None,
        }
    }

    fn subject(check: Check, max_age: Option<Duration>) -> EvaluationSubject {
        EvaluationSubject {
            control: "test/metric".parse().unwrap(),
            title: "a metric".into(),
            kind: (),
            maps_to: Vec::new(),
            evidence: vec![check],
            max_age,
            not_applicable: None,
        }
    }

    fn read(metric: &str, value: Option<f64>, better: Option<Better>, age_days: i64) -> Evidence {
        Evidence {
            metrics: Some(BTreeMap::from([(
                id(metric),
                MetricEvidence {
                    value: MetricValue {
                        id: id(metric),
                        value,
                        as_of: now() - chrono::Duration::days(age_days),
                        reason: value.is_none().then(|| "nothing reported".to_string()),
                    },
                    better,
                },
            )])),
            ..Evidence::default()
        }
    }

    fn judge(check: Check, max_age: Option<Duration>, evidence: &Evidence) -> (StatusKind, String) {
        let status = evaluate(&[subject(check, max_age)], evidence, now())
            .pop()
            .unwrap()
            .status;
        (status.kind(), status.reasons().join("; "))
    }

    #[test]
    fn a_metric_check_parses_with_its_own_spelling_and_refuses_unknown_keys() {
        let parsed: Check = serde_yaml_ng::from_str(
            "{ check: metric, metric: reported.finance.beleg_coverage, above: 0.98, max_age: 35d }",
        )
        .unwrap();
        assert_eq!(
            parsed,
            Check::Metric {
                metric: id("reported.finance.beleg_coverage"),
                above: Some(Threshold(0.98)),
                below: None,
                max_age: Some("35d".parse().unwrap()),
            }
        );
        assert_eq!(parsed.kind_name(), "metric");
        assert_eq!(parsed.own_max_age(), Some("35d".parse().unwrap()));
        assert_eq!(
            parsed.describe(),
            "metric reported.finance.beleg_coverage >= 0.98 (max_age 5w)"
        );
        let integer: Check =
            serde_yaml_ng::from_str("{ check: metric, metric: fail_rate, below: 0 }").unwrap();
        assert_eq!(integer, check("fail_rate", None, Some(0.0)));
        assert!(serde_yaml_ng::from_str::<Check>(
            "{ check: metric, metric: fail_rate, below: 0.1, window: 7d }"
        )
        .is_err());
        assert!(
            serde_yaml_ng::from_str::<Check>("{ check: metric, metric: Not-An-Id, below: 1 }")
                .is_err()
        );
    }

    #[test]
    fn a_threshold_is_equal_to_itself_even_when_it_is_not_a_number() {
        assert_eq!(Threshold(f64::NAN), Threshold(f64::NAN));
        assert_ne!(Threshold(0.1), Threshold(0.2));
    }

    #[test]
    fn check_vocabulary_refuses_circular_unknown_shapeless_and_backwards_metric_checks() {
        let kinds = |c: Check| -> Vec<VocabularyFinding> {
            check_vocabulary(&c).into_iter().map(|(k, _)| k).collect()
        };
        for circular in [
            "compliance.cra",
            "open_controls.gobd",
            "quality.reliability",
        ] {
            assert_eq!(
                kinds(check(circular, Some(0.5), None)),
                vec![VocabularyFinding::CircularMetric],
                "{circular}"
            );
        }
        assert_eq!(
            kinds(check("not_a_metric", None, Some(1.0))),
            vec![VocabularyFinding::UnknownMetric]
        );
        assert_eq!(
            kinds(check("fail_rate", None, None)),
            vec![VocabularyFinding::BadThreshold]
        );
        assert_eq!(
            kinds(check("fail_rate", Some(0.0), Some(0.1))),
            vec![VocabularyFinding::BadThreshold]
        );
        assert_eq!(
            kinds(check("fail_rate", None, Some(f64::NAN))),
            vec![VocabularyFinding::BadThreshold]
        );
        // `fail_rate` is lower-is-better, `first_pass_yield` higher.
        assert_eq!(
            kinds(check("fail_rate", Some(0.1), None)),
            vec![VocabularyFinding::WrongDirection]
        );
        assert_eq!(
            kinds(check("first_pass_yield", None, Some(0.8))),
            vec![VocabularyFinding::WrongDirection]
        );
        assert!(kinds(check("fail_rate", None, Some(0.1))).is_empty());
        assert!(kinds(check("first_pass_yield", Some(0.8), None)).is_empty());
        // A reported metric's direction is its scope's to declare, never
        // `resolve`'s placeholder: nothing is judged at this layer.
        assert!(kinds(check("reported.finance.unresolved", None, Some(0.0))).is_empty());
        assert!(kinds(check("reported.finance.coverage", Some(0.9), None)).is_empty());
    }

    #[test]
    fn a_metric_check_reads_inclusive_bounds_and_stale_before_the_bound() {
        let lower = read("fail_rate", Some(0.1), Some(Better::Lower), 2);
        assert_eq!(
            judge(check("fail_rate", None, Some(0.1)), None, &lower).0,
            StatusKind::Satisfied
        );
        let (kind, reason) = judge(check("fail_rate", None, Some(0.05)), None, &lower);
        assert_eq!(kind, StatusKind::Open);
        assert!(reason.starts_with("metric: fail_rate = 0.1"), "{reason}");
        assert!(reason.contains("above the allowed 0.05"), "{reason}");
        // Older than the control's effective window: stale, even though it
        // would not meet the bound either -- quality's own order.
        let day = Some("1d".parse().unwrap());
        assert_eq!(
            judge(check("fail_rate", None, Some(0.05)), day, &lower).0,
            StatusKind::Stale
        );
        assert_eq!(
            judge(
                check("fail_rate", None, Some(0.1)),
                Some("2d".parse().unwrap()),
                &lower
            )
            .0,
            StatusKind::Satisfied
        );
    }

    #[test]
    fn a_metric_check_is_open_with_a_reason_whenever_it_cannot_judge() {
        let open = |c: Check, evidence: &Evidence, needle: &str| {
            let (kind, reason) = judge(c, None, evidence);
            assert_eq!(kind, StatusKind::Open, "{reason}");
            assert!(reason.starts_with("metric: "), "{reason}");
            assert!(reason.contains(needle), "{needle:?} not in {reason:?}");
        };
        let none = Evidence::default();
        open(
            check("fail_rate", None, Some(0.1)),
            &none,
            "not resolved for fail_rate",
        );
        let other = read("scrap_rate", Some(0.0), Some(Better::Lower), 0);
        open(
            check("fail_rate", None, Some(0.1)),
            &other,
            "no value for fail_rate",
        );
        let missing = read("fail_rate", None, Some(Better::Lower), 0);
        open(
            check("fail_rate", None, Some(0.1)),
            &missing,
            "nothing reported",
        );
        let infinite = read("fail_rate", Some(f64::INFINITY), Some(Better::Lower), 0);
        open(
            check("fail_rate", None, Some(0.1)),
            &infinite,
            "not a finite number",
        );
        let fine = read("fail_rate", Some(0.0), Some(Better::Lower), 0);
        open(
            check("fail_rate", Some(0.0), Some(0.1)),
            &fine,
            "both `above` and `below`",
        );
        open(
            check("fail_rate", Some(0.0), None),
            &fine,
            "lower is better",
        );
        open(
            check("not_a_metric", None, Some(0.1)),
            &fine,
            "not a known metric",
        );
        // A circular metric is refused even with a value in hand.
        let circular = read("compliance.cra", Some(1.0), Some(Better::Higher), 0);
        open(
            check("compliance.cra", Some(0.5), None),
            &circular,
            "cannot be evidence",
        );
    }

    #[test]
    fn a_metric_with_no_declared_direction_is_never_judged_backwards() {
        let undeclared = read("reported.demo.x", Some(3.0), None, 0);
        assert_eq!(
            judge(check("reported.demo.x", Some(1.0), None), None, &undeclared).0,
            StatusKind::Satisfied
        );
        assert_eq!(
            judge(check("reported.demo.x", None, Some(5.0)), None, &undeclared).0,
            StatusKind::Satisfied
        );
    }
}
