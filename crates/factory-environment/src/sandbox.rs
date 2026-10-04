use serde::{Deserialize, Serialize};

/// Where a run declared with this agent executes.
///
/// `openshell` is enforced (`#218`): a task run of an agent that declares it
/// starts its harness inside an NVIDIA OpenShell sandbox -- created for the
/// run, carrying the policy, image and providers its `openshell:` block
/// names, and deleted when the run ends -- and a run that cannot get one
/// fails with the reason rather than starting on the host. See
/// `crate::openshell` and the daemon's `openshell` module.
///
/// `docker` and `srt` are still **declared only**: shown on the L2
/// Environment page's Sandboxes tab and read by the policy
/// `sandbox` check as evidence, but nothing in dispatch acts on them, so an
/// agent declaring one starts exactly as `none` does. `none` is the default,
/// today's behaviour, unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sandbox {
    #[default]
    None,
    Docker,
    Srt,
    Openshell,
}

impl Sandbox {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Docker => "docker",
            Self::Srt => "srt",
            Self::Openshell => "openshell",
        }
    }

    /// Whether dispatch actually puts a run of this agent inside the
    /// sandbox, as opposed to only recording that someone said so. The one
    /// place that answer lives, so the L2 tab, L3's facts and the policy
    /// check cannot disagree about it.
    pub fn is_enforced(self) -> bool {
        matches!(self, Self::Openshell)
    }

    /// So a config file nobody asked to change never grows a `sandbox: none`
    /// line: see `skip_serializing_if` on every field that carries this.
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
}
