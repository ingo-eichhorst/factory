use crate::error::{FactoryError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The directory that marks a Factory instance. Everything Factory owns lives
/// under it; nothing of Factory's is written inside a scope.
pub const FACTORY_DIR: &str = ".factory";
pub const CONFIG_FILE: &str = "config.yaml";
pub const PLUGINS_DIR: &str = "plugins";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_version")]
    pub version: u32,
    pub instance: Instance,
    #[serde(default)]
    pub daemon: DaemonConfig,
    #[serde(default)]
    pub scopes: Vec<Scope>,
    /// Where the daemon looks for out-of-process adapters, relative to
    /// `.factory/`. Defaults to `plugins`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins_dir: Option<PathBuf>,
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    /// Which task-store adapter backs the CRUD contract.
    #[serde(default = "default_store")]
    pub task_store: String,
    /// Interfaces the daemon mounts at startup.
    #[serde(default = "default_interfaces")]
    pub interfaces: Vec<InterfaceConfig>,
    /// How often the scheduler looks for due tasks.
    #[serde(default = "default_tick")]
    pub tick_seconds: u64,
    /// A dispatched task that never reports back is failed after this long.
    #[serde(default = "default_task_timeout")]
    pub task_timeout_seconds: u64,
    /// How long an agent has to say it has started. An agent that is up but
    /// sitting on a prompt nobody will answer -- a first-run trust dialog, a
    /// login -- never acknowledges, and there is no reason to hold the task
    /// open for the full run timeout to find that out.
    #[serde(default = "default_ack_timeout")]
    pub ack_timeout_seconds: u64,
    /// Defaults for tasks that do not name their own.
    #[serde(default = "default_agent")]
    pub default_agent: String,
    #[serde(default = "default_runtime")]
    pub default_runtime: String,
}

fn default_store() -> String {
    "sqlite".into()
}
fn default_tick() -> u64 {
    5
}
fn default_task_timeout() -> u64 {
    3600
}
fn default_ack_timeout() -> u64 {
    180
}
fn default_agent() -> String {
    "claude-code".into()
}
fn default_runtime() -> String {
    "herdr".into()
}
fn default_interfaces() -> Vec<InterfaceConfig> {
    vec![
        InterfaceConfig {
            kind: "cli".into(),
            settings: BTreeMap::new(),
        },
        InterfaceConfig {
            kind: "http".into(),
            settings: BTreeMap::new(),
        },
    ]
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            task_store: default_store(),
            interfaces: default_interfaces(),
            tick_seconds: default_tick(),
            task_timeout_seconds: default_task_timeout(),
            ack_timeout_seconds: default_ack_timeout(),
            default_agent: default_agent(),
            default_runtime: default_runtime(),
        }
    }
}

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
            other => serde_yaml_ng::to_string(other).ok().map(|s| s.trim().to_string()),
        })
    }
}

/// How a scope names its agent.
///
/// `agent: pi` is what this daemon writes. Instances configured before the
/// adapters existed spell it as a block with a `harness:` in it, and those
/// files are still on disk in front of people -- so read both, and let the
/// harness name be the adapter name, which is what it always was.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AgentRef {
    Name(String),
    Declared {
        harness: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

impl AgentRef {
    pub fn adapter(&self) -> &str {
        match self {
            Self::Name(n) => n,
            Self::Declared { harness, .. } => harness,
        }
    }
}

/// A directory Factory can run agents in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scope {
    pub name: String,
    /// Relative to the instance root, or absolute.
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
}

impl Scope {
    pub fn agent_adapter(&self) -> Option<&str> {
        self.agent.as_ref().map(AgentRef::adapter)
    }
}

/// A loaded instance: the config plus the root it was found under.
#[derive(Debug, Clone)]
pub struct Factory {
    pub root: PathBuf,
    pub config: Config,
}

impl Factory {
    /// Walk up from `start` looking for a `.factory/config.yaml`, the way git
    /// finds its own root.
    pub fn discover(start: &Path) -> Result<Option<PathBuf>> {
        let mut dir = if start.is_absolute() {
            start.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|e| FactoryError::Other(e.into()))?
                .join(start)
        };
        loop {
            if dir.join(FACTORY_DIR).join(CONFIG_FILE).is_file() {
                return Ok(Some(dir));
            }
            if !dir.pop() {
                return Ok(None);
            }
        }
    }

    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(FACTORY_DIR).join(CONFIG_FILE);
        let text = std::fs::read_to_string(&path).map_err(|e| {
            FactoryError::Other(anyhow::anyhow!("reading {}: {e}", path.display()))
        })?;
        let config: Config = serde_yaml_ng::from_str(&text).map_err(|e| {
            FactoryError::Other(anyhow::anyhow!("parsing {}: {e}", path.display()))
        })?;
        Ok(Self {
            root: root.to_path_buf(),
            config,
        })
    }

    pub fn factory_dir(&self) -> PathBuf {
        self.root.join(FACTORY_DIR)
    }

    pub fn plugins_dir(&self) -> PathBuf {
        match &self.config.plugins_dir {
            Some(p) if p.is_absolute() => p.clone(),
            Some(p) => self.factory_dir().join(p),
            None => self.factory_dir().join(PLUGINS_DIR),
        }
    }

    pub fn database_path(&self) -> PathBuf {
        self.factory_dir().join("factory.sqlite")
    }

    /// The control socket.
    ///
    /// It belongs in `.factory/`, but a unix socket path has to fit in
    /// `sockaddr_un.sun_path` -- 104 bytes on macOS, 108 on Linux -- and a
    /// deep instance root blows that limit. When it will not fit, fall back to
    /// a short name in the temporary directory derived from the root, so the
    /// daemon and the CLI compute the same path without either reading state
    /// the other wrote.
    pub fn socket_path(&self) -> PathBuf {
        let natural = self.factory_dir().join("factory.sock");
        if natural.as_os_str().len() < 100 {
            return natural;
        }
        let tmp = std::env::temp_dir();
        tmp.join(format!("factory-{}.sock", short_hash(&self.root)))
    }

    pub fn scope_names(&self) -> Vec<String> {
        self.config.scopes.iter().map(|s| s.name.clone()).collect()
    }

    pub fn scope(&self, name: &str) -> Result<&Scope> {
        self.config
            .scopes
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| FactoryError::NoSuchScope(name.to_string()))
    }

    /// The absolute working directory for a scope.
    pub fn scope_path(&self, name: &str) -> Result<PathBuf> {
        let scope = self.scope(name)?;
        Ok(if scope.path.is_absolute() {
            scope.path.clone()
        } else {
            self.root.join(&scope.path)
        })
    }
}

/// FNV-1a. Small, and -- unlike `DefaultHasher` -- defined to stay the same
/// from one release to the next, which is what makes both sides agree.
fn short_hash(path: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.as_os_str().as_encoded_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn factory(root: &str) -> Factory {
        Factory {
            root: PathBuf::from(root),
            config: Config {
                version: 1,
                instance: Instance { id: "i".into(), name: "n".into() },
                daemon: DaemonConfig::default(),
                scopes: vec![],
                plugins_dir: None,
            },
        }
    }

    #[test]
    fn a_scope_may_name_its_agent_either_way() {
        let plain: Scope = serde_yaml_ng::from_str("name: a\npath: .\nagent: pi\n").unwrap();
        assert_eq!(plain.agent_adapter(), Some("pi"));

        let declared: Scope = serde_yaml_ng::from_str(
            "name: a\npath: .\nagent:\n  name: Factory Builder\n  harness: pi\n  max_sessions: 1\n",
        )
        .unwrap();
        assert_eq!(declared.agent_adapter(), Some("pi"));

        let none: Scope = serde_yaml_ng::from_str("name: a\npath: .\n").unwrap();
        assert_eq!(none.agent_adapter(), None);
    }

    #[test]
    fn a_short_root_keeps_its_socket_in_the_instance() {
        let f = factory("/tmp/x");
        assert_eq!(f.socket_path(), PathBuf::from("/tmp/x/.factory/factory.sock"));
    }

    #[test]
    fn a_deep_root_falls_back_to_something_that_fits() {
        let deep = format!("/tmp/{}", "verylongsegment/".repeat(12));
        let f = factory(&deep);
        let socket = f.socket_path();
        assert!(socket.as_os_str().len() < 104, "{} is still too long", socket.display());
        assert_ne!(socket.parent(), Some(f.factory_dir().as_path()));
    }

    #[test]
    fn the_fallback_is_the_same_every_time() {
        let deep = format!("/tmp/{}", "verylongsegment/".repeat(12));
        assert_eq!(factory(&deep).socket_path(), factory(&deep).socket_path());
    }

    #[test]
    fn two_deep_roots_do_not_share_a_socket() {
        let a = format!("/tmp/{}a", "verylongsegment/".repeat(12));
        let b = format!("/tmp/{}b", "verylongsegment/".repeat(12));
        assert_ne!(factory(&a).socket_path(), factory(&b).socket_path());
    }
}
