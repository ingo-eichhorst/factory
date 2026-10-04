//! Authored L2 service dependencies, independent of whole-instance config.
use factory_kernel::{Duration, FactoryError};
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependenciesConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_workflow: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age: Option<Duration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<DependencyService>,
}

impl DependenciesConfig {
    /// Validate services using names supplied by config composition, without
    /// depending on agent declarations or whole-instance configuration.
    pub fn validate_services(
        &self,
        scope_name: &str,
        declared: &std::collections::BTreeSet<String>,
    ) -> factory_kernel::Result<()> {
        let mut names = std::collections::BTreeSet::new();
        for service in &self.services {
            if service.name.trim().is_empty() {
                return Err(FactoryError::BadRequest(format!(
                    "scope {:?} declares a dependency service with an empty name",
                    scope_name
                )));
            }
            if !names.insert(&service.name) {
                return Err(FactoryError::BadRequest(format!(
                    "scope {:?} declares dependency service {:?} more than once",
                    scope_name, service.name
                )));
            }
            for agent in &service.agents {
                if !declared.contains(agent) {
                    return Err(FactoryError::BadRequest(format!(
                        "dependency service {:?} names agent {:?}, which scope {:?} does not declare",
                        service.name, agent, scope_name
                    )));
                }
            }
            match service.transport {
                DependencyTransport::Network if service.endpoints.is_empty() => {
                    return Err(FactoryError::BadRequest(format!(
                        "network dependency service {:?} needs endpoints",
                        service.name
                    )))
                }
                DependencyTransport::Socket | DependencyTransport::File
                    if service.path.is_none() =>
                {
                    return Err(FactoryError::BadRequest(format!(
                        "{:?} dependency service {:?} needs path",
                        service.transport, service.name
                    )))
                }
                _ => {}
            }
        }
        Ok(())
    }
    pub fn is_empty(&self) -> bool {
        self.scan_workflow.is_none() && self.max_age.is_none() && self.services.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyTransport {
    Network,
    Socket,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyDirection {
    In,
    Out,
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyEffect {
    Read,
    Write,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyService {
    pub name: String,
    pub transport: DependencyTransport,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub endpoints: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<DependencyDirection>,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "one_or_many"
    )]
    pub effects: Vec<DependencyEffect>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub data: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<String>,
}

fn one_or_many<'de, D, T>(deserializer: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany<T> {
        One(T),
        Many(Vec<T>),
    }
    Ok(match Option::<OneOrMany<T>>::deserialize(deserializer)? {
        None => Vec::new(),
        Some(OneOrMany::One(v)) => vec![v],
        Some(OneOrMany::Many(v)) => v,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn config(service: &str) -> DependenciesConfig {
        serde_yaml_ng::from_str(&format!("services:\n - {service}\n")).unwrap()
    }

    #[test]
    fn services_validate_without_a_core_scope_or_agent_type() {
        let agents = BTreeSet::from(["finance".to_string()]);
        let services = [
            "{name: bank, transport: network, endpoints: [bank.test:443], agents: [finance], effects: read}",
            "{name: local, transport: socket, path: /tmp/service.sock, effects: [read, write]}",
            "{name: files, transport: file, path: /tmp/input}",
        ];
        for service in services {
            let config = config(service);
            config.validate_services("demo", &agents).unwrap();
            let roundtrip: DependenciesConfig =
                serde_json::from_value(serde_json::to_value(&config).unwrap()).unwrap();
            assert_eq!(config, roundtrip);
            assert!(!config.is_empty());
        }
        assert!(DependenciesConfig::default().is_empty());
    }

    #[test]
    fn invalid_services_preserve_validation_errors_and_strict_fields() {
        let agents = BTreeSet::new();
        for (service, message) in [
            ("{name: '', transport: file, path: /tmp/x}", "empty name"),
            ("{name: remote, transport: network}", "needs endpoints"),
            ("{name: local, transport: socket}", "needs path"),
            ("{name: local, transport: file}", "needs path"),
            (
                "{name: bank, transport: network, endpoints: [bank.test], agents: [auditor]}",
                "auditor",
            ),
        ] {
            assert!(config(service)
                .validate_services("demo", &agents)
                .unwrap_err()
                .to_string()
                .contains(message));
        }
        let mut duplicate = config("{name: local, transport: file, path: /tmp/x}");
        duplicate.services.push(duplicate.services[0].clone());
        assert!(duplicate
            .validate_services("demo", &agents)
            .unwrap_err()
            .to_string()
            .contains("more than once"));
        assert!(serde_yaml_ng::from_str::<DependenciesConfig>("max_gae: 30d").is_err());
        assert!(serde_yaml_ng::from_str::<DependencyService>(
            "name: local\ntransport: file\npath: /tmp/x\nsecret: forbidden"
        )
        .is_err());
    }
}
