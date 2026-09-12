//! What an agent is allowed to do, and where.
//!
//! A role is a name and a list of grants. Two are built in -- `worker` and
//! `foreman` -- and an instance may name as many more as its way of working
//! needs, in the same vocabulary the daemon checks against, so the config and
//! the check cannot drift into different words.
//!
//! This bounds what an agent does by accident, not what it could do if it
//! tried: every agent runs as the owner of the instance and can reach the
//! control socket, so one that simply omits its token is indistinguishable
//! from the person sitting there. A role is a job description, not a wall.

use crate::error::{FactoryError, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The name of a role. Open rather than closed: an instance names its own.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Role(String);

impl Role {
    /// Reads the board, and works the tasks assigned to it.
    pub const WORKER: &'static str = "worker";
    /// Runs a scope: creates the work in it and hands it out.
    pub const FOREMAN: &'static str = "foreman";

    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn worker() -> Self {
        Self::new(Self::WORKER)
    }

    pub fn foreman() -> Self {
        Self::new(Self::FOREMAN)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is(&self, name: &str) -> bool {
        self.0 == name
    }
}

impl Default for Role {
    fn default() -> Self {
        // Default closed: an agent nobody gave a role is a worker.
        Self::worker()
    }
}

impl From<&str> for Role {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for Role {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One thing a role may do. These name requests, not fields: a grant says
/// *which* requests an agent may make, and `Reach` says *whose* subjects it
/// may make them about. The finer rules -- that editing your own task is not
/// the same as handing it to somebody else -- stay in `authorize()`, where
/// they can be stated in a sentence rather than encoded in a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Grant {
    #[serde(rename = "task.create")]
    TaskCreate,
    #[serde(rename = "task.edit")]
    TaskEdit,
    #[serde(rename = "task.delete")]
    TaskDelete,
    #[serde(rename = "task.run")]
    TaskRun,
    #[serde(rename = "task.cancel")]
    TaskCancel,
    #[serde(rename = "task.report")]
    TaskReport,
    #[serde(rename = "agent.start")]
    AgentStart,
    #[serde(rename = "agent.configure")]
    AgentConfigure,
    #[serde(rename = "agent.stop")]
    AgentStop,
    #[serde(rename = "agent.input")]
    AgentInput,
    #[serde(rename = "run.input")]
    RunInput,
}

impl Grant {
    pub const ALL: [Grant; 11] = [
        Grant::TaskCreate,
        Grant::TaskEdit,
        Grant::TaskDelete,
        Grant::TaskRun,
        Grant::TaskCancel,
        Grant::TaskReport,
        Grant::AgentStart,
        Grant::AgentConfigure,
        Grant::AgentStop,
        Grant::AgentInput,
        Grant::RunInput,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::TaskCreate => "task.create",
            Self::TaskEdit => "task.edit",
            Self::TaskDelete => "task.delete",
            Self::TaskRun => "task.run",
            Self::TaskCancel => "task.cancel",
            Self::TaskReport => "task.report",
            Self::AgentStart => "agent.start",
            Self::AgentConfigure => "agent.configure",
            Self::AgentStop => "agent.stop",
            Self::AgentInput => "agent.input",
            Self::RunInput => "run.input",
        }
    }

    /// The phrase used when a role is told it may not do this.
    pub fn describe(self) -> &'static str {
        match self {
            Self::TaskCreate => "create tasks",
            Self::TaskEdit => "change tasks",
            Self::TaskDelete => "delete tasks",
            Self::TaskRun => "start runs",
            Self::TaskCancel => "cancel runs",
            Self::TaskReport => "report on tasks",
            Self::AgentStart => "start agents",
            Self::AgentConfigure => "configure agents",
            Self::AgentStop => "stop agents",
            Self::AgentInput => "type into an agent's session",
            Self::RunInput => "type into a run's session",
        }
    }

    /// One written grant, which may be a wildcard: `task.*`, `agent.*`, `*`.
    /// Anything that names nothing is refused rather than ignored -- a typo
    /// that quietly grants less is the failure nobody notices.
    pub fn expand(written: &str) -> Result<Vec<Grant>> {
        let matched: Vec<Grant> = match written.trim() {
            "*" => Self::ALL.to_vec(),
            prefixed if prefixed.ends_with(".*") => {
                let prefix = &prefixed[..prefixed.len() - 1];
                Self::ALL
                    .into_iter()
                    .filter(|g| g.as_str().starts_with(prefix))
                    .collect()
            }
            exact => Self::ALL.into_iter().filter(|g| g.as_str() == exact).collect(),
        };
        if matched.is_empty() {
            return Err(FactoryError::BadRequest(format!(
                "{written:?} grants nothing. The grants are: {}",
                Self::ALL
                    .iter()
                    .map(|g| g.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        Ok(matched)
    }
}

/// How far a role's grants carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reach {
    /// Its own work: the tasks assigned to it, the run it is holding, itself.
    #[default]
    Own,
    /// Everything in its scope, whoever it belongs to. Never past the scope.
    Scope,
}

impl Reach {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Own => "own",
            Self::Scope => "scope",
        }
    }
}

/// A role as the config writes it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoleSpec {
    /// One line, for the roster and for the agent itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub describe: Option<String>,
    #[serde(default)]
    pub grants: Vec<String>,
    #[serde(default)]
    pub reach: Reach,
}

/// A role as the daemon checks against it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleDef {
    pub name: Role,
    pub describe: String,
    pub grants: BTreeSet<Grant>,
    pub reach: Reach,
}

impl RoleDef {
    pub fn allows(&self, grant: Grant) -> bool {
        self.grants.contains(&grant)
    }

    /// The grants in the order they are written, for a person reading them.
    pub fn written(&self) -> Vec<&'static str> {
        self.grants.iter().map(|g| g.as_str()).collect()
    }
}

/// Every role this instance knows: the two built in, plus whatever it named.
#[derive(Debug, Clone)]
pub struct Roles(BTreeMap<String, RoleDef>);

impl Default for Roles {
    fn default() -> Self {
        Self::presets()
    }
}

impl Roles {
    /// The two that ship, written in the same vocabulary as any other, so what
    /// a worker may do can be read rather than inferred from a match arm.
    pub fn presets() -> Self {
        let worker = RoleDef {
            name: Role::worker(),
            describe: "reads the board, and works the tasks assigned to it".into(),
            grants: [Grant::TaskEdit, Grant::TaskReport, Grant::RunInput]
                .into_iter()
                .collect(),
            reach: Reach::Own,
        };
        let foreman = RoleDef {
            name: Role::foreman(),
            describe: "runs a scope: creates the work in it and hands it out".into(),
            grants: Grant::ALL.into_iter().collect(),
            reach: Reach::Scope,
        };
        Self(BTreeMap::from([
            (Role::WORKER.to_string(), worker),
            (Role::FOREMAN.to_string(), foreman),
        ]))
    }

    /// The presets plus the instance's own. A preset cannot be redefined: an
    /// instance that could rewrite `worker` could widen every agent that never
    /// asked for a role, from one line nobody reads twice.
    pub fn resolve(written: &BTreeMap<String, RoleSpec>) -> Result<Self> {
        let mut roles = Self::presets();
        for (name, spec) in written {
            if roles.0.contains_key(name) {
                return Err(FactoryError::BadRequest(format!(
                    "{name:?} is a built-in role and cannot be redefined"
                )));
            }
            let mut grants = BTreeSet::new();
            for written in &spec.grants {
                grants.extend(Grant::expand(written)?);
            }
            roles.0.insert(
                name.clone(),
                RoleDef {
                    name: Role::new(name.clone()),
                    describe: spec
                        .describe
                        .clone()
                        .unwrap_or_else(|| "a role this instance named".into()),
                    grants,
                    reach: spec.reach,
                },
            );
        }
        Ok(roles)
    }

    pub fn get(&self, role: &Role) -> Option<&RoleDef> {
        self.0.get(role.as_str())
    }

    pub fn contains(&self, role: &Role) -> bool {
        self.0.contains_key(role.as_str())
    }

    pub fn names(&self) -> Vec<String> {
        self.0.keys().cloned().collect()
    }

    pub fn all(&self) -> impl Iterator<Item = &RoleDef> {
        self.0.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_presets_say_what_the_rules_already_said() {
        let roles = Roles::presets();
        let worker = roles.get(&Role::worker()).unwrap();
        assert_eq!(worker.reach, Reach::Own);
        assert!(worker.allows(Grant::TaskEdit));
        assert!(worker.allows(Grant::TaskReport));
        assert!(!worker.allows(Grant::TaskCreate));
        assert!(!worker.allows(Grant::AgentStart));

        let foreman = roles.get(&Role::foreman()).unwrap();
        assert_eq!(foreman.reach, Reach::Scope);
        for grant in Grant::ALL {
            assert!(foreman.allows(grant), "a foreman may {}", grant.as_str());
        }
    }

    #[test]
    fn an_agent_nobody_gave_a_role_is_a_worker() {
        assert_eq!(Role::default(), Role::worker());
        assert!(Roles::presets().contains(&Role::default()));
    }

    #[test]
    fn a_wildcard_names_every_grant_under_it() {
        assert_eq!(Grant::expand("*").unwrap().len(), Grant::ALL.len());
        let tasks = Grant::expand("task.*").unwrap();
        assert!(tasks.contains(&Grant::TaskCreate));
        assert!(!tasks.contains(&Grant::AgentStart));
        assert_eq!(Grant::expand("task.run").unwrap(), vec![Grant::TaskRun]);
    }

    #[test]
    fn a_grant_that_names_nothing_is_refused() {
        let e = Grant::expand("task.approve").unwrap_err().to_string();
        assert!(e.contains("task.approve"), "{e}");
        assert!(e.contains("task.create"), "the message lists what there is: {e}");
        assert!(Grant::expand("wat.*").is_err());
    }

    #[test]
    fn an_instance_names_its_own_roles() {
        let written = BTreeMap::from([(
            "reviewer".to_string(),
            RoleSpec {
                describe: Some("reads the board and reports".into()),
                grants: vec!["task.report".into()],
                reach: Reach::Own,
            },
        )]);
        let roles = Roles::resolve(&written).unwrap();
        let reviewer = roles.get(&Role::new("reviewer")).unwrap();
        assert!(reviewer.allows(Grant::TaskReport));
        assert!(!reviewer.allows(Grant::TaskEdit));
        assert_eq!(roles.names(), vec!["foreman", "reviewer", "worker"]);
    }

    #[test]
    fn a_preset_cannot_be_redefined() {
        let written = BTreeMap::from([(
            "worker".to_string(),
            RoleSpec {
                grants: vec!["*".into()],
                ..Default::default()
            },
        )]);
        let e = Roles::resolve(&written).unwrap_err().to_string();
        assert!(e.contains("worker"), "{e}");
    }

    #[test]
    fn a_role_is_written_as_a_plain_name() {
        let role: Role = serde_yaml_ng::from_str("foreman").unwrap();
        assert_eq!(role, Role::foreman());
        assert_eq!(serde_yaml_ng::to_string(&role).unwrap().trim(), "foreman");
        // A row written before roles were open still reads.
        let stored: Role = serde_json::from_str("\"worker\"").unwrap();
        assert_eq!(stored, Role::worker());
    }
}
