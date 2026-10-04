//! A launch request as data, independent of adapters or provisioning.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How a runtime is to bring this agent up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchKind {
    /// A kind the runtime recognises by name and knows how to detect
    /// (`pi`, `claude`, `codex`, ...).
    Named(String),
    /// A command line to run in the session. The escape hatch that keeps
    /// plugin agents possible on a runtime with a closed list of known kinds.
    Command(Vec<String>),
}

/// What the agent adapter asks the runtime for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchSpec {
    pub kind: LaunchKind,
    /// Extra arguments handed to the agent process.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment for the session. The callback contract travels here as well
    /// as in the prompt, so an agent can read it instead of parsing prose.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// The agent kind a `Command` launch brings up itself, when it does --
    /// a harness started inside a sandbox through a launcher (`#218`). A
    /// runtime that detects agents can then hold that session as the agent
    /// it is, under the session's name, exactly like one it started by
    /// name. `None` for everything else, and absent from the wire then, so
    /// a plugin that predates it sees the same launch it always did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_kind: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn legacy_launch_json_is_unchanged_when_no_harness_kind_is_present() {
        let old = json!({"kind": {"command": ["launcher"]}, "args": [], "env": {}});
        let launch: LaunchSpec = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(launch.agent_kind, None);
        assert_eq!(serde_json::to_value(launch).unwrap(), old);
        let minimal: LaunchSpec =
            serde_json::from_value(json!({"kind": {"named": "claude"}})).unwrap();
        assert!(minimal.args.is_empty() && minimal.env.is_empty() && minimal.agent_kind.is_none());
    }

    #[test]
    fn explicit_command_harness_kind_round_trips_and_rejects_wrong_types() {
        let encoded = json!({
            "kind": {"command": ["launcher"]}, "args": [], "env": {}, "agent_kind": "claude"
        });
        let launch: LaunchSpec = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(launch.agent_kind.as_deref(), Some("claude"));
        assert_eq!(serde_json::to_value(launch).unwrap(), encoded);
        assert!(serde_json::from_value::<LaunchSpec>(json!({
            "kind": {"command": ["launcher"]}, "agent_kind": 1
        }))
        .is_err());
    }
}
