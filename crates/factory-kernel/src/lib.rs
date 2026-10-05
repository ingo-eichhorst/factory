//! The L0 kernel: pure vocabulary shared by code at every level, from
//! `factory-core`'s own lower layers up to the daemon (#193, phase 1).
//!
//! Nothing here may depend on any other `factory-*` crate -- that is the
//! whole point of an L0 nothing else can accidentally reach back up
//! through, and `tests/no_factory_dependency.rs` guards it. What lives here
//! is picked the same way: a type or function that several layers already
//! needed the same copy of, moved down to where duplicating it is no longer
//! possible.
//!
//! - [`Duration`] -- a freshness window (`30d`, `12h`, `2w`), formerly
//!   `factory_core::policy::Duration`. `policy.rs` keeps re-exporting it, so
//!   the wire format does not change; everything below L6 now names this
//!   path instead.
//! - [`nearest_rank`] and [`percentile`] -- the one percentile rule every
//!   layer that forecasts or reports agrees on, formerly
//!   `factory_core::scenario::nearest_rank` and `operations::percentile`'s
//!   own inlined sort-and-index.
//! - [`Level`], `L1`..`L6` and [`Fact`] (#193, phase 2) -- a marker per
//!   level and the trait that names a fact's producer. All [`facts`] schemas
//!   live here with their nested vocabulary. Providers and evaluators
//!   remain outside L0; [`FACT_CATALOGUE`] lists every wired live fact.
//! - [`Provide`] -- a typed pull port, with producer ownership and
//!   fact-only response shapes. The host supplies queries and errors.
//! - [`Facts`] and [`Below`] -- strictly upward level reads; [`People`]
//!   identifies page/API composition outside the six-level ladder.

mod duration;
mod check_results;
pub use check_results::{
    CheckEvaluationFact, CheckObservation, EvidenceFinding, EvidenceRef, EvidenceRefKind, EvaluationResult,
    ScopeCheckEvaluation, CheckComparisonFact, ScopeCheckComparison, Status, StatusKind,
};
mod evaluation_receipts;
pub use evaluation_receipts::{
    Attestation, ClockDeadlineKind, ClockItemRef, ClockMark, ControlRef, CorrectiveMeasureMark,
    Withdrawal,
};
pub mod error;
pub use error::{FactoryError, Result};
mod launch;
pub use launch::{LaunchKind, LaunchSpec};
mod schedule;
pub mod schedule_grid;
pub use schedule::{CronSchedule, Schedule};
mod span;
pub use span::{parse_span, Span};
mod session;
pub use session::SessionRef;
mod scope;
pub use scope::{resolve_scope, scope_ancestors, scope_subtree, ScopeIdentity, ScopeNode, ScopeTree};
mod slug;
mod workflow_identity;
pub use slug::is_slug;
pub use workflow_identity::{WorkflowOrigin, WorkflowWorkspace};
mod knowledge_hints;
pub use knowledge_hints::{KnowledgeHints, KnowledgeHit};
mod process_metrics;
mod metric_values;
pub use metric_values::{is_metric_segment, MetricId, MetricValue, MetricValuesFact};
pub use process_metrics::{
    BenchResolutionFact, ProcessMetricFact, ProductionBin, ProductionBucket, ProductionFact, ProductionQuery,
};
mod fact_vocabulary;
pub mod facts;
pub use fact_vocabulary::*;
mod ports;
mod commands;
pub use commands::{CommandPort, Commands, DirectlyBelow, TaskReceipt};
pub use facts::TaskInventoryQuery;
mod provenance;
pub use provenance::*;
mod spend;
mod stats;
mod workspace;
pub use facts::DeploymentPublicationFact;
pub use facts::{DeploymentMirrorFact, DeploymentMirrorPhase, DeploymentMirrorPlan};
pub use facts::{EnvironmentMetricFact, KnowledgeTags, ScopeCapacityFact};
pub use facts::{
    EnvironmentRecoveryFact, RECOVERY_COMMIT_LABEL, RECOVERY_ENVIRONMENT_LABEL,
    RECOVERY_REASON_LABEL,
};
pub use facts::{RecoveryJournalFact, ScriptRecoveryAction, ScriptRecoveryFinish};
pub use facts::{ReleaseBuildFact, ReleaseSbomFact};
pub use spend::{
    CostGroupBy, CostReport, CostRow, DailySpend, FinishedSpend, SpendBasis, SpendFigure, SpendQuery,
    TokenSums,
};
pub use workspace::{WorkspaceLifetime, WorkspaceSpec};
pub mod renewals;
pub use renewals::*;
mod service_evidence;
pub use ports::{Below, FactProvider, FactValue, Facts, People, Provide, Reader};
pub use service_evidence::*;

pub use duration::Duration;
pub use facts::{
    AgentFact, AttestedRun, ConfirmedSecurityReport, DependenciesFact, ExploitedFinding, GateCase,
    GateFact, RunFact, TaskFact, TaskInventoryFact, WorkflowFact, WorkflowRunFact,
};
pub use facts::TaskSnapshotFact;
pub use facts::{SignpostFact, SignpostObservation};
pub use facts::{
    BackupFact, DaemonConfigFact, Fact, FactCatalogueEntry, Level, SecretsPresence, VerifySummary,
    FACT_CATALOGUE, KNOWN_DAEMON_FACTS, KNOWN_SECRETS_LOCATIONS, L1, L2, L3, L4, L5, L6,
};
pub use stats::{nearest_rank, percentile};
