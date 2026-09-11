use crate::agent::{Lifetime, Role};
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
    /// Give every scope a foreman without writing one into each of them.
    #[serde(default)]
    pub foreman: ForemanConfig,
}

/// A foreman per scope, synthesised rather than written out.
///
/// Off by default, and deliberately so: switching it on starts one real agent
/// session per scope. An instance with ten scopes gets ten of them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForemanConfig {
    #[serde(default)]
    pub enabled: bool,
    /// The name the synthesised agent gets in each scope.
    #[serde(default = "default_foreman_name")]
    pub name: String,
    /// Which adapter it runs on. Unset means the instance's `default_agent` --
    /// a synthesised foreman should not quietly run a different harness from
    /// everything else in the instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// Scopes that get none. The instance root is the usual one: it is the
    /// company, not a project.
    #[serde(default = "default_foreman_exclude")]
    pub exclude: Vec<String>,
}

fn default_foreman_name() -> String {
    "foreman".into()
}
fn default_foreman_exclude() -> Vec<String> {
    vec!["root".into()]
}

impl Default for ForemanConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            name: default_foreman_name(),
            harness: None,
            exclude: default_foreman_exclude(),
        }
    }
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
            foreman: ForemanConfig::default(),
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
        /// `permanent` or `temporary` here makes this a standing agent as well
        /// as the scope's default for tasks -- which is how instances written
        /// before standing agents existed already spell it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lifetime: Option<Lifetime>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        autostart: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<Role>,
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

/// One agent a scope declares. Both spellings below land here:
///
/// ```yaml
/// agent:                    # the scope's default for tasks
///   harness: pi
///   lifetime: permanent     # ... and a standing agent, if it says so
/// agents:                   # any number of further agents
///   - name: watcher
///     harness: claude-code
///     lifetime: permanent
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeAgent {
    /// Unique within the scope. Defaults to the harness name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub harness: String,
    #[serde(default)]
    pub lifetime: Lifetime,
    #[serde(default)]
    pub role: Role,
    /// Whether the daemon brings it up by itself. Permanent agents default to
    /// yes, everything else to no.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autostart: Option<bool>,
}

impl ScopeAgent {
    pub fn name(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.harness.clone())
    }

    pub fn autostart(&self) -> bool {
        self.autostart
            .unwrap_or(self.lifetime == Lifetime::Permanent)
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
    /// Further agents in this scope. A scope can have as many as it likes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<ScopeAgent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
}

impl Scope {
    /// The adapter a task in this scope runs on unless it says otherwise.
    pub fn agent_adapter(&self) -> Option<&str> {
        self.agent.as_ref().map(AgentRef::adapter)
    }

    /// Every agent this scope declares, plus the foreman the instance adds to
    /// each scope when it is configured to.
    pub fn agents_with(&self, foreman: &ForemanConfig) -> Vec<ScopeAgent> {
        let mut out = self.declared_agents();
        if !foreman.enabled || foreman.exclude.iter().any(|e| e == &self.name) {
            return out;
        }
        // A scope that already has a foreman of its own keeps it.
        if out.iter().any(|a| a.role == Role::Foreman) {
            return out;
        }
        out.push(ScopeAgent {
            name: Some(foreman.name.clone()),
            harness: foreman
                .harness
                .clone()
                .or_else(|| self.agent_adapter().map(str::to_string))
                .unwrap_or_else(default_agent),
            lifetime: Lifetime::Permanent,
            role: Role::Foreman,
            autostart: Some(true),
        });
        out
    }

    pub fn standing_agents_with(&self, foreman: &ForemanConfig) -> Vec<ScopeAgent> {
        self.agents_with(foreman)
            .into_iter()
            .filter(|a| a.lifetime.is_standing())
            .collect()
    }

    /// Every agent this scope declares, from either spelling, in the order a
    /// person wrote them.
    pub fn declared_agents(&self) -> Vec<ScopeAgent> {
        let mut out = Vec::new();
        if let Some(AgentRef::Declared {
            harness,
            name,
            lifetime,
            autostart,
            role,
        }) = &self.agent
        {
            out.push(ScopeAgent {
                name: name.clone(),
                harness: harness.clone(),
                lifetime: lifetime.unwrap_or_default(),
                role: role.unwrap_or_default(),
                autostart: *autostart,
            });
        }
        for a in &self.agents {
            // The same agent named twice is the config's business, not ours --
            // but the same *name* twice would collide on the session id, so the
            // first one wins and the rest are ignored.
            if out.iter().any(|e| e.name() == a.name()) {
                continue;
            }
            out.push(a.clone());
        }
        out
    }

    /// Only the ones meant to exist between tasks.
    pub fn standing_agents(&self) -> Vec<ScopeAgent> {
        self.declared_agents()
            .into_iter()
            .filter(|a| a.lifetime.is_standing())
            .collect()
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
    fn a_foreman_is_added_to_every_scope_but_the_excluded_ones() {
        let s: Scope = serde_yaml_ng::from_str("name: demo\npath: .\n").unwrap();
        let root: Scope = serde_yaml_ng::from_str("name: root\npath: .\n").unwrap();
        let cfg = ForemanConfig {
            enabled: true,
            ..Default::default()
        };
        let names: Vec<String> = s.agents_with(&cfg).iter().map(|a| a.name()).collect();
        assert_eq!(names, vec!["foreman"]);
        assert_eq!(s.agents_with(&cfg)[0].role, Role::Foreman);
        assert!(s.agents_with(&cfg)[0].lifetime.is_standing());
        assert!(root.agents_with(&cfg).is_empty(), "root is excluded by default");
        assert!(s.agents_with(&ForemanConfig::default()).is_empty(), "off by default");
    }

    #[test]
    fn a_scope_that_names_its_own_foreman_keeps_it() {
        let s: Scope = serde_yaml_ng::from_str(
            "name: demo\npath: .\nagents:\n  - name: chef\n    harness: pi\n    lifetime: permanent\n    role: foreman\n",
        )
        .unwrap();
        let cfg = ForemanConfig { enabled: true, ..Default::default() };
        let names: Vec<String> = s.agents_with(&cfg).iter().map(|a| a.name()).collect();
        assert_eq!(names, vec!["chef"], "no second foreman is bolted on");
    }

    #[test]
    fn a_scope_collects_agents_from_both_spellings() {
        let s: Scope = serde_yaml_ng::from_str(
            "name: a\npath: .\nagent:\n  harness: pi\n  lifetime: permanent\n\
             agents:\n  - name: watcher\n    harness: claude-code\n    lifetime: permanent\n\
             \x20 - name: helper\n    harness: codex\n",
        )
        .unwrap();
        let declared = s.declared_agents();
        assert_eq!(declared.len(), 3);
        assert_eq!(s.agent_adapter(), Some("pi"));
        let standing: Vec<String> = s.standing_agents().iter().map(|a| a.name()).collect();
        assert_eq!(standing, vec!["pi", "watcher"], "codex is a task agent, not standing");
    }

    #[test]
    fn a_temporary_agent_parses_and_does_not_autostart() {
        // Instances written before standing agents existed already say this.
        let s: Scope = serde_yaml_ng::from_str(
            "name: a\npath: .\nagent:\n  name: reviewer\n  harness: pi\n  max_sessions: 1\n  lifetime: temporary\n",
        )
        .unwrap();
        let a = &s.standing_agents()[0];
        assert_eq!(a.name(), "reviewer");
        assert_eq!(a.lifetime, Lifetime::Temporary);
        assert!(!a.autostart(), "temporary agents wait to be asked for");
    }

    #[test]
    fn a_permanent_agent_autostarts_unless_told_not_to() {
        let s: Scope = serde_yaml_ng::from_str(
            "name: a\npath: .\nagents:\n  - harness: pi\n    lifetime: permanent\n",
        )
        .unwrap();
        assert!(s.standing_agents()[0].autostart());
        let s: Scope = serde_yaml_ng::from_str(
            "name: a\npath: .\nagents:\n  - harness: pi\n    lifetime: permanent\n    autostart: false\n",
        )
        .unwrap();
        assert!(!s.standing_agents()[0].autostart());
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
