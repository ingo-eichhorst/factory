//! L5's sole live check evaluator. Inputs are command declarations and
//! current L0 facts; no status table or persisted evidence log is introduced.
use crate::conformance::{AttestedRun, ConformanceEvidence, StepEvidence};
use crate::metrics::{self, MetricId, MetricValue};
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
/// understands all thirteen -- v1 shipped `knowledge` and `attestation`
/// writable ahead of evaluation, and every ticket since (`#81`, `#82`,
/// `#123`, `#158`, `#278`) taught `evaluate` a few more kinds without ever having to
/// change the file format underneath an author who already wrote one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// `#278`: a scope-reported or built-in metric held to inclusive
    /// bounds and a freshness window, judged by the exact same arithmetic
    /// as a Quality `MetricMeasure` ([`metrics::judge_metric`]), so the two
    /// kinds of authored catalogue never drift apart on what "met" or
    /// "stale" means. Unlike Quality's four-way status, nothing here ever
    /// reads a value as satisfied except `met`: `not_met`, `stale` and no
    /// value at all are all `open`, with the reason -- there is no draft
    /// or stale bucket of its own for a control, only "evidence found" or
    /// "none yet" (ADR 0004's "status is only what evidence says").
    /// `compliance.*`, `open_controls.*` and `quality.*` are refused as
    /// circular ([`is_circular_metric`]), at parse time
    /// ([`check_vocabulary`]) and again here, in case a bad one survives
    /// into evaluation anyway.
    Metric {
        metric: MetricId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        above: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        below: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_age: Option<Duration>,
    },
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
            } => format!(
                "metric: {}",
                metrics::describe_bounds(
                    metric,
                    metrics::MetricBounds {
                        above: *above,
                        below: *below,
                        max_age: *max_age,
                    }
                )
            ),
        }
    }
}

/// Metric families this evaluator itself produces from a policy or quality
/// result (`compliance.<framework>`, `open_controls.<framework>`,
/// `quality.<characteristic>`) -- refused on a `Check::Metric` the same way
/// `quality::is_quality_metric` already refuses `quality.*` for a Quality
/// `MetricMeasure`: judging a control by a number this same evaluation
/// produces would be circular, not a measurement (`#278`).
pub fn is_circular_metric(metric: &MetricId) -> bool {
    crate::metrics_service::is_policy_metric(metric.as_str())
        || crate::metrics_service::is_quality_metric(metric.as_str())
}

/// Resolved command input. L6 alone interprets its classification K; L5
/// only returns it unchanged. Remediation and execution requirements are not
/// part of evaluation. The caller has already folded the effective max_age.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// `#278`: a `check: metric` names `compliance.*`, `open_controls.*` or
    /// `quality.*` -- see [`is_circular_metric`].
    CircularMetric,
    /// `#278`: a `check: metric` names a metric id this build has never
    /// heard of.
    UnknownMetric,
    /// `#278`: a `check: metric` names a metric id this build knows but
    /// cannot compute yet.
    UnavailableMetric,
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
        Check::Metric { metric, above, below, .. } => {
            let mut findings = Vec::new();
            if is_circular_metric(metric) {
                findings.push((
                    VocabularyFinding::CircularMetric,
                    format!(
                        "names metric {metric}, which is computed from policy or quality \
                         evaluation itself and would be circular"
                    ),
                ));
            } else {
                match metrics::resolve(metric) {
                    Ok(_) => {}
                    Err(metrics::MetricError::Unknown(_)) => findings.push((
                        VocabularyFinding::UnknownMetric,
                        format!("names unknown metric {metric}"),
                    )),
                    Err(metrics::MetricError::Unavailable { reason, .. }) => findings.push((
                        VocabularyFinding::UnavailableMetric,
                        format!("names metric {metric} which is not available yet: {reason}"),
                    )),
                }
            }
            if above.is_none() && below.is_none() {
                findings.push((
                    VocabularyFinding::BadCheckTarget,
                    format!("names metric {metric} with neither `above` nor `below`"),
                ));
            }
            findings
        }
        _ => Vec::new(),
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
    /// `#278`: every metric id some applicable `Check::Metric` across this
    /// read actually asked for -- computed once, shared across every
    /// scope and control the same read evaluates (`metrics_service::Service::metric_values`).
    /// An id absent from this map was never asked for by anything
    /// applicable here, not confirmed to have no value; `direct_status`'s
    /// `Check::Metric` arm reports that `open`, by name, the same
    /// restraint every other evidence field gets.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metrics: BTreeMap<MetricId, MetricValue>,
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
/// checks can point at -- every one of `Check`'s thirteen kinds now evaluated
/// for real (`knowledge`/`attestation` in v1, `task`/`workflow`/`gate` in
/// `#81`, `roles`/`sandbox`/`secrets`/`daemon` in `#82`, `dependencies` in
/// `#123`, `attested` in `#158`, `metric` in `#278`). None of `roles`/`sandbox`/`secrets`/`daemon`/
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
            } => {
                // Defence in depth: `check_vocabulary` already catches a
                // circular family as a finding when the catalogue loads,
                // but a bad one surviving into evaluation (nothing drops a
                // check over a finding) must never read as evidence.
                if is_circular_metric(metric) {
                    open.push(format!(
                        "metric: {metric} is computed from policy or quality evaluation \
                         itself and would be circular"
                    ));
                } else {
                    match evidence.metrics.get(metric) {
                        None => {
                            open.push(format!("metric: no value for {metric} yet"));
                        }
                        Some(mv) => {
                            let bounds = metrics::MetricBounds {
                                above: *above,
                                below: *below,
                                // `applied.max_age` is the control's own
                                // *effective* window, after every layer's
                                // `tighten` and every check's own `max_age`
                                // are already folded in -- never this
                                // variant's own field, the same rule
                                // `within_max_age`'s doc comment states for
                                // `task`/`workflow`/`gate`.
                                max_age: applied.max_age,
                            };
                            let (judgement, reasons, _) =
                                metrics::judge_metric(metric, bounds, mv, now);
                            let reasons = reasons.into_iter().map(|r| format!("metric: {r}"));
                            match judgement {
                                metrics::MetricJudgement::Met => satisfied.extend(reasons),
                                metrics::MetricJudgement::NotMet
                                | metrics::MetricJudgement::Stale
                                | metrics::MetricJudgement::NoData => open.extend(reasons),
                            }
                        }
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
