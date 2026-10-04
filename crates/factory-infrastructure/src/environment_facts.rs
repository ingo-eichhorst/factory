//! L1-only environment history, metrics and publication identity.
//! No task/workflow store or upper-level action gatherer enters this owner.
use crate::environment_store::EnvironmentStore;
use crate::environments::{
    self as env, Deployment, EnvironmentDecl, EnvironmentsReport, ReleaseFacts, Sample,
};
use chrono::{DateTime, Duration, Utc};
use factory_kernel::{EnvironmentMetricFact, FactoryError, Provide, Result, ScopeTree};
use std::collections::{BTreeMap, BTreeSet};

pub struct History {
    pub report: EnvironmentsReport,
    pub declarations: Vec<(String, EnvironmentDecl)>,
    pub members: Option<BTreeSet<String>>,
    pub deployments: Vec<Deployment>,
}
pub struct Provider<'a> {
    pub store: &'a EnvironmentStore,
    pub scopes: ScopeTree,
    pub declarations: Vec<(String, EnvironmentDecl)>,
}
impl factory_kernel::FactProvider for Provider<'_> {
    type Level = factory_kernel::L1;
}
impl Provider<'_> {
    pub async fn history(&self, scope: Option<&str>) -> Result<History> {
        let now = Utc::now();
        let members: Option<BTreeSet<String>> = match scope {
            None => None,
            Some(name) => {
                let (asked, subtree) = self.scopes.subtree_scopes(Some(name))?;
                let mut names: BTreeSet<String> =
                    subtree.into_iter().map(|s| s.name.clone()).collect();
                if let Some(asked) = asked {
                    names.insert(asked.name.clone());
                }
                Some(names)
            }
        };
        let within = |s: &str| members.as_ref().is_none_or(|m| m.contains(s));
        let declared: Vec<(String, EnvironmentDecl)> = self
            .declarations
            .iter()
            .cloned()
            .filter(|(s, _)| within(s))
            .collect();
        let window = declared
            .iter()
            .filter_map(|(_, d)| d.slo.as_ref().map(env::Slo::window_days))
            .max()
            .unwrap_or(env::DEFAULT_WINDOW_DAYS)
            .max(env::DEFAULT_WINDOW_DAYS);
        let names: BTreeSet<&str> = declared.iter().map(|(_, d)| d.name.as_str()).collect();
        let samples: Vec<Sample> = self
            .store
            .samples_since(now - Duration::days(window))
            .await?
            .into_iter()
            .filter(|s| names.contains(s.environment.as_str()))
            .collect();
        let history = self.store.deployments().await?;
        let deployments: Vec<Deployment> = history
            .iter()
            .filter(|d| within(&d.scope))
            .cloned()
            .collect();
        let added: Vec<(String, ReleaseFacts, DateTime<Utc>)> = self
            .store
            .releases_added()
            .await?
            .into_iter()
            .filter(|(s, _, _)| within(s))
            .collect();
        let report = env::report(&declared, &samples, &deployments, &added, now);

        Ok(History {
            report,
            declarations: declared,
            members,
            deployments: history,
        })
    }
    pub async fn cards(
        &self,
        scope: Option<&str>,
    ) -> Result<BTreeMap<String, env::EnvironmentCard>> {
        Ok(self
            .history(scope)
            .await?
            .report
            .environments
            .into_iter()
            .map(|card| (card.name.clone(), card))
            .collect())
    }
}
#[async_trait::async_trait]
impl Provide<EnvironmentMetricFact> for Provider<'_> {
    type Query = Option<String>;
    type Value = BTreeMap<String, EnvironmentMetricFact>;
    type Error = FactoryError;
    async fn get(&self, scope: &Self::Query) -> Result<Self::Value> {
        Ok(self
            .cards(scope.as_deref())
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
#[async_trait::async_trait]
impl Provide<factory_kernel::DeploymentPublicationFact> for Provider<'_> {
    type Query = String;
    type Value = Option<factory_kernel::DeploymentPublicationFact>;
    type Error = FactoryError;
    async fn get(&self, id: &String) -> Result<Self::Value> {
        use crate::environments::{DeployStatus, Tier};
        let Some(deployment) = self.store.deployment(id).await? else {
            return Ok(None);
        };
        let declaration = self
            .declarations
            .iter()
            .find(|(scope, env)| *scope == deployment.scope && env.name == deployment.environment);
        let repository = declaration
            .as_ref()
            .and_then(|(_, env)| env.github_deployments.as_ref())
            .map(|mirror| mirror.repository.clone());
        Ok(Some(factory_kernel::DeploymentPublicationFact {
            deployment: deployment.id,
            scope: deployment.scope,
            environment: deployment.environment,
            commit: deployment.release.commit,
            dirty: deployment.release.dirty,
            state: match deployment.status {
                DeployStatus::Running => "in_progress",
                DeployStatus::Succeeded => "success",
                DeployStatus::Failed => "failure",
                DeployStatus::RolledBack => "inactive",
            }
            .into(),
            verified: deployment.verification.map(|verification| verification.ok),
            repository,
            transient: declaration
                .as_ref()
                .is_some_and(|(_, env)| env.tier == Tier::Ephemeral),
            production: declaration
                .as_ref()
                .is_some_and(|(_, env)| env.tier == Tier::Production),
        }))
    }
}
