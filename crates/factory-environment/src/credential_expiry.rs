//! Live L2 authored credential-expiry metadata; no credential values or commands.
use crate::openshell::{self as os, ProviderCredential, ProviderDecl};
use crate::secrets::Expiry;
use chrono::{DateTime, Utc};
use factory_infrastructure::renewals::DEFAULT_LEAD_SECONDS;
use factory_kernel::{
    DateBasis, DateKind, DateSource, ExpiryObservation, FactoryError, Provide, Result,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretUse {
    pub scope: String,
    pub agent: String,
    pub provider: String,
}

#[derive(Clone)]
pub struct AgentProviders {
    pub name: String,
    pub config: os::OpenshellConfig,
}
#[derive(Clone)]
pub struct ScopedProviders {
    pub name: String,
    pub agents: Vec<AgentProviders>,
}
pub struct Provider<'a> {
    pub store: &'a crate::expiry_store::ObservationStore,
    pub instance: String,
    pub declarations: Vec<crate::secrets::SecretDecl>,
    pub providers: Vec<ScopedProviders>,
}
impl factory_kernel::FactProvider for Provider<'_> {
    type Level = factory_kernel::L2;
}
#[async_trait::async_trait]
impl Provide<factory_kernel::CredentialExpiryFact> for Provider<'_> {
    type Query = ();
    type Value = factory_kernel::CredentialExpiryFact;
    type Error = FactoryError;
    async fn get(&self, _: &()) -> Result<Self::Value> {
        let mut observations = self.store.all().await?;
        observations.extend(ledger_observations(
            &self.instance,
            &self.declarations,
            &self.providers,
            Utc::now(),
        ));
        Ok(factory_kernel::CredentialExpiryFact { observations })
    }
}
/// Every managed provider in every scope, with what its credential says:
/// `(scope, agent, provider on the gateway, provider, credential)`.
pub fn managed_providers(
    instance: &str,
    declarations: &[ScopedProviders],
) -> Vec<(String, String, String, os::ManagedProvider)> {
    let suffix = os::instance_suffix(instance);
    let mut out = Vec::new();
    for scope in declarations {
        for agent in &scope.agents {
            let block = &agent.config;
            for provider in &block.providers {
                if let ProviderDecl::Managed(managed) = provider {
                    out.push((
                        scope.name.clone(),
                        agent.name.clone(),
                        provider.gateway_name(&suffix),
                        managed.clone(),
                    ));
                }
            }
        }
    }
    out
}

/// Who names each secret: secret name -> its users.
pub fn users(instance: &str, declarations: &[ScopedProviders]) -> BTreeMap<String, Vec<SecretUse>> {
    let mut out: BTreeMap<String, Vec<SecretUse>> = BTreeMap::new();
    for (scope, agent, provider, managed) in managed_providers(instance, declarations) {
        if let ProviderCredential::Secret(name) = &managed.credential {
            out.entry(name.clone()).or_default().push(SecretUse {
                scope,
                agent,
                provider,
            });
        }
    }
    out
}

/// One Important dates observation per declared secret (`#244`): the
/// renewals ledger (`#236`) reads the catalogue rather than keeping a copy,
/// and its milestones -- the 30-day lead, 7 days, 1 day, the day itself --
/// are the secret's Inbox items, one per secret, naming every scope, agent
/// and provider that uses it. Built from the live snapshot on every read, so
/// an edited date counts at once.
pub fn ledger_observations(
    instance: &str,
    declarations: &[crate::secrets::SecretDecl],
    providers: &[ScopedProviders],
    now: chrono::DateTime<Utc>,
) -> Vec<factory_kernel::ExpiryObservation> {
    use factory_kernel::{DateBasis, DateDependency, DateKind, DateSource};
    let mut used = users(instance, providers);
    declarations
        .iter()
        .map(|secret| {
            let mut item = observation(
                format!("secret:{}", secret.name),
                format!("Secret {}", secret.name),
                DateKind::Credential,
                DateSource::Secret,
                now,
            );
            match secret.expires {
                Some(Expiry::On(date)) => {
                    item.expires_at = date.and_hms_opt(0, 0, 0).map(|midnight| midnight.and_utc());
                    item.basis = DateBasis::Declared;
                    item.detail = "declared in the instance root's secrets:".into();
                }
                Some(Expiry::Never) => {
                    item.no_expiry = true;
                    item.basis = DateBasis::Declared;
                    item.detail = "declared never to expire in the instance root's secrets:".into();
                }
                None => {
                    item.detail = "the instance root's secrets: gives no expires: for it".into()
                }
            }
            if let Some(renew) = &secret.renew {
                item.renew = renew.clone();
            }
            item.affects = used
                .remove(&secret.name)
                .unwrap_or_default()
                .into_iter()
                .map(|u| DateDependency {
                    label: format!("{} / {} / {}", u.scope, u.agent, u.provider),
                    scope: Some(u.scope),
                    agent: Some(u.agent),
                    environment: None,
                    provider: Some(u.provider),
                })
                .collect();
            item
        })
        .collect()
}

fn observation(
    id: String,
    name: String,
    kind: DateKind,
    source: DateSource,
    now: DateTime<Utc>,
) -> ExpiryObservation {
    ExpiryObservation {
        id,
        name,
        kind,
        source,
        scope: None,
        expires_at: None,
        no_expiry: false,
        basis: DateBasis::Unknown,
        detail: "expiry not yet observed".into(),
        observed_at: None,
        attempted_at: now,
        issue: None,
        affects: Vec::new(),
        lead_seconds: DEFAULT_LEAD_SECONDS,
        renew: "renew with the service owner".into(),
        owner: "owner".into(),
    }
}
