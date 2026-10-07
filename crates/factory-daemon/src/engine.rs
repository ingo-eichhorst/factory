//! The core. Everything an interface can ask for arrives here as a `Request`
//! and leaves as a `Response`; the interfaces themselves hold no logic.

use chrono::Utc;
use factory_core::adapter::agent::LaunchSpec;
use factory_core::adapter::runtime::{
    RuntimeConnectionDiagnostic, RuntimeStatus, Screen, StatusReport, StatusSource,
};
use factory_core::adapter::TaskStore;
#[cfg(test)]
use factory_core::adapter::{Agent, AgentRuntime};
use factory_core::config::{Factory, Sandbox, ScopeAgent, SHELL_HARNESS};
use factory_core::error::{FactoryError, Result};
use factory_core::event::{Event, EventBus};
use factory_core::protocol::{
    AgentActivity, AgentView, CapacityRow, CredentialRow, DaemonFacts, Envelope, Payload, ProviderAgent,
    ProviderRow, ProviderRun, ProviderWindow, Request, Response, RuntimeConnectionView, SandboxRow,
    ScopeView, StatusInfo, StoreFacts, UnassignedAgent,
};
use factory_core::agent::{AgentSession, AgentState};
use factory_core::role::{Role, Roles};
use factory_core::run::{FailKind, Run, Trigger};
use factory_core::task::{
    NewTask, Task, TaskEntry, TaskFilter, TaskPatch, TaskStatus,
};
use factory_plugins::registry::Registry;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::schedule;
use factory_agents::dispatch::Environments;
#[cfg(test)]
use factory_core::adapter::runtime::StartRequest;
#[cfg(test)]
use factory_core::adapter::agent::{AgentContext, UpstreamOutput};
#[cfg(test)]
use factory_core::run::{BlockSource, NewRun, RunPatch};
#[cfg(test)]
use factory_core::task::{PendingRetry, RetryPolicy, TaskReport};
use crate::worktree;

/// Request routing, one file per level. A child of this module so the arms keep
/// the access they had when they were one match in this file.
#[path = "router/mod.rs"]
mod router;

/// The Agents page and the runtime connections page: composition of the roster with
/// L4's active runs and the adapter registry. A child module for the same reason as `router`.
#[path = "scope_views.rs"]
mod scope_views;

/// When a run became due, and the schedule slot behind it when there is
/// one -- carried from whoever decided to start a run to the `NewRun` that
/// records it. See `Run::queued_at` for what "due" means per trigger.
#[derive(Debug, Clone, Copy)]
pub struct Due {
    pub queued_at: chrono::DateTime<Utc>,
    pub scheduled_for: Option<chrono::DateTime<Utc>>,
}

impl Due {
    /// Due the moment it was asked for.
    pub fn now() -> Self {
        Self { queued_at: Utc::now(), scheduled_for: None }
    }

    /// A schedule's own slot, queued at the slot itself. `Engine::due_for`
    /// starts from this and moves `queued_at` past any time the slot could
    /// not have been dispatched in.
    pub fn slot(slot: chrono::DateTime<Utc>) -> Self {
        Self { queued_at: slot, scheduled_for: Some(slot) }
    }

    /// A queued retry: due when its backoff ran out. Not a slot -- the
    /// schedule's own firing it stands in front of is a different one.
    pub fn retry(at: chrono::DateTime<Utc>) -> Self {
        Self { queued_at: at, scheduled_for: None }
    }
}

/// What the capacity-release worker (`Engine::spawn_capacity_release_worker`)
/// was sent (`#179`). One channel, one consumer, processed one event at a
/// time: `Release` and `Sweep` both admit from `waiting_tasks()`, and
/// `dispatch` has no guard against two overlapping admission attempts for
/// the same waiting task -- keeping them off separate concurrent tasks is
/// what closes that race, not anything inside `dispatch` itself.
pub(crate) enum CapacityEvent {
    /// A run ending: try to admit whatever is waiting on this (scope, agent).
    Release(String, String),
    /// The scheduler tick's own sweep: try every waiting task, for a
    /// restart, a raised limit, or a `Release` this channel dropped.
    Sweep,
}

/// Where `place_run` should put a run's working directory -- pulled out so
/// the decision of *which* is `resolve_continue`'s alone, and `place_run`
/// itself only ever carries one out (`#178`).
pub(crate) enum Workspace {
    /// Today's behaviour: `git worktree add` a new one, or work in the scope
    /// directly when the task has no worktree of its own. The owner may reuse
    /// a task-lifetime workspace even though its conversation starts fresh.
    Fresh,
    /// `factory task run --continue`, having confirmed the previous run's
    /// worktree is still one of the scope's registered worktrees
    /// (`worktree::is_registered`): reuse it exactly as it stands, on its
    /// own branch, rather than cutting a new one.
    Reuse { path: PathBuf, branch: String },
}

/// What `resolve_continue` decided about a `--continue` request: resume, with
/// everything `dispatch` needs to launch into the same conversation, or fall
/// back to a fresh session with the exact reason to journal (`#178`).
/// `pub(crate)`: `suggestions.rs`'s `ask_agent` (`#275`) asks the same
/// question before deciding whether asking at all is honest.
pub(crate) enum ContinueOutcome {
    Resume(ResumePlan),
    Fresh { reason: String },
}

pub(crate) struct ResumePlan {
    /// The harness's own session id being picked back up.
    pub(crate) session_id: String,
    /// `Agent::resume_spec`'s args, prepended to the launch.
    pub(crate) resume_args: Vec<String>,
    /// `Some` only when the task uses a worktree of its own and the previous
    /// run's is still there to reuse.
    pub(crate) workspace: Option<(PathBuf, String)>,
    /// `#274`: the session id was read off the preserved sandbox
    /// conversation, because the run itself never recorded one.
    pub(crate) from_preserved: bool,
    /// `#274`: `Some` only for a sandboxed claude-code run whose preserved
    /// conversation `resolve_continue` matched against this dispatch --
    /// `preserved_dir` for the task, which `prepare_sandbox` uploads into
    /// the new sandbox before launch. `None` for every other resume, and
    /// for a sandboxed one that already fell back to fresh.
    pub(crate) sandbox_restore: Option<PathBuf>,
}

/// Whether a preserved conversation tree really holds `<session_id>.jsonl`
/// (`projects/<cwd-dir>/<id>.jsonl`, the depth the preserve step reads).
pub(crate) fn preserved_conversation_has(preserved_dir: &Path, session_id: &str) -> bool {
    if session_id.is_empty() || session_id.contains(['/', '\\']) || session_id.contains("..") {
        return false;
    }
    let Ok(dirs) = std::fs::read_dir(preserved_dir.join("projects")) else { return false };
    dirs.flatten().any(|dir| dir.path().join(format!("{session_id}.jsonl")).is_file())
}

/// Why a `schedule_skipped` entry's slots passed -- `data.reason` on the
/// entry. Three causes produce the same state (a past `next_run_at` on a
/// pending task), so the entry says which one it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SkipReason {
    /// The task's previous run was still going when the first skipped slot
    /// came round -- `due()` only fires a `pending` task.
    StillActive,
    /// Held on a declared `max_sessions` limit (`#179`): the run this streak
    /// eventually got was already queued for it, just not started yet, when
    /// the slot came round. A task with no capacity limit never sees this --
    /// its run either starts at once or `StillActive` already explains it.
    Queued,
    /// Nothing was running: the daemon was not there to fire it.
    NotRunning,
}

/// When an attempt due at `due` could first have been dispatched: not
/// before the daemon was up to dispatch it (`booted_at`), and not before
/// the task's previous run -- if it was still going at `due` -- ended, since
/// only a pending task fires. The time between `due` and this is lateness
/// (`started_at - scheduled_for` measures it for a slot), not queue wait.
/// A host asleep with the daemon still running leaves no record, so that
/// stretch still reads as queue wait.
pub(crate) fn dispatchable_from(
    due: chrono::DateTime<Utc>,
    booted_at: chrono::DateTime<Utc>,
    previous: Option<&Run>,
) -> chrono::DateTime<Utc> {
    let mut from = due.max(booted_at);
    if let Some(run) = previous {
        if let Some(end) = run.ended_at.filter(|end| run.started_at <= due && *end > due) {
            from = from.max(end);
        }
    }
    from
}

/// `Engine::skip_reason`'s rule, apart from the store: the newest run was
/// open at `first` (started by then, not yet ended), already queued for but
/// not yet started at `first` (`#179`: it was waiting for a capacity slot),
/// or neither.
pub(crate) fn skip_reason_of(newest: Option<&Run>, first: chrono::DateTime<Utc>) -> SkipReason {
    match newest {
        Some(run) if run.started_at <= first && run.ended_at.is_none_or(|end| end >= first) => {
            SkipReason::StillActive
        }
        Some(run) if run.queued_at.is_some_and(|q| q <= first) && first < run.started_at => {
            SkipReason::Queued
        }
        _ => SkipReason::NotRunning,
    }
}

impl SkipReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::StillActive => "still_active",
            Self::Queued => "queued",
            Self::NotRunning => "not_running",
        }
    }

    pub(crate) fn describe(self) -> &'static str {
        match self {
            Self::StillActive => "the previous run was still going",
            Self::Queued => "it was already queued, waiting for a capacity slot",
            Self::NotRunning => "nothing was running it: the daemon was down, asleep or behind",
        }
    }
}

/// How long a scope's worktree capability is trusted for. Shorter than
/// `site::WALK_TTL`: a person who just ran `git init` in a scope to make the
/// checkbox available should not have to wait five minutes to see it, and the
/// answer is two `git` calls rather than a directory walk.
const CAPABILITY_TTL: Duration = Duration::from_secs(60);

/// Put declaration-specific arguments after the adapter's defaults. Harnesses
/// generally let the last occurrence of a flag win, so this ordering lets one
/// scope override an adapter default as well as add to it.
pub(crate) fn append_declared_args(launch: &mut LaunchSpec, declaration: Option<&ScopeAgent>) {
    if let Some(declaration) = declaration {
        launch.args.extend(declaration.args.iter().cloned());
    }
}

pub struct Engine {
    pub(crate) shared: crate::state::Shared,
    pub(crate) l1: crate::state::L1State,
    pub(crate) l2: crate::state::L2State,
    pub(crate) l3: crate::state::L3State,
    pub(crate) l4: crate::state::L4State,
    pub(crate) l5: crate::state::L5State,
    pub(crate) l6: crate::state::L6State,
}

impl Engine {
    pub fn new(
        factory: Factory,
        registry: Registry,
        store: Arc<dyn TaskStore>,
        factory_bin: PathBuf,
        interfaces: Vec<String>,
    ) -> Self {
        let (bench_judge_tx, bench_judge_rx) = tokio::sync::mpsc::unbounded_channel();
        let (verify_tx, verify_rx) = tokio::sync::mpsc::unbounded_channel();
        let (capacity_release_tx, capacity_release_rx) = tokio::sync::mpsc::unbounded_channel();
        let power = crate::power::PowerAssertions::new(factory.config.daemon.power_assertion);
        let workspaces = worktree::Owner::new(factory.worktrees_dir());
        Self {
            l1: crate::state::L1State {
                backups: crate::backup::BackupStore::in_memory()
                    .expect("an in-memory backup store should open"),
                backup_busy: tokio::sync::Mutex::new(()),
                environments: crate::environments::EnvironmentStore::in_memory()
                    .expect("an in-memory environment store should open"),
                deployment_edit: tokio::sync::Mutex::new(()),
                verify_drill_skip: std::sync::Mutex::new(None),
                infrastructure_expiries: crate::renewals::store::InfrastructureExpiryStore::in_memory().expect("expiry metadata store should open"),
                renewal_declaration_cache: Default::default(),
                host_power: crate::host_power::HostPower::new(),
                power,
                promotion_lock: tokio::sync::Mutex::new(()),
            },
            l2: crate::state::L2State {
                credential_expiries: crate::renewals::store::CredentialExpiryStore::in_memory().expect("expiry metadata store should open"),
                provision: crate::provision::Provisioner::default(),
            },
            l3: crate::state::L3State {
                harness: crate::harness_health::HarnessHealth::new(),
                agents: Arc::new(crate::agent_rows::AgentRows(store.clone())),
            },
            l4: crate::state::L4State {
                store,
                workflows: crate::workflows::WorkflowStore::in_memory()
                    .expect("an in-memory workflow store should open"),
                workflow_edit: tokio::sync::Mutex::new(()),
                run_evidence: factory_process::evidence_store::RunEvidenceStore::in_memory()
                    .expect("an in-memory run evidence store should open"),
                usage_edit: tokio::sync::Mutex::new(()),
                turn_usage_pending: Default::default(),
                verifying: Default::default(),
                verify_tx,
                verify_rx: std::sync::Mutex::new(Some(verify_rx)),
                schedule_lock: tokio::sync::Mutex::new(()),
                admission_lock: tokio::sync::Mutex::new(()),
                intake_receipt_lock: tokio::sync::Mutex::new(()),
                capacity_release_tx,
                capacity_release_rx: std::sync::Mutex::new(Some(capacity_release_rx)),
                workspaces,
                run_lifecycle_locks: Default::default(),
                deployment_mirror_busy: tokio::sync::Mutex::new(()),
                seen_status: Default::default(),
                worktree_caps: Default::default(),
            },
            l5: crate::state::L5State {
                bench: crate::bench::BenchStore::in_memory()
                    .expect("an in-memory bench store should open"),
                suggestions: crate::suggestions::SuggestionStore::in_memory()
                    .expect("an in-memory suggestion store should open"),
                bench_edit: tokio::sync::Mutex::new(()),
                bench_judging: Default::default(),
                bench_judge_tx,
                bench_judge_rx: std::sync::Mutex::new(Some(bench_judge_rx)),
                dataset_locks: Default::default(),
                signpost_cache: factory_assurance::signposts::Cache::default(),
                quality_seen: Default::default(),
                quality_guide_cache: Default::default(),
                #[cfg(test)]
                guide_judgements: Default::default(),
            },
            l6: crate::state::L6State {
                policies: crate::policies::PolicyStore::in_memory()
                    .expect("an in-memory policy store should open"),
                goals: crate::goals::GoalsStore::in_memory()
                    .expect("an in-memory goals store should open"),
                renewal_alerts: crate::renewals::store::AlertStore::in_memory().expect("renewal alert store should open"),
            },
            shared: crate::state::Shared {
                factory: std::sync::RwLock::new(factory),
                configuration_edit: Default::default(),
                registry,
                bus: EventBus::default(),
                factory_bin,
                started: Instant::now(),
                booted_at: Utc::now(),
                interfaces,
                site_walks: Default::default(),
                site_memory: Default::default(),
            },
        }
    }

    /// Production replaces the in-memory test repository with the instance
    /// database. Workflow state belongs to the daemon ledger, regardless of
    /// which task-store adapter a scope selects.
    pub fn with_workflow_store(mut self, workflows: crate::workflows::WorkflowStore) -> Self {
        self.l4.workflows = workflows;
        self
    }

    /// The same, for bench runs.
    pub fn with_bench_store(mut self, bench: crate::bench::BenchStore) -> Self {
        self.l5.bench = bench;
        self
    }

    /// The same, for policy attestations.
    pub fn with_policy_store(mut self, policies: crate::policies::PolicyStore) -> Self {
        self.l6.policies = policies;
        self
    }

    /// The same, for suggestions (`#275`).
    pub fn with_suggestion_store(mut self, suggestions: crate::suggestions::SuggestionStore) -> Self {
        self.l5.suggestions = suggestions;
        self
    }

    /// Install L4's persisted run evidence without replacing L6's receipts.
    pub fn with_run_evidence_store(
        mut self,
        evidence: factory_process::evidence_store::RunEvidenceStore,
    ) -> Self {
        self.l4.run_evidence = evidence;
        self
    }

    /// The same, for goals check-ins.
    pub fn with_goals_store(mut self, goals: crate::goals::GoalsStore) -> Self {
        self.l6.goals = goals;
        self
    }

    /// The same, for deployments and health samples.
    pub fn with_environment_store(mut self, environments: crate::environments::EnvironmentStore) -> Self {
        self.l1.environments = environments;
        self
    }

    /// The same, for the backup history.
    pub fn with_backup_store(mut self, backups: crate::backup::BackupStore) -> Self {
        self.l1.backups = backups;
        self
    }

    /// A coherent configuration snapshot for one operation. A poisoned lock
    /// still contains the last value; recovering it keeps a failed request
    /// from taking the daemon down with it.
    // L5 files (bench, dependencies, suggestions) reach these four through the page until S10 gives L5 a command port down to L4;
    // an L5 file calling `.l4_service()` itself would be a new pull.
    pub(crate) fn check_run_token(&self, run: &Run, given: Option<&str>, task_id: &str) -> Result<()> {
        self.l4_service().check_run_token(run, given, task_id)
    }

    // L5 (bench, suggestions) starts runs through these two until S10 gives it a command port down to L4;
    // an L5 file calling `.l4_service()` itself would be a new pull.
    // -- reaches the run-start path makes into other levels (S9a part 2) -------
    // Kept here, on the page, so the L4 code that moved out carries no level reach of its own.

    /// What L3 says the agent's role is for this scope.
    pub(crate) async fn effective_role_for(&self, scope: &str, agent: &str) -> Role {
        self.l3_service().effective_role(scope, agent).await
    }

    /// The version L3's harness health last observed for this adapter's probe.
    pub(crate) fn harness_version_of(&self, adapter_name: &str, probe: &factory_agents::harness::HealthProbe) -> Option<String> {
        self.l3
            .harness
            .rows(&[(adapter_name.to_string(), probe.clone())], None)
            .into_iter()
            .next()
            .and_then(|row| row.version)
    }

    pub(crate) fn factory_snapshot(&self) -> Factory {
        self.shared.factory
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Every role in effect in `scope` right now.
    ///
    /// Resolved from the live snapshot on every call rather than kept beside
    /// it, so a scope-config write -- a declaration from the roster, a role
    /// from the Roles view -- holds from the next request with nothing to
    /// invalidate, and "what may this agent do" has exactly one answer. It is
    /// the only way the daemon asks: `authorize` checks against it, and the
    /// guide tells an agent what it says.
    ///
    /// Startup and every write validate the chain before it reaches the
    /// snapshot, so resolution can only fail for an instance assembled in
    /// code. Say so and carry on with the two that ship rather than taking
    /// the daemon down.
    pub fn roles_for(&self, scope: &str) -> Roles {
        self.factory_snapshot().roles_for(scope).unwrap_or_else(|e| {
            tracing::error!(scope, "{e}; falling back to the built-in roles");
            Roles::presets()
        })
    }

    /// The resolved dashboard for `scope` (or the instance root's own when
    /// `scope` is `None`): its tiles (`None` for "use the built-in
    /// default") and where the layout came from -- `Factory::dashboard_for`,
    /// resolved from the live snapshot on every call for the same reason
    /// `roles_for` is: a scope-config write holds from the next request with
    /// nothing to invalidate. Unlike `roles_for`, an unknown scope is
    /// propagated as an error rather than defaulted -- see
    /// `Factory::dashboard_for`'s own doc comment for why.
    pub fn dashboard_for(
        &self,
        scope: Option<&str>,
    ) -> Result<(Option<factory_core::dashboard::DashboardConfig>, Option<String>)> {
        self.factory_snapshot().dashboard_for(scope)
    }

    /// Every policy layer in effect for `scope` right now, root first --
    /// resolved from the live snapshot on every call for the same reason
    /// `roles_for` is: a scope-config write holds from the next request with
    /// nothing to invalidate. Folded into a status by `policy::applicable`
    /// and `policy::evaluate` -- see `policy_report`, `policy_control` and
    /// `policy_attest` in `policies/mod.rs`, which are every caller.
    pub fn policy_chain(&self, scope: &str) -> Vec<factory_core::policy::PolicyLayer> {
        self.factory_snapshot().policy_chain(scope)
    }

    /// The instance root's own `roles:`, after a write to its config.
    pub(crate) fn replace_instance_roles(
        &self,
        roles: std::collections::BTreeMap<String, factory_core::role::RoleSpec>,
    ) {
        let mut factory = self
            .shared.factory
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        factory.config.roles = roles;
    }

    /// The instance root's own top-level `dashboard:`, after a write to its
    /// config -- `replace_instance_roles`, for the dashboard layer (`#160`).
    pub(crate) fn replace_instance_dashboard(&self, dashboard: Option<factory_core::dashboard::DashboardConfig>) {
        let mut factory = self
            .shared.factory
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        factory.config.dashboard = dashboard;
    }

    /// The instance root's own `secrets:` catalogue, after a write to its
    /// config (`#244`) -- `replace_instance_dashboard`, for the catalogue.
    pub(crate) fn replace_instance_secrets(&self, secrets: Vec<factory_core::secrets::SecretDecl>) {
        let mut factory = self
            .shared.factory
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        factory.config.secrets = secrets;
    }

    pub(crate) fn replace_scope(&self, id: &str, replacement: factory_core::config::Scope) {
        let mut factory = self
            .shared.factory
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(scope) = factory.config.scopes.iter_mut().find(|scope| scope.id == id) {
            *scope = replacement;
        }
    }

    // -- the API every interface speaks ------------------------------------

    /// Every request goes through here: who is asking, may they, then do it.
    pub async fn handle(self: &Arc<Self>, envelope: Envelope) -> Response {
        let caller = match self.caller_for(envelope.token.as_deref()).await {
            Ok(c) => c,
            Err(e) => return Response::error(e.code(), e.to_string()),
        };
        let request = self.bind_caller(&caller, envelope.request);
        if let Err(e) = self.authorize(&caller, &request).await {
            return Response::error(e.code(), e.to_string());
        }
        // The request dispatcher covers the whole protocol and is a large
        // future in debug builds. Poll it from the heap so callers do not
        // need a multi-megabyte stack merely to issue a small request.
        match Box::pin(self.dispatch_request(&caller, request)).await {
            Ok(payload) => Response::ok(payload),
            Err(e) => Response::error(e.code(), e.to_string()),
        }
    }

    /// An agent's own scope is the one it means. A foreman that creates a task
    /// without naming a scope means its own, not the instance's first.
    fn bind_caller(&self, caller: &crate::access::Caller, request: Request) -> Request {
        let Some(scope) = caller.scope() else {
            return request;
        };
        match request {
            Request::TaskCreate(mut new) => {
                if new.scope.is_none() {
                    new.scope = Some(scope.to_string());
                }
                Request::TaskCreate(new)
            }
            Request::WorkflowCreate(mut draft) => {
                if draft.scope.trim().is_empty() { draft.scope = scope.to_string(); }
                Request::WorkflowCreate(draft)
            }
            // An agent searching means from where it stands, so a provider
            // that weighs `area` weighs it against the agent's own scope.
            Request::KnowledgeSearch { text, tags, scope: None, limit } => Request::KnowledgeSearch {
                text,
                tags,
                scope: Some(scope.to_string()),
                limit,
            },
            other => other,
        }
    }

    /// For callers inside the daemon, which are always the owner.
    pub async fn handle_request(self: &Arc<Self>, request: Request) -> Response {
        self.handle(Envelope::from(request)).await
    }


    async fn status(&self) -> Result<StatusInfo> {
        let factory = self.factory_snapshot();
        let tasks = self.l4.store.list(&TaskFilter::default()).await?;
        let mut capacity = Vec::new();
        // `config.scopes` alone is every scope, root included: discovery
        // already folds the root's own `scope:` block into it
        // (`discovery::apply`), the same list `reconcile_agents` walks.
        // Chaining `config.scope` on top, as `Config::validate` does to
        // cover its own pre-discovery call, would count the root twice.
        for scope in &factory.config.scopes {
            for agent in scope.declared_agents() {
                let Some(max) = agent.max_sessions else { continue };
                let name = agent.name();
                let cap = self.l4_service().capacity_for(&scope.name, &name, Some(max)).await?;
                let waiting = tasks
                    .iter()
                    .filter(|t| t.slot_wait.as_ref().is_some_and(|w| w.scope == scope.name && w.agent == name))
                    .count() as u32;
                capacity.push(CapacityRow { scope: scope.name.clone(), agent: name, in_use: cap.in_use, max, waiting });
            }
        }
        Ok(StatusInfo {
            instance: factory.config.instance.name.clone(),
            instance_id: factory.config.instance.id.clone(),
            root: factory.root.display().to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_seconds: self.shared.started.elapsed().as_secs(),
            tasks_total: tasks.len(),
            tasks_active: self.l4.store.active_runs().await?.len(),
            subscribers: self.shared.bus.subscriber_count(),
            interfaces: self.shared.interfaces.clone(),
            capacity,
            scopes: factory.scope_names(),
        })
    }


    /// The L2 Environment page's whole answer: one sandbox row per
    /// scope/agent, built from the same `ScopeView`/`AgentView` the roster
    /// already computes, plus the credential inventory. Two payloads out of
    /// one call, the same reason `Agents` returns scopes and roles together --
    /// they are never useful apart, and a page that fetched them separately
    /// could show one refreshed and the other stale.
    async fn environment(&self) -> Result<(Vec<SandboxRow>, Vec<CredentialRow>)> {
        let (scopes, _) = self.scope_views().await?;
        let mut sandboxes = Vec::new();
        for sv in &scopes {
            for av in &sv.agents {
                sandboxes.push(SandboxRow {
                    scope: sv.name.clone(),
                    scope_path: sv.path.clone(),
                    runtime: sv.runtime.clone(),
                    agent: av.name.clone(),
                    harness: av.adapter.clone(),
                    lifetime: av.lifetime.clone(),
                    sandbox: av.sandbox.clone(),
                    enforced: av.sandbox == Sandbox::Openshell.as_str(),
                    worktree_capable: sv.worktree_capable,
                    readiness: av.readiness.clone(),
                });
            }
        }
        Ok((sandboxes, self.credential_inventory().await))
    }

    /// The L1 Infrastructure page's whole answer. Nothing here can fail the
    /// request: a host fact that cannot be read is `None`, and everything
    /// else is the live config snapshot and this process's own state.
    ///
    /// Providers are what the root config declares and nothing more -- no
    /// credential file, Keychain entry or environment variable is read to
    /// find or confirm one. The agents are every scope's
    /// `agents_with(foreman)`, the same list the roster, the Environment page
    /// and Benchmarks are built from, so a synthesized foreman shows up
    /// under the account its harness bills. `shell` agents make no model
    /// call and appear in neither list.
    pub(crate) async fn infrastructure(&self) -> Payload {
        let host = tokio::task::spawn_blocking(crate::host::collect)
            .await
            .unwrap_or_default();

        let factory = self.factory_snapshot();
        let daemon_config = &factory.config.daemon;
        let relative = |path: &Path| {
            path.strip_prefix(&factory.root)
                .unwrap_or(path)
                .display()
                .to_string()
        };

        let database = factory.database_path();
        let size_bytes = tokio::fs::metadata(&database).await.ok().map(|m| m.len());
        let interfaces = crate::interfaces::interface_facts(&daemon_config.interfaces);
        // The herdr session is the daemon's own operational setting, which
        // its service definition sets and every herdr call it makes
        // inherits -- not a credential, and not a provider's `env:`, which is
        // never read.
        let herdr_session = (daemon_config.default_runtime == "herdr")
            .then(|| std::env::var("HERDR_SESSION").ok())
            .flatten()
            .filter(|s| !s.is_empty());
        let started_at = Utc::now()
            - chrono::Duration::from_std(self.shared.started.elapsed()).unwrap_or_default();
        let daemon = DaemonFacts {
            version: env!("CARGO_PKG_VERSION").to_string(),
            pid: std::process::id(),
            started_at: chrono::SubsecRound::trunc_subsecs(started_at, 0),
            root: factory.root.display().to_string(),
            store: StoreFacts {
                kind: daemon_config.task_store.clone(),
                path: relative(&database),
                size_bytes,
            },
            socket: relative(&factory.socket_path()),
            interfaces,
            runtime: daemon_config.default_runtime.clone(),
            herdr_session,
        };

        let infrastructure = &factory.config.infrastructure;
        let mut providers: Vec<ProviderRow> = infrastructure
            .providers
            .iter()
            .map(|p| ProviderRow {
                name: p.name.clone(),
                vendor: p.vendor.clone(),
                kind: p.kind,
                plan: p.plan.clone(),
                env: p.env.clone(),
                agents: Vec::new(),
                windows: Vec::new(),
                active_runs: Vec::new(),
                usage_unknown: None,
            })
            .collect();
        let mut unassigned = Vec::new();
        for scope in &factory.config.scopes {
            for agent in scope.agents_with(&daemon_config.foreman) {
                if agent.harness == SHELL_HARNESS {
                    continue;
                }
                match infrastructure.provider_for(&agent) {
                    Some((provider, via)) => {
                        if let Some(row) = providers.iter_mut().find(|row| row.name == provider.name) {
                            row.agents.push(ProviderAgent {
                                scope: scope.name.clone(),
                                agent: agent.name(),
                                harness: agent.harness.clone(),
                                via,
                            });
                        }
                    }
                    None => unassigned.push(UnassignedAgent {
                        scope: scope.name.clone(),
                        agent: agent.name(),
                        harness: agent.harness.clone(),
                    }),
                }
            }
        }

        // Provider usage is derived from the same append-only run snapshots
        // as cost. It is deliberately not persisted as a second aggregate.
        let now = Utc::now();
        let mut recent_runs = self
            .l4.store
            .runs_between(now - chrono::Duration::days(8), now + chrono::Duration::seconds(1))
            .await
            .unwrap_or_default();
        if let Ok(active) = self.l4.store.active_runs().await {
            for active_run in active {
                if !recent_runs.iter().any(|candidate| candidate.id == active_run.id) {
                    recent_runs.push(active_run);
                }
            }
        }
        let mut task_scopes = std::collections::BTreeMap::new();
        for run in &recent_runs {
            if !task_scopes.contains_key(&run.task_id) {
                let scope = self
                    .l4.store
                    .get(&run.task_id)
                    .await
                    .ok()
                    .flatten()
                    .map(|task| factory.canonical_scope_name(&task.scope))
                    .unwrap_or_else(|| "?".into());
                task_scopes.insert(run.task_id.clone(), scope);
            }
        }
        let mut snapshots_by_run = std::collections::BTreeMap::new();
        for run in recent_runs.iter().filter(|run| run.provider_account.is_some()) {
            let snapshots = self.l4.store.usage_snapshots(&run.id).await.unwrap_or_default();
            snapshots_by_run.insert(run.id.clone(), snapshots);
        }
        let bound_runs: Vec<Run> =
            recent_runs.iter().filter(|run| run.provider_account.is_some()).cloned().collect();
        let allocation = factory_core::usage::allocate_plan_share(&bound_runs, &snapshots_by_run);
        for provider in &mut providers {
            let bound: Vec<&Run> = recent_runs
                .iter()
                .filter(|run| run.provider_account.as_deref() == Some(provider.name.as_str()))
                .collect();
            provider.active_runs = bound
                .iter()
                .filter(|run| !run.status.is_terminal())
                .map(|run| ProviderRun {
                    run_id: run.id.clone(),
                    task_id: run.task_id.clone(),
                    scope: task_scopes.get(&run.task_id).cloned().unwrap_or_else(|| "?".into()),
                    agent: run.agent.clone(),
                    instance: run.session.as_ref().map(|session| session.handle.clone()),
                })
                .collect();

            // One timeline per window across every run bound to the account,
            // ordered by when the runtime sampled each reading: a handoff from
            // one run to the next is still one window moving.
            let mut timelines: std::collections::BTreeMap<(u32, String), Vec<WindowReading>> =
                std::collections::BTreeMap::new();
            for run in &bound {
                for snapshot in snapshots_by_run.get(&run.id).into_iter().flatten() {
                    let Some(usage) = &snapshot.usage else { continue };
                    let sampled_at = factory_core::usage::observed_at(snapshot);
                    for session in &usage.sessions {
                        let Some(rate) = &session.rate_limit else { continue };
                        for window in &rate.windows {
                            let (Some(minutes), Some(reset)) = (window.window_minutes, window.resets_at.clone()) else {
                                continue;
                            };
                            let Ok(reset_at) = chrono::DateTime::parse_from_rfc3339(&reset) else {
                                continue;
                            };
                            if reset_at.with_timezone(&Utc) <= now {
                                continue;
                            }
                            timelines.entry((minutes, reset)).or_default().push(WindowReading {
                                sampled_at,
                                sample_time_estimated: usage.sampled_at.is_none(),
                                used_percent: window.used_percent.filter(|used| used.is_finite()),
                                plan_type: rate.plan_type.clone(),
                                attribution_quality: rate.attribution_quality.clone(),
                            });
                        }
                    }
                }
            }
            provider.windows = timelines
                .into_iter()
                .filter_map(|((minutes, reset), mut readings)| {
                    // Readings sampled in one instant are one observation,
                    // the highest standing for it -- the rule plan share
                    // uses (`factory_core::usage::PROVIDER_WINDOW_TIE`), so
                    // the value shown and its badge come from the same one.
                    readings.sort_by(|a, b| {
                        a.sampled_at.cmp(&b.sampled_at).then_with(|| {
                            let used = |r: &WindowReading| r.used_percent.unwrap_or(f64::NEG_INFINITY);
                            used(a).total_cmp(&used(b))
                        })
                    });
                    let latest = readings.pop()?;
                    let trend = latest.used_percent.zip(
                        readings
                            .iter()
                            .rev()
                            .find(|prior| prior.sampled_at < latest.sampled_at && prior.used_percent.is_some())
                            .and_then(|prior| prior.used_percent),
                    )
                    .map(|(current, prior)| current - prior);
                    // Who the reading is charged to is a fact about the
                    // interval it closed, fixed when it was sampled -- not
                    // about who happens to be running when the page is read.
                    let interval = allocation.intervals.iter().find(|interval| {
                        interval.provider_account == provider.name
                            && interval.window_minutes == minutes
                            && interval.resets_at == reset
                            && interval.to == latest.sampled_at
                    });
                    let attribution_unknown = match interval {
                        Some(interval) => interval.unknown.clone(),
                        None if latest.used_percent.is_some() => Some(
                            "no earlier sample of this window to attribute the reading's change from".into(),
                        ),
                        None => None,
                    };
                    Some(ProviderWindow {
                        window_minutes: minutes,
                        used_percent: latest.used_percent,
                        resets_at: reset,
                        sampled_at: latest.sampled_at,
                        sample_time_estimated: latest.sample_time_estimated,
                        plan_type: latest.plan_type,
                        attribution_quality: latest.attribution_quality,
                        attribution: interval.and_then(|interval| interval.attribution),
                        attribution_unknown,
                        // A reading that does not say when it was sampled
                        // cannot be vouched for as fresh.
                        stale: latest.sample_time_estimated
                            || now - latest.sampled_at > chrono::Duration::minutes(15),
                        trend_percent: trend,
                        unknown: latest
                            .used_percent
                            .is_none()
                            .then(|| "provider did not report used percent".into()),
                    })
                })
                .collect();
            provider.windows.sort_by_key(|window| window.window_minutes);
            if provider.kind == factory_core::config::ProviderKind::Subscription && provider.windows.is_empty() {
                provider.usage_unknown = Some("no current rate-limit observation from a bound run".into());
            }
        }

        // One row per harness any scope's agents run on, whether or not it
        // has been probed yet -- plus anything probed since that the config
        // no longer names.
        let mut known: Vec<(String, factory_core::harness::HealthProbe)> = Vec::new();
        for scope in &factory.config.scopes {
            for agent in scope.agents_with(&daemon_config.foreman) {
                let Some(probe) = self.shared.registry.agent(&agent.harness).ok().and_then(|a| a.health_probe()) else {
                    continue;
                };
                let harness = crate::harness_health::harness_name(&probe);
                if !known.iter().any(|(_, p)| *p == probe) {
                    known.push((harness, probe));
                }
            }
        }
        let harnesses = self
            .l3.harness
            .rows(&known, daemon_config.harness_health.repair_script.as_deref());

        Payload::Infrastructure {
            host,
            daemon,
            providers,
            unassigned,
            harnesses,
        }
    }

    /// The honest v1 answer to "what can an agent already reach": a fixed
    /// list of places a credential commonly sits, checked for existence and
    /// nothing else. No value is ever opened, held, or logged -- `present` is
    /// the entire result of each check. `pub(crate)` so `policies::secrets_fact_map`
    /// can build a `secrets` check's evidence from the same inventory this
    /// tab shows -- one fact, read once, never a second copy of what
    /// "present" means.
    pub(crate) async fn credential_inventory(&self) -> Vec<CredentialRow> {
        crate::facts::environment_credentials(self).inventory().await
    }



    // -- naming an agent ----------------------------------------------------

    /// Turn what a task asked for into the agent it will actually run as.
    ///
    /// A task names a concrete agent in its scope -- `assistant`, `scratch` --
    /// and the adapter behind it follows from the config. An adapter name
    /// still works for a scope that declares nothing, or for a one-off with
    /// `--agent claude-code`.
    pub fn resolve_agent(&self, scope_name: &str, name: &str) -> Result<(String, String, Option<ScopeAgent>)> {
        crate::commands::agents(self).resolve_agent(scope_name, name)
    }

    /// Edit a task. Anything that has to stay true of a task is checked here
    /// rather than in the store: an agent that does not resolve, a scope that
    /// does not exist, and -- the one that is easy to miss -- a schedule set
    /// after creation, which would otherwise never fire because nothing
    /// recomputed when it is next due.
    /// `asked` is who made the request and why: pausing or resuming a
    /// schedule is journaled with both (`#106`). `None` is the daemon
    /// changing a task on its own account.
    pub(crate) async fn update(
        &self,
        id: &str,
        mut patch: TaskPatch,
        asked: Option<&crate::operations::Asked>,
    ) -> Result<Task> {
        let _trigger_edit = if patch.after.is_some() || patch.clear_after || patch.schedule.is_some() || patch.clear_schedule {
            Some(self.l4.admission_lock.lock().await)
        } else {
            None
        };
        let factory = self.factory_snapshot();
        let current = self.require(id).await?;
        if current.workflow_origin.is_some() && (patch.after.is_some() || patch.clear_after || patch.after_condition.is_some()) {
            return Err(FactoryError::BadRequest("workflow waits are released by the graph or an explicit --override-wait, not task edits".into()));
        }
        let after = patch.after.as_ref().or_else(|| (!patch.clear_after).then_some(current.after.as_ref()).flatten());
        let scheduled = patch.schedule.is_some() || (!patch.clear_schedule && current.schedule.is_some());
        if after.is_some() && scheduled {
            return Err(FactoryError::BadRequest("after and schedule are exclusive triggers".into()));
        }
        if let Some(after) = &patch.after {
            self.l4_service().validate_after(Some(id), after).await?;
            if current.status != TaskStatus::Pending || current.runs > 0 {
                return Err(FactoryError::BadRequest("an after trigger can only be set on a never-started pending task".into()));
            }
        }
        // The bookkeeping is the daemon's, not a caller's.
        patch.runs = None;
        // So is the intake record, and the way out of intake is a decision
        // (`crate::intake`), not an edit of the status (`#119`).
        if patch.intake.is_some() {
            return Err(FactoryError::BadRequest(
                "the intake record is written by the intake requests, not by an edit".into(),
            ));
        }
        if current.status == TaskStatus::Intake && patch.status.is_some_and(|s| s != TaskStatus::Intake) {
            return Err(FactoryError::BadRequest(
                "an item leaves intake by a decision (factory intake decide), not by an edit of its status".into(),
            ));
        }
        if patch.estimate_seconds == Some(0) {
            return Err(FactoryError::BadRequest(
                "a task estimate must be at least one second".into(),
            ));
        }
        if let Some(estimate) = &patch.estimate {
            estimate.validate().map_err(FactoryError::BadRequest)?;
            patch.estimate_seconds = Some(estimate.time.expected);
        } else if let Some(seconds) = patch.estimate_seconds {
            patch.estimate = Some(factory_core::task::Estimate::point(seconds));
        }
        if let Some(category) = &patch.category {
            factory_core::control_plan::check_category(category).map_err(FactoryError::BadRequest)?;
        }
        if patch.labels.as_ref().is_some_and(|labels| {
            labels.contains_key(crate::verification::REVIEW_RUN_LABEL)
                || labels.contains_key(crate::verification::REVIEW_STEP_LABEL)
                || labels.contains_key(crate::verification::REVIEW_DIGEST_LABEL)
        }) {
            return Err(FactoryError::BadRequest(
                "factory.review_* labels are reserved for daemon-created review tasks".into(),
            ));
        }

        let scope = patch.scope.clone().unwrap_or_else(|| current.scope.clone());
        if patch.scope.is_some() {
            // Store the identity the scope actually has, not necessarily the
            // one the caller typed -- a bare name from before scopes had
            // paths still resolves (`Factory::scope`'s fallback), but writing
            // it back down unchanged would keep manufacturing the very
            // ambiguity that fallback exists to paper over.
            patch.scope = Some(factory.scope(&scope)?.name.clone());
        }
        match &patch.agent {
            Some(agent) => {
                let (name, _, _) = self.resolve_agent(&scope, agent)?;
                patch.agent = Some(name);
            }
            // A task moved to another scope must still have an agent there.
            None if patch.scope.is_some() => {
                self.resolve_agent(&scope, &current.agent)?;
            }
            None => {}
        }
        if let Some(rt) = &patch.runtime {
            self.shared.registry.runtime(rt)?;
        }
        // A wait names the agent and scope it is waiting on (`#179`); moving
        // either makes the wait meaningless -- it would go on counting
        // against a slot this task no longer means to use.
        let clears_wait = current.slot_wait.is_some() && (patch.agent.is_some() || patch.scope.is_some());
        if clears_wait {
            patch.clear_slot_wait = true;
        }
        if let Some(s) = &patch.schedule {
            patch.next_run_at = Some(schedule::next_after(s, Utc::now())?);
        }
        // Pausing keeps the schedule, so it needs one to keep; resuming
        // starts again from now -- the slots that passed while paused were
        // paused, not missed, and must not fire as a catch-up burst. A
        // retry streak the pause interrupted is over too: its backoff time
        // has passed with nothing fired, and leaving `pending_retry` behind
        // would make the resumed schedule's first regular firing claim to
        // be a retry of it (the stale-mirror problem `PendingRetry`
        // describes).
        // A pause belongs to a schedule. Removing the schedule (without
        // setting another in the same patch) takes the pause with it, or a
        // schedule added later would sit paused with nobody having paused
        // it -- and the UI's form clears the schedule on every save that
        // leaves it empty.
        let will_be_scheduled =
            patch.schedule.is_some() || (!patch.clear_schedule && current.schedule.is_some());
        let pause_dropped =
            current.schedule_paused && !will_be_scheduled && patch.schedule_paused != Some(true);
        if pause_dropped {
            patch.schedule_paused = Some(false);
        }
        let pause_change = match patch.schedule_paused {
            Some(paused) if paused != current.schedule_paused => Some(paused),
            // Saying what is already so changes nothing and journals
            // nothing.
            Some(_) => {
                patch.schedule_paused = None;
                None
            }
            None => None,
        };
        if let Some(paused) = pause_change {
            // What the store will leave: it clears before it sets, so a
            // patch carrying both ends up with the new schedule.
            let schedule = match (&patch.schedule, patch.clear_schedule) {
                (Some(s), _) => Some(s.clone()),
                (None, true) => None,
                (None, false) => current.schedule.clone(),
            };
            match (&schedule, paused) {
                (None, true) => {
                    return Err(FactoryError::BadRequest(
                        "only a scheduled task has a schedule to pause".into(),
                    ));
                }
                (Some(s), false) => {
                    patch.next_run_at = Some(schedule::next_after(s, Utc::now())?);
                    if current.pending_retry.is_some() {
                        patch.clear_pending_retry = true;
                    }
                }
                _ => {}
            }
        }
        // Same rule `create_task` enforces, checked against what the task's
        // schedule will actually be once this patch lands rather than what
        // it is now -- clearing the schedule in the same edit that sets a
        // retry policy is just as silently pointless as setting one on a
        // task that never had a schedule to begin with.
        if patch.retry.is_some() {
            let will_be_scheduled = if patch.clear_schedule {
                false
            } else {
                patch.schedule.is_some() || current.schedule.is_some()
            };
            if !will_be_scheduled {
                return Err(FactoryError::BadRequest(
                    "a retry policy only means something for a scheduled task; add a schedule too, or drop retry".into(),
                ));
            }
        }

        let task = self.l4.store.update(id, &patch).await?;
        if let Some(paused) = pause_change {
            // Journal what the store kept, not what was asked: an
            // out-of-process store written before pausing existed takes the
            // patch and quietly drops the field, and a journal saying
            // "paused" over a schedule that still fires would be worse than
            // the error.
            if task.schedule_paused != paused {
                return Err(FactoryError::adapter(
                    self.l4.store.name(),
                    format!(
                        "the task store did not keep schedule_paused = {paused}; it may predate \
                         pausing schedules, so the schedule is unchanged"
                    ),
                ));
            }
            // Who asked, and why when they said (`#106`); the daemon's own
            // changes say nothing of the sort.
            let by = asked.map(|a| format!(" {}", a.words())).unwrap_or_default();
            let (kind, message) = match (pause_dropped, paused, task.next_run_at) {
                (true, _, _) => (
                    "schedule_pause_cleared",
                    format!("schedule removed{by}, and its pause with it; a schedule added later fires"),
                ),
                (false, true, _) => {
                    ("schedule_paused", format!("schedule paused{by}; nothing fires until it is resumed"))
                }
                (false, false, Some(next)) => {
                    ("schedule_resumed", format!("schedule resumed{by}; next firing at {}", next.to_rfc3339()))
                }
                (false, false, None) => ("schedule_resumed", format!("schedule resumed{by}")),
            };
            let entry = match asked {
                Some(asked) => asked.entry(kind, message, serde_json::json!({})),
                None => TaskEntry::new("daemon", kind, message),
            };
            self.entry(&task.id, entry).await;
        }
        if clears_wait && task.slot_wait.is_none() {
            self.entry(
                &task.id,
                TaskEntry::new(
                    "daemon",
                    "capacity_wait_cleared",
                    "no longer waiting for a slot: its agent or scope changed",
                ),
            )
            .await;
        }
        self.shared.bus.publish(Event::TaskUpdated { task: task.clone() });
        Ok(task)
    }

    // -- creating ----------------------------------------------------------

    /// L4's settling path asks L5 to record the answer to a suggestion's ask (a page forwarder: an L4 file calling
    /// `.l5_service()` would be a pull up the ladder; the answer arrives as an event once L5 reads run completion).
    pub(crate) async fn settle_suggestion_ask(&self, run: &Run) {
        self.l5_service().settle_suggestion_ask(run).await
    }

    pub async fn create(&self, new: NewTask) -> Result<Task> {
        self.l4_service().create(new).await
    }

    // -- running -----------------------------------------------------------















    // -- max_sessions: releasing a slot (#179) ------------------------------




    /// Start the capacity-release worker: every [`CapacityEvent`]
    /// `enqueue_capacity_release`/`enqueue_capacity_sweep` sends is handled
    /// here, one at a time, where an `Arc<Self>` is available. One consumer
    /// is the point: `release_waiting` and `recheck_capacity` both read
    /// `waiting_tasks()` and admit from it, and `dispatch` has no guard
    /// against two overlapping admission attempts for the same waiting task,
    /// so running them one after another here -- never on separate spawned
    /// tasks -- is what keeps a release and a sweep from both admitting the
    /// same task at once. Called once, at startup, like `spawn_verifier`; a
    /// second call is a no-op.
    pub fn spawn_capacity_release_worker(self: &Arc<Self>) {
        let Some(mut rx) = self.l4.capacity_release_rx.lock().unwrap().take() else {
            return;
        };
        let engine = self.clone();
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    CapacityEvent::Release(scope, agent) => engine.l4_service().release_waiting(&scope, &agent).await,
                    CapacityEvent::Sweep => engine.l4_service().recheck_capacity().await,
                }
            }
        });
    }













    /// On start: delete every OpenShell sandbox this instance made whose run
    /// is no longer active -- one a crash, a lost session or a failed delete
    /// left behind (`#218`). A sandbox that outlives its run is a leak. Only
    /// asked for current declarations and persisted cleanup records, so
    /// removing or changing a declaration cannot strand an older sandbox.
    pub async fn reconcile_openshell(&self) {
        crate::commands::l3(self)
            .port()
            .reconcile_environments(&crate::commands::StoreLedger(self.l4.store.clone()), &crate::commands::EntryNotes::of(self))
            .await;
    }

    /// Terminal output for a run: live while it is running, the transcript kept
    /// at the end once it is not, and an empty string in the moment between a
    /// run starting and its session existing. Never an error -- a view that
    /// polls this should show a blank pane, not a red one.
    pub async fn output(&self, run: &Run, lines: u32) -> String {
        if let Some(session) = &run.session {
            if let Ok(runtime) = self.shared.registry.runtime(&session.runtime) {
                if let Ok(text) = runtime.read(session, lines).await {
                    return text;
                }
            }
        }
        let entries = self.l4.store.run_entries(&run.id, 500).await.unwrap_or_default();
        for entry in entries.iter().rev() {
            if entry.kind == "transcript" {
                if let Some(text) = entry
                    .data
                    .as_ref()
                    .and_then(|d| d.get("text"))
                    .and_then(|v| v.as_str())
                {
                    return text.to_string();
                }
            }
        }
        String::new()
    }

    // -- what the scheduler needs ------------------------------------------








    /// One frame of a run's session. `None` once the run has ended and its
    /// session is released -- what is left then is the transcript.
    pub async fn run_screen(&self, run: &Run) -> Result<Option<Screen>> {
        let Some(session) = &run.session else {
            return Ok(None);
        };
        let runtime = self.shared.registry.runtime(&session.runtime)?;
        runtime.screen(session).await
    }

    pub async fn session_status(&self, run: &Run) -> RuntimeStatus {
        let Some(session) = &run.session else {
            return RuntimeStatus::Unknown;
        };
        match self.shared.registry.runtime(&session.runtime) {
            Ok(rt) => rt.status(session).await.unwrap_or(RuntimeStatus::Unknown),
            Err(_) => RuntimeStatus::Unknown,
        }
    }

    /// `session_status`, plus where the answer came from. The one caller that
    /// needs provenance is `record_run_liveness` -- the scheduler's `Gone`
    /// check and `supervise_agents` only ever need the status, so they keep
    /// calling `status` through `session_status` rather than paying for a
    /// question they do not ask.
    pub async fn session_status_report(&self, run: &Run) -> StatusReport {
        let unknown = StatusReport {
            status: RuntimeStatus::Unknown,
            source: StatusSource::Unknown,
        };
        let Some(session) = &run.session else {
            return unknown;
        };
        match self.shared.registry.runtime(&session.runtime) {
            Ok(rt) => rt.status_report(session).await.unwrap_or(unknown),
            Err(_) => unknown,
        }
    }

    // -- small helpers ------------------------------------------------------

    pub(crate) async fn require(&self, id: &str) -> Result<Task> {
        self.l4_service().require(id).await
    }

    pub(crate) async fn require_run(&self, id: &str) -> Result<Run> {
        self.l4_service().require_run(id).await
    }

    pub(crate) async fn publish_task(&self, id: &str) {
        self.l4_service().publish_task(id).await
    }

    /// `pub(crate)`: `occupancy::record_run_liveness` journals a hook-reported
    /// block or unblock the same way any other daemon-caused change is
    /// journaled here.
    pub(crate) async fn entry(&self, task_id: &str, entry: TaskEntry) {
        self.l4_service().entry(task_id, entry).await
    }
}

/// One provider rate-limit window as one run's snapshot saw it -- what the
/// L1 provider card's timeline is built from (`Engine::infrastructure`).
struct WindowReading {
    sampled_at: chrono::DateTime<Utc>,
    /// The runtime gave no `sampled_at`, so `sampled_at` is when Factory asked.
    sample_time_estimated: bool,
    used_percent: Option<f64>,
    plan_type: Option<String>,
    attribution_quality: Option<String>,
}

/// The upstream line a workflow feedback run is dispatched with. A round is
/// one in this node's own sequence, and a part workflow's node (`#235`) also
/// runs again when integration sends its part back -- which spends no
/// workflow exit's budget -- so the line says how many were which.
pub(crate) fn feedback_round_title(round: u32, integration_rounds: u32) -> String {
    if integration_rounds == 0 {
        return format!("Workflow rework round {round}");
    }
    format!(
        "Workflow rework round {round} ({} sent back by a workflow step, {integration_rounds} by integration)",
        round.saturating_sub(integration_rounds)
    )
}

/// The request L2's `prepare` is given for a run's sandbox, from what dispatch has at hand.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sandbox_request<'a>(
    factory: &'a Factory,
    task: &'a Task,
    run_id: &'a str,
    cwd: &'a Path,
    launch: &'a factory_kernel::LaunchSpec,
    prompt: &'a str,
    resume: Option<(&'a factory_kernel::LaunchSpec, &'a str, &'a Path)>,
    config: &'a factory_core::openshell::OpenshellConfig,
    callback: &'a factory_core::openshell::CallbackTarget,
    resolved: &'a crate::provision::Resolved,
) -> factory_environment::provision::PrepareRequest<'a> {
    factory_environment::provision::PrepareRequest {
        instance_id: &factory.config.instance.id,
        root: factory.root.clone(),
        factory_dir: factory.factory_dir(),
        guides_dir: factory.guides_dir(),
        task_id: &task.id,
        scope: &task.scope,
        agent: &task.agent,
        run_id,
        cwd,
        launch,
        prompt,
        resume,
        config,
        callback,
        image: &resolved.image,
        providers: &resolved.providers,
    }
}

pub(crate) fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let head: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

impl Engine {

    /// Tell the knowledge provider what a write just put in the vault. The
    /// write has already happened and is the source of truth, so a provider
    /// that cannot keep up is a warning, never a failed write.
    async fn knowledge_changed(&self, files: &[String]) {
        if files.is_empty() {
            return;
        }
        let (root, name) = {
            let factory = self.factory_snapshot();
            (factory.root, factory.config.daemon.knowledge_provider)
        };
        let result = match self.shared.registry.knowledge(&name) {
            Ok(provider) => provider.changed(&root, files).await,
            Err(e) => Err(e),
        };
        if let Err(e) = result {
            tracing::warn!("knowledge provider {name:?} was not told about {} written file(s): {e}", files.len());
        }
    }
}

/// `knowledge::WriteResult` to the wire shape `Request::KnowledgeImport`,
/// `KnowledgeAdd` and `KnowledgeWriteFile` all answer with.
fn knowledge_write_payload(result: factory_core::knowledge::WriteResult) -> Payload {
    Payload::KnowledgeWrite {
        copied: result.copied,
        skipped_existing: result.skipped_existing,
        skipped_hidden: result.skipped_hidden,
        refused: result.refused,
        truncated: result.truncated,
    }
}

/// For tests anywhere in the daemon that need a task whose newest run
/// failed (`#122`), without a runtime to fail one for real: a run row as
/// dispatch makes it, ended the way the watchdog or a report ends one.
#[cfg(test)]
impl Engine {
    pub(crate) async fn fail_task_for_test(&self, task_id: &str, kind: FailKind) -> Task {
        let run = self
            .l4.store
            .create_run(&NewRun {
                task_id: task_id.to_string(),
                trigger: Trigger::Manual,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: "herdr".into(),
                token: "tok".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        self.l4_service().fail_run(&run.id, kind, "the remediation run failed").await;
        let task = self.require(task_id).await.unwrap();
        assert!(task.blocked_by_failure(), "sanity: {:?}", task.status);
        task
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_core::adapter::RuntimeConnectionState;
    use factory_core::agent::Lifetime;
    use factory_core::config::{Config, DaemonConfig, Instance, PolicyDeclaration, Scope};
    use factory_core::run::RunStatus;
    use factory_core::task::Schedule;
    use factory_plugins::{Registry, SqliteStore};

    #[tokio::test]
    async fn isolated_process_fact_wiring_uses_a_fresh_scope_tree_after_configuration_changes() {
        use crate::facts::{Facts, TaskInventoryQuery};
        use factory_kernel::{ScheduledRunDatesFact, TaskInventoryFact, L6};
        let engine = test_engine(PathBuf::from("projects/legacy"));
        let mut task = factory_process::store::task_from_new(
            NewTask { title: "scheduled".into(), ..Default::default() },
            "legacy".into(), "shell".into(), "quiet".into(),
        );
        task.schedule = Some(Schedule::Cron("0 9 * * *".into()));
        task.next_run_at = Some("2026-10-05T09:00:00Z".parse().unwrap());
        engine.l4.store.create(&task).await.unwrap();
        let facts = Facts::<L6>::new(&engine);
        let first = facts.get::<ScheduledRunDatesFact>(&()).await.unwrap();
        assert_eq!(first.runs[0].scope, "demo");
        engine.shared.factory.write().unwrap().config.scopes[0].name = "renamed".into();
        assert_eq!(facts.get::<ScheduledRunDatesFact>(&()).await.unwrap().runs[0].scope, "renamed");
        let selected = facts.get::<TaskInventoryFact>(
            &TaskInventoryQuery::Members(["legacy".into()].into_iter().collect()),
        ).await.unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].scope, "legacy", "inventory never rewrites persisted identity");
        engine.shared.factory.write().unwrap().config.scopes.clear();
        assert_eq!(facts.get::<ScheduledRunDatesFact>(&()).await.unwrap().runs[0].scope, "legacy");
        assert!(facts.get::<TaskInventoryFact>(&TaskInventoryQuery::Exact("legacy".into())).await.is_err());
    }

    #[test]
    fn request_future_stays_bounded_as_control_orchestration_grows() {
        let engine = test_engine(PathBuf::from("/tmp"));
        let caller = crate::access::Caller::Owner;
        let future = engine.dispatch_request(&caller, Request::Status);
        let bytes = std::mem::size_of_val(&future);
        assert!(bytes < 64 * 1024, "request future uses {bytes} bytes before HTTP routing frames");
    }

    /// A scope pointed at `scope_path`, one store in memory, and every
    /// built-in adapter registered -- enough to dispatch a task without a
    /// real herdr or a real agent, since the paths under test here never
    /// reach either.
    fn test_engine(scope_path: PathBuf) -> Arc<Engine> {
        let config = Config {
            version: 1,
            instance: Instance {
                id: "test".into(),
                name: "test".into(),
            },
            daemon: DaemonConfig {
                // Every dispatching test in this module runs in this same
                // process, several in parallel, and the default is on -- a
                // real `power_assertion` would fork a real `caffeinate`
                // holding a real PreventSystemSleep assertion on whatever
                // machine runs `cargo test`, once per test, for tests that
                // never mean to exercise that. `crate::power`'s own tests
                // already cover the platform call in isolation with a fake;
                // what the tests in *this* file need from `engine.l1.power` is
                // only the bookkeeping (`active_count`), which is identical
                // whichever backend sits behind it.
                power_assertion: false,
                ..DaemonConfig::default()
            },
            roles: Default::default(),
            dashboard: None,
            policies: Default::default(),
            quality: Default::default(),
            scope: None,
            scopes: vec![Scope {
                id: "scope-id".into(),
                name: "demo".into(),
                path: scope_path,
                agent: None,
                agents: Vec::new(),
                runtime: None,
                git: None,
                task_store: None,
                max_sessions: None,
                roles: Default::default(),
                dashboard: None,
                policies: Default::default(),
                quality: Default::default(),
                intake: Default::default(),
                dependencies: Default::default(),
                environments: Vec::new(),
                renewals: Vec::new(),
                metrics: None,
                backup: None,
            }],
            infrastructure: Default::default(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let factory = Factory {
            root: std::env::temp_dir().join(format!("factory-engine-test-{}", uuid::Uuid::new_v4())),
            config,
        };
        let registry = Registry::with_builtins();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        Arc::new(Engine::new(
            factory,
            registry,
            store,
            PathBuf::from("factory"),
            Vec::new(),
        ))
    }

    #[tokio::test]
    async fn process_command_persists_journal_and_events_then_people_reads_a_live_task_fact() {
        use factory_process::creation::TaskCommands;
        use factory_kernel::{People, TaskSnapshotFact};
        let engine = test_engine(PathBuf::from("/tmp/command-demo"));
        let observer = crate::commands::CreationObserver(engine.shared.bus.clone());
        let mut events = engine.shared.bus.subscribe();
        let service = crate::commands::process(&engine, &observer);
        let receipt = service.submit(NewTask {
            title: "Owned creation".into(),
            instructions: "Exact payload".into(),
            scope: Some("command-demo".into()),
            agent: Some("shell".into()),
            estimate_seconds: Some(23),
            labels: std::collections::BTreeMap::from([("quality".into(), "demo/reliability/restore".into())]),
            ..Default::default()
        }).await.unwrap();
        let task = engine.l4.store.get(&receipt.id).await.unwrap().unwrap();
        assert_eq!(task.scope, "demo");
        assert_eq!(task.estimate.as_ref().unwrap().time.expected, 23);
        let journal = engine.l4.store.entries(&task.id, 10).await.unwrap();
        assert_eq!(journal.len(), 1);
        assert_eq!(journal[0].kind, "created");
        assert_eq!(journal[0].message, "created: Owned creation");
        assert!(matches!(events.try_recv().unwrap(), Event::TaskEntry { id, .. } if id == task.id));
        assert!(matches!(events.try_recv().unwrap(), Event::TaskCreated { task: t } if t.id == task.id));
        let fact = crate::facts::Facts::<People>::new(&engine)
            .get::<TaskSnapshotFact>(&receipt.id).await.unwrap();
        assert_eq!(fact.0, serde_json::to_value(&task).unwrap());
        let updated = engine.l4.store.update(&task.id, &TaskPatch {
            title: Some("Changed after acknowledgement".into()),
            status: Some(TaskStatus::Cancelled),
            ..Default::default()
        }).await.unwrap();
        let live = crate::commands::task_snapshot(&engine, receipt.id.clone()).await.unwrap();
        assert_eq!(serde_json::to_value(live).unwrap(), serde_json::to_value(updated).unwrap());
        assert!(engine.l4.store.delete(&receipt.id).await.unwrap());
        assert!(matches!(crate::commands::task_snapshot(&engine, receipt.id.clone()).await,
            Err(FactoryError::TaskNotFound(id)) if id == receipt.id));
    }

    #[tokio::test]
    async fn owned_creation_refuses_invalid_runtime_or_reserved_review_labels_without_persisting() {
        use factory_process::creation::TaskCommands;
        let engine = test_engine(PathBuf::from("/tmp/command-demo"));
        let observer = crate::commands::CreationObserver(engine.shared.bus.clone());
        let mut events = engine.shared.bus.subscribe();
        let service = crate::commands::process(&engine, &observer);
        assert!(service.submit(NewTask {
            title: "Invalid runtime".into(), agent: Some("shell".into()),
            runtime: Some("unregistered".into()), ..Default::default()
        }).await.is_err());
        assert!(service.submit(NewTask {
            title: "Reserved".into(),
            labels: std::collections::BTreeMap::from([(factory_process::creation::REVIEW_RUN_LABEL.into(), "run".into())]),
            ..Default::default()
        }).await.is_err());
        assert!(engine.l4.store.list(&TaskFilter::default()).await.unwrap().is_empty());
        assert!(events.try_recv().is_err());
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("factory-engine-test-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // ==================================================================
    // #179: max_sessions enforced with a real admission queue
    // ==================================================================
    mod capacity_tests {
        use super::*;
        use async_trait::async_trait;
        use factory_core::adapter::runtime::{AgentRuntime, RuntimeEventStream};
        use factory_core::config::HarnessHealthConfig;
        use factory_core::task::SessionRef;

        /// A runtime that opens a session without doing anything real: a
        /// dispatch really succeeds and the run really stays open -- counted
        /// as using a slot -- until something reports on it. No subprocess,
        /// no herdr socket, nothing the machine running the test needs.
        struct StubRuntime;

        #[async_trait]
        impl AgentRuntime for StubRuntime {
            fn name(&self) -> &str {
                "stub"
            }
            async fn start(&self, req: &StartRequest) -> Result<SessionRef> {
                Ok(SessionRef { runtime: "stub".into(), handle: format!("stub-{}", req.id), meta: Default::default() })
            }
            async fn submit(&self, _session: &SessionRef, _text: &str) -> Result<()> {
                Ok(())
            }
            async fn status(&self, _session: &SessionRef) -> Result<RuntimeStatus> {
                Ok(RuntimeStatus::Unknown)
            }
            async fn send_text(&self, _session: &SessionRef, _text: &str) -> Result<()> {
                Ok(())
            }
            async fn send_keys(&self, _session: &SessionRef, _keys: &[String]) -> Result<()> {
                Ok(())
            }
            async fn read(&self, _session: &SessionRef, _lines: u32) -> Result<String> {
                Ok(String::new())
            }
            async fn stop(&self, _session: &SessionRef) -> Result<()> {
                Ok(())
            }
            async fn watch(&self) -> Result<Option<RuntimeEventStream>> {
                Ok(None)
            }
        }

        /// A scope declaring `agents` over a `stub` runtime with the harness
        /// probe off, so a dispatch really succeeds and really keeps its
        /// session open -- exactly what counting slots needs.
        fn capacity_engine(agents: Vec<ScopeAgent>, scope_max_sessions: Option<u32>) -> Arc<Engine> {
            let scope_path = temp_dir("capacity");
            let config = Config {
                version: 1,
                instance: Instance { id: "test".into(), name: "test".into() },
                daemon: DaemonConfig {
                    power_assertion: false,
                    harness_health: HarnessHealthConfig { enabled: false, ..Default::default() },
                    ..DaemonConfig::default()
                },
                roles: Default::default(),
                dashboard: None,
                policies: Default::default(),
                quality: Default::default(),
                scope: None,
                scopes: vec![Scope {
                    id: "scope-id".into(),
                    name: "demo".into(),
                    path: scope_path,
                    agent: None,
                    agents,
                    runtime: Some("stub".into()),
                    git: None,
                    task_store: None,
                    max_sessions: scope_max_sessions,
                    roles: Default::default(),
                    dashboard: None,
                    policies: Default::default(),
                    quality: Default::default(),
                    intake: Default::default(),
                    dependencies: Default::default(),
                    environments: Vec::new(),
                    renewals: Vec::new(),
                    metrics: None,
                    backup: None,
                }],
                infrastructure: Default::default(),
                secrets: Vec::new(),
                plugins_dir: None,
                renewals: Vec::new(),
                renewals_notify: None,
            };
            let factory = Factory {
                root: std::env::temp_dir().join(format!("factory-capacity-test-{}", uuid::Uuid::new_v4())),
                config,
            };
            let mut registry = Registry::with_builtins();
            registry.add_runtime(Arc::new(StubRuntime), "test");
            let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
            Arc::new(Engine::new(factory, registry, store, PathBuf::from("factory"), Vec::new()))
        }

        fn agent(name: &str, harness: &str, max_sessions: Option<u32>) -> ScopeAgent {
            ScopeAgent {
                name: Some(name.into()),
                harness: harness.into(),
                lifetime: Lifetime::Task,
                role: Role::default(),
                autostart: None,
                args: Vec::new(),
                sandbox: Sandbox::None,
                openshell: None,
                provider: None,
                max_sessions,
            }
        }

        async fn task(engine: &Arc<Engine>, title: &str, agent_name: &str) -> Task {
            engine
                .create(NewTask {
                    title: title.into(),
                    instructions: "true".into(),
                    scope: Some("demo".into()),
                    agent: Some(agent_name.into()),
                    worktree: Some(false),
                    ..Default::default()
                })
                .await
                .unwrap()
        }

        async fn report_done(engine: &Arc<Engine>, task_id: &str) {
            let run = engine.l4.store.active_run(task_id).await.unwrap().unwrap();
            engine
                .l4_service()
                .report(
                    task_id,
                    TaskReport {
                        artifacts: Vec::new(),
                        status: Some(RunStatus::Done),
                        message: None,
                        result: Some("ok".into()),
                        send_to: None,
                        error: None,
                        token: run.token.clone(),
                    },
                )
                .await
                .unwrap();
        }

        /// The triage's own scenario: `max_sessions: 2` on a shell agent,
        /// four manual runs -- two run, two wait with their reason journaled.
        #[tokio::test]
        async fn capacity_holds_the_third_and_fourth_of_four_manual_runs_at_a_limit_of_two() {
            let engine = capacity_engine(vec![agent("codex", "shell", Some(2))], None);
            let mut tasks = Vec::new();
            for i in 0..4 {
                let t = task(&engine, &format!("run {i}"), "codex").await;
                engine.l4_service().start_run(&t.id, Trigger::Manual).await;
                tasks.push(t);
            }

            let mut running = 0;
            let mut waiting = 0;
            for t in &tasks {
                let stored = engine.l4.store.get(&t.id).await.unwrap().unwrap();
                if engine.l4.store.active_run(&t.id).await.unwrap().is_some() {
                    running += 1;
                    assert!(stored.slot_wait.is_none());
                } else {
                    waiting += 1;
                    let wait = stored.slot_wait.expect("a held task carries its wait");
                    assert_eq!(wait.agent, "codex");
                    assert_eq!(wait.scope, "demo");
                    assert_eq!(wait.trigger, Trigger::Manual);
                    assert_eq!(stored.status, TaskStatus::Pending, "waiting for a slot, not blocked on a person");
                    let entries = engine.l4.store.entries(&t.id, 10).await.unwrap();
                    assert!(
                        entries.iter().any(|e| e.kind == "capacity_held"),
                        "a held task journals why: {entries:?}"
                    );
                }
            }
            assert_eq!(running, 2, "the limit");
            assert_eq!(waiting, 2);
        }

        /// Finishing one of the two running tasks should wake the oldest
        /// waiting one, through the capacity-release channel, and carry its
        /// original queue-wait forward onto the run it finally gets.
        #[tokio::test]
        async fn capacity_finishing_a_run_admits_the_oldest_waiting_task_with_its_original_queued_at() {
            let engine = capacity_engine(vec![agent("codex", "shell", Some(1))], None);
            engine.spawn_capacity_release_worker();
            let first = task(&engine, "first", "codex").await;
            engine.l4_service().start_run(&first.id, Trigger::Manual).await;
            let second = task(&engine, "second", "codex").await;
            engine.l4_service().start_run(&second.id, Trigger::Manual).await;

            let waiting = engine.l4.store.get(&second.id).await.unwrap().unwrap();
            let original_queued_at = waiting.slot_wait.expect("held").queued_at;
            assert!(engine.l4.store.active_run(&second.id).await.unwrap().is_none());

            let mut bus = engine.shared.bus.subscribe();
            report_done(&engine, &first.id).await;

            // The release goes through a channel to a spawned worker; wait
            // for its effect instead of sleeping for it.
            let started = tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    if let Event::RunStarted { run } = bus.recv().await.unwrap() {
                        if run.task_id == second.id {
                            return run;
                        }
                    }
                }
            })
            .await
            .expect("the waiting task should be admitted once the slot frees");

            assert_eq!(started.queued_at, Some(original_queued_at), "the whole wait counts as queue wait");
            let settled = engine.l4.store.get(&second.id).await.unwrap().unwrap();
            assert!(settled.slot_wait.is_none(), "the wait is over");
        }

        /// The release channel is a convenience; the scheduler tick's own
        /// sweep must reach the same waiting task on its own, for a restart
        /// or a wakeup the channel dropped.
        #[tokio::test]
        async fn capacity_release_also_works_through_the_tick_sweep_alone() {
            let engine = capacity_engine(vec![agent("codex", "shell", Some(1))], None);
            // Deliberately never spawned: `recheck_capacity` is the only
            // thing that may admit the waiting task in this test.
            let first = task(&engine, "first", "codex").await;
            engine.l4_service().start_run(&first.id, Trigger::Manual).await;
            let second = task(&engine, "second", "codex").await;
            engine.l4_service().start_run(&second.id, Trigger::Manual).await;
            assert!(engine.l4.store.active_run(&second.id).await.unwrap().is_none());

            report_done(&engine, &first.id).await;
            assert!(
                engine.l4.store.active_run(&second.id).await.unwrap().is_none(),
                "nothing has swept yet"
            );

            engine.l4_service().recheck_capacity().await;
            assert!(engine.l4.store.active_run(&second.id).await.unwrap().is_some(), "the sweep admitted it");
            assert!(engine.l4.store.get(&second.id).await.unwrap().unwrap().slot_wait.is_none());
        }

        /// A release and the tick's own sweep both read `waiting_tasks()`
        /// and admit from it, and `dispatch` has no guard of its own against
        /// two overlapping admission attempts for the same waiting task --
        /// routing both through the one capacity-release worker, never onto
        /// separate spawned tasks, is what keeps them from both admitting
        /// the same waiter at once. A burst of releases and sweeps racing
        /// for the one freed slot must still open exactly one run.
        #[tokio::test]
        async fn capacity_a_release_and_a_sweep_racing_for_the_same_slot_never_both_admit_it() {
            let engine = capacity_engine(vec![agent("shell", "shell", Some(1))], None);
            engine.spawn_capacity_release_worker();
            let holder = task(&engine, "holder", "shell").await;
            engine.l4_service().start_run(&holder.id, Trigger::Manual).await;
            let waiter = task(&engine, "waiter", "shell").await;
            engine.l4_service().start_run(&waiter.id, Trigger::Manual).await;
            assert!(engine.l4.store.active_run(&waiter.id).await.unwrap().is_none());

            let mut bus = engine.shared.bus.subscribe();
            report_done(&engine, &holder.id).await;
            // A burst: before the fix, `release_waiting` and
            // `recheck_capacity` ran on separate spawned tasks and could
            // both pass admission for `waiter` before either cleared its
            // wait.
            for _ in 0..20 {
                engine.l4_service().enqueue_capacity_release("demo", "shell");
                engine.l4_service().enqueue_capacity_sweep();
            }

            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    if let Event::RunStarted { run } = bus.recv().await.unwrap() {
                        if run.task_id == waiter.id {
                            return;
                        }
                    }
                }
            })
            .await
            .expect("the waiter should be admitted once");

            let runs = engine.l4.store.runs(&waiter.id, 10).await.unwrap();
            assert_eq!(runs.len(), 1, "admitted exactly once, however many release/sweep events raced for it: {runs:?}");
        }

        /// A scope-wide cap holds across every agent in it, with no agent
        /// cap of its own on either.
        #[tokio::test]
        async fn capacity_a_scope_cap_holds_across_two_different_agents() {
            let engine = capacity_engine(
                vec![agent("codex", "shell", None), agent("claude", "shell", None)],
                Some(2),
            );
            let a1 = task(&engine, "a1", "codex").await;
            engine.l4_service().start_run(&a1.id, Trigger::Manual).await;
            let b1 = task(&engine, "b1", "claude").await;
            engine.l4_service().start_run(&b1.id, Trigger::Manual).await;
            let a2 = task(&engine, "a2", "codex").await;
            engine.l4_service().start_run(&a2.id, Trigger::Manual).await;

            assert!(engine.l4.store.active_run(&a1.id).await.unwrap().is_some());
            assert!(engine.l4.store.active_run(&b1.id).await.unwrap().is_some());
            assert!(engine.l4.store.active_run(&a2.id).await.unwrap().is_none(), "the scope is already at 2/2");
            let wait = engine.l4.store.get(&a2.id).await.unwrap().unwrap().slot_wait.unwrap();
            assert_eq!(wait.agent, "codex");
        }

        /// Every trigger dispatch can arrive on is held the same way, and
        /// carries through to the wait it leaves behind.
        #[tokio::test]
        async fn capacity_holds_a_task_however_it_was_triggered() {
            for trigger in [Trigger::Manual, Trigger::Schedule, Trigger::Retry, Trigger::Workflow, Trigger::Agent] {
                let engine = capacity_engine(vec![agent("codex", "shell", Some(1))], None);
                let holder = task(&engine, "holder", "codex").await;
                engine.l4_service().start_run(&holder.id, Trigger::Manual).await;
                let waiter = task(&engine, "waiter", "codex").await;

                let due = Due::now();
                engine.l4_service().start_run_due(&waiter.id, trigger, due).await;

                let stored = engine.l4.store.get(&waiter.id).await.unwrap().unwrap();
                let wait = stored.slot_wait.unwrap_or_else(|| panic!("{trigger:?} should have been held"));
                assert_eq!(wait.trigger, trigger);
                assert_eq!(stored.status, TaskStatus::Pending);
            }
        }

        /// A scheduled task that exhausted its retries is `Blocked` (with a
        /// `failure`), not `Pending` -- but its next regular slot still
        /// tries it again (`fires()`'s `blocked_by_failure` branch). If that
        /// attempt is held on capacity, the hold must force it out of
        /// `Blocked`: `waiting_tasks` only ever lists `Pending` rows, so a
        /// wait left sitting on `Blocked` would be invisible to every
        /// release and the tick sweep alike, and stay stuck forever.
        #[tokio::test]
        async fn capacity_a_task_blocked_by_a_failure_is_forced_pending_when_held_and_still_gets_released() {
            let engine = capacity_engine(vec![agent("shell", "shell", Some(1))], None);
            let failing = task(&engine, "exhausted its retries", "shell").await;
            let after_failure = engine.fail_task_for_test(&failing.id, FailKind::AgentFailed).await;
            assert_eq!(after_failure.status, TaskStatus::Blocked, "sanity: blocked by the failure, not pending");

            let holder = task(&engine, "holds the one slot", "shell").await;
            engine.l4_service().start_run(&holder.id, Trigger::Manual).await;
            assert!(engine.l4.store.active_run(&holder.id).await.unwrap().is_some());

            // Its next regular slot tries again, and is held.
            engine.l4_service().start_run_due(&failing.id, Trigger::Schedule, Due::now()).await;
            let stored = engine.l4.store.get(&failing.id).await.unwrap().unwrap();
            assert_eq!(stored.status, TaskStatus::Pending, "forced out of Blocked so it can be found and released");
            assert!(stored.slot_wait.is_some());

            // Free the slot; the tick sweep (not just a same-agent release)
            // must still find and admit it.
            report_done(&engine, &holder.id).await;
            engine.l4_service().recheck_capacity().await;
            assert!(
                engine.l4.store.active_run(&failing.id).await.unwrap().is_some(),
                "the previously-blocked task was admitted, not left waiting forever"
            );
        }

        /// `factory status`'s own reading: one row per agent that declares
        /// its own `max_sessions`, with the same in-use count admission
        /// uses and the waiting count alongside it.
        #[tokio::test]
        async fn capacity_shows_up_in_status_as_in_use_and_waiting_per_agent() {
            let engine = capacity_engine(vec![agent("codex", "shell", Some(2))], None);
            for i in 0..4 {
                let t = task(&engine, &format!("t{i}"), "codex").await;
                engine.l4_service().start_run(&t.id, Trigger::Manual).await;
            }
            let status = engine.status().await.unwrap();
            let row = status.capacity.iter().find(|r| r.agent == "codex").expect("a row for the declared cap");
            assert_eq!(row.scope, "demo");
            assert_eq!((row.in_use, row.max, row.waiting), (2, 2, 2));
        }

        /// Regression: `discovery::apply` folds the instance root's own
        /// `scope:` block into `config.scopes` and leaves `config.scope` set
        /// too (`Config::validate`'s own `self.scope.iter().chain(&self.scopes)`
        /// relies on exactly this to also cover its pre-discovery call). A
        /// runtime reader that chains the same way, after discovery has
        /// already run, counts the root scope's agents twice.
        #[tokio::test]
        async fn capacity_root_scope_is_not_counted_twice_after_discovery() {
            let engine = capacity_engine(vec![agent("codex", "shell", Some(2))], None);
            {
                let mut factory = engine.shared.factory.write().unwrap();
                let root = factory.config.scopes[0].clone();
                factory.config.scope = Some(root);
            }
            let status = engine.status().await.unwrap();
            assert_eq!(
                status.capacity.iter().filter(|r| r.agent == "codex").count(),
                1,
                "one row, not one per place the root scope's data is reachable from: {:?}",
                status.capacity
            );
        }

        /// A run this daemon is holding open on someone's answer, with no
        /// session of its own (the `#184` approval-hold shape) -- built
        /// directly, per the triage's own testing note, rather than
        /// dispatched -- must not consume a slot.
        #[tokio::test]
        async fn capacity_a_session_less_blocked_run_does_not_consume_a_slot() {
            let engine = capacity_engine(vec![agent("codex", "shell", Some(1))], None);
            let held = task(&engine, "approval held", "codex").await;
            let run = engine
                .l4.store
                .create_run(&NewRun {
                    task_id: held.id.clone(),
                    trigger: Trigger::Manual,
                    agent: "codex".into(),
                    adapter: "shell".into(),
                    runtime: "stub".into(),
                    token: "tok".into(),
                    queued_at: None,
                    scheduled_for: None,
                })
                .await
                .unwrap();
            // `Blocked`, no session: an approval hold, not a dispatch. It
            // must stay non-terminal so it still shows up in `active_runs`.
            engine
                .l4.store
                .update_run(&run.id, &RunPatch { status: Some(RunStatus::Blocked), ..Default::default() })
                .await
                .unwrap();
            assert!(engine.l4.store.active_run(&held.id).await.unwrap().is_some(), "still open, just not counted");

            let waiter = task(&engine, "waiter", "codex").await;
            engine.l4_service().start_run(&waiter.id, Trigger::Manual).await;
            assert!(
                engine.l4.store.active_run(&waiter.id).await.unwrap().is_some(),
                "the approval-held run left the one slot free"
            );
        }

        /// The atomicity guarantee itself: with the admission lock doing its
        /// job, ten dispatches racing for one slot never open more than one
        /// run between them, however they interleave.
        #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
        async fn capacity_ten_concurrent_dispatches_against_a_limit_of_one_never_open_more_than_one_run() {
            let engine = capacity_engine(vec![agent("codex", "shell", Some(1))], None);
            let mut tasks = Vec::new();
            for i in 0..10 {
                tasks.push(task(&engine, &format!("t{i}"), "codex").await);
            }
            let mut handles = Vec::new();
            for t in &tasks {
                let engine = engine.clone();
                let id = t.id.clone();
                handles.push(tokio::spawn(async move {
                    engine.l4_service().start_run(&id, Trigger::Manual).await;
                }));
            }
            for h in handles {
                h.await.unwrap();
            }

            let open = engine.l4.store.active_runs().await.unwrap();
            assert_eq!(open.len(), 1, "never more than the declared limit, however the dispatches interleaved");
            let mut waiting_count = 0;
            for t in &tasks {
                if engine.l4.store.get(&t.id).await.unwrap().unwrap().slot_wait.is_some() {
                    waiting_count += 1;
                }
            }
            assert_eq!(waiting_count, 9);
        }
    }

    #[tokio::test]
    async fn runtime_diagnostics_group_scopes_and_isolate_a_missing_adapter_as_data() {
        let scope_dir = temp_dir("runtime-diagnostic");
        let engine = test_engine(scope_dir.clone());
        {
            let mut factory = engine.shared.factory.write().unwrap();
            factory.config.scopes[0].runtime = Some("not-registered".into());
            let mut second = factory.config.scopes[0].clone();
            second.id = "second-id".into();
            second.name = "other".into();
            second.path = scope_dir.join("other");
            factory.config.scopes.push(second);
        }

        let views = engine.runtime_connections().await;
        assert_eq!(views.len(), 1, "one connection is probed once for both scopes");
        assert_eq!(views[0].runtime, "not-registered");
        assert_eq!(views[0].scopes, vec!["demo", "other"]);
        assert_eq!(views[0].source, "missing");
        assert_eq!(views[0].diagnostic.state, RuntimeConnectionState::Error);
        assert!(
            views[0]
                .diagnostic
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("not-registered"),
            "the card says which configured adapter is absent"
        );

        std::fs::remove_dir_all(scope_dir).ok();
    }

    /// `policy_chain` delegates to `Factory::policy_chain` on the live
    /// snapshot, the same way `roles_for` delegates for roles -- never a
    /// second, flat copy of the chain.
    #[tokio::test]
    async fn policy_chain_delegates_to_the_live_snapshot() {
        let scope_dir = temp_dir("policy-chain");
        let engine = test_engine(scope_dir.clone());
        {
            let mut factory = engine.shared.factory.write().unwrap();
            factory.config.policies = PolicyDeclaration {
                frameworks: vec!["cra".into()],
                ..Default::default()
            };
            factory.config.scopes[0].policies = PolicyDeclaration {
                frameworks: vec!["gdpr".into()],
                ..Default::default()
            };
        }

        let chain = engine.policy_chain("demo");
        assert_eq!(chain, engine.factory_snapshot().policy_chain("demo"));
        let scopes: Vec<&str> = chain.iter().map(|l| l.scope.as_str()).collect();
        assert_eq!(scopes, vec!["test", "demo"], "root layer, then the scope's own");

        std::fs::remove_dir_all(scope_dir).ok();
    }

    /// `infrastructure()` backs the L1 page: the binding rule applied to
    /// every agent the roster shows -- the synthesized foreman included --
    /// with `shell` in neither list, and the daemon's own facts. The host
    /// facts depend on the machine and are only checked for what every
    /// platform answers.
    #[tokio::test]
    async fn infrastructure_binds_every_model_agent_or_lists_it_unassigned() {
        let scope_dir = temp_dir("infrastructure");
        let engine = test_engine(scope_dir.clone());
        {
            let mut factory = engine.shared.factory.write().unwrap();
            factory.config.infrastructure = serde_yaml_ng::from_str(
                "providers:\n\
                 \x20 - name: claude-max\n    vendor: anthropic\n    kind: subscription\n    plan: Max 20x\n    harnesses: [claude-code]\n\
                 \x20 - name: openrouter\n    vendor: openrouter\n    kind: api-key\n    env: OPENROUTER_API_KEY\n    harnesses: [pi]\n",
            )
            .unwrap();
            factory.config.daemon.foreman.enabled = true;
            factory.config.daemon.foreman.harness = Some("claude-code".into());
            factory.config.scopes[0].agents = serde_yaml_ng::from_str(
                "- name: builder\n  harness: claude-code\n\
                 - name: model-lab\n  harness: opencode\n  provider: openrouter\n\
                 - name: helper\n  harness: codex\n\
                 - name: scripted\n  harness: shell\n",
            )
            .unwrap();
        }

        let Payload::Infrastructure { host, daemon, providers, unassigned, .. } = engine.infrastructure().await else {
            panic!("not an infrastructure payload");
        };

        let bound: Vec<(&str, Vec<(&str, &str)>)> = providers
            .iter()
            .map(|p| {
                (
                    p.name.as_str(),
                    p.agents.iter().map(|a| (a.agent.as_str(), a.via.as_str())).collect(),
                )
            })
            .collect();
        assert_eq!(
            bound,
            vec![
                ("claude-max", vec![("builder", "harness"), ("foreman", "harness")]),
                ("openrouter", vec![("model-lab", "agent")]),
            ]
        );
        assert_eq!(providers[1].env.as_deref(), Some("OPENROUTER_API_KEY"));
        assert_eq!(providers[0].plan.as_deref(), Some("Max 20x"));
        let unassigned: Vec<(&str, &str)> =
            unassigned.iter().map(|u| (u.agent.as_str(), u.harness.as_str())).collect();
        assert_eq!(unassigned, vec![("helper", "codex")], "shell is in neither list");

        assert_eq!(daemon.pid, std::process::id());
        assert_eq!(daemon.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(daemon.store.kind, "sqlite");
        assert_eq!(daemon.store.path, ".factory/factory.sqlite", "relative to the root");
        assert_eq!(daemon.store.size_bytes, None, "a database that is not there is null, not an error");
        // This test's root is deep enough in the temporary directory that the
        // socket falls back out of it -- and then the absolute path is shown.
        let socket = engine.factory_snapshot().socket_path();
        let root = engine.factory_snapshot().root;
        match socket.strip_prefix(&root) {
            Ok(rel) => assert_eq!(daemon.socket, rel.display().to_string()),
            Err(_) => assert_eq!(daemon.socket, socket.display().to_string()),
        }
        assert_eq!(daemon.runtime, "herdr");
        let interfaces: Vec<(&str, Option<&str>)> =
            daemon.interfaces.iter().map(|i| (i.kind.as_str(), i.bind.as_deref())).collect();
        assert_eq!(
            interfaces,
            vec![("cli", None), ("http", Some(factory_core::config::DEFAULT_HTTP_BIND))],
            "the default interfaces, with the address http falls back to"
        );
        assert!(daemon.started_at <= Utc::now());
        assert_eq!(host.arch.as_deref(), Some(std::env::consts::ARCH));

        std::fs::remove_dir_all(scope_dir).ok();
    }

    /// A subscription provider, a task, and `n` runs bound to it -- all
    /// started now, so every sample below is placed relative to that start.
    async fn provider_runs(engine: &Arc<Engine>, n: usize) -> (Vec<Run>, chrono::DateTime<Utc>) {
        {
            let mut factory = engine.shared.factory.write().unwrap();
            factory.config.infrastructure = serde_yaml_ng::from_str(
                "providers:\n\
                 \x20 - name: claude-max\n    vendor: anthropic\n    kind: subscription\n    harnesses: [claude-code]\n",
            )
            .unwrap();
        }
        let task = engine
            .create(NewTask {
                title: "plan share".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        let mut runs = Vec::new();
        for i in 0..n {
            let run = engine
                .l4.store
                .create_run(&factory_core::run::NewRun {
                    task_id: task.id.clone(),
                    trigger: factory_core::run::Trigger::Manual,
                    agent: "builder".into(),
                    adapter: "claude-code".into(),
                    runtime: "herdr".into(),
                    token: format!("token-{i}"),
                    queued_at: None,
                    scheduled_for: None,
                })
                .await
                .unwrap();
            let run = engine
                .l4.store
                .update_run(&run.id, &RunPatch { provider_account: Some("claude-max".into()), ..Default::default() })
                .await
                .unwrap();
            runs.push(run);
        }
        let start = runs.iter().map(|run| run.started_at).max().unwrap();
        (runs, start)
    }

    /// One reading of `run`, sampled `seconds` after `start`, with the
    /// harness's cumulative tokens and, if given, the 5-hour window's use.
    fn provider_snapshot(
        run: &Run,
        start: chrono::DateTime<Utc>,
        seconds: i64,
        tokens: u64,
        used_percent: Option<f64>,
    ) -> factory_core::usage::UsageSnapshot {
        use factory_core::usage::*;
        let sampled = start + chrono::Duration::seconds(seconds);
        let resets_at = (start + chrono::Duration::hours(5)).to_rfc3339();
        UsageSnapshot {
            run_id: run.id.clone(),
            task_id: run.task_id.clone(),
            point: if seconds <= 1 { SnapshotPoint::Dispatch } else { SnapshotPoint::TurnEnded },
            at: sampled,
            runtime: "herdr".into(),
            usage: Some(SessionUsage {
                schema: USAGE_SCHEMA,
                handle: None,
                sampled_at: Some(sampled),
                sessions: vec![HarnessUsage {
                    session_id: run.id.clone(),
                    adapter: Some("claude-code".into()),
                    model: None,
                    tokens: TokenCounts { input: Some(tokens), output: Some(0), cache_read: Some(0), cache_write: Some(0) },
                    cost: UsageCost::default(),
                    elapsed_seconds: None,
                    active_seconds: None,
                    subagents: Vec::new(),
                    rate_limit: used_percent.map(|used| RateLimit {
                        provider: Some("anthropic".into()),
                        plan_type: Some("max".into()),
                        attribution_quality: Some("confirmed".into()),
                        windows: vec![RateLimitWindow {
                            window_minutes: Some(300),
                            used_percent: Some(used),
                            resets_at: Some(resets_at.clone()),
                        }],
                    }),
                    unavailable: Default::default(),
                }],
            }),
            unknown: None,
        }
    }

    async fn five_hour_window(engine: &Arc<Engine>) -> ProviderWindow {
        let Payload::Infrastructure { providers, .. } = engine.infrastructure().await else {
            panic!("not an infrastructure payload");
        };
        let provider = providers.into_iter().find(|p| p.name == "claude-max").unwrap();
        assert_eq!(provider.windows.len(), 1, "{:?}", provider.windows);
        provider.windows.into_iter().next().unwrap()
    }

    /// A window read by one run and then by the next is one window moving:
    /// the trend spans the handoff, and the badge is the interval's.
    #[tokio::test]
    async fn a_provider_window_trend_spans_a_handoff_between_runs() {
        let scope_dir = temp_dir("provider-handoff");
        let engine = test_engine(scope_dir.clone());
        let (runs, start) = provider_runs(&engine, 2).await;
        let (a, b) = (&runs[0], &runs[1]);
        for snapshot in [
            provider_snapshot(a, start, 1, 0, Some(40.0)),
            provider_snapshot(b, start, 1, 0, None),
            provider_snapshot(a, start, 3, 50, Some(42.0)),
            provider_snapshot(b, start, 3, 0, None),
            provider_snapshot(b, start, 5, 100, Some(45.0)),
        ] {
            engine.l4.store.append_usage(&snapshot).await.unwrap();
        }
        engine
            .l4.store
            .update_run(&a.id, &RunPatch {
                status: Some(RunStatus::Done),
                ended_at: Some(start + chrono::Duration::seconds(3)),
                ..Default::default()
            })
            .await
            .unwrap();

        let window = five_hour_window(&engine).await;
        assert_eq!(window.used_percent, Some(45.0), "the newest sample, whichever run took it");
        assert_eq!(window.sampled_at, start + chrono::Duration::seconds(5));
        assert!((window.trend_percent.unwrap() - 3.0).abs() < 1e-9, "42 -> 45 across the handoff: {window:?}");
        assert_eq!(window.attribution, Some(factory_core::usage::PlanShareAttribution::Direct));
        assert_eq!(window.attribution_unknown, None);
        assert!(!window.sample_time_estimated);

        std::fs::remove_dir_all(scope_dir).ok();
    }

    /// An apportioned reading stays apportioned after one of its consumers
    /// ends: the badge is what the sampled interval said, not who is live.
    #[tokio::test]
    async fn an_apportioned_reading_keeps_its_badge_after_a_consumer_ends() {
        let scope_dir = temp_dir("provider-apportioned");
        let engine = test_engine(scope_dir.clone());
        let (runs, start) = provider_runs(&engine, 2).await;
        let (a, b) = (&runs[0], &runs[1]);
        for snapshot in [
            provider_snapshot(a, start, 1, 0, Some(10.0)),
            provider_snapshot(b, start, 1, 0, None),
            provider_snapshot(a, start, 4, 100, Some(14.0)),
            provider_snapshot(b, start, 4, 300, None),
        ] {
            engine.l4.store.append_usage(&snapshot).await.unwrap();
        }
        let apportioned = Some(factory_core::usage::PlanShareAttribution::Apportioned);
        assert_eq!(five_hour_window(&engine).await.attribution, apportioned);

        for run in [b, a] {
            engine
                .l4.store
                .update_run(&run.id, &RunPatch {
                    status: Some(RunStatus::Done),
                    ended_at: Some(start + chrono::Duration::seconds(6)),
                    ..Default::default()
                })
                .await
                .unwrap();
            let window = five_hour_window(&engine).await;
            assert_eq!(window.attribution, apportioned, "after {} ended: {window:?}", run.id);
        }

        std::fs::remove_dir_all(scope_dir).ok();
    }

    /// Two runs reading the window in one instant are one observation: the
    /// card shows the highest, as plan share does, whichever was stored last,
    /// and its badge is that reading's interval.
    #[tokio::test]
    async fn simultaneous_provider_readings_show_the_one_plan_share_used() {
        let scope_dir = temp_dir("provider-tie");
        let engine = test_engine(scope_dir.clone());
        let (runs, start) = provider_runs(&engine, 2).await;
        let (a, b) = (&runs[0], &runs[1]);
        for snapshot in [
            provider_snapshot(a, start, 1, 0, Some(10.0)),
            provider_snapshot(b, start, 1, 0, None),
            provider_snapshot(a, start, 4, 100, Some(13.0)),
            provider_snapshot(b, start, 4, 300, Some(12.0)),
        ] {
            engine.l4.store.append_usage(&snapshot).await.unwrap();
        }
        let window = five_hour_window(&engine).await;
        assert_eq!(window.used_percent, Some(13.0), "{window:?}");
        assert!((window.trend_percent.unwrap() - 3.0).abs() < 1e-9);
        assert_eq!(window.attribution, Some(factory_core::usage::PlanShareAttribution::Apportioned));

        std::fs::remove_dir_all(scope_dir).ok();
    }

    /// A reading is as fresh as its sample, not its request: a cached answer
    /// sampled forty minutes ago is stale however recently it was asked for,
    /// and one that does not say when it was sampled is never called fresh.
    #[tokio::test]
    async fn a_provider_reading_is_dated_by_its_sample_time() {
        let scope_dir = temp_dir("provider-stale");
        let engine = test_engine(scope_dir.clone());
        let (runs, start) = provider_runs(&engine, 1).await;
        let run = &runs[0];
        let mut cached = provider_snapshot(run, start, 1, 0, Some(20.0));
        let sampled = Utc::now() - chrono::Duration::minutes(40);
        cached.usage.as_mut().unwrap().sampled_at = Some(sampled);
        cached.at = Utc::now();
        engine.l4.store.append_usage(&cached).await.unwrap();

        let window = five_hour_window(&engine).await;
        assert_eq!(window.sampled_at, sampled);
        assert!(window.stale, "{window:?}");
        assert!(!window.sample_time_estimated);

        let mut undated = provider_snapshot(run, start, 2, 0, Some(21.0));
        undated.usage.as_mut().unwrap().sampled_at = None;
        undated.at = Utc::now();
        engine.l4.store.append_usage(&undated).await.unwrap();
        let window = five_hour_window(&engine).await;
        assert_eq!(window.used_percent, Some(21.0));
        assert_eq!(window.sampled_at, undated.at, "the request time stands in");
        assert!(window.sample_time_estimated);
        assert!(window.stale, "an undated reading is never vouched for as fresh");

        std::fs::remove_dir_all(scope_dir).ok();
    }

    /// `environment()` backs the L2 page: one sandbox row per scope/agent,
    /// reusing `scope_views()` rather than recomputing it, and one credential
    /// row per scope's `.env`. The ambient rows (`~/.claude/...` and friends)
    /// depend on `$HOME` and are not asserted here -- see the doc comment on
    /// `credential_inventory` -- only the scope's own `.env`, which this test
    /// fully controls, and the shape of the sandbox row.
    #[tokio::test]
    async fn environment_carries_a_declared_sandbox_and_the_scopes_env_presence() {
        let scope_dir = temp_dir("environment");
        let engine = test_engine(scope_dir.clone());
        {
            let mut factory = engine.shared.factory.write().unwrap();
            factory.config.scopes[0].agents.push(ScopeAgent {
                name: Some("boxed".into()),
                harness: "shell".into(),
                lifetime: Lifetime::Task,
                role: Role::default(),
                autostart: None,
                args: Vec::new(),
                sandbox: Sandbox::Docker,
                openshell: None,
                provider: None,
                max_sessions: None,
            });
        }

        let (sandboxes, credentials) = engine.environment().await.unwrap();

        let row = sandboxes
            .iter()
            .find(|r| r.agent == "boxed")
            .expect("the declared agent has a sandbox row");
        assert_eq!(row.scope, "demo");
        assert_eq!(row.harness, "shell");
        assert_eq!(row.sandbox, "docker");
        assert!(!row.enforced, "docker is declared only (#218)");
        assert!(!row.worktree_capable, "not a git repository");

        let env_row = credentials
            .iter()
            .find(|c| c.integration == "scope env")
            .expect("every configured scope gets a .env row");
        assert_eq!(env_row.label, "demo .env");
        assert!(
            !env_row.present,
            "nothing wrote one into this scratch scope"
        );
        // A `.env` belongs to the scope it sits in, so the page can narrow to
        // the rail's selection. The ambient rows deliberately carry no scope:
        // they sit outside every one and are reachable from all of them.
        assert_eq!(env_row.scope.as_deref(), Some("demo"));
        assert!(
            credentials
                .iter()
                .filter(|c| c.integration != "scope env")
                .all(|c| c.scope.is_none()),
            "a credential in the owner's home belongs to no scope"
        );

        std::fs::remove_dir_all(scope_dir).ok();
    }

    /// A scope registered on the instance root itself stores its path as `.`,
    /// so `scope_path` hands back `<root>/.` and the naive join prints
    /// `<root>/./.env` on the page. The path shown has to be the path a
    /// person would type.
    #[tokio::test]
    async fn a_scope_on_the_instance_root_gets_a_tidy_env_path() {
        let scope_dir = temp_dir("root-scope");
        let engine = test_engine(scope_dir.clone());
        {
            let mut factory = engine.shared.factory.write().unwrap();
            factory.root.clone_from(&scope_dir);
            factory.config.scopes[0].path = PathBuf::from(".");
        }

        let (_, credentials) = engine.environment().await.unwrap();

        let env_row = credentials
            .iter()
            .find(|c| c.integration == "scope env")
            .expect("the root scope still gets a .env row");
        assert_eq!(
            env_row.path,
            scope_dir.join(".env").display().to_string(),
            "the `.` component must not survive into what the page prints"
        );

        std::fs::remove_dir_all(scope_dir).ok();
    }

    /// `Request::Knowledge` reads `<factory root>/.factory/knowledge`, not
    /// the scope directory -- the vault is company-wide, not per-scope --
    /// and runs the walk in `spawn_blocking` rather than inline.
    #[tokio::test]
    async fn a_knowledge_request_indexes_the_factory_roots_vault() {
        let scope_dir = temp_dir("knowledge-request");
        let engine = test_engine(scope_dir.clone());
        let root = temp_dir("knowledge-request-root");
        {
            let mut factory = engine.shared.factory.write().unwrap();
            factory.root.clone_from(&root);
        }
        std::fs::create_dir_all(root.join(".factory/knowledge")).unwrap();
        std::fs::write(
            root.join(".factory/knowledge/page.md"),
            "---\ntitle: A Page\n---\nNo links.\n",
        )
        .unwrap();

        let response = engine.handle_request(Request::Knowledge).await;
        match response {
            Response::Ok { data: Payload::Knowledge { present, pages, .. } } => {
                assert!(present);
                assert_eq!(pages.len(), 1);
                assert_eq!(pages[0].id, "page");
            }
            other => panic!("expected a knowledge payload: {other:?}"),
        }

        std::fs::remove_dir_all(scope_dir).ok();
        std::fs::remove_dir_all(root).ok();
    }

    fn search(text: &str, limit: Option<usize>) -> Request {
        Request::KnowledgeSearch { text: text.into(), tags: Vec::new(), scope: None, limit }
    }

    fn engine_with_vault(name: &str) -> (Arc<Engine>, PathBuf, PathBuf) {
        let scope_dir = temp_dir(name);
        let engine = test_engine(scope_dir.clone());
        let root = temp_dir(&format!("{name}-root"));
        engine.shared.factory.write().unwrap().root.clone_from(&root);
        std::fs::create_dir_all(root.join(".factory/knowledge")).unwrap();
        (engine, scope_dir, root)
    }

    /// The whole pull path: the configured provider answers from the root's
    /// vault, the payload names the vault so a hit can be opened, no page
    /// text travels, and a page written a moment ago is found by the very
    /// next search -- nothing to restart, nothing to rebuild.
    #[tokio::test]
    async fn a_search_answers_from_the_vault_and_sees_a_page_written_just_before() {
        let (engine, scope_dir, root) = engine_with_vault("knowledge-search");
        std::fs::write(
            root.join(".factory/knowledge/acme.md"),
            "---\ntitle: Acme\ntags: [invoicing]\n---\nSECRET-BODY-TEXT\n",
        )
        .unwrap();

        match engine.handle_request(search("invoicing", None)).await {
            Response::Ok { data: Payload::KnowledgeHits { provider, vault, hits } } => {
                assert_eq!(provider, "keyword");
                assert_eq!(vault, root.join(".factory/knowledge").display().to_string());
                assert_eq!(hits.len(), 1);
                assert_eq!(hits[0].page, "acme");
                let wire = serde_json::to_string(&hits).unwrap();
                assert!(!wire.contains("SECRET-BODY-TEXT"), "no page text on the wire: {wire}");
            }
            other => panic!("expected hits: {other:?}"),
        }

        let written = engine
            .handle_request(Request::KnowledgeWriteFile {
                path: "billing.md".into(),
                overwrite: false,
                bytes: b"---\ntitle: Invoicing runbook\n---\nsteps\n".to_vec(),
            })
            .await;
        assert!(matches!(written, Response::Ok { .. }), "{written:?}");
        match engine.handle_request(search("invoicing", None)).await {
            Response::Ok { data: Payload::KnowledgeHits { hits, .. } } => {
                let ids: Vec<&str> = hits.iter().map(|h| h.page.as_str()).collect();
                assert_eq!(ids, ["acme", "billing"]);
            }
            other => panic!("expected hits: {other:?}"),
        }

        std::fs::remove_dir_all(scope_dir).ok();
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_search_with_nothing_to_look_for_is_refused_and_a_huge_limit_is_capped() {
        let (engine, scope_dir, root) = engine_with_vault("knowledge-search-limits");
        for i in 0..60 {
            std::fs::write(root.join(format!(".factory/knowledge/p{i:02}.md")), "#ops\n").unwrap();
        }

        match engine.handle_request(search("  ", None)).await {
            Response::Error { message, .. } => assert!(message.contains("nothing to search for"), "{message}"),
            other => panic!("expected a refusal: {other:?}"),
        }
        for (asked, got) in [(None, factory_core::adapter::knowledge::DEFAULT_SEARCH_LIMIT), (Some(1000), 50)] {
            match engine.handle_request(search("ops", asked)).await {
                Response::Ok { data: Payload::KnowledgeHits { hits, .. } } => assert_eq!(hits.len(), got),
                other => panic!("expected hits: {other:?}"),
            }
        }

        std::fs::remove_dir_all(scope_dir).ok();
        std::fs::remove_dir_all(root).ok();
    }

    async fn created(engine: &Arc<Engine>, new: NewTask) -> Task {
        match engine.handle_request(Request::TaskCreate(new)).await {
            Response::Ok { data: Payload::Task { task } } => task,
            other => panic!("expected a task: {other:?}"),
        }
    }

    /// Push, not pull: a task that asks for hints gets the pages its own
    /// title and instructions match, from its own scope, capped, with the
    /// vault to open them from -- and its journal records exactly which,
    /// in the run the hints were for.
    #[tokio::test]
    async fn a_task_that_asks_for_knowledge_hints_is_handed_them_and_the_journal_says_which() {
        let (engine, scope_dir, root) = engine_with_vault("knowledge-hints");
        std::fs::write(root.join(".factory/knowledge/acme.md"), "---\ntitle: Acme\ntags: [invoicing]\n---\nx\n").unwrap();
        for i in 0..8 {
            std::fs::write(root.join(format!(".factory/knowledge/q{i}.md")), "#quarterly\n").unwrap();
        }
        let task = created(
            &engine,
            NewTask {
                title: "Send the quarterly invoicing run".into(),
                agent: Some("shell".into()),
                knowledge_hints: true,
                ..Default::default()
            },
        )
        .await;
        assert!(task.knowledge_hints);

        let hints = engine.l4_service().knowledge_hints(&task, "run-1").await.expect("something matched");
        assert_eq!(hints.vault, root.join(".factory/knowledge").display().to_string());
        assert_eq!(hints.hits.len(), factory_core::adapter::knowledge::KNOWLEDGE_HINTS_LIMIT);
        assert_eq!(hints.hits[0].page, "acme", "tag matches all score alike, so page id decides");

        let entries = engine.l4.store.entries(&task.id, 50).await.unwrap();
        let entry = entries.iter().find(|e| e.kind == "knowledge").expect("journaled");
        assert_eq!(entry.run_id.as_deref(), Some("run-1"));
        assert!(entry.message.starts_with("handed 5 knowledge page(s) (keyword): acme, "), "{}", entry.message);
        assert_eq!(entry.data.as_ref().unwrap()["hits"].as_array().unwrap().len(), 5);

        std::fs::remove_dir_all(scope_dir).ok();
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_task_that_does_not_ask_gets_no_hints_and_no_journal_line() {
        let (engine, scope_dir, root) = engine_with_vault("knowledge-hints-off");
        std::fs::write(root.join(".factory/knowledge/acme.md"), "#invoicing\n").unwrap();
        let task = created(
            &engine,
            NewTask { title: "invoicing".into(), agent: Some("shell".into()), ..Default::default() },
        )
        .await;
        assert!(!task.knowledge_hints, "off unless asked for");
        assert!(engine.l4_service().knowledge_hints(&task, "run-1").await.is_none());
        let entries = engine.l4.store.entries(&task.id, 50).await.unwrap();
        assert!(!entries.iter().any(|e| e.kind == "knowledge"));

        // Turned on by an edit, it takes effect on the next run; a task whose
        // words match nothing says so rather than staying silent.
        let patch = TaskPatch { knowledge_hints: Some(true), title: Some("unrelated".into()), ..Default::default() };
        let task = match engine.handle_request(Request::TaskUpdate { id: task.id.clone(), patch, reason: None }).await {
            Response::Ok { data: Payload::Task { task } } => task,
            other => panic!("expected a task: {other:?}"),
        };
        assert!(task.knowledge_hints);
        assert!(engine.l4_service().knowledge_hints(&task, "run-2").await.is_none());
        let entries = engine.l4.store.entries(&task.id, 50).await.unwrap();
        let entry = entries.iter().find(|e| e.kind == "knowledge").expect("journaled");
        assert_eq!(entry.message, "no knowledge page matched (keyword)");

        std::fs::remove_dir_all(scope_dir).ok();
        std::fs::remove_dir_all(root).ok();
    }

    /// A provider that cannot answer never stops a run from starting.
    #[tokio::test]
    async fn a_failed_hint_search_is_journaled_and_the_run_goes_ahead_without() {
        let (engine, scope_dir, root) = engine_with_vault("knowledge-hints-broken");
        let task = created(
            &engine,
            NewTask { title: "anything".into(), agent: Some("shell".into()), knowledge_hints: true, ..Default::default() },
        )
        .await;
        // Unknown only after startup's check -- the case a hand edit to a
        // running instance's snapshot could still produce.
        engine.shared.factory.write().unwrap().config.daemon.knowledge_provider = "gone".into();
        assert!(engine.l4_service().knowledge_hints(&task, "run-1").await.is_none());
        let entries = engine.l4.store.entries(&task.id, 50).await.unwrap();
        let entry = entries.iter().find(|e| e.kind == "knowledge").expect("journaled");
        assert!(entry.message.starts_with("knowledge search failed"), "{}", entry.message);

        std::fs::remove_dir_all(scope_dir).ok();
        std::fs::remove_dir_all(root).ok();
    }

    /// The seam is real: another provider, registered under the configured
    /// name, answers instead -- and hears about every write -- with nothing
    /// in the engine, the protocol or the CLI knowing the difference.
    #[tokio::test]
    async fn the_configured_provider_is_the_one_asked_and_told() {
        struct Stub(std::sync::Mutex<Vec<String>>);
        #[async_trait::async_trait]
        impl factory_core::adapter::KnowledgeProvider for Stub {
            fn name(&self) -> &str {
                "stub"
            }
            async fn search(
                &self,
                _root: &std::path::Path,
                query: &factory_core::adapter::KnowledgeQuery,
            ) -> Result<Vec<factory_core::adapter::KnowledgeHit>> {
                Ok(vec![factory_core::adapter::KnowledgeHit {
                    page: format!("stub/{}", query.text),
                    title: "Stub".into(),
                    score: 1.0,
                    why: format!("scope: {}", query.scope.as_deref().unwrap_or("-")),
                }])
            }
            async fn changed(&self, _root: &std::path::Path, files: &[String]) -> Result<()> {
                self.0.lock().unwrap().extend(files.iter().cloned());
                Ok(())
            }
        }

        let (mut engine, scope_dir, root) = engine_with_vault("knowledge-search-stub");
        let stub = Arc::new(Stub(Default::default()));
        Arc::get_mut(&mut engine)
            .expect("nothing else holds the engine yet")
            .shared.registry
            .add_knowledge(stub.clone(), "test");
        engine.shared.factory.write().unwrap().config.daemon.knowledge_provider = "stub".into();

        // An agent that names no scope searches from its own.
        let caller = crate::access::Caller::Agent {
            scope: "projects/demo".into(),
            name: "w".into(),
            role: factory_core::role::Role::worker(),
            run_id: None,
        };
        let request = engine.bind_caller(&caller, search("anything", None));
        assert!(engine.authorize(&caller, &request).await.is_ok(), "searching is a read, open to a worker");
        match engine.dispatch_request(&caller, request).await.unwrap() {
            Payload::KnowledgeHits { provider, hits, .. } => {
                assert_eq!(provider, "stub");
                assert_eq!(hits[0].page, "stub/anything");
                assert_eq!(hits[0].why, "scope: projects/demo");
            }
            other => panic!("expected hits: {other:?}"),
        }

        engine
            .handle_request(Request::KnowledgeWriteFile {
                path: "new.md".into(),
                overwrite: false,
                bytes: b"hello\n".to_vec(),
            })
            .await;
        assert_eq!(*stub.0.lock().unwrap(), ["new.md"]);

        std::fs::remove_dir_all(scope_dir).ok();
        std::fs::remove_dir_all(root).ok();
    }

    /// The write requests need `Grant::KnowledgeWrite`, and even holding it
    /// is refused unless the caller's own scope *is* the instance root -- the
    /// knowledge base has no per-scope subject to check `reach` against.
    #[tokio::test]
    async fn knowledge_writes_need_the_grant_and_the_root_scope_both() {
        let scope_dir = temp_dir("knowledge-write-access");
        let engine = test_engine(scope_dir.clone());
        {
            let mut factory = engine.shared.factory.write().unwrap();
            factory.config.scopes[0].path = PathBuf::from(".");
            factory.config.scope = Some(factory.config.scopes[0].clone());
        }
        let root_scope_name = engine.factory_snapshot().config.scopes[0].name.clone();

        let request = Request::KnowledgeAdd {
            sources: vec!["/tmp/does-not-matter.md".into()],
            into: None,
            overwrite: false,
        };

        // A foreman (holds every grant, `Grant::ALL`) whose own scope is the
        // root may write.
        let root_foreman = crate::access::Caller::Agent {
            scope: root_scope_name.clone(),
            name: "boss".into(),
            role: factory_core::role::Role::foreman(),
            run_id: None,
        };
        assert!(engine.authorize(&root_foreman, &request).await.is_ok());

        // A foreman of a nested scope holds the same grant, but is not the
        // root -- and is refused for exactly that, not for lacking the grant.
        let nested_foreman = crate::access::Caller::Agent {
            scope: "nested".into(),
            name: "boss".into(),
            role: factory_core::role::Role::foreman(),
            run_id: None,
        };
        let err = engine.authorize(&nested_foreman, &request).await.unwrap_err().to_string();
        assert!(err.contains("root scope"), "{err}");

        // A worker in the root scope holds no write grant at all.
        let root_worker = crate::access::Caller::Agent {
            scope: root_scope_name,
            name: "w".into(),
            role: factory_core::role::Role::worker(),
            run_id: None,
        };
        assert!(engine.authorize(&root_worker, &request).await.is_err());

        std::fs::remove_dir_all(scope_dir).ok();
    }

    /// `Request::Benchmarks` derives configurations from the current config's
    /// scopes and its foreman settings, including a synthesized foreman.
    #[tokio::test]
    async fn a_benchmarks_request_lists_a_configuration_per_declared_agent() {
        let scope_dir = temp_dir("benchmarks-request");
        let engine = test_engine(scope_dir.clone());
        {
            let mut factory = engine.shared.factory.write().unwrap();
            factory.config.scopes[0].agents.push(ScopeAgent {
                name: Some("builder".into()),
                harness: "claude-code".into(),
                lifetime: Lifetime::Task,
                role: Role::default(),
                autostart: None,
                args: vec!["--model".into(), "opus".into(), "--api-key".into(), "s3cret".into()],
                sandbox: Sandbox::None,
                openshell: None,
                provider: None,
                max_sessions: None,
            });
            factory.config.daemon.foreman.enabled = true;
        }

        let response = engine.handle_request(Request::Benchmarks).await;
        match response {
            Response::Ok { data: Payload::Benchmarks { configurations } } => {
                let builder = configurations
                    .iter()
                    .find(|c| c.agents.iter().any(|a| a.agent == "builder"))
                    .expect("the declared agent gets a configuration");
                assert_eq!(builder.model.as_deref(), Some("opus"));
                assert!(!builder.pinned);
                let json = serde_json::to_string(&configurations).unwrap();
                assert!(!json.contains("s3cret"));

                assert!(
                    configurations.iter().any(|c| c.agents.iter().any(|a| a.agent == "foreman" && !a.declared)),
                    "the synthesized foreman is included: {configurations:?}"
                );
            }
            other => panic!("expected a benchmarks payload: {other:?}"),
        }

        std::fs::remove_dir_all(scope_dir).ok();
    }

    #[tokio::test]
    async fn a_run_whose_worktree_creation_fails_does_not_get_a_session_and_reports_gits_error() {
        // Not a git repository, so `git worktree add` has nothing to work
        // with. The checkbox defaults to on, so this is the ordinary case for
        // a scope nobody has run `git init` in yet -- exactly what a person
        // must never see silently turn into a run in the scope itself.
        let scope_dir = temp_dir("scope");
        let engine = test_engine(scope_dir.clone());

        let task = engine
            .create(NewTask {
                title: "try the worktree".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(task.worktree, "on by default, and this task never said otherwise");

        engine.l4_service().start_run(&task.id, Trigger::Manual).await;

        let runs = engine.l4.store.runs(&task.id, 10).await.unwrap();
        assert_eq!(runs.len(), 1, "the run row was made before the worktree was attempted");
        let run = &runs[0];
        assert_eq!(run.status, RunStatus::Failed);
        assert!(run.session.is_none(), "it never got as far as opening a session");
        assert!(run.worktree_path.is_none(), "nothing to record -- the worktree never existed");
        let error = run.error.clone().unwrap_or_default();
        assert!(
            error.contains("not a git repository"),
            "run.error should carry git's own complaint, got: {error:?}"
        );

        let entries = engine.l4.store.run_entries(&run.id, 50).await.unwrap();
        assert!(
            entries.iter().any(|e| e.message.contains("not a git repository")),
            "the journal gets git's complaint too"
        );

        // `start_run`'s own error handling ran this all the way through
        // `fail_run` -> `close_session`, so the assertion `dispatch` took
        // out for this run when its row was made must be back down to zero
        // by now -- see issue #61.
        assert_eq!(
            engine.l1.power.active_count().await,
            0,
            "a failed dispatch must not leave the power assertion held forever"
        );

        std::fs::remove_dir_all(&scope_dir).ok();
    }

    // -- power assertion, issue #61 ------------------------------------------

    /// `dispatch` itself -- called directly here, below `start_run`'s own
    /// error handling -- is where the assertion is acquired, right after the
    /// run's row exists. This proves that half of the wiring on its own:
    /// the companion test above proves the release half, once `start_run`'s
    /// failure handling has run `close_session` for it.
    #[tokio::test]
    async fn dispatch_acquires_the_power_assertion_as_soon_as_the_run_row_exists() {
        let scope_dir = temp_dir("power-acquire");
        let engine = test_engine(scope_dir.clone());

        let task = engine
            .create(NewTask {
                title: "try the worktree".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                ..Default::default()
            })
            .await
            .unwrap();

        assert_eq!(engine.l1.power.active_count().await, 0, "nothing has dispatched yet");

        // Not a git repository, so this fails inside `place_run` -- after
        // the run row (and so the acquire) but before a session. Calling
        // `dispatch` directly rather than through `start_run` is what lets
        // this test see the assertion still held: `start_run` would carry
        // the same error straight into `fail_run` and release it again.
        let err = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), None).await.unwrap_err();
        assert!(err.to_string().contains("not a git repository"), "got: {err}");

        assert_eq!(
            engine.l1.power.active_count().await,
            1,
            "the run row exists, so its share of the assertion must already be held"
        );

        std::fs::remove_dir_all(&scope_dir).ok();
    }

    // `blocked_since` promises to be `None` whenever the status is not
    // `Blocked`. The agent's own report honoured that; the daemon giving up on
    // a run did not, so a failed run kept saying it was still waiting for
    // somebody.
    #[tokio::test]
    async fn a_run_the_daemon_fails_out_of_a_block_stops_claiming_to_be_waiting() {
        let scope_dir = temp_dir("scope");
        let engine = test_engine(scope_dir.clone());

        let task = engine
            .create(NewTask {
                title: "asks a question and is given up on".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                worktree: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();

        let run = engine
            .l4.store
            .create_run(&factory_core::run::NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Manual,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: task.runtime.clone(),
                token: "tok".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();

        let blocked = engine
            .l4.store
            .update_run(
                &run.id,
                &RunPatch {
                    status: Some(RunStatus::Blocked),
                    blocked_since: Some(Utc::now()),
                    blocked_source: Some(BlockSource::Agent),
                    block_suspected_since: Some(Utc::now()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(blocked.blocked_since.is_some(), "the block is on before we fail it");

        engine.l4_service().fail_run(&run.id, FailKind::AgentFailed, "nobody ever answered").await;

        let failed = engine.l4.store.get_run(&run.id).await.unwrap().unwrap();
        assert_eq!(failed.status, RunStatus::Failed);
        assert!(failed.blocked_since.is_none(), "a finished run is not still waiting");
        assert!(failed.blocked_source.is_none(), "and nobody is holding it");
        assert!(
            failed.block_suspected_since.is_none(),
            "a guess about a session that is gone is not worth keeping either"
        );
    }

    // -- cron timezones --------------------------------------------------------

    fn berlin_monday_nine() -> Schedule {
        Schedule::Cron(factory_core::task::CronSchedule {
            expr: "0 9 * * 1".into(),
            timezone: Some("Europe/Berlin".into()),
        })
    }

    fn scheduled(schedule: Schedule) -> NewTask {
        NewTask {
            title: "the weekly audit".into(),
            instructions: "true".into(),
            scope: Some("demo".into()),
            agent: Some("shell".into()),
            worktree: Some(false),
            schedule: Some(schedule),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_task_with_a_zoned_schedule_is_next_due_on_that_wall_clock() {
        let engine = test_engine(temp_dir("tz-create"));
        let task = engine.create(scheduled(berlin_monday_nine())).await.unwrap();
        let next = task.next_run_at.unwrap().with_timezone(&chrono_tz::Europe::Berlin);
        assert_eq!(next.format("%a %H:%M").to_string(), "Mon 09:00", "{next}");
    }

    #[tokio::test]
    async fn a_misspelt_timezone_is_refused_when_the_schedule_is_set() {
        let engine = test_engine(temp_dir("tz-typo"));
        let typo = Schedule::Cron(factory_core::task::CronSchedule {
            expr: "0 9 * * 1".into(),
            timezone: Some("Europe/Berln".into()),
        });
        let err = engine.create(scheduled(typo.clone())).await.unwrap_err().to_string();
        assert!(err.contains("Europe/Berln"), "{err}");

        // And by an edit, which leaves the task as it was.
        let task = engine.create(scheduled(Schedule::Cron("0 7 * * 1".into()))).await.unwrap();
        let err = engine
            .update(&task.id, TaskPatch { schedule: Some(typo), ..Default::default() }, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("Europe/Berln"), "{err}");
        let unchanged = engine.l4.store.get(&task.id).await.unwrap().unwrap();
        assert_eq!(unchanged.schedule, Some(Schedule::Cron("0 7 * * 1".into())));
    }

    #[tokio::test]
    async fn editing_a_utc_schedule_into_a_zoned_one_moves_its_next_firing() {
        // The live audit's fix, in miniature: 07:00 UTC becomes 09:00 Berlin.
        let engine = test_engine(temp_dir("tz-edit"));
        let task = engine.create(scheduled(Schedule::Cron("0 7 * * 1".into()))).await.unwrap();
        let edited = engine
            .update(&task.id, TaskPatch { schedule: Some(berlin_monday_nine()), ..Default::default() }, None)
            .await
            .unwrap();
        assert_eq!(edited.schedule, Some(berlin_monday_nine()));
        let next = edited.next_run_at.unwrap().with_timezone(&chrono_tz::Europe::Berlin);
        assert_eq!(next.format("%a %H:%M").to_string(), "Mon 09:00", "{next}");
    }

    /// A weekly-scheduled task, freshly created -- `engine.create()` has
    /// already set `next_run_at` to the coming Monday, exactly as
    /// `advance_schedule` would before a real dispatch.
    async fn weekly_task(engine: &Engine, retry: Option<RetryPolicy>) -> Task {
        engine
            .create(NewTask {
                title: "the weekly audit".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                worktree: Some(false),
                schedule: Some(Schedule::Cron("0 7 * * 1".into())),
                retry,
                ..Default::default()
            })
            .await
            .unwrap()
    }

    /// A fresh run row for a task, as `dispatch` would leave one right before
    /// handing it to an agent -- enough for `fail_run`/`finish_run` to act on
    /// without going through a real runtime.
    async fn run_for(engine: &Engine, task_id: &str, trigger: Trigger) -> Run {
        engine
            .l4.store
            .create_run(&factory_core::run::NewRun {
                task_id: task_id.to_string(),
                trigger,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: "herdr".into(),
                token: "tok".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap()
    }

    // -- a harness's turn-end hook (issue #69) --------------------------------

    fn stop(token: Option<&str>, pending_background: u32) -> factory_core::task::TurnEnded {
        factory_core::task::TurnEnded {
            event: factory_core::task::TurnEndEvent::Stop,
            pending_background,
            error: None,
            error_details: None,
            last_message: Some("I think that is everything.".into()),
            token: token.map(str::to_string),
            session_id: None,
        }
    }

    /// A one-off task with a run the agent has said `running` on, as the
    /// daemon sees it mid-turn.
    async fn running_run(engine: &Engine, status: RunStatus) -> (Task, Run) {
        let task = engine
            .create(NewTask {
                title: "a turn that ends quietly".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                worktree: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        let run = run_for(engine, &task.id, Trigger::Manual).await;
        let run = engine
            .l4.store
            .update_run(&run.id, &RunPatch { status: Some(status), ..Default::default() })
            .await
            .unwrap();
        (task, run)
    }

    /// Through `handle`, the way the CLI arrives: envelope, caller, authorize,
    /// dispatch. The run token doubles as the caller's identity token, exactly
    /// as `FACTORY_TOKEN` and `FACTORY_TASK_TOKEN` do in a real session.
    async fn send_turn_end(engine: &Arc<Engine>, task_id: &str, turn: factory_core::task::TurnEnded) -> Response {
        engine
            .handle(Envelope {
                token: turn.token.clone(),
                request: Request::TaskTurnEnded { id: task_id.to_string(), turn },
            })
            .await
    }

    #[tokio::test]
    async fn a_stop_failure_with_no_report_before_it_fails_the_run_on_the_spot() {
        let engine = test_engine(temp_dir("turn-end"));
        let (task, run) = running_run(&engine, RunStatus::Running).await;
        let mut turn = stop(Some("tok"), 0);
        turn.event = factory_core::task::TurnEndEvent::StopFailure;
        turn.error = Some("server_error".into());

        let response = send_turn_end(&engine, &task.id, turn).await;
        assert!(matches!(response, Response::Ok { .. }), "{response:?}");

        let failed = engine.l4.store.get_run(&run.id).await.unwrap().unwrap();
        assert_eq!(failed.status, RunStatus::Failed, "not left for the timeout to find");
        let error = failed.error.unwrap();
        assert!(error.contains("API error (server_error)"), "{error}");
        assert!(error.contains("I think that is everything."), "{error}");
        let entries = engine.l4.store.entries(&task.id, 50).await.unwrap();
        assert!(
            entries.iter().any(|e| e.source == "daemon" && e.kind == "failed"),
            "the daemon ended it, and says so -- it is never journalled as the agent's report"
        );
        assert!(!entries.iter().any(|e| e.source == "agent"), "{entries:?}");
    }

    #[tokio::test]
    async fn a_stop_is_held_on_the_run_and_the_agents_next_report_lets_it_go() {
        // Another Stop hook may have kept the turn going, so a Stop fails
        // nothing by itself: it is written down for a later liveness tick.
        let engine = test_engine(temp_dir("turn-end-held"));
        let (task, run) = running_run(&engine, RunStatus::Running).await;

        let response = send_turn_end(&engine, &task.id, stop(Some("tok"), 0)).await;
        assert!(matches!(response, Response::Ok { .. }), "{response:?}");
        let held = engine.l4.store.get_run(&run.id).await.unwrap().unwrap();
        assert_eq!(held.status, RunStatus::Running, "still running");
        assert!(held.turn_ended_at.is_some());
        let why = held.turn_end_reason.as_deref().unwrap();
        assert!(why.contains("turn ended without reporting") && why.contains("Stop hook"), "{why}");

        engine
            .l4_service()
            .report(
                &task.id,
                TaskReport {
                    artifacts: Vec::new(),
                    status: None,
                    message: Some("the hook was wrong, I am still here".into()),
                    result: None,
                    send_to: None,
                    error: None,
                    token: Some("tok".into()),
                },
            )
            .await
            .unwrap();
        let resumed = engine.l4.store.get_run(&run.id).await.unwrap().unwrap();
        assert!(resumed.turn_ended_at.is_none(), "the agent talking is a turn that did not end");
        assert!(resumed.turn_end_reason.is_none());
    }

    #[tokio::test]
    async fn a_run_that_ends_takes_its_held_stop_with_it() {
        let engine = test_engine(temp_dir("turn-end-finish"));
        let (task, run) = running_run(&engine, RunStatus::Running).await;
        send_turn_end(&engine, &task.id, stop(Some("tok"), 0)).await;

        engine.l4_service().fail_run(&run.id, FailKind::AgentFailed, "for some other reason").await;
        let failed = engine.l4.store.get_run(&run.id).await.unwrap().unwrap();
        assert!(failed.turn_ended_at.is_none(), "nothing left to settle on a finished run");
        assert!(failed.turn_end_reason.is_none());
        assert_eq!(failed.error.as_deref(), Some("for some other reason"));
    }

    #[tokio::test]
    async fn a_turn_end_without_the_runs_token_changes_nothing() {
        let engine = test_engine(temp_dir("turn-end-token"));
        let (task, run) = running_run(&engine, RunStatus::Running).await;

        for token in [Some("not-it"), None] {
            let response = send_turn_end(&engine, &task.id, stop(token, 0)).await;
            assert!(matches!(response, Response::Error { .. }), "{token:?}: {response:?}");
            let still = engine.l4.store.get_run(&run.id).await.unwrap().unwrap();
            assert_eq!(still.status, RunStatus::Running, "{token:?}");
        }
    }

    #[tokio::test]
    async fn the_stop_after_a_done_report_changes_nothing() {
        // The hook fires at the end of every turn, the successful one
        // included. By then the run is over and its token cleared, so the
        // envelope's identity is refused before anything else looks at it --
        // which the CLI swallows into the harness's debug log -- and even a
        // caller whose identity is not in question (the owner, below) finds
        // no run in progress and is answered with nothing done.
        let engine = test_engine(temp_dir("turn-end-done"));
        let (task, run) = running_run(&engine, RunStatus::Running).await;
        engine
            .l4_service()
            .report(
                &task.id,
                TaskReport {
                    artifacts: Vec::new(),
                    status: Some(RunStatus::Done),
                    result: Some("did it".into()),
                    send_to: None,
                    token: Some("tok".into()),
                    message: None,
                    error: None,
                },
            )
            .await
            .unwrap();
        let before = engine.l4.store.entries(&task.id, 50).await.unwrap().len();

        // Pinned rather than narrated: this is what every successful
        // claude-code run's last Stop hook actually gets back.
        match send_turn_end(&engine, &task.id, stop(Some("tok"), 0)).await {
            Response::Error { code, .. } => assert_eq!(code, "denied"),
            other => panic!("a finished run's token is nobody's: {other:?}"),
        }
        engine.turn_ended(&task.id, stop(Some("tok"), 0)).await.unwrap();

        let done = engine.l4.store.get_run(&run.id).await.unwrap().unwrap();
        assert_eq!(done.status, RunStatus::Done);
        assert!(done.error.is_none());
        assert_eq!(engine.l4.store.entries(&task.id, 50).await.unwrap().len(), before, "nothing journalled");
    }

    #[tokio::test]
    async fn a_blocked_run_or_one_with_background_work_is_left_to_go_on() {
        let engine = test_engine(temp_dir("turn-end-waiting"));

        let (task, run) = running_run(&engine, RunStatus::Blocked).await;
        send_turn_end(&engine, &task.id, stop(Some("tok"), 0)).await;
        let still = engine.l4.store.get_run(&run.id).await.unwrap().unwrap();
        assert_eq!(still.status, RunStatus::Blocked, "it asked for a human and is waiting for one");
        assert!(still.turn_ended_at.is_none(), "and nothing is held against it");

        let (task, run) = running_run(&engine, RunStatus::Running).await;
        send_turn_end(&engine, &task.id, stop(Some("tok"), 2)).await;
        let still = engine.l4.store.get_run(&run.id).await.unwrap().unwrap();
        assert_eq!(still.status, RunStatus::Running, "the harness will wake it again");
        assert!(still.turn_ended_at.is_none(), "a paused turn is not held as an ended one");
    }

    #[tokio::test]
    async fn a_failed_scheduled_run_queues_a_retry_without_disturbing_the_regular_firing() {
        let scope_dir = temp_dir("retry-queue");
        let engine = test_engine(scope_dir.clone());
        // The instance default (3 attempts, 5 minutes apart) applies: the
        // task names no policy of its own.
        let task = weekly_task(&engine, None).await;
        let regular_next_run = task.next_run_at.expect("a schedule always sets one");
        let run = run_for(&engine, &task.id, Trigger::Schedule).await;

        engine.l4_service().fail_run(&run.id, FailKind::AgentFailed, "the audit script exited 1").await;

        let task = engine.l4.store.get(&task.id).await.unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Pending, "recurring tasks go back to pending");
        assert_eq!(
            task.error.as_deref(),
            Some("the audit script exited 1"),
            "the mirror still shows what just happened -- there is no successful attempt yet to clear it"
        );
        let pending = task.pending_retry.expect("the default policy allows a retry");
        assert_eq!(pending.attempts, 1);
        assert_eq!(
            pending.resume_at, regular_next_run,
            "the regular firing `advance_schedule` set before dispatch is carried forward untouched"
        );
        let next_run_at = task.next_run_at.expect("a retry is queued");
        assert!(
            next_run_at < regular_next_run,
            "the retry fires well before next Monday, not on it"
        );
        assert!(
            (next_run_at - Utc::now()).num_seconds() <= 300 + 5,
            "the default backoff is five minutes, so the retry should be due within that window"
        );

        let entries = engine.l4.store.entries(&task.id, 20).await.unwrap();
        assert!(
            entries.iter().any(|e| e.kind == "retrying" && e.message.contains("retry 1 of 3")),
            "the retry is visible in the journal without reading the daemon log: {entries:?}"
        );
        assert!(
            !entries.iter().any(|e| e.kind == "rearmed"),
            "a task about to retry in five minutes must not also claim it is \"waiting for its \
             next turn\" -- `mirror_to_task` defers that line to `queue_or_end_retry` for exactly \
             this reason: {entries:?}"
        );
    }

    /// The exact bug `AGENTS.md` warns about: "if you add a field to that
    /// mirror, clear it too, or a successful retry will show the previous
    /// attempt's error." This drives a failure through to a queued retry and
    /// then a successful one, and checks that nothing about the failed
    /// attempt survives on the task once the retry lands.
    #[tokio::test]
    async fn a_successful_retry_clears_the_stale_error_and_restores_the_regular_firing() {
        let scope_dir = temp_dir("retry-clears-error");
        let engine = test_engine(scope_dir.clone());
        let task = weekly_task(&engine, None).await;
        let regular_next_run = task.next_run_at.unwrap();

        let first = run_for(&engine, &task.id, Trigger::Schedule).await;
        engine.l4_service().fail_run(&first.id, FailKind::AgentFailed, "the audit script exited 1").await;
        let mid = engine.l4.store.get(&task.id).await.unwrap().unwrap();
        assert!(mid.error.is_some(), "sanity: the failure is on the mirror before the retry runs");
        assert!(mid.pending_retry.is_some(), "sanity: a retry is queued before it runs");
        assert_eq!(mid.status, TaskStatus::Pending, "a queued retry keeps the task scheduled (#122)");
        assert!(mid.failure.is_some(), "and it says what it is retrying after");

        // The scheduler's own retry pickup (`resume_from_retry`) would run
        // here in production; a fresh run row is all `finish_run` needs.
        let retry = run_for(&engine, &task.id, Trigger::Retry).await;
        engine
            .l4_service()
            .finish_run(
                &retry.id,
                RunStatus::Done,
                RunPatch { result: Some("all clear".into()), ..Default::default() },
                "attempt ended",
            )
            .await
            .unwrap();

        let task = engine.l4.store.get(&task.id).await.unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Pending);
        assert_eq!(task.result.as_deref(), Some("all clear"));
        assert!(
            task.error.is_none(),
            "a successful retry must not leave the previous attempt's error standing -- got {:?}",
            task.error
        );
        assert!(
            task.pending_retry.is_none(),
            "the streak is over; a stale `pending_retry` would misreport this task as still mid-retry"
        );
        assert!(task.failure.is_none(), "nor may the failure it retried after outlive it (#122)");
        assert_eq!(
            task.next_run_at,
            Some(regular_next_run),
            "the regular firing the streak was standing in front of is restored exactly"
        );
    }

    #[tokio::test]
    async fn a_streak_that_uses_up_its_attempts_stops_retrying_and_falls_back_to_the_regular_firing() {
        let scope_dir = temp_dir("retry-exhausted");
        let engine = test_engine(scope_dir.clone());
        let task = weekly_task(
            &engine,
            Some(RetryPolicy::Backoff { max_attempts: 1, backoff_seconds: 60 }),
        )
        .await;
        let regular_next_run = task.next_run_at.unwrap();

        let first = run_for(&engine, &task.id, Trigger::Schedule).await;
        engine.l4_service().fail_run(&first.id, FailKind::AgentFailed, "attempt 1 failed").await;
        let mid = engine.l4.store.get(&task.id).await.unwrap().unwrap();
        assert_eq!(mid.pending_retry.map(|p| p.attempts), Some(1), "the one allowed retry is queued");

        let retry = run_for(&engine, &task.id, Trigger::Retry).await;
        engine.l4_service().fail_run(&retry.id, FailKind::AgentFailed, "attempt 2 failed too").await;

        let task = engine.l4.store.get(&task.id).await.unwrap().unwrap();
        assert!(
            task.pending_retry.is_none(),
            "the single allowed attempt is used up, so the streak ends here"
        );
        assert_eq!(
            task.next_run_at,
            Some(regular_next_run),
            "no more retries -- the task falls back to its next real firing"
        );
        assert_eq!(
            task.error.as_deref(),
            Some("attempt 2 failed too"),
            "the task honestly shows the last thing that happened; nothing to clear here since nothing succeeded"
        );

        assert_eq!(
            task.status,
            TaskStatus::Blocked,
            "retries exhausted: blocked on the failure, never back in Scheduled looking healthy (#122)"
        );
        assert_eq!(task.failure.as_ref().and_then(|f| f.run_id.clone()), Some(retry.id.clone()));

        let entries = engine.l4.store.entries(&task.id, 20).await.unwrap();
        assert!(
            entries.iter().any(|e| e.kind == "retry_settled" && e.message.contains("exhausted")),
            "exhausting the policy is visible in the journal too: {entries:?}"
        );
        assert!(entries.iter().any(|e| e.kind == "blocked_on_failure"), "{entries:?}");
    }

    #[tokio::test]
    async fn a_task_with_retry_none_is_never_retried_matching_todays_behaviour() {
        let scope_dir = temp_dir("retry-none");
        let engine = test_engine(scope_dir.clone());
        let task = weekly_task(&engine, Some(RetryPolicy::None)).await;
        let regular_next_run = task.next_run_at.unwrap();

        let run = run_for(&engine, &task.id, Trigger::Schedule).await;
        engine.l4_service().fail_run(&run.id, FailKind::AgentFailed, "boom").await;

        let task = engine.l4.store.get(&task.id).await.unwrap().unwrap();
        assert!(task.pending_retry.is_none(), "`retry: none` queues nothing");
        assert_eq!(
            task.next_run_at,
            Some(regular_next_run),
            "the schedule is exactly as `advance_schedule` left it before dispatch -- untouched"
        );
        assert_eq!(task.error.as_deref(), Some("boom"));

        // Nothing will try again before the next regular firing, so the
        // task is blocked on the failure (`#122`) rather than sitting in
        // Scheduled looking healthy -- and still due at that firing.
        assert_eq!(task.status, TaskStatus::Blocked);
        assert!(task.blocked_by_failure());
        assert_eq!(task.failure.as_ref().and_then(|f| f.kind), Some(FailKind::AgentFailed));
        assert_eq!(task.failure.as_ref().and_then(|f| f.run_id.clone()), Some(run.id.clone()));

        // `mirror_to_task` stays silent on a `Failed` recurring task so it
        // never contradicts a retry that might follow -- for a policy that
        // never retries at all, `queue_or_end_retry` is the one that has to
        // say what happens next.
        let entries = engine.l4.store.entries(&task.id, 20).await.unwrap();
        assert!(
            entries.iter().any(|e| e.kind == "blocked_on_failure" && e.message.contains("`none`")),
            "a task that will never retry still needs a line saying what happens next: {entries:?}"
        );
        assert!(!entries.iter().any(|e| e.kind == "rearmed"), "it is not rearmed as if nothing happened");
    }

    #[tokio::test]
    async fn creating_a_retry_policy_without_a_schedule_is_refused() {
        let scope_dir = temp_dir("retry-needs-schedule");
        let engine = test_engine(scope_dir.clone());

        let err = engine
            .create(NewTask {
                title: "a one-off with a retry policy that would never apply".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                worktree: Some(false),
                retry: Some(RetryPolicy::Backoff { max_attempts: 3, backoff_seconds: 60 }),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("scheduled task"), "{err}");
    }

    #[tokio::test]
    async fn editing_in_a_retry_policy_while_clearing_the_schedule_in_the_same_edit_is_refused() {
        let scope_dir = temp_dir("retry-needs-schedule-edit");
        let engine = test_engine(scope_dir.clone());
        let task = weekly_task(&engine, None).await;

        let err = engine
            .update(
                &task.id,
                TaskPatch {
                    clear_schedule: true,
                    retry: Some(RetryPolicy::Backoff { max_attempts: 3, backoff_seconds: 60 }),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("scheduled task"), "{err}");
    }

    #[tokio::test]
    async fn resume_from_retry_restores_the_captured_slot_rather_than_recomputing_it() {
        // The whole reason `resume_from_retry` exists instead of reusing
        // `advance_schedule`: for an `Every` schedule, recomputing from `now`
        // would push the regular firing out every time a retry is picked up.
        // `resume_at` must come back exactly as captured, however much later
        // `now` has drifted.
        let scope_dir = temp_dir("resume-from-retry");
        let engine = test_engine(scope_dir.clone());
        let task = engine
            .create(NewTask {
                title: "a tight interval".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                worktree: Some(false),
                schedule: Some(Schedule::Every { seconds: 60 }),
                ..Default::default()
            })
            .await
            .unwrap();

        let captured_resume_at = Utc::now() + chrono::Duration::hours(3);
        let task = engine
            .l4.store
            .update(
                &task.id,
                &TaskPatch {
                    pending_retry: Some(PendingRetry { attempts: 1, resume_at: captured_resume_at }),
                    next_run_at: Some(Utc::now()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        engine.l4_service().resume_from_retry(&task).await.unwrap();

        let task = engine.l4.store.get(&task.id).await.unwrap().unwrap();
        assert_eq!(
            task.next_run_at,
            Some(captured_resume_at),
            "the captured slot comes back exactly, not `now + 60s`"
        );
    }

    #[tokio::test]
    async fn a_task_with_the_checkbox_off_still_runs_in_the_scope_even_when_it_is_not_a_git_repository() {
        // `place_run` is called directly rather than through `dispatch`,
        // which would go on to call a real runtime -- this machine actually
        // has herdr installed, and a unit test has no business starting a
        // real session. `place_run` is the whole of the decision `dispatch`
        // makes here, so exercising it alone is exercising the real thing.
        let scope_dir = temp_dir("scope"); // not a git repository, on purpose
        let engine = test_engine(scope_dir.clone());

        let task = engine
            .create(NewTask {
                title: "stay in the scope".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                worktree: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(!task.worktree);

        let run = engine
            .l4.store
            .create_run(&factory_core::run::NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Manual,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: task.runtime.clone(),
                token: "tok".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();

        let (cwd, run) = engine.l4_service().place_run(&task, run, &scope_dir, Workspace::Fresh).await.unwrap();
        assert_eq!(cwd, scope_dir, "the checkbox is off, so this stays the scope itself");
        assert!(run.worktree_path.is_none());
        assert!(run.worktree_branch.is_none());
    }

    /// `scope_views` asks `git` per configured scope, potentially many
    /// subprocesses on an endpoint the Agents view refetches on every event.
    /// The answer is cached, so the second board
    /// within `CAPABILITY_TTL` runs no `git` at all -- observed here as the
    /// cached answer surviving a change on disk that would flip it.
    #[tokio::test]
    async fn a_scopes_worktree_capability_is_asked_of_git_once_per_ttl() {
        let scope_dir = temp_dir("caps");
        let engine = test_engine(scope_dir.clone());

        // No repository yet, so the first board says so.
        let (first, _) = engine.scope_views().await.unwrap();
        assert_eq!(first[0].id, "scope-id", "the scope identity reaches the API view");
        assert!(!first[0].worktree_capable);

        for args in [
            &["init", "-q"][..],
            &["config", "user.email", "factory@example.com"][..],
            &["config", "user.name", "factory"][..],
            &["commit", "-q", "--allow-empty", "-m", "base"][..],
        ] {
            assert!(std::process::Command::new("git")
                .args(args)
                .current_dir(&scope_dir)
                .status()
                .unwrap()
                .success());
        }

        let (second, _) = engine.scope_views().await.unwrap();
        assert!(
            !second[0].worktree_capable,
            "within the TTL the cached answer stands, git is not asked again"
        );

        engine.l4.worktree_caps.lock().unwrap().clear();
        let (third, _) = engine.scope_views().await.unwrap();
        assert!(third[0].worktree_capable, "once stale, git is asked and sees the repository");
    }

    #[tokio::test]
    async fn a_capable_scope_gives_the_run_its_own_worktree_and_the_run_remembers_where() {
        let scope_dir = temp_dir("scope");
        for args in [
            &["init", "-q"][..],
            &["config", "user.email", "factory@example.com"][..],
            &["config", "user.name", "factory"][..],
            &["commit", "-q", "--allow-empty", "-m", "base"][..],
        ] {
            assert!(std::process::Command::new("git")
                .args(args)
                .current_dir(&scope_dir)
                .status()
                .unwrap()
                .success());
        }

        let engine = test_engine(scope_dir.clone());
        let root = engine.factory_snapshot().root.clone();
        let task = engine
            .create(NewTask {
                title: "do the thing".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                ..Default::default()
            })
            .await
            .unwrap();

        let run = engine
            .l4.store
            .create_run(&factory_core::run::NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Manual,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: task.runtime.clone(),
                token: "tok".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        let run_id = run.id.clone();

        let (cwd, run) = engine.l4_service().place_run(&task, run, &scope_dir, Workspace::Fresh).await.unwrap();
        assert_ne!(cwd, scope_dir, "the checkbox is on, so this is not the scope itself");
        assert_eq!(cwd, engine.factory_snapshot().worktrees_dir().join(&run_id), "named after the run");
        assert!(cwd.join(".git").exists(), "a real worktree, not just a path");
        assert_eq!(run.worktree_path.as_deref(), Some(cwd.display().to_string().as_str()));
        assert!(
            run.worktree_branch.as_deref().unwrap_or_default().starts_with("factory/"),
            "the branch reads as this daemon's, got {:?}",
            run.worktree_branch
        );

        std::fs::remove_dir_all(&scope_dir).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    // -- args land in the order the daemon promises -------------------------

    #[test]
    fn declared_args_land_after_whatever_the_guide_already_injected() {
        // `launch_spec` puts the adapter's own defaults first and then the
        // guide's flag; this pins the half `append_declared_args` owns -- a
        // scope's own `args:` still lands after both, last word wins.
        let mut launch = LaunchSpec {
            kind: factory_core::adapter::agent::LaunchKind::Named("pi".into()),
            args: vec!["--append-system-prompt".into(), "/tmp/guide.md".into()],
            env: Default::default(),
            agent_kind: None,
        };
        let declared: ScopeAgent = serde_yaml_ng::from_str(
            "name: watcher\nharness: pi\nlifetime: permanent\nargs: [--model, opus]\n",
        )
        .unwrap();

        append_declared_args(&mut launch, Some(&declared));

        assert_eq!(
            launch.args,
            vec!["--append-system-prompt", "/tmp/guide.md", "--model", "opus"],
            "the declared override comes after the injected flag, not before it"
        );
    }

    #[tokio::test]
    async fn a_dispatched_runs_declared_args_follow_the_guide_launch_spec_injects() {
        // The same guarantee, exercised through the real adapter rather than
        // a hand-built `LaunchSpec`: `HarnessAgent::launch_spec` still puts
        // its own defaults first and the guide's flag after them, so
        // `append_declared_args` has something correctly ordered to append to.
        use factory_core::role::Role;
        use factory_core::Agent as _;
        use factory_plugins::HarnessAgent;

        let root = std::env::temp_dir().join(format!("factory-args-order-test-{}", uuid::Uuid::new_v4()));
        let ctx = AgentContext {
            scope: "demo".into(),
            agent_name: "watcher".into(),
            cwd: root.join("cwd"),
            factory_bin: PathBuf::from("factory"),
            socket: root.join("factory.sock"),
            callback_url: None,
            guides_dir: root.join("guides"),
            task: None,
            identity_token: Some("identity".into()),
            role: Some(
                factory_core::role::Roles::presets()
                    .get(&Role::worker())
                    .unwrap()
                    .clone(),
            ),
            policy_frameworks: Vec::new(),
            goal: None,
            quality: Vec::new(),
        };
        let agent = HarnessAgent::pi().with_args(vec!["--model".into(), "sonnet".into()]);
        let mut launch = agent.launch_spec(&ctx).await.unwrap();
        let declared: ScopeAgent = serde_yaml_ng::from_str(
            "name: watcher\nharness: pi\nlifetime: permanent\nargs: [--model, opus]\n",
        )
        .unwrap();

        append_declared_args(&mut launch, Some(&declared));

        assert_eq!(&launch.args[0..2], ["--model", "sonnet"], "the adapter's own defaults come first");
        assert_eq!(launch.args[2], "--append-system-prompt", "then the guide's flag");
        assert_eq!(
            &launch.args[launch.args.len() - 2..],
            ["--model", "opus"],
            "the scope's declared override lands last of all"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    // -- operations facts, issue #106 -----------------------------------------

    #[tokio::test]
    async fn every_way_a_run_ends_badly_records_its_kind() {
        let scope_dir = temp_dir("fail-kinds");
        let engine = test_engine(scope_dir.clone());
        let task = weekly_task(&engine, None).await;

        let run = run_for(&engine, &task.id, Trigger::Schedule).await;
        engine.l4_service().fail_run(&run.id, FailKind::RunTimeout, "ran for longer than 60s").await;
        let failed = engine.l4.store.get_run(&run.id).await.unwrap().unwrap();
        assert_eq!(failed.fail_kind, Some(FailKind::RunTimeout));
        assert_eq!(failed.error.as_deref(), Some("ran for longer than 60s"), "the prose stays beside it");

        let run = run_for(&engine, &task.id, Trigger::Manual).await;
        engine
            .l4_service()
            .report(
                &task.id,
                TaskReport {
                    artifacts: Vec::new(),
                    status: Some(RunStatus::Failed),
                    message: None,
                    result: None,
                    send_to: None,
                    error: Some("the tests do not pass".into()),
                    token: Some("tok".into()),
                },
            )
            .await
            .unwrap();
        let reported = engine.l4.store.get_run(&run.id).await.unwrap().unwrap();
        assert_eq!(reported.fail_kind, Some(FailKind::AgentFailed));

        let run = run_for(&engine, &task.id, Trigger::Manual).await;
        let response = engine.handle_request(Request::TaskCancel { id: task.id.clone(), reason: None, run: None }).await;
        assert!(matches!(response, Response::Ok { .. }), "{response:?}");
        let cancelled = engine.l4.store.get_run(&run.id).await.unwrap().unwrap();
        assert_eq!(cancelled.status, RunStatus::Cancelled);
        assert_eq!(
            cancelled.fail_kind,
            Some(FailKind::CancelledByPerson),
            "a cancel that came in with no token came in as the owner"
        );

        let done = run_for(&engine, &task.id, Trigger::Manual).await;
        engine
            .l4_service()
            .report(
                &task.id,
                TaskReport {
                    artifacts: Vec::new(),
                    status: Some(RunStatus::Done),
                    message: None,
                    result: Some("fine".into()),
                    send_to: None,
                    error: None,
                    token: Some("tok".into()),
                },
            )
            .await
            .unwrap();
        let done = engine.l4.store.get_run(&done.id).await.unwrap().unwrap();
        assert_eq!(done.fail_kind, None, "a run that succeeded has no fail kind");

        std::fs::remove_dir_all(&scope_dir).ok();
    }

    #[tokio::test]
    async fn a_run_started_for_a_slot_records_the_slot_and_a_dispatch_failure_says_so() {
        let scope_dir = temp_dir("due-slot");
        let engine = test_engine(scope_dir.clone());
        // A worktree task in a scope that is not a git repository: the run
        // row exists, and then `place_run` refuses.
        let task = engine
            .create(NewTask {
                title: "fires on a slot".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        let slot = Utc::now() - chrono::Duration::seconds(40);

        engine.l4_service().start_run_due(&task.id, Trigger::Schedule, Due::slot(slot)).await;

        let run = engine.l4.store.runs(&task.id, 1).await.unwrap().remove(0);
        assert_eq!(run.queued_at, Some(slot), "due at the slot, not at the tick that noticed it");
        assert_eq!(run.scheduled_for, Some(slot));
        assert_eq!(run.status, RunStatus::Failed);
        assert_eq!(run.fail_kind, Some(FailKind::DispatchFailed));

        std::fs::remove_dir_all(&scope_dir).ok();
    }

    #[tokio::test]
    async fn advancing_past_several_slots_journals_one_schedule_skipped_entry() {
        let scope_dir = temp_dir("skipped");
        let engine = test_engine(scope_dir.clone());
        let task = engine
            .create(NewTask {
                title: "every minute".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                worktree: Some(false),
                schedule: Some(Schedule::Every { seconds: 60 }),
                ..Default::default()
            })
            .await
            .unwrap();
        // The slot three and a half minutes ago fires now; the three after
        // it never will.
        let slot = Utc::now() - chrono::Duration::seconds(210);
        let task = engine
            .l4.store
            .update(&task.id, &TaskPatch { next_run_at: Some(slot), ..Default::default() })
            .await
            .unwrap();

        engine.l4_service().advance_schedule(&task).await.unwrap();

        let entries = engine.l4.store.entries(&task.id, 20).await.unwrap();
        let skipped: Vec<_> = entries.iter().filter(|e| e.kind == "schedule_skipped").collect();
        assert_eq!(skipped.len(), 1, "one entry for the whole gap, not one per slot: {entries:?}");
        let data = skipped[0].data.as_ref().unwrap();
        assert_eq!(data["count"], 3);
        assert_eq!(data["reason"], "not_running");
        assert_eq!(
            data["first"].as_str().unwrap().parse::<chrono::DateTime<Utc>>().unwrap(),
            slot + chrono::Duration::seconds(60)
        );

        // Fired on time: nothing skipped, nothing journalled.
        let task = engine.l4.store.get(&task.id).await.unwrap().unwrap();
        let on_time = engine
            .l4.store
            .update(&task.id, &TaskPatch { next_run_at: Some(Utc::now()), ..Default::default() })
            .await
            .unwrap();
        engine.l4_service().advance_schedule(&on_time).await.unwrap();
        let entries = engine.l4.store.entries(&task.id, 20).await.unwrap();
        assert_eq!(entries.iter().filter(|e| e.kind == "schedule_skipped").count(), 1);

        std::fs::remove_dir_all(&scope_dir).ok();
    }

    #[test]
    fn a_skip_is_blamed_on_the_previous_run_only_if_it_was_open_at_the_first_skipped_slot() {
        let t0 = Utc::now();
        let mut run = Run {
            artifacts: Vec::new(),
            id: "r".into(),
            task_id: "t".into(),
            attempt: 1,
            status: RunStatus::Done,
            trigger: Trigger::Schedule,
            agent: "shell".into(),
            adapter: "shell".into(),
            worktree_path: None,
            worktree_branch: None,
            runtime: "herdr".into(),
            session: None,
            last_session: None,
            token: None,
            spent_token_sha256: None,
            superseded_token_sha256s: Vec::new(),
            continued_from: None,
            workflow_round: 0,
            feedback: None,
            resume_context: None,
            resumed_session: None,
            original_estimate: None,
            provider_account: None,
            re_estimate: None,
            result: None,
            routed_to: None,
            error: None,
            started_at: t0,
            ended_at: Some(t0 + chrono::Duration::minutes(10)),
            queued_at: None,
            scheduled_for: None,
            fail_kind: None,
            blocked_since: None,
            blocked_source: None,
            block_suspected_since: None,
            turn_ended_at: None,
            turn_end_reason: None,
            turn_ended_session_id: None,
            required_steps: Vec::new(),
            usage: None,
        };
        let first = t0 + chrono::Duration::minutes(5);
        assert_eq!(skip_reason_of(Some(&run), first), SkipReason::StillActive);
        run.ended_at = Some(t0 + chrono::Duration::minutes(2));
        assert_eq!(skip_reason_of(Some(&run), first), SkipReason::NotRunning);
        run.ended_at = None;
        assert_eq!(skip_reason_of(Some(&run), first), SkipReason::StillActive);
        assert_eq!(skip_reason_of(None, first), SkipReason::NotRunning);
    }

    /// `#179`: a slot that passed while its run was already queued but held
    /// on `max_sessions`, not yet started, reads as `Queued` rather than
    /// `NotRunning` -- detected from the run row alone, no extra state.
    #[test]
    fn a_skip_is_blamed_on_a_capacity_wait_when_the_run_was_queued_but_not_yet_started() {
        let t0 = Utc::now();
        let run = Run {
            artifacts: Vec::new(),
            id: "r".into(),
            task_id: "t".into(),
            attempt: 1,
            status: RunStatus::Running,
            trigger: Trigger::Schedule,
            agent: "codex".into(),
            adapter: "codex".into(),
            worktree_path: None,
            worktree_branch: None,
            runtime: "herdr".into(),
            session: None,
            last_session: None,
            token: None,
            spent_token_sha256: None,
            superseded_token_sha256s: Vec::new(),
            continued_from: None,
            workflow_round: 0,
            feedback: None,
            resume_context: None,
            resumed_session: None,
            original_estimate: None,
            provider_account: None,
            re_estimate: None,
            result: None,
            routed_to: None,
            error: None,
            // Queued at the original slot, but not actually started (held on
            // capacity) until well past the next one.
            started_at: t0 + chrono::Duration::minutes(12),
            ended_at: None,
            queued_at: Some(t0),
            scheduled_for: Some(t0),
            fail_kind: None,
            blocked_since: None,
            blocked_source: None,
            block_suspected_since: None,
            turn_ended_at: None,
            turn_end_reason: None,
            turn_ended_session_id: None,
            required_steps: Vec::new(),
            usage: None,
        };
        let first = t0 + chrono::Duration::minutes(5);
        assert_eq!(skip_reason_of(Some(&run), first), SkipReason::Queued);
        assert_eq!(SkipReason::Queued.describe(), "it was already queued, waiting for a capacity slot");
    }

    #[tokio::test]
    async fn a_paused_schedule_is_kept_skipped_by_due_and_resumes_from_now() {
        let scope_dir = temp_dir("pause");
        let engine = test_engine(scope_dir.clone());
        let task = weekly_task(&engine, None).await;
        let schedule = task.schedule.clone();

        // Mid-streak, and overdue: the case a pause has to hold back.
        let run = run_for(&engine, &task.id, Trigger::Schedule).await;
        engine.l4_service().fail_run(&run.id, FailKind::AgentFailed, "exit 1").await;
        let long_ago = Utc::now() - chrono::Duration::hours(2);
        engine
            .l4.store
            .update(&task.id, &TaskPatch { next_run_at: Some(long_ago), ..Default::default() })
            .await
            .unwrap();
        assert!(engine.l4_service().due_now().await.unwrap().iter().any(|t| t.id == task.id), "sanity: due before the pause");

        let paused = engine
            .update(&task.id, TaskPatch { schedule_paused: Some(true), ..Default::default() }, None)
            .await
            .unwrap();
        assert!(paused.schedule_paused);
        assert_eq!(paused.schedule, schedule, "a paused task keeps its schedule");
        assert!(!engine.l4_service().due_now().await.unwrap().iter().any(|t| t.id == task.id), "nothing fires while paused");

        let resumed = engine
            .update(&task.id, TaskPatch { schedule_paused: Some(false), ..Default::default() }, None)
            .await
            .unwrap();
        assert!(!resumed.schedule_paused);
        assert!(resumed.next_run_at.unwrap() > Utc::now(), "recomputed from now: no catch-up firing");
        assert!(resumed.pending_retry.is_none(), "the interrupted retry streak is over");
        assert!(!engine.l4_service().due_now().await.unwrap().iter().any(|t| t.id == task.id));

        let kinds: Vec<String> = engine.l4.store.entries(&task.id, 50).await.unwrap().into_iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&"schedule_paused".to_string()), "{kinds:?}");
        assert!(kinds.contains(&"schedule_resumed".to_string()), "{kinds:?}");

        // Saying it again changes nothing and journals nothing.
        engine
            .update(&task.id, TaskPatch { schedule_paused: Some(false), ..Default::default() }, None)
            .await
            .unwrap();
        let resumes = engine
            .l4.store
            .entries(&task.id, 50)
            .await
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "schedule_resumed")
            .count();
        assert_eq!(resumes, 1);

        std::fs::remove_dir_all(&scope_dir).ok();
    }

    /// The review's regression: the task form clears an empty schedule on
    /// every save, and a pause left behind on a task with no schedule held
    /// back the next schedule anyone gave it.
    #[tokio::test]
    async fn clearing_a_paused_schedule_clears_the_pause_so_a_new_one_fires() {
        let scope_dir = temp_dir("pause-clear");
        let engine = test_engine(scope_dir.clone());
        let task = weekly_task(&engine, None).await;
        engine
            .update(&task.id, TaskPatch { schedule_paused: Some(true), ..Default::default() }, None)
            .await
            .unwrap();

        let cleared = engine
            .update(&task.id, TaskPatch { clear_schedule: true, ..Default::default() }, None)
            .await
            .unwrap();
        assert!(cleared.schedule.is_none());
        assert!(!cleared.schedule_paused, "the pause went with the schedule");
        let kinds: Vec<String> = engine.l4.store.entries(&task.id, 50).await.unwrap().into_iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&"schedule_pause_cleared".to_string()), "{kinds:?}");

        let rescheduled = engine
            .update(&task.id, TaskPatch { schedule: Some(Schedule::Every { seconds: 60 }), ..Default::default() }, None)
            .await
            .unwrap();
        assert!(!rescheduled.schedule_paused);
        // Due once its first slot comes round.
        engine
            .l4.store
            .update(&task.id, &TaskPatch { next_run_at: Some(Utc::now() - chrono::Duration::seconds(1)), ..Default::default() })
            .await
            .unwrap();
        assert!(engine.l4_service().due_now().await.unwrap().iter().any(|t| t.id == task.id), "the new schedule fires");

        std::fs::remove_dir_all(&scope_dir).ok();
    }

    #[tokio::test]
    async fn replacing_a_paused_schedule_in_one_patch_keeps_it_paused() {
        let scope_dir = temp_dir("pause-replace");
        let engine = test_engine(scope_dir.clone());
        let task = weekly_task(&engine, None).await;
        engine
            .update(&task.id, TaskPatch { schedule_paused: Some(true), ..Default::default() }, None)
            .await
            .unwrap();
        let replaced = engine
            .update(
                &task.id,
                TaskPatch {
                    clear_schedule: true,
                    schedule: Some(Schedule::Every { seconds: 60 }),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert!(replaced.schedule.is_some());
        assert!(replaced.schedule_paused, "a schedule is still there, so the pause holds");

        std::fs::remove_dir_all(&scope_dir).ok();
    }

    #[test]
    fn a_slot_is_queued_from_when_it_could_first_have_been_dispatched() {
        let base = Utc::now();
        let t = |m: i64| base - chrono::Duration::minutes(m);
        let mut previous = Run {
            artifacts: Vec::new(),
            id: "r".into(),
            task_id: "t".into(),
            attempt: 1,
            status: RunStatus::Done,
            trigger: Trigger::Schedule,
            agent: "shell".into(),
            adapter: "shell".into(),
            worktree_path: None,
            worktree_branch: None,
            runtime: "herdr".into(),
            session: None,
            last_session: None,
            token: None,
            spent_token_sha256: None,
            superseded_token_sha256s: Vec::new(),
            continued_from: None,
            workflow_round: 0,
            feedback: None,
            resume_context: None,
            resumed_session: None,
            original_estimate: None,
            provider_account: None,
            re_estimate: None,
            result: None,
            routed_to: None,
            error: None,
            started_at: t(90),
            ended_at: Some(t(80)),
            queued_at: None,
            scheduled_for: None,
            fail_kind: None,
            blocked_since: None,
            blocked_source: None,
            block_suspected_since: None,
            turn_ended_at: None,
            turn_end_reason: None,
            turn_ended_session_id: None,
            required_steps: Vec::new(),
            usage: None,
        };
        let slot = t(60);
        let booted = t(1000);
        // Nothing in the way: the slot itself.
        assert_eq!(dispatchable_from(slot, booted, Some(&previous)), slot);
        assert_eq!(dispatchable_from(slot, booted, None), slot);
        // The daemon came up after the slot: from then.
        assert_eq!(dispatchable_from(slot, t(30), None), t(30));
        // The previous run was still going at the slot: from its end.
        previous.ended_at = Some(t(40));
        assert_eq!(dispatchable_from(slot, booted, Some(&previous)), t(40));
        // Both: whichever came later.
        assert_eq!(dispatchable_from(slot, t(20), Some(&previous)), t(20));
        // A previous run that started after the slot did not hold it up.
        previous.started_at = t(50);
        assert_eq!(dispatchable_from(slot, booted, Some(&previous)), slot);
    }

    #[tokio::test]
    async fn only_a_scheduled_task_can_be_paused() {
        let scope_dir = temp_dir("pause-unscheduled");
        let engine = test_engine(scope_dir.clone());
        let task = engine
            .create(NewTask {
                title: "one-off".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                worktree: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        let err = engine
            .update(&task.id, TaskPatch { schedule_paused: Some(true), ..Default::default() }, None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("only a scheduled task"), "{err}");

        std::fs::remove_dir_all(&scope_dir).ok();
    }

    // -- a failure goes to Blocked; closing is a person's act (#122) -------

    async fn one_off_task(engine: &Engine, title: &str) -> Task {
        engine
            .create(NewTask {
                title: title.into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                worktree: Some(false),
                ..Default::default()
            })
            .await
            .unwrap()
    }

    fn refusal(response: factory_core::protocol::Response) -> String {
        match response {
            factory_core::protocol::Response::Error { message, .. } => message,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    fn task_of(response: factory_core::protocol::Response) -> Task {
        match response {
            factory_core::protocol::Response::Ok { data: Payload::Task { task } } => task,
            other => panic!("expected a task, got {other:?}"),
        }
    }

    fn close(id: &str, reason: factory_core::task::CloseReason) -> Request {
        Request::TaskClose { id: id.into(), reason, duplicate_of: None, note: None }
    }

    #[tokio::test]
    async fn a_one_off_task_whose_run_failed_is_blocked_on_that_failure_for_every_fail_kind() {
        let scope_dir = temp_dir("failed-blocks");
        let engine = test_engine(scope_dir.clone());
        for kind in [
            FailKind::AgentFailed,
            FailKind::SessionGone,
            FailKind::TurnEnded,
            FailKind::StopFailure,
            FailKind::AckTimeout,
            FailKind::RunTimeout,
            FailKind::BlockedTimeout,
            FailKind::DispatchFailed,
        ] {
            let task = one_off_task(&engine, kind.as_str()).await;
            let run = run_for(&engine, &task.id, Trigger::Manual).await;
            engine.l4_service().fail_run(&run.id, kind, "it went wrong").await;

            let task = engine.require(&task.id).await.unwrap();
            assert_eq!(task.status, TaskStatus::Blocked, "{kind:?}: never closed by a failure");
            assert!(task.blocked_by_failure(), "{kind:?}");
            assert!(!task.status.is_terminal(), "{kind:?}: a failure is an open item");
            assert!(task.is_settled(), "{kind:?}: but nothing is running");
            let failure = task.failure.as_ref().unwrap();
            assert_eq!(failure.kind, Some(kind));
            assert_eq!(failure.run_id.as_deref(), Some(run.id.as_str()));
            assert_eq!(failure.attempt, Some(1));
            assert_eq!(task.error.as_deref(), Some("it went wrong"));

            let run = engine.require_run(&run.id).await.unwrap();
            assert_eq!(run.status, RunStatus::Failed, "the run keeps its meaning");
            assert!(run.status.is_terminal());
            assert_eq!(run.fail_kind, Some(kind));
        }
        std::fs::remove_dir_all(&scope_dir).ok();
    }

    #[tokio::test]
    async fn a_dispatch_refused_before_any_run_existed_blocks_the_task_on_dispatch_failed() {
        let scope_dir = temp_dir("dispatch-refused");
        let engine = test_engine(scope_dir.clone());
        let task = one_off_task(&engine, "nowhere to run").await;
        engine
            .l4.store
            .update(&task.id, &TaskPatch { agent: Some("no-such-agent".into()), ..Default::default() })
            .await
            .unwrap();

        engine.l4_service().start_run_due(&task.id, Trigger::Manual, Due::now()).await;

        let task = engine.require(&task.id).await.unwrap();
        assert!(engine.l4.store.runs(&task.id, 5).await.unwrap().is_empty(), "sanity: no run was made");
        assert!(task.blocked_by_failure(), "{:?}", task.status);
        let failure = task.failure.unwrap();
        assert_eq!(failure.kind, Some(FailKind::DispatchFailed));
        assert_eq!(failure.run_id, None);
        assert!(task.error.is_some());
        std::fs::remove_dir_all(&scope_dir).ok();
    }

    #[tokio::test]
    async fn a_run_that_succeeds_after_a_failure_clears_the_block() {
        let scope_dir = temp_dir("failure-cleared");
        let engine = test_engine(scope_dir.clone());
        let task = one_off_task(&engine, "flaky").await;
        let first = run_for(&engine, &task.id, Trigger::Manual).await;
        engine.l4_service().fail_run(&first.id, FailKind::AgentFailed, "boom").await;

        let second = run_for(&engine, &task.id, Trigger::Manual).await;
        let mid = engine.require(&task.id).await.unwrap();
        assert_eq!(mid.status, TaskStatus::Dispatching);
        assert!(mid.failure.is_none(), "a new attempt is newer than the failure it follows");

        engine
            .l4_service()
            .finish_run(&second.id, RunStatus::Done, RunPatch { result: Some("fine".into()), ..Default::default() }, "ended")
            .await
            .unwrap();
        let task = engine.require(&task.id).await.unwrap();
        assert_eq!(task.status, TaskStatus::Done);
        assert!(task.failure.is_none());
        assert!(task.error.is_none());
        assert_eq!(task.close_reason(), Some(factory_core::task::CloseReason::Completed));
        std::fs::remove_dir_all(&scope_dir).ok();
    }

    /// The hazard the issue names first: a task blocked by a failure has no
    /// active run, and the blocked timeout only ever looks at active runs --
    /// so it is never timed out again, and never turns back into a failure.
    #[tokio::test]
    async fn a_task_blocked_by_a_failure_is_never_timed_out_again() {
        let scope_dir = temp_dir("blocked-timeout-once");
        let engine = test_engine(scope_dir.clone());
        let task = one_off_task(&engine, "waits for a human").await;
        engine
            .l4.store
            .update(&task.id, &TaskPatch { blocked_timeout_seconds: Some(1), ..Default::default() })
            .await
            .unwrap();
        let run = run_for(&engine, &task.id, Trigger::Manual).await;
        let long_ago = Utc::now() - chrono::Duration::hours(1);
        engine
            .l4.store
            .update_run(
                &run.id,
                &RunPatch {
                    status: Some(RunStatus::Blocked),
                    blocked_since: Some(long_ago),
                    blocked_source: Some(BlockSource::Agent),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        // What the scheduler does with it, once.
        engine.l4_service().fail_run(&run.id, FailKind::BlockedTimeout, "nobody answered").await;

        let task = engine.require(&task.id).await.unwrap();
        assert!(task.blocked_by_failure());
        assert_eq!(task.failure.as_ref().and_then(|f| f.kind), Some(FailKind::BlockedTimeout));
        let active = engine.l4_service().active_runs().await.unwrap();
        assert!(
            active.iter().all(|r| r.task_id != task.id),
            "no blocked run stands in for the failure, so nothing is left for the timeout to find: {active:?}"
        );
        assert_eq!(engine.l4.store.runs(&task.id, 10).await.unwrap().len(), 1, "and no new run was made to show it");
        std::fs::remove_dir_all(&scope_dir).ok();
    }

    #[tokio::test]
    async fn a_task_with_no_run_at_all_closes_as_not_planned_and_the_journal_says_who() {
        let scope_dir = temp_dir("close-no-run");
        let engine = test_engine(scope_dir.clone());
        let task = one_off_task(&engine, "never mind").await;

        let closed = task_of(
            engine
                .handle_request(Request::TaskClose {
                    id: task.id.clone(),
                    reason: factory_core::task::CloseReason::NotPlanned,
                    duplicate_of: None,
                    note: Some("  the client dropped it  ".into()),
                })
                .await,
        );
        assert_eq!(closed.status, TaskStatus::Cancelled);
        assert_eq!(closed.close_reason(), Some(factory_core::task::CloseReason::NotPlanned));
        let closure = closed.closure.as_ref().unwrap();
        assert_eq!(closure.by, "the owner");
        assert_eq!(closure.note.as_deref(), Some("the client dropped it"));

        let entries = engine.l4.store.entries(&task.id, 20).await.unwrap();
        let entry = entries.iter().find(|e| e.kind == "closed").expect("journaled");
        assert_eq!(entry.source, "owner");
        assert!(entry.message.contains("won't do") && entry.message.contains("the client dropped it"), "{}", entry.message);
        let data = entry.data.as_ref().unwrap();
        assert_eq!(data["by"], "the owner");
        assert_eq!(data["close_reason"], "not_planned");
        std::fs::remove_dir_all(&scope_dir).ok();
    }

    #[tokio::test]
    async fn a_task_blocked_by_a_failure_closes_as_a_duplicate_of_another() {
        let scope_dir = temp_dir("close-duplicate");
        let engine = test_engine(scope_dir.clone());
        let original = one_off_task(&engine, "the real one").await;
        let task = one_off_task(&engine, "the copy").await;
        let run = run_for(&engine, &task.id, Trigger::Manual).await;
        engine.l4_service().fail_run(&run.id, FailKind::AgentFailed, "boom").await;

        let closed = task_of(
            engine
                .handle_request(Request::TaskClose {
                    id: task.id.clone(),
                    reason: factory_core::task::CloseReason::Duplicate,
                    duplicate_of: Some(original.id.clone()),
                    note: None,
                })
                .await,
        );
        assert_eq!(closed.status, TaskStatus::Cancelled);
        assert_eq!(closed.close_reason(), Some(factory_core::task::CloseReason::Duplicate));
        assert_eq!(closed.closure.as_ref().unwrap().duplicate_of.as_deref(), Some(original.id.as_str()));
        let entries = engine.l4.store.entries(&task.id, 20).await.unwrap();
        let entry = entries.iter().find(|e| e.kind == "closed").unwrap();
        assert!(entry.message.starts_with("its last attempt failed; closed as a duplicate of"), "{}", entry.message);
        assert_eq!(entry.data.as_ref().unwrap()["fail_kind"], "agent_failed");
        std::fs::remove_dir_all(&scope_dir).ok();
    }

    #[tokio::test]
    async fn closing_is_refused_while_a_run_is_active_when_already_closed_and_for_a_bad_duplicate() {
        let scope_dir = temp_dir("close-refused");
        let engine = test_engine(scope_dir.clone());
        use factory_core::task::CloseReason::*;

        let busy = one_off_task(&engine, "busy").await;
        run_for(&engine, &busy.id, Trigger::Manual).await;
        let why = refusal(engine.handle_request(close(&busy.id, NotPlanned)).await);
        assert!(why.contains("cancel it before closing"), "{why}");
        assert_eq!(engine.require(&busy.id).await.unwrap().status, TaskStatus::Dispatching, "untouched");

        let done = one_off_task(&engine, "done").await;
        task_of(engine.handle_request(close(&done.id, Completed)).await);
        let why = refusal(engine.handle_request(close(&done.id, NotPlanned)).await);
        assert!(why.contains("already closed (completed)"), "{why}");

        let other = one_off_task(&engine, "other").await;
        let why = refusal(
            engine
                .handle_request(Request::TaskClose {
                    id: other.id.clone(),
                    reason: NotPlanned,
                    duplicate_of: Some(done.id.clone()),
                    note: None,
                })
                .await,
        );
        assert!(why.contains("goes with the reason duplicate"), "{why}");
        let why = refusal(
            engine
                .handle_request(Request::TaskClose {
                    id: other.id.clone(),
                    reason: Duplicate,
                    duplicate_of: Some(other.id.clone()),
                    note: None,
                })
                .await,
        );
        assert!(why.contains("cannot duplicate itself"), "{why}");
        let why = refusal(
            engine
                .handle_request(Request::TaskClose {
                    id: other.id.clone(),
                    reason: Duplicate,
                    duplicate_of: Some("no-such-task".into()),
                    note: None,
                })
                .await,
        );
        assert!(why.contains("no-such-task"), "{why}");
        assert_eq!(engine.require(&other.id).await.unwrap().status, TaskStatus::Pending);
        std::fs::remove_dir_all(&scope_dir).ok();
    }

    #[tokio::test]
    async fn a_closed_task_reopens_to_pending_and_a_run_clears_a_close_record() {
        let scope_dir = temp_dir("reopen");
        let engine = test_engine(scope_dir.clone());
        let task = one_off_task(&engine, "back on").await;
        let run = run_for(&engine, &task.id, Trigger::Manual).await;
        engine.l4_service().fail_run(&run.id, FailKind::AgentFailed, "boom").await;
        task_of(engine.handle_request(close(&task.id, factory_core::task::CloseReason::NotPlanned)).await);

        let reopened = task_of(
            engine.handle_request(Request::TaskReopen { id: task.id.clone(), reason: Some("worth another go".into()) }).await,
        );
        assert_eq!(reopened.status, TaskStatus::Pending);
        assert!(reopened.closure.is_none());
        assert!(reopened.failure.is_none(), "a pending task must not read as one mid-retry");
        assert_eq!(reopened.close_reason(), None);
        let entries = engine.l4.store.entries(&task.id, 20).await.unwrap();
        assert!(entries.iter().any(|e| e.kind == "reopened" && e.message.contains("worth another go")), "{entries:?}");

        let why = refusal(engine.handle_request(Request::TaskReopen { id: task.id.clone(), reason: None }).await);
        assert!(why.contains("not closed"), "{why}");

        // A run on a task closed on purpose clears the close record with it.
        task_of(engine.handle_request(close(&task.id, factory_core::task::CloseReason::Completed)).await);
        run_for(&engine, &task.id, Trigger::Manual).await;
        let task = engine.require(&task.id).await.unwrap();
        assert_eq!(task.status, TaskStatus::Dispatching);
        assert!(task.closure.is_none());
        std::fs::remove_dir_all(&scope_dir).ok();
    }

    #[tokio::test]
    async fn a_person_cancelling_a_run_closes_the_task_as_not_planned() {
        let scope_dir = temp_dir("cancel-closes");
        let engine = test_engine(scope_dir.clone());
        let task = one_off_task(&engine, "stop that").await;
        run_for(&engine, &task.id, Trigger::Manual).await;
        let response = engine
            .handle_request(Request::TaskCancel { id: task.id.clone(), reason: None, run: None })
            .await;
        assert!(matches!(response, factory_core::protocol::Response::Ok { .. }), "{response:?}");
        let task = engine.require(&task.id).await.unwrap();
        assert_eq!(task.status, TaskStatus::Cancelled);
        assert_eq!(task.close_reason(), Some(factory_core::task::CloseReason::NotPlanned));
        assert!(task.failure.is_none(), "a cancel is not a failure");
        std::fs::remove_dir_all(&scope_dir).ok();
    }

    #[tokio::test]
    async fn a_scheduled_task_blocked_by_a_failure_stays_due_and_a_closed_one_never_fires() {
        let scope_dir = temp_dir("blocked-still-due");
        let engine = test_engine(scope_dir.clone());
        let blocked = weekly_task(&engine, Some(RetryPolicy::None)).await;
        let run = run_for(&engine, &blocked.id, Trigger::Schedule).await;
        engine.l4_service().fail_run(&run.id, FailKind::AgentFailed, "boom").await;
        let closed = weekly_task(&engine, None).await;
        task_of(engine.handle_request(close(&closed.id, factory_core::task::CloseReason::NotPlanned)).await);
        let asking = weekly_task(&engine, None).await;
        let run = run_for(&engine, &asking.id, Trigger::Schedule).await;
        engine
            .l4.store
            .update_run(&run.id, &RunPatch { status: Some(RunStatus::Blocked), ..Default::default() })
            .await
            .unwrap();
        let asking_now = engine.require(&asking.id).await.unwrap();
        engine.l4_service().mirror_to_task(&engine.require_run(&run.id).await.unwrap()).await;
        assert_eq!(engine.require(&asking.id).await.unwrap().status, TaskStatus::Blocked, "sanity");
        let _ = asking_now;

        let past = Utc::now() - chrono::Duration::minutes(1);
        for id in [&blocked.id, &closed.id, &asking.id] {
            engine.l4.store.update(id, &TaskPatch { next_run_at: Some(past), ..Default::default() }).await.unwrap();
        }

        let due: Vec<String> = engine.l4_service().due_now().await.unwrap().into_iter().map(|t| t.id).collect();
        assert_eq!(due, vec![blocked.id.clone()], "the failure keeps firing; the close and the question do not");

        // Closed between `due_now` and the scheduler's locked re-read: not fired.
        let seen = engine.require(&blocked.id).await.unwrap();
        assert!(engine.l4_service().still_due(&seen).await.is_some());
        task_of(engine.handle_request(close(&blocked.id, factory_core::task::CloseReason::NotPlanned)).await);
        assert!(engine.l4_service().still_due(&seen).await.is_none(), "closed in between, so nothing fires it");
        std::fs::remove_dir_all(&scope_dir).ok();
    }

    #[tokio::test]
    async fn reopening_a_scheduled_task_picks_its_schedule_up_from_now() {
        let scope_dir = temp_dir("reopen-scheduled");
        let engine = test_engine(scope_dir.clone());
        let task = weekly_task(&engine, None).await;
        task_of(engine.handle_request(close(&task.id, factory_core::task::CloseReason::NotPlanned)).await);
        let long_ago = Utc::now() - chrono::Duration::days(30);
        engine.l4.store.update(&task.id, &TaskPatch { next_run_at: Some(long_ago), ..Default::default() }).await.unwrap();

        let reopened = task_of(engine.handle_request(Request::TaskReopen { id: task.id.clone(), reason: None }).await);
        assert_eq!(reopened.status, TaskStatus::Pending);
        assert!(reopened.next_run_at.unwrap() > Utc::now(), "no burst of the slots that passed while it was closed");
        std::fs::remove_dir_all(&scope_dir).ok();
    }

    #[tokio::test]
    async fn a_task_stored_as_failed_is_moved_to_blocked_once_at_start() {
        let scope_dir = temp_dir("migrate-failed");
        let engine = test_engine(scope_dir.clone());

        // The shape a failed run left before #122: run failed, task failed.
        let old = one_off_task(&engine, "failed long ago").await;
        let run = run_for(&engine, &old.id, Trigger::Manual).await;
        engine
            .l4.store
            .update_run(
                &run.id,
                &RunPatch {
                    status: Some(RunStatus::Failed),
                    fail_kind: Some(FailKind::RunTimeout),
                    ended_at: Some(Utc::now()),
                    clear_token: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        engine.l4.store.update(&old.id, &TaskPatch { status: Some(TaskStatus::Failed), ..Default::default() }).await.unwrap();
        // One that never got a run, scheduled, its slot long past.
        let never = weekly_task(&engine, None).await;
        let long_ago = Utc::now() - chrono::Duration::days(30);
        engine
            .l4.store
            .update(&never.id, &TaskPatch { status: Some(TaskStatus::Failed), next_run_at: Some(long_ago), ..Default::default() })
            .await
            .unwrap();
        let fine = one_off_task(&engine, "untouched").await;

        assert_eq!(engine.l4_service().migrate_failed_tasks().await, 2);

        let old = engine.require(&old.id).await.unwrap();
        assert!(old.blocked_by_failure());
        assert_eq!(old.failure.as_ref().and_then(|f| f.kind), Some(FailKind::RunTimeout));
        assert_eq!(old.failure.as_ref().and_then(|f| f.run_id.clone()), Some(run.id.clone()));
        let entries = engine.l4.store.entries(&old.id, 20).await.unwrap();
        assert!(entries.iter().any(|e| e.kind == "migrated"), "{entries:?}");

        let never = engine.require(&never.id).await.unwrap();
        assert!(never.blocked_by_failure());
        assert_eq!(never.failure.as_ref().and_then(|f| f.kind), Some(FailKind::DispatchFailed));
        assert!(never.next_run_at.unwrap() > Utc::now(), "a migrated schedule does not fire a burst");

        assert_eq!(engine.require(&fine.id).await.unwrap().status, TaskStatus::Pending);
        assert_eq!(engine.l4_service().migrate_failed_tasks().await, 0, "nothing writes failed any more, so a second start finds none");
        std::fs::remove_dir_all(&scope_dir).ok();
    }

    // ==================================================================
    // #178: `factory task run --continue` after an infrastructure failure
    // ==================================================================
    mod continue_tests {
        use super::*;
        use async_trait::async_trait;
        use factory_core::adapter::agent::{LaunchKind, ResumeSpec};
        use factory_core::adapter::runtime::RuntimeEventStream;
        use factory_core::task::SessionRef;
        use factory_core::usage::{HarnessUsage, SessionUsage, SnapshotPoint, UsageSnapshot};
        use std::sync::Mutex;

        /// Declares `resume_spec` when `resumable`, exactly the shape
        /// `claude-code`/`codex` are given in `factory-plugins`; `false` is
        /// every other built-in's shape (`pi`, `opencode`, and -- used
        /// directly in one test below -- `shell`).
        struct RecordingAgent {
            resumable: bool,
            /// What `TaskBinding.upstream` looked like on the most recent
            /// `launch_spec` call -- `#275`'s `ask_agent` has nowhere else
            /// to put its question, so this is what proves it actually
            /// reached the binding a real harness's `prompt()` renders from
            /// (`upstream_section`, tested on its own in
            /// `factory-plugins/src/builtin/agents.rs`).
            captured_upstream: Mutex<Option<Vec<UpstreamOutput>>>,
            /// The goal context on the most recent `launch_spec` call: what dispatch resolved from the task's `goal` label.
            captured_goal: Mutex<Option<factory_core::adapter::agent::GoalContext>>,
        }

        impl RecordingAgent {
            fn new(resumable: bool) -> Self {
                Self { resumable, captured_upstream: Mutex::new(None), captured_goal: Mutex::new(None) }
            }
        }

        #[async_trait]
        impl Agent for RecordingAgent {
            fn name(&self) -> &str {
                "recording"
            }
            async fn launch_spec(&self, ctx: &AgentContext) -> Result<LaunchSpec> {
                *self.captured_upstream.lock().unwrap() = ctx.task.as_ref().map(|t| t.upstream.clone());
                *self.captured_goal.lock().unwrap() = ctx.goal.clone();
                Ok(LaunchSpec { kind: LaunchKind::Command(vec!["true".into()]), args: Vec::new(), env: Default::default(), agent_kind: None })
            }
            async fn prompt(&self, _ctx: &AgentContext) -> Result<String> {
                Ok(String::new())
            }
            fn resume_spec(&self, session_id: &str) -> Option<ResumeSpec> {
                self.resumable.then(|| ResumeSpec { args: vec!["--resume".into(), session_id.into()] })
            }
        }

        /// A runtime that opens a session without doing anything real
        /// (`capacity_tests::StubRuntime`'s own comment explains why), with
        /// two knobs this module's tests need on top: every `StartRequest`
        /// it was asked to `start` (so a test can inspect the resume args
        /// and the working directory an agent actually launched into), and
        /// a configurable `status` answer (so a test can say whether the
        /// previous run's session is confirmed gone).
        struct RecordingRuntime {
            status: Mutex<RuntimeStatus>,
            starts: Mutex<Vec<StartRequest>>,
            launch_pause: Mutex<Option<Arc<tokio::sync::Notify>>>,
        }

        impl RecordingRuntime {
            fn new(status: RuntimeStatus) -> Self {
                Self { status: Mutex::new(status), starts: Mutex::new(Vec::new()), launch_pause: Mutex::new(None) }
            }
        }

        #[async_trait]
        impl AgentRuntime for RecordingRuntime {
            fn name(&self) -> &str {
                "stub-run"
            }
            async fn start(&self, req: &StartRequest) -> Result<SessionRef> {
                self.starts.lock().unwrap().push(req.clone());
                let pause = self.launch_pause.lock().unwrap().clone();
                if let Some(pause) = pause { pause.notified().await; }
                Ok(SessionRef { runtime: "stub-run".into(), handle: format!("stub-{}", req.id), meta: Default::default() })
            }
            async fn submit(&self, _session: &SessionRef, _text: &str) -> Result<()> {
                Ok(())
            }
            async fn status(&self, _session: &SessionRef) -> Result<RuntimeStatus> {
                Ok(*self.status.lock().unwrap())
            }
            async fn send_text(&self, _session: &SessionRef, _text: &str) -> Result<()> {
                Ok(())
            }
            async fn send_keys(&self, _session: &SessionRef, _keys: &[String]) -> Result<()> {
                Ok(())
            }
            async fn read(&self, _session: &SessionRef, _lines: u32) -> Result<String> {
                Ok(String::new())
            }
            async fn stop(&self, _session: &SessionRef) -> Result<()> {
                Ok(())
            }
            async fn watch(&self) -> Result<Option<RuntimeEventStream>> {
                Ok(None)
            }
        }

        fn continue_engine(scope_path: PathBuf, resumable: bool, runtime_status: RuntimeStatus) -> (Arc<Engine>, Arc<RecordingRuntime>) {
            let config = Config {
                version: 1,
                instance: Instance { id: "test".into(), name: "test".into() },
                daemon: DaemonConfig { power_assertion: false, ..DaemonConfig::default() },
                roles: Default::default(),
                dashboard: None,
                policies: Default::default(),
                quality: Default::default(),
                scope: None,
                scopes: vec![Scope {
                    id: "scope-id".into(),
                    name: "demo".into(),
                    path: scope_path,
                    agent: None,
                    agents: Vec::new(),
                    runtime: None,
                    git: None,
                    task_store: None,
                    max_sessions: None,
                    roles: Default::default(),
                    dashboard: None,
                    policies: Default::default(),
                    quality: Default::default(),
                    intake: Default::default(),
                    dependencies: Default::default(),
                    environments: Vec::new(),
                    renewals: Vec::new(),
                    metrics: None,
                    backup: None,
                }],
                infrastructure: Default::default(),
                secrets: Vec::new(),
                plugins_dir: None,
                renewals: Vec::new(),
                renewals_notify: None,
            };
            let factory = Factory {
                root: std::env::temp_dir().join(format!("factory-continue-test-{}", uuid::Uuid::new_v4())),
                config,
            };
            let mut registry = Registry::with_builtins();
            registry.add_agent(Arc::new(RecordingAgent::new(resumable)), "test");
            let runtime = Arc::new(RecordingRuntime::new(runtime_status));
            registry.add_runtime(runtime.clone(), "test");
            let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
            let engine = Arc::new(Engine::new(factory, registry, store, PathBuf::from("factory"), Vec::new()));
            (engine, runtime)
        }

        /// The same as `continue_engine`, resumable with the previous
        /// session already confirmed gone -- what every successful-resume
        /// test above already sets up -- except this one hands back the
        /// `RecordingAgent` itself rather than the runtime, so a test can
        /// read `captured_upstream` after dispatching. A separate function
        /// rather than widening `continue_engine`'s own return type, which
        /// every other test in this module already destructures as a pair.
        fn continue_engine_recording_upstream(scope_path: PathBuf) -> (Arc<Engine>, Arc<RecordingAgent>) {
            let config = Config {
                version: 1,
                instance: Instance { id: "test".into(), name: "test".into() },
                daemon: DaemonConfig { power_assertion: false, ..DaemonConfig::default() },
                roles: Default::default(),
                dashboard: None,
                policies: Default::default(),
                quality: Default::default(),
                scope: None,
                scopes: vec![Scope {
                    id: "scope-id".into(),
                    name: "demo".into(),
                    path: scope_path,
                    agent: None,
                    agents: Vec::new(),
                    runtime: None,
                    git: None,
                    task_store: None,
                    max_sessions: None,
                    roles: Default::default(),
                    dashboard: None,
                    policies: Default::default(),
                    quality: Default::default(),
                    intake: Default::default(),
                    dependencies: Default::default(),
                    environments: Vec::new(),
                    renewals: Vec::new(),
                    backup: None,
                    metrics: None,
                }],
                infrastructure: Default::default(),
                secrets: Vec::new(),
                plugins_dir: None,
                renewals: Vec::new(),
                renewals_notify: None,
            };
            let factory = Factory {
                root: std::env::temp_dir().join(format!("factory-continue-test-{}", uuid::Uuid::new_v4())),
                config,
            };
            let mut registry = Registry::with_builtins();
            let agent = Arc::new(RecordingAgent::new(true));
            registry.add_agent(agent.clone(), "test");
            registry.add_runtime(Arc::new(RecordingRuntime::new(RuntimeStatus::Gone)), "test");
            let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
            let engine = Arc::new(Engine::new(factory, registry, store, PathBuf::from("factory"), Vec::new()));
            (engine, agent)
        }

        async fn task_for(engine: &Arc<Engine>, worktree: bool) -> Task {
            engine
                .create(NewTask {
                    title: "resume me".into(),
                    instructions: "true".into(),
                    scope: Some("demo".into()),
                    agent: Some("recording".into()),
                    runtime: Some("stub-run".into()),
                    worktree: Some(worktree),
                    ..Default::default()
                })
                .await
                .unwrap()
        }

        fn harness_usage(session_id: &str, adapter: &str) -> HarnessUsage {
            serde_json::from_value(serde_json::json!({ "session_id": session_id, "adapter": adapter })).unwrap()
        }

        /// A later usage snapshot naming `session_id` for the `recording`
        /// adapter -- what `resolve_continue` reads to find a resumable
        /// session (`usage::newest_session_id_for_adapter`).
        async fn seed_session_id(engine: &Arc<Engine>, run: &Run, session_id: &str) {
            engine
                .l4.store
                .append_usage(&UsageSnapshot {
                    run_id: run.id.clone(),
                    task_id: run.task_id.clone(),
                    point: SnapshotPoint::RunEnd,
                    at: Utc::now(),
                    runtime: "stub-run".into(),
                    usage: Some(SessionUsage {
                        schema: 1,
                        handle: None,
                        sampled_at: None,
                        sessions: vec![harness_usage(session_id, "recording")],
                    }),
                    unknown: None,
                })
                .await
                .unwrap();
        }

        /// A real dispatch (fresh, not `--continue`) failed with `kind` --
        /// what every fallback and success test in this module starts from,
        /// so the previous run's session, worktree and token are exactly
        /// what a real `dispatch` would have left, not a hand-rolled
        /// approximation of it.
        async fn dispatched_then_failed(engine: &Arc<Engine>, task: &Task, kind: FailKind) -> Run {
            let run = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), None).await.unwrap();
            engine.l4_service().fail_run(&run.id, kind, "infra hiccup").await;
            engine.l4.store.get_run(&run.id).await.unwrap().unwrap()
        }

        async fn git_scope_dir(name: &str) -> PathBuf {
            let dir = temp_dir(name);
            for args in [
                vec!["init", "-q"],
                vec!["config", "user.email", "factory@example.com"],
                vec!["config", "user.name", "factory"],
                vec!["commit", "-q", "--allow-empty", "-m", "base"],
            ] {
                assert!(tokio::process::Command::new("git").args(&args).current_dir(&dir).status().await.unwrap().success());
            }
            dir
        }

        async fn continue_fallback_reasons(engine: &Engine, task_id: &str) -> Vec<String> {
            engine
                .l4.store
                .entries(task_id, 50)
                .await
                .unwrap()
                .into_iter()
                .filter(|e| e.kind == "continue_fallback")
                .map(|e| e.message)
                .collect()
        }

        #[tokio::test]
        async fn continuing_resumes_in_the_same_worktree_with_a_new_token() {
            let scope_dir = git_scope_dir("continue-resume").await;
            let (engine, runtime) = continue_engine(scope_dir.clone(), true, RuntimeStatus::Gone);
            let task = task_for(&engine, true).await;
            let prev = dispatched_then_failed(&engine, &task, FailKind::AckTimeout).await;
            assert!(prev.worktree_path.is_some(), "sanity: the failed run got a worktree");
            seed_session_id(&engine, &prev, "sess-123").await;
            let prev = engine.l4.store.get_run(&prev.id).await.unwrap().unwrap();

            let run = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), Some(prev.clone())).await.unwrap();

            assert_eq!(run.worktree_path, prev.worktree_path, "the same worktree, not a fresh one");
            assert_eq!(run.resumed_session.as_deref(), Some("sess-123"));
            assert_eq!(run.continued_from.as_deref(), Some(prev.id.as_str()));
            assert_ne!(run.token, prev.token, "a new token");
            assert!(
                run.superseded_token_sha256s.contains(prev.spent_token_sha256.as_ref().unwrap()),
                "{:?}",
                run.superseded_token_sha256s
            );

            let starts = runtime.starts.lock().unwrap();
            let start = starts.last().unwrap();
            assert_eq!(start.launch.args.first().map(String::as_str), Some("--resume"), "{:?}", start.launch.args);
            assert_eq!(start.launch.args.get(1).map(String::as_str), Some("sess-123"), "{:?}", start.launch.args);
            assert_eq!(start.cwd, PathBuf::from(prev.worktree_path.clone().unwrap()));

            std::fs::remove_dir_all(&scope_dir).ok();
        }

        // ==============================================================
        // #193 S9b design step: a task's `goal` label reaches the agent's context
        // through `Wiring::intent()`, with no call up into L6
        // ==============================================================

        #[tokio::test]
        async fn a_goal_label_reaches_the_agents_context_from_the_authored_catalogue() {
            let scope_dir = git_scope_dir("goal-label-intent").await;
            let (engine, agent) = continue_engine_recording_upstream(scope_dir.clone());
            let goals = factory_direction::goals::goals_dir(&engine.factory_snapshot().root);
            std::fs::create_dir_all(&goals).unwrap();
            std::fs::write(
                goals.join("2026-q4.yaml"),
                "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
                 objectives:\n\
                 \x20\x20- id: obj\n\x20\x20\x20\x20title: Objective Title\n\x20\x20\x20\x20key_results:\n\
                 \x20\x20\x20\x20\x20\x20- {id: kr, title: KR Title, kind: committed, manual: true, baseline: 0, target: 1}\n",
            )
            .unwrap();

            let labelled = engine
                .create(NewTask {
                    title: "labelled".into(),
                    instructions: "true".into(),
                    scope: Some("demo".into()),
                    agent: Some("recording".into()),
                    runtime: Some("stub-run".into()),
                    worktree: Some(false),
                    labels: [("goal".to_string(), "obj/kr".to_string())].into(),
                    ..Default::default()
                })
                .await
                .unwrap();
            engine.l4_service().dispatch(&labelled.id, Trigger::Manual, Due::now(), None).await.unwrap();
            let goal = agent.captured_goal.lock().unwrap().clone().expect("the label resolved to a goal context");
            assert_eq!((goal.objective_title.as_str(), goal.kr_title.as_str()), ("Objective Title", "KR Title"));

            let plain = task_for(&engine, false).await;
            engine.l4_service().dispatch(&plain.id, Trigger::Manual, Due::now(), None).await.unwrap();
            assert!(agent.captured_goal.lock().unwrap().is_none(), "a task without a goal label carries none");

            std::fs::remove_dir_all(&scope_dir).ok();
        }

        // ==============================================================
        // #275: "ask the agent" actually delivers the question
        // ==============================================================

        #[tokio::test]
        async fn asking_a_suggestion_delivers_the_question_in_the_resumed_binding_and_settles_the_answer() {
            let scope_dir = git_scope_dir("ask-delivers-question").await;
            let (engine, agent) = continue_engine_recording_upstream(scope_dir.clone());
            let task = task_for(&engine, true).await;

            // A real dispatch, so the previous run's worktree/session shape
            // is exactly what `dispatch` itself would have left -- then file
            // a suggestion against it *before* it ends, while its token is
            // still live, the way an agent filing one mid-run actually would.
            let prev = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), None).await.unwrap();
            let suggestion = engine
                .l5_service()
                .file_suggestion(
                    &task.id,
                    factory_core::protocol::SuggestionReport {
                        kind: factory_core::protocol::SuggestionKind::Capability,
                        target: "L2 secret stripe_key".into(),
                        summary: "a blocked outbound call".into(),
                        detail: None,
                        wasted_tokens: Some(1200),
                        token: prev.token.clone(),
                    },
                )
                .await
                .unwrap();
            engine.l4_service().fail_run(&prev.id, FailKind::AckTimeout, "infra hiccup").await;
            seed_session_id(&engine, &prev, "sess-ask").await;

            let answered = engine
                .l5_service()
                .suggestion_ask(&crate::access::Caller::Owner, &suggestion.id, "why did this fail?".into())
                .await
                .unwrap();
            let ask_run_id = answered.ask.as_ref().unwrap().run_id.clone();
            let ask_run = engine.l4.store.get_run(&ask_run_id).await.unwrap().unwrap();
            assert_eq!(ask_run.resumed_session.as_deref(), Some("sess-ask"), "the ask run actually resumed, not a fresh one");

            // The question reached `TaskBinding.upstream` -- what a real
            // harness's `prompt()` renders from (`upstream_section`,
            // `factory-plugins/src/builtin/agents.rs`) -- not just the
            // suggestion record.
            let captured = agent.captured_upstream.lock().unwrap().clone().expect("launch_spec was called");
            let ask_entry = captured.iter().find(|u| u.node_id == "ask").expect("an ask upstream entry");
            let text = ask_entry.result.as_deref().unwrap_or("");
            assert!(text.contains("why did this fail?"), "{text}");
            assert!(text.contains("blocked outbound call"), "{text}");
            assert!(text.contains("Report done"), "{text}");

            // And once that resumed run reports, the answer settles onto
            // the suggestion (`Engine::finish_run` -> `settle_suggestion_ask`).
            engine
                .l4_service()
                .report(&task.id, TaskReport {
                    artifacts: Vec::new(),
                    status: Some(RunStatus::Done),
                    message: None,
                    result: Some("it needed a different grant".into()),
                    send_to: None,
                    error: None,
                    token: ask_run.token.clone(),
                })
                .await
                .unwrap();
            let settled = engine.l5_service().suggestion_get(&suggestion.id).await.unwrap();
            assert_eq!(settled.ask.as_ref().unwrap().answer.as_deref(), Some("it needed a different grant"));

            std::fs::remove_dir_all(&scope_dir).ok();
        }

        async fn wait_node_attempt(engine: &Engine, node: &str, attempt: u32) -> (Task, Run) {
            for _ in 0..500 {
                for task in engine.l4.store.list(&TaskFilter::default()).await.unwrap() {
                    if task.workflow_origin.as_ref().is_some_and(|origin| origin.node_id == node) {
                        if let Some(run) = engine.l4.store.active_run(&task.id).await.unwrap().filter(|run| run.attempt == attempt) {
                            if engine.l4.store.entries(&task.id, 50).await.unwrap().iter().any(|entry|
                                entry.kind == "dispatched" && entry.run_id.as_deref() == Some(run.id.as_str())) {
                                return (task, run);
                            }
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            panic!("no attempt {attempt} on node {node}");
        }

        #[tokio::test]
        async fn workflow_feedback_resumes_both_same_tasks_and_records_rounds_on_runs() {
            workflow_feedback_round(false).await;
        }

        #[tokio::test]
        async fn workspace_lifetime_keeps_failed_attempt_files_for_fresh_retry_and_releases_on_close() {
            let scope_dir = git_scope_dir("workspace-task-lifetime").await;
            let (engine, _) = continue_engine(scope_dir.clone(), true, RuntimeStatus::Gone);
            let task = task_for(&engine, true).await;
            let first = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), None).await.unwrap();
            let path = PathBuf::from(first.worktree_path.as_ref().unwrap());
            std::fs::write(path.join("unfinished"), "keep across fresh sessions").unwrap();
            engine.l4_service().fail_run(&first.id, FailKind::AckTimeout, "outage").await;
            assert!(path.exists(), "a failed attempt is not a closed task");
            let second = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), None).await.unwrap();
            assert_eq!(second.worktree_path, first.worktree_path);
            assert_eq!(second.worktree_branch, first.worktree_branch);
            assert!(second.resumed_session.is_none());
            assert_eq!(std::fs::read_to_string(path.join("unfinished")).unwrap(), "keep across fresh sessions");
            engine.l4_service().fail_run(&second.id, FailKind::AckTimeout, "outage again").await;
            // Remove only this fixture's ownership ledger to emulate an older
            // daemon. Recovery proves ownership from the recorded run path.
            std::fs::remove_file(engine.factory_snapshot().worktrees_dir().join(".workspace-owner.json")).unwrap();
            engine.l4_service().recover_workspaces().await;
            assert_eq!(engine.l4.workspaces.records().await.unwrap().len(), 1);
            assert!(path.exists(), "recovering a failed task does not close it");
            let response = engine.handle_request(Request::TaskClose {
                id: task.id.clone(), reason: factory_core::task::CloseReason::NotPlanned,
                duplicate_of: None, note: Some("test close".into()),
            }).await;
            assert!(!matches!(response, Response::Error { .. }), "{response:?}");
            assert!(path.exists(), "untracked work is retained even after close");
            assert!(engine.l4.store.entries(&task.id, 100).await.unwrap().iter().any(|entry| entry.kind == "workspace_retained"));
            // Simulate the person moving their unfinished data out of the tree.
            let saved = scope_dir.join("saved-work");
            std::fs::rename(path.join("unfinished"), &saved).unwrap();
            engine.l4_service().sweep_workspaces().await;
            assert!(!path.exists());
            assert_eq!(std::fs::read_to_string(saved).unwrap(), "keep across fresh sessions");
            assert!(engine.l4.store.entries(&task.id, 100).await.unwrap().iter().any(|entry| entry.kind == "workspace_released"));
            let root = engine.factory_snapshot().root;
            std::fs::remove_dir_all(root).ok();
            std::fs::remove_dir_all(scope_dir).ok();
        }

        #[tokio::test]
        async fn workspace_cancel_during_slow_launch_cannot_attach_a_pane_to_a_terminal_run() {
            let scope_dir = temp_dir("workspace-cancel-launch");
            let (engine, runtime) = continue_engine(scope_dir.clone(), true, RuntimeStatus::Gone);
            let task = task_for(&engine, false).await;
            let pause = Arc::new(tokio::sync::Notify::new());
            *runtime.launch_pause.lock().unwrap() = Some(pause.clone());
            let launched = { let engine = engine.clone(); let id = task.id.clone();
                tokio::spawn(async move { engine.l4_service().dispatch(&id, Trigger::Manual, Due::now(), None).await }) };
            for _ in 0..100 {
                if !runtime.starts.lock().unwrap().is_empty() { break; }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            assert_eq!(runtime.starts.lock().unwrap().len(), 1);
            let cancelled = { let engine = engine.clone(); let id = task.id.clone();
                tokio::spawn(async move { engine.l4_service().cancel_task_run(&id, None, FailKind::CancelledByPerson).await }) };
            tokio::task::yield_now().await;
            pause.notify_one();
            launched.await.unwrap().unwrap();
            let cancelled = cancelled.await.unwrap().unwrap();
            assert_eq!(cancelled.status, RunStatus::Cancelled);
            let final_run = engine.require_run(&cancelled.id).await.unwrap();
            assert!(final_run.last_session.is_some(), "the session that came up late is still recorded for safety checks");
            assert!(final_run.session.is_none(), "no late launch reattaches a pane after completion");
            engine.l4_service().fail_run(&cancelled.id, FailKind::DispatchFailed, "late submit failed").await;
            assert_eq!(engine.require_run(&cancelled.id).await.unwrap().status, RunStatus::Cancelled);
            std::fs::remove_dir_all(scope_dir).ok();
        }

        #[tokio::test]
        async fn workspace_sweep_racing_new_admission_never_removes_its_live_tree() {
            let scope_dir = git_scope_dir("workspace-admission-sweep").await;
            let (engine, _) = continue_engine(scope_dir.clone(), true, RuntimeStatus::Gone);
            let task = task_for(&engine, true).await;
            let previous = dispatched_then_failed(&engine, &task, FailKind::AckTimeout).await;
            // A crash between closing intent and sending release leaves this
            // clean tree eligible for the next sweep, while manual retry races.
            engine.l4.store.update(&task.id, &TaskPatch { status: Some(TaskStatus::Done), ..Default::default() }).await.unwrap();
            let l4 = engine.l4_service();
            let (started, ()) = tokio::join!(l4.dispatch(&task.id, Trigger::Manual, Due::now(), None), l4.sweep_workspaces());
            let started = started.unwrap();
            let path = PathBuf::from(started.worktree_path.unwrap());
            assert!(path.exists());
            assert!(worktree::is_registered(&scope_dir, &path).await);
            assert!(!engine.require_run(&started.id).await.unwrap().status.is_terminal());
            assert!(previous.worktree_path.is_some());
            engine.l4_service().cancel_task_run(&task.id, None, FailKind::CancelledByPerson).await.unwrap();
            std::fs::remove_dir_all(engine.factory_snapshot().root).ok();
            std::fs::remove_dir_all(scope_dir).ok();
        }

        #[tokio::test]
        async fn workspace_terminal_workflow_waits_for_blocked_sibling_and_confirmed_process_exit() {
            let scope_dir = git_scope_dir("workspace-blocked-sibling").await;
            let (engine, runtime) = continue_engine(scope_dir.clone(), true, RuntimeStatus::Gone);
            let draft = serde_yaml_ng::from_str::<factory_core::workflow::WorkflowDraft>(r#"
name: workspace siblings
scope: demo
nodes:
  - id: a
    task: {title: A, instructions: a, scope: demo, agent: recording, runtime: stub-run, worktree: true}
  - id: b
    task: {title: B, instructions: b, scope: demo, agent: recording, runtime: stub-run, worktree: true}
edges: []
"#).unwrap();
            let definition = engine.l4_service().create_workflow(draft).await.unwrap();
            let workflow = engine.start_workflow(&definition.id, Default::default(), &crate::access::Caller::Owner).await.unwrap();
            let (a, first) = wait_node_attempt(&engine, "a", 1).await;
            let (b, sibling) = wait_node_attempt(&engine, "b", 1).await;
            let first_path = PathBuf::from(first.worktree_path.clone().unwrap());
            let sibling_path = PathBuf::from(sibling.worktree_path.clone().unwrap());
            engine.l4_service().report(&b.id, TaskReport {
                status: Some(RunStatus::Blocked), result: None, token: sibling.token.clone(),
                artifacts: Vec::new(), message: Some("need a person".into()), send_to: None, error: None,
            }).await.unwrap();
            engine.l4_service().fail_run(&first.id, FailKind::AckTimeout, "upstream outage").await;
            engine.sync_workflow_for_task(&a.id).await;
            assert!(engine.l4_service().workflow_run(&workflow.id).await.unwrap().status.is_terminal());
            engine.l4_service().sweep_workspaces().await;
            assert!(first_path.exists() && sibling_path.exists(), "terminal workflow must keep all trees while a sibling waits on a person");
            // Even a terminal report does not prove that a failed stop worked.
            *runtime.status.lock().unwrap() = RuntimeStatus::Working;
            engine.l4_service().cancel_task_run(&b.id, None, FailKind::CancelledByPerson).await.unwrap();
            engine.sync_workflow_for_task(&b.id).await;
            engine.l4_service().sweep_workspaces().await;
            assert!(first_path.exists() && sibling_path.exists());
            *runtime.status.lock().unwrap() = RuntimeStatus::Gone;
            engine.l4_service().sweep_workspaces().await;
            assert!(!first_path.exists() && !sibling_path.exists());
            let root = engine.factory_snapshot().root;
            std::fs::remove_dir_all(root).ok();
            std::fs::remove_dir_all(scope_dir).ok();
        }

        #[tokio::test]
        async fn workflow_feedback_fresh_review_policy_keeps_task_but_not_conversation() {
            workflow_feedback_round(true).await;
        }

        async fn workflow_feedback_round(fresh_review: bool) {
            let scope_dir = git_scope_dir("workflow-resume").await;
            let (engine, runtime) = continue_engine(scope_dir.clone(), true, RuntimeStatus::Gone);
            let mut draft = serde_yaml_ng::from_str::<factory_core::workflow::WorkflowDraft>(r#"
name: feedback resume
scope: demo
nodes:
  - id: implement
    task: {title: Implement, instructions: implement, scope: demo, agent: recording, runtime: stub-run, worktree: true}
  - id: review
    task: {title: Review, instructions: review, scope: demo, agent: recording, runtime: stub-run, worktree: true}
    exits: [{to: implement, agent: needs fixes, max_rounds: 3}]
edges: [{id: next, from: implement, to: review}]
"#).unwrap();
            if fresh_review {
                draft.nodes.iter_mut().find(|node| node.id == "review").unwrap().session = factory_core::workflow::SessionPolicy::Fresh;
            }
            let definition = engine.l4_service().create_workflow(draft).await.unwrap();
            let workflow = engine.start_workflow(&definition.id, Default::default(), &crate::access::Caller::Owner).await.unwrap();
            let (implement, first) = wait_node_attempt(&engine, "implement", 1).await;
            let old_token = first.token.clone().unwrap();
            seed_session_id(&engine, &first, "implement-session").await;
            engine.l4_service().report(&implement.id, TaskReport {
                status: Some(RunStatus::Done), result: Some("first work".into()), token: first.token,
                artifacts: Vec::new(), message: None, send_to: None, error: None,
            }).await.unwrap();
            engine.sync_workflow_for_task(&implement.id).await;
            let (review, review_first) = wait_node_attempt(&engine, "review", 1).await;
            seed_session_id(&engine, &review_first, "review-session").await;
            engine.l4_service().report(&review.id, TaskReport {
                status: Some(RunStatus::Done), result: Some("fix the parser".into()),
                send_to: Some("implement".into()), token: review_first.token,
                artifacts: Vec::new(), message: None, error: None,
            }).await.unwrap();
            engine.sync_workflow_for_task(&review.id).await;
            let (again, second) = wait_node_attempt(&engine, "implement", 2).await;
            assert_eq!(again.id, implement.id);
            assert_eq!(again.title, "Implement");
            assert_eq!(second.worktree_path, first.worktree_path);
            assert_eq!(second.resumed_session.as_deref(), Some("implement-session"));
            assert_eq!(second.workflow_round, 1);
            assert_eq!(second.feedback.as_ref().unwrap().feedback.as_deref(), Some("fix the parser"));
            assert_ne!(second.token.as_deref(), Some(old_token.as_str()));
            assert!(engine.caller_for(Some(&old_token)).await.unwrap_err().to_string().contains("newer run"));
            engine.recover_workflows().await;
            assert_eq!(engine.l4.store.runs(&implement.id, 50).await.unwrap().len(), 2, "recovery cannot replay feedback");
            engine.l4_service().report(&implement.id, TaskReport {
                status: Some(RunStatus::Done), result: Some("fixed".into()), token: second.token,
                artifacts: Vec::new(), message: None, send_to: None, error: None,
            }).await.unwrap();
            engine.sync_workflow_for_task(&implement.id).await;
            let (review_again, review_second) = wait_node_attempt(&engine, "review", 2).await;
            assert_eq!(review_again.id, review.id);
            let duplicate = engine.l4_service().dispatch(&implement.id, Trigger::Workflow, Due::now(), None).await.unwrap_err();
            assert!(matches!(duplicate, FactoryError::DispatchSuperseded(_)), "a settled feedback round cannot be replayed");
            assert_eq!(engine.l4.store.runs(&implement.id, 50).await.unwrap().len(), 2);
            if fresh_review {
                assert_eq!(review_second.worktree_path, review_first.worktree_path, "fresh conversation, same task files");
                assert!(review_second.resumed_session.is_none());
                assert!(continue_fallback_reasons(&engine, &review.id).await.iter().any(|reason| reason.contains("session: fresh")));
            } else {
                assert_eq!(review_second.worktree_path, review_first.worktree_path);
                assert_eq!(review_second.resumed_session.as_deref(), Some("review-session"));
            }
            engine.l4_service().report(&review.id, TaskReport {
                status: Some(RunStatus::Done), result: Some("passed".into()), token: review_second.token,
                artifacts: Vec::new(), message: None, send_to: None, error: None,
            }).await.unwrap();
            engine.sync_workflow_for_task(&review.id).await;
            let settled = engine.l4_service().workflow_run(&workflow.id).await.unwrap();
            assert_eq!(settled.status, factory_core::workflow::WorkflowRunStatus::Done);
            assert_eq!(engine.l4.store.list(&TaskFilter::default()).await.unwrap().len(), 2);
            for node in settled.nodes {
                assert_eq!(node.attempts.iter().map(|attempt| attempt.round).collect::<Vec<_>>(), vec![0, 1]);
                assert!(node.superseded_task_ids.is_empty());
            }
            assert_eq!(runtime.starts.lock().unwrap().len(), 4);
            std::fs::remove_dir_all(&scope_dir).ok();
        }

        #[tokio::test]
        async fn continue_checkpoint_is_taken_at_run_end_and_rejects_a_changed_branch() {
            let scope_dir = git_scope_dir("continue-checkpoint").await;
            let (engine, _) = continue_engine(scope_dir.clone(), true, RuntimeStatus::Gone);
            let task = task_for(&engine, true).await;
            let first = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), None).await.unwrap();
            let directory = PathBuf::from(first.worktree_path.as_ref().unwrap());
            assert!(tokio::process::Command::new("git")
                .args(["commit", "--allow-empty", "-m", "work during this run"])
                .current_dir(&directory).status().await.unwrap().success());
            seed_session_id(&engine, &first, "recorded-session").await;
            engine.l4_service().fail_run(&first.id, FailKind::AckTimeout, "outage").await;
            let previous = engine.require_run(&first.id).await.unwrap();
            assert_ne!(previous.resume_context.as_ref().unwrap().branch_head, first.resume_context.as_ref().unwrap().branch_head);
            let current = crate::resume::checkpoint(&directory, "unused".into(), 0).await;
            assert_eq!(previous.resume_context.as_ref().unwrap().branch_head, current.branch_head);
            assert!(tokio::process::Command::new("git").args(["switch", "-c", "different-branch"])
                .current_dir(&directory).status().await.unwrap().success());
            let fresh = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), Some(previous)).await.unwrap();
            assert!(fresh.resumed_session.is_none());
            assert_ne!(fresh.worktree_path, first.worktree_path);
            assert!(continue_fallback_reasons(&engine, &task.id).await.iter().any(|reason| reason.contains("recorded branch")));
            std::fs::remove_dir_all(scope_dir).ok();
        }

        #[tokio::test]
        async fn continue_concurrent_dispatch_claims_only_one_run() {
            let scope_dir = temp_dir("continue-concurrent");
            let (engine, runtime) = continue_engine(scope_dir.clone(), true, RuntimeStatus::Gone);
            let task = task_for(&engine, false).await;
            let previous = dispatched_then_failed(&engine, &task, FailKind::AckTimeout).await;
            seed_session_id(&engine, &previous, "recorded-session").await;
            let l4 = engine.l4_service();
            let (left, right) = tokio::join!(
                l4.dispatch(&task.id, Trigger::Manual, Due::now(), Some(previous.clone())),
                l4.dispatch(&task.id, Trigger::Manual, Due::now(), Some(previous.clone()))
            );
            let (winner, loser) = match (left, right) {
                (Ok(run), Err(error)) | (Err(error), Ok(run)) => (run, error),
                pair => panic!("exactly one dispatch must win: {pair:?}"),
            };
            assert!(matches!(loser, FactoryError::DispatchSuperseded(_)));
            assert_eq!(engine.l4.store.runs(&task.id, 50).await.unwrap().len(), 2);
            assert_eq!(runtime.starts.lock().unwrap().len(), 2);
            assert_eq!(engine.l4.store.active_run(&task.id).await.unwrap().unwrap().id, winner.id);
            engine.l4_service().start_run_due_continue(&task.id, Due::now(), previous.clone()).await;
            assert!(!engine.require_run(&winner.id).await.unwrap().status.is_terminal(), "the loser cannot fail the winner");
            engine.l4_service().fail_run(&winner.id, FailKind::AckTimeout, "next outage").await;
            let stale = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), Some(previous)).await.unwrap_err();
            assert!(matches!(stale, FactoryError::DispatchSuperseded(_)));
            assert_eq!(runtime.starts.lock().unwrap().len(), 2);
            std::fs::remove_dir_all(scope_dir).ok();
        }

        #[tokio::test]
        async fn continue_resets_after_eight_resumes_and_rejects_changed_guide_context() {
            let scope_dir = temp_dir("continue-guard");
            let (engine, _) = continue_engine(scope_dir.clone(), true, RuntimeStatus::Gone);
            let task = task_for(&engine, false).await;
            let previous = dispatched_then_failed(&engine, &task, FailKind::AckTimeout).await;
            seed_session_id(&engine, &previous, "recorded-session").await;
            let mut previous = engine.require_run(&previous.id).await.unwrap();
            previous.resume_context.as_mut().unwrap().resumes = 8;
            let next = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), Some(previous)).await.unwrap();
            assert!(next.resumed_session.is_none());
            assert_eq!(next.resume_context.as_ref().unwrap().resumes, 0);
            assert!(continue_fallback_reasons(&engine, &task.id).await.iter().any(|reason| reason.contains("eight-round")));
            engine.l4_service().fail_run(&next.id, FailKind::AckTimeout, "another outage").await;
            seed_session_id(&engine, &next, "fresh-session").await;
            let mut previous = engine.require_run(&next.id).await.unwrap();
            previous.resume_context.as_mut().unwrap().fingerprint = "old-guide".into();
            let fresh = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), Some(previous)).await.unwrap();
            assert!(fresh.resumed_session.is_none());
            assert!(continue_fallback_reasons(&engine, &task.id).await.iter().any(|reason| reason.contains("guide, role")));
            std::fs::remove_dir_all(scope_dir).ok();
        }

        #[tokio::test]
        async fn continue_falls_back_to_fresh_when_no_session_id_was_recorded() {
            let scope_dir = temp_dir("continue-no-session-id");
            let (engine, _runtime) = continue_engine(scope_dir.clone(), true, RuntimeStatus::Gone);
            let task = task_for(&engine, false).await;
            let prev = dispatched_then_failed(&engine, &task, FailKind::AckTimeout).await;
            // No usage snapshot seeded, and no `turn-ended` hook ever fired:
            // nothing to resume from.

            let run = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), Some(prev.clone())).await.unwrap();

            assert!(run.resumed_session.is_none());
            assert_eq!(run.continued_from.as_deref(), Some(prev.id.as_str()), "recorded even though it fell back");
            let reasons = continue_fallback_reasons(&engine, &task.id).await;
            assert!(reasons.iter().any(|r| r.contains("no session id was recorded")), "{reasons:?}");
            std::fs::remove_dir_all(&scope_dir).ok();
        }

        #[tokio::test]
        async fn continue_falls_back_to_fresh_when_the_adapter_declares_no_resume() {
            let scope_dir = temp_dir("continue-no-resume-spec");
            let (engine, _runtime) = continue_engine(scope_dir.clone(), false, RuntimeStatus::Gone);
            let task = task_for(&engine, false).await;
            let prev = dispatched_then_failed(&engine, &task, FailKind::AckTimeout).await;
            seed_session_id(&engine, &prev, "sess-456").await;
            let prev = engine.l4.store.get_run(&prev.id).await.unwrap().unwrap();

            let run = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), Some(prev.clone())).await.unwrap();

            assert!(run.resumed_session.is_none());
            let reasons = continue_fallback_reasons(&engine, &task.id).await;
            assert!(reasons.iter().any(|r| r.contains("declares no resume")), "{reasons:?}");
            std::fs::remove_dir_all(&scope_dir).ok();
        }

        #[tokio::test]
        async fn continue_falls_back_to_fresh_when_the_previous_session_is_not_confirmed_gone() {
            let scope_dir = temp_dir("continue-not-gone");
            let (engine, _runtime) = continue_engine(scope_dir.clone(), true, RuntimeStatus::Working);
            let task = task_for(&engine, false).await;
            let prev = dispatched_then_failed(&engine, &task, FailKind::AckTimeout).await;
            seed_session_id(&engine, &prev, "sess-789").await;
            let prev = engine.l4.store.get_run(&prev.id).await.unwrap().unwrap();

            let run = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), Some(prev.clone())).await.unwrap();

            assert!(run.resumed_session.is_none());
            let reasons = continue_fallback_reasons(&engine, &task.id).await;
            assert!(reasons.iter().any(|r| r.contains("not confirmed gone")), "{reasons:?}");
            std::fs::remove_dir_all(&scope_dir).ok();
        }

        #[tokio::test]
        async fn continue_falls_back_to_fresh_when_the_worktree_is_gone() {
            let scope_dir = git_scope_dir("continue-worktree-gone").await;
            let (engine, _runtime) = continue_engine(scope_dir.clone(), true, RuntimeStatus::Gone);
            let task = task_for(&engine, true).await;
            let prev = dispatched_then_failed(&engine, &task, FailKind::AckTimeout).await;
            seed_session_id(&engine, &prev, "sess-gone").await;
            let prev = engine.l4.store.get_run(&prev.id).await.unwrap().unwrap();
            // Removed by hand -- Factory itself never does this -- the way
            // `worktree::is_registered` is meant to catch.
            std::fs::remove_dir_all(prev.worktree_path.as_ref().unwrap()).unwrap();

            let run = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), Some(prev.clone())).await.unwrap();

            assert!(run.resumed_session.is_none());
            assert_ne!(run.worktree_path, prev.worktree_path, "a fresh worktree, not the missing one");
            let reasons = continue_fallback_reasons(&engine, &task.id).await;
            assert!(reasons.iter().any(|r| r.contains("worktree is gone")), "{reasons:?}");
            std::fs::remove_dir_all(&scope_dir).ok();
        }

        #[tokio::test]
        async fn continue_falls_back_to_fresh_when_the_worktree_branch_was_not_recorded() {
            let scope_dir = git_scope_dir("continue-worktree-no-branch").await;
            let (engine, _runtime) = continue_engine(scope_dir.clone(), true, RuntimeStatus::Gone);
            let task = task_for(&engine, true).await;
            let prev = dispatched_then_failed(&engine, &task, FailKind::AckTimeout).await;
            seed_session_id(&engine, &prev, "sess-no-branch").await;
            let prev = engine.l4.store.get_run(&prev.id).await.unwrap().unwrap();
            assert!(prev.worktree_path.is_some(), "sanity: the failed run got a worktree");
            assert!(prev.worktree_branch.is_some(), "sanity: a real dispatch always records both");
            // Nothing in this codebase ever clears `worktree_branch` once
            // set -- this simulates the one way it could still be missing:
            // a run row from before the field existed, the same shape
            // `queued_at`/`fail_kind` handle for an older row elsewhere.
            let mut old_shape = serde_json::to_value(&prev).unwrap();
            old_shape.as_object_mut().unwrap().remove("worktree_branch");
            let prev: Run = serde_json::from_value(old_shape).unwrap();
            assert!(prev.worktree_branch.is_none(), "sanity: the field really is gone now");

            let run = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), Some(prev.clone())).await.unwrap();

            assert!(run.resumed_session.is_none());
            assert_eq!(run.worktree_path, prev.worktree_path, "the durable owner receipt can prove the workspace without guessing its branch");
            let reasons = continue_fallback_reasons(&engine, &task.id).await;
            assert!(reasons.iter().any(|r| r.contains("worktree branch was not recorded")), "{reasons:?}");
            std::fs::remove_dir_all(&scope_dir).ok();
        }

        #[tokio::test]
        async fn continue_falls_back_to_fresh_when_the_agent_changed_since_the_previous_run() {
            let scope_dir = temp_dir("continue-agent-changed");
            let (engine, _runtime) = continue_engine(scope_dir.clone(), true, RuntimeStatus::Gone);
            let task = task_for(&engine, false).await;
            let prev = dispatched_then_failed(&engine, &task, FailKind::AckTimeout).await;
            seed_session_id(&engine, &prev, "sess-changed").await;
            let mut prev = engine.l4.store.get_run(&prev.id).await.unwrap().unwrap();
            // Nothing changed the task's own agent -- only the *previous
            // run's own record* of what it ran as, which is what a task
            // whose agent was edited after that run would actually look
            // like from `resolve_continue`'s side.
            prev.agent = "someone-else".into();

            let run = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), Some(prev.clone())).await.unwrap();

            assert!(run.resumed_session.is_none());
            let reasons = continue_fallback_reasons(&engine, &task.id).await;
            assert!(reasons.iter().any(|r| r.contains("agent changed")), "{reasons:?}");
            std::fs::remove_dir_all(&scope_dir).ok();
        }

        #[tokio::test]
        async fn continue_is_refused_outright_when_the_newest_run_did_not_fail_on_infrastructure() {
            let scope_dir = temp_dir("continue-not-infra");
            let (engine, _runtime) = continue_engine(scope_dir.clone(), true, RuntimeStatus::Gone);
            let task = task_for(&engine, false).await;
            dispatched_then_failed(&engine, &task, FailKind::AgentFailed).await;

            let response = engine
                .handle_request(Request::TaskRun { override_wait: false, id: task.id.clone(), reason: None, continue_run: true })
                .await;
            let message = match response {
                Response::Error { message, .. } => message,
                other => panic!("expected a refusal, got {other:?}"),
            };
            assert!(message.contains("not an infrastructure failure"), "{message}");
            std::fs::remove_dir_all(&scope_dir).ok();
        }

        /// `#274`: resuming after a blocked-timeout reclaim is the whole
        /// point of preserving a blocked sandbox's conversation, so the
        /// `--continue` request gate (distinct from `resolve_continue`,
        /// which separately decides whether a session can really resume)
        /// must not refuse it outright the way it refuses every other
        /// non-infrastructure ending.
        #[tokio::test]
        async fn continue_is_accepted_outright_for_a_run_that_ended_on_the_blocked_timeout() {
            let scope_dir = temp_dir("continue-blocked-timeout-gate");
            let (engine, _runtime) = continue_engine(scope_dir.clone(), true, RuntimeStatus::Gone);
            let task = task_for(&engine, false).await;
            dispatched_then_failed(&engine, &task, FailKind::BlockedTimeout).await;

            let response = engine
                .handle_request(Request::TaskRun { override_wait: false, id: task.id.clone(), reason: None, continue_run: true })
                .await;
            let refused_as_non_infra = matches!(
                &response,
                Response::Error { message, .. } if message.contains("not an infrastructure failure")
            );
            assert!(!refused_as_non_infra, "{response:?}");
            std::fs::remove_dir_all(&scope_dir).ok();
        }

        #[tokio::test]
        async fn continue_is_refused_outright_when_the_task_has_no_previous_run() {
            let scope_dir = temp_dir("continue-no-previous-run");
            let (engine, _runtime) = continue_engine(scope_dir.clone(), true, RuntimeStatus::Gone);
            let task = task_for(&engine, false).await;

            let response = engine
                .handle_request(Request::TaskRun { override_wait: false, id: task.id.clone(), reason: None, continue_run: true })
                .await;
            let message = match response {
                Response::Error { message, .. } => message,
                other => panic!("expected a refusal, got {other:?}"),
            };
            assert!(message.contains("no previous run"), "{message}");
            std::fs::remove_dir_all(&scope_dir).ok();
        }

        #[tokio::test]
        async fn a_stale_token_after_continue_names_the_newer_run_instead_of_a_generic_refusal() {
            let scope_dir = temp_dir("continue-stale-token");
            let (engine, _runtime) = continue_engine(scope_dir.clone(), true, RuntimeStatus::Gone);
            let task = task_for(&engine, false).await;
            // Inlined, not `dispatched_then_failed`: the raw token has to be
            // captured before `fail_run` clears it -- once cleared, only its
            // digest (`spent_token_sha256`) survives, by design.
            let first = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), None).await.unwrap();
            let old_token = first.token.clone().expect("sanity: a fresh run has a token");
            engine.l4_service().fail_run(&first.id, FailKind::AckTimeout, "infra hiccup").await;
            let prev = engine.l4.store.get_run(&first.id).await.unwrap().unwrap();
            assert_eq!(
                prev.spent_token_sha256.as_deref(),
                Some(factory_core::run::token_digest(&old_token).as_str()),
                "sanity: finish_run recorded the digest of the token that just cleared"
            );
            seed_session_id(&engine, &prev, "sess-stale").await;
            let prev = engine.l4.store.get_run(&prev.id).await.unwrap().unwrap();

            let run = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), Some(prev.clone())).await.unwrap();
            assert!(run.resumed_session.is_some(), "sanity: this one actually resumed");

            let stale_report = TaskReport {
                artifacts: Vec::new(),
                status: Some(RunStatus::Running),
                message: Some("still going".into()),
                result: None,
                send_to: None,
                error: None,
                token: Some(old_token),
            };
            let err = engine.l4_service().report(&task.id, stale_report).await.unwrap_err();
            assert!(err.to_string().contains("a newer run"), "{err}");
            assert!(err.to_string().contains(&task.id), "{err}");

            // The new run's own token still works.
            let fresh_report = TaskReport {
                artifacts: Vec::new(),
                status: Some(RunStatus::Running),
                message: Some("continuing".into()),
                result: None,
                send_to: None,
                error: None,
                token: run.token.clone(),
            };
            assert!(engine.l4_service().report(&task.id, fresh_report).await.is_ok());

            std::fs::remove_dir_all(&scope_dir).ok();
        }
    }

    // ==================================================================
    // #218: `sandbox: openshell` at dispatch and at the end of a run
    // ==================================================================
    mod openshell_tests {
        use super::*;
        use async_trait::async_trait;
        use factory_core::adapter::agent::LaunchKind;
        use factory_core::adapter::runtime::RuntimeEventStream;
        use factory_core::task::{SessionRef, TaskReport};
        use factory_core::usage::{SessionUsage, SnapshotPoint, UsageSnapshot};
        use std::os::unix::fs::PermissionsExt;
        use std::sync::Mutex;

        #[derive(Default)]
        struct Recorder {
            starts: Mutex<Vec<StartRequest>>,
            submits: Mutex<Vec<String>>,
        }

        #[async_trait]
        impl AgentRuntime for Recorder {
            fn name(&self) -> &str {
                "stub-os"
            }
            async fn start(&self, req: &StartRequest) -> Result<SessionRef> {
                self.starts.lock().unwrap().push(req.clone());
                Ok(SessionRef { runtime: "stub-os".into(), handle: format!("stub-{}", req.id), meta: Default::default() })
            }
            async fn submit(&self, _session: &SessionRef, text: &str) -> Result<()> {
                self.submits.lock().unwrap().push(text.to_string());
                Ok(())
            }
            async fn status(&self, _session: &SessionRef) -> Result<RuntimeStatus> {
                // `#274`: `resolve_continue`'s rule 1 ("never run two
                // processes on one conversation") needs a session
                // confirmed `Gone` before it will even consider resuming --
                // true for every one of this module's own previous-run
                // sessions, which by test time are long past their report.
                Ok(RuntimeStatus::Gone)
            }
            async fn send_text(&self, _session: &SessionRef, _text: &str) -> Result<()> {
                Ok(())
            }
            async fn send_keys(&self, _session: &SessionRef, _keys: &[String]) -> Result<()> {
                Ok(())
            }
            async fn read(&self, _session: &SessionRef, _lines: u32) -> Result<String> {
                Ok(String::new())
            }
            async fn stop(&self, _session: &SessionRef) -> Result<()> {
                Ok(())
            }
            async fn watch(&self) -> Result<Option<RuntimeEventStream>> {
                Ok(None)
            }
        }

        /// A stand-in `openshell` that logs every call and succeeds. A
        /// `sandbox download` of the projects path (`#274`) writes one
        /// fake `.jsonl`, named by `session_id`, so a preserve/resume test
        /// can drive the whole thing without a real gateway.
        fn fake_cli(dir: &Path) -> PathBuf {
            fake_cli_with_session(dir, "resumed-session")
        }

        fn fake_cli_with_session(dir: &Path, session_id: &str) -> PathBuf {
            let path = dir.join("openshell");
            std::fs::write(
                &path,
                format!(
                    "#!/bin/sh\n\
                     echo \"$*\" >> '{calls}'\n\
                     case \"$1 $2\" in\n\
                     'status -o') echo '{{\"status\":\"connected\"}}' ;;\n\
                     'sandbox download') if [ \"$4\" = '/sandbox/.claude/projects' ]; then mkdir -p \"$5/cwd-dir\" && echo '{{}}' > \"$5/cwd-dir/{session_id}.jsonl\"; fi ;;\n\
                     esac\n",
                    calls = dir.join("calls").display(),
                ),
            )
            .unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        /// A CLI whose `sandbox download` of the preserved conversation's
        /// path always fails -- nothing ever gets preserved, the same
        /// observable outcome (to `resolve_continue`) as there having never
        /// been one.
        fn fake_cli_with_failing_preserve_download(dir: &Path) -> PathBuf {
            let path = dir.join("openshell");
            std::fs::write(
                &path,
                format!(
                    "#!/bin/sh\n\
                     echo \"$*\" >> '{calls}'\n\
                     case \"$1 $2\" in\n\
                     'status -o') echo '{{\"status\":\"connected\"}}' ;;\n\
                     'sandbox download') if [ \"$4\" = '/sandbox/.claude/projects' ]; then echo 'gateway unreachable' >&2; exit 1; fi ;;\n\
                     esac\n",
                    calls = dir.join("calls").display(),
                ),
            )
            .unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        /// Launches as `claude` and says, in its prompt, the working
        /// directory it was given -- what a harness tells its agent. Also
        /// declares a `claude`-shaped `resume_spec`, and a prompt that
        /// names the resumed session when `#274`'s tentative resume sets
        /// one, so a preserve/resume test can tell the two variants apart.
        struct CwdAgent;

        #[async_trait]
        impl Agent for CwdAgent {
            fn name(&self) -> &str {
                "cwd-probe"
            }
            async fn launch_spec(&self, ctx: &AgentContext) -> Result<LaunchSpec> {
                Ok(LaunchSpec { kind: LaunchKind::Named("claude".into()), args: Vec::new(), env: ctx.env(), agent_kind: None })
            }
            async fn prompt(&self, ctx: &AgentContext) -> Result<String> {
                let binding = ctx.binding()?;
                if let Some(session_id) = &binding.resumed_session {
                    return Ok(format!("Resumed session {session_id}. Working directory: {}\n", ctx.cwd.display()));
                }
                Ok(format!("Working directory: {}\n", ctx.cwd.display()))
            }
            fn resume_spec(&self, session_id: &str) -> Option<factory_core::adapter::agent::ResumeSpec> {
                Some(factory_core::adapter::agent::ResumeSpec { args: vec!["--resume".into(), session_id.into()] })
            }
        }

        fn engine_with(scope_dir: PathBuf, cli: &str) -> (Arc<Engine>, Arc<Recorder>) {
            let engine = test_engine(scope_dir);
            let runtime = Arc::new(Recorder::default());
            let mut registry = Registry::with_builtins();
            registry.add_runtime(runtime.clone(), "test");
            registry.add_agent(Arc::new(CwdAgent), "test");
            let mut factory = engine.factory_snapshot();
            let mut probe: ScopeAgent = serde_yaml_ng::from_str(&format!(
                "name: boxed-claude\nharness: cwd-probe\nsandbox: openshell\nopenshell:\n  image: img\n  cli: {cli}\n  policy: {{}}\n"
            ))
            .unwrap();
            probe.lifetime = Lifetime::Task;
            factory.config.scopes[0].agents.push(probe);
            factory.config.scopes[0].agents.push(ScopeAgent {
                name: Some("boxed".into()),
                harness: "shell".into(),
                lifetime: Lifetime::Task,
                role: Role::default(),
                autostart: None,
                args: Vec::new(),
                sandbox: Sandbox::Openshell,
                openshell: Some(
                    serde_yaml_ng::from_str(&format!(
                        "image: img\ncli: {cli}\nproviders: [factory-claude]\npolicy:\n  network_policies: {{}}\n"
                    ))
                    .unwrap(),
                ),
                provider: None,
                max_sessions: None,
            });
            let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
            let engine = Arc::new(Engine::new(factory, registry, store, PathBuf::from("factory"), Vec::new()));
            (engine, runtime)
        }

        async fn boxed_task(engine: &Arc<Engine>) -> Task {
            engine
                .create(NewTask {
                    title: "in the box".into(),
                    instructions: "echo hi".into(),
                    scope: Some("demo".into()),
                    agent: Some("boxed".into()),
                    runtime: Some("stub-os".into()),
                    worktree: Some(false),
                    ..Default::default()
                })
                .await
                .unwrap()
        }

        /// The sandboxed claude-code probe (`#274`): `boxed-claude`, whose
        /// `resume_spec` is real and whose conversation `finish` tries to
        /// preserve.
        async fn boxed_claude_task(engine: &Arc<Engine>) -> Task {
            engine
                .create(NewTask {
                    title: "resume me".into(),
                    instructions: "say".into(),
                    scope: Some("demo".into()),
                    agent: Some("boxed-claude".into()),
                    runtime: Some("stub-os".into()),
                    worktree: Some(false),
                    ..Default::default()
                })
                .await
                .unwrap()
        }

        /// A later usage snapshot naming `session_id` for the `cwd-probe`
        /// adapter -- what `resolve_continue` reads to find a resumable
        /// session, the same as a real `claude` run's `Stop` hook would
        /// relay over `FACTORY_URL`.
        async fn seed_session_id(engine: &Arc<Engine>, run: &Run, session_id: &str) {
            engine
                .l4.store
                .append_usage(&UsageSnapshot {
                    run_id: run.id.clone(),
                    task_id: run.task_id.clone(),
                    point: SnapshotPoint::RunEnd,
                    at: Utc::now(),
                    runtime: "stub-os".into(),
                    usage: Some(SessionUsage {
                        schema: 1,
                        handle: None,
                        sampled_at: None,
                        sessions: vec![serde_json::from_value(
                            serde_json::json!({ "session_id": session_id, "adapter": "cwd-probe" }),
                        )
                        .unwrap()],
                    }),
                    unknown: None,
                })
                .await
                .unwrap();
        }

        /// Ends the run `status`ed as the agent would, closing its session
        /// the same way a real report or cancel does -- `finish` runs in
        /// the background after, so a test polls for its result.
        async fn end_and_wait_for_teardown(engine: &Arc<Engine>, task_id: &str, run: &Run, tools: &Path, sandbox: &str) {
            engine
                .l4_service()
                .report(
                    task_id,
                    TaskReport {
                        artifacts: Vec::new(),
                        status: Some(RunStatus::Done),
                        message: None,
                        result: Some("ok".into()),
                        send_to: None,
                        error: None,
                        token: run.token.clone(),
                    },
                )
                .await
                .unwrap();
            for _ in 0..100 {
                let calls = std::fs::read_to_string(tools.join("calls")).unwrap_or_default();
                if calls.trim_end().ends_with(&format!("sandbox delete {sandbox}")) {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            panic!("sandbox {sandbox} was never torn down");
        }

        /// The host path names nothing inside the sandbox; the agent is told
        /// the directory it actually works in there.
        #[tokio::test]
        async fn a_sandboxed_agent_is_told_its_working_directory_inside_the_sandbox() {
            let scope_dir = temp_dir("openshell-cwd");
            let tools = temp_dir("openshell-cwd-cli");
            let cli = fake_cli(&tools);
            let (engine, runtime) = engine_with(scope_dir.clone(), &cli.display().to_string());
            let task = engine
                .create(NewTask {
                    title: "where am i".into(),
                    instructions: "say".into(),
                    scope: Some("demo".into()),
                    agent: Some("boxed-claude".into()),
                    runtime: Some("stub-os".into()),
                    worktree: Some(false),
                    ..Default::default()
                })
                .await
                .unwrap();
            let run = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), None).await.unwrap();
            let session = engine.l4.store.get_run(&run.id).await.unwrap().unwrap().session.unwrap();
            let teardown = crate::openshell::Teardown::from_meta(&session.meta).unwrap();
            let prompt = std::fs::read_to_string(teardown.state_dir.join(".factory-run/prompt.md")).unwrap();
            let name = scope_dir.file_name().unwrap().to_string_lossy().to_string();
            assert_eq!(prompt.trim(), format!("Working directory: /sandbox/work/{name}"));
            assert!(!prompt.contains(&scope_dir.display().to_string()), "{prompt}");
            // A harness is handed to the runtime as the agent it is (`#218`),
            // under its own argv0, so herdr can hold it like any other.
            let starts = runtime.starts.lock().unwrap().clone();
            assert_eq!(starts[0].launch.agent_kind.as_deref(), Some("claude"));
            let pane_script = std::fs::read_to_string(teardown.state_dir.join("pane.sh")).unwrap();
            assert!(pane_script.contains("exec -a 'claude' "), "{pane_script}");
            std::fs::remove_dir_all(&scope_dir).ok();
            std::fs::remove_dir_all(&tools).ok();
        }

        #[tokio::test]
        async fn restart_restores_and_deletes_an_orphan_even_after_its_declaration_is_removed() {
            let scope_dir = temp_dir("openshell-restart");
            let tools = temp_dir("openshell-restart-cli");
            let cli = fake_cli(&tools);
            let (engine, _) = engine_with(scope_dir.clone(), &cli.display().to_string());
            let task = boxed_task(&engine).await;
            let factory = engine.factory_snapshot();
            let run = uuid::Uuid::new_v4().to_string();
            let sandbox = format!("factory-{}", &run[..8]);
            let state = factory.factory_dir().join("openshell").join(&run);
            let download_dir = state.join("download");
            let base = vec![cli.display().to_string()];
            let teardown = crate::openshell::Teardown {
                sandbox: sandbox.clone(), state_dir: state.clone(), cwd: scope_dir.clone(),
                download: Some(vec![base[0].clone(), "sandbox".into(), "download".into(), sandbox.clone(), "/sandbox/work/demo".into(), download_dir.display().to_string()]),
                download_dir, fast_forward: false,
                delete: vec![base[0].clone(), "sandbox".into(), "delete".into(), sandbox.clone()],
                service_evidence: None,
                task: task.id.clone(),
                workdir: String::new(),
                preserve_download: None,
            };
            crate::openshell::Pending { instance: factory.config.instance.id.clone(), run: run.clone(), task: task.id.clone(), base, teardown }.save().unwrap();
            let listed = serde_json::json!({"sandboxes":[{"name":sandbox,"labels":{"factory.instance":factory.config.instance.id,"factory.run":run}}]});
            std::fs::write(&cli, format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\ncase \"$1 $2\" in\n'sandbox list') echo '{}' ;;\n'sandbox download') mkdir -p \"$5\" && echo restored > \"$5/result.txt\" ;;\nesac\n",
                tools.join("calls").display(), listed
            )).unwrap();
            engine.shared.factory.write().unwrap().config.scopes[0].agents.retain(|agent| agent.openshell.is_none());
            engine.reconcile_openshell().await;
            assert_eq!(std::fs::read_to_string(scope_dir.join("result.txt")).unwrap(), "restored\n");
            assert!(!state.exists());
            let calls = std::fs::read_to_string(tools.join("calls")).unwrap();
            assert!(calls.contains(&format!("sandbox delete {sandbox}")), "{calls}");
            assert!(engine.l4.store.run_entries(&run, 20).await.unwrap().iter().any(|entry| entry.message.contains("deleted sandbox")));
            std::fs::remove_dir_all(scope_dir).ok();
            std::fs::remove_dir_all(tools).ok();
        }

        #[tokio::test]
        async fn restart_keeps_active_sandboxes_even_when_their_declaration_is_removed() {
            let scope_dir = temp_dir("openshell-active");
            let tools = temp_dir("openshell-active-cli");
            let cli = fake_cli(&tools);
            let (engine, _) = engine_with(scope_dir.clone(), &cli.display().to_string());
            let task = boxed_task(&engine).await;
            let run = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), None).await.unwrap();
            let factory = engine.factory_snapshot();
            let state = factory.factory_dir().join("openshell").join(&run.id);
            assert!(state.join("pending.json").exists(), "saved before create, not just in session metadata");
            let sandbox = format!("factory-{}", &run.id[..8]);
            let listed = serde_json::json!({"sandboxes":[{"name":sandbox,"labels":{"factory.instance":factory.config.instance.id,"factory.run":run.id}}]});
            std::fs::write(&cli, format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\ncase \"$1 $2\" in\n'sandbox list') echo '{}' ;;\nesac\n",
                tools.join("calls").display(), listed
            )).unwrap();
            engine.shared.factory.write().unwrap().config.scopes[0].agents.retain(|agent| agent.openshell.is_none());
            engine.reconcile_openshell().await;
            assert!(state.join("pending.json").exists());
            let calls = std::fs::read_to_string(tools.join("calls")).unwrap();
            assert!(!calls.contains("sandbox delete"), "{calls}");
            std::fs::remove_dir_all(scope_dir).ok();
            std::fs::remove_dir_all(tools).ok();
        }

        #[tokio::test]
        async fn a_missing_openshell_cli_fails_the_run_and_never_starts_a_session() {
            let scope_dir = temp_dir("openshell-missing");
            let (engine, runtime) = engine_with(scope_dir.clone(), "/nonexistent/openshell");
            let task = boxed_task(&engine).await;
            engine.l4_service().start_run(&task.id, Trigger::Manual).await;
            assert!(runtime.starts.lock().unwrap().is_empty(), "nothing ran on the host in its place");
            let runs = engine.l4.store.runs(&task.id, 10).await.unwrap();
            let run = runs.iter().max_by_key(|r| r.attempt).expect("the run row exists and carries the failure");
            assert_eq!(run.status, RunStatus::Failed);
            let error = run.error.clone().unwrap_or_default();
            assert!(error.contains("openshell") && error.contains("does not exist"), "{error}");
            std::fs::remove_dir_all(&scope_dir).ok();
        }

        /// `#234`: an agent whose prerequisites the daemon keeps is
        /// dispatched only when they are ready, and fails closed -- with the
        /// one missing thing -- when they are not.
        #[tokio::test]
        async fn a_managed_agent_runs_only_when_ready_and_fails_closed_with_what_it_needs() {
            use factory_core::openshell::{Readiness, ReadinessState};
            let scope_dir = temp_dir("openshell-gate");
            let tools = temp_dir("openshell-gate-cli");
            let cli = fake_cli(&tools);
            let (engine, runtime) = engine_with(scope_dir.clone(), &cli.display().to_string());
            let mut factory = engine.factory_snapshot();
            let template = factory.config.scopes[0].agents.last().unwrap().clone();
            factory.config.scopes[0].agents.push(ScopeAgent {
                name: Some("boxed-managed".into()),
                openshell: Some(
                    serde_yaml_ng::from_str(&format!(
                        "cli: {}\nproviders:\n  - {{ name: factory-claude, type: claude-code-oauth, credential: {{ from: file, path: /nonexistent/token }} }}\npolicy:\n  network_policies: {{}}\n",
                        cli.display()
                    ))
                    .unwrap(),
                ),
                ..template
            });
            *engine.shared.factory.write().unwrap() = factory;
            let key = ("demo".to_string(), "boxed-managed".to_string());
            let at = Utc::now();
            let readiness = |state, thing: Option<&str>, image: Option<&str>| Readiness {
                state,
                thing: thing.map(str::to_string),
                command: thing.map(|_| "claude setup-token, then (umask 077; cat > /nonexistent/token)".to_string()),
                since: at,
                checked_at: at,
                image: image.map(str::to_string),
                image_build_failure: None,
                notes: vec![],
                expiring: vec![],
            };
            engine.l2.provision.set_for_test(&key, readiness(ReadinessState::Needs, Some("the credential for factory-claude from the file /nonexistent/token"), None));
            let task = engine
                .create(NewTask {
                    title: "managed".into(),
                    instructions: "echo hi".into(),
                    scope: Some("demo".into()),
                    agent: Some("boxed-managed".into()),
                    runtime: Some("stub-os".into()),
                    worktree: Some(false),
                    ..Default::default()
                })
                .await
                .unwrap();
            engine.l4_service().start_run(&task.id, Trigger::Manual).await;
            assert!(runtime.starts.lock().unwrap().is_empty(), "nothing ran on the host in its place");
            let run = engine.l4.store.runs(&task.id, 10).await.unwrap().into_iter().max_by_key(|r| r.attempt).unwrap();
            assert_eq!(run.status, RunStatus::Failed);
            let error = run.error.clone().unwrap_or_default();
            assert!(error.contains("needs the credential for factory-claude") && error.contains("claude setup-token") && error.contains("not started on the host"), "{error}");
            assert!(!std::fs::read_to_string(tools.join("calls")).unwrap_or_default().contains("sandbox create"));

            // Ready, with the image the daemon built: the run is made from it.
            // (A ready agent's managed sources are re-read at dispatch; the
            // provisioner's own tests cover that, and the suffixed names.)
            let mut factory = engine.factory_snapshot();
            let agent = factory.config.scopes[0].agents.iter_mut().find(|a| a.name.as_deref() == Some("boxed-managed")).unwrap();
            agent.openshell = Some(
                serde_yaml_ng::from_str(&format!("cli: {}\nproviders: [by-hand]\npolicy:\n  network_policies: {{}}\n", cli.display())).unwrap(),
            );
            *engine.shared.factory.write().unwrap() = factory;
            engine.l2.provision.set_for_test(&key, readiness(ReadinessState::Ready, None, Some("/images/built/factory-agent-rootfs.tar.gz")));
            let run = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), None).await.unwrap();
            assert_eq!(runtime.starts.lock().unwrap().len(), 1);
            let calls = std::fs::read_to_string(tools.join("calls")).unwrap();
            let create = calls.lines().find(|l| l.starts_with("sandbox create")).unwrap();
            assert!(create.contains("--from /images/built/factory-agent-rootfs.tar.gz") && create.contains("--provider by-hand"), "{create}");
            let _ = run;
            std::fs::remove_dir_all(&scope_dir).ok();
        }

        #[tokio::test]
        async fn a_sandboxed_run_launches_through_openshell_with_its_prompt_and_is_deleted_when_it_ends() {
            let scope_dir = temp_dir("openshell-run");
            let tools = temp_dir("openshell-cli");
            let cli = fake_cli(&tools);
            let (engine, runtime) = engine_with(scope_dir.clone(), &cli.display().to_string());
            let task = boxed_task(&engine).await;
            let run = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), None).await.unwrap();

            let starts = runtime.starts.lock().unwrap().clone();
            assert_eq!(starts.len(), 1);
            let LaunchKind::Command(words) = &starts[0].launch.kind else { panic!("{:?}", starts[0].launch.kind) };
            assert!(words[0].starts_with("bash '") && words[0].ends_with("pane.sh'"), "{words:?}");
            assert_eq!(starts[0].launch.agent_kind, None, "the shell agent has no harness for herdr to see");
            assert!(starts[0].launch.env.is_empty(), "no token in the pane's environment");
            assert!(runtime.submits.lock().unwrap().is_empty(), "the prompt went in at launch, never typed");
            let calls = std::fs::read_to_string(tools.join("calls")).unwrap();
            assert!(calls.contains("provider get factory-claude"), "{calls}");
            assert!(calls.contains("sandbox create --name factory-"), "{calls}");
            assert!(!calls.contains(run.token.as_deref().unwrap()), "the run token is on no command line: {calls}");

            let stored = engine.l4.store.get_run(&run.id).await.unwrap().unwrap();
            let session = stored.session.expect("the session is recorded");
            let teardown = crate::openshell::Teardown::from_meta(&session.meta).expect("with its teardown");
            assert!(teardown.state_dir.starts_with(engine.factory_snapshot().factory_dir()), "state lives under .factory");
            let pane_script = std::fs::read_to_string(teardown.state_dir.join("pane.sh")).unwrap();
            assert!(pane_script.contains("'sandbox' 'exec'") && pane_script.contains(&teardown.sandbox), "{pane_script}");
            assert!(!pane_script.contains("exec -a"), "a shell stays a shell: {pane_script}");
            let launcher = std::fs::read_to_string(teardown.state_dir.join(".factory-run/launch.sh")).unwrap();
            let workdir = format!("/sandbox/work/{}", scope_dir.file_name().unwrap().to_string_lossy());
            assert!(launcher.contains(&format!("cd '{workdir}'")), "{launcher}");

            engine
                .l4_service()
                .report(
                    &task.id,
                    TaskReport {
                        artifacts: Vec::new(),
                        status: Some(RunStatus::Done),
                        message: None,
                        result: Some("ok".into()),
                        send_to: None,
                        error: None,
                        token: run.token.clone(),
                    },
                )
                .await
                .unwrap();
            // The teardown runs in the background, after the run is settled.
            assert_eq!(engine.l4.store.get_run(&run.id).await.unwrap().unwrap().status, RunStatus::Done);
            let mut calls = String::new();
            let mut entries = Vec::new();
            for _ in 0..100 {
                calls = std::fs::read_to_string(tools.join("calls")).unwrap();
                entries = engine.l4.store.run_entries(&run.id, 100).await.unwrap();
                // Removal precedes the background worker's journal writes.
                // Await the whole observable contract, not only its first
                // side effects, before asserting the completion evidence.
                if calls.trim_end().ends_with(&format!("sandbox delete {}", teardown.sandbox))
                    && !teardown.state_dir.exists()
                    && entries.iter().any(|e| e.kind == "sandbox" && e.message.contains("deleted sandbox")) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            assert!(calls.trim_end().ends_with(&format!("sandbox delete {}", teardown.sandbox)), "{calls}");
            assert!(!teardown.state_dir.exists(), "the run's openshell files are gone");
            assert!(
                entries.iter().any(|e| e.kind == "sandbox" && e.message.contains("deleted sandbox")),
                "{:?}",
                entries.iter().map(|e| (&e.kind, &e.message)).collect::<Vec<_>>()
            );
            std::fs::remove_dir_all(&scope_dir).ok();
            std::fs::remove_dir_all(&tools).ok();
        }

        // -- `#274`: preserving a sandboxed conversation and resuming it --

        fn sessions_root(engine: &Arc<Engine>) -> PathBuf {
            engine.factory_snapshot().factory_dir().join("openshell").join("sessions")
        }

        #[tokio::test]
        async fn a_done_claude_runs_conversation_is_preserved_under_its_task_and_the_sandbox_still_deletes() {
            let scope_dir = temp_dir("openshell-preserve");
            let tools = temp_dir("openshell-preserve-cli");
            let cli = fake_cli(&tools);
            let (engine, _runtime) = engine_with(scope_dir.clone(), &cli.display().to_string());
            let task = boxed_claude_task(&engine).await;
            let run = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), None).await.unwrap();
            seed_session_id(&engine, &run, "resumed-session").await;
            let sandbox = format!("factory-{}", &run.id[..8]);
            end_and_wait_for_teardown(&engine, &task.id, &run, &tools, &sandbox).await;

            let record = crate::openshell::load_preserved(&sessions_root(&engine), &task.id)
                .expect("a preserved record for this task");
            assert_eq!(record.run, run.id);
            assert_eq!(record.task, task.id);
            assert_eq!(record.session_id.as_deref(), Some("resumed-session"));
            std::fs::remove_dir_all(&scope_dir).ok();
            std::fs::remove_dir_all(&tools).ok();
        }

        #[tokio::test]
        async fn a_blocked_timeout_reclaim_also_preserves_the_conversation() {
            // Lifecycle decision 1: every teardown preserves first, blocked
            // timeout included -- the same path `finish` always takes,
            // whatever ended the run.
            let scope_dir = temp_dir("openshell-preserve-blocked");
            let tools = temp_dir("openshell-preserve-blocked-cli");
            let cli = fake_cli(&tools);
            let (engine, _runtime) = engine_with(scope_dir.clone(), &cli.display().to_string());
            let task = boxed_claude_task(&engine).await;
            let run = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), None).await.unwrap();
            seed_session_id(&engine, &run, "resumed-session").await;
            engine.l4_service().fail_run(&run.id, FailKind::BlockedTimeout, "nobody answered").await;
            let sandbox = format!("factory-{}", &run.id[..8]);
            for _ in 0..100 {
                let calls = std::fs::read_to_string(tools.join("calls")).unwrap_or_default();
                if calls.trim_end().ends_with(&format!("sandbox delete {sandbox}")) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            assert!(crate::openshell::load_preserved(&sessions_root(&engine), &task.id).is_some());
            std::fs::remove_dir_all(&scope_dir).ok();
            std::fs::remove_dir_all(&tools).ok();
        }

        #[tokio::test]
        async fn continuing_restores_the_preserved_conversation_and_launches_with_resume() {
            let scope_dir = temp_dir("openshell-resume");
            let tools = temp_dir("openshell-resume-cli");
            let cli = fake_cli(&tools);
            let (engine, _runtime) = engine_with(scope_dir.clone(), &cli.display().to_string());
            let task = boxed_claude_task(&engine).await;
            let first = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), None).await.unwrap();
            seed_session_id(&engine, &first, "resumed-session").await;
            let sandbox = format!("factory-{}", &first.id[..8]);
            end_and_wait_for_teardown(&engine, &task.id, &first, &tools, &sandbox).await;
            let preserved = crate::openshell::load_preserved(&sessions_root(&engine), &task.id).unwrap();
            let first = engine.l4.store.get_run(&first.id).await.unwrap().unwrap();

            let second = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), Some(first.clone())).await.unwrap();

            assert_eq!(second.resumed_session.as_deref(), Some("resumed-session"));
            assert!(
                continue_fallback_reasons_openshell(&engine, &task.id).await.is_empty(),
                "a matching preserved session resumes outright"
            );
            let teardown = crate::openshell::Teardown::from_meta(&second.session.as_ref().unwrap().meta).unwrap();
            let launcher = std::fs::read_to_string(teardown.state_dir.join(".factory-run/launch.sh")).unwrap();
            assert!(launcher.contains("--resume") && launcher.contains("resumed-session"), "{launcher}");
            let calls = std::fs::read_to_string(tools.join("calls")).unwrap();
            assert!(
                calls.contains("sandbox upload") && calls.contains("/sandbox/.claude"),
                "the preserved conversation was uploaded into the new sandbox: {calls}"
            );
            assert!(!preserved.workdir.is_empty(), "the record carries the workdir it was captured from");
            assert!(
                crate::openshell::load_preserved(&sessions_root(&engine), &task.id).is_none(),
                "the preserved copy is gone once it is used"
            );
            std::fs::remove_dir_all(&scope_dir).ok();
            std::fs::remove_dir_all(&tools).ok();
        }

        async fn continue_fallback_reasons_openshell(engine: &Arc<Engine>, task_id: &str) -> Vec<String> {
            engine
                .l4.store
                .entries(task_id, 50)
                .await
                .unwrap()
                .into_iter()
                .filter(|e| e.kind == "continue_fallback")
                .map(|e| e.message)
                .collect()
        }

        #[tokio::test]
        async fn continuing_falls_back_to_fresh_when_the_preserved_sessions_id_does_not_match() {
            let scope_dir = temp_dir("openshell-resume-mismatch");
            let tools = temp_dir("openshell-resume-mismatch-cli");
            let cli = fake_cli(&tools);
            let (engine, _runtime) = engine_with(scope_dir.clone(), &cli.display().to_string());
            let task = boxed_claude_task(&engine).await;
            let first = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), None).await.unwrap();
            // The preserved conversation is for "resumed-session" (the fake
            // CLI's default), but the previous run's own resolved session
            // id -- what a real Stop hook would have relayed -- differs.
            seed_session_id(&engine, &first, "a-different-session").await;
            let sandbox = format!("factory-{}", &first.id[..8]);
            end_and_wait_for_teardown(&engine, &task.id, &first, &tools, &sandbox).await;
            assert!(crate::openshell::load_preserved(&sessions_root(&engine), &task.id).is_some());
            let first = engine.l4.store.get_run(&first.id).await.unwrap().unwrap();

            let second = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), Some(first.clone())).await.unwrap();

            assert!(second.resumed_session.is_none());
            let reasons = continue_fallback_reasons_openshell(&engine, &task.id).await;
            assert!(reasons.iter().any(|r| r.contains("does not match")), "{reasons:?}");
            let teardown = crate::openshell::Teardown::from_meta(&second.session.as_ref().unwrap().meta).unwrap();
            let launcher = std::fs::read_to_string(teardown.state_dir.join(".factory-run/launch.sh")).unwrap();
            assert!(!launcher.contains("--resume"), "{launcher}");
            std::fs::remove_dir_all(&scope_dir).ok();
            std::fs::remove_dir_all(&tools).ok();
        }

        /// `#274`: a sandboxed run that timed out mid-turn recorded no
        /// session id (no usage snapshot, no `turn-ended` hook). Ends the
        /// first run that way, lets `finish` preserve its conversation, and
        /// lets `tamper` change the preserved copy before the continue.
        async fn continue_after_unrecorded_run(
            name: &str,
            tamper: impl FnOnce(&Path, &crate::openshell::PreservedRecord),
        ) -> (Arc<Engine>, Run, Vec<String>, PathBuf, PathBuf) {
            let scope_dir = temp_dir(&format!("openshell-{name}"));
            let tools = temp_dir(&format!("openshell-{name}-cli"));
            let cli = fake_cli(&tools);
            let (engine, _runtime) = engine_with(scope_dir.clone(), &cli.display().to_string());
            let task = boxed_claude_task(&engine).await;
            let first = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), None).await.unwrap();
            let sandbox = format!("factory-{}", &first.id[..8]);
            end_and_wait_for_teardown(&engine, &task.id, &first, &tools, &sandbox).await;
            let root = sessions_root(&engine);
            let record = crate::openshell::load_preserved(&root, &task.id).expect("preserved");
            assert_eq!(record.session_id.as_deref(), Some("resumed-session"));
            tamper(&crate::openshell::preserved_dir(&root, &task.id), &record);
            let first = engine.l4.store.get_run(&first.id).await.unwrap().unwrap();
            assert!(first.turn_ended_session_id.is_none());
            let second = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), Some(first)).await.unwrap();
            let reasons = continue_fallback_reasons_openshell(&engine, &task.id).await;
            (engine, second, reasons, scope_dir, tools)
        }

        #[tokio::test]
        async fn continuing_a_sandboxed_run_with_no_recorded_session_id_resumes_from_its_preserved_record() {
            let (engine, second, reasons, scope_dir, tools) = continue_after_unrecorded_run("resume-unrecorded", |_, _| {}).await;
            assert_eq!(second.resumed_session.as_deref(), Some("resumed-session"));
            assert!(reasons.is_empty(), "{reasons:?}");
            let teardown = crate::openshell::Teardown::from_meta(&second.session.as_ref().unwrap().meta).unwrap();
            let launcher = std::fs::read_to_string(teardown.state_dir.join(".factory-run/launch.sh")).unwrap();
            assert!(launcher.contains("--resume") && launcher.contains("resumed-session"), "{launcher}");
            let journal = engine.l4.store.entries(&second.task_id, 50).await.unwrap();
            assert!(
                journal.iter().any(|e| e.kind == "continue_session_source" && e.message.contains("preserved sandbox conversation")),
                "the source of the id is journaled"
            );
            std::fs::remove_dir_all(&scope_dir).ok();
            std::fs::remove_dir_all(&tools).ok();
        }

        #[tokio::test]
        async fn a_preserved_record_for_another_run_does_not_supply_the_session_id() {
            let (_engine, second, reasons, scope_dir, tools) = continue_after_unrecorded_run("resume-other-run", |dir, record| {
                let other = crate::openshell::PreservedRecord { run: "some-earlier-run".into(), ..record.clone() };
                std::fs::write(dir.join("record.json"), serde_json::to_vec(&other).unwrap()).unwrap();
            })
            .await;
            assert!(second.resumed_session.is_none());
            assert!(reasons.iter().any(|r| r.contains("no session id was recorded")), "{reasons:?}");
            std::fs::remove_dir_all(&scope_dir).ok();
            std::fs::remove_dir_all(&tools).ok();
        }

        #[tokio::test]
        async fn a_preserved_record_whose_jsonl_is_missing_does_not_supply_the_session_id() {
            let (_engine, second, reasons, scope_dir, tools) = continue_after_unrecorded_run("resume-no-jsonl", |dir, _| {
                std::fs::remove_dir_all(dir.join("projects")).unwrap();
            })
            .await;
            assert!(second.resumed_session.is_none());
            assert!(reasons.iter().any(|r| r.contains("no session id was recorded")), "{reasons:?}");
            std::fs::remove_dir_all(&scope_dir).ok();
            std::fs::remove_dir_all(&tools).ok();
        }

        #[tokio::test]
        async fn a_failed_preserve_download_leaves_nothing_and_continuing_falls_back_to_fresh() {
            let scope_dir = temp_dir("openshell-resume-none");
            let tools = temp_dir("openshell-resume-none-cli");
            let cli = fake_cli_with_failing_preserve_download(&tools);
            let (engine, _runtime) = engine_with(scope_dir.clone(), &cli.display().to_string());
            let task = boxed_claude_task(&engine).await;
            let first = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), None).await.unwrap();
            seed_session_id(&engine, &first, "resumed-session").await;
            let sandbox = format!("factory-{}", &first.id[..8]);
            end_and_wait_for_teardown(&engine, &task.id, &first, &tools, &sandbox).await;
            // `calls` ending in `sandbox delete` only proves `finish`'s own
            // work is done; its journal writes are a separate loop after
            // that (same race the module's other end-to-end test notes).
            let mut journaled = false;
            for _ in 0..100 {
                if engine.l4.store.run_entries(&first.id, 50).await.unwrap().iter()
                    .any(|e| e.kind == "sandbox" && e.message.contains("not preserved")) {
                    journaled = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            assert!(journaled, "the failed download is journaled");
            assert!(crate::openshell::load_preserved(&sessions_root(&engine), &task.id).is_none());
            let first = engine.l4.store.get_run(&first.id).await.unwrap().unwrap();

            let second = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), Some(first.clone())).await.unwrap();

            assert!(second.resumed_session.is_none());
            let reasons = continue_fallback_reasons_openshell(&engine, &task.id).await;
            assert!(reasons.iter().any(|r| r.contains("no preserved")), "{reasons:?}");
            std::fs::remove_dir_all(&scope_dir).ok();
            std::fs::remove_dir_all(&tools).ok();
        }

        #[tokio::test]
        async fn a_failed_restore_upload_falls_back_to_fresh_and_corrects_the_run_row() {
            let scope_dir = temp_dir("openshell-resume-upload-fails");
            let tools = temp_dir("openshell-resume-upload-fails-cli");
            let cli = fake_cli(&tools);
            // Preserving still has to work for the first run's teardown;
            // only the *second* dispatch's restore upload fails -- so make
            // `sandbox upload` fail only once destination is `/sandbox/.claude`.
            let script = std::fs::read_to_string(&cli).unwrap().replace(
                "case \"$1 $2\" in\n",
                "case \"$1 $2\" in\n'sandbox upload') if [ \"$5\" = '/sandbox/.claude' ]; then echo 'gateway unreachable' >&2; exit 1; fi ;;\n",
            );
            std::fs::write(&cli, script).unwrap();
            let (engine, _runtime) = engine_with(scope_dir.clone(), &cli.display().to_string());
            let task = boxed_claude_task(&engine).await;
            let first = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), None).await.unwrap();
            seed_session_id(&engine, &first, "resumed-session").await;
            let sandbox = format!("factory-{}", &first.id[..8]);
            end_and_wait_for_teardown(&engine, &task.id, &first, &tools, &sandbox).await;
            assert!(crate::openshell::load_preserved(&sessions_root(&engine), &task.id).is_some());
            let first = engine.l4.store.get_run(&first.id).await.unwrap().unwrap();

            let second = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), Some(first.clone())).await.unwrap();

            // The run row's tentative claim is corrected once the upload's
            // real failure is known -- `resumed_session` is set once, at
            // dispatch, except for exactly this case (`#274`).
            assert!(second.resumed_session.is_none(), "{second:?}");
            assert_eq!(second.resume_context.as_ref().map(|c| c.resumes), Some(0));
            let reasons = continue_fallback_reasons_openshell(&engine, &task.id).await;
            assert!(reasons.iter().any(|r| r.contains("session was not possible")), "{reasons:?}");
            let teardown = crate::openshell::Teardown::from_meta(&second.session.as_ref().unwrap().meta).unwrap();
            let launcher = std::fs::read_to_string(teardown.state_dir.join(".factory-run/launch.sh")).unwrap();
            assert!(!launcher.contains("--resume"), "a failed restore never launches with --resume: {launcher}");
            std::fs::remove_dir_all(&scope_dir).ok();
            std::fs::remove_dir_all(&tools).ok();
        }

        #[tokio::test]
        async fn closing_the_task_removes_its_preserved_session() {
            let scope_dir = temp_dir("openshell-preserve-close");
            let tools = temp_dir("openshell-preserve-close-cli");
            let cli = fake_cli(&tools);
            let (engine, _runtime) = engine_with(scope_dir.clone(), &cli.display().to_string());
            let task = boxed_claude_task(&engine).await;
            let run = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), None).await.unwrap();
            seed_session_id(&engine, &run, "resumed-session").await;
            // Failed, not done: a `done` task reads as already closed
            // (`TaskStatus::Done` derives `CloseReason::Completed`), which
            // `Request::TaskClose` refuses outright -- this test wants a
            // still-open task to actually close.
            engine.l4_service().fail_run(&run.id, FailKind::AckTimeout, "outage").await;
            let sandbox = format!("factory-{}", &run.id[..8]);
            for _ in 0..100 {
                let calls = std::fs::read_to_string(tools.join("calls")).unwrap_or_default();
                if calls.trim_end().ends_with(&format!("sandbox delete {sandbox}")) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            assert!(crate::openshell::load_preserved(&sessions_root(&engine), &task.id).is_some());

            let response = engine
                .handle_request(Request::TaskClose {
                    id: task.id.clone(),
                    reason: factory_core::task::CloseReason::NotPlanned,
                    duplicate_of: None,
                    note: Some("done with this".into()),
                })
                .await;
            assert!(!matches!(response, Response::Error { .. }), "{response:?}");

            assert!(crate::openshell::load_preserved(&sessions_root(&engine), &task.id).is_none());
            std::fs::remove_dir_all(&scope_dir).ok();
            std::fs::remove_dir_all(&tools).ok();
        }

        #[tokio::test]
        async fn deleting_the_task_removes_its_preserved_session() {
            let scope_dir = temp_dir("openshell-preserve-delete");
            let tools = temp_dir("openshell-preserve-delete-cli");
            let cli = fake_cli(&tools);
            let (engine, _runtime) = engine_with(scope_dir.clone(), &cli.display().to_string());
            let task = boxed_claude_task(&engine).await;
            let run = engine.l4_service().dispatch(&task.id, Trigger::Manual, Due::now(), None).await.unwrap();
            seed_session_id(&engine, &run, "resumed-session").await;
            let sandbox = format!("factory-{}", &run.id[..8]);
            end_and_wait_for_teardown(&engine, &task.id, &run, &tools, &sandbox).await;
            assert!(crate::openshell::load_preserved(&sessions_root(&engine), &task.id).is_some());

            let response = engine.handle_request(Request::TaskDelete { id: task.id.clone() }).await;
            assert!(!matches!(response, Response::Error { .. }), "{response:?}");

            assert!(crate::openshell::load_preserved(&sessions_root(&engine), &task.id).is_none());
            std::fs::remove_dir_all(&scope_dir).ok();
            std::fs::remove_dir_all(&tools).ok();
        }
    }
}
