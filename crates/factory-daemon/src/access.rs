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
#[cfg(test)]
use factory_core::role::GrantExpansion;
use factory_core::task::{NewTask, Task};
use factory_core::workflow::WorkflowActor;

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

    /// The durable form a workflow run remembers. Only who, never what they
    /// were allowed to do at the time -- see `WorkflowActor` and
    /// `Engine::caller_for_actor`, which re-derives the latter every time it
    /// matters instead of trusting a stale copy of it.
    pub fn as_workflow_actor(&self) -> WorkflowActor {
        match self {
            Caller::Owner => WorkflowActor::Owner,
            Caller::Agent { scope, name, .. } => WorkflowActor::Agent {
                scope: scope.clone(),
                name: name.clone(),
            },
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

        let active = self.store.active_runs().await?;
        for run in &active {
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
                    name: run.agent.clone(),
                    role,
                    run_id: Some(run.id.clone()),
                });
            }
        }

        // `#178`: a token from a run `--continue` replaced is not just
        // unknown -- it is *this task's own*, one generation stale. Told
        // apart from a genuinely unknown token so the caller hears why,
        // rather than the generic denial below; never a grant (only a
        // digest is ever compared, never the token itself), since the
        // active run above already had its chance to match.
        let given_digest = factory_core::run::token_digest(token);
        for run in &active {
            if run.superseded_token_sha256s.iter().any(|d| d == &given_digest) {
                return Err(FactoryError::Denied(format!(
                    "a newer run of task {} exists; use the latest reporting commands",
                    run.task_id
                )));
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
        let factory = self.factory_snapshot();
        factory
            .scope(scope)
            .ok()
            .map(|s| s.agents_with(&factory.config.daemon.foreman))
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
            // Intake (`#119`) used to reuse the task grants: handing
            // something in was creating a task, triaging one was editing it.
            // `#172` gives it its own vocabulary instead, one grant per
            // request and no wildcard arm here either -- a new intake
            // request still fails to compile until it is mapped.
            Request::IntakeAdd(_) => Grant::IntakeAdd,
            Request::IntakeInfo { .. } => Grant::IntakeInfo,
            Request::IntakeTriage { .. } => Grant::IntakeTriage,
            Request::IntakeAssess { .. } => Grant::IntakeAssess,
            Request::IntakeDecide { .. } => Grant::IntakeDecide,
            // The one outward-effect grant: never `foreman`'s free ALL,
            // never a wildcard, named exactly or not at all (`#171`).
            Request::IntakePublish { .. } => Grant::IntakePublish,
            // Flagging only adds scrutiny -- the same door `intake.assess`
            // already opens, not a sixth grant (`#170`). Confirming or
            // dismissing is never an agent's, whatever it holds: see the
            // `Needs::Owner` arm below.
            Request::IntakeFlagSecurity { .. } => Grant::IntakeAssess,
            Request::TaskDelete { .. } => Grant::TaskDelete,
            Request::TaskRun { .. } => Grant::TaskRun,
            Request::TaskCancel { .. } => Grant::TaskCancel,
            // Closing a task and taking that back are one decision, made
            // about the task rather than a run (`#122`).
            Request::TaskClose { .. } | Request::TaskReopen { .. } => Grant::TaskClose,
            // Moving a schedule's next slot is editing the task, exactly as
            // pausing it through `task.update` is (`#106`).
            Request::TaskSkipNext { .. } => Grant::TaskEdit,
            // The harness saying a turn ended is a report on the run in
            // everything but who is speaking: same token, same authority.
            Request::TaskReport { .. } | Request::TaskTurnEnded { .. } => Grant::TaskReport,
            Request::TaskAttach { .. } => Grant::TaskAttach,
            Request::AgentStart { .. } => Grant::AgentStart,
            Request::AgentConfigure { .. } | Request::AgentDelete { .. } => Grant::AgentConfigure,
            // Saving or resetting a scope's own dashboard layout (`#160`).
            // An ordinary grant, unlike the role-layer writes it otherwise
            // resembles: a layout cannot widen what an agent may do.
            Request::DashboardSet { .. } | Request::DashboardReset { .. } => Grant::DashboardEdit,
            Request::AgentStop { .. } => Grant::AgentStop,
            Request::AgentInput { .. } => Grant::AgentInput,
            Request::RunInput { .. } => Grant::RunInput,
            // Typing into a run's session, however narrowed (`#106`).
            Request::RunAnswer { .. } => Grant::RunInput,
            Request::RunApprove { .. } | Request::RunReject { .. } | Request::RunRework { .. } => Grant::RunApprove,
            Request::WorkflowCreate(_) => Grant::WorkflowCreate,
            Request::WorkflowUpdate { .. } => Grant::WorkflowEdit,
            Request::WorkflowDelete { .. } => Grant::WorkflowDelete,
            Request::WorkflowStart { .. } => Grant::WorkflowRun,
            Request::WorkflowRunCancel { .. } => Grant::WorkflowCancel,
            Request::KnowledgeImport { .. }
            | Request::KnowledgeAdd { .. }
            | Request::KnowledgeWriteFile { .. } => Grant::KnowledgeWrite,
            // The subject of both is the root scope: datasets and bench
            // runs are company-wide, not one project's -- see `in_root_scope`
            // in `authorize` below.
            Request::DatasetCreate { .. }
            | Request::DatasetAddCases { .. }
            | Request::DatasetImport { .. }
            | Request::DatasetFromTasks { .. }
            | Request::DatasetDeleteCase { .. }
            | Request::DatasetDelete { .. } => Grant::DatasetEdit,
            Request::BenchRunStart { .. } | Request::BenchRunCancel { .. } => Grant::BenchRun,
            // Recording and withdrawing are the same grant, checked against
            // the root scope like `knowledge.write` -- see `in_root_scope`.
            Request::PolicyAttest { .. } | Request::PolicyWithdraw { .. } => Grant::PolicyAttest,
            // The same grant `TaskCreate` itself needs -- this is not a way
            // around it, it is the same door (`#83`).
            Request::PolicyRemediate { .. } => Grant::TaskCreate,
            // Checked against the root scope for the same reason
            // `policy.attest` is -- see `in_root_scope`.
            Request::GoalsCheckIn { .. } => Grant::GoalsCheckIn,
            // Verifying with a supplied identity decrypts an encrypted
            // snapshot (`#152`) -- the owner's alone, like restore, and
            // checked before the shared arm below: a role grant must never
            // become "read this key file".
            Request::BackupVerify { identity: Some(_), .. } => return Needs::Owner,
            Request::EnvironmentPromote(_) | Request::EnvironmentRecover(_) => return Needs::Owner,
            // Checked against the root scope too: a backup is of the whole
            // instance's state (`#116`).
            Request::BackupRun | Request::BackupVerify { .. } => Grant::BackupRun,
            // Checked against the environment's own scope (`#185`).
            Request::DeployStart(_) | Request::DeployFinish(_) | Request::ReleaseAdd(_) | Request::EnvironmentCheck { .. } => Grant::DeployRecord,
            // The same door `TaskCreate`/`PolicyRemediate` already open --
            // this is not a second one (`#100`).
            Request::ScenarioPromote { .. } => Grant::TaskCreate,
            // And the same door again (`#107`): a remediation task is an
            // ordinary task in every way but how it was asked for.
            Request::QualityRemediate { .. } => Grant::TaskCreate,

            Request::Status
            | Request::Adapters
            | Request::RuntimeConnections
            | Request::Agents
            | Request::Occupancy { .. }
            | Request::Production { .. }
            | Request::SiteFootprint
            | Request::Environment
            | Request::Dependencies { .. }
            | Request::DependenciesVex { .. }
            | Request::Doctor
            | Request::Infrastructure
            // Lists the destination and reads the history; writes nothing.
            | Request::Backup
            // Samples and deployments folded on read; writes nothing.
            | Request::Environments { .. }
            | Request::EnvironmentSamples(_)
            | Request::ReleaseDetail(_)
            | Request::DependencyDocument { .. }
            | Request::Knowledge
            | Request::KnowledgeSearch { .. }
            | Request::Benchmarks
            | Request::Datasets
            | Request::Dataset { .. }
            | Request::BenchRuns { .. }
            | Request::BenchRunGet { .. }
            | Request::Policy { .. }
            // The CRA Art. 14 reporting clock (`#157`, phase 1): a
            // projection over the same L2/L4 reads `Policy` folds, and
            // itself computed fresh on every call. Open the same way.
            | Request::PolicyClock { .. }
            | Request::PolicyControl { .. }
            | Request::PolicyExport { .. }
            | Request::Metrics { .. }
            // The dashboard's resolved layout (`#159`): a projection over
            // config already readable through the roster/config views, the
            // same reasoning `Metrics` gets read access with no grant.
            | Request::Dashboard { .. }
            | Request::Goals { .. }
            // Read like `Goals`: authored profiles and evidence folded
            // fresh, nothing written (`#107`).
            | Request::Quality { .. }
            // Recomputed server-side over authored files and computed
            // metrics alone -- nothing written, whatever `scenario` or
            // `drivers` say (`#100`).
            | Request::ScenarioWhatIf { .. }
            | Request::Scenarios { .. }
            // A projection over what the reads below already return (`#106`).
            | Request::Operations { .. }
            // A projection over the task list, like `Operations` (`#119`).
            | Request::IntakeBoard { .. }
            // The confirmed-report read (`#170`): a projection over the task
            // list too, and the L4 fact `#157`'s clock will read -- open the
            // same way `IntakeBoard` is.
            | Request::IntakeSecurityReports { .. }
            | Request::TaskGet { .. }
            | Request::TaskList(_)
            | Request::TaskEntries { .. }
            | Request::TaskOutput { .. }
            | Request::RunList { .. }
            | Request::RunGet { .. }
            | Request::RunEntries { .. }
            | Request::RunOutput { .. }
            // Usage and cost (#117): read off runs and their snapshots,
            // nothing written.
            | Request::RunUsage { .. }
            | Request::TaskUsage { .. }
            | Request::Costs { .. }
            // Read-only authored intent and spend, like Costs and Goals.
            | Request::Budget { .. }
            | Request::AgentOutput { .. }
            | Request::AgentScreen { .. }
            | Request::RunScreen { .. }
            | Request::WorkflowGet { .. }
            | Request::WorkflowList { .. }
            | Request::WorkflowRunGet { .. }
            | Request::WorkflowRunList { .. }
            // A preview folded from authored catalogues, and the evidence a
            // run already carries: nothing written (`#118`).
            | Request::WorkflowLint { .. }
            | Request::RunAttestations { .. }
            | Request::RunProvenance { .. }
            | Request::RoleList { .. }
            | Request::Subscribe => return Needs::Nothing,

            // Giving an agent a role is the owner's alone. An agent that could
            // hand itself one is not bounded by the one it has.
            Request::AgentRole { .. } => return Needs::Owner,
            // For the same reason, and not `agent.configure`: an agent that can
            // rewrite what a role allows can widen the one it holds.
            Request::RoleDefine { .. } | Request::RoleDelete { .. } => return Needs::Owner,
            // Explicit, destructive, and rare: only the owner cleans a bench
            // run's worktrees and branches away.
            Request::BenchRunClean { .. } => return Needs::Owner,
            // Restore materializes the whole company into a new root. Unlike
            // run/verify, no role grant opens it: choosing a cutover candidate
            // is the owner's decision alone (#153).
            Request::BackupRestore { .. } => return Needs::Owner,
            // Confirming or dismissing a possible security report is a
            // person's call, never an agent's, whatever role it holds --
            // AGENTS.md's "bounds what an agent does by accident, not what
            // it could do" (`#170`). Flagging (above) stays `intake.assess`;
            // this is the one door no grant opens.
            Request::IntakeSecurity { .. } => return Needs::Owner,
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

        // A role nothing in the caller's chain defines is refused rather than
        // defaulted: the agent is holding a job description nobody can read.
        // The chain is the caller's own scope's -- a role defined in
        // `projects/a` is no role at all to an agent in `projects/b`.
        let roles = self.roles_for(scope);
        let Some(def) = roles.get(role) else {
            return Err(deny(&format!(
                "act: {scope} does not define the role {role:?}. The roles available in {scope} are: {}",
                roles.names().join(", ")
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

        // The subject `knowledge.write`, `dataset.edit`, `bench.run`,
        // `policy.attest` and `goals.checkin` are checked against: the
        // knowledge base, datasets, bench runs, policy attestations and
        // goals check-ins are company-wide, not one project's, so only a
        // caller whose own scope *is* the instance's configured root scope
        // may hold any of them -- resolved from the live config, never from
        // a literal name like "root", which is only ever a convention for
        // what somebody chose to call theirs. An instance that declares no
        // root scope at all grants none of them to any agent, however it is
        // named.
        let in_root_scope = || -> Result<()> {
            match &self.factory_snapshot().config.scope {
                Some(root) if root.name == *scope => Ok(()),
                Some(root) => Err(FactoryError::Denied(format!(
                    "{} works in {scope}; the knowledge base, datasets, bench runs, policy \
                     attestations, goals check-ins and backups are company-wide and belong to the \
                     root scope ({:?}) alone",
                    caller.describe(),
                    root.name
                ))),
                None => Err(FactoryError::Denied(format!(
                    "{} may not write knowledge, manage datasets, run benchmarks, attest to a \
                     policy control, check in a goal, or take a backup; this instance declares no \
                     root scope",
                    caller.describe()
                ))),
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
            // The same reach rule as `TaskCreate` -- `scope` is required on
            // this request rather than optional, so there is no caller's-
            // own-scope fallback to mirror, but the check itself is
            // identical: a caller may only remediate a control in the one
            // scope it works in.
            Request::PolicyRemediate { scope: s, .. } => in_scope(s),
            // Same reach rule as `PolicyRemediate` -- a scenario is
            // promoted into the one scope the caller works in (`#100`).
            Request::ScenarioPromote { scope: s, .. } => in_scope(s),
            // And for a quality scenario's remediation task (`#107`).
            Request::QualityRemediate { scope: s, .. } => in_scope(s),

            Request::TaskUpdate { id, patch, .. } => {
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

            // Handing in follows `TaskCreate`'s rule to the letter.
            Request::IntakeAdd(new) => in_scope(new.scope.as_deref().unwrap_or(scope)),
            // Starting a triage run creates a task in the item's scope.
            Request::IntakeTriage { id, .. } => match self.store.get(id).await? {
                Some(item) => task_in_reach(def, &item),
                None => Ok(()),
            },
            // Assessing and deciding: reach over the item, or being the run
            // of its own triage task -- the one way an agent with no reach
            // over an item gets to answer for it, and only for that item.
            // Releasing moves it, so the route has to be in reach as well.
            Request::IntakeAssess { id, assessment, .. } => {
                let Some(item) = self.store.get(id).await? else { return Ok(()) };
                if task_in_reach(def, &item).is_err() && !self.is_items_triage_run(caller, &item).await? {
                    return Err(deny("assess an intake item that is neither in its reach nor its own triage run's"));
                }
                in_scope(&assessment.routing.scope)
            }
            // Flagging (`#170`): the same reach `IntakeAssess` checks, minus
            // the route -- there is none to be in scope of.
            Request::IntakeFlagSecurity { id, .. } => {
                let Some(item) = self.store.get(id).await? else { return Ok(()) };
                if task_in_reach(def, &item).is_err() && !self.is_items_triage_run(caller, &item).await? {
                    return Err(deny("flag an intake item that is neither in its reach nor its own triage run's"));
                }
                Ok(())
            }
            Request::IntakeDecide { id, decision } => {
                let Some(item) = self.store.get(id).await? else { return Ok(()) };
                if task_in_reach(def, &item).is_err() {
                    if !self.is_items_triage_run(caller, &item).await? {
                        return Err(deny("decide an intake item that is neither in its reach nor its own triage run's"));
                    }
                    // Its triage run proposes a split; making the items is
                    // for whoever answers for the item.
                    if matches!(decision, factory_core::intake::Decision::Split { .. }) {
                        return Err(deny("split an intake item it only triages -- propose the split in the assessment"));
                    }
                }
                // Deliberate tightening (`#172`): releasing and dispatching
                // in the same call needs `task.run` too. A workflow-routed
                // release already needs it, through `start_workflow`'s
                // create-and-run-every-node rule -- a direct release should
                // not be the one door around task execution.
                if matches!(decision, factory_core::intake::Decision::Ready { run: true }) && !def.allows(Grant::TaskRun)
                {
                    return Err(deny(&format!(
                        "release an intake item with run: true without holding {}",
                        Grant::TaskRun.as_str()
                    )));
                }
                let routed = item.intake.as_ref().and_then(|i| i.triage.as_ref()).map(|t| &t.assessment.routing.scope);
                match (decision, routed) {
                    (factory_core::intake::Decision::Ready { .. }, Some(routed)) => in_scope(routed),
                    _ => Ok(()),
                }
            }
            // Publishing: reach over the item, like `IntakeTriage` -- no
            // triage-run fallback, since publishing is never a run's own
            // work the way assessing or deciding can be.
            Request::IntakePublish { id } => match self.store.get(id).await? {
                Some(item) => task_in_reach(def, &item),
                None => Ok(()),
            },
            // Answering a needs-info: reach over the item, or having handed
            // it in.
            Request::IntakeInfo { id, .. } => {
                let Some(item) = self.store.get(id).await? else { return Ok(()) };
                let me = caller.describe();
                let requested = item
                    .intake
                    .as_ref()
                    .is_some_and(|i| i.requester == me || i.requester.ends_with(&format!("(via {me})")));
                if requested { Ok(()) } else { task_in_reach(def, &item) }
            }

            Request::TaskDelete { id }
            | Request::TaskRun { id, .. }
            | Request::TaskCancel { id, .. }
            | Request::TaskClose { id, .. }
            | Request::TaskReopen { id, .. }
            | Request::TaskSkipNext { id, .. } => {
                match self.store.get(id).await? {
                    Some(task) => task_in_reach(def, &task),
                    None => Ok(()), // let the engine report "no such task"
                }
            }

            Request::TaskReport { id, .. }
            | Request::TaskTurnEnded { id, .. }
            | Request::TaskAttach { id, .. } => {
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

            Request::AgentConfigure { scope: s, .. }
            | Request::AgentDelete { scope: s, .. } => match def.reach {
                Reach::Scope => in_scope(s),
                Reach::Own => Err(deny("configure an agent declaration")),
            },

            Request::DashboardSet { scope: s, .. } | Request::DashboardReset { scope: s } => {
                match def.reach {
                    Reach::Scope => in_scope(s),
                    Reach::Own => Err(deny("edit a scope's dashboard layout")),
                }
            }

            Request::AgentStop { id } | Request::AgentInput { id, .. } => match def.reach {
                Reach::Scope => match self.store.get_agent(id).await? {
                    Some(agent) => in_scope(&agent.scope),
                    None => Ok(()),
                },
                Reach::Own if id == &AgentSession::id_for(scope, name) => Ok(()),
                Reach::Own => Err(deny("reach into another agent's session")),
            },

            Request::RunInput { id, .. } | Request::RunAnswer { id, .. } => match def.reach {
                Reach::Scope => match self.task_of_run(id).await? {
                    Some(task) => in_scope(&task.scope),
                    None => Ok(()),
                },
                Reach::Own => match run_id {
                    Some(own) if own == id => Ok(()),
                    _ => Err(deny("type into another agent's session")),
                },
            },

            Request::RunApprove { id, .. }
            | Request::RunReject { id, .. }
            | Request::RunRework { id } => match self.task_of_run(id).await? {
                Some(task) => task_in_reach(def, &task),
                None => Ok(()),
            },

            Request::WorkflowCreate(draft) => match def.reach {
                Reach::Scope => in_scope(&draft.scope),
                Reach::Own => Err(deny("manage workflows; that requires scope reach")),
            },
            Request::WorkflowUpdate { id, workflow } => match def.reach {
                Reach::Scope => {
                    in_scope(&workflow.scope)?;
                    if let Some(found) = self.workflows.get_definition(id).await? {
                        in_scope(&found.scope)?;
                    }
                    Ok(())
                }
                Reach::Own => Err(deny("manage workflows; that requires scope reach")),
            },
            Request::WorkflowDelete { id } | Request::WorkflowStart { id, .. } => match def.reach {
                Reach::Scope => match self.workflows.get_definition(id).await? {
                    Some(found) => in_scope(&found.scope),
                    None => Ok(()),
                },
                Reach::Own => Err(deny("manage workflows; that requires scope reach")),
            },
            Request::WorkflowRunCancel { id } => match def.reach {
                Reach::Scope => match self.workflows.get_run(id).await? {
                    Some(found) => in_scope(&found.scope),
                    None => Ok(()),
                },
                Reach::Own => Err(deny("manage workflows; that requires scope reach")),
            },

            // The knowledge base is company-wide, like datasets and bench
            // runs, so its subject is the same fixed one: the root scope.
            Request::KnowledgeImport { .. }
            | Request::KnowledgeAdd { .. }
            | Request::KnowledgeWriteFile { .. } => match def.reach {
                Reach::Scope => in_root_scope(),
                Reach::Own => Err(deny("write to the knowledge base; that requires scope reach")),
            },
            Request::DatasetCreate { .. }
            | Request::DatasetAddCases { .. }
            | Request::DatasetImport { .. }
            | Request::DatasetFromTasks { .. }
            | Request::DatasetDeleteCase { .. }
            | Request::DatasetDelete { .. } => match def.reach {
                Reach::Scope => in_root_scope(),
                Reach::Own => Err(deny("manage datasets; that requires scope reach")),
            },
            Request::BenchRunStart { .. } | Request::BenchRunCancel { .. } => match def.reach {
                Reach::Scope => in_root_scope(),
                Reach::Own => Err(deny("run benchmarks; that requires scope reach")),
            },
            Request::PolicyAttest { .. } | Request::PolicyWithdraw { .. } => match def.reach {
                Reach::Scope => in_root_scope(),
                Reach::Own => Err(deny("record or withdraw a policy attestation; that requires scope reach")),
            },
            Request::GoalsCheckIn { .. } => match def.reach {
                Reach::Scope => in_root_scope(),
                Reach::Own => Err(deny("record a goals check-in; that requires scope reach")),
            },
            Request::BackupRun | Request::BackupVerify { .. } => match def.reach {
                Reach::Scope => in_root_scope(),
                Reach::Own => Err(deny("take or verify a backup; that requires scope reach")),
            },
            // A deployment belongs to its environment's scope. Own reach
            // covers what the caller's own run deploys and nothing else: it
            // may start one from a run, and finish only one its run started.
            Request::DeployStart(req) => {
                in_scope(&self.deploy_scope(req, caller)?)?;
                match def.reach {
                    Reach::Scope => Ok(()),
                    Reach::Own if run_id.is_some() => Ok(()),
                    Reach::Own => Err(deny("record a deployment outside a run of its own")),
                }
            }
            Request::EnvironmentCheck { environment } => {
                let scope = self.factory_snapshot().config.environments().into_iter()
                    .find(|(_, declaration)| declaration.name == *environment).map(|(scope, _)| scope)
                    .ok_or_else(|| FactoryError::BadRequest(format!("no declared environment {environment}")))?;
                in_scope(&scope)?;
                match def.reach {
                    Reach::Scope => Ok(()),
                    Reach::Own if run_id.is_some() => Ok(()),
                    Reach::Own => Err(deny("verify an environment outside a run of its own")),
                }
            }
            Request::DeployFinish(req) => {
                let Some(deployment) = self.environments.deployment(&req.id).await? else {
                    return Err(FactoryError::BadRequest(format!("no deployment {}", req.id)));
                };
                in_scope(&deployment.scope)?;
                match def.reach {
                    Reach::Scope => Ok(()),
                    Reach::Own if run_id.is_some() && deployment.actor.run_id == *run_id => Ok(()),
                    Reach::Own => Err(deny("finish a deployment its own run did not start")),
                }
            }
            Request::ReleaseAdd(req) => {
                in_scope(&req.scope)?;
                match def.reach {
                    Reach::Scope => Ok(()),
                    Reach::Own if run_id.is_some() => Ok(()),
                    Reach::Own => Err(deny("add a release outside a run of its own")),
                }
            }

            // Reads returned above, and anything needing a grant nobody holds
            // was refused above. Nothing should arrive here.
            _ => Err(deny("do that")),
        }
    }

    /// Re-derive who a persisted `WorkflowActor` is right now, honouring a
    /// role change since the run started -- a workflow run remembers who
    /// asked, not a frozen copy of what they were allowed to do that moment.
    pub(crate) async fn caller_for_actor(&self, actor: &WorkflowActor) -> Caller {
        match actor {
            WorkflowActor::Owner => Caller::Owner,
            WorkflowActor::Agent { scope, name } => Caller::Agent {
                scope: scope.clone(),
                name: name.clone(),
                role: self.effective_role(scope, name).await,
                run_id: None,
            },
        }
    }

    /// The authority a hand-typed `task.create` immediately followed by
    /// `task.run` would need from `caller` -- exactly what a workflow node's
    /// spawn must never exceed. The task does not exist yet when a node
    /// becomes eligible, so `TaskRun` is checked against an id nothing has
    /// created: `authorize` already treats an unknown id as "let the engine
    /// report `no such task`" rather than as anybody's, which is task.run's
    /// grant-and-scope shape with no task-specific reach left to weigh in.
    pub(crate) async fn authorize_workflow_spawn(
        &self,
        caller: &Caller,
        template: &NewTask,
    ) -> Result<()> {
        self.authorize(caller, &Request::TaskCreate(template.clone()))
            .await?;
        self.authorize(
            caller,
            &Request::TaskRun {
                override_wait: false,
                id: uuid::Uuid::new_v4().to_string(),
                reason: None,
                continue_run: false,
            },
        )
        .await
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
    use factory_core::config::{Config, DaemonConfig, Factory, Instance, ScopeAgent};
    use factory_core::intake::{Assessment, Decision, Level, NewIntake, Routing, WontfixReason};
    use factory_core::run::{NewRun, RunStatus, Trigger};
    use factory_core::task::{CloseReason, NewTask, Task, TaskPatch, TaskReport, TaskStatus};
    use factory_core::workflow::WorkflowDraft;
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
            scope: None,
            scopes: vec![
                serde_yaml_ng::from_str("name: demo\npath: .\n").unwrap(),
                serde_yaml_ng::from_str("name: other\npath: .\n").unwrap(),
            ],
            roles: Default::default(),
            dashboard: None,
            policies: Default::default(),
            quality: Default::default(),
            infrastructure: Default::default(),
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

    /// The same, for an instance that also names a root scope -- what
    /// `dataset.edit` and `bench.run` are checked against, since datasets and
    /// bench runs are company-wide rather than any one scope's own.
    fn engine_with_roles_and_root_scope(root: &str, yaml: &str) -> Arc<Engine> {
        let config: Config = serde_yaml_ng::from_str(&format!(
            "instance:\n  id: i\n  name: test\nscope:\n  name: {root}\n  path: .\nscopes:\n  - name: demo\n    path: .\n  - name: other\n    path: .\n{yaml}"
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

    #[tokio::test]
    async fn workflow_grants_require_scope_reach_and_stop_at_the_scope_boundary() {
        let e = engine();
        let here = Request::WorkflowCreate(WorkflowDraft {
            name: "flow".into(), scope: "demo".into(), ..Default::default()
        });
        let elsewhere = Request::WorkflowCreate(WorkflowDraft {
            name: "flow".into(), scope: "other".into(), ..Default::default()
        });
        assert!(allowed(&e, &foreman(), here.clone()).await);
        assert!(!allowed(&e, &foreman(), elsewhere).await);
        assert!(!allowed(&e, &worker("w"), here).await);

        let own_reach = engine_with_roles("roles:\n  workflow-author:\n    grants: [workflow.create, workflow.edit, workflow.delete, workflow.run, workflow.cancel]\n    reach: own\n");
        assert!(!allowed(
            &own_reach,
            &wearing("workflow-author"),
            Request::WorkflowCreate(WorkflowDraft {
                name: "flow".into(), scope: "demo".into(), ..Default::default()
            })
        ).await, "workflow mutation is deliberately a scope-level ability");

        // `WorkflowStart` is refused the same way `WorkflowCreate` is: `own`
        // reach is not enough to run one at all, regardless of which id it
        // names. This is unmodified by B1's spawn-time authorization -- the
        // request-level grant check below it stays the gate it always was.
        assert!(
            !allowed(
                &own_reach,
                &wearing("workflow-author"),
                Request::WorkflowStart { id: "missing".into(), inputs: Default::default() }
            )
            .await,
            "starting a workflow is refused for reach alone, before any id is even looked up"
        );
    }

    /// `dataset.edit` and `bench.run` are subject to the instance's
    /// *configured* root scope, resolved from `config.scope` -- never a
    /// literal scope named "root", and never granted at all when the
    /// instance declares no root scope. Without either grant, both write
    /// paths are refused; holding the grant outside the root scope is
    /// refused too, since the subject is the root scope, not the caller's
    /// own.
    #[tokio::test]
    async fn dataset_and_bench_grants_are_checked_against_the_configured_root_scope() {
        let e = engine_with_roles_and_root_scope(
            "demo",
            "roles:\n  bencher:\n    grants: [dataset.edit, bench.run]\n    reach: scope\n",
        );

        let create = Request::DatasetCreate { name: "ds".into(), description: None };
        let start = Request::BenchRunStart {
            dataset: "ds".into(),
            agents: vec!["shell".into()],
            attempts: None,
            concurrency: None,
            cases: None,
        };

        let in_root = Caller::Agent {
            scope: "demo".into(),
            name: "w".into(),
            role: Role::new("bencher"),
            run_id: None,
        };
        let outside_root = Caller::Agent {
            scope: "other".into(),
            name: "w".into(),
            role: Role::new("bencher"),
            run_id: None,
        };

        assert!(
            allowed(&e, &in_root, create.clone()).await,
            "the root scope's own caller, holding the grant, may create a dataset"
        );
        assert!(
            allowed(&e, &in_root, start.clone()).await,
            "the root scope's own caller, holding the grant, may start a bench run"
        );
        assert!(
            !allowed(&e, &outside_root, create).await,
            "the same grant held outside the root scope is refused -- the subject is the \
             root scope, not wherever the caller happens to work"
        );
        assert!(!allowed(&e, &outside_root, start).await);

        // A caller in the root scope but without the grant at all is refused
        // the ordinary way, before the root-scope check is ever reached.
        assert!(
            !allowed(&e, &worker("w"), Request::DatasetCreate { name: "ds2".into(), description: None }).await
        );

        // An instance that declares no root scope at all grants neither to
        // anybody, however the caller is named or where it works.
        let no_root = engine_with_roles(
            "roles:\n  bencher:\n    grants: [dataset.edit, bench.run]\n    reach: scope\n",
        );
        let demo_caller = Caller::Agent {
            scope: "demo".into(),
            name: "w".into(),
            role: Role::new("bencher"),
            run_id: None,
        };
        assert!(
            !allowed(&no_root, &demo_caller, Request::DatasetCreate { name: "ds3".into(), description: None })
                .await,
            "no root scope is configured, so nobody may manage datasets"
        );

        // The owner is never subject to any of this.
        assert!(allowed(&e, &Caller::Owner, Request::DatasetCreate { name: "ds4".into(), description: None }).await);
    }

    /// `policy.attest` is checked against the configured root scope exactly
    /// like `dataset.edit` and `bench.run` above -- the same rule the ADR
    /// states for attestations, and the same one `access.rs` enforces
    /// through the same `in_root_scope` closure.
    #[tokio::test]
    async fn policy_attest_is_checked_against_the_configured_root_scope() {
        let e = engine_with_roles_and_root_scope(
            "demo",
            "roles:\n  attestor:\n    grants: [policy.attest]\n    reach: scope\n",
        );

        let attest = Request::PolicyAttest {
            control: "cra/a".parse().unwrap(),
            scope: "demo".into(),
            evidence: "https://example.com/policy".into(),
            note: None,
            expires_at: Utc::now() + chrono::Duration::days(30),
            corrective: None,
            clock: None,
        };
        let withdraw = Request::PolicyWithdraw { id: "att-1".into(), reason: None };

        let in_root = Caller::Agent {
            scope: "demo".into(),
            name: "w".into(),
            role: Role::new("attestor"),
            run_id: None,
        };
        let outside_root = Caller::Agent {
            scope: "other".into(),
            name: "w".into(),
            role: Role::new("attestor"),
            run_id: None,
        };

        assert!(
            allowed(&e, &in_root, attest.clone()).await,
            "the root scope's own caller, holding the grant, may attest"
        );
        assert!(
            allowed(&e, &in_root, withdraw.clone()).await,
            "the root scope's own caller, holding the grant, may withdraw"
        );
        assert!(
            !allowed(&e, &outside_root, attest).await,
            "the same grant held outside the root scope is refused -- the subject is the \
             root scope, not wherever the caller happens to work"
        );
        assert!(!allowed(&e, &outside_root, withdraw).await);

        // A caller in the root scope but without the grant at all is refused
        // the ordinary way, before the root-scope check is ever reached.
        assert!(
            !allowed(
                &e,
                &worker("w"),
                Request::PolicyAttest {
                    control: "cra/a".parse().unwrap(),
                    scope: "demo".into(),
                    evidence: "https://example.com/policy".into(),
                    note: None,
                    expires_at: Utc::now() + chrono::Duration::days(30),
                    corrective: None,
                    clock: None,
                }
            )
            .await
        );

        // The owner is never subject to any of this.
        assert!(
            allowed(
                &e,
                &Caller::Owner,
                Request::PolicyAttest {
                    control: "cra/a".parse().unwrap(),
                    scope: "demo".into(),
                    evidence: "https://example.com/policy".into(),
                    note: None,
                    expires_at: Utc::now() + chrono::Duration::days(30),
                    corrective: None,
                    clock: None,
                }
            )
            .await
        );
    }

    /// `backup.run` covers taking and verifying a backup, and is checked
    /// against the configured root scope exactly like `policy.attest`;
    /// reading the backup status is open to every agent (`#116`).
    #[tokio::test]
    async fn backup_run_is_checked_against_the_configured_root_scope_and_reading_is_open() {
        let e = engine_with_roles_and_root_scope(
            "demo",
            "roles:\n  keeper:\n    grants: [backup.run]\n    reach: scope\n  \
             own-keeper:\n    grants: [backup.run]\n    reach: own\n",
        );
        let caller = |scope: &str, role: &str| Caller::Agent {
            scope: scope.into(),
            name: "w".into(),
            role: Role::new(role),
            run_id: None,
        };
        let verify = Request::BackupVerify { snapshot: None, identity: None };

        assert!(allowed(&e, &caller("demo", "keeper"), Request::BackupRun).await);
        assert!(allowed(&e, &caller("demo", "keeper"), verify.clone()).await);
        assert!(
            !allowed(&e, &caller("other", "keeper"), Request::BackupRun).await,
            "the same grant outside the root scope is refused"
        );
        assert!(
            !allowed(&e, &caller("demo", "own-keeper"), Request::BackupRun).await,
            "own reach never covers the whole instance"
        );
        assert!(!allowed(&e, &worker("w"), verify).await, "a worker holds no backup.run");
        assert!(allowed(&e, &worker("w"), Request::Backup).await, "reading the status is open to every agent");
        assert!(allowed(&e, &worker("w"), Request::Doctor).await, "Doctor is a read open to every agent");
        assert!(allowed(&e, &Caller::Owner, Request::BackupRun).await);
    }

    /// `#152`: verifying an encrypted snapshot decrypts it, so an `identity`
    /// makes the request the owner's alone -- even for a caller whose role
    /// holds `backup.run` and reaches the whole scope.
    #[tokio::test]
    async fn backup_verify_with_an_identity_is_the_owners_alone_even_with_backup_run() {
        let e = engine_with_roles_and_root_scope(
            "demo",
            "roles:\n  keeper:\n    grants: [backup.run]\n    reach: scope\n",
        );
        let caller = Caller::Agent { scope: "demo".into(), name: "w".into(), role: Role::new("keeper"), run_id: None };
        let with_identity = Request::BackupVerify { snapshot: None, identity: Some(PathBuf::from("/tmp/key.txt")) };
        assert!(
            !allowed(&e, &caller, with_identity.clone()).await,
            "a role grant must never become \"read this key file\""
        );
        assert!(allowed(&e, &Caller::Owner, with_identity).await);
        // Without an identity, the same caller's `backup.run` still works --
        // this is not a general backup.verify regression.
        assert!(allowed(&e, &caller, Request::BackupVerify { snapshot: None, identity: None }).await);
    }

    #[tokio::test]
    async fn backup_restore_is_the_owners_alone_even_when_a_role_has_every_grant() {
        let e = engine_with_roles("roles:\n  everything:\n    grants: ['*']\n    reach: scope\n");
        let request = Request::BackupRestore {
            snapshot: "factory-backup-demo-20260925T030000Z.tar.zst".into(),
            into: PathBuf::from("/tmp/restored-factory"),
            identity: None,
        };
        assert!(allowed(&e, &Caller::Owner, request.clone()).await);
        assert!(!allowed(&e, &wearing("everything"), request.clone()).await);
        assert!(!allowed(&e, &foreman(), request.clone()).await);
        assert!(!allowed(&e, &worker("w"), request).await);
    }

    /// `deploy.record` is checked against the environment's own scope; own
    /// reach covers only what a run of its own deploys, and reading the
    /// environments is open to every agent (`#185`).
    #[tokio::test]
    async fn starting_a_promotion_is_owner_only_even_for_a_foreman() {
        let e = engine_with_roles_and_root_scope("company", "roles: {}\n");
        let request = || Request::EnvironmentPromote(factory_core::environments::Promote {
            environment: "staging".into(), deployment: "selected".into(),
        });
        let agent = Caller::Agent { scope: "demo".into(), name: "release".into(), role: Role::foreman(), run_id: None };
        assert!(!allowed(&e, &agent, request()).await);
        assert!(allowed(&e, &Caller::Owner, request()).await);
    }

    #[tokio::test]
    async fn recovery_is_owner_only_and_checking_requires_deploy_record_in_the_environment_scope() {
        let source = engine_with_roles_and_root_scope("company", "roles:\n  checker:\n    grants: [deploy.record]\n    reach: scope\n  own-checker:\n    grants: [deploy.record]\n    reach: own\n");
        let mut factory = source.factory_snapshot();
        factory.config.scopes[0].environments = serde_yaml_ng::from_str("[{ name: production, checks: [{ kind: command, command: 'true' }] }]").unwrap();
        let e = Engine::new(factory, Registry::with_builtins(), source.store.clone(), PathBuf::from("factory"), vec![]);
        let caller = |scope: &str, role: &str, run: Option<&str>| Caller::Agent {
            scope: scope.into(), name: "operator".into(), role: Role::new(role), run_id: run.map(str::to_owned),
        };
        let recover = || Request::EnvironmentRecover(factory_core::environments::Recover { environment: "production".into(), reason: "restart".into() });
        assert!(allowed(&e, &Caller::Owner, recover()).await);
        assert!(!allowed(&e, &caller("demo", "foreman", None), recover()).await);
        let check = || Request::EnvironmentCheck { environment: "production".into() };
        assert!(allowed(&e, &caller("demo", "checker", None), check()).await);
        assert!(!allowed(&e, &caller("other", "checker", None), check()).await);
        assert!(!allowed(&e, &caller("demo", "own-checker", None), check()).await);
        assert!(allowed(&e, &caller("demo", "own-checker", Some("r")), check()).await);
        assert!(!allowed(&e, &worker("w"), check()).await);
        assert!(!allowed(&e, &caller("demo", "checker", None), Request::EnvironmentCheck { environment: "missing".into() }).await);
    }

    #[tokio::test]
    async fn deploy_record_follows_the_environments_scope_and_own_reach_needs_a_run() {
        use factory_core::environments::{DeployFinish, DeployStart, DeployStatus, ReleaseFacts};
        let e = engine_with_roles_and_root_scope(
            "company",
            "roles:\n  releaser:\n    grants: [deploy.record]\n    reach: scope\n  \
             own-releaser:\n    grants: [deploy.record]\n    reach: own\n",
        );
        let caller = |role: &str, run: Option<&str>| Caller::Agent {
            scope: "demo".into(),
            name: "r".into(),
            role: Role::new(role),
            run_id: run.map(str::to_string),
        };
        let start = |scope: Option<&str>| {
            Request::DeployStart(DeployStart {
                environment: "review13".into(),
                strict_verification: false,
                scope: scope.map(str::to_string),
                release: ReleaseFacts { commit: "abc".into(), ..Default::default() },
                via: None,
                started_at: None,
            })
        };
        assert!(allowed(&e, &caller("releaser", None), start(None)).await, "its own scope");
        assert!(!allowed(&e, &caller("releaser", None), start(Some("other"))).await, "another scope");
        assert!(!allowed(&e, &caller("own-releaser", None), start(None)).await, "own reach without a run");
        assert!(allowed(&e, &caller("own-releaser", Some("run-1")), start(None)).await, "its own run's deployment");
        assert!(!allowed(&e, &worker("w"), start(None)).await, "a worker holds no deploy.record");
        assert!(allowed(&e, &worker("w"), Request::Environments { scope: None }).await, "reading is open");

        let d = e.deploy_start(&caller("releaser", Some("run-1")), DeployStart {
            environment: "review13".into(),
            strict_verification: false,
            scope: None,
            release: ReleaseFacts { commit: "abc".into(), ..Default::default() },
            via: None,
            started_at: None,
        })
        .await
        .unwrap();
        let finish = Request::DeployFinish(DeployFinish { id: d.id.clone(), status: DeployStatus::Failed, reason: None, verify: true });
        assert!(allowed(&e, &caller("own-releaser", Some("run-1")), finish.clone()).await, "the run that started it");
        assert!(!allowed(&e, &caller("own-releaser", Some("run-2")), finish.clone()).await, "another run");
        assert!(allowed(&e, &caller("releaser", None), finish).await, "scope reach");
        assert!(allowed(&e, &Caller::Owner, start(Some("other"))).await);
    }

    /// `goals.checkin` is checked against the configured root scope exactly
    /// like `policy.attest` -- the same `in_root_scope` closure, since a
    /// check-in speaks for the company's own goals, not for one project.
    #[tokio::test]
    async fn goals_checkin_is_checked_against_the_configured_root_scope() {
        let e = engine_with_roles_and_root_scope(
            "demo",
            "roles:\n  checker:\n    grants: [goals.checkin]\n    reach: scope\n",
        );

        let checkin = || Request::GoalsCheckIn {
            kr: "ship-compliant/dpa-signed".parse().unwrap(),
            value: 3.0,
            confidence: 7,
            note: None,
        };
        let in_root = Caller::Agent {
            scope: "demo".into(),
            name: "w".into(),
            role: Role::new("checker"),
            run_id: None,
        };
        let outside_root = Caller::Agent {
            scope: "other".into(),
            name: "w".into(),
            role: Role::new("checker"),
            run_id: None,
        };

        assert!(
            allowed(&e, &in_root, checkin()).await,
            "the root scope's own caller, holding the grant, may check in"
        );
        assert!(
            !allowed(&e, &outside_root, checkin()).await,
            "the same grant held outside the root scope is refused -- goals check-ins are \
             company-wide, not scoped to wherever the caller works"
        );
        assert!(
            !allowed(&e, &worker("w"), checkin()).await,
            "a caller in the root scope but without the grant at all is refused the ordinary way"
        );
        assert!(allowed(&e, &Caller::Owner, checkin()).await, "the owner is never subject to any of this");
    }

    /// `policy.remediate` is checked exactly like `TaskCreate` itself: the
    /// same grant (`task.create`), the same reach (the caller's own scope,
    /// never a name it does not work in). Unlike `policy.attest`, this is
    /// not a root-scope-only grant -- a caller with `task.create` may
    /// remediate in its own scope, same as it may create a task there.
    /// A positive case is the point here, not just "the owner may" and "no
    /// grant is refused": those two alone would still pass with the
    /// `authorize` arm for `PolicyRemediate` missing entirely (the owner
    /// bypasses `authorize`, and a missing arm still falls through to the
    /// final `_ => Err(deny("do that"))`), so this also proves a role that
    /// *does* hold the grant is actually let through.
    #[tokio::test]
    async fn policy_remediate_needs_task_create_in_the_callers_own_scope() {
        let e = engine_with_roles("roles:\n  remediator:\n    grants: [task.create]\n    reach: scope\n");

        let request = |scope: &str| Request::PolicyRemediate {
            control: "cra/a".parse().unwrap(),
            scope: scope.into(),
            agent: None,
        };

        let in_scope = Caller::Agent {
            scope: "demo".into(),
            name: "w".into(),
            role: Role::new("remediator"),
            run_id: None,
        };
        assert!(
            allowed(&e, &in_scope, request("demo")).await,
            "a role holding task.create may remediate in its own scope"
        );
        assert!(
            !allowed(&e, &in_scope, request("other")).await,
            "the same grant does not reach a scope this caller does not work in"
        );

        assert!(
            !allowed(&e, &worker("w"), request("demo")).await,
            "a role without task.create is refused, the same as TaskCreate itself would be"
        );

        assert!(
            allowed(&e, &Caller::Owner, request("demo")).await,
            "the owner is never subject to any of this"
        );
    }

    /// `scenario.promote` (`#100`) is the same door `policy.remediate`
    /// already is: `Grant::TaskCreate`, the caller's own scope, never a
    /// wildcard. Mirrors `policy_remediate_needs_task_create_in_the_callers_own_scope`
    /// exactly, for the same reason that test states its own purpose: a
    /// positive case proves the `authorize` arm is actually wired, not just
    /// absent and falling through to the owner-only default.
    #[tokio::test]
    async fn scenario_promote_needs_task_create_in_the_callers_own_scope() {
        let e = engine_with_roles("roles:\n  remediator:\n    grants: [task.create]\n    reach: scope\n");

        let request = |scope: &str| Request::ScenarioPromote {
            scenario: "s".into(),
            scope: scope.into(),
            agent: None,
        };

        let in_scope = Caller::Agent {
            scope: "demo".into(),
            name: "w".into(),
            role: Role::new("remediator"),
            run_id: None,
        };
        assert!(
            allowed(&e, &in_scope, request("demo")).await,
            "a role holding task.create may promote a scenario in its own scope"
        );
        assert!(
            !allowed(&e, &in_scope, request("other")).await,
            "the same grant does not reach a scope this caller does not work in"
        );

        assert!(
            !allowed(&e, &worker("w"), request("demo")).await,
            "a role without task.create is refused, the same as TaskCreate itself would be"
        );

        assert!(
            allowed(&e, &Caller::Owner, request("demo")).await,
            "the owner is never subject to any of this"
        );
    }

    /// `quality.remediate` (`#107`) is the same door once more -- mirrors
    /// `scenario_promote_needs_task_create_in_the_callers_own_scope`, for
    /// the same reason: the positive case proves the arm is wired.
    #[tokio::test]
    async fn quality_remediate_needs_task_create_in_the_callers_own_scope() {
        let e = engine_with_roles("roles:\n  remediator:\n    grants: [task.create]\n    reach: scope\n");

        let request = |scope: &str| Request::QualityRemediate {
            scope: scope.into(),
            attribute: "reliability".into(),
            scenario: "s".into(),
            agent: None,
        };

        let in_scope = Caller::Agent {
            scope: "demo".into(),
            name: "w".into(),
            role: Role::new("remediator"),
            run_id: None,
        };
        assert!(
            allowed(&e, &in_scope, request("demo")).await,
            "a role holding task.create may remediate a quality scenario in its own scope"
        );
        assert!(
            !allowed(&e, &in_scope, request("other")).await,
            "the same grant does not reach a scope this caller does not work in"
        );
        assert!(
            !allowed(&e, &worker("w"), request("demo")).await,
            "a role without task.create is refused, the same as TaskCreate itself would be"
        );
        assert!(
            allowed(&e, &Caller::Owner, request("demo")).await,
            "the owner is never subject to any of this"
        );
    }

    async fn task_in(engine: &Engine, id: &str, scope: &str, agent: &str) -> Task {
        let now = Utc::now();
        let task = Task {
            after: None,
            after_condition: None,
            id: id.into(),
            title: id.into(),
            instructions: String::new(),
            scope: scope.into(),
            agent: agent.into(),
            runtime: "herdr".into(),
            status: TaskStatus::Pending,
            schedule: None,
            estimate_seconds: None,
            estimate: None,
            result: None,
            routed_to: None,
            error: None,
            runs: 0,
            ack_timeout_seconds: None,
            timeout_seconds: None,
            blocked_timeout_seconds: None,
            worktree: false,
            knowledge_hints: false,
            labels: Default::default(),
            created_at: now,
            updated_at: now,
            last_run_at: None,
            next_run_at: None,
            workflow_origin: None,
            parent_task_id: None,
            decomposition_part: None,
            depends_on: Vec::new(),
            bench_origin: None,
            retry: None,
            pending_retry: None,
            schedule_paused: false,
            category: None,
            intake: None,
            failure: None,
            closure: None,
            slot_wait: None,
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
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap()
            .id
    }

    #[tokio::test]
    async fn run_approval_is_a_central_grant_and_follows_the_subject_tasks_reach() {
        let e =
            engine_with_roles("roles:\n  approver:\n    grants: [run.approve]\n    reach: scope\n");
        task_in(&e, "mine", "demo", "maker").await;
        task_in(&e, "theirs", "other", "maker").await;
        let mine = run_of(&e, "mine", "maker").await;
        let theirs = run_of(&e, "theirs", "maker").await;
        let request = |id: &str| Request::RunApprove {
            id: id.into(),
            reason: "checked".into(),
        };
        assert!(
            !allowed(&e, &worker("maker"), request(&mine)).await,
            "worker has no run.approve"
        );
        assert!(allowed(&e, &wearing("approver"), request(&mine)).await);
        let executing_approver = Caller::Agent {
            scope: "demo".into(),
            name: "maker".into(),
            role: Role::new("approver"),
            run_id: Some(mine.clone()),
        };
        assert!(
            allowed(&e, &executing_approver, Request::RunRework { id: mine.clone() }).await,
            "reach/grant admits the request; the engine's identity check must reject self-rework"
        );
        assert!(
            !allowed(&e, &wearing("approver"), request(&theirs)).await,
            "scope reach stops at demo"
        );
        assert!(allowed(&e, &Caller::Owner, request(&theirs)).await);
    }

    fn report() -> TaskReport {
        TaskReport {
            artifacts: Vec::new(),
            status: Some(RunStatus::Running),
            message: Some("working".into()),
            result: None,
            send_to: None,
            error: None,
            token: None,
        }
    }

    fn declaration() -> ScopeAgent {
        serde_yaml_ng::from_str("name: reviewer\nharness: pi\nlifetime: task\n").unwrap()
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
            Request::AgentConfigure {
                scope: "other".into(),
                agent: declaration(),
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
                Request::RuntimeConnections,
                Request::Agents,
                Request::Occupancy { minutes: None, from: None, to: None },
                Request::Production { minutes: None, bin: None, scope: None },
                Request::TaskGet { id: "t".into() },
                Request::TaskList(Default::default()),
                Request::TaskEntries {
                    id: "t".into(),
                    limit: None,
                    task_only: true,
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
                Request::Policy { scope: None },
                Request::PolicyControl {
                    control: "cra/a".parse().unwrap(),
                    scope: "demo".into(),
                },
                Request::Scenarios { scope: None },
                Request::Quality { scope: None },
                Request::ScenarioWhatIf {
                    scope: None,
                    scenario: None,
                    drivers: Default::default(),
                },
                Request::Operations {
                    scope: None,
                    window: Default::default(),
                    detail: true,
                },
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
                    reason: None,
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
                    reason: None,
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
    async fn a_turn_end_is_held_to_exactly_what_a_report_is() {
        // The harness speaks for the run its session belongs to, no more:
        // the same grant and the same reach as the agent's own report.
        let e = engine();
        task_in(&e, "mine", "demo", "w").await;
        task_in(&e, "theirs", "demo", "someone-else").await;
        let turn = || factory_core::task::TurnEnded {
            event: factory_core::task::TurnEndEvent::Stop,
            pending_background: 0,
            error: None,
            error_details: None,
            last_message: None,
            token: None,
            session_id: None,
        };
        assert!(allowed(&e, &worker("w"), Request::TaskTurnEnded { id: "mine".into(), turn: turn() }).await);
        assert!(!allowed(&e, &worker("w"), Request::TaskTurnEnded { id: "theirs".into(), turn: turn() }).await);
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
                        reason: None,
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
        // An answer is typing into a session, and reaches exactly as far.
        assert!(
            !allowed(
                &e,
                &caller,
                Request::RunAnswer {
                    id: "somebody-elses-run".into(),
                    text: "yes".into(),
                    reason: "it asked".into(),
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
            Request::TaskRun { override_wait: false, id: "t".into(), reason: None, continue_run: false },
            Request::TaskCancel { id: "t".into(), reason: None, run: None },
            // Closing a task is a decision about it, not work on it (`#122`).
            Request::TaskClose {
                id: "t".into(),
                reason: factory_core::task::CloseReason::NotPlanned,
                duplicate_of: None,
                note: None,
            },
            Request::TaskReopen { id: "t".into(), reason: None },
            Request::AgentStart {
                scope: "demo".into(),
                name: "w".into(),
            },
            Request::AgentConfigure {
                scope: "demo".into(),
                agent: declaration(),
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
        // Skipping its own task's next slot is editing its own task, which a
        // worker may do -- the same grant, and the same reach, as pausing
        // that schedule through `task.update`.
        assert!(allowed(&e, &worker("w"), Request::TaskSkipNext { id: "t".into(), reason: None, slot: None }).await);
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
                reason: None,
            },
            Request::TaskDelete { id: "here".into() },
            Request::TaskRun { override_wait: false, id: "here".into(), reason: None, continue_run: false },
            Request::TaskCancel { id: "here".into(), reason: None, run: None },
            Request::TaskSkipNext { id: "here".into(), reason: None, slot: None },
            Request::TaskReport {
                id: "here".into(),
                report: report(),
            },
            Request::AgentStart {
                scope: "demo".into(),
                name: "w".into(),
            },
            Request::AgentConfigure {
                scope: "demo".into(),
                agent: declaration(),
            },
            Request::RunInput {
                id: run.clone(),
                text: Some("hi".into()),
                keys: vec![],
            },
            Request::RunAnswer {
                id: run,
                text: "yes".into(),
                reason: "it asked".into(),
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
                reason: None,
            },
            Request::TaskDelete {
                id: "elsewhere".into(),
            },
            Request::TaskRun {
                override_wait: false,
                id: "elsewhere".into(),
                reason: None,
                continue_run: false,
            },
            Request::TaskCancel {
                id: "elsewhere".into(),
                reason: None,
                run: None,
            },
            Request::TaskSkipNext {
                id: "elsewhere".into(),
                reason: None,
                slot: None,
            },
            Request::TaskReport {
                id: "elsewhere".into(),
                report: report(),
            },
            Request::AgentStart {
                scope: "other".into(),
                name: "w".into(),
            },
            Request::AgentConfigure {
                scope: "other".into(),
                agent: declaration(),
            },
            Request::RunInput {
                id: run.clone(),
                text: Some("hi".into()),
                keys: vec![],
            },
            Request::RunAnswer {
                id: run,
                text: "yes".into(),
                reason: "it asked".into(),
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
                    reason: None,
                }
            )
            .await
        );
    }

    #[tokio::test]
    async fn task_close_is_its_own_grant_and_reaches_only_as_far_as_the_role() {
        let e = engine_with_roles("roles:\n  closer:\n    grants: [task.close]\n    reach: scope\n");
        task_in(&e, "here", "demo", "somebody").await;
        let close = |id: &str| Request::TaskClose {
            id: id.into(),
            reason: factory_core::task::CloseReason::NotPlanned,
            duplicate_of: None,
            note: None,
        };
        assert!(allowed(&e, &wearing("closer"), close("here")).await);
        assert!(allowed(&e, &wearing("closer"), Request::TaskReopen { id: "here".into(), reason: None }).await);
        assert!(
            !allowed(&e, &wearing("closer"), Request::TaskCancel { id: "here".into(), reason: None, run: None }).await,
            "closing a task is not cancelling a run"
        );
        assert_eq!(Grant::expand("task.close").unwrap(), vec![Grant::TaskClose]);
    }

    // -- roles an instance names for itself --------------------------------

    #[tokio::test]
    async fn a_named_role_may_do_what_its_grants_say_and_no_more() {
        let e = engine_with_roles(
            "roles:\n  runner:\n    grants: [task.run, task.cancel]\n    reach: scope\n",
        );
        task_in(&e, "here", "demo", "somebody").await;
        assert!(allowed(&e, &wearing("runner"), Request::TaskRun { override_wait: false, id: "here".into(), reason: None, continue_run: false }).await);
        assert!(allowed(&e, &wearing("runner"), Request::TaskCancel { id: "here".into(), reason: None, run: None }).await);
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
                    reason: None,
                }
            )
            .await
        );
    }

    #[tokio::test]
    async fn configuring_agents_requires_scope_reach() {
        let e = engine_with_roles(
            "roles:\n  self-editor:\n    grants: [agent.configure]\n    reach: own\n  scope-editor:\n    grants: [agent.configure]\n    reach: scope\n",
        );
        let request = |scope: &str| Request::AgentConfigure {
            scope: scope.into(),
            agent: declaration(),
        };

        assert!(!allowed(&e, &wearing("self-editor"), request("demo")).await);
        assert!(allowed(&e, &wearing("scope-editor"), request("demo")).await);
        assert!(!allowed(&e, &wearing("scope-editor"), request("other")).await);
        assert!(
            allowed(
                &e,
                &wearing("scope-editor"),
                Request::AgentDelete {
                    scope: "demo".into(),
                    name: "reviewer".into(),
                },
            )
            .await
        );
        assert!(
            !allowed(
                &e,
                &wearing("scope-editor"),
                Request::AgentDelete {
                    scope: "other".into(),
                    name: "reviewer".into(),
                },
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
                    reason: None,
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
                    reason: None,
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

    // -- intake (`#172`): its own grants, never `task.create`/`task.edit` --

    fn intake_assessment(routed: &str) -> Assessment {
        Assessment {
            axes: Vec::new(),
            category: "bugfix".into(),
            impact: Level::Medium,
            urgency: Level::Medium,
            complexity: 3,
            estimate: None,
            routing: Routing { scope: routed.into(), ..Default::default() },
            summary: String::new(),
            questions: Vec::new(),
            split: Vec::new(),
            duplicates: Vec::new(),
            checks: Vec::new(),
            areas: Vec::new(),
        }
    }

    fn ready(run: bool) -> Decision {
        Decision::Ready { run }
    }

    fn needs_info() -> Decision {
        Decision::NeedsInfo { questions: Vec::new() }
    }

    fn wontfix() -> Decision {
        Decision::Wontfix { reason: WontfixReason::Invalid, evidence: "not ours".into(), duplicate_of: None }
    }

    fn split() -> Decision {
        Decision::Split { parts: Vec::new() }
    }

    #[tokio::test]
    async fn intake_add_needs_its_own_grant_and_follows_task_creates_scope_rule() {
        let e = engine();
        let new = |scope: &str| {
            Request::IntakeAdd(NewIntake { title: "x".into(), scope: Some(scope.into()), ..Default::default() })
        };
        assert!(!allowed(&e, &worker("w"), new("demo")).await, "worker never had task.create, so never intake.add");
        assert!(allowed(&e, &foreman(), new("demo")).await);
        assert!(!allowed(&e, &foreman(), new("other")).await, "a foreman's authority stops at its scope");
        assert!(allowed(&e, &wearing("triager"), new("demo")).await);
        assert!(!allowed(&e, &wearing("triager"), new("other")).await);
    }

    #[tokio::test]
    async fn intake_info_needs_its_own_grant_and_reach_over_the_item() {
        let e = engine();
        task_in(&e, "mine", "demo", "w").await;
        task_in(&e, "not_mine", "demo", "someone").await;
        task_in(&e, "far", "other", "anyone").await;
        let info = |id: &str| Request::IntakeInfo { id: id.into(), text: "more".into() };

        // `worker` never held `task.create`, so it never held `intake.info`
        // either -- own reach over the item is not enough without the grant.
        assert!(!allowed(&e, &worker("w"), info("mine")).await);
        assert!(!allowed(&e, &worker("w"), info("not_mine")).await);
        assert!(allowed(&e, &foreman(), info("not_mine")).await, "scope reach covers everything in demo");
        assert!(!allowed(&e, &foreman(), info("far")).await, "out of the foreman's scope");
        assert!(allowed(&e, &wearing("triager"), info("not_mine")).await);
        assert!(!allowed(&e, &wearing("triager"), info("far")).await);
    }

    #[tokio::test]
    async fn intake_triage_needs_its_own_grant_and_reach_over_the_item() {
        let e = engine();
        task_in(&e, "mine", "demo", "w").await;
        task_in(&e, "not_mine", "demo", "someone").await;
        task_in(&e, "far", "other", "anyone").await;
        let triage = |id: &str| Request::IntakeTriage { id: id.into(), agent: None };

        // `worker` never held `task.create`, so it never held `intake.triage`
        // either -- own reach over the item is not enough without the grant.
        assert!(!allowed(&e, &worker("w"), triage("mine")).await);
        assert!(allowed(&e, &foreman(), triage("not_mine")).await);
        assert!(!allowed(&e, &foreman(), triage("far")).await, "out of the foreman's scope");
        assert!(allowed(&e, &wearing("triager"), triage("not_mine")).await);
        assert!(!allowed(&e, &wearing("triager"), triage("far")).await);
    }

    #[tokio::test]
    async fn intake_assess_needs_its_own_grant_reach_over_the_item_and_the_routes_own_scope() {
        let e = engine();
        task_in(&e, "mine", "demo", "w").await;
        task_in(&e, "not_mine", "demo", "someone").await;
        task_in(&e, "far", "other", "anyone").await;
        let assess = |id: &str, routed: &str| Request::IntakeAssess {
            id: id.into(),
            assessment: intake_assessment(routed),
            decide: false,
        };

        // What `task.edit` used to give a worker for intake.
        assert!(allowed(&e, &worker("w"), assess("mine", "demo")).await);
        assert!(
            !allowed(&e, &worker("w"), assess("not_mine", "demo")).await,
            "not assigned to it, and it holds no triage run of its own"
        );
        assert!(allowed(&e, &foreman(), assess("not_mine", "demo")).await, "scope reach covers everything in demo");
        assert!(!allowed(&e, &foreman(), assess("far", "demo")).await, "the item is out of the foreman's scope");
        assert!(!allowed(&e, &foreman(), assess("not_mine", "other")).await, "routing it out of the foreman's own scope");
        assert!(allowed(&e, &wearing("triager"), assess("not_mine", "demo")).await);
        assert!(!allowed(&e, &wearing("triager"), assess("far", "demo")).await);
        assert!(!allowed(&e, &wearing("triager"), assess("not_mine", "other")).await);
    }

    #[tokio::test]
    async fn intake_decide_needs_its_own_grant_and_reach_and_a_run_release_also_needs_task_run() {
        let e = engine();
        task_in(&e, "mine", "demo", "w").await;
        task_in(&e, "not_mine", "demo", "someone").await;
        task_in(&e, "far", "other", "anyone").await;
        let decide = |id: &str, decision: Decision| Request::IntakeDecide { id: id.into(), decision };

        // worker: own reach, and intake.decide is what task.edit used to give
        // it -- except releasing with `run: true`, the one worker behaviour
        // this narrowing deliberately changes (it needs `task.run` now).
        assert!(allowed(&e, &worker("w"), decide("mine", ready(false))).await);
        assert!(!allowed(&e, &worker("w"), decide("not_mine", ready(false))).await, "not assigned to it");
        assert!(!allowed(&e, &worker("w"), decide("mine", ready(true))).await, "no task.run");
        assert!(allowed(&e, &worker("w"), decide("mine", needs_info())).await);
        assert!(allowed(&e, &worker("w"), decide("mine", wontfix())).await);
        assert!(allowed(&e, &worker("w"), decide("mine", split())).await);

        // foreman: scope reach and every grant, task.run included.
        assert!(allowed(&e, &foreman(), decide("not_mine", ready(false))).await);
        assert!(allowed(&e, &foreman(), decide("not_mine", ready(true))).await);
        assert!(!allowed(&e, &foreman(), decide("far", ready(false))).await, "out of scope");

        // triager: scope reach and all five intake grants, but it runs
        // nothing -- `Ready { run: true }` needs `task.run`, which it lacks.
        assert!(allowed(&e, &wearing("triager"), decide("not_mine", ready(false))).await);
        assert!(!allowed(&e, &wearing("triager"), decide("not_mine", ready(true))).await, "the triager runs nothing");
        assert!(allowed(&e, &wearing("triager"), decide("not_mine", needs_info())).await);
        assert!(allowed(&e, &wearing("triager"), decide("not_mine", wontfix())).await);
        assert!(allowed(&e, &wearing("triager"), decide("not_mine", split())).await);
        assert!(!allowed(&e, &wearing("triager"), decide("far", ready(false))).await, "out of scope");
    }

    #[tokio::test]
    async fn a_worker_deciding_ready_with_run_is_denied_and_names_task_run() {
        let e = engine();
        task_in(&e, "mine", "demo", "w").await;
        let why = e
            .authorize(&worker("w"), &Request::IntakeDecide { id: "mine".into(), decision: ready(true) })
            .await
            .unwrap_err()
            .to_string();
        assert!(why.contains("task.run"), "{why}");
    }

    #[tokio::test]
    async fn a_triager_may_do_nothing_but_coordinate_intake_in_its_own_scope() {
        let e = engine();
        task_in(&e, "t", "demo", "somebody").await;
        let triager = wearing("triager");

        assert!(!allowed(&e, &triager, Request::TaskCreate(NewTask { scope: Some("demo".into()), ..Default::default() })).await);
        assert!(
            !allowed(&e, &triager, Request::TaskUpdate { id: "t".into(), patch: titled("renamed"), reason: None }).await
        );
        assert!(!allowed(&e, &triager, Request::TaskRun { override_wait: false, id: "t".into(), reason: None, continue_run: false }).await);
        assert!(
            !allowed(
                &e,
                &triager,
                Request::TaskClose { id: "t".into(), reason: CloseReason::NotPlanned, duplicate_of: None, note: None }
            )
            .await
        );
        assert!(!allowed(&e, &triager, Request::TaskReport { id: "t".into(), report: report() }).await);
        assert!(
            !allowed(&e, &triager, Request::AgentConfigure { scope: "demo".into(), agent: declaration() }).await
        );
        assert!(!allowed(&e, &triager, Request::AgentStart { scope: "demo".into(), name: "x".into() }).await);
        assert!(!allowed(&e, &triager, Request::WorkflowStart { id: "wf".into(), inputs: Default::default() }).await);

        // And intake in a sibling scope is exactly as far out of reach.
        task_in(&e, "elsewhere", "other", "anybody").await;
        assert!(!allowed(&e, &triager, Request::IntakeInfo { id: "elsewhere".into(), text: "x".into() }).await);
        assert!(!allowed(&e, &triager, Request::IntakeTriage { id: "elsewhere".into(), agent: None }).await);
        assert!(
            !allowed(
                &e,
                &triager,
                Request::IntakeAssess { id: "elsewhere".into(), assessment: intake_assessment("other"), decide: false }
            )
            .await
        );
        assert!(
            !allowed(&e, &triager, Request::IntakeDecide { id: "elsewhere".into(), decision: ready(false) }).await
        );
        assert!(!allowed(&e, &triager, Request::IntakeAdd(NewIntake { title: "x".into(), scope: Some("other".into()), ..Default::default() })).await);
    }

    #[tokio::test]
    async fn intake_publish_needs_its_own_grant_named_exactly_and_reach_over_the_item() {
        let e = engine();
        task_in(&e, "mine", "demo", "w").await;
        task_in(&e, "far", "other", "anyone").await;
        let publish = |id: &str| Request::IntakePublish { id: id.into() };

        // Nobody gets it by accident: not a worker (never held `task.create`,
        // so never `intake.publish` either), not `foreman` (otherwise every
        // grant), not `triager` (coordinates triage, never posts outward).
        assert!(!allowed(&e, &worker("w"), publish("mine")).await);
        assert!(!allowed(&e, &foreman(), publish("mine")).await, "foreman holds every grant but this one (#171)");
        assert!(!allowed(&e, &wearing("triager"), publish("mine")).await);

        // A role written `*` or `intake.*` does not pick it up either --
        // `Grant::expand` excludes it from both wildcards.
        let starred = engine_with_roles("roles:\n  starred:\n    grants: ['*']\n    reach: scope\n");
        task_in(&starred, "s", "demo", "anyone").await;
        assert!(!allowed(&starred, &wearing("starred"), Request::IntakePublish { id: "s".into() }).await, "`*` excludes it");

        let dotted = engine_with_roles("roles:\n  dotted:\n    grants: ['intake.*']\n    reach: scope\n");
        task_in(&dotted, "d", "demo", "anyone").await;
        assert!(!allowed(&dotted, &wearing("dotted"), Request::IntakePublish { id: "d".into() }).await, "`intake.*` excludes it");

        // Named exactly, it works, and stops at the scope boundary like any
        // other grant.
        let named = engine_with_roles("roles:\n  publisher:\n    grants: [intake.publish]\n    reach: scope\n");
        task_in(&named, "n", "demo", "anyone").await;
        task_in(&named, "far", "other", "anyone").await;
        assert!(allowed(&named, &wearing("publisher"), publish("n")).await);
        assert!(!allowed(&named, &wearing("publisher"), publish("far")).await, "out of the role's scope");

        // The owner always passes -- publishing itself is the approval.
        assert!(allowed(&e, &Caller::Owner, publish("far")).await);
    }

    // -- intake security fast lane (`#170`) ---------------------------------

    #[tokio::test]
    async fn intake_flag_security_needs_intake_assess_and_reach_over_the_item() {
        let e = engine();
        task_in(&e, "mine", "demo", "w").await;
        task_in(&e, "not_mine", "demo", "someone").await;
        task_in(&e, "far", "other", "anyone").await;
        let flag = |id: &str| Request::IntakeFlagSecurity { id: id.into(), reason: "looks exploitable".into() };

        // Flagging rides `intake.assess` -- the same grant, not a sixth one.
        assert!(allowed(&e, &worker("w"), flag("mine")).await);
        assert!(!allowed(&e, &worker("w"), flag("not_mine")).await, "not assigned to it");
        assert!(allowed(&e, &foreman(), flag("not_mine")).await, "scope reach covers everything in demo");
        assert!(!allowed(&e, &foreman(), flag("far")).await, "out of the foreman's scope");
        assert!(allowed(&e, &wearing("triager"), flag("not_mine")).await);
        assert!(!allowed(&e, &wearing("triager"), flag("far")).await);
    }

    #[tokio::test]
    async fn intake_security_confirm_or_dismiss_is_the_owners_alone() {
        let e = engine();
        task_in(&e, "mine", "demo", "w").await;
        let confirm = || Request::IntakeSecurity {
            id: "mine".into(),
            verdict: factory_core::intake::SecurityVerdict::Confirm,
            evidence: String::new(),
        };
        // Neither a worker in reach, a foreman with scope reach, nor the
        // triager -- whatever grants a role holds -- may decide. Only the
        // owner, no token at all, can.
        assert!(!allowed(&e, &worker("w"), confirm()).await);
        assert!(!allowed(&e, &foreman(), confirm()).await);
        assert!(!allowed(&e, &wearing("triager"), confirm()).await);
        assert!(allowed(&e, &Caller::Owner, confirm()).await);
    }

    #[tokio::test]
    async fn intake_security_reports_needs_no_grant() {
        let e = engine();
        assert!(allowed(&e, &worker("w"), Request::IntakeSecurityReports { scope: None }).await);
        assert!(allowed(&e, &Caller::Owner, Request::IntakeSecurityReports { scope: None }).await);
    }

    /// The CRA Art. 14 reporting clock (`#157`, phase 1) is a read, open the
    /// same way `Request::Policy`/`Request::IntakeSecurityReports` are.
    #[tokio::test]
    async fn policy_clock_needs_no_grant() {
        let e = engine();
        assert!(allowed(&e, &worker("w"), Request::PolicyClock { scope: None }).await);
        assert!(allowed(&e, &Caller::Owner, Request::PolicyClock { scope: None }).await);
    }

    /// A clock submission (`Request::PolicyAttest.clock`) is still an
    /// ordinary `policy.attest` write underneath -- carrying one must never
    /// let a `reach: own` role bypass the scope-reach rule an ordinary
    /// attestation is already checked against.
    #[tokio::test]
    async fn policy_attest_with_a_clock_mark_is_still_refused_to_an_own_reach_agent() {
        let e = engine_with_roles_and_root_scope(
            "demo",
            "roles:\n  attestor:\n    grants: [policy.attest]\n    reach: own\n",
        );
        let caller = Caller::Agent {
            scope: "demo".into(),
            name: "w".into(),
            role: Role::new("attestor"),
            run_id: None,
        };
        let attest = Request::PolicyAttest {
            control: "cra/art-14".parse().unwrap(),
            scope: "demo".into(),
            evidence: "https://example.com/notice".into(),
            note: None,
            expires_at: Utc::now() + chrono::Duration::days(30),
            corrective: None,
            clock: Some(factory_core::reporting_clock::ClockMark {
                item: factory_core::reporting_clock::ClockItemRef::Finding {
                    scope: "demo".into(),
                    vulnerability: "CVE-2026-1".into(),
                },
                deadline: factory_core::reporting_clock::ClockDeadlineKind::EarlyWarning,
            }),
        };
        assert!(!allowed(&e, &caller, attest).await);
    }

    // -- roles inherited down the scope tree -------------------------------

    /// A small tree whose scope names deliberately do not match their paths:
    /// inheritance must follow `Scope.path`, and a name that reads like a child
    /// must not be one.
    ///
    ///     .                    company
    ///     projects             engineering   defines reviewer (own), lead (scope)
    ///     projects/demo        demo-app
    ///     projects/demo/inner  demo-app/inner
    ///     projects/sibling     sibling
    ///     elsewhere            engineering/outsider
    fn engine_tree() -> Arc<Engine> {
        let at = |name: &str, path: &str, roles: &str| {
            let mut yaml = format!("id: {name}-id\nname: {name}\n");
            if !roles.is_empty() {
                yaml.push_str(&format!("roles:\n{roles}"));
            }
            let mut scope: factory_core::config::Scope = serde_yaml_ng::from_str(&yaml).unwrap();
            scope.path = PathBuf::from(path);
            scope
        };
        let mut config: Config =
            serde_yaml_ng::from_str("instance:\n  id: i\n  name: test\n").unwrap();
        config.scopes = vec![
            at("company", ".", ""),
            at(
                "engineering",
                "projects",
                "  reviewer:\n    grants: [task.edit, task.report]\n    reach: own\n  lead:\n    grants: [task.run, task.cancel]\n    reach: scope\n",
            ),
            at("demo-app", "projects/demo", ""),
            at("demo-app/inner", "projects/demo/inner", ""),
            at("sibling", "projects/sibling", ""),
            at("engineering/outsider", "elsewhere", ""),
        ];
        config.validate().unwrap();
        Arc::new(Engine::new(
            Factory {
                root: PathBuf::from("/tmp/factory-access-tree-test"),
                config,
            },
            Registry::with_builtins(),
            Arc::new(SqliteStore::in_memory().unwrap()),
            PathBuf::from("factory"),
            vec![],
        ))
    }

    fn in_scope(scope: &str, name: &str, role: &str) -> Caller {
        Caller::Agent {
            scope: scope.into(),
            name: name.into(),
            role: Role::new(role),
            run_id: None,
        }
    }

    #[tokio::test]
    async fn an_agent_below_the_defining_scope_may_use_the_role_it_inherits() {
        let e = engine_tree();
        task_in(&e, "mine", "demo-app", "critic").await;
        let critic = in_scope("demo-app", "critic", "reviewer");
        assert!(
            allowed(&e, &critic, Request::TaskReport { id: "mine".into(), report: report() }).await,
            "demo-app is below projects on disk, so it has engineering's reviewer"
        );
        assert!(!allowed(&e, &critic, Request::TaskRun { override_wait: false, id: "mine".into(), reason: None, continue_run: false }).await);
    }

    #[tokio::test]
    async fn a_role_from_outside_the_chain_is_no_role_and_the_refusal_lists_what_there_is() {
        let e = engine_tree();
        task_in(&e, "theirs", "engineering/outsider", "critic").await;
        let outsider = in_scope("engineering/outsider", "critic", "reviewer");
        let err = e
            .authorize(&outsider, &Request::TaskReport { id: "theirs".into(), report: report() })
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("reviewer"), "{err}");
        assert!(
            err.contains("available in engineering/outsider are: foreman, triager, worker"),
            "a name that reads like engineering's child is not below it: {err}"
        );
    }

    #[tokio::test]
    async fn an_inherited_scope_reach_role_still_stops_at_the_agents_own_scope() {
        let e = engine_tree();
        for (id, scope) in [
            ("here", "demo-app"),
            ("parent", "engineering"),
            ("sibling", "sibling"),
            ("child", "demo-app/inner"),
        ] {
            task_in(&e, id, scope, "somebody").await;
        }
        let lead = in_scope("demo-app", "boss", "lead");
        assert!(allowed(&e, &lead, Request::TaskRun { override_wait: false, id: "here".into(), reason: None, continue_run: false }).await);
        for elsewhere in ["parent", "sibling", "child"] {
            assert!(
                !allowed(&e, &lead, Request::TaskRun { override_wait: false, id: elsewhere.into(), reason: None, continue_run: false }).await,
                "inheriting lead from projects gives no authority over {elsewhere}"
            );
            assert!(!allowed(&e, &lead, Request::TaskCancel { id: elsewhere.into(), reason: None, run: None }).await);
        }
    }

    #[tokio::test]
    async fn changing_role_definitions_is_the_owners_alone() {
        let e = engine_with_roles("roles:\n  everything:\n    grants: ['*']\n    reach: scope\n");
        let requests = [
            Request::RoleDefine {
                scope: "demo".into(),
                name: "runner".into(),
                role: factory_core::role::RoleSpec::default(),
                replace: false,
            },
            Request::RoleDelete {
                scope: "demo".into(),
                name: "runner".into(),
            },
        ];
        for request in requests {
            assert!(allowed(&e, &Caller::Owner, request.clone()).await);
            // Not a foreman, and not a role handed every grant there is: an
            // agent that can rewrite what a role allows can widen its own.
            assert!(!allowed(&e, &foreman(), request.clone()).await);
            assert!(!allowed(&e, &wearing("everything"), request.clone()).await);
            assert!(!allowed(&e, &worker("w"), request).await);
        }
        assert!(
            allowed(&e, &worker("w"), Request::RoleList { scope: Some("demo".into()) }).await,
            "reading the roles is reading the board"
        );
    }

    #[tokio::test]
    async fn a_role_is_given_only_where_its_scope_can_see_it() {
        let e = engine_tree();
        for scope in ["demo-app", "engineering/outsider"] {
            let agent = AgentSession::new(scope, "watcher", "pi", "herdr", Lifetime::Permanent, Role::worker());
            e.store.put_agent(&agent).await.unwrap();
        }
        let given = e
            .set_agent_role("demo-app/watcher", Some(Role::new("reviewer")))
            .await
            .unwrap();
        assert_eq!(given.role, Role::new("reviewer"));

        let err = e
            .set_agent_role("engineering/outsider/watcher", Some(Role::new("reviewer")))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("engineering/outsider"), "{err}");
        assert!(err.contains("foreman, triager, worker"), "it lists the roles that scope has: {err}");
    }

    #[tokio::test]
    async fn roles_for_resolves_down_the_path_tree_and_never_up_or_sideways() {
        let e = engine_tree();
        assert!(e.roles_for("demo-app/inner").contains(&Role::new("lead")));
        assert!(!e.roles_for("engineering/outsider").contains(&Role::new("lead")));
        assert!(!e.roles_for("company").contains(&Role::new("lead")), "never up");
    }

    // -- dashboard_for --------------------------------------------------------

    /// The same topology `engine_tree` uses for roles (`#159`): `engineering`
    /// (path `projects`) overrides the dashboard, `demo-app` (path
    /// `projects/demo`, below it) inherits, and `sibling`/`engineering/outsider`
    /// (named like a child of `engineering` but not below `projects` on
    /// disk) do not.
    fn engine_dashboard_tree() -> Arc<Engine> {
        let at = |name: &str, path: &str, dashboard: &str| {
            let mut yaml = format!("id: {name}-id\nname: {name}\n");
            if !dashboard.is_empty() {
                yaml.push_str(&format!("dashboard:\n{dashboard}"));
            }
            let mut scope: factory_core::config::Scope = serde_yaml_ng::from_str(&yaml).unwrap();
            scope.path = PathBuf::from(path);
            scope
        };
        let mut config: Config = serde_yaml_ng::from_str("instance:\n  id: i\n  name: test\n").unwrap();
        config.scopes = vec![
            at("company", ".", ""),
            at(
                "engineering",
                "projects",
                "  tiles:\n    - { metric: throughput_week, size: s }\n",
            ),
            at("demo-app", "projects/demo", ""),
            at("sibling", "projects/sibling", ""),
            at("engineering/outsider", "elsewhere", ""),
        ];
        Arc::new(Engine::new(
            Factory {
                root: PathBuf::from("/tmp/factory-access-dashboard-tree-test"),
                config,
            },
            Registry::with_builtins(),
            Arc::new(SqliteStore::in_memory().unwrap()),
            PathBuf::from("factory"),
            vec![],
        ))
    }

    #[tokio::test]
    async fn dashboard_for_resolves_down_the_path_tree_and_never_up_or_sideways() {
        let e = engine_dashboard_tree();
        let (tiles, source) = e.dashboard_for(Some("demo-app")).unwrap();
        assert_eq!(source, Some("engineering".to_string()), "projects/demo is below projects on disk");
        assert_eq!(tiles.unwrap().tiles[0].metric.as_ref().unwrap().as_str(), "throughput_week");

        // `sibling` (path `projects/sibling`) is below `projects` too, so it
        // inherits the same override.
        let (tiles, source) = e.dashboard_for(Some("sibling")).unwrap();
        assert_eq!(source, Some("engineering".to_string()));
        assert!(tiles.is_some());

        // `engineering/outsider` reads like a child of `engineering` by name,
        // but its path (`elsewhere`) is not below `projects` at all. This
        // fixture names no root `dashboard:` either, so it falls all the way
        // through to the built-in default: `None` on both sides, never the
        // magic string `"default"`.
        let (tiles, source) = e.dashboard_for(Some("engineering/outsider")).unwrap();
        assert!(
            tiles.is_none(),
            "engineering/outsider is not below projects by path, whatever its name says"
        );
        assert_eq!(source, None, "never up or sideways");
    }

    #[tokio::test]
    async fn dashboard_for_an_unknown_scope_is_refused() {
        let e = engine_dashboard_tree();
        let err = e.dashboard_for(Some("nope")).unwrap_err().to_string();
        assert!(err.contains("nope"), "{err}");
    }

    #[tokio::test]
    async fn reading_the_dashboard_needs_no_grant() {
        let e = engine();
        assert!(allowed(&e, &worker("w"), Request::Dashboard { scope: None }).await);
        assert!(allowed(&e, &Caller::Owner, Request::Dashboard { scope: None }).await);
    }

    fn a_tile() -> factory_core::dashboard::Tile {
        serde_yaml_ng::from_str("view: kpis\nsize: s\n").unwrap()
    }

    #[tokio::test]
    async fn setting_or_resetting_the_dashboard_needs_dashboard_edit_and_scope_reach() {
        let e = engine_with_roles(
            "roles:\n  self-editor:\n    grants: [dashboard.edit]\n    reach: own\n  scope-editor:\n    grants: [dashboard.edit]\n    reach: scope\n",
        );
        let set = |scope: &str| Request::DashboardSet { scope: scope.into(), tiles: vec![a_tile()] };
        let reset = |scope: &str| Request::DashboardReset { scope: scope.into() };

        // Own reach cannot edit a dashboard at all -- there is no "its own"
        // dashboard the way there is its own task.
        assert!(!allowed(&e, &wearing("self-editor"), set("demo")).await);
        assert!(!allowed(&e, &wearing("self-editor"), reset("demo")).await);

        // Scope reach can, and stops at the caller's own scope.
        assert!(allowed(&e, &wearing("scope-editor"), set("demo")).await);
        assert!(allowed(&e, &wearing("scope-editor"), reset("demo")).await);
        assert!(!allowed(&e, &wearing("scope-editor"), set("other")).await);
        assert!(!allowed(&e, &wearing("scope-editor"), reset("other")).await);

        // Neither a worker, nor a custom role that simply does not hold the
        // grant, gets it by accident.
        assert!(!allowed(&e, &worker("w"), set("demo")).await);
        let plain = engine_with_roles("roles:\n  plain:\n    grants: [task.edit]\n    reach: scope\n");
        assert!(!allowed(&plain, &wearing("plain"), set("demo")).await);

        // `foreman` holds every grant, `dashboard.edit` included, and a
        // foreman outside the target scope's reach is still denied.
        assert!(allowed(&e, &foreman(), set("demo")).await);
        assert!(!allowed(&e, &foreman(), set("other")).await);

        // The owner always passes.
        assert!(allowed(&e, &Caller::Owner, set("other")).await);
        assert!(allowed(&e, &Caller::Owner, reset("other")).await);
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
