//! Outside-stack construction only. Actual gathering and Quality judgement
//! are in L5; no callback or Engine reference enters that service.
use super::Port;
use crate::engine::Engine;
use factory_assurance::evidence::{Ports as EvidencePorts, Service};
use factory_assurance::metrics_service::Ports as MetricPorts;
use factory_kernel::{
    AgentFact, AttestedRun, BackupFact, DaemonConfigFact, DependenciesFact, SecretsPresence,
    TaskFact,
};

pub(crate) struct Ports<'a> {
    process: factory_process::facts::Provider<'a>,
    measurements: factory_process::measurements::MeasurementProvider<'a>,
    agents: factory_agents::roster::Provider,
    daemon: factory_infrastructure::settings_facts::SettingsProvider,
    secrets: factory_environment::credentials::Provider,
    dependencies: factory_environment::dependency_inventory::Provider,
    backup: factory_infrastructure::backup_facts::Provider<'a>,
    environments: factory_infrastructure::environment_facts::Provider<'a>,
}

macro_rules! capability {
    ($associated:ident, $method:ident, $field:ident, $provider:ty) => {
        type $associated = $provider;
        fn $method(&self) -> &Self::$associated {
            &self.$field
        }
    };
}
impl<'a> EvidencePorts for Ports<'a> {
    capability!(Tasks, tasks, process, factory_process::facts::Provider<'a>);
    capability!(
        Workflows,
        workflows,
        process,
        factory_process::facts::Provider<'a>
    );
    capability!(Agents, agents, agents, factory_agents::roster::Provider);
    capability!(
        Daemon,
        daemon,
        daemon,
        factory_infrastructure::settings_facts::SettingsProvider
    );
    capability!(
        Secrets,
        secrets,
        secrets,
        factory_environment::credentials::Provider
    );
    capability!(
        Dependencies,
        dependencies,
        dependencies,
        factory_environment::dependency_inventory::Provider
    );
    capability!(
        Backup,
        backup,
        backup,
        factory_infrastructure::backup_facts::Provider<'a>
    );
    capability!(
        Attested,
        attested,
        measurements,
        factory_process::measurements::MeasurementProvider<'a>
    );
    capability!(
        Spend,
        spend,
        measurements,
        factory_process::measurements::MeasurementProvider<'a>
    );
}

impl<'a> MetricPorts for Ports<'a> {
    capability!(
        Production,
        production,
        measurements,
        factory_process::measurements::MeasurementProvider<'a>
    );
    capability!(
        Process,
        process,
        measurements,
        factory_process::measurements::MeasurementProvider<'a>
    );
    capability!(
        Environments,
        environments,
        environments,
        factory_infrastructure::environment_facts::Provider<'a>
    );
}

pub(crate) fn service(
    engine: &Engine,
    scopes: factory_kernel::ScopeTree,
) -> Service<'_, Ports<'_>> {
    Service::new(
        Ports {
            process: TaskFact::provider(engine),
            measurements: AttestedRun::provider(engine),
            agents: AgentFact::provider(engine),
            daemon: DaemonConfigFact::provider(engine),
            secrets: SecretsPresence::provider(engine),
            dependencies: DependenciesFact::provider(engine),
            backup: BackupFact::provider(engine),
            environments: super::l1::environment_provider(engine),
        },
        super::l5::provider(engine),
        scopes,
    )
}
