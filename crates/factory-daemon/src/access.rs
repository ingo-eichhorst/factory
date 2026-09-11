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
//!
//! A role is a name, a set of grants, and a reach (`factory_core::role`). The
//! grants decide *which* requests an agent may make; the reach decides *whose*
//! tasks, agents and sessions it may make them about. What neither can say --
//! that editing your own task is not the same as handing it to somebody else
//! -- is written out below, in the arm it belongs to.

use factory_core::agent::AgentSession;
use factory_core::error::{FactoryError, Result};
use factory_core::protocol::Request;
use factory_core::role::{Grant, Reach, Role, RoleDef};
use factory_core::task::Task;

use crate::engine::Engine;

/// What a request asks of its caller.
enum Needs {
    /// Nothing: any agent may make it.
    Nothing,
    /// This grant, in the caller's reach.
    Grant(Grant),
    /// The owner, and nobody else, whatever role they hold.
    Owner,
}

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
                format!("{name} ({role}) in {scope}")
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
                let role = self.effective_role(&scope, &run.agent).await;
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
                let role = self.effective_role(&agent.scope, &agent.name).await;
                return Ok(Caller::Agent {
                    scope: agent.scope,
                    name: agent.name,
                    role,
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

    /// The role an agent is actually working under: the one somebody gave it
    /// if there is one, and the config's otherwise. Every answer to "what may
    /// this agent do" comes through here, so a standing agent and a run of the
    /// same agent cannot be told two different things.
    pub async fn effective_role(&self, scope: &str, name: &str) -> Role {
        let declared = self.role_of(scope, name);
        match self.store.get_agent(&AgentSession::id_for(scope, name)).await {
            Ok(Some(agent)) => agent.role_with(&declared),
            _ => declared,
        }
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

    /// What a request asks of whoever is making it.
    ///
    /// Reading is open to every agent: one that cannot see the board cannot
    /// coordinate with anyone. Every read is listed by name rather than caught
    /// by a `_` arm, so a request added later will not fall open by default --
    /// it will fail to compile until somebody says which it is.
    fn needs(request: &Request) -> Needs {
        Needs::Grant(match request {
            Request::TaskCreate(_) => Grant::TaskCreate,
            Request::TaskUpdate { .. } => Grant::TaskEdit,
            Request::TaskDelete { .. } => Grant::TaskDelete,
            Request::TaskRun { .. } => Grant::TaskRun,
            Request::TaskCancel { .. } => Grant::TaskCancel,
            Request::TaskReport { .. } => Grant::TaskReport,
            Request::AgentStart { .. } => Grant::AgentStart,
            Request::AgentStop { .. } => Grant::AgentStop,
            Request::AgentInput { .. } => Grant::AgentInput,
            Request::RunInput { .. } => Grant::RunInput,

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
            | Request::Subscribe => return Needs::Nothing,

            // Giving an agent a role is the owner's alone. An agent that could
            // hand itself one is not bounded by the one it has.
            Request::AgentRole { .. } => return Needs::Owner,
        })
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
            FactoryError::Denied(format!("{} may not {what}", caller.describe()))
        };

        let grant = match Self::needs(request) {
            Needs::Nothing => return Ok(()), // a read
            Needs::Owner => return Err(deny("do that; it is the owner's to do")),
            Needs::Grant(grant) => grant,
        };

        // A role the config no longer defines is refused rather than defaulted:
        // the agent is holding a job description nobody can read.
        let Some(def) = self.roles.get(role) else {
            return Err(deny(&format!(
                "act: this instance no longer defines the role {role:?}"
            )));
        };
        if !def.allows(grant) {
            return Err(deny(grant.describe()));
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

        // Whose work a grant carries to: everything in the scope, or only what
        // this agent was given.
        let task_in_reach = |def: &RoleDef, task: &Task| -> Result<()> {
            match def.reach {
                Reach::Scope => in_scope(&task.scope),
                Reach::Own if Self::assigned_to(task, scope, name) => Ok(()),
                Reach::Own => Err(deny("touch a task that is not assigned to it")),
            }
        };

        match request {
            Request::TaskCreate(new) => in_scope(new.scope.as_deref().unwrap_or(scope)),

            Request::TaskUpdate { id, patch } => {
                // Handing a task to somebody else is not editing it. A role
                // that reaches only its own work may change what its task says,
                // never whose it is.
                if def.reach == Reach::Own && (patch.scope.is_some() || patch.agent.is_some()) {
                    return Err(deny("hand its task to somebody else"));
                }
                if let Some(task) = self.store.get(id).await? {
                    task_in_reach(def, &task)?;
                }
                // Moving a task out of the scope would be handing it to
                // somebody the caller does not answer for.
                if let Some(s) = &patch.scope {
                    in_scope(s)?;
                }
                Ok(())
            }

            Request::TaskDelete { id } | Request::TaskRun { id } | Request::TaskCancel { id } => {
                match self.store.get(id).await? {
                    Some(task) => task_in_reach(def, &task),
                    None => Ok(()), // let the engine report "no such task"
                }
            }

            Request::TaskReport { id, .. } => {
                let Some(task) = self.store.get(id).await? else {
                    return Ok(());
                };
                if task_in_reach(def, &task).is_ok() {
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

            Request::AgentStart { scope: s, name: n } => match def.reach {
                Reach::Scope => in_scope(s),
                Reach::Own if s == scope && n == name => Ok(()),
                Reach::Own => Err(deny("start an agent other than itself")),
            },

            Request::AgentStop { id } | Request::AgentInput { id, .. } => match def.reach {
                Reach::Scope => match self.store.get_agent(id).await? {
                    Some(agent) => in_scope(&agent.scope),
                    None => Ok(()),
                },
                Reach::Own if id == &AgentSession::id_for(scope, name) => Ok(()),
                Reach::Own => Err(deny("reach into another agent's session")),
            },

            Request::RunInput { id, .. } => match def.reach {
                Reach::Scope => match self.task_of_run(id).await? {
                    Some(task) => in_scope(&task.scope),
                    None => Ok(()),
                },
                Reach::Own => match run_id {
                    Some(own) if own == id => Ok(()),
                    _ => Err(deny("type into another agent's session")),
                },
            },

            // Reads returned above, and anything needing a grant nobody holds
            // was refused above. Nothing should arrive here.
            _ => Err(deny("do that")),
        }
    }
}
#[cfg(test)]
mod tests {
    //! What each role may do, pinned request by request.
    //!
    //! These are the rules themselves, not an implementation of them: a change
    //! here is a change to what an agent is allowed to do by accident, and
    //! should be as deliberate as one to the prose in the README.

    use super::*;
    use crate::engine::Engine;
    use chrono::Utc;
    use factory_core::agent::Lifetime;
    use factory_core::config::{Config, DaemonConfig, Factory, Instance};
    use factory_core::run::{NewRun, RunStatus, Trigger};
    use factory_core::task::{NewTask, Task, TaskPatch, TaskReport, TaskStatus};
    use factory_plugins::registry::Registry;
    use factory_plugins::SqliteStore;
    use std::path::PathBuf;
    use std::sync::Arc;

    /// Two scopes, so "its authority stops at the scope boundary" can be tested
    /// rather than asserted.
    fn engine() -> Arc<Engine> {
        let config = Config {
            version: 1,
            instance: Instance {
                id: "i".into(),
                name: "test".into(),
            },
            daemon: DaemonConfig::default(),
            scopes: vec![
                serde_yaml_ng::from_str("name: demo\npath: .\n").unwrap(),
                serde_yaml_ng::from_str("name: other\npath: .\n").unwrap(),
            ],
            roles: Default::default(),
            plugins_dir: None,
        };
        let factory = Factory {
            root: PathBuf::from("/tmp/factory-access-test"),
            config,
        };
        Arc::new(Engine::new(
            factory,
            Registry::with_builtins(),
            Arc::new(SqliteStore::in_memory().unwrap()),
            PathBuf::from("factory"),
            vec![],
        ))
    }

    /// The same, for an instance that names roles of its own.
    fn engine_with_roles(yaml: &str) -> Arc<Engine> {
        let config: Config = serde_yaml_ng::from_str(&format!(
            "instance:\n  id: i\n  name: test\nscopes:\n  - name: demo\n    path: .\n  - name: other\n    path: .\n{yaml}"
        ))
        .unwrap();
        config.validate().unwrap();
        let factory = Factory {
            root: PathBuf::from("/tmp/factory-access-test"),
            config,
        };
        Arc::new(Engine::new(
            factory,
            Registry::with_builtins(),
            Arc::new(SqliteStore::in_memory().unwrap()),
            PathBuf::from("factory"),
            vec![],
        ))
    }

    fn wearing(role: &str) -> Caller {
        Caller::Agent {
            scope: "demo".into(),
            name: "w".into(),
            role: Role::new(role),
            run_id: None,
        }
    }

    fn worker(name: &str) -> Caller {
        Caller::Agent {
            scope: "demo".into(),
            name: name.into(),
            role: Role::worker(),
            run_id: None,
        }
    }

    fn foreman() -> Caller {
        Caller::Agent {
            scope: "demo".into(),
            name: "boss".into(),
            role: Role::foreman(),
            run_id: None,
        }
    }

    async fn task_in(engine: &Engine, id: &str, scope: &str, agent: &str) -> Task {
        let now = Utc::now();
        let task = Task {
            id: id.into(),
            title: id.into(),
            instructions: String::new(),
            scope: scope.into(),
            agent: agent.into(),
            runtime: "herdr".into(),
            status: TaskStatus::Pending,
            schedule: None,
            result: None,
            error: None,
            runs: 0,
            ack_timeout_seconds: None,
            timeout_seconds: None,
            labels: Default::default(),
            created_at: now,
            updated_at: now,
            last_run_at: None,
            next_run_at: None,
        };
        engine.store.create(&task).await.unwrap()
    }

    async fn run_of(engine: &Engine, task_id: &str, agent: &str) -> String {
        engine
            .store
            .create_run(&NewRun {
                task_id: task_id.into(),
                trigger: Trigger::Manual,
                agent: agent.into(),
                adapter: "shell".into(),
                runtime: "herdr".into(),
                token: format!("token-{task_id}"),
            })
            .await
            .unwrap()
            .id
    }

    fn report() -> TaskReport {
        TaskReport {
            status: Some(RunStatus::Running),
            message: Some("working".into()),
            result: None,
            error: None,
            token: None,
        }
    }

    fn titled(title: &str) -> TaskPatch {
        TaskPatch {
            title: Some(title.into()),
            ..Default::default()
        }
    }

    async fn allowed(engine: &Engine, caller: &Caller, request: Request) -> bool {
        engine.authorize(caller, &request).await.is_ok()
    }

    // -- the owner ---------------------------------------------------------

    #[tokio::test]
    async fn the_owner_may_do_anything() {
        let e = engine();
        for request in [
            Request::TaskCreate(NewTask::default()),
            Request::TaskDelete { id: "t".into() },
            Request::AgentStart {
                scope: "other".into(),
                name: "x".into(),
            },
        ] {
            assert!(allowed(&e, &Caller::Owner, request).await);
        }
    }

    // -- reading -----------------------------------------------------------

    #[tokio::test]
    async fn every_agent_may_read_the_board() {
        let e = engine();
        for caller in [worker("w"), foreman()] {
            for request in [
                Request::Status,
                Request::Adapters,
                Request::Agents,
                Request::Occupancy { minutes: None },
                Request::TaskGet { id: "t".into() },
                Request::TaskList(Default::default()),
                Request::TaskEntries {
                    id: "t".into(),
                    limit: None,
                },
                Request::TaskOutput {
                    id: "t".into(),
                    lines: None,
                },
                Request::RunList {
                    task_id: "t".into(),
                    limit: None,
                },
                Request::RunGet { id: "r".into() },
                Request::RunEntries {
                    id: "r".into(),
                    limit: None,
                },
                Request::RunOutput {
                    id: "r".into(),
                    lines: None,
                },
                Request::AgentOutput {
                    id: "other/x".into(),
                    lines: None,
                },
                Request::AgentScreen {
                    id: "other/x".into(),
                },
                Request::RunScreen { id: "r".into() },
                Request::Subscribe,
            ] {
                assert!(
                    allowed(&e, &caller, request.clone()).await,
                    "{} should be able to read {request:?}",
                    caller.describe()
                );
            }
        }
    }

    // -- the worker --------------------------------------------------------

    #[tokio::test]
    async fn a_worker_may_change_and_report_its_own_task_and_nothing_else() {
        let e = engine();
        task_in(&e, "mine", "demo", "w").await;
        task_in(&e, "theirs", "demo", "someone-else").await;

        assert!(
            allowed(
                &e,
                &worker("w"),
                Request::TaskUpdate {
                    id: "mine".into(),
                    patch: titled("a better title"),
                }
            )
            .await
        );
        assert!(
            allowed(
                &e,
                &worker("w"),
                Request::TaskReport {
                    id: "mine".into(),
                    report: report(),
                }
            )
            .await
        );
        assert!(
            !allowed(
                &e,
                &worker("w"),
                Request::TaskUpdate {
                    id: "theirs".into(),
                    patch: titled("mine now"),
                }
            )
            .await
        );
        assert!(
            !allowed(
                &e,
                &worker("w"),
                Request::TaskReport {
                    id: "theirs".into(),
                    report: report(),
                }
            )
            .await
        );
    }

    #[tokio::test]
    async fn a_worker_may_not_hand_its_task_to_anybody_else() {
        let e = engine();
        task_in(&e, "mine", "demo", "w").await;
        for patch in [
            TaskPatch {
                agent: Some("someone-else".into()),
                ..Default::default()
            },
            TaskPatch {
                scope: Some("other".into()),
                ..Default::default()
            },
        ] {
            assert!(
                !allowed(
                    &e,
                    &worker("w"),
                    Request::TaskUpdate {
                        id: "mine".into(),
                        patch,
                    }
                )
                .await
            );
        }
    }

    #[tokio::test]
    async fn a_worker_closes_the_run_it_holds_even_if_its_name_changed() {
        let e = engine();
        task_in(&e, "t", "demo", "the-old-name").await;
        let run = run_of(&e, "t", "the-old-name").await;
        let renamed = Caller::Agent {
            scope: "demo".into(),
            name: "the-new-name".into(),
            role: Role::worker(),
            run_id: Some(run),
        };
        assert!(
            allowed(
                &e,
                &renamed,
                Request::TaskReport {
                    id: "t".into(),
                    report: report(),
                }
            )
            .await
        );
    }

    #[tokio::test]
    async fn a_worker_types_into_its_own_session_only() {
        let e = engine();
        task_in(&e, "t", "demo", "w").await;
        let mine = run_of(&e, "t", "w").await;
        let caller = Caller::Agent {
            scope: "demo".into(),
            name: "w".into(),
            role: Role::worker(),
            run_id: Some(mine.clone()),
        };
        assert!(
            allowed(
                &e,
                &caller,
                Request::RunInput {
                    id: mine,
                    text: Some("hello".into()),
                    keys: vec![],
                }
            )
            .await
        );
        assert!(
            !allowed(
                &e,
                &caller,
                Request::RunInput {
                    id: "somebody-elses-run".into(),
                    text: Some("hello".into()),
                    keys: vec![],
                }
            )
            .await
        );
    }

    #[tokio::test]
    async fn a_worker_does_not_run_the_board() {
        let e = engine();
        task_in(&e, "t", "demo", "w").await;
        for request in [
            Request::TaskCreate(NewTask::default()),
            Request::TaskDelete { id: "t".into() },
            Request::TaskRun { id: "t".into() },
            Request::TaskCancel { id: "t".into() },
            Request::AgentStart {
                scope: "demo".into(),
                name: "w".into(),
            },
            Request::AgentStop {
                id: "demo/w".into(),
            },
            Request::AgentInput {
                id: "demo/w".into(),
                text: Some("hi".into()),
                keys: vec![],
            },
        ] {
            assert!(
                !allowed(&e, &worker("w"), request.clone()).await,
                "a worker should not be able to {request:?}"
            );
        }
    }

    // -- the foreman -------------------------------------------------------

    #[tokio::test]
    async fn a_foreman_runs_its_own_scope() {
        let e = engine();
        task_in(&e, "here", "demo", "w").await;
        let run = run_of(&e, "here", "w").await;
        for request in [
            Request::TaskCreate(NewTask {
                title: "a task".into(),
                scope: Some("demo".into()),
                ..Default::default()
            }),
            Request::TaskCreate(NewTask {
                title: "a task".into(),
                scope: None,
                ..Default::default()
            }),
            Request::TaskUpdate {
                id: "here".into(),
                patch: titled("renamed"),
            },
            Request::TaskDelete { id: "here".into() },
            Request::TaskRun { id: "here".into() },
            Request::TaskCancel { id: "here".into() },
            Request::TaskReport {
                id: "here".into(),
                report: report(),
            },
            Request::AgentStart {
                scope: "demo".into(),
                name: "w".into(),
            },
            Request::RunInput {
                id: run,
                text: Some("hi".into()),
                keys: vec![],
            },
        ] {
            assert!(
                allowed(&e, &foreman(), request.clone()).await,
                "a foreman should be able to {request:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_foremans_authority_stops_at_the_scope_boundary() {
        let e = engine();
        task_in(&e, "elsewhere", "other", "w").await;
        let run = run_of(&e, "elsewhere", "w").await;
        for request in [
            Request::TaskCreate(NewTask {
                title: "a task".into(),
                scope: Some("other".into()),
                ..Default::default()
            }),
            Request::TaskUpdate {
                id: "elsewhere".into(),
                patch: titled("renamed"),
            },
            Request::TaskDelete {
                id: "elsewhere".into(),
            },
            Request::TaskRun {
                id: "elsewhere".into(),
            },
            Request::TaskCancel {
                id: "elsewhere".into(),
            },
            Request::TaskReport {
                id: "elsewhere".into(),
                report: report(),
            },
            Request::AgentStart {
                scope: "other".into(),
                name: "w".into(),
            },
            Request::RunInput {
                id: run,
                text: Some("hi".into()),
                keys: vec![],
            },
        ] {
            assert!(
                !allowed(&e, &foreman(), request.clone()).await,
                "a foreman in demo should not be able to {request:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_foreman_may_not_move_a_task_out_of_its_scope() {
        let e = engine();
        task_in(&e, "here", "demo", "w").await;
        assert!(
            !allowed(
                &e,
                &foreman(),
                Request::TaskUpdate {
                    id: "here".into(),
                    patch: TaskPatch {
                        scope: Some("other".into()),
                        ..Default::default()
                    },
                }
            )
            .await
        );
    }

    // -- roles an instance names for itself --------------------------------

    #[tokio::test]
    async fn a_named_role_may_do_what_its_grants_say_and_no_more() {
        let e = engine_with_roles(
            "roles:\n  runner:\n    grants: [task.run, task.cancel]\n    reach: scope\n",
        );
        task_in(&e, "here", "demo", "somebody").await;
        assert!(allowed(&e, &wearing("runner"), Request::TaskRun { id: "here".into() }).await);
        assert!(allowed(&e, &wearing("runner"), Request::TaskCancel { id: "here".into() }).await);
        // Not granted: it may look at the board, and start what is on it.
        assert!(allowed(&e, &wearing("runner"), Request::TaskList(Default::default())).await);
        assert!(!allowed(&e, &wearing("runner"), Request::TaskCreate(NewTask::default())).await);
        assert!(
            !allowed(
                &e,
                &wearing("runner"),
                Request::TaskUpdate {
                    id: "here".into(),
                    patch: titled("renamed"),
                }
            )
            .await
        );
    }

    #[tokio::test]
    async fn reach_decides_whose_work_a_grant_carries_to() {
        let e = engine_with_roles(
            "roles:\n  tidier:\n    grants: [task.delete]\n    reach: own\n  sweeper:\n    grants: [task.delete]\n    reach: scope\n",
        );
        task_in(&e, "mine", "demo", "w").await;
        task_in(&e, "theirs", "demo", "somebody").await;

        assert!(allowed(&e, &wearing("tidier"), Request::TaskDelete { id: "mine".into() }).await);
        assert!(!allowed(&e, &wearing("tidier"), Request::TaskDelete { id: "theirs".into() }).await);
        assert!(allowed(&e, &wearing("sweeper"), Request::TaskDelete { id: "theirs".into() }).await);
    }

    #[tokio::test]
    async fn a_role_that_reaches_its_own_work_may_not_hand_it_away() {
        let e = engine_with_roles(
            "roles:\n  editor:\n    grants: [task.edit]\n    reach: own\n",
        );
        task_in(&e, "mine", "demo", "w").await;
        assert!(
            allowed(
                &e,
                &wearing("editor"),
                Request::TaskUpdate {
                    id: "mine".into(),
                    patch: titled("renamed"),
                }
            )
            .await
        );
        assert!(
            !allowed(
                &e,
                &wearing("editor"),
                Request::TaskUpdate {
                    id: "mine".into(),
                    patch: TaskPatch {
                        agent: Some("somebody".into()),
                        ..Default::default()
                    },
                }
            )
            .await
        );
    }

    #[tokio::test]
    async fn a_role_this_instance_does_not_define_may_do_nothing() {
        let e = engine();
        assert!(
            !allowed(&e, &wearing("ghost"), Request::TaskCreate(NewTask::default())).await,
            "a role nothing defines is not a role"
        );
        // Reading is still open: that is checked before the role is looked up
        // at all, and an agent that cannot see the board cannot even say so.
        assert!(allowed(&e, &wearing("ghost"), Request::TaskList(Default::default())).await);
    }

    // -- giving an agent a role --------------------------------------------

    #[tokio::test]
    async fn only_the_owner_gives_an_agent_a_role() {
        let e = engine_with_roles("roles:\n  runner:\n    grants: [task.run]\n");
        let request = Request::AgentRole {
            id: "demo/w".into(),
            role: Some("runner".into()),
        };
        assert!(allowed(&e, &Caller::Owner, request.clone()).await);
        // Not even a foreman, and not a role that was given every grant there
        // is: an agent that can hand itself one is not bounded by the one it
        // has.
        assert!(!allowed(&e, &foreman(), request.clone()).await);
        assert!(!allowed(&e, &worker("w"), request.clone()).await);
        assert!(!allowed(&e, &wearing("runner"), request).await);
    }

    #[tokio::test]
    async fn a_given_role_wins_over_the_config_until_it_is_cleared() {
        let e = engine_with_roles(
            "roles:\n  runner:\n    grants: [task.run]\n",
        );
        let agent = AgentSession::new("demo", "watcher", "pi", "herdr", Lifetime::Permanent, Role::worker());
        e.store.put_agent(&agent).await.unwrap();
        assert_eq!(e.effective_role("demo", "watcher").await, Role::worker());

        let given = e
            .set_agent_role("demo/watcher", Some(Role::new("runner")))
            .await
            .unwrap();
        assert_eq!(given.role, Role::new("runner"));
        assert_eq!(e.effective_role("demo", "watcher").await, Role::new("runner"));

        e.set_agent_role("demo/watcher", None).await.unwrap();
        assert_eq!(e.effective_role("demo", "watcher").await, Role::worker());
    }

    #[tokio::test]
    async fn a_role_nothing_defines_cannot_be_given() {
        let e = engine();
        let agent = AgentSession::new("demo", "watcher", "pi", "herdr", Lifetime::Permanent, Role::worker());
        e.store.put_agent(&agent).await.unwrap();
        let err = e
            .set_agent_role("demo/watcher", Some(Role::new("ghost")))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("ghost"), "{err}");
        assert!(err.contains("foreman"), "it says what there is: {err}");
    }
}
