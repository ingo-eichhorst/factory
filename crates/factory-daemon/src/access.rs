//! Who is asking, and what that lets them do.
//!
//! An agent says which agent it is by presenting the token Factory gave it: a
//! run token for a task agent, an identity token for a standing one. No token
//! means the owner -- the person at the socket.
//!
//! This is not a wall. Every agent runs as the owner of the instance and can
//! reach the same socket, so an agent that simply leaves the token out is
//! indistinguishable from a person. What it buys is that an agent following
//! its instructions stays inside its job: a worker cannot quietly reassign its
//! own work, a foreman cannot reach into another scope. Guard-rails, not
//! security.

use factory_core::agent::Role;
use factory_core::error::{FactoryError, Result};
use factory_core::protocol::Request;
use factory_core::task::Task;

use crate::engine::Engine;

#[derive(Debug, Clone)]
pub enum Caller {
    /// No token: the person who owns the instance.
    Owner,
    Agent {
        scope: String,
        name: String,
        role: Role,
        /// Set when the token was a run's rather than a standing agent's.
        run_id: Option<String>,
    },
}

impl Caller {
    pub fn describe(&self) -> String {
        match self {
            Caller::Owner => "the owner".into(),
            Caller::Agent { scope, name, role, .. } => {
                format!("{name} ({}) in {scope}", role.as_str())
            }
        }
    }

    pub fn scope(&self) -> Option<&str> {
        match self {
            Caller::Owner => None,
            Caller::Agent { scope, .. } => Some(scope),
        }
    }
}

impl Engine {
    /// Turn a token into who is holding it. An unknown token is refused rather
    /// than treated as anonymous: a stale one must not quietly become owner.
    pub async fn caller_for(&self, token: Option<&str>) -> Result<Caller> {
        let Some(token) = token.filter(|t| !t.is_empty()) else {
            return Ok(Caller::Owner);
        };

        for run in self.store.active_runs().await? {
            if run.token.as_deref() == Some(token) {
                let scope = self
                    .store
                    .get(&run.task_id)
                    .await?
                    .map(|t| t.scope)
                    .unwrap_or_default();
                let role = self.role_of(&scope, &run.agent);
                return Ok(Caller::Agent {
                    scope,
                    name: run.agent,
                    role,
                    run_id: Some(run.id),
                });
            }
        }

        for agent in self.store.agents().await? {
            if agent.token.as_deref() == Some(token) {
                return Ok(Caller::Agent {
                    scope: agent.scope,
                    name: agent.name,
                    role: agent.role,
                    run_id: None,
                });
            }
        }

        Err(FactoryError::Denied(
            "that token belongs to no agent Factory knows about".into(),
        ))
    }

    /// The role the config gives an agent. An agent nobody named -- a one-off
    /// `--agent claude-code` -- is a worker.
    pub fn role_of(&self, scope: &str, name: &str) -> Role {
        self.factory
            .scope(scope)
            .ok()
            .map(|s| s.agents_with(&self.factory.config.daemon.foreman))
            .unwrap_or_default()
            .into_iter()
            .find(|a| a.name() == name)
            .map(|a| a.role)
            .unwrap_or_default()
    }

    /// A task belongs to a worker when it is in the worker's scope and names it.
    fn assigned_to(task: &Task, scope: &str, name: &str) -> bool {
        task.scope == scope && task.agent == name
    }

    async fn task_of_run(&self, run_id: &str) -> Result<Option<Task>> {
        let Some(run) = self.store.get_run(run_id).await? else {
            return Ok(None);
        };
        self.store.get(&run.task_id).await
    }

    /// Say no before anything happens, with a reason a person can act on.
    pub async fn authorize(&self, caller: &Caller, request: &Request) -> Result<()> {
        let Caller::Agent {
            scope,
            name,
            role,
            run_id,
        } = caller
        else {
            return Ok(()); // the owner
        };

        let deny = |what: &str| -> FactoryError {
            FactoryError::Denied(format!(
                "{} may not {what}",
                caller.describe()
            ))
        };

        // Reading is open to every agent: an agent that cannot see the board
        // cannot coordinate with anyone.
        match request {
            Request::Status
            | Request::Adapters
            | Request::Agents
            | Request::Occupancy { .. }
            | Request::TaskGet { .. }
            | Request::TaskList(_)
            | Request::TaskEntries { .. }
            | Request::TaskOutput { .. }
            | Request::RunList { .. }
            | Request::RunGet { .. }
            | Request::RunEntries { .. }
            | Request::RunOutput { .. }
            | Request::AgentOutput { .. }
            | Request::AgentScreen { .. }
            | Request::RunScreen { .. }
            | Request::Subscribe => return Ok(()),
            _ => {}
        }

        let in_scope = |s: &str| -> Result<()> {
            if s == scope {
                Ok(())
            } else {
                Err(FactoryError::Denied(format!(
                    "{} works in {scope}, not in {s}",
                    caller.describe()
                )))
            }
        };

        match role {
            Role::Foreman => match request {
                Request::TaskCreate(new) => in_scope(new.scope.as_deref().unwrap_or(scope)),
                Request::TaskUpdate { id, patch } => {
                    let task = self.store.get(id).await?;
                    if let Some(task) = task {
                        in_scope(&task.scope)?;
                    }
                    // Moving a task out of the scope would be handing it to
                    // somebody the foreman does not answer for.
                    if let Some(s) = &patch.scope {
                        in_scope(s)?;
                    }
                    Ok(())
                }
                Request::TaskDelete { id }
                | Request::TaskRun { id }
                | Request::TaskCancel { id }
                | Request::TaskReport { id, .. } => {
                    match self.store.get(id).await? {
                        Some(task) => in_scope(&task.scope),
                        None => Ok(()), // let the engine report "no such task"
                    }
                }
                Request::AgentStart { scope: s, .. } => in_scope(s),
                Request::AgentStop { id } | Request::AgentInput { id, .. } => {
                    match self.store.get_agent(id).await? {
                        Some(agent) => in_scope(&agent.scope),
                        None => Ok(()),
                    }
                }
                Request::RunInput { id, .. } => match self.task_of_run(id).await? {
                    Some(task) => in_scope(&task.scope),
                    None => Ok(()),
                },
                _ => Ok(()),
            },

            Role::Worker => match request {
                // Its own work, and only that.
                Request::TaskUpdate { id, patch } => {
                    let Some(task) = self.store.get(id).await? else {
                        return Ok(());
                    };
                    if !Self::assigned_to(&task, scope, name) {
                        return Err(deny("change a task that is not assigned to it"));
                    }
                    if patch.scope.is_some() || patch.agent.is_some() {
                        return Err(deny("hand its task to somebody else"));
                    }
                    Ok(())
                }
                Request::TaskReport { id, .. } => {
                    let Some(task) = self.store.get(id).await? else {
                        return Ok(());
                    };
                    if Self::assigned_to(&task, scope, name) {
                        return Ok(());
                    }
                    // The token already proved it owns this run; a task whose
                    // agent was renamed under it should still be closable.
                    match run_id {
                        Some(run) if self.task_of_run(run).await?.map(|t| t.id) == Some(task.id) => {
                            Ok(())
                        }
                        _ => Err(deny("report on a task that is not assigned to it")),
                    }
                }
                Request::RunInput { id, .. } => match run_id {
                    Some(own) if own == id => Ok(()),
                    _ => Err(deny("type into another agent's session")),
                },
                Request::TaskCreate(_) => Err(deny("create tasks")),
                Request::TaskDelete { .. } => Err(deny("delete tasks")),
                Request::TaskRun { .. } | Request::TaskCancel { .. } => {
                    Err(deny("start or stop runs"))
                }
                Request::AgentStart { .. } | Request::AgentStop { .. } => {
                    Err(deny("start or stop agents"))
                }
                Request::AgentInput { .. } => Err(deny("type into another agent's session")),
                _ => Err(deny("do that")),
            },
        }
    }
}
