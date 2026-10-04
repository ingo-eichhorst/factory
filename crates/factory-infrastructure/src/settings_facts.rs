//! L1 settings facts derived from plain current declarations.
use crate::interfaces::{interface_facts, InterfaceConfig};
use factory_kernel::{DaemonConfigFact, FactoryError, Provide, Result, ScopeCapacityFact};
pub struct SettingsProvider {
    pub foreman_enabled: bool,
    pub power_assertion: bool,
    pub tick_seconds: u64,
    pub scopes: Vec<(String, Option<u32>)>,
    pub interfaces: Vec<InterfaceConfig>,
}
impl factory_kernel::FactProvider for SettingsProvider {
    type Level = factory_kernel::L1;
}
impl SettingsProvider {
    fn daemon_facts(&self) -> DaemonConfigFact {
        let http_binds: Vec<String> = interface_facts(&self.interfaces)
            .into_iter()
            .filter(|i| i.kind == "http")
            .filter_map(|i| i.bind)
            .collect();
        // No listener is vacuously loopback-only; a hostname is unknown,
        // never resolved or guessed here.
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
            foreman_enabled: self.foreman_enabled,
            http_loopback_only,
            power_assertion: self.power_assertion,
        }
    }
}
#[async_trait::async_trait]
impl Provide<DaemonConfigFact> for SettingsProvider {
    type Query = ();
    type Value = DaemonConfigFact;
    type Error = FactoryError;
    async fn get(&self, _: &()) -> Result<Self::Value> {
        Ok(self.daemon_facts())
    }
}
#[async_trait::async_trait]
impl Provide<ScopeCapacityFact> for SettingsProvider {
    type Query = ();
    type Value = ScopeCapacityFact;
    type Error = FactoryError;
    async fn get(&self, _: &()) -> Result<Self::Value> {
        Ok(ScopeCapacityFact {
            max_sessions: self
                .scopes
                .iter()
                .filter_map(|(name, max)| max.map(|max| (name.clone(), max)))
                .collect(),
            tick_seconds: self.tick_seconds,
        })
    }
}
