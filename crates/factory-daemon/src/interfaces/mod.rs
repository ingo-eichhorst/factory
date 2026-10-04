pub mod http;
pub mod socket;

pub use http::HttpInterface;
pub use socket::SocketInterface;

pub(crate) use factory_infrastructure::interfaces::interface_facts;
#[cfg(test)]
use factory_core::config::InterfaceConfig;

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
