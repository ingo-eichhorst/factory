//! Infrastructure-owned providers. Report computations remain in L1.
use crate::engine::Engine;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use factory_core::error::{FactoryError, Result};
use factory_kernel::{
    BackupFact, DaemonConfigFact, EnvironmentMetricFact, Provide, ScopeCapacityFact,
};
use std::collections::BTreeMap;

pub(crate) struct Provider<'a> {
    pub(super) engine: &'a Engine,
}
impl factory_kernel::FactProvider for Provider<'_> {
    type Level = factory_kernel::L1;
}

#[async_trait]
impl Provide<factory_kernel::DeploymentPublicationFact> for Provider<'_> {
    type Query = String;
    type Value = Option<factory_kernel::DeploymentPublicationFact>;
    type Error = FactoryError;
    async fn get(&self, id: &String) -> Result<Self::Value> {
        use factory_core::environments::{DeployStatus, Tier};
        let Some(deployment) = self.engine.environments.deployment(id).await? else { return Ok(None); };
        let snapshot = self.engine.factory_snapshot();
        let declaration = snapshot.config.environments().into_iter().find(|(scope, env)| *scope == deployment.scope && env.name == deployment.environment);
        let repository = declaration.as_ref().and_then(|(_, env)| env.github_deployments.as_ref()).map(|mirror| mirror.repository.clone());
        Ok(Some(factory_kernel::DeploymentPublicationFact {
            deployment: deployment.id, scope: deployment.scope, environment: deployment.environment,
            commit: deployment.release.commit, dirty: deployment.release.dirty,
            state: match deployment.status { DeployStatus::Running => "in_progress", DeployStatus::Succeeded => "success", DeployStatus::Failed => "failure", DeployStatus::RolledBack => "inactive" }.into(),
            verified: deployment.verification.map(|verification| verification.ok), repository,
            transient: declaration.as_ref().is_some_and(|(_, env)| env.tier == Tier::Ephemeral),
            production: declaration.as_ref().is_some_and(|(_, env)| env.tier == Tier::Production),
        }))
    }
}

#[async_trait]
impl Provide<BackupFact> for Provider<'_> {
    type Query = DateTime<Utc>;
    type Value = BackupFact;
    type Error = FactoryError;
    async fn get(&self, now: &Self::Query) -> Result<Self::Value> {
        self.engine.backup_fact(*now).await
    }
}
#[async_trait]
impl Provide<ScopeCapacityFact> for Provider<'_> {
    type Query = ();
    type Value = ScopeCapacityFact;
    type Error = FactoryError;
    async fn get(&self, _: &()) -> Result<Self::Value> {
        let snapshot = self.engine.factory_snapshot();
        Ok(ScopeCapacityFact {
            max_sessions: snapshot
                .config
                .scopes
                .iter()
                .filter_map(|s| s.max_sessions.map(|m| (s.name.clone(), m)))
                .collect(),
            tick_seconds: snapshot.config.daemon.tick_seconds,
        })
    }
}
#[async_trait]
impl Provide<EnvironmentMetricFact> for Provider<'_> {
    type Query = Option<String>;
    type Value = BTreeMap<String, EnvironmentMetricFact>;
    type Error = FactoryError;
    async fn get(&self, scope: &Self::Query) -> Result<Self::Value> {
        Ok(self
            .engine
            .environment_cards(scope.as_deref())
            .await?
            .into_iter()
            .map(|(name, c)| {
                (
                    name,
                    EnvironmentMetricFact {
                        name: c.name,
                        availability: c.uptime_window,
                        has_slo: c.slo.is_some(),
                        error_budget: c.error_budget,
                        incidents: c.incidents.len(),
                        mttr: c.dora.mttr,
                        time_to_restore_p50: c.dora.time_to_restore_p50,
                        deploy_frequency: c.dora.deploy_frequency,
                        lead_time_p50: c.dora.lead_time_p50,
                        change_failure_rate: c.dora.change_failure_rate,
                    },
                )
            })
            .collect())
    }
}
impl Provider<'_> {
    fn daemon_facts(&self) -> DaemonConfigFact {
        let snapshot = self.engine.factory_snapshot();
        let daemon_config = &snapshot.config.daemon;

        let http_binds: Vec<String> = crate::interfaces::interface_facts(&daemon_config.interfaces)
            .into_iter()
            .filter(|i| i.kind == "http")
            .filter_map(|i| i.bind)
            .collect();
        // No `http` interface at all is vacuously loopback-only -- nothing
        // is exposed beyond loopback either way. A `bind` that does not
        // parse as a socket address (a bare hostname, say) is left `None`:
        // this module makes no DNS lookup and no guess about what a name
        // resolves to.
        let http_loopback_only = if http_binds.is_empty() {
            Some(true)
        } else {
            http_binds
                .iter()
                .map(|bind| {
                    bind.parse::<std::net::SocketAddr>()
                        .map(|addr| addr.ip().is_loopback())
                })
                .collect::<std::result::Result<Vec<bool>, _>>()
                .ok()
                .map(|loopback| loopback.iter().all(|l| *l))
        };

        DaemonConfigFact {
            foreman_enabled: daemon_config.foreman.enabled,
            http_loopback_only,
            power_assertion: daemon_config.power_assertion,
        }
    }
}
#[async_trait]
impl Provide<DaemonConfigFact> for Provider<'_> {
    type Query = ();
    type Value = DaemonConfigFact;
    type Error = FactoryError;
    async fn get(&self, _: &()) -> Result<Self::Value> {
        Ok(self.daemon_facts())
    }
}
