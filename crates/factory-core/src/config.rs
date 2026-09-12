use crate::agent::Lifetime;
use crate::role::{Role, RoleSpec, Roles};
use crate::error::{FactoryError, Result};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The directory that marks a Factory instance or a configured scope. Runtime
/// state belongs to the instance root's directory; nested copies contain only
/// the configuration for the scope whose directory they sit in.
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
    /// The scope rooted in the same directory as this config file. On the
    /// instance root this sits alongside the instance-wide settings above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Scope>,
    /// Scopes loaded from their own `.factory/config.yaml` files at startup.
    /// This runtime list is never written back into the instance config. The
    /// old central list remains parse-compatible, but discovery replaces it.
    #[serde(default, skip_serializing)]
    pub scopes: Vec<Scope>,
    /// Roles this instance names for itself, on top of `worker` and `foreman`.
    /// Keyed by the name an agent is given in its `role:`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub roles: BTreeMap<String, RoleSpec>,
    /// Where the daemon looks for out-of-process adapters, relative to
    /// `.factory/`. Defaults to `plugins`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins_dir: Option<PathBuf>,
}

impl Config {
    /// Every role this instance knows: the two built in, plus its own.
    pub fn roles(&self) -> Result<Roles> {
        Roles::resolve(&self.roles)
    }

    /// Refuse a config that gives an agent a role nothing defines, and say
    /// which agent it was. Falling back to the default instead would demote an
    /// agent on a typo and never mention it.
    pub fn validate(&self) -> Result<()> {
        let roles = self.roles()?;
        for scope in self.scope.iter().chain(&self.scopes) {
            for agent in scope.declared_agents() {
                if agent.harness == "shell" && !agent.args.is_empty() {
                    return Err(FactoryError::BadRequest(format!(
                        "scope {:?} gives shell agent {:?} arguments, but the shell agent runs the task's instructions directly and cannot use them",
                        scope.name,
                        agent.name(),
                    )));
                }
                if !roles.contains(&agent.role) {
                    return Err(FactoryError::BadRequest(format!(
                        "scope {:?} gives {:?} the role {:?}, which this instance does not define. \
                         The roles it has are: {}",
                        scope.name,
                        agent.name(),
                        agent.role.as_str(),
                        roles.names().join(", ")
                    )));
                }
            }
        }
        Ok(())
    }

    /// Validate the instance file before discovery replaces its legacy scope
    /// list. Local scope files are checked by `validate` after discovery.
    pub fn validate_instance(&self) -> Result<()> {
        let roles = self.roles()?;
        if let Some(scope) = &self.scope {
            for agent in scope.declared_agents() {
                if agent.harness == "shell" && !agent.args.is_empty() {
                    return Err(FactoryError::BadRequest(format!(
                        "scope {:?} gives shell agent {:?} arguments, but the shell agent runs the task's instructions directly and cannot use them",
                        scope.name,
                        agent.name(),
                    )));
                }
                if !roles.contains(&agent.role) {
                    return Err(FactoryError::BadRequest(format!(
                        "scope {:?} gives {:?} the role {:?}, which this instance does not define. \
                         The roles it has are: {}",
                        scope.name,
                        agent.name(),
                        agent.role.as_str(),
                        roles.names().join(", ")
                    )));
                }
            }
        }
        Ok(())
    }
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
    /// How long a run reported `Blocked` by a runtime hook may sit waiting
    /// for a human before the daemon gives up on it too. Deliberately its own
    /// clock rather than a reuse of `task_timeout_seconds`: a person needs a
    /// real chance to notice and answer, but a wedged run still cannot wait
    /// forever with nobody told. A day by default.
    #[serde(default = "default_blocked_timeout")]
    pub blocked_timeout_seconds: u64,
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
fn default_blocked_timeout() -> u64 {
    86400
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
            blocked_timeout_seconds: default_blocked_timeout(),
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
#[derive(Debug, Clone, Serialize)]
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
        /// Arguments added after the adapter's own defaults for this agent.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
    },
}

impl<'de> Deserialize<'de> for AgentRef {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Declaration {
            harness: String,
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            lifetime: Option<Lifetime>,
            #[serde(default)]
            autostart: Option<bool>,
            #[serde(default)]
            role: Option<Role>,
            #[serde(default)]
            args: Vec<String>,
            // Older Factory configs wrote this in the singular declaration.
            // It has no effect now, but those files must continue to load.
            #[serde(default, rename = "max_sessions")]
            _max_sessions: Option<u32>,
        }

        let value = serde_yaml_ng::Value::deserialize(deserializer)?;
        match value {
            serde_yaml_ng::Value::String(name) => Ok(Self::Name(name)),
            serde_yaml_ng::Value::Mapping(_) => {
                let declaration: Declaration =
                    serde_yaml_ng::from_value(value).map_err(serde::de::Error::custom)?;
                Ok(Self::Declared {
                    harness: declaration.harness,
                    name: declaration.name,
                    lifetime: declaration.lifetime,
                    autostart: declaration.autostart,
                    role: declaration.role,
                    args: declaration.args,
                })
            }
            _ => Err(serde::de::Error::custom(
                "agent must be an adapter name or a declaration",
            )),
        }
    }
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
#[serde(deny_unknown_fields)]
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
    /// Arguments added after the adapter's own defaults for this agent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
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
    /// Stable identity stored with the scope rather than inferred from its
    /// location. Discovery requires this to be non-empty.
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// Derived from the directory containing `.factory/config.yaml`.
    #[serde(default, skip_serializing)]
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
    /// Which task-store adapter holds this scope's tasks. Absent means the
    /// instance default, `daemon.task_store`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_store: Option<String>,
}

/// The last `/`-separated segment of `s`, or all of `s` when it has none.
/// This is what a bare scope name -- one written before a scope's identity
/// became its path -- is compared against: see `Factory::scope` and the
/// `exclude` check in `agents_with`.
fn last_segment(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

/// Whether `pattern` (an `exclude` entry, or a name somebody typed) means
/// `scope_name`: exactly, or -- when `pattern` carries no `/` of its own --
/// by matching just its last segment. A pattern that does name a path is
/// never loosened this way, so a full path always means exactly itself.
fn names_scope(pattern: &str, scope_name: &str) -> bool {
    pattern == scope_name || (!pattern.contains('/') && last_segment(scope_name) == pattern)
}

impl Scope {
    /// The adapter a task in this scope runs on unless it says otherwise.
    pub fn agent_adapter(&self) -> Option<&str> {
        self.agent.as_ref().map(AgentRef::adapter)
    }

    /// Every agent this scope declares, plus the foreman the instance adds.
    /// Discovery admits only explicitly configured scope directories.
    pub fn agents_with(&self, foreman: &ForemanConfig) -> Vec<ScopeAgent> {
        let mut out = self.declared_agents();
        if !foreman.enabled {
            return out;
        }
        if foreman.exclude.iter().any(|e| names_scope(e, &self.name)) {
            return out;
        }
        // A scope that already has a foreman of its own keeps it.
        if out.iter().any(|a| a.role.is(Role::FOREMAN)) {
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
            role: Role::foreman(),
            autostart: Some(true),
            args: Vec::new(),
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
            args,
        }) = &self.agent
        {
            out.push(ScopeAgent {
                name: name.clone(),
                harness: harness.clone(),
                lifetime: lifetime.unwrap_or_default(),
                role: role.clone().unwrap_or_default(),
                autostart: *autostart,
                args: args.clone(),
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
    /// Walk up from `start` looking for the config that owns an instance. A
    /// nested scope has the same marker path, so scope-only files are passed
    /// over until a document with `instance:` is found.
    pub fn discover(start: &Path) -> Result<Option<PathBuf>> {
        let mut dir = if start.is_absolute() {
            start.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|e| FactoryError::Other(e.into()))?
                .join(start)
        };
        loop {
            let config = dir.join(FACTORY_DIR).join(CONFIG_FILE);
            if config.is_file() {
                let text = std::fs::read_to_string(&config).map_err(|e| {
                    FactoryError::Other(anyhow::anyhow!("reading {}: {e}", config.display()))
                })?;
                match serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text) {
                    Ok(serde_yaml_ng::Value::Mapping(values))
                        if values.contains_key(serde_yaml_ng::Value::String("instance".into())) =>
                    {
                        return Ok(Some(dir));
                    }
                    // Let `load` report a malformed nearby marker rather than
                    // silently hiding it behind an instance farther up.
                    Err(_) => return Ok(Some(dir)),
                    _ => {}
                }
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
        config.validate_instance()?;
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

    /// Where a run's own worktree lives, when its task asked for one. Under
    /// `.factory/`, never inside a scope -- one subdirectory per run, named
    /// after the run once it exists, so a retry never inherits the dirty tree
    /// a failed attempt left behind.
    pub fn worktrees_dir(&self) -> PathBuf {
        self.factory_dir().join("worktrees")
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

    /// Every scope discovered from its own local configuration.
    pub fn scope_names(&self) -> Vec<String> {
        self.config
            .scopes
            .iter()
            .map(|s| s.name.clone())
            .collect()
    }

    /// Look a scope up by its configured name, then by the path-shaped names
    /// older tasks may still carry. Discovery makes configured names unique.
    ///
    /// Older tasks may carry either the relative path used by directory
    /// discovery or the leaf name used before that. Exact relative paths are
    /// accepted; a leaf resolves only when one scope matches. More than one is
    /// refused rather than guessed at.
    pub fn scope(&self, name: &str) -> Result<&Scope> {
        if let Some(s) = self.config.scopes.iter().find(|s| s.name == name) {
            return Ok(s);
        }
        if let Some(s) = self.config.scopes.iter().find(|s| {
            s.path
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
                == name
        }) {
            return Ok(s);
        }
        if !name.contains('/') {
            let matches: Vec<&Scope> = self
                .config
                .scopes
                .iter()
                .filter(|s| {
                    last_segment(&s.name) == name
                        || s.path.file_name().map(|p| p == name).unwrap_or(false)
                })
                .collect();
            match matches.len() {
                0 => {}
                1 => return Ok(matches[0]),
                _ => {
                    let candidates: Vec<&str> = matches.iter().map(|s| s.name.as_str()).collect();
                    return Err(FactoryError::BadRequest(format!(
                        "{name:?} could mean any of: {} -- name one of these instead",
                        candidates.join(", ")
                    )));
                }
            }
        }
        Err(FactoryError::NoSuchScope(name.to_string()))
    }

    /// `name`'s canonical identity, for joining data written under the name a
    /// scope used to have -- a task's `scope` field, a run's liveness --
    /// against `Scope.name` as it reads today. Falls back to `name` itself
    /// when nothing resolves it at all, so a scope that is genuinely gone
    /// still groups its old data under the name it was last known by instead
    /// of losing it to a join that silently matches nothing.
    pub fn canonical_scope_name(&self, name: &str) -> String {
        self.scope(name)
            .map(|s| s.name.clone())
            .unwrap_or_else(|_| name.to_string())
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

    /// The task-store adapter a scope's tasks live in. A scope that does not
    /// name one uses the instance default.
    pub fn task_store_for(&self, scope: &str) -> &str {
        self.config
            .scopes
            .iter()
            .find(|s| s.name == scope)
            .and_then(|s| s.task_store.as_deref())
            .unwrap_or(&self.config.daemon.task_store)
    }

    /// Every distinct task-store adapter this instance uses: the default,
    /// plus whatever the scopes name. In a stable order, default first.
    pub fn task_stores(&self) -> Vec<String> {
        let mut out = vec![self.config.daemon.task_store.clone()];
        for scope in &self.config.scopes {
            if let Some(store) = &scope.task_store {
                if !out.contains(store) {
                    out.push(store.clone());
                }
            }
        }
        out
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
                scope: None,
                scopes: vec![],
                roles: BTreeMap::new(),
                plugins_dir: None,
            },
        }
    }

    /// A factory rooted at `/inst`, carrying exactly the scopes given --
    /// for the resolution tests below, where the root itself never matters.
    fn factory_with(scopes: Vec<Scope>) -> Factory {
        let mut f = factory("/inst");
        f.config.scopes = scopes;
        f
    }

    fn scope_at(name: &str, path: &str) -> Scope {
        let mut scope: Scope = serde_yaml_ng::from_str(&format!(
            "id: {name}-id\nname: {name}\n"
        ))
        .unwrap();
        scope.path = PathBuf::from(path);
        scope
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
        assert_eq!(s.agents_with(&cfg)[0].role, Role::foreman());
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
    fn foreman_exclude_matches_a_bare_name_by_its_last_segment() {
        // A pre-migration `exclude: ["demo"]` must keep meaning the same
        // directory once that scope's identity becomes `projects/demo`.
        let s: Scope = serde_yaml_ng::from_str("name: projects/demo\npath: projects/demo\n").unwrap();
        let cfg = ForemanConfig {
            enabled: true,
            exclude: vec!["demo".into()],
            ..Default::default()
        };
        assert!(s.agents_with(&cfg).is_empty(), "the bare exclude entry still reaches it");

        // A full path in `exclude` is not loosened the same way -- it means
        // exactly the scope it names.
        let other: Scope = serde_yaml_ng::from_str("name: projects/other\npath: projects/other\n").unwrap();
        let cfg = ForemanConfig {
            enabled: true,
            exclude: vec!["projects/demo".into()],
            ..Default::default()
        };
        assert!(!other.agents_with(&cfg).is_empty(), "a path-shaped exclude does not match a sibling");
    }

    #[test]
    fn a_bare_name_resolves_to_the_scope_whose_last_segment_matches() {
        let f = factory_with(vec![
            scope_at("projects/factory", "projects/factory"),
            scope_at("projects/other", "projects/other"),
        ]);
        // No scope is literally named "factory" any more -- only the last
        // segment of one matches -- and the fallback still finds it.
        assert_eq!(f.scope("factory").unwrap().name, "projects/factory");
        // The full identity keeps working too.
        assert_eq!(f.scope("projects/other").unwrap().name, "projects/other");
    }

    #[test]
    fn an_old_path_name_resolves_after_the_scope_gets_a_configured_name() {
        let f = factory_with(vec![scope_at("demo-project", "projects/demo")]);
        assert_eq!(f.scope("projects/demo").unwrap().name, "demo-project");
        assert_eq!(f.scope("demo").unwrap().name, "demo-project");
    }

    #[test]
    fn an_ambiguous_bare_name_is_refused_with_the_candidates_named() {
        let f = factory_with(vec![
            scope_at("projects/a/src", "projects/a/src"),
            scope_at("projects/b/src", "projects/b/src"),
        ]);
        let e = f.scope("src").unwrap_err().to_string();
        assert!(e.contains("projects/a/src"), "{e}");
        assert!(e.contains("projects/b/src"), "{e}");
    }

    #[test]
    fn canonical_scope_name_normalizes_a_legacy_bare_name_and_keeps_an_unknown_one() {
        let f = factory_with(vec![scope_at("projects/factory", "projects/factory")]);
        assert_eq!(f.canonical_scope_name("factory"), "projects/factory");
        assert_eq!(
            f.canonical_scope_name("gone"),
            "gone",
            "a scope that resolves to nothing keeps its old data grouped under the name it had"
        );
    }

    fn config_with(yaml: &str) -> Config {
        serde_yaml_ng::from_str(&format!(
            "instance:\n  id: i\n  name: n\n{yaml}"
        ))
        .unwrap()
    }

    #[test]
    fn a_role_nothing_defines_is_refused_with_the_agent_named() {
        let c = config_with(
            "scopes:\n  - name: demo\n    path: .\n    agents:\n      - name: watcher\n        harness: pi\n        role: reviewer\n",
        );
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("watcher"), "{e}");
        assert!(e.contains("reviewer"), "{e}");
        assert!(e.contains("worker"), "it says what there is instead: {e}");
    }

    #[test]
    fn a_role_the_instance_named_is_accepted() {
        let c = config_with(
            "roles:\n  reviewer:\n    grants: [task.report]\nscopes:\n  - name: demo\n    path: .\n    agents:\n      - name: watcher\n        harness: pi\n        role: reviewer\n",
        );
        c.validate().unwrap();
        assert!(c.roles().unwrap().contains(&Role::new("reviewer")));
    }

    #[test]
    fn shell_agent_arguments_are_refused_with_the_declaration_named() {
        for declaration in [
            "    agent:\n      harness: shell\n      args: [--login]\n",
            "    agents:\n      - name: scripted\n        harness: shell\n        args: [--login]\n",
        ] {
            let c = config_with(&format!(
                "scopes:\n  - name: demo\n    path: .\n{declaration}"
            ));
            let e = c.validate().unwrap_err().to_string();
            assert!(e.contains("demo"), "{e}");
            assert!(e.contains("shell"), "{e}");
            assert!(e.contains("arguments"), "{e}");
        }
    }

    #[test]
    fn misspelled_agent_arguments_are_not_silently_ignored() {
        for yaml in [
            "name: a\npath: .\nagent:\n  harness: pi\n  arg: [--model, opus]\n",
            "name: a\npath: .\nagents:\n  - harness: pi\n    arg: [--model, opus]\n",
        ] {
            let e = serde_yaml_ng::from_str::<Scope>(yaml)
                .unwrap_err()
                .to_string();
            assert!(
                e.contains("arg"),
                "the error should name the bad field: {e}"
            );
        }
    }

    #[test]
    fn agent_arguments_have_to_be_a_list() {
        let e = serde_yaml_ng::from_str::<Scope>(
            "name: a\npath: .\nagent:\n  harness: pi\n  args: --model opus\n",
        )
        .unwrap_err()
        .to_string();
        assert!(
            e.contains("sequence"),
            "the error should say what args expects: {e}"
        );
    }

    #[test]
    fn the_two_that_ship_need_no_declaring() {
        let c = config_with(
            "scopes:\n  - name: demo\n    path: .\n    agents:\n      - name: boss\n        harness: pi\n        role: foreman\n",
        );
        c.validate().unwrap();
    }

    #[test]
    fn a_scope_collects_agents_from_both_spellings() {
        let s: Scope = serde_yaml_ng::from_str(
            "name: a\npath: .\nagent:\n  harness: pi\n  lifetime: permanent\n  args: [--model, opus]\n\
             agents:\n  - name: watcher\n    harness: claude-code\n    lifetime: permanent\n\
             \x20   args: [--model, haiku]\n\
             \x20 - name: helper\n    harness: codex\n",
        )
        .unwrap();
        let declared = s.declared_agents();
        assert_eq!(declared.len(), 3);
        assert_eq!(s.agent_adapter(), Some("pi"));
        assert_eq!(declared[0].args, ["--model", "opus"]);
        assert_eq!(declared[1].args, ["--model", "haiku"]);
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
    fn instance_discovery_walks_past_a_nested_scope_config() {
        let root = std::env::temp_dir().join(format!(
            "factory-config-discovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let scope = root.join("projects/demo");
        std::fs::create_dir_all(root.join(FACTORY_DIR)).unwrap();
        std::fs::create_dir_all(scope.join(FACTORY_DIR)).unwrap();
        std::fs::write(
            root.join(FACTORY_DIR).join(CONFIG_FILE),
            "version: 1\ninstance: { id: i, name: instance }\n",
        )
        .unwrap();
        std::fs::write(
            scope.join(FACTORY_DIR).join(CONFIG_FILE),
            "version: 1\nscope: { id: s, name: demo }\n",
        )
        .unwrap();

        assert_eq!(Factory::discover(&scope.join("src")).unwrap(), Some(root.clone()));
        std::fs::remove_dir_all(root).unwrap();
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

    #[test]
    fn a_scope_that_names_a_task_store_gets_it() {
        let mut f = factory("/tmp/x");
        f.config.scopes = vec![
            serde_yaml_ng::from_str("name: demo\npath: .\ntask_store: postgres\n").unwrap(),
        ];
        assert_eq!(f.task_store_for("demo"), "postgres");
    }

    #[test]
    fn a_scope_without_a_task_store_uses_the_instance_default() {
        let mut f = factory("/tmp/x");
        f.config.scopes = vec![serde_yaml_ng::from_str("name: demo\npath: .\n").unwrap()];
        assert_eq!(f.task_store_for("demo"), "sqlite");
    }

    #[test]
    fn an_unknown_scope_name_gets_the_instance_default() {
        let f = factory("/tmp/x");
        assert_eq!(f.task_store_for("ghost"), "sqlite");
    }

    #[test]
    fn task_stores_lists_the_default_first_and_dedupes_the_rest() {
        let mut f = factory("/tmp/x");
        f.config.scopes = vec![
            serde_yaml_ng::from_str("name: a\npath: .\ntask_store: sqlite\n").unwrap(),
            serde_yaml_ng::from_str("name: b\npath: .\ntask_store: postgres\n").unwrap(),
            serde_yaml_ng::from_str("name: c\npath: .\ntask_store: postgres\n").unwrap(),
            serde_yaml_ng::from_str("name: d\npath: .\n").unwrap(),
        ];
        assert_eq!(
            f.task_stores(),
            vec!["sqlite".to_string(), "postgres".to_string()],
            "the instance default leads, and postgres only appears once"
        );
    }

    #[test]
    fn a_config_with_no_scope_naming_a_task_store_still_parses() {
        // The back-compat guarantee that matters: instances written before
        // this field existed must keep loading unchanged.
        let yaml = "version: 1\ninstance:\n  id: i\n  name: n\nscopes:\n  - name: demo\n    path: .\n";
        let cfg: Config = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(cfg.scopes[0].task_store, None);
    }

    #[test]
    fn a_scopes_task_store_round_trips_and_stays_silent_when_unset() {
        let named: Scope =
            serde_yaml_ng::from_str("name: demo\npath: .\ntask_store: postgres\n").unwrap();
        assert!(serde_yaml_ng::to_string(&named).unwrap().contains("task_store: postgres"));

        let unnamed: Scope = serde_yaml_ng::from_str("name: demo\npath: .\n").unwrap();
        assert!(!serde_yaml_ng::to_string(&unnamed).unwrap().contains("task_store"));
    }
}
