//! Live fact vocabulary for the six-level ladder (#193 phases 2-3).
//! All schemas are defined in L0; producing levels retain their providers
//! and behavior. Provide defines live ports; Below enforces reader direction.

use crate::fact_vocabulary::*;
use crate::{ArtifactProvenance, CostReport, ProductionFact, ProcessMetricFact, BenchResolutionFact};
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

// ================================================================== levels

/// A level in Factory's six-level ladder (the README's L1-L6), as a
/// zero-sized marker -- never constructed, only named as a [`Fact`]'s
/// `Producer`. Nothing about a level's behaviour lives here, only its
/// identity: the marker is what lets the compiler tell L1's facts apart
/// from L6's. `Below`/`Provide` bound who may read what; the people-side
/// interfaces are readers outside this six-level identity.
pub trait Level {}

/// Infrastructure: the daemon's own process, configuration and mounted
/// interfaces (`factory infra`, the L1 Infrastructure page).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct L1;
impl Level for L1 {}

/// Environment: secrets presence and the dependency/SBOM inventory (the L2
/// Environment page's Secrets and Dependencies tabs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct L2;
impl Level for L2 {}

/// Agent runtime: the roster, its roles, sandboxes and grants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct L3;
impl Level for L3 {}

/// Process: tasks, runs, workflows and intake (the L4 Process page).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct L4;
impl Level for L4 {}

/// Improvement: quality attributes, benchmarks and datasets (the L5
/// Improvement page).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct L5;
impl Level for L5 {}

/// Policy and Direction: controls, goals and scenarios (the L6 Policy and
/// Direction pages).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct L6;
impl Level for L6 {}

// ==================================================================== fact

/// A fact type one level's own state produces, for a level above it to read.
/// The producer owns a `Provide<F>` implementation; readers use a typed
/// Facts handle with a strict Below bound. Level-crate enforcement is separate.
pub trait Fact: Serialize + DeserializeOwned {
    /// The level that derives this fact fresh, live, on every read (ADR
    /// 0004: "status is computed on every read") -- never the level that
    /// merely reads it, and never stored as of-record evidence (that is
    /// `policy::EvidenceRef`'s question, deferred to its own later ADR).
    type Producer: Level;
}

impl Fact for ArtifactProvenance {
    type Producer = L4;
}

impl Fact for CostReport {
    type Producer = L4;
}

impl Fact for ProductionFact {
    type Producer = L4;
}
impl Fact for ProcessMetricFact {
    type Producer = L4;
}
impl Fact for BenchResolutionFact {
    type Producer = L5;
}

// ============================================================ daemon (L1)

/// The `daemon` check's own configuration facts, resolved once per report
/// by the engine (`Engine::daemon_facts`) rather than per scope -- the
/// daemon's own configuration is the same wherever it is asked from, the
/// same reasoning `Evidence::gates` already uses for datasets.
///
/// Named `DaemonConfigFact`, not `DaemonFact`: `factory_core::protocol`
/// already has a `DaemonFacts` (plural -- the `factory infra`/`GET
/// /api/infrastructure` wire payload, a different type for a different
/// purpose, the daemon's whole runtime shape rather than this narrower
/// policy vocabulary). The triage that planned this phase flagged the two
/// names as confusable and left resolving it to phase 2. `policy.rs`
/// re-exports this type as `DaemonFact`, so no L6 caller's source changes;
/// the wire form was never affected either way, since serde never
/// serializes a Rust type's own name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonConfigFact {
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

impl Fact for DaemonConfigFact {
    type Producer = L1;
}

/// Live scope session ceilings and the scheduler's tick, for L4 Line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeCapacityFact {
    pub max_sessions: BTreeMap<String, u32>,
    pub tick_seconds: u64,
}
impl Fact for ScopeCapacityFact {
    type Producer = L1;
}

/// L1 Operations' existing computed figures, without exposing its report
/// or deployment internals to metrics. Unknown values remain unknown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentMetricFact {
    pub name: String,
    pub availability: Option<f64>,
    pub has_slo: bool,
    pub error_budget: Option<f64>,
    pub incidents: usize,
    pub mttr: Option<f64>,
    pub time_to_restore_p50: Option<f64>,
    pub deploy_frequency: Option<f64>,
    pub lead_time_p50: Option<f64>,
    pub change_failure_rate: Option<f64>,
}
impl Fact for EnvironmentMetricFact {
    type Producer = L1;
}

/// Current tags in the L5 knowledge vault, never a persisted status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeTags {
    pub tags: BTreeSet<String>,
}
impl Fact for KnowledgeTags {
    type Producer = L5;
}

/// The `daemon` check's whole fixed vocabulary -- everything else
/// `DaemonConfig` carries is either not a policy-relevant fact or ambiguous
/// enough that "does it hold" would be a guess (ADR 0004's "facts only, no
/// heuristics"). `policy::load_all` checks a `fact` string against this at
/// parse time, so an authoring mistake shows up on the catalogue, not only
/// once a report is evaluated.
///
/// - `foreman_enabled` -- `daemon.foreman.enabled`.
/// - `http_loopback_only` -- every `http` interface the daemon mounts binds
///   to a loopback address, or none is mounted at all.
/// - `power_assertion` -- `daemon.power_assertion`.
/// - `backup_recent` -- `#154`: the newest backup is within its schedule (or
///   the unscheduled yardstick) plus grace -- [`BackupFact::recent`].
/// - `backup_offsite` -- `#154`: the destination is not on the same device
///   as the instance root -- [`BackupFact::offsite`].
/// - `backup_verified` -- `#154`: the newest verification of a snapshot
///   still in the destination passed, within 30 days --
///   [`BackupFact::verified`].
pub const KNOWN_DAEMON_FACTS: &[&str] = &[
    "foreman_enabled",
    "http_loopback_only",
    "power_assertion",
    "backup_recent",
    "backup_offsite",
    "backup_verified",
];

// ============================================================ backup (L1)

/// One verification of a snapshot still present in a backup destination --
/// carried by [`BackupFact::last_verified`] whatever
/// [`BackupFact::verified`] itself decided, so a reader can say why. Not a
/// [`Fact`] of its own: nothing reads a `VerifySummary` except as part of
/// the `BackupFact` it travels inside, so it is not a row of
/// [`FACT_CATALOGUE`] either.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifySummary {
    pub snapshot: String,
    pub at: DateTime<Utc>,
    pub ok: bool,
}

/// The one L1 fact a policy `daemon` check or a registry metric reads about
/// backups (`#154`) -- computed by `Engine::backup_fact(now)` in
/// `factory-daemon`'s `backup` module, off the same captured state
/// `backup_report` reads, via `factory_core::backup::resolve_backup_fact`.
/// Plain data: nothing here reads a clock, a store or a disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupFact {
    /// The instant this fact was derived -- the same "now" a policy report
    /// or a metric call reads every other fact against, and a metric
    /// value's own `as_of`.
    pub at: DateTime<Utc>,
    /// Whether `infrastructure.backup` names a destination at all.
    pub configured: bool,
    /// The newest archive's timestamp -- `None` before the first backup, or
    /// when the destination cannot be listed at all.
    pub newest: Option<DateTime<Utc>>,
    /// Within its schedule (or the unscheduled yardstick) plus grace.
    /// `None` when the destination is missing or unmounted: even "no" would
    /// be a guess about an archive nobody can currently list.
    pub recent: Option<bool>,
    /// The destination is not on the same device as the instance root.
    /// `None` when that cannot be determined -- the destination is
    /// missing, or its device could not be read.
    pub offsite: Option<bool>,
    /// The newest verification of a snapshot still in the destination
    /// passed, within 30 days of `at`. `None` when the destination is
    /// missing or unmounted.
    pub verified: Option<bool>,
    /// The same verification `verified` is read from, whatever it decided
    /// -- present even when `verified` is `Some(false)` (failed, or too
    /// old), so a reader can say why.
    pub last_verified: Option<VerifySummary>,
}

impl Fact for BackupFact {
    type Producer = L1;
}

// =========================================================== secrets (L2)

/// Whether each of [`KNOWN_SECRETS_LOCATIONS`] a `secrets` check names is
/// present, keyed by that location id -- a scoped slice of the L2 Secrets
/// tab's own inventory (`Engine::credential_inventory`). A location absent
/// from this map was never asked about, not confirmed absent.
///
/// A nominal fact with transparent map serialization. Other maps do not
/// accidentally become L2 facts.
///
/// ```compile_fail
/// use factory_kernel::Fact;
/// use std::collections::BTreeMap;
/// fn is_fact<F: Fact>() {}
/// is_fact::<BTreeMap<String, bool>>();
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretsPresence(pub BTreeMap<String, bool>);

impl SecretsPresence {
    pub fn new() -> Self {
        Self::default()
    }
}
impl std::ops::Deref for SecretsPresence {
    type Target = BTreeMap<String, bool>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for SecretsPresence {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl From<BTreeMap<String, bool>> for SecretsPresence {
    fn from(present: BTreeMap<String, bool>) -> Self {
        Self(present)
    }
}
impl FromIterator<(String, bool)> for SecretsPresence {
    fn from_iter<T: IntoIterator<Item = (String, bool)>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl Fact for SecretsPresence {
    type Producer = L2;
}

/// The `secrets` check's fixed vocabulary -- exactly the locations the L2
/// Secrets tab already reports on (`Engine::credential_inventory`): the
/// five machine-wide locations every scope shares (an agent runs as the
/// daemon's owner, so these are the same regardless of scope) plus a
/// scope's own `.env`.
pub const KNOWN_SECRETS_LOCATIONS: &[&str] =
    &["anthropic", "github", "aws", "netrc", "ssh", "scope_env"];

/// Shared RunFact evidence schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunFact {
    pub id: String,
    pub status: RunStatus,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Missing for an in-flight run; readers must not call it finished.
    pub ended_at: Option<DateTime<Utc>>,
}

/// Shared TaskFact evidence schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskFact {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    /// Bounded lookback, newest first. A check ignores non-terminal runs.
    pub runs: Vec<RunFact>,
}

/// A standing task's inventory metadata, not its run history or an L4
/// command response. L4 derives `open` from the task's authoritative status;
/// a blocked/failed task is still open, a done/cancelled task is not.
/// Scope is the persisted scope name. Providers retain store ordering so
/// remediation lookups keep selecting the same task when labels collide.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskInventoryFact {
    pub id: String,
    pub title: String,
    pub scope: String,
    pub open: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

impl Fact for TaskInventoryFact {
    type Producer = L4;
}

/// Shared WorkflowRunFact evidence schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowRunFact {
    pub id: String,
    pub status: WorkflowRunStatus,
    /// Completion time only when the workflow status is terminal.
    pub updated_at: DateTime<Utc>,
}

/// Shared WorkflowFact evidence schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowFact {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<WorkflowRunFact>,
}

/// Shared GateCase evidence schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateCase {
    pub id: String,
    pub gated: bool,
    pub verdicts: Vec<BenchVerdict>,
}

/// Shared GateFact evidence schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateFact {
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    pub cases: Vec<GateCase>,
}

/// Shared AgentFact evidence schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentFact {
    pub name: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Missing means an unresolved role, not a role with no grants.
    pub grants: Option<BTreeSet<Grant>>,
    /// The agent declares a sandbox other than `none`.
    pub has_sandbox: bool,
    /// And dispatch actually puts its runs inside it (`#218`: only
    /// `openshell` today). Declared-only sandboxes (`docker`, `srt`) are
    /// `has_sandbox` without this. Absent in evidence written before it
    /// existed, which reads as not enforced -- what it was.
    #[serde(default)]
    pub sandbox_enforced: bool,
}

impl Fact for TaskFact {
    type Producer = L4;
}
impl Fact for WorkflowFact {
    type Producer = L4;
}

/// An explicit environment recovery is process work, not a deployment or
/// an SLA sample. Its status comes from the workflow and reported task run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentRecoveryFact {
    pub scope: String,
    pub environment: String,
    pub workflow_id: String,
    pub workflow_run_id: String,
    pub status: WorkflowRunStatus,
    pub requested_at: DateTime<Utc>,
    pub requested_by: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunFact>,
}
pub const RECOVERY_ENVIRONMENT_LABEL: &str = "factory.recovery.environment";
pub const RECOVERY_REASON_LABEL: &str = "factory.recovery.reason";
pub const RECOVERY_COMMIT_LABEL: &str = "factory.recovery.expected_commit";
impl Fact for EnvironmentRecoveryFact {
    type Producer = L4;
}

/// An operator/script's reported action, never a fabricated Factory run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptRecoveryAction {
    pub id: String,
    pub scope: String,
    pub environment: String,
    pub source: String,
    pub actor: String,
    pub reason: String,
    pub command: String,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish: Option<ScriptRecoveryFinish>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptRecoveryFinish {
    pub at: DateTime<Utc>,
    pub exit_code: u8,
    /// Script observations, not the environment's declared Factory checks.
    pub local_http: Option<bool>,
    pub network_routes: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// L4 imports immutable offline receipts into its append-only action journal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryJournalFact {
    pub actions: Vec<ScriptRecoveryAction>,
    pub findings: Vec<String>,
}
impl Fact for RecoveryJournalFact {
    type Producer = L4;
}

/// The exact metadata approved for an outbound GitHub deployment mirror.
/// Never contains health-check output, source credentials or private actor text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploymentMirrorPlan {
    pub deployment: String,
    pub scope: String,
    pub repository: String,
    pub environment: String,
    pub commit: String,
    pub state: String,
    pub verified: Option<bool>,
    pub transient: bool,
    pub production: bool,
    pub approval: String,
}

/// L1's publication-safe projection. No private deployment reason or probe output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploymentPublicationFact {
    pub deployment: String,
    pub scope: String,
    pub environment: String,
    pub commit: String,
    pub dirty: bool,
    pub state: String,
    pub verified: Option<bool>,
    pub repository: Option<String>,
    pub transient: bool,
    pub production: bool,
}
impl Fact for DeploymentPublicationFact { type Producer = L1; }
pub use crate::renewals::{InfrastructureExpiryFact, CredentialExpiryFact, ScheduledRunDatesFact, RenewalDeclarationsFact};
impl Fact for InfrastructureExpiryFact { type Producer = L1; }
impl Fact for RenewalDeclarationsFact { type Producer = L1; }
impl Fact for CredentialExpiryFact { type Producer = L2; }
impl Fact for ScheduledRunDatesFact { type Producer = L4; }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentMirrorPhase { Approved, Created, Published, Failed }

/// Append-only L4 receipt for an explicitly approved outbound effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploymentMirrorFact {
    pub id: String,
    pub plan: DeploymentMirrorPlan,
    pub phase: DeploymentMirrorPhase,
    pub at: DateTime<Utc>,
    pub approved_by: String,
    pub remote_id: Option<u64>,
    pub status_id: Option<u64>,
    pub error: Option<String>,
}
impl Fact for DeploymentMirrorFact { type Producer = L4; }

/// The actual producing run and its immutable, source-matching artifacts.
/// A deployment actor alone is never evidence of a build.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleaseBuildFact {
    pub scope: String,
    pub commit: String,
    pub task_id: String,
    pub run: RunFact,
    pub artifacts: Vec<ArtifactProvenance>,
    pub attestations: Vec<StepAttestation>,
}
impl Fact for ReleaseBuildFact {
    type Producer = L4;
}

/// A build-lifecycle SBOM whose own product identity names this exact release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseSbomFact {
    pub scope: String,
    pub commit: String,
    pub version: String,
    pub attachment: Attachment,
}
impl Fact for ReleaseSbomFact {
    type Producer = L2;
}
impl Fact for GateFact {
    type Producer = L5;
}
impl Fact for AgentFact {
    type Producer = L3;
}

/// Live evidence produced by L2.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependenciesFact {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_sbom_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built_sbom_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub open: BTreeMap<Severity, u32>,
    #[serde(default)]
    pub exploited_open: u32,
}

impl Fact for DependenciesFact {
    type Producer = L2;
}

/// Live evidence produced by L2.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExploitedFinding {
    pub scope: String,
    pub vulnerability: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<AffectedComponent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<LifecycleState>,
    /// First qualifying vulnerability document's attachment time, not the
    /// scanner's own clock or a later remediation/confirmation time.
    pub first_seen_at: DateTime<Utc>,
    pub first_document: Attachment,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Only a later not_affected/false_positive judgment excludes a clock.
    pub latest_vex: Option<String>,
    #[serde(default)]
    /// Absence from the newest document does not erase awareness.
    pub reported_now: bool,
}

impl Fact for ExploitedFinding {
    type Producer = L2;
}

/// Live evidence produced by L4.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfirmedSecurityReport {
    pub item: String,
    pub scope: String,
    /// Original receipt time, never the confirmation time.
    pub awareness_at: DateTime<Utc>,
    pub source: IntakeSource,
    pub confirmed_by: String,
    pub confirmed_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Split ancestry; the clock follows confirmed parents to the root.
    pub parent: Option<String>,
}

impl Fact for ConfirmedSecurityReport {
    type Producer = L4;
}

/// Live evidence produced by L4.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestedRun {
    pub run_id: String,
    pub task_id: String,
    /// Canonical exact scope, not rolled up to an ancestor.
    pub scope: String,
    pub category: String,
    pub agent: String,
    pub status: RunStatus,
    pub ended_at: DateTime<Utc>,
    pub fail_kind: Option<FailKind>,
    /// Immutable dispatch-time plan, including the frozen functionary.
    pub required_steps: Vec<RequiredStep>,
    pub attestations: Vec<StepAttestation>,
}

impl Fact for AttestedRun {
    type Producer = L4;
}
// =============================================================== catalogue

/// One row of [`FACT_CATALOGUE`]: a fact's name, the level that produces it,
/// which levels read it today, and where its shared schema lives.
#[derive(Debug, Clone, Copy)]
pub struct FactCatalogueEntry {
    pub fact: &'static str,
    pub producer: &'static str,
    pub readers: &'static [&'static str],
    pub lives_in_kernel: bool,
    pub note: &'static str,
}

/// Every fact behind `policy::Evidence`, whatever module its type actually
/// lives in -- the catalogue the issue's guardrails ask for ("a catalogue
/// test lists fact, producer and readers"). `readers` is documentation, not
/// a compiled reference: the Facts read boundary's Below bound makes
/// a wrong level reader a compile error instead of a comment. All listed schemas
/// and their nested vocabulary now live in L0.
pub const FACT_CATALOGUE: &[FactCatalogueEntry] = &[
    FactCatalogueEntry {
        fact: "ProductionFact", producer: "L4", readers: &["L6 metrics and Scenarios", "People production"],
        lives_in_kernel: true, note: "live production buckets with the producer's rework and scope rules",
    },
    FactCatalogueEntry {
        fact: "ProcessMetricFact", producer: "L4", readers: &["L6 metrics"],
        lives_in_kernel: true, note: "process-owned run, intake, occupancy and goal-task measurements",
    },
    FactCatalogueEntry {
        fact: "BenchResolutionFact", producer: "L5", readers: &["L6 metrics"],
        lives_in_kernel: true, note: "newest settled benchmark counts and immutable evidence timestamps",
    },
    FactCatalogueEntry {
        fact: "RenewalDeclarationsFact", producer: "L1", readers: &["L6 important dates"],
        lives_in_kernel: true, note: "validated authored expiry metadata read live; no credential values (#236)",
    },
    FactCatalogueEntry {
        fact: "InfrastructureExpiryFact", producer: "L1", readers: &["L6 important dates", "People router"],
        lives_in_kernel: true, note: "public TLS certificate expiry metadata; never private keys (#236)",
    },
    FactCatalogueEntry {
        fact: "CredentialExpiryFact", producer: "L2", readers: &["L6 important dates", "People router"],
        lives_in_kernel: true, note: "credential expiry metadata only, never credential contents (#236)",
    },
    FactCatalogueEntry {
        fact: "ScheduledRunDatesFact", producer: "L4", readers: &["L6 important dates"],
        lives_in_kernel: true, note: "authoritative next scheduled attempts and agent dependencies (#236)",
    },
    FactCatalogueEntry {
        fact: "CostReport",
        producer: "L4",
        readers: &["L6 Budget, policy, Scenarios and cost metrics", "People router"],
        lives_in_kernel: true,
        note: "single live spend read with explicit unknown/partial counts (#164)",
    },
    FactCatalogueEntry {
        fact: "ArtifactProvenance",
        producer: "L4",
        readers: &["People router (run provenance)"],
        lives_in_kernel: true,
        note: "append-only release artifact evidence (#158)",
    },
    FactCatalogueEntry {
        fact: "KnowledgeTags",
        producer: "L5",
        readers: &["L6 policy"],
        lives_in_kernel: true,
        note: "live vault index port, phase 3",
    },
    FactCatalogueEntry {
        fact: "ScopeCapacityFact",
        producer: "L1",
        readers: &["L4 Line"],
        lives_in_kernel: true,
        note: "live configuration port, phase 3",
    },
    FactCatalogueEntry {
        fact: "EnvironmentMetricFact",
        producer: "L1",
        readers: &["L6 metrics"],
        lives_in_kernel: true,
        note: "existing Operations figures, phase 3",
    },
    FactCatalogueEntry {
        fact: "AttestedRun",
        producer: "L4",
        readers: &["L5 quality", "L6 policy and metrics"],
        lives_in_kernel: true,
        note: "finished run and immutable verification evidence",
    },
    FactCatalogueEntry {
        fact: "DaemonConfigFact",
        producer: "L1",
        readers: &["L5 quality", "L6 policy (the `daemon` check)"],
        lives_in_kernel: true,
        note: "moved whole in phase 2: no field's type is owned by another level's module",
    },
    FactCatalogueEntry {
        fact: "BackupFact",
        producer: "L1",
        readers: &[
            "L5 quality",
            "L6 policy (the `daemon` check's backup_* names)",
            "L6 metrics (goals/scenario drivers)",
        ],
        lives_in_kernel: true,
        note: "moved whole in phase 2, with VerifySummary",
    },
    FactCatalogueEntry {
        fact: "SecretsPresence",
        producer: "L2",
        readers: &["L5 quality", "L6 policy (the `secrets` check)"],
        lives_in_kernel: true,
        note: "nominal transparent map: its original JSON representation is unchanged",
    },
    FactCatalogueEntry {
        fact: "DependenciesFact",
        producer: "L2",
        readers: &["L5 quality", "L6 policy (the `dependencies` check)"],
        lives_in_kernel: true,
        note: "moved with its nested shared vocabulary in phase 2",
    },
    FactCatalogueEntry {
        fact: "ExploitedFinding",
        producer: "L2",
        readers: &["L6 policy (the CRA Article 14 reporting clock, `#157`)"],
        lives_in_kernel: true,
        note: "moved with its nested shared vocabulary in phase 2",
    },
    FactCatalogueEntry {
        fact: "TaskFact",
        producer: "L4",
        readers: &["L5 quality", "L6 policy (the `task` check)"],
        lives_in_kernel: true,
        note: "moved with its nested shared vocabulary in phase 2",
    },
    FactCatalogueEntry {
        fact: "TaskInventoryFact",
        producer: "L4",
        readers: &["L5 quality remediation inventory", "L6 policy and scenario task inventory"],
        lives_in_kernel: true,
        note: "live standing intent metadata; no run history or task command payload",
    },
    FactCatalogueEntry {
        fact: "WorkflowFact",
        producer: "L4",
        readers: &["L5 quality", "L6 policy (the `workflow` check)"],
        lives_in_kernel: true,
        note: "moved with its nested shared vocabulary in phase 2",
    },
    FactCatalogueEntry {
        fact: "EnvironmentRecoveryFact",
        producer: "L4",
        readers: &["People Operations page facade"],
        lives_in_kernel: true,
        note: "reported recovery workflow/run evidence; never an L1 deployment or metric input",
    },
    FactCatalogueEntry {
        fact: "RecoveryJournalFact",
        producer: "L4",
        readers: &["People Operations page facade"],
        lives_in_kernel: true,
        note: "offline script receipts imported into append-only process action history; not Factory runs",
    },
    FactCatalogueEntry {
        fact: "DeploymentMirrorFact",
        producer: "L4",
        readers: &["People Operations page facade"],
        lives_in_kernel: true,
        note: "explicitly approved GitHub effect receipts, separate from actual deployment outcomes",
    },
    FactCatalogueEntry {
        fact: "DeploymentPublicationFact",
        producer: "L1",
        readers: &["L4 approved deployment publisher"],
        lives_in_kernel: true,
        note: "publication-safe metadata and scope-owned opt-in; excludes credentials, reasons and probe output",
    },
    FactCatalogueEntry {
        fact: "ReleaseBuildFact",
        producer: "L4",
        readers: &["People Operations release-detail facade"],
        lives_in_kernel: true,
        note: "explicitly selected completed producing run with exact scope/source artifact provenance",
    },
    FactCatalogueEntry {
        fact: "ReleaseSbomFact",
        producer: "L2",
        readers: &["People Operations release-detail facade"],
        lives_in_kernel: true,
        note: "immutable build SBOM attachments selected by their exact product commit and optional version",
    },
    FactCatalogueEntry {
        fact: "ConfirmedSecurityReport",
        producer: "L4",
        readers: &["L6 policy (the CRA Article 14 reporting clock, `#157`)"],
        lives_in_kernel: true,
        note: "moved with its nested shared vocabulary in phase 2",
    },
    FactCatalogueEntry {
        fact: "GateFact",
        producer: "L5",
        readers: &["L6 policy (the `gate` check)"],
        lives_in_kernel: true,
        note: "moved with its nested shared vocabulary in phase 2",
    },
    FactCatalogueEntry {
        fact: "AgentFact",
        producer: "L3",
        readers: &["L5 quality", "L6 policy (the `roles`/`sandbox` checks)"],
        lives_in_kernel: true,
        note: "moved with its nested shared vocabulary in phase 2",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Pinned to the exact shape `policy::DaemonFact` serialized as before
    /// this phase -- field names and the `http_loopback_only` `Option`
    /// omitted when `None`, unaffected by the Rust-side rename to
    /// `DaemonConfigFact`.
    #[test]
    fn daemon_config_fact_serializes_exactly_as_daemon_fact_used_to() {
        let with_bind = DaemonConfigFact {
            foreman_enabled: true,
            http_loopback_only: Some(false),
            power_assertion: true,
        };
        assert_eq!(
            serde_json::to_string(&with_bind).unwrap(),
            r#"{"foreman_enabled":true,"http_loopback_only":false,"power_assertion":true}"#
        );

        let never_gathered_bind = DaemonConfigFact {
            foreman_enabled: false,
            http_loopback_only: None,
            power_assertion: false,
        };
        assert_eq!(
            serde_json::to_string(&never_gathered_bind).unwrap(),
            r#"{"foreman_enabled":false,"power_assertion":false}"#
        );
    }

    #[test]
    fn backup_fact_round_trips_through_json_unchanged() {
        let fact = BackupFact {
            at: "2026-09-29T00:00:00Z".parse().unwrap(),
            configured: true,
            newest: Some("2026-09-28T00:00:00Z".parse().unwrap()),
            recent: Some(true),
            offsite: Some(false),
            verified: Some(true),
            last_verified: Some(VerifySummary {
                snapshot: "snap-1".into(),
                at: "2026-09-28T01:00:00Z".parse().unwrap(),
                ok: true,
            }),
        };
        let json = serde_json::to_string(&fact).unwrap();
        let back: BackupFact = serde_json::from_str(&json).unwrap();
        assert_eq!(fact, back);
    }

    #[test]
    fn secrets_presence_is_exactly_a_string_to_bool_map_on_the_wire() {
        let mut presence: SecretsPresence = SecretsPresence::new();
        presence.insert("github".to_string(), true);
        presence.insert("scope_env".to_string(), false);
        assert_eq!(
            serde_json::to_string(&presence).unwrap(),
            r#"{"github":true,"scope_env":false}"#
        );
    }

    #[test]
    fn known_daemon_facts_and_secrets_locations_are_unchanged() {
        assert_eq!(
            KNOWN_DAEMON_FACTS,
            &[
                "foreman_enabled",
                "http_loopback_only",
                "power_assertion",
                "backup_recent",
                "backup_offsite",
                "backup_verified"
            ]
        );
        assert_eq!(
            KNOWN_SECRETS_LOCATIONS,
            &["anthropic", "github", "aws", "netrc", "ssh", "scope_env"]
        );
    }
}
