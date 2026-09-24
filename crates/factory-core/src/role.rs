//! What an agent is allowed to do, and where.
//!
//! A role is a name and a list of grants. Two are built in -- `worker` and
//! `foreman` -- and an instance may name as many more as its way of working
//! needs, in the same vocabulary the daemon checks against, so the config and
//! the check cannot drift into different words.
//!
//! Roles are written in layers: the two built in, the instance root's
//! `roles:`, then each scope's `scope.roles` from the top of the tree down to
//! the scope itself. The nearest definition wins, and it wins whole -- a
//! definition that merged grants with the one it replaced could only ever
//! widen, and nobody reading either file could say what the result was.
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
    #[serde(rename = "workflow.create")]
    WorkflowCreate,
    #[serde(rename = "workflow.edit")]
    WorkflowEdit,
    #[serde(rename = "workflow.delete")]
    WorkflowDelete,
    #[serde(rename = "workflow.run")]
    WorkflowRun,
    #[serde(rename = "workflow.cancel")]
    WorkflowCancel,
    /// Add files to the knowledge base. The root scope is the subject: the
    /// knowledge base is company-wide, not one project's.
    #[serde(rename = "knowledge.write")]
    KnowledgeWrite,
    /// Create, edit, and delete datasets and their cases. The root scope is
    /// the subject: datasets are company-wide, not one project's.
    #[serde(rename = "dataset.edit")]
    DatasetEdit,
    /// Start, cancel, and clean bench runs. The root scope is the subject,
    /// for the same reason as `dataset.edit`.
    #[serde(rename = "bench.run")]
    BenchRun,
    /// Record an attestation for a control, or withdraw one already
    /// recorded. The root scope is the subject, for the same reason as
    /// `knowledge.write`: an attestation speaks for the company, not for
    /// one project.
    #[serde(rename = "policy.attest")]
    PolicyAttest,
}

impl Grant {
    pub const ALL: [Grant; 20] = [
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
        Grant::WorkflowCreate,
        Grant::WorkflowEdit,
        Grant::WorkflowDelete,
        Grant::WorkflowRun,
        Grant::WorkflowCancel,
        Grant::KnowledgeWrite,
        Grant::DatasetEdit,
        Grant::BenchRun,
        Grant::PolicyAttest,
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
            Self::WorkflowCreate => "workflow.create",
            Self::WorkflowEdit => "workflow.edit",
            Self::WorkflowDelete => "workflow.delete",
            Self::WorkflowRun => "workflow.run",
            Self::WorkflowCancel => "workflow.cancel",
            Self::KnowledgeWrite => "knowledge.write",
            Self::DatasetEdit => "dataset.edit",
            Self::BenchRun => "bench.run",
            Self::PolicyAttest => "policy.attest",
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
            Self::WorkflowCreate => "create workflows",
            Self::WorkflowEdit => "change workflows",
            Self::WorkflowDelete => "delete workflows",
            Self::WorkflowRun => "start workflows",
            Self::WorkflowCancel => "cancel workflows",
            Self::KnowledgeWrite => "add files to the knowledge base",
            Self::DatasetEdit => "create, edit, and delete datasets and their cases",
            Self::BenchRun => "start, cancel, and clean bench runs",
            Self::PolicyAttest => "record and withdraw policy attestations",
        }
    }

    /// Which part of the board this grant is about, as a person reads it. The
    /// view groups a role's grants by this rather than keeping its own copy of
    /// which grant belongs where.
    pub fn group(self) -> &'static str {
        match self {
            Self::TaskCreate
            | Self::TaskEdit
            | Self::TaskDelete
            | Self::TaskRun
            | Self::TaskCancel
            | Self::TaskReport => "Tasks",
            Self::AgentStart | Self::AgentConfigure | Self::AgentStop | Self::AgentInput => {
                "Agents"
            }
            Self::RunInput => "Runs",
            Self::WorkflowCreate
            | Self::WorkflowEdit
            | Self::WorkflowDelete
            | Self::WorkflowRun
            | Self::WorkflowCancel => "Workflows",
            Self::KnowledgeWrite => "Knowledge",
            Self::DatasetEdit => "Datasets",
            Self::BenchRun => "Bench",
            Self::PolicyAttest => "Policy",
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
            exact => Self::ALL
                .into_iter()
                .filter(|g| g.as_str() == exact)
                .collect(),
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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

/// Where a role's definition was written.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RoleOrigin {
    /// Ships with Factory: `worker` and `foreman`.
    #[default]
    Builtin,
    /// The instance root's top-level `roles:`.
    Instance,
    /// A scope's own `scope.roles`, which also holds in every scope below it.
    Scope { scope: String },
}

impl RoleOrigin {
    /// The words an error uses for where a definition was written.
    pub fn describe(&self) -> String {
        match self {
            Self::Builtin => "Factory itself".into(),
            Self::Instance => "the instance root".into(),
            Self::Scope { scope } => format!("scope {scope:?}"),
        }
    }
}

/// One role in effect, and which layer put it there.
#[derive(Debug, Clone)]
pub struct RoleEntry {
    pub def: RoleDef,
    pub origin: RoleOrigin,
    /// The inherited definition this one replaced, when it replaced one.
    pub overrides: Option<RoleOrigin>,
}

/// Every role in effect somewhere: the two built in, plus whatever the layers
/// above that place named.
#[derive(Debug, Clone)]
pub struct Roles(BTreeMap<String, RoleEntry>);

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
        let builtin = |def: RoleDef| RoleEntry {
            def,
            origin: RoleOrigin::Builtin,
            overrides: None,
        };
        Self(BTreeMap::from([
            (Role::WORKER.to_string(), builtin(worker)),
            (Role::FOREMAN.to_string(), builtin(foreman)),
        ]))
    }

    /// The presets plus the instance's own.
    pub fn resolve(written: &BTreeMap<String, RoleSpec>) -> Result<Self> {
        Self::presets().layered(RoleOrigin::Instance, written)
    }

    /// These roles with one more layer written over them. A same-named role
    /// replaces the one it inherits entirely -- description, grants and reach.
    ///
    /// A preset cannot be redefined at any level: a layer that could rewrite
    /// `worker` could widen every agent that never asked for a role, from one
    /// line nobody reads twice.
    pub fn layered(mut self, origin: RoleOrigin, written: &BTreeMap<String, RoleSpec>) -> Result<Self> {
        for (name, spec) in written {
            let replaced = self.0.get(name).map(|entry| entry.origin.clone());
            if replaced == Some(RoleOrigin::Builtin) {
                return Err(FactoryError::BadRequest(format!(
                    "{} redefines {name:?}, a built-in role that cannot be redefined at any level",
                    origin.describe()
                )));
            }
            let mut grants = BTreeSet::new();
            for written in &spec.grants {
                grants.extend(Grant::expand(written).map_err(|e| {
                    FactoryError::BadRequest(format!("role {name:?} in {}: {e}", origin.describe()))
                })?);
            }
            let describe = spec.describe.clone().unwrap_or_else(|| match &origin {
                RoleOrigin::Scope { scope } => format!("a role scope {scope} named"),
                _ => "a role this instance named".into(),
            });
            self.0.insert(
                name.clone(),
                RoleEntry {
                    def: RoleDef {
                        name: Role::new(name.clone()),
                        describe,
                        grants,
                        reach: spec.reach,
                    },
                    origin: origin.clone(),
                    overrides: replaced,
                },
            );
        }
        Ok(self)
    }

    pub fn get(&self, role: &Role) -> Option<&RoleDef> {
        self.0.get(role.as_str()).map(|entry| &entry.def)
    }

    /// The role with where it was written, for anything that has to say so.
    pub fn entry(&self, role: &Role) -> Option<&RoleEntry> {
        self.0.get(role.as_str())
    }

    pub fn contains(&self, role: &Role) -> bool {
        self.0.contains_key(role.as_str())
    }

    pub fn names(&self) -> Vec<String> {
        self.0.keys().cloned().collect()
    }

    pub fn all(&self) -> impl Iterator<Item = &RoleDef> {
        self.0.values().map(|entry| &entry.def)
    }

    pub fn entries(&self) -> impl Iterator<Item = &RoleEntry> {
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
        assert!(
            e.contains("task.create"),
            "the message lists what there is: {e}"
        );
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

    fn spec(describe: &str, grants: &[&str], reach: Reach) -> RoleSpec {
        RoleSpec {
            describe: Some(describe.into()),
            grants: grants.iter().map(|g| g.to_string()).collect(),
            reach,
        }
    }

    #[test]
    fn a_nearer_layer_replaces_the_whole_definition_and_says_what_it_replaced() {
        let instance = BTreeMap::from([(
            "reviewer".to_string(),
            spec("works its own tasks", &["task.edit", "task.report"], Reach::Own),
        )]);
        let scope = BTreeMap::from([(
            "reviewer".to_string(),
            spec("reviews the whole scope", &["task.create"], Reach::Scope),
        )]);
        let roles = Roles::resolve(&instance)
            .unwrap()
            .layered(RoleOrigin::Scope { scope: "projects".into() }, &scope)
            .unwrap();

        let entry = roles.entry(&Role::new("reviewer")).unwrap();
        assert_eq!(entry.def.describe, "reviews the whole scope");
        assert_eq!(entry.def.reach, Reach::Scope);
        // Replaced, never merged: the inherited task.edit is gone.
        assert_eq!(entry.def.grants, BTreeSet::from([Grant::TaskCreate]));
        assert_eq!(entry.origin, RoleOrigin::Scope { scope: "projects".into() });
        assert_eq!(entry.overrides, Some(RoleOrigin::Instance));
        assert_eq!(roles.entry(&Role::worker()).unwrap().origin, RoleOrigin::Builtin);
    }

    #[test]
    fn a_preset_cannot_be_redefined_by_a_scope_either() {
        for name in [Role::WORKER, Role::FOREMAN] {
            let written = BTreeMap::from([(name.to_string(), spec("wider", &["*"], Reach::Scope))]);
            let e = Roles::presets()
                .layered(RoleOrigin::Scope { scope: "projects/demo".into() }, &written)
                .unwrap_err()
                .to_string();
            assert!(e.contains(name), "{e}");
            assert!(e.contains("projects/demo"), "the message says where: {e}");
        }
    }

    #[test]
    fn every_grant_belongs_to_a_group_a_person_reads() {
        for grant in Grant::ALL {
            assert!(
                ["Tasks", "Agents", "Runs", "Workflows", "Knowledge", "Datasets", "Bench", "Policy"]
                    .contains(&grant.group()),
                "{} has no group",
                grant.as_str()
            );
        }
        assert_eq!(Grant::RunInput.group(), "Runs");
        assert_eq!(Grant::AgentInput.group(), "Agents");
        assert_eq!(Grant::KnowledgeWrite.group(), "Knowledge");
        assert_eq!(Grant::DatasetEdit.group(), "Datasets");
        assert_eq!(Grant::BenchRun.group(), "Bench");
        assert_eq!(Grant::PolicyAttest.group(), "Policy");
    }

    #[test]
    fn dataset_edit_bench_run_and_policy_attest_are_appended_at_the_end_and_a_wildcard_still_catches_them() {
        // The task's own instructions: these land at the end of the enum
        // and of `Grant::ALL`, in the order each was added, so a parallel
        // track appending its own grant there too merges without a real
        // conflict.
        assert_eq!(Grant::ALL[Grant::ALL.len() - 3], Grant::DatasetEdit);
        assert_eq!(Grant::ALL[Grant::ALL.len() - 2], Grant::BenchRun);
        assert_eq!(Grant::ALL[Grant::ALL.len() - 1], Grant::PolicyAttest);
        let all = Grant::expand("*").unwrap();
        assert!(all.contains(&Grant::DatasetEdit));
        assert!(all.contains(&Grant::BenchRun));
        assert!(all.contains(&Grant::PolicyAttest));
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
