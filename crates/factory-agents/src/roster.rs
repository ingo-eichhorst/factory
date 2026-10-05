//! L3 declarations, roster derivation and live AgentFact provider.
use crate::{
    agent::Lifetime,
    role::{Role, RoleSpec, Roles},
    role_chain::{self, ScopeRoles},
};
use factory_environment::sandbox::Sandbox;
use factory_kernel::{AgentFact, FactoryError, Result, ScopeIdentity};
use serde::{Deserialize, Deserializer, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

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
        /// See `Sandbox`'s doc comment.
        #[serde(default, skip_serializing_if = "Sandbox::is_none")]
        sandbox: Sandbox,
        /// See `ScopeAgent::openshell`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        openshell: Option<factory_environment::openshell::OpenshellConfig>,
        /// See `ScopeAgent::provider`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        /// See `ScopeAgent::max_sessions`. Instances written before `#179`
        /// already carried this key; it just had no effect.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_sessions: Option<u32>,
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
            #[serde(default)]
            sandbox: Sandbox,
            #[serde(default)]
            openshell: Option<factory_environment::openshell::OpenshellConfig>,
            #[serde(default)]
            provider: Option<String>,
            // Older Factory configs wrote this in the singular declaration.
            // It used to have no effect; `#179` makes it live.
            #[serde(default)]
            max_sessions: Option<u32>,
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
                    sandbox: declaration.sandbox,
                    openshell: declaration.openshell,
                    provider: declaration.provider,
                    max_sessions: declaration.max_sessions,
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
    /// See `Sandbox`'s doc comment.
    #[serde(default, skip_serializing_if = "Sandbox::is_none")]
    pub sandbox: Sandbox,
    /// How an `openshell` sandbox is made for this agent's runs: the image,
    /// the providers attached by name, the policy, and what crosses in and
    /// out. Present exactly when `sandbox: openshell` is -- either one alone
    /// is refused at load (`refuse_bad_openshell`). Holds no secret: a
    /// provider is named, and its credential stays in OpenShell's store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openshell: Option<factory_environment::openshell::OpenshellConfig>,
    /// The AI account this agent's model calls go to, by the name
    /// `infrastructure.providers` in the root config declares it under.
    /// Absent means the provider that claims this agent's harness, if any
    /// does -- see `Infrastructure::provider_for`. A name nothing declares
    /// is refused at load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// How many sessions of this agent may be open at once -- a run
    /// `Dispatching` or holding a session, plus one for a live permanent
    /// agent of this name (`#179`). Absent is unlimited, today's behaviour.
    /// `0` is refused at load: it would never run anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_sessions: Option<u32>,
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

pub fn default_agent() -> String {
    "claude-code".into()
}

pub fn declared_agents(agent: Option<&AgentRef>, agents: &[ScopeAgent]) -> Vec<ScopeAgent> {
    let mut out = Vec::new();
    if let Some(AgentRef::Declared {
        harness,
        name,
        lifetime,
        autostart,
        role,
        args,
        sandbox,
        openshell,
        provider,
        max_sessions,
    }) = agent
    {
        out.push(ScopeAgent {
            name: name.clone(),
            harness: harness.clone(),
            lifetime: lifetime.unwrap_or_default(),
            role: role.clone().unwrap_or_default(),
            autostart: *autostart,
            args: args.clone(),
            sandbox: *sandbox,
            openshell: openshell.clone(),
            provider: provider.clone(),
            max_sessions: *max_sessions,
        });
    }
    for a in agents {
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

pub fn agents_with(
    scope_name: &str,
    agent: Option<&AgentRef>,
    agents: &[ScopeAgent],
    foreman: &ForemanConfig,
) -> Vec<ScopeAgent> {
    let mut out = declared_agents(agent, agents);
    if !foreman.enabled {
        return out;
    }
    if foreman.exclude.iter().any(|e| names_scope(e, scope_name)) {
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
            .or_else(|| agent.map(AgentRef::adapter).map(str::to_string))
            .unwrap_or_else(default_agent),
        lifetime: Lifetime::Permanent,
        role: Role::foreman(),
        autostart: Some(true),
        args: Vec::new(),
        // Synthesized, not declared: nothing names a sandbox for a
        // foreman nobody wrote, so it gets today's default forever.
        sandbox: Sandbox::None,
        openshell: None,
        // Likewise: its harness's default provider, if one claims it.
        provider: None,
        // And no cap of its own -- only the scope's, if it has one.
        max_sessions: None,
    });
    out
}

/// Fresh plain L3 declarations; no cross-level configuration object or facts.
#[derive(Debug, Clone)]
pub struct RosterScope {
    pub name: String,
    pub path: PathBuf,
    pub agent: Option<AgentRef>,
    pub agents: Vec<ScopeAgent>,
    pub roles: BTreeMap<String, RoleSpec>,
}
impl ScopeIdentity for RosterScope {
    fn scope_name(&self) -> &str {
        &self.name
    }
    fn scope_path(&self) -> &Path {
        &self.path
    }
}
impl ScopeRoles for RosterScope {
    fn role_specs(&self) -> &BTreeMap<String, RoleSpec> {
        &self.roles
    }
}

pub struct Provider {
    pub scopes: Vec<RosterScope>,
    pub root_roles: BTreeMap<String, RoleSpec>,
    pub foreman: ForemanConfig,
}
impl factory_kernel::FactProvider for Provider {
    type Level = factory_kernel::L3;
}
#[async_trait::async_trait]
impl factory_kernel::Provide<AgentFact> for Provider {
    type Query = String;
    type Value = Vec<AgentFact>;
    type Error = FactoryError;
    async fn get(&self, query: &String) -> Result<Self::Value> {
        let scope = factory_kernel::resolve_scope(&self.scopes, query)?;
        let roles = role_chain::roles_for_scope(&self.root_roles, &self.scopes, scope)
            .unwrap_or_else(|e| {
                tracing::error!(
                    scope = scope.name,
                    "{e}; falling back to the built-in roles"
                );
                Roles::presets()
            });
        Ok(agent_facts_for(scope, &self.foreman, &roles))
    }
}
pub fn agent_facts_for(
    scope: &RosterScope,
    foreman: &ForemanConfig,
    roles: &Roles,
) -> Vec<AgentFact> {
    // A synthesized foreman is a real dispatchable agent and has no
    // sandbox; do not exempt it by reading only declared_agents.
    agents_with(&scope.name, scope.agent.as_ref(), &scope.agents, foreman)
        .into_iter()
        .map(|agent| {
            let grants = roles.get(&agent.role).map(|def| def.grants.clone());
            AgentFact {
                name: agent.name(),
                role: agent.role.as_str().to_string(),
                grants,
                has_sandbox: !agent.sandbox.is_none(),
                sandbox_enforced: agent.sandbox.is_enforced(),
            }
        })
        .collect()
}

#[async_trait::async_trait]
impl factory_kernel::Provide<factory_kernel::FunctionaryRosterFact> for Provider {
    type Query = String;
    type Value = factory_kernel::FunctionaryRosterFact;
    type Error = FactoryError;
    async fn get(&self, query: &String) -> Result<Self::Value> {
        let scope = factory_kernel::resolve_scope(&self.scopes, query)?;
        Ok(factory_kernel::FunctionaryRosterFact {
            scope: scope.name.clone(),
            default_agent: scope.agent.as_ref().map(|agent|agent.adapter().to_owned()),
            names: agents_with(&scope.name, scope.agent.as_ref(), &scope.agents, &self.foreman)
                .iter().map(|agent|agent.name()).collect(),
        })
    }
}

#[cfg(test)]
mod tests;
