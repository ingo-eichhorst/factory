//! Live fact ports (#193 phase 3). The level modules own gathering;
//! this is only typed wiring. The Engine reference is temporary wiring
//! until phase 6 splits services, not an implementation of Provide on Engine.
mod l1;
pub(crate) mod checks;
mod l2;
mod l3;
mod l4;
mod l5;
pub(crate) use factory_process::facts::{
    NamedQuery, RecoveryQuery, ReleaseBuildQuery, TaskInventoryQuery,
};
pub(crate) use l1::selected_http_bind;
pub(crate) use l1::environment_provider as infrastructure_environments;
pub(crate) use l2::{
    credentials_provider as environment_credentials,
    dependencies_provider as environment_dependencies,
    provider_declarations as environment_provider_declarations,
};
pub(crate) use l4::{import_recovery_journal, process_security_reports};

use crate::engine::Engine;
use chrono::{DateTime, Utc};
use factory_core::error::{FactoryError, Result};
use factory_kernel::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    marker::PhantomData,
};

pub(crate) use factory_process::measurements::{
    AttestedQuery, ProcessMetricsQuery, ProductionQuery,
};

pub(crate) use factory_environment::dependency_inventory::ReleaseSbomQuery;

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
        factory_kernel::Facts::<R>::new()
            .get::<F, _>(&F::provider(self.engine), query)
            .await
    }
}

/// What a level service is handed to reach the rest of the daemon while provider
/// construction still takes `&Engine` (#193 phase 6; S12 replaces this with the
/// services' own handles). It is bound to the service's level `L`:
/// `provider::<F>()` and `facts()` compile only for facts `L` may read (the sealed
/// `Below` relation), so a service cannot build or read an upward provider through
/// it. The `Engine` is private to this type, so the service also cannot name
/// another level's state. Configuration, the bus and, for L6, the command chain
/// below are the only other things it offers.
pub(crate) struct Wiring<'a, L: Level> {
    engine: &'a Engine,
    wired: factory_kernel::Wired<'a, L, Engine>,
}
impl<L: Level> Clone for Wiring<'_, L> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<L: Level> Copy for Wiring<'_, L> {}
impl<'a, L: Level> Wiring<'a, L> {
    pub(crate) fn new(engine: &'a Engine) -> Self {
        Self { engine, wired: factory_kernel::Wired::new(engine) }
    }
    /// The live configuration snapshot (shared by every level).
    pub(crate) fn snapshot(&self) -> factory_core::config::Factory {
        self.engine.factory_snapshot()
    }
    /// The path of the running `factory` binary (instance-wide, not level state).
    pub(crate) fn factory_bin(&self) -> &'a std::path::Path {
        &self.engine.shared.factory_bin
    }
    /// When this daemon came up (instance-wide).
    pub(crate) fn booted_at(&self) -> chrono::DateTime<chrono::Utc> {
        self.engine.shared.booted_at
    }
    /// The observer bus: events are published here, never read as evidence.
    pub(crate) fn bus(&self) -> &'a factory_core::event::EventBus {
        &self.engine.shared.bus
    }
    /// The live provider for `F`, from a producer below this level.
    pub(crate) fn provider<F: Port>(&self) -> F::Provider<'a>
    where
        F::Producer: Below<L>,
    {
        F::provider(self.wired.reach::<F>())
    }
    /// The checked fact read for this level.
    pub(crate) fn facts(&self) -> Facts<'a, L> {
        Facts::new(self.engine)
    }
}
impl<'a> Wiring<'a, L6> {
    /// The command chain below L6 (L6 -> L5 -> L4 -> L3).
    pub(crate) fn direction<'o>(
        &self,
        observer: &'o crate::commands::CreationObserver,
    ) -> crate::commands::Direction<'o>
    where
        'a: 'o,
    {
        crate::commands::direction(self.engine, observer)
    }
    /// A created task as the legacy wire payload, composed beside the ladder.
    pub(crate) async fn task_snapshot(&self, id: String) -> Result<factory_core::task::Task> {
        crate::commands::task_snapshot(self.engine, id).await
    }
    /// The L5 check-evidence service over this wiring's providers (tests only).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn check_service(
        &self,
        scopes: factory_kernel::ScopeTree,
    ) -> factory_assurance::evidence::Service<'a, checks::Ports<'a>> {
        checks::service(self.engine, scopes)
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
}
port!(
    DaemonConfigFact,
    factory_infrastructure::settings_facts::SettingsProvider,
    (),
    DaemonConfigFact,
    l1::settings_provider
);
port!(
    InfrastructureExpiryFact,
    factory_infrastructure::expiry_store::ObservationStore,
    (),
    InfrastructureExpiryFact,
    l1::expiry_provider
);
port!(
    RenewalDeclarationsFact,
    factory_infrastructure::renewal_declarations::Provider<'a>,
    (),
    RenewalDeclarationsFact,
    l1::renewal_provider
);
port!(
    CredentialExpiryFact,
    factory_environment::credential_expiry::Provider<'a>,
    (),
    CredentialExpiryFact,
    l2::expiry_provider
);
port!(
    ScheduledRunDatesFact,
    factory_process::facts::Provider<'a>,
    (),
    ScheduledRunDatesFact,
    l4::provider
);
port!(
    ProductionFact,
    factory_process::measurements::MeasurementProvider<'a>,
    ProductionQuery,
    ProductionFact,
    l4::measurement_provider
);
port!(ProcessMetricFact, factory_process::measurements::MeasurementProvider<'a>, ProcessMetricsQuery, BTreeMap<String, ProcessMetricFact>, l4::measurement_provider);
port!(
    BenchResolutionFact,
    factory_assurance::facts::Provider<'a>,
    String,
    Option<BenchResolutionFact>,
    l5::provider
);
port!(
    SignpostFact,
    factory_assurance::signposts::Provider<'a, checks::Ports<'a>>,
    factory_assurance::signposts::Read,
    SignpostFact,
    l5::signpost_provider
);
port!(
    MetricValuesFact,
    factory_assurance::metric_values::Provider<'a, checks::Ports<'a>>,
    factory_assurance::metric_values::Read,
    MetricValuesFact,
    l5::metric_provider
);
port!(
    CheckEvaluationFact,
    factory_assurance::check_evaluation::Provider<'a, checks::Ports<'a>>,
    factory_assurance::check_evaluation::Read,
    CheckEvaluationFact,
    l5::check_provider
);
port!(
    CheckComparisonFact,
    factory_assurance::check_evaluation::Provider<'a, checks::Ports<'a>>,
    factory_assurance::check_evaluation::ComparisonRead,
    CheckComparisonFact,
    l5::check_provider
);
port!(
    CompiledPlanFact,
    factory_assurance::plan_service::Provider,
    factory_assurance::plan_service::Read,
    CompiledPlanFact,
    l5::plan_provider
);
port!(
    WorkflowBlueprintFact,
    factory_process::workflow_blueprints::Provider<'a>,
    factory_kernel::WorkflowBlueprintQuery,
    Vec<WorkflowBlueprintFact>,
    l4::blueprint_provider
);
port!(
    FunctionaryRosterFact,
    factory_agents::roster::Provider,
    String,
    FunctionaryRosterFact,
    l3::provider
);
port!(
    WorkflowTargetsFact,
    factory_assurance::workflow_preview::Provider<factory_agents::roster::Provider>,
    WorkflowBlueprintFact,
    WorkflowTargetsFact,
    l5::preview_provider
);
port!(
    WorkflowPreviewFact,
    factory_assurance::workflow_preview::Provider<factory_agents::roster::Provider>,
    factory_assurance::workflow_preview::Read,
    WorkflowPreviewFact,
    l5::preview_provider
);
port!(
    BackupFact,
    factory_infrastructure::backup_facts::Provider<'a>,
    DateTime<Utc>,
    BackupFact,
    l1::backup_provider
);
port!(
    ScopeCapacityFact,
    factory_infrastructure::settings_facts::SettingsProvider,
    (),
    ScopeCapacityFact,
    l1::settings_provider
);
port!(EnvironmentMetricFact, factory_infrastructure::environment_facts::Provider<'a>, Option<String>, BTreeMap<String, EnvironmentMetricFact>, l1::environment_provider);
port!(SecretsPresence, factory_environment::credentials::Provider, BTreeSet<String>, BTreeMap<String, SecretsPresence>, l2::credentials_provider);
port!(
    DependenciesFact,
    factory_environment::dependency_inventory::Provider,
    String,
    DependenciesFact,
    l2::dependencies_provider
);
port!(
    SandboxServiceEvidenceFact,
    factory_environment::service_observations::Provider,
    String,
    SandboxServiceEvidenceFact,
    l2::evidence_provider
);
port!(
    ExploitedFinding,
    factory_environment::dependency_inventory::Provider,
    String,
    Vec<ExploitedFinding>,
    l2::dependencies_provider
);
port!(AgentFact, factory_agents::roster::Provider, String, Vec<AgentFact>, l3::provider);
port!(TaskFact, factory_process::facts::Provider<'a>, NamedQuery, BTreeMap<String, Vec<TaskFact>>, l4::provider);
port!(TaskSnapshotFact, factory_process::facts::Provider<'a>, String, TaskSnapshotFact, l4::provider);
port!(
    TaskInventoryFact,
    factory_process::facts::Provider<'a>,
    TaskInventoryQuery,
    Vec<TaskInventoryFact>,
    l4::provider
);
port!(WorkflowFact, factory_process::facts::Provider<'a>, NamedQuery, BTreeMap<String, Vec<WorkflowFact>>, l4::provider);
port!(
    EnvironmentRecoveryFact,
    factory_process::facts::Provider<'a>,
    RecoveryQuery,
    Vec<EnvironmentRecoveryFact>,
    l4::provider
);
port!(
    RecoveryJournalFact,
    factory_process::facts::Provider<'a>,
    RecoveryQuery,
    RecoveryJournalFact,
    l4::provider
);
port!(
    DeploymentPublicationFact,
    factory_infrastructure::environment_facts::Provider<'a>,
    String,
    Option<DeploymentPublicationFact>,
    l1::environment_provider
);
port!(
    DeploymentMirrorFact,
    factory_process::workflow_store::WorkflowStore,
    String,
    Vec<DeploymentMirrorFact>,
    l4::mirror_provider
);
port!(
    ReleaseBuildFact,
    factory_process::facts::ProvenanceProvider<'a>,
    ReleaseBuildQuery,
    Option<ReleaseBuildFact>,
    l4::provenance_provider
);
port!(
    ReleaseSbomFact,
    factory_environment::dependency_inventory::Provider,
    ReleaseSbomQuery,
    Vec<ReleaseSbomFact>,
    l2::dependencies_provider
);
port!(
    AttestedRun,
    factory_process::measurements::MeasurementProvider<'a>,
    AttestedQuery,
    Vec<AttestedRun>,
    l4::measurement_provider
);
port!(
    ArtifactProvenance,
    factory_process::facts::ProvenanceProvider<'a>,
    String,
    Vec<ArtifactProvenance>,
    l4::provenance_provider
);
port!(
    CostReport,
    factory_process::measurements::MeasurementProvider<'a>,
    factory_core::usage::SpendQuery,
    CostReport,
    l4::measurement_provider
);
port!(
    ConfirmedSecurityReport,
    factory_process::facts::Provider<'a>,
    Option<String>,
    Vec<ConfirmedSecurityReport>,
    l4::provider
);
port!(GateFact, factory_assurance::facts::Provider<'a>, BTreeSet<String>, BTreeMap<String, GateFact>, l5::provider);
port!(
    KnowledgeTags,
    factory_assurance::facts::Provider<'a>,
    (),
    KnowledgeTags,
    l5::provider
);

#[cfg(test)]
mod tests {
    use super::*;
    fn registered<F: Port>() {}
    #[tokio::test]
    async fn l3_constructor_follows_scope_and_role_edits_and_shares_authorization_resolution() {
        use factory_core::role::{Grant, Role};
        let (engine, root) = crate::environments::tests::engine_with("  - name: prod\n");
        let facts = Facts::<People>::new(&engine);
        let mut scope = engine.factory_snapshot().config.scopes[0].clone();
        let id = scope.id.clone();
        scope.agents = serde_yaml_ng::from_str(
            "- name: critic\n  harness: shell\n  role: reviewer\n  sandbox: docker"
        ).unwrap();
        engine.replace_scope(&id, scope.clone());
        let before = facts.get::<AgentFact>(&"company".into()).await.unwrap();
        assert_eq!(before[0].grants, None);
        assert!(before[0].has_sandbox && !before[0].sandbox_enforced);
        engine.replace_instance_roles(serde_yaml_ng::from_str(
            "reviewer:\n  grants: [task.report]"
        ).unwrap());
        let root_roles = facts.get::<AgentFact>(&"company".into()).await.unwrap();
        assert_eq!(root_roles[0].grants, Some(BTreeSet::from([Grant::TaskReport])));
        assert_eq!(root_roles[0].grants,
            Some(engine.roles_for("company").get(&Role::new("reviewer")).unwrap().grants.clone()));
        scope.roles = serde_yaml_ng::from_str("reviewer:\n  grants: []").unwrap();
        scope.agents[0].sandbox = factory_core::config::Sandbox::Openshell;
        scope.name = "renamed".into();
        scope.path = "projects/demo".into();
        engine.replace_scope(&id, scope);
        let edited = facts.get::<AgentFact>(&"demo".into()).await.unwrap();
        assert_eq!(edited[0].grants, Some(BTreeSet::new()));
        assert!(edited[0].has_sandbox && edited[0].sandbox_enforced);
        assert_eq!(edited[0].grants,
            Some(engine.roles_for("renamed").get(&Role::new("reviewer")).unwrap().grants.clone()));
        assert!(matches!(facts.get::<AgentFact>(&"company".into()).await,
            Err(FactoryError::NoSuchScope(_))));
        assert_eq!(facts.get::<AgentFact>(&"projects/demo".into()).await.unwrap(), edited);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn l2_expiry_constructor_reads_an_edited_catalogue_on_the_next_read() {
        let (engine, root) = crate::environments::tests::engine_with("  - name: prod\n");
        let facts = Facts::<People>::new(&engine);
        let declarations = serde_yaml_ng::from_str(
            "- name: catalogue\n  kind: token\n  source: {from: command, run: no-such-credential-program}\n  expires: never"
        ).unwrap();
        engine.replace_instance_secrets(declarations);
        let before = facts.get::<CredentialExpiryFact>(&()).await.unwrap();
        assert!(
            before
                .observations
                .iter()
                .find(|o| o.id == "secret:catalogue")
                .unwrap()
                .no_expiry
        );
        engine.replace_instance_secrets(Vec::new());
        assert!(!facts
            .get::<CredentialExpiryFact>(&())
            .await
            .unwrap()
            .observations
            .iter()
            .any(|o| o.id == "secret:catalogue"));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn l1_constructors_follow_configuration_reload_not_a_boot_roster() {
        let (source, root) = crate::environments::tests::engine_with("  - name: prod\n");
        let mut snapshot = source.factory_snapshot();
        // This models a discovered scope, not the instance-file root alias
        // that deliberately takes precedence over its duplicate in scopes.
        snapshot.config.scope = None;
        let engine = Engine::new(
            snapshot,
            factory_plugins::Registry::with_builtins(),
            source.l4.store.clone(),
            std::path::PathBuf::from("factory"),
            Vec::new(),
        )
        .with_environment_store(source.l1.environments.clone());
        let facts = Facts::<People>::new(&engine);
        let mut scope = engine.factory_snapshot().config.scopes[0].clone();
        let id = scope.id.clone();
        assert!(facts
            .get::<ScopeCapacityFact>(&())
            .await
            .unwrap()
            .max_sessions
            .is_empty());
        assert!(facts
            .get::<EnvironmentMetricFact>(&Some("company".into()))
            .await
            .unwrap()
            .contains_key("prod"));
        scope.max_sessions = Some(2);
        scope.environments.clear();
        engine.replace_scope(&id, scope);
        assert_eq!(
            facts
                .get::<ScopeCapacityFact>(&())
                .await
                .unwrap()
                .max_sessions["company"],
            2
        );
        assert!(facts
            .get::<EnvironmentMetricFact>(&Some("company".into()))
            .await
            .unwrap()
            .is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn isolated_live_fact_wiring_uses_the_actual_physical_owner_types() {
        let _: fn(
            <AgentFact as Port>::Provider<'static>,
        ) -> factory_agents::roster::Provider = |provider| provider;
        let l3 = include_str!("l3.rs");
        assert!(!l3.contains("impl Provide") && !l3.contains("async fn"));
        assert!(!l3.contains("roles_for") && !l3.contains("agents_with"));
        for owner in [
            include_str!("../../../factory-agents/src/roster.rs"),
            include_str!("../../../factory-agents/src/role_chain.rs"),
        ] {
            assert!(!owner.contains("Engine")
                && !owner.contains("factory_core")
                && !owner.contains("factory_composition")
                && !owner.contains("dyn Fn"));
        }
        let _: fn(
            <SecretsPresence as Port>::Provider<'static>,
        ) -> factory_environment::credentials::Provider = |provider| provider;
        let _: fn(
            <DependenciesFact as Port>::Provider<'static>,
        ) -> factory_environment::dependency_inventory::Provider = |provider| provider;
        let _: fn(
            <ExploitedFinding as Port>::Provider<'static>,
        ) -> factory_environment::dependency_inventory::Provider = |provider| provider;
        let _: fn(
            <ReleaseSbomFact as Port>::Provider<'static>,
        ) -> factory_environment::dependency_inventory::Provider = |provider| provider;
        let _: fn(
            <SandboxServiceEvidenceFact as Port>::Provider<'static>,
        ) -> factory_environment::service_observations::Provider = |provider| provider;
        let _: fn(
            <CredentialExpiryFact as Port>::Provider<'static>,
        ) -> factory_environment::credential_expiry::Provider<'static> = |provider| provider;
        let l2 = include_str!("l2.rs");
        assert!(!l2.contains("impl Provide") && !l2.contains("async fn"));
        for owner in [
            include_str!("../../../factory-environment/src/credentials.rs"),
            include_str!("../../../factory-environment/src/credential_expiry.rs"),
            include_str!("../../../factory-environment/src/dependency_inventory.rs"),
            include_str!("../../../factory-environment/src/sandbox_runtime.rs"),
            include_str!("../../../factory-environment/src/service_observations.rs"),
        ] {
            assert!(
                !owner.contains("Engine")
                    && !owner.contains("factory_core")
                    && !owner.contains("dyn Fn")
            );
        }
        let _: fn(
            <DaemonConfigFact as Port>::Provider<'static>,
        ) -> factory_infrastructure::settings_facts::SettingsProvider = |provider| provider;
        let _: fn(
            <ScopeCapacityFact as Port>::Provider<'static>,
        ) -> factory_infrastructure::settings_facts::SettingsProvider = |provider| provider;
        let _: fn(
            <BackupFact as Port>::Provider<'static>,
        ) -> factory_infrastructure::backup_facts::Provider<'static> = |provider| provider;
        let _: fn(
            <EnvironmentMetricFact as Port>::Provider<'static>,
        ) -> factory_infrastructure::environment_facts::Provider<'static> = |provider| provider;
        let _: fn(
            <DeploymentPublicationFact as Port>::Provider<'static>,
        ) -> factory_infrastructure::environment_facts::Provider<'static> = |provider| provider;
        let _: fn(
            <RenewalDeclarationsFact as Port>::Provider<'static>,
        ) -> factory_infrastructure::renewal_declarations::Provider<'static> = |provider| provider;
        let l1 = include_str!("l1.rs");
        assert!(!l1.contains("impl Provide") && !l1.contains("async fn"));
        for owner in [
            include_str!("../../../factory-infrastructure/src/backup_facts.rs"),
            include_str!("../../../factory-infrastructure/src/environment_facts.rs"),
            include_str!("../../../factory-infrastructure/src/renewal_declarations.rs"),
            include_str!("../../../factory-infrastructure/src/settings_facts.rs"),
            include_str!("../../../factory-infrastructure/src/host.rs"),
            include_str!("../../../factory-infrastructure/src/interfaces.rs"),
        ] {
            assert!(
                !owner.contains("Engine")
                    && !owner.contains("factory_core")
                    && !owner.contains("dyn Fn")
            );
        }
        let _: fn(
            <CostReport as Port>::Provider<'static>,
        ) -> factory_process::measurements::MeasurementProvider<'static> = |provider| provider;
        let _: fn(
            <AttestedRun as Port>::Provider<'static>,
        ) -> factory_process::measurements::MeasurementProvider<'static> = |provider| provider;
        let _: fn(
            <ProductionFact as Port>::Provider<'static>,
        ) -> factory_process::measurements::MeasurementProvider<'static> = |provider| provider;
        let _: fn(
            <ProcessMetricFact as Port>::Provider<'static>,
        ) -> factory_process::measurements::MeasurementProvider<'static> = |provider| provider;
        let _: fn(
            <GateFact as Port>::Provider<'static>,
        ) -> factory_assurance::facts::Provider<'static> = |provider| provider;
        let _: fn(
            <KnowledgeTags as Port>::Provider<'static>,
        ) -> factory_assurance::facts::Provider<'static> = |provider| provider;
        let _: fn(
            <BenchResolutionFact as Port>::Provider<'static>,
        ) -> factory_assurance::facts::Provider<'static> = |provider| provider;
        let _: fn(
            <InfrastructureExpiryFact as Port>::Provider<'static>,
        ) -> factory_infrastructure::expiry_store::ObservationStore = |provider| provider;
        let _: fn(
            <DeploymentMirrorFact as Port>::Provider<'static>,
        ) -> factory_process::workflow_store::WorkflowStore = |provider| provider;
        let _: fn(
            <ArtifactProvenance as Port>::Provider<'static>,
        ) -> factory_process::facts::ProvenanceProvider<'static> = |provider| provider;
        let _: fn(
            <ReleaseBuildFact as Port>::Provider<'static>,
        ) -> factory_process::facts::ProvenanceProvider<'static> = |provider| provider;
        let _: fn(
            <TaskInventoryFact as Port>::Provider<'static>,
        ) -> factory_process::facts::Provider<'static> = |provider| provider;
        let _: fn(
            <ScheduledRunDatesFact as Port>::Provider<'static>,
        ) -> factory_process::facts::Provider<'static> = |provider| provider;
        let _: fn(
            <TaskFact as Port>::Provider<'static>,
        ) -> factory_process::facts::Provider<'static> = |provider| provider;
        let _: fn(
            <WorkflowFact as Port>::Provider<'static>,
        ) -> factory_process::facts::Provider<'static> = |provider| provider;
        let _: fn(
            <EnvironmentRecoveryFact as Port>::Provider<'static>,
        ) -> factory_process::facts::Provider<'static> = |provider| provider;
        let _: fn(
            <RecoveryJournalFact as Port>::Provider<'static>,
        ) -> factory_process::facts::Provider<'static> = |provider| provider;
        let _: fn(
            <ConfirmedSecurityReport as Port>::Provider<'static>,
        ) -> factory_process::facts::Provider<'static> = |provider| provider;
        let wiring = include_str!("l5.rs");
        assert!(
            !wiring.contains("impl Provide")
                && !wiring.contains("BenchStore::runs")
                && !wiring.contains("knowledge::index")
        );
        let l4 = include_str!("l4.rs");
        assert!(
            !l4.contains("impl Provide<factory_kernel::ArtifactProvenance>")
                && !l4.contains("impl Provide<factory_kernel::DeploymentMirrorFact>")
        );
        for fact in [
            "TaskInventoryFact",
            "ScheduledRunDatesFact",
            "RecoveryJournalFact",
            "EnvironmentRecoveryFact",
            "ReleaseBuildFact",
            "TaskFact",
            "WorkflowFact",
            "ConfirmedSecurityReport",
            "CostReport",
            "AttestedRun",
            "ProductionFact",
            "ProcessMetricFact",
        ] {
            assert!(
                !l4.contains(&format!("impl Provide<{fact}>"))
                    && !l4.contains(&format!("impl Provide<factory_kernel::{fact}>"))
            );
        }
        let owner = include_str!("../../../factory-assurance/src/facts.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(
            !owner.contains("Engine")
                && !owner.contains("factory_core")
                && !owner.contains("dyn Fn")
        );
        let artifact_owner = include_str!("../../../factory-process/src/facts.rs");
        assert!(
            !artifact_owner.contains("Engine")
                && !artifact_owner.contains("factory_core")
                && !artifact_owner.contains("dyn Fn")
        );
        for owner in [
            include_str!("../../../factory-process/src/measurements.rs"),
            include_str!("../../../factory-process/src/process_metrics.rs"),
            include_str!("../../../factory-process/src/occupancy_history.rs"),
            include_str!("../../../factory-process/src/operations.rs"),
        ] {
            assert!(
                !owner.contains("Engine")
                    && !owner.contains("factory_core")
                    && !owner.contains("dyn Fn")
            );
        }
        let endpoint = include_str!("../production.rs");
        assert!(endpoint.contains("get::<factory_kernel::ProductionFact>"));
        assert!(!endpoint.contains("runs_between") && !endpoint.contains("fn totals"));
        let operations = include_str!("../../../factory-composition/src/operations.rs");
        assert!(operations.contains("pub use factory_process::operations::*;"));
        assert!(!operations.contains("pub fn registry_metric("));
    }
    #[test]
    fn every_catalogued_fact_has_a_typed_producer_owned_port() {
        registered::<ProductionFact>();
        registered::<ProcessMetricFact>();
        registered::<BenchResolutionFact>();
        registered::<SignpostFact>();
        registered::<MetricValuesFact>();
        registered::<CheckEvaluationFact>();
        registered::<CheckComparisonFact>();
        registered::<CompiledPlanFact>();
        registered::<WorkflowBlueprintFact>();
        registered::<FunctionaryRosterFact>();
        registered::<WorkflowTargetsFact>();
        registered::<WorkflowPreviewFact>();
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
        registered::<TaskSnapshotFact>();
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
    fn physical_policy_service_reads_live_check_facts_and_classifies_only_its_own_declarations() {
        let owner = include_str!("../../../factory-direction/src/policy_service.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        let compact: String = owner.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(compact.contains("Facts::<L6>::new().get::<CheckEvaluationFact,_>"));
        assert!(compact.contains("Facts::<L6>::new().get::<TaskInventoryFact,_>"));
        assert!(owner.contains("kind: declaration.kind"));
        for forbidden in [
            "Engine",
            "factory_core",
            "factory_composition",
            "factory_interfaces",
            "factory_daemon",
            "TaskStore",
            "dyn Fn",
            "BoxFuture",
            "policy::evaluate(",
        ] {
            assert!(
                !owner.contains(forbidden),
                "Policy service gained backedge/evaluator {forbidden}"
            );
        }
        let producer = include_str!("../../../factory-assurance/src/check_evaluation.rs");
        for required in [
            "Provide<CheckEvaluationFact>",
            // `#278`: the evidence seam moved behind `metrics_service::Service`
            // (so a `Check::Metric` can share its own lazy metric read with
            // it), never behind an Engine callback or a precomputed report.
            "self.metrics.evidence.shared",
            ".for_scope(",
            "checks::evaluate",
            "checks::evidence_findings",
            "metric_values",
        ] {
            assert!(producer.contains(required), "provider lost {required}");
        }
        for forbidden in [
            "Engine",
            "factory_core",
            "factory_direction",
            "factory_composition",
            "factory_interfaces",
            "TaskStore",
            "dyn Fn",
            "PolicyReport",
        ] {
            assert!(
                !producer.contains(forbidden),
                "check provider gained {forbidden}"
            );
        }
        let schema = include_str!("../../../factory-kernel/src/check_results.rs");
        assert!(
            !schema.contains("fn rank(")
                && !schema.contains("fn from_kind(")
                && !schema.contains("fn evaluate(")
        );
        let evaluator = include_str!("../../../factory-assurance/src/checks.rs");
        assert!(evaluator.contains("impl StatusOrder for StatusKind"));
        assert!(evaluator.contains("impl BuildStatus for Status"));
        let wiring = include_str!("../policies/mod.rs")
            .split("#[cfg(test)]\nmod tests")
            .next()
            .unwrap();
        let endpoint = wiring
            .split("pub(crate) async fn policy_report(")
            .nth(1)
            .unwrap()
            .split("/// Record an attestation:")
            .next()
            .unwrap();
        assert!(endpoint.contains("policy_service(&snapshot)"));
        assert!(
            !endpoint.contains("policy::evaluate(")
                && !endpoint.contains("evidence_for_scope(")
                && !endpoint.contains("policy::applicable(")
        );
    }

    #[test]
    fn policy_clock_and_receipts_have_actual_direction_owners_not_router_callbacks() {
        let owner = include_str!("../../../factory-direction/src/policy_service/receipts.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        let compact: String = owner.chars().filter(|c| !c.is_whitespace()).collect();
        for required in [
            "Facts::<L6>::new()",
            ".get::<ExploitedFinding,_>",
            ".get::<ConfirmedSecurityReport,_>",
            "reporting_clock::compute(",
            ".receipts.append_attestation(",
            ".receipts.append_withdrawal(",
            "self.intent.catalogues_with_tags(knowledge)",
        ] {
            assert!(
                compact.contains(required),
                "L6 clock/receipt service lost {required}"
            );
        }
        for forbidden in [
            "Engine",
            "factory_core",
            "factory_process",
            "factory_environment",
            "factory_interfaces",
            "TaskStore",
            "dyn Fn",
            "BoxFuture",
            "self.policy_clock(",
        ] {
            assert!(
                !owner.contains(forbidden),
                "L6 clock service gained callback/backedge {forbidden}"
            );
        }
        let clock_wire = include_str!("../policies/clock.rs");
        assert!(clock_wire.contains("policy_service(&snapshot)"));
        assert!(
            !clock_wire.contains("reporting_clock::compute(")
                && !clock_wire.contains(".get::<")
                && !clock_wire.contains("self.policies")
        );
        let wiring = include_str!("../policies/mod.rs");
        let endpoints = wiring
            .split("pub(crate) async fn policy_attest(")
            .nth(1)
            .unwrap()
            .split("/// Close a gap:")
            .next()
            .unwrap();
        assert!(endpoints.contains(".attest(") && endpoints.contains(".withdraw("));
        for forbidden in [
            "append_attestation",
            "append_withdrawal",
            "policy::applicable",
            "self.policy_clock(",
            "self.policies",
        ] {
            assert!(
                !endpoints.contains(forbidden),
                "router kept receipt behaviour {forbidden}"
            );
        }
    }

    #[test]
    fn goals_own_live_reads_and_checkins_and_the_metric_fact_owns_its_computation() {
        let goals = include_str!("../../../factory-direction/src/goals_service.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        let compact: String = goals.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(compact.contains("Facts::<L6>::new().get::<MetricValuesFact,_>"));
        for required in [
            "goals::load",
            "goals::evaluate",
            "self.store.all()",
            "self.store.append",
            "goals::current_cycle",
            "ancestors_of",
        ] {
            assert!(goals.contains(required), "Goals owner lost {required}");
        }
        let metric = include_str!("../../../factory-assurance/src/metric_values.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(metric.contains("Provide<MetricValuesFact>"));
        assert!(
            metric.contains(".gather_measurements(")
                && metric.contains(".gather_policy(")
                && metric.contains(".finish(")
        );
        for owner in [goals, metric] {
            for forbidden in [
                "factory_core",
                "factory_composition",
                "factory_interfaces",
                "factory_daemon",
                "Engine",
                "TaskStore",
                "dyn Fn",
                "BoxFuture",
            ] {
                assert!(
                    !owner.contains(forbidden),
                    "Owner gained backedge/callback {forbidden}"
                );
            }
        }
        let query = metric
            .split("pub struct Read")
            .nth(1)
            .unwrap()
            .split("pub struct Provider")
            .next()
            .unwrap();
        assert!(
            !query.contains("MetricValue")
                && !query.contains("Gathered")
                && !query.contains("Metrics>")
        );
        let wiring = include_str!("../goals/mod.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        // Test-only imports occur before the runtime. Inspect the complete
        // prefix through the actual test module instead of stopping there.
        let runtime = include_str!("../goals/mod.rs")
            .split("#[cfg(test)]\nmod tests")
            .next()
            .unwrap();
        assert!(runtime.contains("goals_service::Service::new"));
        for forbidden in [
            "goals::load",
            "goals::evaluate",
            "self.goals.all",
            "self.goals.append",
            "self.metrics(",
        ] {
            assert!(
                !runtime.contains(forbidden),
                "Router retained Goals behavior {forbidden}"
            );
        }
        assert!(wiring.contains("Outside-stack"));
    }

    #[test]
    fn policy_intent_owns_catalogues_receipts_budget_inputs_and_the_single_declaration_chain() {
        let owner = include_str!("../../../factory-direction/src/policy_intent.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        let compact: String = owner.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(compact.contains("Facts::<L6>::new().get::<KnowledgeTags,_>"));
        for required in [
            "policy::load_all",
            "self.receipts.all()",
            "budget::load",
            "policy::applicable",
            "policy::metric_subject",
            "scope_ancestors",
            "scope_subtree",
        ] {
            assert!(
                owner.contains(required),
                "Policy intent owner lost {required}"
            );
        }
        for forbidden in [
            "factory_core",
            "factory_composition",
            "factory_interfaces",
            "factory_daemon",
            "Engine",
            "TaskStore",
            "dyn Fn",
            "BoxFuture",
            "PolicyReport",
            "CostReport",
        ] {
            assert!(
                !owner.contains(forbidden),
                "Policy intent gained backedge/result {forbidden}"
            );
        }
        let config = include_str!("../../../factory-composition/src/config.rs");
        assert!(config.contains("pub use factory_direction::policy_intent::PolicyDeclaration"));
        assert!(config.contains("factory_direction::policy_intent::chain_for_scope("));
        assert!(!config.contains("pub struct PolicyDeclaration"));
        let wiring = include_str!("../policies/mod.rs")
            .split("#[cfg(test)]\nmod tests")
            .next()
            .unwrap();
        let metric_input = wiring
            .split("pub(crate) async fn metric_policy_inputs(")
            .nth(1)
            .unwrap()
            .split("/// Historical request failure")
            .next()
            .unwrap();
        let compact_input: String = metric_input
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(compact_input.contains("policy_intent_service(snapshot).metric_inputs(scope)"));
        for forbidden in [
            "policy::load_all",
            "policy::applicable",
            "self.policies.all",
            "check_budget_intent",
        ] {
            assert!(
                !metric_input.contains(forbidden),
                "Router retained policy input behavior {forbidden}"
            );
        }
    }

    #[test]
    fn live_reads_use_the_kernel_boundary_and_router_facades_are_not_levels() {
        let wiring = include_str!("mod.rs").split("#[cfg(test)]").next().unwrap();
        assert!(wiring.contains("F::Producer: Below<R>"));
        let compact: String = wiring.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(compact.contains("factory_kernel::Facts::<R>::new().get::<F,_>"));
        assert!(!compact.contains("F::provider(self.engine).get(query)"));
        let router = include_str!("../router/l4.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        let provenance = router
            .split("Request::RunProvenance { id } =>")
            .nth(1)
            .unwrap()
            .split("Request::RunApprove")
            .next()
            .unwrap();
        assert!(provenance.contains("Facts::<factory_kernel::People>"));
        let costs = router
            .split("Request::Costs { group_by, from, to, scope } =>")
            .nth(1)
            .unwrap()
            .split("Request::RunEntries")
            .next()
            .unwrap();
        assert!(costs.contains("Facts::<factory_kernel::People>"));
        let release = include_str!("../environments/report.rs");
        assert!(release.contains("let reader = Facts::<factory_kernel::People>"));
        let environments = include_str!("../environments/report.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(environments.contains("Facts::<factory_kernel::People>::new(self).get::<factory_kernel::EnvironmentRecoveryFact>"));
    }

    #[test]
    fn spend_consumers_have_no_second_aggregation_or_upward_l6_helper() {
        let budget = include_str!("../budgets.rs")
            .split("#[cfg(test)]\nmod tests")
            .next()
            .unwrap();
        assert!(budget.contains("check_budget_intent"));
        assert!(budget.contains("factory_direction::budget_service::Service::new"));
        assert!(!budget.contains("get::<CostReport>") && !budget.contains("month_spend"));
        let direction = include_str!("../../../factory-direction/src/budget_service.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        let compact: String = direction.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(compact.contains("Facts::<L6>::new().get::<CostReport,_>"));
        assert!(direction.contains("budget::load") && direction.contains("budget::assess"));
        assert!(direction.contains("scope_ancestors") && direction.contains("resolve_scope"));
        for forbidden in [
            "factory_core",
            "factory_process",
            "factory_composition",
            "factory_daemon",
            "TaskStore",
            "Engine",
            "dyn Fn",
            "BoxFuture",
        ] {
            assert!(
                !direction.contains(forbidden),
                "Budget owner gained a backedge/callback: {forbidden}"
            );
        }
        assert!(!direction.contains("runs_between") && !direction.contains("CostRowExt"));
        assert!(!budget.contains("async fn budget_policy_input"));
        let evidence = include_str!("../../../factory-assurance/src/evidence.rs")
            .split("\nmod tests")
            .next()
            .unwrap();
        assert!(
            evidence.contains("pub async fn budget(")
                && evidence.contains("pub async fn month_spend(")
        );
        assert!(evidence.contains("get::<CostReport, _>") && !evidence.contains("runs_between"));
        assert!(!budget.contains("runs_between") && !budget.contains("crate::costs"));
        let costs = include_str!("../costs.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(!costs.contains("fn spend(") && !costs.contains("fn costs_report("));
        let producer = include_str!("l4.rs");
        assert!(!producer.contains("policies::subtree_scopes"));
        assert!(!producer.contains("impl Provide<CostReport>"));
        let owner = include_str!("../../../factory-process/src/measurements.rs");
        assert!(owner.contains("impl Provide<CostReport> for MeasurementProvider"));
        assert!(!owner.contains("factory_core") && !owner.contains("Engine"));
        let metrics = include_str!("../../../factory-assurance/src/metrics_service.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(
            metrics.contains("get::<factory_kernel::CostReport, _>")
                && metrics.contains("SpendBasis::Finished")
        );
        let branch = metrics
            .split("} else if matches!(id.as_str(), \"unit_cost\" | \"tokens_per_run\") {")
            .nth(1)
            .unwrap()
            .split("} else if is_usage_metric")
            .next()
            .unwrap();
        let branch: String = branch.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(branch.contains("sources.spend") && !branch.contains("usage_value"));
    }

    #[test]
    fn registry_process_and_benchmark_reads_are_producer_owned() {
        let metrics = include_str!("../../../factory-assurance/src/metrics_service.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        for forbidden in [
            "self.store.",
            "self.bench.",
            ".occupancy(",
            ".production_at(",
        ] {
            assert!(
                !metrics.contains(forbidden),
                "metrics bypasses a fact port: {forbidden}"
            );
        }
        for fact in ["ProductionFact", "ProcessMetricFact"] {
            assert!(metrics.contains(&format!("get::<{fact}, _>")));
        }
        let compact: String = metrics.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(compact
            .contains("Provide::<factory_kernel::BenchResolutionFact>::get(&self.evidence.own"));
        assert!(metrics.contains("Facts::<L5>::new()"));
        for forbidden in [
            "factory_core",
            "factory_direction",
            "factory_interfaces",
            "Engine",
            "Facts::<L6>",
            "self.policy_report(",
        ] {
            assert!(
                !metrics.contains(forbidden),
                "L5 metric owner back-edge: {forbidden}"
            );
        }
        let service = metrics.split("/// Sum `finished`").next().unwrap();
        for forbidden in ["Fn(", "FnMut(", "FnOnce("] {
            assert!(
                !service.contains(forbidden),
                "L5 metric owner callback: {forbidden}"
            );
        }
        let request = include_str!("../metrics.rs")
            .split("#[cfg(test)]\nmod tests")
            .next()
            .unwrap();
        assert!(
            request.contains("service.gather_measurements(")
                && request.contains("service.gather_policy(")
                && request.contains("service.finish(")
        );
        for forbidden in [
            "fn compute_one",
            "fn compliance_value",
            "fn rolling_series",
            ".policy_report(",
            "Facts::<L6>::",
        ] {
            assert!(
                !request.contains(forbidden),
                "metric computation left in router: {forbidden}"
            );
        }
        let process = include_str!("../../../factory-process/src/process_metrics.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(process.contains("impl Provide<ProductionFact> for MeasurementProvider"));
        assert!(process.contains("impl Provide<ProcessMetricFact> for MeasurementProvider"));
        assert!(!process.contains("factory_core") && !process.contains("Engine"));
        for file in [
            include_str!("../operations.rs"),
            include_str!("../intake.rs"),
            include_str!("../quality/mod.rs"),
            include_str!("../environments/mod.rs"),
            include_str!("../environments/recovery.rs"),
        ] {
            assert!(!file
                .split("#[cfg(test)]\nmod tests")
                .next()
                .unwrap()
                .contains("crate::policies::subtree_scopes"));
        }
    }

    #[test]
    fn upper_level_task_inventory_reads_cannot_bypass_the_l4_port() {
        for file in [
            include_str!("../policies/mod.rs"),
            include_str!("../../../factory-direction/src/scenarios_service.rs"),
        ] {
            let production = file.split("#[cfg(test)]\nmod tests").next().unwrap();
            let compact: String = production.chars().filter(|c| !c.is_whitespace()).collect();
            assert!(
                !compact.contains("self.store"),
                "upper-level task read bypasses L4"
            );
            assert!(production.contains("get::<TaskInventoryFact>")
                || compact.contains("get::<TaskInventoryFact,_>"));
        }
        // Quality report and remediation both read L4 through facts. A
        // command acknowledgement cannot supply inventory or a task record.
        let quality = include_str!("../quality/mod.rs")
            .split("pub(crate) async fn quality_remediate")
            .next()
            .unwrap();
        let compact: String = quality.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(!compact.contains("self.store"));
        assert!(quality.contains("Facts::<L5>::new(self)"));
        assert!(quality.contains("get::<TaskInventoryFact>"));
        let producer = include_str!("../../../factory-process/src/facts.rs");
        assert!(producer.contains("impl Provide<factory_kernel::TaskInventoryFact> for Provider"));
        for (owner, reader) in [
            (include_str!("../../../factory-assurance/src/remediation.rs"), "L5"),
            (include_str!("../../../factory-direction/src/remediation.rs"), "L6"),
        ] {
            let production = owner.split("#[cfg(test)]\nmod tests").next().unwrap();
            let compact: String = production.chars().filter(|c| !c.is_whitespace()).collect();
            assert!(compact.contains(&format!("Facts::<{reader}>::new()")));
            assert!(compact.contains("get::<TaskInventoryFact,_>"));
            assert!(!compact.contains("TaskStore") && !compact.contains("Engine"));
        }
    }

    #[test]
    fn scenarios_have_actual_l6_reads_folds_and_adjacent_promotion_not_engine_callbacks() {
        let owner = include_str!("../../../factory-direction/src/scenarios_service.rs")
            .split("#[cfg(test)]\nmod tests")
            .next()
            .unwrap();
        let compact: String = owner.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(compact.contains("Facts::<L6>::new()"));
        for fact in [
            "MetricValuesFact",
            "TaskInventoryFact",
            "CheckEvaluationFact",
            "CheckComparisonFact",
            "ProductionFact",
        ] {
            assert!(
                compact.contains(&format!("get::<{fact},_>")),
                "missing actual Scenarios read: {fact}"
            );
        }
        for operation in [
            "scenario::load(&dir)",
            "scenario::overlay_chain(",
            "scenario::policy_delta(",
            "scenario::forecast_completion(",
            "scenario::goal_probability(",
            "commands.promote(",
        ] {
            assert!(
                compact.contains(operation),
                "Scenarios behaviour missing from owner: {operation}"
            );
        }
        for forbidden in [
            "factory_core",
            "factory_process",
            "factory_interfaces",
            "Engine",
            "TaskStore",
            "dyn Fn",
            "BoxFuture",
            "TaskSnapshotFact",
        ] {
            assert!(
                !owner.contains(forbidden),
                "Scenarios owner acquired a backedge: {forbidden}"
            );
        }
        let wiring = include_str!("../scenarios/mod.rs")
            .split("#[cfg(test)]\nmod tests")
            .next()
            .unwrap();
        let compact_wiring: String = wiring.chars().filter(|c| !c.is_whitespace()).collect();
        for call in [
            "service.prepare_report(",
            "plan.read_metrics(",
            "service.finish_report(",
            "service.prepare_whatif(",
            "service.finish_whatif(",
            ".promote(scenario_name,scope,agent,",
        ] {
            assert!(
                compact_wiring.contains(call),
                "request bypasses actual Scenarios owner: {call}"
            );
        }
        for forbidden in [
            "self.store",
            "self.metrics(",
            "scenario::load(",
            "scenario::policy_delta(",
            "scenario::forecast_completion(",
            "evidence_for_scope(",
            "checks::evaluate(",
        ] {
            assert!(
                !compact_wiring.contains(forbidden),
                "Scenarios logic remains outside: {forbidden}"
            );
        }
        assert!(compact_wiring.contains("self.wiring.task_snapshot(entry.task.id)"));
        assert!(compact_wiring.contains("ifprovider.policy_was_gathered()"));
        let provider = include_str!("../../../factory-assurance/src/check_evaluation.rs");
        let _: fn(
            <CheckComparisonFact as Port>::Provider<'static>,
        ) -> factory_assurance::check_evaluation::Provider<'static, checks::Ports<'static>> =
            |provider| provider;
        assert!(provider.contains("Provide<factory_kernel::CheckComparisonFact>"));
        assert!(provider.contains("primary: observations(&input.subjects)"));
        assert!(provider.contains("alternative: observations(&alternative.subjects)"));
        assert!(provider.contains(".for_scope("));
        let schema = FACT_CATALOGUE
            .iter()
            .find(|f| f.fact == "CheckComparisonFact")
            .unwrap();
        assert_eq!(schema.producer, "L5");
        assert_eq!(schema.readers, ["L6 Scenarios promotion"]);
    }
    #[test]
    fn signposts_are_computed_in_l5_and_read_directly_by_people_not_operations() {
        let owner = include_str!("../../../factory-assurance/src/signposts.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        let compact: String = owner.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(compact.contains("typeLevel=L5"));
        assert!(compact.contains("Provide<SignpostFact>"));
        assert!(
            compact.contains("self.metrics.gather(") && compact.contains("self.metrics.finish(")
        );
        assert!(compact.contains("evaluate_signposts(&scenario.signposts"));
        for forbidden in [
            "factory_core",
            "factory_direction",
            "factory_interfaces",
            "Engine",
            "dyn Fn",
            "BoxFuture",
        ] {
            assert!(
                !owner.contains(forbidden),
                "upper/outside backedge in signpost producer: {forbidden}"
            );
        }
        let query = owner
            .split("pub struct Read")
            .nth(1)
            .unwrap()
            .split("pub fn metric_ids")
            .next()
            .unwrap();
        assert!(
            !query.contains("MetricValue")
                && !query.contains("SignpostStatus")
                && !query.contains("SignpostFact")
        );
        let wiring = include_str!("../signposts.rs")
            .split("#[cfg(test)]\n    pub(crate)")
            .next()
            .unwrap();
        let compact_wiring: String = wiring.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(compact_wiring.contains("Facts::<People>::new(self).get::<SignpostFact>"));
        assert!(!wiring.contains("self.metrics("));
        let operations = include_str!("../operations.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(operations.contains("signposts: Vec::new()"));
        assert!(
            !operations.contains("triggered_signposts") && !operations.contains("signpost_cache")
        );
        let direction = include_str!("../../../factory-direction/src/scenario.rs")
            .split("#[cfg(test)]\nmod tests")
            .next()
            .unwrap();
        assert!(!direction.contains("fn evaluate_signpost("));
        assert!(direction.contains("factory_assurance::signposts::Signpost"));
        let dashboard = include_str!("../../../../ui/js/dashboard.js");
        assert!(dashboard.contains("api(\"/api/signposts\")"));
        let schema = FACT_CATALOGUE
            .iter()
            .find(|entry| entry.fact == "SignpostFact")
            .unwrap();
        assert_eq!(schema.producer, "L5");
        assert_eq!(schema.readers, ["People Dashboard"]);
    }

    #[test]
    fn check_evidence_and_quality_judgement_have_real_l5_ownership() {
        let evidence = include_str!("../../../factory-assurance/src/evidence.rs")
            .split("\nmod tests").next().unwrap();
        let compact: String = evidence.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(compact.contains("Facts::<L5>::new()"));
        for fact in ["TaskFact", "WorkflowFact", "AgentFact", "DaemonConfigFact", "SecretsPresence", "DependenciesFact", "BackupFact", "AttestedRun", "CostReport"] {
            assert!(compact.contains(&format!("get::<{fact},_>")), "missing checked L5 read: {fact}");
            assert!(!compact.contains(&format!("Provide::<{fact}>::get")), "unchecked lower read: {fact}");
        }
        assert!(compact.contains("Provide::<GateFact>::get(&self.own") && compact.contains("Provide::<KnowledgeTags>::get(&self.own"));
        for forbidden in ["Engine", "factory_core", "factory_direction", "factory_infrastructure", "factory_environment", "factory_agents", "TaskStore", "dyn Fn", "BoxFuture"] {
            assert!(!evidence.contains(forbidden), "owner acquired an outside callback/backedge: {forbidden}");
        }
        assert!(compact.contains("quality::evaluate(scope.tree,values,&evidence,now)"));
        assert!(compact.contains("self.scopes.ancestors_of(scope)"));
        let policy = include_str!("../policies/mod.rs").split("\nmod tests").next().unwrap();
        for fact in ["TaskFact", "WorkflowFact", "AgentFact", "DaemonConfigFact", "SecretsPresence", "DependenciesFact", "BackupFact", "AttestedRun"] {
            assert!(!policy.contains(&format!("get::<{fact}>")), "second gathering path in policy: {fact}");
        }
        assert!(!policy.contains("async fn attested_evidence") && !policy.contains("fn needs_agent_facts"));
        let quality = include_str!("../quality/mod.rs").split("\nmod tests").next().unwrap();
        assert!(quality.contains("crate::facts::checks::service(self, inputs.snapshot.scope_tree()).judge_quality"));
        assert!(!quality.contains("reports.push(quality::evaluate") && !quality.contains(".evidence_for_scope("));
        let wiring = include_str!("checks.rs");
        assert!(wiring.contains("Service::new") && !wiring.contains("async fn") && !wiring.contains(".get::<"));
        let direction = include_str!("../../../factory-direction/src/budget.rs").split("\nmod tests").next().unwrap();
        assert!(direction.contains("pub fn check_intent") && direction.contains("factory_assurance::evidence::BudgetIntent"));
        assert!(!direction.contains("get::<CostReport") && !direction.contains("async fn"));
    }

    #[test]
    fn sandbox_service_evidence_uses_l2_ownership_and_people_composition() {
        let l2_router = include_str!("../router/l2.rs");
        let branch = l2_router
            .split("Request::Dependencies { scope } =>")
            .nth(1)
            .unwrap()
            .split("Request::DependenciesVex")
            .next()
            .unwrap();
        assert!(branch.contains("Facts::<factory_kernel::People>"));
        assert!(branch.contains("get::<factory_kernel::SandboxServiceEvidenceFact>"));
        let producer = include_str!("../service_observations.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(!producer.contains(".store") && !producer.contains("factory_core::policy"));
        assert!(
            include_str!("../../../factory-environment/src/service_observations.rs")
                .contains("impl factory_kernel::Provide<SandboxServiceEvidenceFact> for Provider")
        );
    }
}
