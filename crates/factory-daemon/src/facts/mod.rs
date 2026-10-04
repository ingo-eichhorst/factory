//! Live fact ports (#193 phase 3). The level modules own gathering;
//! this is only typed wiring. The Engine reference is temporary wiring
//! until phase 6 splits services, not an implementation of Provide on Engine.
mod l1;
mod l2;
mod l3;
mod l4;
mod l5;
pub(crate) use l5::{assurance_gate_facts, assurance_knowledge_tags};
mod process_metrics;

use crate::engine::Engine;
use chrono::{DateTime, Utc};
use factory_core::{
    error::{FactoryError, Result},
    operations::Window,
};
use factory_kernel::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    marker::PhantomData,
};

/// Exact scope and requested task/workflow names, not an unbounded history.
#[derive(Clone)]
pub(crate) struct NamedQuery {
    pub scope: String,
    pub names: BTreeSet<String>,
}

/// Exact selection is not subtree expansion: callers pass the scope-model
/// members they need. All preserves instance-wide remediation label reads;
/// Exact retains the single-store lookup used by remediation commands.
pub(crate) enum TaskInventoryQuery {
    All,
    Exact(String),
    Members(BTreeSet<String>),
}

pub(crate) struct AttestedQuery {
    pub scopes: Option<BTreeSet<String>>,
    pub categories: Option<BTreeSet<String>>,
    pub window: Window,
}

pub(crate) struct RecoveryQuery {
    pub scopes: Option<BTreeSet<String>>,
    pub limit: u32,
}

pub(crate) struct ReleaseBuildQuery {
    pub scope: String,
    pub commit: String,
    pub run_id: String,
}
pub(crate) struct ReleaseSbomQuery {
    pub scope: String,
    pub commit: String,
    pub version: Option<String>,
}

pub(crate) struct ProductionQuery {
    pub scope: Option<String>,
    pub now: DateTime<Utc>,
    pub minutes: Option<u32>,
    pub bin: ProductionBin,
}

pub(crate) struct ProcessMetricsQuery {
    pub scope: Option<String>,
    pub now: DateTime<Utc>,
    pub window: Option<factory_core::metrics::MetricsWindow>,
    pub names: BTreeSet<String>,
}

/// Registered producer, with a response constrained to its own fact type.
/// No serialization, Any downcasts or string-keyed provider lookup.
pub(crate) trait Port: Fact + Sized {
    type Query: Sync;
    type Value: FactValue<Self>;
    type Provider<'a>: Provide<Self, Query = Self::Query, Value = Self::Value, Error = FactoryError>;
    fn provider(engine: &Engine) -> Self::Provider<'_>;
}

/// Host wiring for the kernel's checked read boundary. A level must name
/// itself; page/API composition names People rather than borrowing a level.
/// Crate dependency enforcement and service isolation remain separate work.
pub(crate) struct Facts<'a, R: Reader> {
    engine: &'a Engine,
    reader: PhantomData<R>,
}
impl<'a, R: Reader> Facts<'a, R> {
    pub(crate) fn new(engine: &'a Engine) -> Self {
        Self {
            engine,
            reader: PhantomData,
        }
    }
    pub(crate) async fn get<F: Port>(&self, query: &F::Query) -> Result<F::Value>
    where
        F::Producer: Below<R>,
    {
        factory_kernel::Facts::<R>::new().get::<F, _>(&F::provider(self.engine), query).await
    }
}

macro_rules! port {
    ($fact:ty, $provider:ty, $query:ty, $value:ty, $construct:path) => {
        impl Port for $fact {
            type Query = $query;
            type Value = $value;
            type Provider<'a> = $provider;
            fn provider(engine: &Engine) -> Self::Provider<'_> {
                $construct(engine)
            }
        }
    };
    ($fact:ty, $level:ident, $query:ty, $value:ty) => {
        impl Port for $fact {
            type Query = $query;
            type Value = $value;
            type Provider<'a> = $level::Provider<'a>;
            fn provider(engine: &Engine) -> Self::Provider<'_> {
                $level::Provider { engine }
            }
        }
    };
}
port!(DaemonConfigFact, l1, (), DaemonConfigFact);
port!(InfrastructureExpiryFact, factory_infrastructure::expiry_store::ObservationStore, (), InfrastructureExpiryFact, l1::expiry_provider);
port!(RenewalDeclarationsFact, l1, (), RenewalDeclarationsFact);
port!(CredentialExpiryFact, l2, (), CredentialExpiryFact);
port!(ScheduledRunDatesFact, l4, (), ScheduledRunDatesFact);
port!(ProductionFact, l4, ProductionQuery, ProductionFact);
port!(ProcessMetricFact, l4, ProcessMetricsQuery, BTreeMap<String, ProcessMetricFact>);
port!(BenchResolutionFact, factory_assurance::facts::Provider<'a>, String, Option<BenchResolutionFact>, l5::provider);
port!(BackupFact, l1, DateTime<Utc>, BackupFact);
port!(ScopeCapacityFact, l1, (), ScopeCapacityFact);
port!(EnvironmentMetricFact, l1, Option<String>, BTreeMap<String, EnvironmentMetricFact>);
port!(SecretsPresence, l2, BTreeSet<String>, BTreeMap<String, SecretsPresence>);
port!(DependenciesFact, l2, String, DependenciesFact);
port!(SandboxServiceEvidenceFact, l2, String, SandboxServiceEvidenceFact);
port!(ExploitedFinding, l2, String, Vec<ExploitedFinding>);
port!(AgentFact, l3, String, Vec<AgentFact>);
port!(TaskFact, l4, NamedQuery, BTreeMap<String, Vec<TaskFact>>);
port!(TaskInventoryFact, l4, TaskInventoryQuery, Vec<TaskInventoryFact>);
port!(WorkflowFact, l4, NamedQuery, BTreeMap<String, Vec<WorkflowFact>>);
port!(EnvironmentRecoveryFact, l4, RecoveryQuery, Vec<EnvironmentRecoveryFact>);
port!(RecoveryJournalFact, l4, RecoveryQuery, RecoveryJournalFact);
port!(DeploymentPublicationFact, l1, String, Option<DeploymentPublicationFact>);
port!(DeploymentMirrorFact, factory_process::workflow_store::WorkflowStore, String, Vec<DeploymentMirrorFact>, l4::mirror_provider);
port!(ReleaseBuildFact, l4, ReleaseBuildQuery, Option<ReleaseBuildFact>);
port!(ReleaseSbomFact, l2, ReleaseSbomQuery, Vec<ReleaseSbomFact>);
port!(AttestedRun, l4, AttestedQuery, Vec<AttestedRun>);
port!(ArtifactProvenance, factory_process::facts::ProvenanceProvider<'a>, String, Vec<ArtifactProvenance>, l4::provenance_provider);
port!(CostReport, l4, factory_core::usage::SpendQuery, CostReport);
port!(
    ConfirmedSecurityReport,
    l4,
    Option<String>,
    Vec<ConfirmedSecurityReport>
);
port!(GateFact, factory_assurance::facts::Provider<'a>, BTreeSet<String>, BTreeMap<String, GateFact>, l5::provider);
port!(KnowledgeTags, factory_assurance::facts::Provider<'a>, (), KnowledgeTags, l5::provider);

#[cfg(test)]
mod tests {
    use super::*;
    fn registered<F: Port>() {}
    #[test]
    fn isolated_live_fact_wiring_uses_the_actual_physical_owner_types() {
        let _: fn(<GateFact as Port>::Provider<'static>) -> factory_assurance::facts::Provider<'static> = |provider| provider;
        let _: fn(<KnowledgeTags as Port>::Provider<'static>) -> factory_assurance::facts::Provider<'static> = |provider| provider;
        let _: fn(<BenchResolutionFact as Port>::Provider<'static>) -> factory_assurance::facts::Provider<'static> = |provider| provider;
        let _: fn(<InfrastructureExpiryFact as Port>::Provider<'static>) -> factory_infrastructure::expiry_store::ObservationStore = |provider| provider;
        let _: fn(<DeploymentMirrorFact as Port>::Provider<'static>) -> factory_process::workflow_store::WorkflowStore = |provider| provider;
        let _: fn(<ArtifactProvenance as Port>::Provider<'static>) -> factory_process::facts::ProvenanceProvider<'static> = |provider| provider;
        let wiring = include_str!("l5.rs");
        assert!(!wiring.contains("impl Provide") && !wiring.contains("BenchStore::runs") && !wiring.contains("knowledge::index"));
        let l4 = include_str!("l4.rs");
        assert!(!l4.contains("impl Provide<factory_kernel::ArtifactProvenance>") && !l4.contains("impl Provide<factory_kernel::DeploymentMirrorFact>"));
        let owner = include_str!("../../../factory-assurance/src/facts.rs").split("#[cfg(test)]").next().unwrap();
        assert!(!owner.contains("Engine") && !owner.contains("factory_core") && !owner.contains("dyn Fn"));
        let artifact_owner = include_str!("../../../factory-process/src/facts.rs");
        assert!(!artifact_owner.contains("Engine") && !artifact_owner.contains("factory_core") && !artifact_owner.contains("dyn Fn"));
    }
    #[test]
    fn every_catalogued_fact_has_a_typed_producer_owned_port() {
        registered::<ProductionFact>();
        registered::<ProcessMetricFact>();
        registered::<BenchResolutionFact>();
        registered::<DaemonConfigFact>();
        registered::<BackupFact>();
        registered::<ScopeCapacityFact>();
        registered::<EnvironmentMetricFact>();
        registered::<SecretsPresence>();
        registered::<DependenciesFact>();
        registered::<SandboxServiceEvidenceFact>();
        registered::<ExploitedFinding>();
        registered::<AgentFact>();
        registered::<TaskFact>();
        registered::<TaskInventoryFact>();
        registered::<WorkflowFact>();
        registered::<EnvironmentRecoveryFact>();
        registered::<RecoveryJournalFact>();
        registered::<DeploymentPublicationFact>();
        registered::<DeploymentMirrorFact>();
        registered::<ReleaseBuildFact>();
        registered::<ReleaseSbomFact>();
        registered::<AttestedRun>();
        registered::<ArtifactProvenance>();
        registered::<CostReport>();
        registered::<ConfirmedSecurityReport>();
        registered::<GateFact>();
        registered::<KnowledgeTags>();
        let source = include_str!("mod.rs").split("#[cfg(test)]").next().unwrap();
        let mut wired: Vec<_> = source
            .split("port!(")
            .skip(1)
            .map(|rest| rest.trim_start().split(',').next().unwrap().trim())
            .collect();
        let mut catalogue: Vec<_> = FACT_CATALOGUE.iter().map(|e| e.fact).collect();
        wired.sort_unstable();
        catalogue.sort_unstable();
        assert_eq!(wired, catalogue);
    }

    #[test]
    fn live_reads_use_the_kernel_boundary_and_router_facades_are_not_levels() {
        let wiring = include_str!("mod.rs").split("#[cfg(test)]").next().unwrap();
        assert!(wiring.contains("F::Producer: Below<R>"));
        assert!(wiring.contains("factory_kernel::Facts::<R>::new().get::<F, _>"));
        assert!(!wiring.contains("F::provider(self.engine).get(query)"));
        let router = include_str!("../engine.rs").split("#[cfg(test)]").next().unwrap();
        let provenance = router.split("Request::RunProvenance { id } =>").nth(1).unwrap().split("Request::RunApprove").next().unwrap();
        assert!(provenance.contains("Facts::<factory_kernel::People>"));
        let costs = router.split("Request::Costs { group_by, from, to, scope } =>").nth(1).unwrap().split("Request::RunEntries").next().unwrap();
        assert!(costs.contains("Facts::<factory_kernel::People>"));
        let release = include_str!("../environments/releases.rs");
        assert!(release.contains("let reader = Facts::<factory_kernel::People>"));
        let environments = include_str!("../environments/mod.rs").split("#[cfg(test)]").next().unwrap();
        assert!(environments.contains("Facts::<factory_kernel::People>::new(self).get::<factory_kernel::EnvironmentRecoveryFact>"));
    }

    #[test]
    fn spend_consumers_have_no_second_aggregation_or_upward_l6_helper() {
        let budget = include_str!("../budgets.rs").split("#[cfg(test)]").next().unwrap();
        assert!(budget.contains("get::<CostReport>"));
        assert!(budget.contains("budget_policy_input") && budget.contains("month_spend"));
        assert!(!budget.contains("runs_between") && !budget.contains("crate::costs"));
        let costs = include_str!("../costs.rs").split("#[cfg(test)]").next().unwrap();
        assert!(!costs.contains("fn spend(") && !costs.contains("fn costs_report("));
        let producer = include_str!("l4.rs");
        assert!(!producer.contains("policies::subtree_scopes"));
        assert!(producer.contains("impl Provide<CostReport> for Provider"));
        let metrics = include_str!("../metrics.rs").split("#[cfg(test)]").next().unwrap();
        assert!(metrics.contains("get::<factory_kernel::CostReport>") && metrics.contains("SpendBasis::Finished"));
        let branch = metrics.split("} else if matches!(id.as_str(), \"unit_cost\" | \"tokens_per_run\") {").nth(1).unwrap().split("} else if is_usage_metric").next().unwrap();
        assert!(branch.contains("sources.spend") && !branch.contains("usage_value"));
    }

    #[test]
    fn registry_process_and_benchmark_reads_are_producer_owned() {
        let metrics = include_str!("../metrics.rs").split("#[cfg(test)]").next().unwrap();
        for forbidden in ["self.store.", "self.bench.", ".occupancy(", ".production_at("] {
            assert!(!metrics.contains(forbidden), "metrics bypasses a fact port: {forbidden}");
        }
        for fact in ["ProductionFact", "ProcessMetricFact", "BenchResolutionFact"] {
            assert!(metrics.contains(&format!("get::<factory_kernel::{fact}>")));
        }
        let process = include_str!("process_metrics.rs").split("#[cfg(test)]").next().unwrap();
        assert!(process.contains("impl Provide<ProductionFact> for Provider"));
        assert!(process.contains("impl Provide<ProcessMetricFact> for Provider"));
        assert!(!process.contains("crate::metrics") && !process.contains("crate::policies"));
        for file in [
            include_str!("../operations.rs"), include_str!("../intake.rs"),
            include_str!("../quality/mod.rs"), include_str!("../environments/mod.rs"),
            include_str!("../environments/recovery.rs"),
        ] {
            assert!(!file.split("#[cfg(test)]").next().unwrap().contains("crate::policies::subtree_scopes"));
        }
    }

    #[test]
    fn upper_level_task_inventory_reads_cannot_bypass_the_l4_port() {
        for file in [include_str!("../policies/mod.rs"), include_str!("../scenarios/mod.rs")] {
            let production = file.split("#[cfg(test)]").next().unwrap();
            let compact: String = production.chars().filter(|c| !c.is_whitespace()).collect();
            assert!(!compact.contains("self.store"), "upper-level task read bypasses L4");
            assert!(production.contains("get::<TaskInventoryFact>"));
        }
        // The existing quality remediation command returns a full Task and
        // still awaits the phase-5 command ladder. Its read-only report must
        // not use that as an excuse to reach into L4's store.
        let quality = include_str!("../quality/mod.rs").split("pub(crate) async fn quality_remediate").next().unwrap();
        let compact: String = quality.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(!compact.contains("self.store"));
        assert!(quality.contains("Facts::<L5>::new(self)"));
        assert!(quality.contains("get::<TaskInventoryFact>"));
        let producer = include_str!("l4.rs");
        assert!(producer.contains("impl Provide<factory_kernel::TaskInventoryFact> for Provider"));
    }

    #[test]
    fn sandbox_service_evidence_uses_l2_ownership_and_people_composition() {
        let engine = include_str!("../engine.rs");
        let branch = engine.split("Request::Dependencies { scope } =>").nth(1).unwrap()
            .split("Request::DependenciesVex").next().unwrap();
        assert!(branch.contains("Facts::<factory_kernel::People>"));
        assert!(branch.contains("get::<factory_kernel::SandboxServiceEvidenceFact>"));
        let producer = include_str!("../service_observations.rs").split("#[cfg(test)]").next().unwrap();
        assert!(!producer.contains(".store") && !producer.contains("factory_core::policy"));
        assert!(include_str!("l2.rs").contains("impl Provide<factory_kernel::SandboxServiceEvidenceFact> for Provider"));
    }
}
