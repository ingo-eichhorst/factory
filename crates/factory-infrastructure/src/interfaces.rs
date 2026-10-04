//! L1 configured interface vocabulary and the sole bind derivation.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The listener and all readers share this fallback through `http_bind`.
pub const DEFAULT_HTTP_BIND: &str = "127.0.0.1:8787";

/// An interface adapter to mount. `kind` names the adapter; everything else is
/// passed through untouched, so a plugin interface can carry its own settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterfaceConfig {
    pub kind: String,
    #[serde(flatten, default)]
    pub settings: BTreeMap<String, serde_yaml_ng::Value>,
}

impl InterfaceConfig {
    pub fn string(&self, key: &str) -> Option<String> {
        self.settings.get(key).and_then(|v| match v {
            serde_yaml_ng::Value::String(s) => Some(s.clone()),
            other => serde_yaml_ng::to_string(other)
                .ok()
                .map(|s| s.trim().to_string()),
        })
    }

    /// The bind this interface would use as `http`: its own `settings.bind`
    /// when it names one, else [`DEFAULT_HTTP_BIND`]. Every reader of a
    /// configured http interface's address goes through this rather than
    /// repeating the fallback.
    pub fn http_bind(&self) -> String {
        self.string("bind")
            .unwrap_or_else(|| DEFAULT_HTTP_BIND.to_string())
    }
}

/// One interface the config mounts. `bind` is the one field on this page
/// that is left out rather than `null`: the `cli` interface is a socket and
/// has no address, which is not the same as one that could not be read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InterfaceFacts {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<String>,
}

pub fn interface_facts(interfaces: &[InterfaceConfig]) -> Vec<InterfaceFacts> {
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
