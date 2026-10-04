//! Environment-owned fact providers. Presence only; never secret values.
use crate::engine::Engine;
use async_trait::async_trait;
use factory_core::{
    error::{FactoryError, Result},
    protocol::CredentialRow,
};
use factory_kernel::{DependenciesFact, ExploitedFinding, Provide, SecretsPresence};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct Provider<'a> {
    pub(super) engine: &'a Engine,
}
impl factory_kernel::FactProvider for Provider<'_> {
    type Level = factory_kernel::L2;
}

#[async_trait]
impl Provide<factory_kernel::CredentialExpiryFact> for Provider<'_> {
    type Query = ();
    type Value = factory_kernel::CredentialExpiryFact;
    type Error = FactoryError;
    async fn get(&self, _: &()) -> Result<Self::Value> {
        Ok(factory_kernel::CredentialExpiryFact { observations: self.engine.credential_expiries.all().await? })
    }
}
#[async_trait]
impl Provide<SecretsPresence> for Provider<'_> {
    type Query = BTreeSet<String>;
    type Value = BTreeMap<String, SecretsPresence>;
    type Error = FactoryError;
    async fn get(&self, scopes: &Self::Query) -> Result<Self::Value> {
        if scopes.is_empty() {
            return Ok(BTreeMap::new());
        }
        let snapshot = self.engine.factory_snapshot();
        let scopes: Vec<_> = scopes
            .iter()
            .map(|s| snapshot.scope(s).map(|s| s.name.clone()))
            .collect::<Result<_>>()?;
        let rows = self.engine.credential_inventory().await;
        Ok(scopes
            .into_iter()
            .map(|s| {
                let fact = secrets_fact_map(&rows, &s).into();
                (s, fact)
            })
            .collect())
    }
}
#[async_trait]
impl Provide<DependenciesFact> for Provider<'_> {
    type Query = String;
    type Value = DependenciesFact;
    type Error = FactoryError;
    async fn get(&self, scope: &String) -> Result<Self::Value> {
        Ok(crate::dependencies::fact(
            &self.engine.dependencies_report(scope).await?,
        ))
    }
}
#[async_trait]
impl Provide<ExploitedFinding> for Provider<'_> {
    type Query = String;
    type Value = Vec<ExploitedFinding>;
    type Error = FactoryError;
    async fn get(&self, scope: &String) -> Result<Self::Value> {
        self.engine.exploited_findings(scope).await
    }
}

#[async_trait]
impl Provide<factory_kernel::ReleaseSbomFact> for Provider<'_> {
    type Query = super::ReleaseSbomQuery;
    type Value = Vec<factory_kernel::ReleaseSbomFact>;
    type Error = FactoryError;
    async fn get(&self, query: &Self::Query) -> Result<Self::Value> {
        self.engine.release_sboms(&query.scope, &query.commit, query.version.as_deref()).await
    }
}
fn secrets_fact_map(rows: &[CredentialRow], scope: &str) -> BTreeMap<String, bool> {
    let mut map = BTreeMap::new();
    for row in rows {
        let id = match (row.integration.as_str(), row.scope.as_deref()) {
            ("anthropic", None) => "anthropic",
            ("github", None) => "github",
            ("aws", None) => "aws",
            ("netrc", None) => "netrc",
            ("ssh", None) => "ssh",
            ("scope env", Some(s)) if s == scope => "scope_env",
            _ => continue,
        };
        map.insert(id.to_string(), row.present);
    }
    map
}
