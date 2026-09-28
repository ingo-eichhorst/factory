pub mod http;
pub mod socket;

pub use http::HttpInterface;
pub use socket::SocketInterface;

use factory_core::config::InterfaceConfig;
use factory_core::protocol::InterfaceFacts;

/// `DaemonConfig.interfaces`, resolved into the facts payload both
/// `Engine::infrastructure` (L1's `DaemonFacts.interfaces`) and
/// `policies::daemon_facts` (L6's `http_loopback_only`) show -- one
/// derivation of what each interface would actually bind to, not two
/// (#193, phase 1, F8). `cli` is a socket with no address, shown on its own
/// line rather than as `null`; `http` falls back to
/// [`factory_core::config::DEFAULT_HTTP_BIND`] through
/// [`InterfaceConfig::http_bind`] when `settings` names none; any other
/// kind is shown only if it names its own `bind`.
pub(crate) fn interface_facts(interfaces: &[InterfaceConfig]) -> Vec<InterfaceFacts> {
    interfaces
        .iter()
        .map(|interface| InterfaceFacts {
            kind: interface.kind.clone(),
            bind: match interface.kind.as_str() {
                "cli" => None,
                "http" => Some(interface.http_bind()),
                _ => interface.string("bind"),
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interface(kind: &str, bind: Option<&str>) -> InterfaceConfig {
        let mut settings = std::collections::BTreeMap::new();
        if let Some(bind) = bind {
            settings.insert("bind".to_string(), serde_yaml_ng::Value::String(bind.to_string()));
        }
        InterfaceConfig { kind: kind.to_string(), settings }
    }

    #[test]
    fn cli_has_no_bind_http_falls_back_and_other_kinds_show_their_own() {
        let interfaces = vec![
            interface("cli", None),
            interface("http", None),
            interface("http", Some("0.0.0.0:9000")),
            interface("plugin-interface", Some("unix:/tmp/x.sock")),
            interface("plugin-interface", None),
        ];
        let facts = interface_facts(&interfaces);
        let shape: Vec<(&str, Option<&str>)> =
            facts.iter().map(|f| (f.kind.as_str(), f.bind.as_deref())).collect();
        assert_eq!(
            shape,
            vec![
                ("cli", None),
                ("http", Some(factory_core::config::DEFAULT_HTTP_BIND)),
                ("http", Some("0.0.0.0:9000")),
                ("plugin-interface", Some("unix:/tmp/x.sock")),
                ("plugin-interface", None),
            ]
        );
    }
}
