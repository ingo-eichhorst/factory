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
