//! The core. Everything an interface can ask for arrives here as a `Request`
//! and leaves as a `Response`; the interfaces themselves hold no logic.

use chrono::Utc;
use factory_core::adapter::agent::{
    knowledge_hints_path, run_guide_path, run_hook_settings_path, run_shell_script_path, truncate_tail,
    upstream_output_path, AgentContext, AgentExitContext, LaunchSpec, TaskBinding, UpstreamOutput,
    UPSTREAM_RESULT_BYTE_CAP,
};
use factory_core::adapter::runtime::{
    RuntimeConnectionDiagnostic, RuntimeStatus, Screen, StartRequest, StatusReport, StatusSource,
};
use factory_core::adapter::TaskStore;
use factory_core::config::{Factory, Sandbox, ScopeAgent, SHELL_HARNESS};
use factory_core::error::{FactoryError, Result};
use factory_core::event::{Event, EventBus};
use factory_core::protocol::{
    AgentActivity, AgentView, CredentialRow, DaemonFacts, Envelope, InterfaceFacts, Payload,
    ProviderAgent, ProviderRow, Request, Response, RuntimeConnectionView, SandboxRow, ScopeView,
    StatusInfo, StoreFacts, UnassignedAgent,
};
use factory_core::agent::{AgentSession, AgentState};
use factory_core::role::{Role, Roles};
use factory_core::run::{BlockSource, FailKind, NewRun, Run, RunPatch, RunStatus, Trigger};
use factory_core::task::{
    NewTask, PendingRetry, RetryPolicy, Task, TaskEntry, TaskFailure, TaskFilter, TaskPatch, TaskReport,
    TaskStatus, WorkflowOrigin,
};
use factory_plugins::registry::Registry;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::schedule;
use crate::worktree;

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

/// Why a `schedule_skipped` entry's slots passed -- `data.reason` on the
/// entry. Two causes produce the same state (a past `next_run_at` on a
/// pending task), so the entry says which one it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SkipReason {
    /// The task's previous run was still going when the first skipped slot
    /// came round -- `due()` only fires a `pending` task.
    StillActive,
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
fn dispatchable_from(
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
/// open at `first` (started by then, not yet ended) or it was not.
fn skip_reason_of(newest: Option<&Run>, first: chrono::DateTime<Utc>) -> SkipReason {
    match newest {
        Some(run) if run.started_at <= first && run.ended_at.is_none_or(|end| end >= first) => {
            SkipReason::StillActive
        }
        _ => SkipReason::NotRunning,
    }
}

impl SkipReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::StillActive => "still_active",
            Self::NotRunning => "not_running",
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Self::StillActive => "the previous run was still going",
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
    /// The instance settings are stable, while a successful scope-config edit
    /// replaces the affected scope in this snapshot. Readers clone it before
    /// awaiting so no filesystem or runtime operation holds the lock.
    factory: std::sync::RwLock<Factory>,
    /// Serializes read-modify-write edits to local scope config files.
    pub(crate) configuration_edit: std::sync::Mutex<()>,
    pub registry: Registry,
    pub store: Arc<dyn TaskStore>,
    pub(crate) workflows: crate::workflows::WorkflowStore,
    pub(crate) workflow_edit: tokio::sync::Mutex<()>,
    pub(crate) bench: crate::bench::BenchStore,
    /// The attestations audit trail -- see `policies::PolicyStore`. Nothing
    /// else in a policy request is stateful: the catalogues are read fresh
    /// off disk on every call, like `.factory/knowledge/` and
    /// `.factory/datasets/`.
    pub(crate) policies: crate::policies::PolicyStore,
    /// The check-ins audit trail -- see `goals::GoalsStore`. Nothing else in
    /// a goals request is stateful: the direction and cycle catalogues are
    /// read fresh off disk on every call, like the policy catalogues.
    pub(crate) goals: crate::goals::GoalsStore,
    /// What happened to backups -- see `backup::BackupStore`. The archives
    /// themselves are the destination's, listed fresh on every request.
    pub(crate) backups: crate::backup::BackupStore,
    /// Held for the whole of a backup or a verification, so the job and a
    /// person can never run two at once over one destination. Taken with
    /// `try_lock`: a second request is refused, never queued.
    pub(crate) backup_busy: tokio::sync::Mutex<()>,
    /// Serializes a bench run's own read-modify-write: choosing which
    /// pending attempts to start, and recomputing the run's own status once
    /// every attempt has settled. Coarse -- one lock for every run, the same
    /// trade `workflow_edit` already makes -- rather than one per run.
    pub(crate) bench_edit: tokio::sync::Mutex<()>,
    /// Serializes a run's usage read-modify-write -- append a snapshot, read
    /// them all back, write the derived `usage` -- so two snapshots landing
    /// together never leave the older sum on the run (`costs.rs`).
    pub(crate) usage_edit: tokio::sync::Mutex<()>,
    /// Task ids already enqueued for judgement, or currently being judged by
    /// the worker: the guard that keeps a report and a cancel racing each
    /// other (or a live enqueue racing recovery's own sweep) from queuing
    /// the same attempt's gate command twice over. The worker itself only
    /// ever processes one task id at a time, so this is a dedup on the
    /// queue, not a lock a gate holds -- nothing here is held for the
    /// gate's own duration.
    pub(crate) bench_judging: std::sync::Mutex<std::collections::HashSet<String>>,
    /// Where a bench attempt's judgement is actually carried out: sending a
    /// task id here is the only thing `record_bench_task_state` and
    /// `sync_bench_for_task` do now. Judging never runs on a caller's own
    /// path -- a request handler, `fail_run`, the scheduler watchdog -- only
    /// on `spawn_bench_judge`'s dedicated worker, which receives from the
    /// other end of this channel. Unbounded: a bounded channel's `send`
    /// would have to be awaited, reintroducing the exact "the caller waits
    /// on a gate" problem this exists to remove, and a full channel's
    /// `try_send` would silently drop a judgement.
    pub(crate) bench_judge_tx: tokio::sync::mpsc::UnboundedSender<String>,
    /// Taken by `spawn_bench_judge` the one time it runs. `Engine::new`
    /// cannot itself spawn the worker -- it returns `Self`, not `Arc<Self>`,
    /// and the worker needs to hold an `Arc` to call back into judging and
    /// advancing -- so the receiver waits here until an `Arc<Engine>` exists
    /// to spawn it from.
    pub(crate) bench_judge_rx: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<String>>>,
    pub(crate) dataset_locks: crate::datasets::DatasetLocks,
    /// Run ids queued for, or in, verification (`#118`). The bool remembers
    /// a wake-up that arrived while the verifier owned the run, so releasing
    /// that ownership replays it instead of losing a fast review result.
    /// See `verification.rs`.
    pub(crate) verifying: std::sync::Mutex<std::collections::HashMap<String, bool>>,
    /// Where `report` hands a `done` that needs verifying. The verifier
    /// (`spawn_verifier`) holds the other end; like `bench_judge_tx`, this is
    /// how a gate that runs for minutes stays off the report's own path.
    pub(crate) verify_tx: tokio::sync::mpsc::UnboundedSender<String>,
    pub(crate) verify_rx: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<String>>>,
    pub bus: EventBus,
    pub factory_bin: PathBuf,
    started: Instant,
    /// The wall-clock moment this daemon came up. A slot that fell before it
    /// could not have been dispatched before it -- see `Engine::due_for`.
    pub(crate) booted_at: chrono::DateTime<Utc>,
    interfaces: Vec<String>,
    /// The last liveness we wrote down for each session, so a poll that finds
    /// no change writes nothing. Lost on restart, which is right: after a
    /// restart the first observation is genuinely new information.
    pub(crate) seen_status: std::sync::Mutex<std::collections::HashMap<String, RuntimeStatus>>,
    /// The last walk of each scope's directory, with when it was taken. The
    /// site view asks for a scope's size on every run and agent event now, and
    /// a repository does not change size between two of them -- see
    /// `site::WALK_TTL`.
    pub(crate) site_walks:
        std::sync::Mutex<std::collections::HashMap<String, (Instant, crate::site::Measured)>>,
    /// Whether each scope's directory can host a worktree, with when that was
    /// asked. `worktree::capability` is one or two `git` subprocesses, and
    /// `scope_views` asks it per configured scope, potentially many on a real
    /// instance, on an endpoint the
    /// Agents view refetches on every run and agent event. A directory does
    /// not become a git repository between two of those. Cached for
    /// `CAPABILITY_TTL`; the first board after a restart still pays in full,
    /// the same trade `site::WALK_TTL` already makes.
    pub(crate) worktree_caps:
        std::sync::Mutex<std::collections::HashMap<String, (Instant, (bool, Option<String>))>>,
    /// The tier and activity level each hall was last drawn at, which is what
    /// makes both steps sticky instead of flipping whenever a metric sits on a
    /// threshold. Lost on restart, like `seen_status`, and for the same
    /// reason: the first answer after one is genuinely new.
    pub(crate) site_memory: std::sync::Mutex<
        std::collections::HashMap<
            String,
            (
                factory_core::building::Tier,
                factory_core::building::ActivityLevel,
            ),
        >,
    >,
    /// Keeps the host awake for as long as any run is active -- see
    /// `crate::power` and issue #61. Acquired once a run's row exists
    /// (`dispatch`), released for every run `close_session` ever sees,
    /// terminal outcome or not.
    pub(crate) power: crate::power::PowerAssertions,
    /// Whether each harness binary starts, probed before a dispatch and
    /// cached -- see `crate::harness_health` and issue #131.
    pub(crate) harness: crate::harness_health::HarnessHealth,
    /// The fingerprint of what the last successful `Request::Quality`
    /// loaded -- every profile in `.factory/quality/` and every scope's
    /// quality chain -- with when it loaded them, so the next read can tell
    /// that it moved and publish `Event::QualityChanged`, and an older read
    /// finishing late never overwrites a newer one (`quality/mod.rs`).
    /// `None` until the first read, which has nothing to compare against
    /// and so publishes nothing; lost on restart for the same reason
    /// `seen_status` is.
    pub(crate) quality_seen: std::sync::Mutex<Option<(Instant, u64)>>,
    /// Each scope's guide quality block, with when it was judged and the
    /// profiles' fingerprint it was judged under -- reused for
    /// `quality::GUIDE_TTL` so a burst of dispatches does not each read the
    /// run history. An async lock, held across the judging itself, so that
    /// burst waits for one answer rather than computing it once each.
    pub(crate) quality_guide_cache: tokio::sync::Mutex<crate::quality::GuideCache>,
    /// Serializes the two things that move a schedule's next slot on a
    /// person's or the clock's account: the scheduler firing a due task, and
    /// `task.skip_next`. Each re-reads the task under it, so a slot is
    /// either fired or skipped, never both (`#106`).
    pub(crate) schedule_lock: tokio::sync::Mutex<()>,
    /// The Operations report's triggered signposts, kept for a minute --
    /// computing them runs the metrics they name. See
    /// `Engine::triggered_signposts_cached`.
    pub(crate) signpost_cache: std::sync::Mutex<Option<crate::operations::SignpostCache>>,
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
        let power = crate::power::PowerAssertions::new(factory.config.daemon.power_assertion);
        Self {
            factory: std::sync::RwLock::new(factory),
            configuration_edit: Default::default(),
            registry,
            store,
            workflows: crate::workflows::WorkflowStore::in_memory()
                .expect("an in-memory workflow store should open"),
            workflow_edit: tokio::sync::Mutex::new(()),
            bench: crate::bench::BenchStore::in_memory()
                .expect("an in-memory bench store should open"),
            policies: crate::policies::PolicyStore::in_memory()
                .expect("an in-memory policy store should open"),
            goals: crate::goals::GoalsStore::in_memory()
                .expect("an in-memory goals store should open"),
            backups: crate::backup::BackupStore::in_memory()
                .expect("an in-memory backup store should open"),
            backup_busy: tokio::sync::Mutex::new(()),
            bench_edit: tokio::sync::Mutex::new(()),
            usage_edit: tokio::sync::Mutex::new(()),
            bench_judging: Default::default(),
            bench_judge_tx,
            bench_judge_rx: std::sync::Mutex::new(Some(bench_judge_rx)),
            dataset_locks: Default::default(),
            verifying: Default::default(),
            verify_tx,
            verify_rx: std::sync::Mutex::new(Some(verify_rx)),
            bus: EventBus::default(),
            factory_bin,
            started: Instant::now(),
            booted_at: Utc::now(),
            interfaces,
            seen_status: Default::default(),
            site_walks: Default::default(),
            worktree_caps: Default::default(),
            site_memory: Default::default(),
            power,
            harness: crate::harness_health::HarnessHealth::new(),
            quality_seen: Default::default(),
            quality_guide_cache: Default::default(),
            schedule_lock: tokio::sync::Mutex::new(()),
            signpost_cache: std::sync::Mutex::new(None),
        }
    }

    /// Production replaces the in-memory test repository with the instance
    /// database. Workflow state belongs to the daemon ledger, regardless of
    /// which task-store adapter a scope selects.
    pub fn with_workflow_store(mut self, workflows: crate::workflows::WorkflowStore) -> Self {
        self.workflows = workflows;
        self
    }

    /// The same, for bench runs.
    pub fn with_bench_store(mut self, bench: crate::bench::BenchStore) -> Self {
        self.bench = bench;
        self
    }

    /// The same, for policy attestations.
    pub fn with_policy_store(mut self, policies: crate::policies::PolicyStore) -> Self {
        self.policies = policies;
        self
    }

    /// The same, for goals check-ins.
    pub fn with_goals_store(mut self, goals: crate::goals::GoalsStore) -> Self {
        self.goals = goals;
        self
    }

    /// The same, for the backup history.
    pub fn with_backup_store(mut self, backups: crate::backup::BackupStore) -> Self {
        self.backups = backups;
        self
    }

    /// A coherent configuration snapshot for one operation. A poisoned lock
    /// still contains the last value; recovering it keeps a failed request
    /// from taking the daemon down with it.
    pub(crate) fn factory_snapshot(&self) -> Factory {
        self.factory
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
            .factory
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        factory.config.roles = roles;
    }

    pub(crate) fn replace_scope(&self, id: &str, replacement: factory_core::config::Scope) {
        let mut factory = self
            .factory
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
        // `dispatch_request` is an exhaustive match over the whole API. Keep
        // that large future off each interface task's stack as new request
        // families are added.
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

    async fn dispatch_request(
        self: &Arc<Self>,
        caller: &crate::access::Caller,
        req: Request,
    ) -> Result<Payload> {
        match req {
            Request::Status => Ok(Payload::Status {
                status: self.status().await?,
            }),
            Request::Adapters => Ok(self.registry.list().into()),
            Request::RuntimeConnections => Ok(Payload::RuntimeConnections {
                runtimes: self.runtime_connections().await,
            }),
            Request::Agents => {
                let (scopes, available) = self.scope_views().await?;
                let (roles, scope_roles) = self.role_views();
                Ok(Payload::Scopes {
                    scopes,
                    available,
                    roles,
                    scope_roles,
                })
            }
            Request::RoleList { scope } => Ok(Payload::Roles {
                board: self.role_board(scope.as_deref()).await?,
            }),
            Request::RoleDefine {
                scope,
                name,
                role,
                replace,
            } => {
                let (scope, name) = self.define_role(&scope, &name, role, replace).await?;
                self.bus.publish(Event::RolesChanged { scope, name });
                Ok(Payload::Ok)
            }
            Request::RoleDelete { scope, name } => {
                let (scope, name) = self.delete_role(&scope, &name).await?;
                self.bus.publish(Event::RolesChanged { scope, name });
                Ok(Payload::Deleted { deleted: true })
            }
            Request::Occupancy { minutes, from, to } => Ok(Payload::Occupancy {
                occupancy: self.occupancy(minutes, from, to).await?,
            }),
            Request::Production { minutes, bin, scope } => Ok(Payload::Production {
                production: self.production(minutes, bin, scope).await?,
            }),
            Request::SiteFootprint => Ok(Payload::SiteFootprint {
                footprint: self.site_footprint().await?,
            }),
            Request::Environment => {
                let (sandboxes, credentials) = self.environment().await?;
                Ok(Payload::Environment {
                    sandboxes,
                    credentials,
                })
            }
            Request::Dependencies { scope } => Ok(Payload::Dependencies {
                report: self.dependencies_report(&scope).await?,
            }),
            Request::DependenciesVex { scope } => Ok(Payload::Text {
                text: self.dependencies_vex(&scope).await?,
            }),
            Request::Infrastructure => Ok(self.infrastructure().await),
            Request::Backup => Ok(Payload::Backup {
                report: Box::new(self.backup_report().await?),
            }),
            // `backup_completed`/`backup_failed`/`backup_verified` are
            // published inside, where the job's own backups publish them too.
            Request::BackupRun => Ok(Payload::BackupRun {
                snapshot: self
                    .backup_run(
                        factory_core::backup::BackupTrigger::Manual,
                        crate::policies::caller_name(caller),
                    )
                    .await?,
            }),
            Request::BackupVerify { snapshot } => Ok(Payload::BackupVerify {
                verification: self
                    .backup_verify(snapshot, crate::policies::caller_name(caller))
                    .await?,
            }),
            Request::Knowledge => {
                let root = self.factory_snapshot().root;
                let index = tokio::task::spawn_blocking(move || factory_core::knowledge::index(&root))
                    .await
                    .map_err(|e| FactoryError::Other(anyhow::anyhow!("knowledge walk: {e}")))?;
                Ok(Payload::Knowledge {
                    root: index.root,
                    present: index.present,
                    legacy: index.legacy,
                    pages: index.pages,
                    tags: index.tags,
                    documents: index.documents,
                    gaps: index.gaps,
                    findings: index.findings,
                })
            }
            Request::KnowledgeSearch { text, tags, scope, limit } => {
                let (root, provider_name) = {
                    let factory = self.factory_snapshot();
                    (factory.root, factory.config.daemon.knowledge_provider)
                };
                let provider = self.registry.knowledge(&provider_name)?;
                let query = factory_core::adapter::KnowledgeQuery {
                    text,
                    tags,
                    scope,
                    limit: limit
                        .unwrap_or(factory_core::adapter::knowledge::DEFAULT_SEARCH_LIMIT)
                        .min(factory_core::adapter::knowledge::MAX_SEARCH_LIMIT),
                };
                if query.text.trim().is_empty() && query.tags.is_empty() {
                    return Err(FactoryError::BadRequest(
                        "nothing to search for: give some text, a tag, or both".into(),
                    ));
                }
                let hits = provider.search(&root, &query).await?;
                Ok(Payload::KnowledgeHits {
                    provider: provider_name,
                    vault: factory_core::knowledge::vault_root(&root).display().to_string(),
                    hits,
                })
            }
            Request::KnowledgeImport { source, into, overwrite } => {
                let root = self.factory_snapshot().root;
                let source = PathBuf::from(source);
                let result = tokio::task::spawn_blocking(move || {
                    factory_core::knowledge::import(&root, &source, into.as_deref(), overwrite)
                })
                .await
                .map_err(|e| FactoryError::Other(anyhow::anyhow!("knowledge import: {e}")))?
                .map_err(FactoryError::BadRequest)?;
                self.knowledge_changed(&result.copied).await;
                Ok(knowledge_write_payload(result))
            }
            Request::KnowledgeAdd { sources, into, overwrite } => {
                let root = self.factory_snapshot().root;
                let sources: Vec<PathBuf> = sources.into_iter().map(PathBuf::from).collect();
                let result = tokio::task::spawn_blocking(move || {
                    factory_core::knowledge::add(&root, &sources, into.as_deref(), overwrite)
                })
                .await
                .map_err(|e| FactoryError::Other(anyhow::anyhow!("knowledge add: {e}")))?;
                self.knowledge_changed(&result.copied).await;
                Ok(knowledge_write_payload(result))
            }
            Request::KnowledgeWriteFile { path, overwrite, bytes } => {
                let root = self.factory_snapshot().root;
                let written = tokio::task::spawn_blocking(move || {
                    factory_core::knowledge::write_bytes(&root, &path, overwrite, &bytes)
                })
                .await
                .map_err(|e| FactoryError::Other(anyhow::anyhow!("knowledge write: {e}")))?
                .map_err(FactoryError::BadRequest)?;
                self.knowledge_changed(std::slice::from_ref(&written)).await;
                Ok(knowledge_write_payload(factory_core::knowledge::WriteResult {
                    copied: vec![written],
                    ..Default::default()
                }))
            }
            Request::Benchmarks => {
                let factory = self.factory_snapshot();
                let configurations =
                    factory_core::benchmark::configurations(&factory.config.scopes, &factory.config.daemon.foreman);
                Ok(Payload::Benchmarks { configurations })
            }
            Request::Datasets => Ok(Payload::Datasets {
                root: self.factory_snapshot().datasets_dir().display().to_string(),
                datasets: self.dataset_summaries()?,
            }),
            Request::Dataset { name } => {
                let (dataset, findings) = self.dataset_view(&name)?;
                Ok(Payload::Dataset { dataset, findings })
            }
            Request::DatasetCreate { name, description } => {
                let dataset = self.dataset_create(&name, description).await?;
                let findings = factory_core::dataset::findings(&dataset, &self.known_scope_names());
                Ok(Payload::Dataset { dataset, findings })
            }
            Request::DatasetAddCases { name, cases } => {
                let dataset = self.dataset_add_cases(&name, cases).await?;
                let findings = factory_core::dataset::findings(&dataset, &self.known_scope_names());
                Ok(Payload::Dataset { dataset, findings })
            }
            Request::DatasetImport { name, format, content, replace } => {
                let dataset = self.dataset_import(&name, &format, &content, replace).await?;
                let findings = factory_core::dataset::findings(&dataset, &self.known_scope_names());
                Ok(Payload::Dataset { dataset, findings })
            }
            Request::DatasetFromTasks { name, task_ids } => {
                let dataset = self.dataset_from_tasks(&name, task_ids).await?;
                let findings = factory_core::dataset::findings(&dataset, &self.known_scope_names());
                Ok(Payload::Dataset { dataset, findings })
            }
            Request::DatasetDeleteCase { name, id } => {
                let dataset = self.dataset_delete_case(&name, &id).await?;
                let findings = factory_core::dataset::findings(&dataset, &self.known_scope_names());
                Ok(Payload::Dataset { dataset, findings })
            }
            Request::DatasetDelete { name } => Ok(Payload::Deleted {
                deleted: self.dataset_delete(&name).await?,
            }),
            Request::BenchRunStart {
                dataset,
                agents,
                attempts,
                concurrency,
                cases,
            } => {
                // Only the trailing name matters: the agent that actually
                // resolves in each case's own scope, not the scope a person
                // happened to find it under in the Configurations roster.
                let agents: Vec<String> = agents
                    .iter()
                    .map(|a| a.rsplit('/').next().unwrap_or(a).to_string())
                    .collect();
                let run = self
                    .start_bench_run(&dataset, agents, attempts.unwrap_or(1), concurrency.unwrap_or(1), cases)
                    .await?;
                let results = factory_core::bench::aggregate(&run.attempts);
                Ok(Payload::BenchRun { run, results })
            }
            Request::BenchRuns { dataset } => Ok(Payload::BenchRuns {
                runs: self.bench.runs(dataset.as_deref(), 200).await?,
            }),
            Request::BenchRunGet { id } => {
                let run = self.bench.get_run(&id).await?.ok_or_else(|| {
                    FactoryError::BadRequest(format!("no such bench run: {id:?}"))
                })?;
                let results = factory_core::bench::aggregate(&run.attempts);
                Ok(Payload::BenchRun { run, results })
            }
            Request::BenchRunCancel { id } => {
                let run = self.cancel_bench_run(&id).await?;
                let results = factory_core::bench::aggregate(&run.attempts);
                Ok(Payload::BenchRun { run, results })
            }
            Request::BenchRunClean { id } => {
                let run = self.clean_bench_run(&id).await?;
                let results = factory_core::bench::aggregate(&run.attempts);
                Ok(Payload::BenchRun { run, results })
            }
            Request::Policy { scope } => Ok(Payload::Policy {
                report: self.policy_report(scope.as_deref()).await?,
            }),
            Request::PolicyControl { control, scope } => Ok(Payload::PolicyControl {
                detail: self.policy_control(control, &scope).await?,
            }),
            Request::PolicyAttest {
                control,
                scope,
                evidence,
                note,
                expires_at,
            } => {
                let attestation = self
                    .policy_attest(caller, control, scope, evidence, note, expires_at)
                    .await?;
                self.bus.publish(Event::PolicyChanged {
                    scope: attestation.scope.clone(),
                    control: attestation.control.clone(),
                });
                Ok(Payload::PolicyAttestation { attestation })
            }
            Request::PolicyWithdraw { id, reason } => {
                let attestation = self.policy_withdraw(caller, id, reason).await?;
                self.bus.publish(Event::PolicyChanged {
                    scope: attestation.scope.clone(),
                    control: attestation.control.clone(),
                });
                Ok(Payload::PolicyAttestation { attestation })
            }
            // An ordinary task in every way but how it was asked for, so it
            // answers the same way `Request::TaskCreate` itself does --
            // `Event::TaskCreated` already fired inside `policy_remediate`
            // (`Engine::create`), not published a second time here.
            Request::PolicyRemediate { control, scope, agent } => Ok(Payload::Task {
                task: self.policy_remediate(control, scope, agent).await?,
            }),
            Request::PolicyExport { scope, format } => {
                let (filename, body) = self.policy_export_render(scope.as_deref(), &format).await?;
                Ok(Payload::PolicyExport { format, filename, body })
            }
            Request::Metrics { ids } => {
                let now = Utc::now();
                let ids = if ids.is_empty() { self.default_metric_ids().await } else { ids };
                let metrics = self.metrics(&ids, now).await?;
                Ok(Payload::Metrics {
                    values: metrics.values,
                    series: metrics.series,
                    registry: metrics.registry,
                })
            }
            Request::Goals { scope, cycle } => Ok(Payload::Goals {
                report: self.goals_report(scope.as_deref(), cycle.as_deref()).await?,
            }),
            Request::GoalsCheckIn { kr, value, confidence, note } => {
                let checkin = self.goals_checkin(caller, kr, value, confidence, note).await?;
                self.bus.publish(Event::GoalsChanged { kr: checkin.kr.clone() });
                Ok(Payload::GoalsCheckIn { checkin })
            }
            Request::Scenarios { scope } => Ok(Payload::Scenarios {
                report: self.scenarios_report(scope.as_deref()).await?,
            }),
            // No event of its own: every fact it reads changes through
            // `TaskUpdated`, `RunUpdated`, `TaskEntry` or `AgentUpdated`
            // already, and a viewer re-reads on those.
            Request::Operations { scope, window, detail } => Ok(Payload::Operations {
                report: Box::new(self.operations_report(scope.as_deref(), window, detail).await?),
            }),
            // No event: promote creates ordinary tasks through `Engine::create`,
            // which already publishes `Event::TaskCreated` for each one --
            // the same "not published a second time here" rule
            // `PolicyRemediate` follows just above.
            Request::ScenarioPromote { scenario, scope, agent } => Ok(Payload::ScenarioPromote {
                result: self.scenario_promote(scenario, scope, agent).await?,
            }),
            Request::ScenarioWhatIf { scenario, drivers } => Ok(Payload::ScenarioWhatIf {
                result: self.scenario_whatif(scenario, drivers).await?,
            }),
            // `Event::QualityChanged`, when due, is published inside
            // `quality_report` itself -- it is a read that notices, not a write.
            Request::Quality { scope } => Ok(Payload::Quality {
                report: self.quality_report(scope.as_deref()).await?,
            }),
            // No event of its own: a created task already fired
            // `Event::TaskCreated` inside `Engine::create`, and answering an
            // already-open one changes nothing -- `PolicyRemediate`'s rule.
            Request::QualityRemediate { scope, attribute, scenario, agent } => Ok(Payload::QualityRemediate {
                result: self.quality_remediate(scope, attribute, scenario, agent).await?,
            }),
            Request::AgentStart { scope, name } => Ok(Payload::Agent {
                agent: self.start_agent(&scope, &name).await?.redacted(),
            }),
            Request::AgentConfigure { scope, agent } => {
                let (scope, agent) = self.configure_agent(&scope, agent)?;
                let name = agent.name();
                let autostart = agent.lifetime.is_standing() && agent.autostart();
                self.bus.publish(Event::AgentConfigured {
                    scope: scope.clone(),
                    name: name.clone(),
                });
                if autostart {
                    let engine = self.clone();
                    tokio::spawn(async move {
                        if let Err(error) = engine.start_agent(&scope, &name).await {
                            tracing::warn!(scope, name, "could not start newly configured agent: {error}");
                        }
                    });
                }
                Ok(Payload::Ok)
            }
            Request::AgentDelete { scope, name } => {
                let (scope, removed) = self.delete_agent_declaration(&scope, &name)?;
                let name = removed.name();
                self.bus.publish(Event::AgentDeleted { scope, name });
                self.reconcile_agents().await;
                Ok(Payload::Deleted { deleted: true })
            }
            Request::AgentStop { id } => Ok(Payload::Agent {
                agent: self.stop_agent(&id).await?.redacted(),
            }),
            Request::AgentRole { id, role } => Ok(Payload::Agent {
                agent: self
                    .set_agent_role(&id, role.map(Role::new))
                    .await?
                    .redacted(),
            }),
            Request::AgentInput { id, text, keys } => {
                self.agent_input(&id, text.as_deref(), &keys).await?;
                Ok(Payload::Ok)
            }
            Request::AgentOutput { id, lines } => Ok(Payload::Text {
                text: self.agent_output(&id, lines.unwrap_or(200)).await?,
            }),
            Request::AgentScreen { id } => match self.agent_screen(&id).await? {
                Some(screen) => Ok(Payload::Screen { screen }),
                None => Err(FactoryError::BadRequest(
                    "this runtime cannot render a screen for that session".into(),
                )),
            },
            Request::RunScreen { id } => {
                let run = self.require_run(&id).await?;
                match self.run_screen(&run).await? {
                    Some(screen) => Ok(Payload::Screen { screen }),
                    None => Err(FactoryError::BadRequest(
                        "this run has no session to show".into(),
                    )),
                }
            }
            Request::RunInput { id, text, keys } => {
                self.run_input(&id, text.as_deref(), &keys).await?;
                Ok(Payload::Ok)
            }
            Request::RunAnswer { id, text, reason } => {
                let asked = crate::operations::Asked::new(caller, Some(reason));
                self.answer_run(&id, &text, &asked).await?;
                Ok(Payload::Ok)
            }

            Request::TaskCreate(new) => Ok(Payload::Task {
                task: self.create(new).await?,
            }),
            Request::TaskGet { id } => Ok(Payload::Task {
                task: self.require(&id).await?,
            }),
            Request::TaskList(filter) => Ok(Payload::Tasks {
                tasks: self.store.list(&filter).await?,
            }),
            Request::TaskUpdate { id, patch, reason } => {
                let asked = crate::operations::Asked::new(caller, reason);
                Ok(Payload::Task {
                    task: self.update(&id, patch, Some(&asked)).await?,
                })
            }
            Request::TaskSkipNext { id, reason, slot } => {
                let asked = crate::operations::Asked::new(caller, reason);
                Ok(Payload::Task {
                    task: self.skip_next(&id, slot, &asked).await?,
                })
            }
            Request::TaskDelete { id } => {
                if let Some(run) = self.store.active_run(&id).await? {
                    self.close_session(&run).await;
                }
                let deleted = self.store.delete(&id).await?;
                if deleted {
                    self.bus.publish(Event::TaskDeleted { id });
                }
                Ok(Payload::Deleted { deleted })
            }
            Request::TaskRun { id, reason } => {
                let task = self.require(&id).await?;
                // The gate (`#119`): an item still in intake has not been
                // released, and nothing but a decision releases it.
                if task.status == TaskStatus::Intake {
                    return Err(FactoryError::BadRequest(
                        "this task is still in intake: triage it and release it (factory intake decide) before it can run".into(),
                    ));
                }
                // A task is a standing intent; a run is one attempt at it. Two
                // attempts at once would race for the same working directory.
                if let Some(run) = self.store.active_run(&id).await? {
                    return Err(FactoryError::BadRequest(format!(
                        "attempt {} of this task is still {}; cancel it before starting another",
                        run.attempt,
                        run.status.as_str()
                    )));
                }
                // Who asked, written down before the run exists: the record
                // otherwise says only that a run was `manual`, which a
                // person and an agent both are (`#106`).
                // Stamped here, not inside the spawned dispatch: the queue
                // wait of a manual run starts when it was asked for.
                let due = Due::now();
                // `queued_at` is what ties this entry to the run it starts,
                // which does not exist yet: the Operations report leaves a
                // run an agent asked for out of the interventions by it.
                let asked = crate::operations::Asked::new(caller, reason);
                self.entry(
                    &id,
                    asked.entry(
                        crate::operations::RUN_REQUESTED_KIND,
                        format!("run requested {}", asked.words()),
                        serde_json::json!({ "queued_at": due.queued_at }),
                    ),
                )
                .await;
                let engine = self.clone();
                // Dispatch can take a minute: opening a pane, waiting for an
                // agent to be ready. The caller gets its answer now.
                tokio::spawn(async move {
                    engine.start_run_due(&id, Trigger::Manual, due).await;
                });
                Ok(Payload::Ok)
            }
            Request::TaskCancel { id, reason, run } => {
                // Who asked is the one thing that tells a person's
                // intervention from an agent tidying up -- see `FailKind`
                // for the limit of that.
                let kind = match caller {
                    crate::access::Caller::Owner => FailKind::CancelledByPerson,
                    crate::access::Caller::Agent { .. } => FailKind::CancelledByAgent,
                };
                let run = self.cancel_task_run(&id, run.as_deref(), kind).await?;
                let asked = crate::operations::Asked::new(caller, reason);
                self.entry(
                    &id,
                    asked
                        .entry("cancel_requested", format!("cancelled {}", asked.words()), serde_json::json!({}))
                        .in_run(&run.id),
                )
                .await;
                self.sync_workflow_for_task(&id).await;
                self.sync_bench_for_task(&id).await;
                Ok(Payload::Run { run: run.redacted() })
            }
            Request::TaskClose { id, reason, duplicate_of, note } => {
                let asked = crate::operations::Asked::new(caller, note);
                let task = self.close_task(&id, reason, duplicate_of, &asked).await?;
                self.sync_workflow_for_task(&id).await;
                self.sync_bench_for_task(&id).await;
                Ok(Payload::Task { task })
            }
            Request::TaskReopen { id, reason } => {
                let asked = crate::operations::Asked::new(caller, reason);
                let task = self.reopen_task(&id, &asked).await?;
                self.sync_workflow_for_task(&id).await;
                self.sync_bench_for_task(&id).await;
                Ok(Payload::Task { task })
            }
            Request::TaskReport { id, report } => {
                let run = self.report(&id, report).await?;
                self.sync_workflow_for_task(&id).await;
                self.sync_bench_for_task(&id).await;
                Ok(Payload::Run { run: run.redacted() })
            }
            Request::TaskAttach { id, token, kind, filename: _, bytes } => {
                let attachment = self.attach_dependency(&id, kind, bytes, Some(&token)).await?;
                Ok(Payload::Attachment { attachment })
            }
            Request::TaskTurnEnded { id, turn } => {
                self.turn_ended(&id, turn).await?;
                self.sync_workflow_for_task(&id).await;
                self.sync_bench_for_task(&id).await;
                Ok(Payload::Ok)
            }
            Request::TaskEntries { id, limit, task_only } => Ok(Payload::Entries {
                entries: if task_only {
                    self.store.task_own_entries(&id, limit.unwrap_or(200)).await?
                } else {
                    self.store.entries(&id, limit.unwrap_or(200)).await?
                },
            }),
            Request::TaskOutput { id, lines } => {
                let latest = self.store.runs(&id, 1).await?.into_iter().next();
                Ok(Payload::Text {
                    text: match latest {
                        Some(run) => self.output(&run, lines.unwrap_or(200)).await,
                        None => String::new(),
                    },
                })
            }

            // Intake (`#119`). Every write publishes `TaskCreated` or
            // `TaskUpdated` itself; the board is a read over tasks.
            Request::IntakeAdd(new) => Ok(Payload::Task { task: self.intake_add(caller, new).await? }),
            Request::IntakeBoard { scope } => Ok(Payload::IntakeBoard {
                board: self.intake_board(scope.as_deref()).await?,
            }),
            Request::IntakeTriage { id, agent } => Ok(Payload::Task {
                task: self.intake_triage(caller, &id, agent).await?,
            }),
            Request::IntakeAssess { id, assessment, decide } => Ok(Payload::Task {
                task: self.intake_assess(caller, &id, assessment, decide).await?,
            }),
            Request::IntakeDecide { id, decision } => Ok(Payload::Task {
                task: self.intake_decide(caller, &id, decision).await?,
            }),
            Request::IntakeInfo { id, text } => Ok(Payload::Task {
                task: self.intake_info(caller, &id, &text).await?,
            }),

            Request::WorkflowCreate(draft) => Ok(Payload::Workflow {
                workflow: self.create_workflow(draft).await?,
            }),
            Request::WorkflowGet { id } => Ok(Payload::Workflow {
                workflow: self.workflow_definition(&id).await?,
            }),
            Request::WorkflowList { scope } => Ok(Payload::Workflows {
                workflows: self.workflows.definitions(scope.as_deref()).await?,
            }),
            Request::WorkflowUpdate { id, workflow } => Ok(Payload::Workflow {
                workflow: self.update_workflow(&id, workflow).await?,
            }),
            Request::WorkflowDelete { id } => Ok(Payload::Deleted {
                deleted: self.delete_workflow(&id).await?,
            }),
            Request::WorkflowStart { id, inputs } => Ok(Payload::WorkflowRun {
                run: self.start_workflow(&id, inputs, caller).await?,
            }),
            Request::WorkflowRunGet { id } => Ok(Payload::WorkflowRun {
                run: self.workflow_run(&id).await?,
            }),
            Request::WorkflowRunList { workflow_id, scope, limit } => Ok(Payload::WorkflowRuns {
                runs: self.workflows.runs(workflow_id.as_deref(), scope.as_deref(), limit.unwrap_or(50)).await?,
            }),
            Request::WorkflowRunCancel { id } => Ok(Payload::WorkflowRun {
                run: self.cancel_workflow(&id).await?,
            }),
            Request::WorkflowLint { workflow, task, scope, category } => Ok(Payload::WorkflowLint {
                lint: self.workflow_lint(workflow, task, scope, category).await?,
            }),
            Request::RunAttestations { id } => Ok(Payload::Attestations {
                attestations: self.run_attestations(&id).await?,
            }),
            Request::RunApprove { id, reason } => Ok(Payload::Run {
                run: Box::pin(self.decide_approval(
                        caller,
                        &id,
                        factory_core::control_plan::AttestationVerdict::Pass,
                        &reason,
                    ))
                    .await?
                    .redacted(),
            }),
            Request::RunReject { id, reason } => Ok(Payload::Run {
                run: Box::pin(self.decide_approval(
                        caller,
                        &id,
                        factory_core::control_plan::AttestationVerdict::Fail,
                        &reason,
                    ))
                    .await?
                    .redacted(),
            }),
            Request::RunRework { id } => Ok(Payload::Run {
                run: Box::pin(self.accept_rework(&id)).await?.redacted(),
            }),

            Request::RunList { task_id, limit } => Ok(Payload::Runs {
                runs: self
                    .store
                    .runs(&task_id, limit.unwrap_or(50))
                    .await?
                    .into_iter()
                    .map(|r| r.redacted())
                    .collect(),
            }),
            Request::RunGet { id } => Ok(Payload::Run {
                run: self.require_run(&id).await?.redacted(),
            }),
            Request::RunUsage { id } => {
                let run = self.require_run(&id).await?;
                Ok(Payload::UsageSnapshots {
                    snapshots: self.store.usage_snapshots(&run.id).await?,
                })
            }
            Request::TaskUsage { id } => Ok(Payload::TaskUsage {
                usage: self.task_usage(&id).await?,
            }),
            Request::Costs { group_by, from, to, scope } => Ok(Payload::Costs {
                report: self.costs_report(group_by, from, to, scope.as_deref()).await?,
            }),
            Request::RunEntries { id, limit } => Ok(Payload::Entries {
                entries: self.store.run_entries(&id, limit.unwrap_or(200)).await?,
            }),
            Request::RunOutput { id, lines } => {
                let run = self.require_run(&id).await?;
                Ok(Payload::Text {
                    text: self.output(&run, lines.unwrap_or(200)).await,
                })
            }

            Request::Subscribe => Err(FactoryError::BadRequest(
                "this interface does not stream events on the request channel".into(),
            )),
        }
    }

    async fn status(&self) -> Result<StatusInfo> {
        let factory = self.factory_snapshot();
        let tasks = self.store.list(&TaskFilter::default()).await?;
        Ok(StatusInfo {
            instance: factory.config.instance.name.clone(),
            instance_id: factory.config.instance.id.clone(),
            root: factory.root.display().to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_seconds: self.started.elapsed().as_secs(),
            tasks_total: tasks.len(),
            tasks_active: self.store.active_runs().await?.len(),
            subscribers: self.bus.subscriber_count(),
            interfaces: self.interfaces.clone(),
            scopes: factory.scope_names(),
        })
    }

    /// One probe per effective runtime connection, not one per scope. A probe
    /// that fails becomes that connection's error card; it never prevents a
    /// different runtime from reporting its own state.
    async fn runtime_connections(&self) -> Vec<RuntimeConnectionView> {
        let factory = self.factory_snapshot();
        let mut scopes_by_runtime: std::collections::BTreeMap<String, Vec<String>> =
            Default::default();
        for scope in &factory.config.scopes {
            let runtime = scope
                .runtime
                .clone()
                .unwrap_or_else(|| factory.config.daemon.default_runtime.clone());
            scopes_by_runtime
                .entry(runtime)
                .or_default()
                .push(scope.name.clone());
        }

        let metadata: std::collections::BTreeMap<String, (String, String)> = self
            .registry
            .list()
            .adapters
            .into_iter()
            .filter(|adapter| adapter.kind == "runtime")
            .map(|adapter| (adapter.name, (adapter.source, adapter.description)))
            .collect();

        let mut views = Vec::with_capacity(scopes_by_runtime.len());
        for (runtime_name, scopes) in scopes_by_runtime {
            let checked_at = Utc::now();
            let diagnostic = match self.registry.runtime(&runtime_name) {
                Ok(runtime) => runtime
                    .connection_diagnostic()
                    .await
                    .unwrap_or_else(|error| RuntimeConnectionDiagnostic::error(error.to_string())),
                Err(error) => RuntimeConnectionDiagnostic::error(error.to_string()),
            };
            let (source, description) = metadata.get(&runtime_name).cloned().unwrap_or_else(|| {
                (
                    "missing".into(),
                    "this configured runtime adapter is not registered".into(),
                )
            });
            views.push(RuntimeConnectionView {
                runtime: runtime_name,
                source,
                description,
                scopes,
                checked_at,
                diagnostic,
            });
        }
        views
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
                    worktree_capable: sv.worktree_capable,
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
        let interfaces = daemon_config
            .interfaces
            .iter()
            .map(|interface| InterfaceFacts {
                kind: interface.kind.clone(),
                bind: match interface.kind.as_str() {
                    // A socket, shown on its own line, not an address.
                    "cli" => None,
                    "http" => Some(
                        interface
                            .string("bind")
                            .unwrap_or_else(|| crate::interfaces::http::DEFAULT_BIND.to_string()),
                    ),
                    _ => interface.string("bind"),
                },
            })
            .collect();
        // The herdr session is the daemon's own operational setting, which
        // its service definition sets and every herdr call it makes
        // inherits -- not a credential, and not a provider's `env:`, which is
        // never read.
        let herdr_session = (daemon_config.default_runtime == "herdr")
            .then(|| std::env::var("HERDR_SESSION").ok())
            .flatten()
            .filter(|s| !s.is_empty());
        let started_at = Utc::now()
            - chrono::Duration::from_std(self.started.elapsed()).unwrap_or_default();
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

        // One row per harness any scope's agents run on, whether or not it
        // has been probed yet -- plus anything probed since that the config
        // no longer names.
        let mut known: Vec<(String, factory_core::harness::HealthProbe)> = Vec::new();
        for scope in &factory.config.scopes {
            for agent in scope.agents_with(&daemon_config.foreman) {
                let Some(probe) = self.registry.agent(&agent.harness).ok().and_then(|a| a.health_probe()) else {
                    continue;
                };
                let harness = crate::harness_health::harness_name(&probe);
                if !known.iter().any(|(_, p)| *p == probe) {
                    known.push((harness, probe));
                }
            }
        }
        let harnesses = self
            .harness
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
        let mut rows = Vec::new();

        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            let fixed = [
                (
                    "Claude Code credentials",
                    home.join(".claude/.credentials.json"),
                    "anthropic",
                ),
                (
                    "GitHub CLI hosts",
                    home.join(".config/gh/hosts.yml"),
                    "github",
                ),
                ("AWS credentials", home.join(".aws/credentials"), "aws"),
                ("netrc", home.join(".netrc"), "netrc"),
            ];
            for (label, path, integration) in fixed {
                let present = tokio::fs::try_exists(&path).await.unwrap_or(false);
                rows.push(CredentialRow {
                    label: label.into(),
                    path: path.display().to_string(),
                    integration: integration.into(),
                    present,
                    scope: None,
                });
            }

            // Presence only: an id_* file that is not a `.pub` is treated as
            // a private key without ever being opened to check.
            let ssh_dir = home.join(".ssh");
            let mut ssh_present = false;
            if let Ok(mut entries) = tokio::fs::read_dir(&ssh_dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if name.starts_with("id_") && !name.ends_with(".pub") {
                        ssh_present = true;
                        break;
                    }
                }
            }
            rows.push(CredentialRow {
                label: "SSH private keys".into(),
                path: ssh_dir.join("id_*").display().to_string(),
                integration: "ssh".into(),
                present: ssh_present,
                scope: None,
            });
        }

        let factory = self.factory_snapshot();
        for scope in &factory.config.scopes {
            let scope_dir = factory
                .scope_path(&scope.name)
                .unwrap_or_else(|_| scope.path.clone());
            // A scope registered on the instance root has the path `<root>/.`,
            // so joining onto it raw would print `<root>/./.env` on the page.
            // Collecting the components drops the `.` without touching what
            // the path means.
            let env_path = scope_dir.components().collect::<PathBuf>().join(".env");
            let present = tokio::fs::try_exists(&env_path).await.unwrap_or(false);
            rows.push(CredentialRow {
                label: format!("{} .env", scope.name),
                path: env_path.display().to_string(),
                integration: "scope env".into(),
                present,
                scope: Some(scope.name.clone()),
            });
        }

        rows
    }

    /// The agents page: scopes first, then the agents each one declares, then
    /// what they are doing -- and, once, every adapter registered, which
    /// belongs to the whole answer rather than to any one scope in it. One
    /// call, because a page that had to join config, adapters, standing
    /// agents, runs and tasks itself would be showing five different moments
    /// in time.
    /// One scope's worktree capability, asked of `git` at most every
    /// `CAPABILITY_TTL`. See `worktree_caps` for why this is cached at all.
    async fn worktree_capability(&self, name: &str, dir: &Path) -> (bool, Option<String>) {
        if let Some((at, answer)) = self.worktree_caps.lock().unwrap().get(name) {
            if at.elapsed() < CAPABILITY_TTL {
                return answer.clone();
            }
        }
        let answer = worktree::capability(dir).await;
        self.worktree_caps
            .lock()
            .unwrap()
            .insert(name.to_string(), (Instant::now(), answer.clone()));
        answer
    }

    pub(crate) async fn scope_views(&self) -> Result<(Vec<ScopeView>, Vec<String>)> {
        let factory = self.factory_snapshot();
        let adapters = self.registry.list();
        let described: std::collections::BTreeMap<String, (String, String)> = adapters
            .adapters
            .iter()
            .filter(|a| a.kind == "agent")
            .map(|a| (a.name.clone(), (a.description.clone(), a.source.clone())))
            .collect();
        let available: Vec<String> = described.keys().cloned().collect();
        let available_stores: Vec<String> = adapters
            .adapters
            .iter()
            .filter(|a| a.kind == "task")
            .map(|a| a.name.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();

        let standing = self.store.agents().await?;
        let active = self.store.active_runs().await?;

        // Runs, grouped by the scope and adapter that are actually doing them.
        let mut work: std::collections::BTreeMap<(String, String), Vec<AgentActivity>> =
            Default::default();
        for run in &active {
            let task = self.store.get(&run.task_id).await?;
            // Canonicalized: a task written before a scope's identity became
            // its path still carries the bare name it was given, and this is
            // what lets its active runs land on the same row as everything
            // else in that scope rather than opening an orphan one next to
            // it.
            let scope = task
                .as_ref()
                .map(|t| factory.canonical_scope_name(&t.scope))
                .unwrap_or_default();
            work.entry((scope.clone(), run.agent.clone()))
                .or_default()
                .push(AgentActivity {
                    run_id: run.id.clone(),
                    task_id: run.task_id.clone(),
                    task_title: task
                        .map(|t| t.title)
                        .unwrap_or_else(|| "(deleted task)".into()),
                    scope,
                    attempt: run.attempt,
                    status: run.status.as_str().to_string(),
                    runtime: run.runtime.clone(),
                    trigger: run.trigger.as_str().to_string(),
                    started_at: run.started_at,
                    session: run.session.as_ref().map(|s| s.handle.clone()),
                });
        }

        let instance_default = factory.config.daemon.default_agent.clone();
        let mut views = Vec::new();

        for scope in &factory.config.scopes {
            let default_agent = scope
                .agent_adapter()
                .unwrap_or(&instance_default)
                .to_string();
            let runtime = scope
                .runtime
                .clone()
                .unwrap_or_else(|| factory.config.daemon.default_runtime.clone());
            let task_store = factory.task_store_for(&scope.name).to_string();
            let scope_dir = factory
                .scope_path(&scope.name)
                .unwrap_or_else(|_| scope.path.clone());
            let (worktree_capable, worktree_reason) = self.worktree_capability(&scope.name, &scope_dir).await;

            let mut agents = Vec::new();
            let mut covered = std::collections::BTreeSet::new();
            let deletable: std::collections::BTreeSet<String> = scope
                .declared_agents()
                .into_iter()
                .map(|agent| agent.name())
                .collect();

            for decl in scope.agents_with(&factory.config.daemon.foreman) {
                let name = decl.name();
                // Runs are keyed by the name a task asked for, which is this
                // name -- not the harness behind it. Key both sides the same
                // way or live runs quietly stop appearing here.
                covered.insert(name.clone());
                let live = decl
                    .lifetime
                    .is_standing()
                    .then(|| {
                        standing
                            .iter()
                            .find(|a| a.id == AgentSession::id_for(&scope.name, &name))
                    })
                    .flatten();
                let (description, source) = described
                    .get(&decl.harness)
                    .cloned()
                    .unwrap_or_else(|| ("not registered".into(), "missing".into()));

                agents.push(AgentView {
                    id: live.map(|a| a.id.clone()),
                    name: name.clone(),
                    adapter: decl.harness.clone(),
                    description,
                    source,
                    lifetime: decl.lifetime.as_str().to_string(),
                    role: live
                        .map(|a| a.role_with(&decl.role))
                        .unwrap_or_else(|| decl.role.clone())
                        .as_str()
                        .to_string(),
                    sandbox: decl.sandbox.as_str().to_string(),
                    assigned_role: live
                        .and_then(|a| a.assigned_role.clone())
                        .map(|r| r.as_str().to_string()),
                    autostart: decl.autostart(),
                    state: live
                        .map(|a| a.state.as_str().to_string())
                        .unwrap_or_else(|| {
                            if decl.lifetime.is_standing() {
                                AgentState::Stopped.as_str().to_string()
                            } else {
                                "task".into()
                            }
                        }),
                    // "default" means a task that names no agent lands here --
                    // which is a question about the name, not the harness.
                    // Three agents sharing a harness are not all the default.
                    is_default: name == default_agent,
                    declared: true,
                    deletable: deletable.contains(&name),
                    attach: live.and_then(|a| a.attach.clone()),
                    session: live
                        .and_then(|a| a.session.as_ref())
                        .map(|s| s.handle.clone()),
                    started_at: live.map(|a| a.started_at),
                    error: live.and_then(|a| a.error.clone()),
                    active: work
                        .get(&(scope.name.clone(), name.clone()))
                        .cloned()
                        .unwrap_or_default(),
                });
            }

            // The scope's default, when nothing above already named it.
            if !covered.contains(&default_agent) {
                let (description, source) = described
                    .get(&default_agent)
                    .cloned()
                    .unwrap_or_else(|| ("not registered".into(), "missing".into()));
                covered.insert(default_agent.clone());
                agents.insert(
                    0,
                    AgentView {
                        id: None,
                        name: default_agent.clone(),
                        adapter: default_agent.clone(),
                        description,
                        source,
                        lifetime: "task".into(),
                        role: Role::default().as_str().to_string(),
                        // Nothing declared this agent, so there is no
                        // `sandbox:` to read -- today's default, unstated.
                        sandbox: Sandbox::None.as_str().to_string(),
                        assigned_role: None,
                        autostart: false,
                        state: "task".into(),
                        is_default: true,
                        declared: false,
                        deletable: false,
                        attach: None,
                        session: None,
                        started_at: None,
                        error: None,
                        active: work
                            .get(&(scope.name.clone(), default_agent.clone()))
                            .cloned()
                            .unwrap_or_default(),
                    },
                );
            }

            // An agent working here that the scope never declared -- somebody
            // started a task with `--agent`. It is doing work, so it belongs on
            // the page whatever the config says.
            for ((s, adapter), jobs) in &work {
                if s != &scope.name || covered.contains(adapter) {
                    continue;
                }
                let (description, source) = described
                    .get(adapter)
                    .cloned()
                    .unwrap_or_else(|| ("not registered".into(), "missing".into()));
                agents.push(AgentView {
                    id: None,
                    name: adapter.clone(),
                    adapter: adapter.clone(),
                    description,
                    source,
                    lifetime: "task".into(),
                    role: Role::default().as_str().to_string(),
                    sandbox: Sandbox::None.as_str().to_string(),
                    assigned_role: None,
                    autostart: false,
                    state: "task".into(),
                    is_default: false,
                    declared: false,
                    deletable: false,
                    attach: None,
                    session: None,
                    started_at: None,
                    error: None,
                    active: jobs.clone(),
                });
            }

            views.push(ScopeView {
                id: scope.id.clone(),
                name: scope.name.clone(),
                path: scope_dir.display().to_string(),
                default_agent,
                runtime,
                agents,
                // Nothing today gives one scope a different roster of
                // adapters than any other, so there is no per-scope override
                // to carry -- the shared list returned alongside `views` is
                // the whole answer.
                available: None,
                task_store,
                available_stores: available_stores.clone(),
                worktree_capable,
                worktree_reason,
            });
        }

        Ok((views, available))
    }

    // -- naming an agent ----------------------------------------------------

    /// Turn what a task asked for into the agent it will actually run as.
    ///
    /// A task names a concrete agent in its scope -- `assistant`, `scratch` --
    /// and the adapter behind it follows from the config. An adapter name
    /// still works for a scope that declares nothing, or for a one-off with
    /// `--agent claude-code`.
    pub fn resolve_agent(
        &self,
        scope_name: &str,
        name: &str,
    ) -> Result<(String, String, Option<ScopeAgent>)> {
        let factory = self.factory_snapshot();
        let scope = factory.scope(scope_name)?;
        let declared_here = scope.agents_with(&factory.config.daemon.foreman);
        if let Some(declared) = declared_here.iter().find(|a| a.name() == name).cloned() {
            // The name resolves; the adapter behind it still has to exist.
            self.registry.agent(&declared.harness)?;
            return Ok((
                declared.name(),
                declared.harness.clone(),
                Some(declared),
            ));
        }
        if self.registry.agent(name).is_ok() {
            return Ok((name.to_string(), name.to_string(), None));
        }

        let declared: Vec<String> = declared_here.iter().map(|a| a.name()).collect();
        let adapters: Vec<String> = self
            .registry
            .list()
            .adapters
            .into_iter()
            .filter(|a| a.kind == "agent")
            .map(|a| a.name)
            .collect();
        Err(FactoryError::BadRequest(format!(
            "scope {scope_name:?} has no agent named {name:?}. It declares: {}. \
             Any adapter also works: {}.",
            if declared.is_empty() { "none".into() } else { declared.join(", ") },
            adapters.join(", "),
        )))
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
        let factory = self.factory_snapshot();
        let current = self.require(id).await?;
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
        if let Some(category) = &patch.category {
            factory_core::control_plan::check_category(category).map_err(FactoryError::BadRequest)?;
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
            self.registry.runtime(rt)?;
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

        let task = self.store.update(id, &patch).await?;
        if let Some(paused) = pause_change {
            // Journal what the store kept, not what was asked: an
            // out-of-process store written before pausing existed takes the
            // patch and quietly drops the field, and a journal saying
            // "paused" over a schedule that still fires would be worse than
            // the error.
            if task.schedule_paused != paused {
                return Err(FactoryError::adapter(
                    self.store.name(),
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
        self.bus.publish(Event::TaskUpdated { task: task.clone() });
        Ok(task)
    }

    // -- creating ----------------------------------------------------------

    pub async fn create(&self, new: NewTask) -> Result<Task> {
        self.create_task(new, None, None, None, None).await
    }

    /// A task born inside the intake gate (`#119`): `TaskStatus::Intake`
    /// from its first write, so there is no moment it could be dispatched.
    pub(crate) async fn create_intake_task(
        &self,
        new: NewTask,
        intake: factory_core::intake::Intake,
    ) -> Result<Task> {
        if new.schedule.is_some() {
            return Err(FactoryError::BadRequest(
                "an intake item has no schedule; release it first, then schedule the task".into(),
            ));
        }
        self.create_task(new, None, None, None, Some(intake)).await
    }

    pub(crate) async fn create_workflow_task(
        &self,
        new: NewTask,
        origin: WorkflowOrigin,
        id: String,
    ) -> Result<Task> {
        self.create_task(new, Some(origin), None, Some(id), None).await
    }

    pub(crate) async fn create_bench_task(
        &self,
        new: NewTask,
        origin: factory_core::bench::BenchOrigin,
        id: String,
    ) -> Result<Task> {
        self.create_task(new, None, Some(origin), Some(id), None).await
    }

    async fn create_task(
        &self,
        new: NewTask,
        workflow_origin: Option<WorkflowOrigin>,
        bench_origin: Option<factory_core::bench::BenchOrigin>,
        id: Option<String>,
        intake: Option<factory_core::intake::Intake>,
    ) -> Result<Task> {
        let factory = self.factory_snapshot();
        if new.title.trim().is_empty() {
            return Err(FactoryError::BadRequest("a task needs a title".into()));
        }
        if new.estimate_seconds == Some(0) {
            return Err(FactoryError::BadRequest(
                "a task estimate must be at least one second".into(),
            ));
        }
        // A retry policy governs what happens after a *scheduled* run fails
        // (`Engine::settle_retry` never looks at it for a task with no
        // `schedule`) -- refused here, at creation, rather than accepted and
        // silently ignored until whoever set it notices nothing ever
        // retries.
        if new.retry.is_some() && new.schedule.is_none() {
            return Err(FactoryError::BadRequest(
                "a retry policy only means something for a scheduled task; add a schedule too, or drop retry".into(),
            ));
        }
        if let Some(category) = &new.category {
            factory_core::control_plan::check_category(category).map_err(FactoryError::BadRequest)?;
        }

        let scope = match new.scope.clone() {
            Some(s) => s,
            None => factory
                .config
                .scopes
                .first()
                .map(|s| s.name.clone())
                .ok_or_else(|| {
                    FactoryError::BadRequest("no scope given and the instance declares none".into())
                })?,
        };
        let declared = factory.scope(&scope)?.clone();

        let agent = new
            .agent
            .clone()
            .or_else(|| declared.agent_adapter().map(str::to_string))
            .unwrap_or_else(|| factory.config.daemon.default_agent.clone());
        let runtime = new
            .runtime
            .clone()
            .or_else(|| declared.runtime.clone())
            .unwrap_or_else(|| factory.config.daemon.default_runtime.clone());

        // Refuse now, with the list of what this scope offers, rather than at
        // dispatch time when whoever asked has stopped watching.
        let (agent, _adapter, _) = self.resolve_agent(&scope, &agent)?;
        self.registry.runtime(&runtime)?;

        // The scope's canonical identity, not necessarily what the caller
        // typed -- `declared` is already resolved through the bare-name
        // fallback above, and storing its own name keeps a freshly created
        // task from starting life needing that fallback itself.
        let mut task = factory_core::adapter::store::task_from_new(new, declared.name.clone(), agent, runtime);
        if let Some(id) = id { task.id = id; }
        task.workflow_origin = workflow_origin;
        task.bench_origin = bench_origin;
        if let Some(intake) = intake {
            task.status = TaskStatus::Intake;
            task.intake = Some(intake);
        }
        if let Some(s) = &task.schedule {
            task.next_run_at = Some(schedule::next_after(s, Utc::now())?);
        }

        let task = self.store.create(&task).await?;
        self.entry(
            &task.id,
            TaskEntry::new("daemon", "created", format!("created: {}", task.title)),
        )
        .await;
        self.bus.publish(Event::TaskCreated { task: task.clone() });
        Ok(task)
    }

    // -- running -----------------------------------------------------------

    /// Start one attempt at a task and hand it to an agent, due now -- what
    /// a workflow node, an agent's request and a bench attempt all mean by
    /// starting one: the moment the daemon asks is the moment it became
    /// eligible. See `start_run_due` for one that became due earlier.
    pub async fn start_run(self: &Arc<Self>, task_id: &str, trigger: Trigger) {
        self.start_run_due(task_id, trigger, Due::now()).await
    }

    /// Start one attempt at a task that became due at `due` -- a schedule's
    /// slot, a retry's backoff running out, a request that arrived before
    /// the dispatch got going. Failures here end the run rather than
    /// escaping, because nobody is waiting on the answer.
    pub async fn start_run_due(self: &Arc<Self>, task_id: &str, trigger: Trigger, due: Due) {
        // However it was asked for, an item still in intake is not started --
        // and not failed either, which is what a dispatch error below would
        // do to it (`#119`).
        if let Ok(Some(task)) = self.store.get(task_id).await {
            if task.status == TaskStatus::Intake {
                self.entry(
                    task_id,
                    TaskEntry::new("daemon", "intake_held", "not started: the task is still in intake"),
                )
                .await;
                return;
            }
        }
        let run = match self.dispatch(task_id, trigger, due).await {
            Ok(run) => run,
            // Blocked, not failed: `harness_gate` has already said why on
            // the task, and there is no run to close.
            Err(FactoryError::HarnessUnhealthy(reason)) => {
                tracing::warn!(task = task_id, "held on its harness: {reason}");
                self.record_workflow_task_state(task_id).await;
                self.record_bench_task_state(task_id).await;
                return;
            }
            Err(e) => {
                // The run may or may not exist yet; if it does, close it.
                if let Ok(Some(run)) = self.store.active_run(task_id).await {
                    self.fail_run(&run.id, FailKind::DispatchFailed, &format!("dispatch failed: {e}"))
                        .await;
                } else {
                    self.entry(
                        task_id,
                        TaskEntry::new("daemon", "failed", format!("dispatch failed: {e}")),
                    )
                    .await;
                    // Refused before any run existed, so there is no run
                    // to mirror: the task is blocked on the failure itself
                    // (`#122`), exactly as if a run had failed to dispatch.
                    let _ = self
                        .store
                        .update(
                            task_id,
                            &TaskPatch {
                                status: Some(TaskStatus::Blocked),
                                error: Some(e.to_string()),
                                clear_result: true,
                                failure: Some(TaskFailure {
                                    kind: Some(FailKind::DispatchFailed),
                                    run_id: None,
                                    attempt: None,
                                    at: Utc::now(),
                                }),
                                clear_closure: true,
                                clear_pending_retry: true,
                                ..Default::default()
                            },
                        )
                        .await;
                    self.publish_task(task_id).await;
                }
                self.record_workflow_task_state(task_id).await;
                self.record_bench_task_state(task_id).await;
                return;
            }
        };
        tracing::info!(task = task_id, run = %run.id, attempt = run.attempt, "dispatched");
        self.record_workflow_task_state(task_id).await;
        self.record_bench_task_state(task_id).await;
    }

    pub(crate) async fn dispatch(
        self: &Arc<Self>,
        task_id: &str,
        trigger: Trigger,
        due: Due,
    ) -> Result<Run> {
        let task = self.require(task_id).await?;
        // Resolve again rather than trusting what was written down: the config
        // may have changed since the task was created.
        let (agent_name, adapter_name, declaration) =
            self.resolve_agent(&task.scope, &task.agent)?;
        let agent = self.registry.agent(&adapter_name)?;
        let runtime = self.registry.runtime(&task.runtime)?;
        // Before anything else exists (`#131`): a harness that does not
        // start blocks the task here, with no run row and no session, rather
        // than dispatching into a pane nobody answers and failing it as an
        // `ack_timeout` three minutes later.
        self.harness_gate(&task, agent.as_ref(), trigger).await?;
        let factory = self.factory_snapshot();
        let scope_path = factory.scope_path(&task.scope)?;
        if !scope_path.is_dir() {
            return Err(FactoryError::BadRequest(format!(
                "scope {:?} points at {}, which is not a directory",
                task.scope,
                scope_path.display()
            )));
        }

        // What `done` will need, fixed now (`#118`) -- before the run row
        // exists, so a plan that cannot be resolved fails the dispatch
        // rather than letting the work start unplanned.
        let required_steps = self.required_steps_for_task(&task, &agent_name).await?;

        // An approval-held run already exists but has never launched. Once
        // approved, resume that exact frozen snapshot instead of creating a
        // second attempt that would ask for the same approval again.
        let existing = self.store.active_run(task_id).await?.filter(|run| {
            run.status == RunStatus::Blocked
                && run.blocked_source == Some(BlockSource::Verification)
                && run.session.is_none()
                && run
                    .required_steps
                    .iter()
                    .any(|s| s.kind == factory_core::control_plan::StepKind::Approval)
        });
        let resumed = existing.is_some();
        let token = existing
            .as_ref()
            .and_then(|r| r.token.clone())
            .unwrap_or_else(factory_core::new_token);
        let run = match existing {
            Some(run) => {
                self.store
                    .update_run(
                        &run.id,
                        &RunPatch {
                            status: Some(RunStatus::Dispatching),
                            clear_blocked: true,
                            ..Default::default()
                        },
                    )
                    .await?
            }
            None => {
                self.store
                    .create_run(&NewRun {
                        task_id: task.id.clone(),
                        trigger,
                        agent: agent_name.clone(),
                        adapter: adapter_name.clone(),
                        runtime: task.runtime.clone(),
                        token: token.clone(),
                        queued_at: Some(due.queued_at),
                        scheduled_for: due.scheduled_for,
                    })
                    .await?
            }
        };

        // The run exists from here on, so it holds its share of the power
        // assertion from here on too -- every `?` below this point ends the
        // run through `start_run`'s own error handling, which reaches
        // `fail_run` (since `active_run` now finds this row) and so
        // `close_session`, the one place this is ever released. Acquired
        // before the worktree and the agent/runtime calls rather than after
        // them: those are exactly the slow, fallible steps a sleeping host
        // could stall inside, which is the case this issue is about.
        let run = if resumed || required_steps.is_empty() {
            run
        } else {
            self.store
                .update_run(&run.id, &RunPatch { required_steps: Some(required_steps), ..Default::default() })
                .await?
        };

        if resumed {
            self.bus.publish(Event::RunUpdated { run: run.clone() });
        } else {
            self.bus.publish(Event::RunStarted { run: run.clone() });
        }
        self.publish_task(task_id).await;
        if !resumed {
            self.entry(
                task_id,
                TaskEntry::new(
                    "daemon",
                    "started",
                    format!("attempt {} started ({})", run.attempt, trigger.as_str()),
                )
                .in_run(&run.id),
            )
            .await;
        }
        if !resumed && !run.required_steps.is_empty() {
            let steps: Vec<String> = run
                .required_steps
                .iter()
                .map(|s| match s.required_by.is_empty() {
                    true => s.step.clone(),
                    false => format!("{} ({})", s.step, s.required_by.join(", ")),
                })
                .collect();
            self.entry(
                task_id,
                TaskEntry::new(
                    "daemon",
                    "plan",
                    format!(
                        "planned as {}; done needs: {}",
                        factory_core::control_plan::effective_category(task.category.as_deref()),
                        steps.join(", ")
                    ),
                )
                .in_run(&run.id)
                .with_data(serde_json::json!({ "required_steps": run.required_steps })),
            )
            .await;
        }

        let approval_evidence = self.policies.step_attestations(&run.id).await?;
        let pending_approval = run
            .required_steps
            .iter()
            .filter(|s| s.kind == factory_core::control_plan::StepKind::Approval)
            .find(|approval| {
                !approval_evidence
                    .iter()
                    .rev()
                    .find(|a| {
                        a.step == approval.step
                            && a.kind == factory_core::control_plan::StepKind::Approval
                    })
                    .is_some_and(|a| {
                        a.verdict == factory_core::control_plan::AttestationVerdict::Pass
                    })
            });
        if let Some(approval) = pending_approval {
                let held = self
                    .store
                    .update_run(
                        &run.id,
                        &RunPatch {
                            status: Some(RunStatus::Blocked),
                            blocked_since: Some(Utc::now()),
                            blocked_source: Some(BlockSource::Verification),
                            ..Default::default()
                        },
                    )
                    .await?;
                self.entry(
                    task_id,
                    TaskEntry::new(
                        "daemon",
                        "blocked",
                        format!("approval {} is required before dispatch", approval.step),
                    )
                    .in_run(&run.id)
                    .with_data(
                        serde_json::json!({ "approval": approval.step, "actor": approval.actor }),
                    ),
                )
                .await;
                self.bus.publish(Event::RunUpdated { run: held.clone() });
                self.mirror_to_task(&held).await;
                return Ok(held);
        }

        // Approval has passed (or none was required); only now does this run
        // acquire its liveness assertion and create an outward agent session.
        self.power.acquire(&run.id).await;

        // A worktree of its own, made now rather than left to the harness --
        // the run row already exists, so it is named after it. Nothing below
        // this point may hand the agent the scope itself when the checkbox is
        // on: a failure here ends the run right here, with git's own
        // complaint, rather than quietly falling back to the scope.
        let (cwd, run) = self.place_run(&task, run, &scope_path).await?;

        // Resolved the same way `caller_for` resolves it for every other
        // request, off the agent this run actually landed on rather than
        // whatever the task's own record says -- `resolve_agent` may have
        // fallen back to a bare adapter name the task did not ask for.
        let role = self.effective_role(&task.scope, &agent_name).await;
        let role = self.roles_for(&task.scope).get(&role).cloned();

        // Direct parents only, computed now rather than when the node's task
        // was created (`create_workflow_task`) -- so a restart's recovery
        // pass, which dispatches through this same function, needs no
        // change of its own to pick this up.
        let upstream = self.upstream_outputs(&task).await;
        let agent_exits = self.agent_exit_context(&task).await;
        let knowledge = self.knowledge_hints(&task, &run.id).await;
        // Same chain the L6 tab and `policy attest` fold against
        // (`Engine::policy_chain`), reduced to just the names the guide
        // names -- see `factory_core::policy::frameworks_in_chain`.
        let policy_frameworks = factory_core::policy::frameworks_in_chain(&self.policy_chain(&task.scope));
        // The one sentence the guide says about a `goal=` label -- resolved
        // once here, the same as `policy_frameworks`, never re-read once the
        // guide is built.
        let goal = self.goal_context(factory.root.clone(), task.labels.get("goal").cloned()).await;
        // The scope's H-importance quality attributes (`#107`), judged now
        // and never again for this run -- the same once-at-dispatch rule.
        let quality = self.quality_context(&task.scope).await;

        let ctx = AgentContext {
            scope: task.scope.clone(),
            agent_name: agent_name.clone(),
            cwd: cwd.clone(),
            factory_bin: self.factory_bin.clone(),
            socket: factory.socket_path(),
            guides_dir: factory.guides_dir(),
            task: Some(TaskBinding {
                task: task.clone(),
                run_id: run.id.clone(),
                attempt: run.attempt,
                token,
                worktree_branch: run.worktree_branch.clone(),
                upstream,
                knowledge,
                required_steps: run.required_steps.clone(),
                agent_exits,
            }),
            identity_token: None,
            role,
            policy_frameworks,
            goal,
            quality,
        };

        let mut launch = agent.launch_spec(&ctx).await?;
        append_declared_args(&mut launch, declaration.as_ref());
        // A task's stored `scope` can still be a scope's legacy bare name --
        // canonicalize it the same way `start_agent` does, so a legacy-named
        // task's run lands in the same workspace as that scope's standing
        // agents rather than a second one keyed on the old name.
        let canonical_scope = factory.canonical_scope_name(&task.scope);
        // A run's own id fragment is its discriminator: `start()` adopts any
        // agent already carrying the name it asks for, and reconcile can
        // dispatch this scope/agent pair again while an earlier run is still
        // live, so two concurrent runs must never resolve to the same herdr
        // agent.
        let run_id_fragment = &run.id[..8.min(run.id.len())];
        let session = runtime
            .start(&StartRequest {
                id: run.id.clone(),
                scope: canonical_scope.clone(),
                name: crate::agents::herdr_name(
                    &format!("factory-{}-{}", canonical_scope, agent_name),
                    Some(run_id_fragment),
                ),
                label: truncate(&task.title, 40),
                cwd,
                launch,
            })
            .await?;

        let run = self
            .store
            .update_run(
                &run.id,
                &RunPatch {
                    session: Some(session.clone()),
                    ..Default::default()
                },
            )
            .await?;
        self.bus.publish(Event::RunUpdated { run: run.clone() });

        // The baseline, before the task is handed over: whatever the session
        // had already used -- a pane an earlier run left behind, a harness
        // that spent tokens coming up -- is not this run's (#117). Never a
        // `?`: a runtime with no usage to give must not fail the run.
        self.snapshot_usage(&run, factory_core::usage::SnapshotPoint::Dispatch)
            .await;

        let prompt = agent.prompt(&ctx).await?;
        runtime.submit(&session, &prompt).await?;

        self.entry(
            task_id,
            TaskEntry::new(
                "daemon",
                "dispatched",
                format!("handed to {agent_name} ({adapter_name}) on {}", session.runtime),
            )
            .in_run(&run.id)
            .with_data(serde_json::json!({ "session": session })),
        )
        .await;
        Ok(run)
    }

    /// Where a run actually works: its own worktree, or the scope directly.
    /// Pulled out of `dispatch` so the decision -- and the one way it can
    /// fail -- has no need of a real agent or runtime on the other end of it,
    /// which is what lets it be tested on its own.
    ///
    /// `task.worktree` off is the whole of the "quietly ignored" case this
    /// function refuses to have: it is checked once, here, and every path out
    /// of it either returns the scope path unchanged or a worktree that
    /// `git worktree add` actually made. There is no third path.
    async fn place_run(&self, task: &Task, run: Run, scope_path: &Path) -> Result<(PathBuf, Run)> {
        if !task.worktree {
            return Ok((scope_path.to_path_buf(), run));
        }
        let branch = worktree::branch_name(&task.id, &task.title, run.attempt);
        let dir = self.factory_snapshot().worktrees_dir().join(&run.id);
        let base = match &task.bench_origin {
            Some(origin) => self.bench_case_base(origin).await,
            None => None,
        };
        worktree::create(scope_path, &dir, &branch, base.as_deref())
            .await
            .map_err(|e| FactoryError::adapter("git", e))?;
        let run = self
            .store
            .update_run(
                &run.id,
                &RunPatch {
                    worktree_path: Some(dir.display().to_string()),
                    worktree_branch: Some(branch.clone()),
                    ..Default::default()
                },
            )
            .await?;
        self.bus.publish(Event::RunUpdated { run: run.clone() });
        self.entry(
            &run.task_id,
            TaskEntry::new(
                "daemon",
                "worktree",
                format!("working in {} on {branch}", dir.display()),
            )
            .in_run(&run.id),
        )
        .await;

        // A bench case is never run dirty: its reset command, when it has
        // one, runs in the fresh worktree before the agent is handed
        // anything. A non-zero exit ends the run here -- `fail_run` gives it
        // the daemon's usual terminal handling, and `sync_bench_for_task`
        // reads this exact "reset failed" prefix back off `run.error` to
        // settle the attempt `skipped` rather than `error`, without ever
        // dispatching the agent.
        if let Some(origin) = &task.bench_origin {
            if let Some(reset) = self.bench_case_reset(origin).await {
                if let Err(detail) = self.run_bench_reset(&dir, &reset).await {
                    return Err(FactoryError::BadRequest(format!("reset failed: {detail}")));
                }
            }
        }
        Ok((dir, run))
    }

    /// This task's direct parents in a workflow, in the definition's own edge
    /// order (deterministic run to run), with the result each finished with.
    /// Empty for a root node or a task outside any workflow at all. A store
    /// or workflow-run read failure is logged and treated as "nothing found"
    /// rather than failing the dispatch -- the task still runs, just without
    /// the section or file it would otherwise have carried.
    async fn upstream_outputs(&self, task: &Task) -> Vec<UpstreamOutput> {
        let Some(origin) = &task.workflow_origin else {
            return Vec::new();
        };
        let run = match self.workflows.get_run(&origin.workflow_run_id).await {
            Ok(Some(run)) => run,
            Ok(None) => {
                tracing::warn!(
                    task = task.id,
                    workflow_run = origin.workflow_run_id,
                    "workflow run not found; dispatching without upstream outputs"
                );
                return Vec::new();
            }
            Err(error) => {
                tracing::warn!(
                    task = task.id,
                    workflow_run = origin.workflow_run_id,
                    "reading workflow run for upstream outputs: {error}"
                );
                return Vec::new();
            }
        };

        let mut outputs = Vec::new();
        for edge in run.definition.edges.iter().filter(|edge| edge.to == origin.node_id) {
            let Some(parent_task_id) = run
                .nodes
                .iter()
                .find(|node| node.node_id == edge.from)
                .and_then(|node| node.task_id.clone())
            else {
                // The parent node never got a task (denied, or the run
                // failed before it was its turn) -- nothing to report.
                continue;
            };
            match self.store.get(&parent_task_id).await {
                Ok(Some(parent)) => outputs.push(UpstreamOutput {
                    node_id: edge.from.clone(),
                    task_id: parent.id.clone(),
                    title: parent.title.clone(),
                    result: parent
                        .result
                        .as_deref()
                        .map(|r| truncate_tail(r, UPSTREAM_RESULT_BYTE_CAP).into_owned()),
                }),
                Ok(None) => tracing::warn!(
                    task = task.id,
                    parent_task = parent_task_id,
                    "parent task for upstream output no longer exists"
                ),
                Err(error) => tracing::warn!(
                    task = task.id,
                    parent_task = parent_task_id,
                    "reading parent task for upstream output: {error}"
                ),
            }
        }
        // Work sent back here comes with what the node that sent it said:
        // the result from `done --send-to`, or the error/results retained by
        // an in-flight run loaded from the legacy failure-routing format.
        let request = run
            .nodes
            .iter()
            .find(|node| node.node_id == origin.node_id && node.task_id.as_deref() == Some(task.id.as_str()))
            .and_then(|node| node.rework_request.clone());
        if let Some(request) = request {
            match self.store.get(&request.from_task).await {
                Ok(Some(reviewer)) => {
                    let said: Vec<&str> = [reviewer.error.as_deref(), reviewer.result.as_deref()]
                        .into_iter()
                        .flatten()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .collect();
                    outputs.push(UpstreamOutput {
                        node_id: request.from_node.clone(),
                        task_id: reviewer.id.clone(),
                        title: format!(
                            "{} sent this work back -- rework round {} of {}",
                            reviewer.title, request.round, request.max_rounds
                        ),
                        result: (!said.is_empty())
                            .then(|| truncate_tail(&said.join("\n\n"), UPSTREAM_RESULT_BYTE_CAP).into_owned()),
                    });
                }
                Ok(None) => tracing::warn!(task = task.id, from_task = request.from_task, "rework source no longer exists"),
                Err(error) => tracing::warn!(task = task.id, from_task = request.from_task, "reading rework source: {error}"),
            }
        }
        outputs
    }

    /// Agent-selectable exits for this exact workflow-node round. This is
    /// computed once at dispatch so the reporting contract cannot change
    /// underneath a running agent.
    async fn agent_exit_context(&self, task: &Task) -> Vec<AgentExitContext> {
        let Some(origin) = &task.workflow_origin else {
            return Vec::new();
        };
        let Ok(Some(run)) = self.workflows.get_run(&origin.workflow_run_id).await else {
            return Vec::new();
        };
        let used = run
            .nodes
            .iter()
            .find(|node| node.node_id == origin.node_id)
            .map_or(0, |node| node.round);
        run.definition
            .nodes
            .iter()
            .find(|node| node.id == origin.node_id)
            .into_iter()
            .flat_map(|node| &node.exits)
            .filter_map(|exit| {
                exit.agent.as_ref().map(|rule| AgentExitContext {
                    to: exit.to.clone(),
                    rule: rule.clone(),
                    max_rounds: exit.max_rounds,
                    rounds_used: used,
                })
            })
            .collect()
    }

    /// The token is what makes a call about a run come from that run --
    /// the agent's own report, or its harness's turn-end hook -- rather than
    /// from anyone on the socket closing anyone's run.
    pub(crate) fn check_run_token(&self, run: &Run, given: Option<&str>, task_id: &str) -> Result<()> {
        let Some(expected) = &run.token else {
            return Ok(());
        };
        match given {
            Some(given) if given == expected => Ok(()),
            Some(_) => Err(FactoryError::Denied(format!(
                "wrong token for attempt {} of task {task_id}",
                run.attempt
            ))),
            None => Err(FactoryError::Denied(format!(
                "attempt {} of task {task_id} needs its run token; it is FACTORY_TASK_TOKEN in the session, or pass --run-token",
                run.attempt
            ))),
        }
    }

    /// What an agent says about its own run. The token is what makes this a
    /// report rather than anyone on the socket closing anyone's run.
    pub async fn report(&self, task_id: &str, report: TaskReport) -> Result<Run> {
        let run = self.store.active_run(task_id).await?.ok_or_else(|| {
            FactoryError::BadRequest(format!(
                "task {task_id} has no run in progress; reports are no longer accepted"
            ))
        })?;

        self.check_run_token(&run, report.token.as_deref(), task_id)?;
        let reporting_task = self.require(task_id).await?;
        let review_report = reporting_task
            .labels
            .contains_key(crate::verification::REVIEW_RUN_LABEL);
        if !review_report {
            self.validate_workflow_send_to(task_id, report.status, report.send_to.as_deref())
                .await?;
        } else if report.send_to.is_some() && report.status != Some(RunStatus::Done) {
            return Err(FactoryError::BadRequest(
                "a review rejects only with status done and --send-to".into(),
            ));
        }

        // While its gates run, a run's status is the verifier's to set. The
        // agent may still add a note, or give the run up; nothing else.
        if run.status == RunStatus::Verifying
            && report.status.is_some_and(|s| !matches!(s, RunStatus::Failed | RunStatus::Cancelled))
        {
            return Err(FactoryError::BadRequest(format!(
                "attempt {} of task {task_id} is being verified -- its required steps are running; \
                 the run becomes done or blocked when they finish",
                run.attempt
            )));
        }
        if report.status == Some(RunStatus::Verifying) {
            return Err(FactoryError::BadRequest(
                "verifying is the daemon's to set; report done and it verifies the run".into(),
            ));
        }

        let message = report.message.clone().unwrap_or_else(|| {
            report
                .result
                .clone()
                .or_else(|| report.error.clone())
                .unwrap_or_else(|| "(no message)".into())
        });

        self.entry(
            task_id,
            TaskEntry::new(
                "agent",
                report
                    .status
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_else(|| "note".into()),
                message,
            )
            .in_run(&run.id),
        )
        .await;

        let mut patch = RunPatch {
            status: report.status,
            result: report.result,
            routed_to: report.send_to.clone(),
            clear_routed_to: report.status == Some(RunStatus::Done) && report.send_to.is_none(),
            error: report.error,
            // The agent is talking, so whatever turn a `Stop` hook said had
            // ended has not -- see `occupancy::settle_turn_end`.
            clear_turn_ended: true,
            ..Default::default()
        };

        if let Some(to) = &report.send_to {
            self.entry(
                task_id,
                TaskEntry::new(
                    "agent",
                    "routed_to",
                    format!("reported done; routed to {to}"),
                )
                .in_run(&run.id)
                .with_data(serde_json::json!({ "routed_to": to })),
            )
            .await;
        }

        // The agent's own report is the one thing that may set or clear
        // `Blocked` honestly for its own sake -- see `AGENTS.md` and issue
        // #7. Reporting `blocked` again while already blocked leaves
        // `blocked_since` alone, so the clock still reads from when the
        // block actually began; reporting anything else always lets go of
        // it, agent-set or not, because this report is the agent speaking.
        // The agent ending its own run is the one terminal report that says
        // why in so many words.
        patch.fail_kind = match report.status {
            Some(RunStatus::Failed) => Some(FailKind::AgentFailed),
            Some(RunStatus::Cancelled) => Some(FailKind::CancelledByAgent),
            _ => None,
        };

        match report.status {
            Some(RunStatus::Blocked) => {
                patch.blocked_source = Some(BlockSource::Agent);
                if run.status != RunStatus::Blocked {
                    patch.blocked_since = Some(Utc::now());
                }
            }
            Some(_) => patch.clear_blocked = true,
            None => {}
        };

        let updated = match report.status {
            // `#118`'s done gate: a run held to required steps is not done on
            // its agent's word. The session stays open -- see
            // `verification.rs`.
            Some(RunStatus::Done) if !run.required_steps.is_empty() => {
                self.begin_verification(&run, patch).await?
            }
            Some(status) if status.is_terminal() => {
                self.close_session(&run).await;
                self.finish_run(&run.id, status, patch, &format!("attempt {} ended", run.attempt))
                .await?
            }
            _ => {
                let run = self.store.update_run(&run.id, &patch).await?;
                self.bus.publish(Event::RunUpdated { run: run.clone() });
                self.mirror_to_task(&run).await;
                run
            }
        };
        if review_report && report.status == Some(RunStatus::Done) {
            self.settle_review_task(&reporting_task, &updated).await?;
        }
        Ok(updated)
    }

    /// Cancel a task's active run. `kind` says on whose word -- a person, an
    /// agent, or the workflow or bench run above it -- and is recorded on
    /// the run; the journal line stays the same for all three.
    pub(crate) async fn cancel_task_run(&self, task_id: &str, expected: Option<&str>, kind: FailKind) -> Result<Run> {
        let run = self.store.active_run(task_id).await?.ok_or_else(|| {
            FactoryError::BadRequest(format!("task {task_id} has no run to cancel"))
        })?;
        // The caller named the attempt it saw. Anything else running now --
        // a retry that started since, a manual run -- is not what it asked
        // to end.
        if let Some(expected) = expected.filter(|e| *e != run.id) {
            return Err(FactoryError::BadRequest(format!(
                "run {expected} is no longer the task's active run (attempt {} is, {}); nothing was cancelled",
                run.attempt,
                run.status.as_str()
            )));
        }
        self.close_session(&run).await;
        self.finish_run(
            &run.id,
            RunStatus::Cancelled,
            RunPatch {
                status: Some(RunStatus::Cancelled),
                fail_kind: Some(kind),
                ..Default::default()
            },
            "cancelled by request",
        ).await
    }

    /// End a run and settle the task behind it. A task with a schedule goes
    /// back to `pending` so the scheduler will pick it up again; one without
    /// keeps the run's own outcome.
    pub(crate) async fn finish_run(
        &self,
        run_id: &str,
        status: RunStatus,
        patch: RunPatch,
        _why: &str,
    ) -> Result<Run> {
        let run = self
            .store
            .update_run(
                run_id,
                &RunPatch {
                    status: Some(status),
                    clear_session: true,
                    clear_token: true,
                    // A run that has ended is not waiting on anybody, so the
                    // block's own clock and the runtime's standing guess both
                    // go with the session -- `blocked_since` is documented to
                    // be `None` whenever the status is not `Blocked`, and a
                    // finished run is the one path that could otherwise leave
                    // it set. Forced here rather than left to `patch`: every
                    // terminal status comes through this function, and only
                    // the agent's own report remembered to clear it.
                    clear_blocked: true,
                    clear_block_suspicion: true,
                    // Likewise a held `Stop` turn end: nothing is left to
                    // settle once the run is over.
                    clear_turn_ended: true,
                    ended_at: Some(Utc::now()),
                    ..patch
                },
            )
            .await?;
        self.bus.publish(Event::RunUpdated { run: run.clone() });
        // The run's session is released here, so nothing will poll it again.
        // Close its liveness span now or the chart draws the agent as still
        // working, forever.
        if let Ok(Some(task)) = self.store.get(&run.task_id).await {
            self.record_gone(&format!("run:{}", run.id), &task.scope, &run.agent)
                .await;
        }
        self.mirror_to_task(&run).await;
        self.settle_retry(&run).await;
        Ok(run)
    }

    /// After the task's mirror is updated, decide what a scheduled task's
    /// retry state should be. Only a task with a `schedule` retries at all --
    /// a one-off task that fails is left blocked on the failure by
    /// `mirror_to_task` (`#122`), and nothing here touches it.
    ///
    /// A run that finished by succeeding or by being cancelled ends any
    /// retry streak in progress: `pending_retry` is the mirror `AGENTS.md`
    /// warns about, and leaving it set past the run it describes would
    /// misreport a task that just succeeded as still mid-retry, or -- worse
    /// -- let a later, unrelated failure resume counting from someone else's
    /// streak instead of starting its own. A run that failed either queues
    /// the next retry, if the effective policy allows one, or lets the
    /// streak end where it stands.
    async fn settle_retry(&self, run: &Run) {
        let Ok(Some(task)) = self.store.get(&run.task_id).await else {
            return;
        };
        if task.schedule.is_none() {
            return;
        }

        match run.status {
            RunStatus::Failed => self.queue_or_end_retry(&task, run).await,
            status if status.is_terminal() => {
                // `Failed` is handled above, so this is `Done` or
                // `Cancelled` -- either way, if a streak was in progress it
                // is over now.
                if let Some(pending) = &task.pending_retry {
                    let why = if status == RunStatus::Done {
                        "a retry succeeded"
                    } else {
                        "the pending retry was cancelled"
                    };
                    self.end_retry_streak(&task, pending.resume_at, why).await;
                }
            }
            _ => {}
        }
    }

    /// A scheduled task's run just failed. Either queue its next retry --
    /// moving `next_run_at` to `now + backoff` without disturbing the regular
    /// firing this streak is standing in front of -- or, if the policy
    /// forbids retrying at all or this streak has used up its attempts, let
    /// it end and fall back to that regular firing.
    async fn queue_or_end_retry(&self, task: &Task, run: &Run) {
        let policy = task
            .retry
            .unwrap_or(self.factory_snapshot().config.daemon.default_retry);
        let (max_attempts, backoff_seconds) = match policy {
            RetryPolicy::None => {
                match &task.pending_retry {
                    // A streak already in progress when the policy changes to
                    // `none` (an edit landed mid-streak) is honoured
                    // immediately rather than firing the retry it no longer
                    // wants.
                    Some(pending) => {
                        self.end_retry_streak(task, pending.resume_at, "the task's retry policy is now `none`")
                            .await;
                    }
                    // The ordinary case: no retry was ever queued, so
                    // `mirror_to_task` deliberately left this task without a
                    // "what happens next" line -- see its own comment on why
                    // it stays silent for a `Failed` recurring task -- and
                    // this is the one place that knows the answer: nothing
                    // will try again before its next regular firing.
                    None => {}
                }
                self.block_on_failure(&task.id, &format!("attempt {} failed and the retry policy is `none`", run.attempt))
                    .await;
                return;
            }
            RetryPolicy::Backoff { max_attempts, backoff_seconds } => (max_attempts, backoff_seconds),
        };

        // The regular firing this streak must never disturb. Captured once,
        // the first time a run in this streak fails, from `next_run_at` as
        // `advance_schedule` (or, mid-streak, `resume_from_retry`) had
        // already left it before this attempt was dispatched -- every later
        // failure in the same streak carries it forward from `pending_retry`
        // rather than reading `next_run_at` again, which by then holds an
        // earlier retry time, not the regular slot.
        let resume_at = task
            .pending_retry
            .as_ref()
            .map(|p| p.resume_at)
            .or(task.next_run_at)
            .unwrap_or_else(Utc::now);
        let attempts = task.pending_retry.as_ref().map(|p| p.attempts).unwrap_or(0) + 1;

        if attempts > max_attempts {
            let why = format!(
                "retries exhausted after {max_attempts} attempt{}",
                if max_attempts == 1 { "" } else { "s" }
            );
            self.end_retry_streak(task, resume_at, &why).await;
            self.block_on_failure(&task.id, &why).await;
            return;
        }

        let retry_at = Utc::now() + chrono::Duration::seconds(backoff_seconds as i64);
        self.entry(
            &task.id,
            TaskEntry::new(
                "daemon",
                "retrying",
                format!(
                    "attempt {} failed; retrying at {} (retry {attempts} of {max_attempts}), \
                     without touching the regular firing at {}",
                    run.attempt,
                    retry_at.to_rfc3339(),
                    resume_at.to_rfc3339(),
                ),
            )
            .in_run(&run.id),
        )
        .await;

        // Back to `Pending` from the block `mirror_to_task` left it in: a
        // retry is coming, and the task shows that it is retrying.
        let patch = TaskPatch {
            status: Some(TaskStatus::Pending),
            next_run_at: Some(retry_at),
            pending_retry: Some(PendingRetry { attempts, resume_at }),
            ..Default::default()
        };
        if let Ok(updated) = self.store.update(&task.id, &patch).await {
            self.bus.publish(Event::TaskUpdated { task: updated });
        }
    }

    /// End a retry streak: clear `pending_retry` and restore `next_run_at` to
    /// the regular firing the streak was standing in front of. A no-op patch
    /// is avoided when there was nothing to clear -- the common case, a task
    /// whose very first failure got no retry at all, in which `next_run_at`
    /// already holds `resume_at` untouched and `pending_retry` is already
    /// `None`.
    async fn end_retry_streak(&self, task: &Task, resume_at: chrono::DateTime<Utc>, why: &str) {
        if task.pending_retry.is_none() && task.next_run_at == Some(resume_at) {
            return;
        }
        self.entry(
            &task.id,
            TaskEntry::new(
                "daemon",
                "retry_settled",
                format!("{why}; resuming the regular schedule at {}", resume_at.to_rfc3339()),
            ),
        )
        .await;

        let patch = TaskPatch {
            next_run_at: Some(resume_at),
            clear_pending_retry: true,
            ..Default::default()
        };
        if let Ok(updated) = self.store.update(&task.id, &patch).await {
            self.bus.publish(Event::TaskUpdated { task: updated });
        }
    }

    /// The task's own row carries the latest run's outcome, so a list does not
    /// have to read every run.
    ///
    /// `pub(crate)`: `occupancy::record_run_liveness` mirrors a run it just
    /// moved into or out of `Blocked` the same way `report` does here --
    /// the same pattern as `record_gone`, which already crosses this
    /// boundary the other way.
    pub(crate) async fn mirror_to_task(&self, run: &Run) {
        let recurring = self
            .store
            .get(&run.task_id)
            .await
            .ok()
            .flatten()
            .and_then(|t| t.schedule)
            .is_some();

        // A failed run blocks its task, recurring or not (`#122`). For a
        // recurring one that is the safe side of what `settle_retry` --
        // which `finish_run` calls right after this -- decides next: a
        // queued retry puts it back to `Pending` (`queue_or_end_retry`), and
        // anything else leaves it blocked. A crash in between leaves it
        // blocked, never in Scheduled looking healthy.
        let status = if run.status.is_terminal() && recurring && run.status != RunStatus::Failed {
            TaskStatus::Pending
        } else {
            run.status.as_task_status()
        };

        // A `Failed` recurring task does not necessarily get its next real
        // turn next -- `finish_run` calls `settle_retry` right after this,
        // and that may queue a retry sooner than the schedule's own next
        // slot. Saying "waiting for its next turn" here and then, a moment
        // later, "retrying in 5 minutes" would leave the journal
        // contradicting itself on every retry, so this stays silent for
        // `Failed` and leaves the "what happens next" line to whichever of
        // `queue_or_end_retry` or `end_retry_streak` actually decides it.
        if run.status.is_terminal() && recurring && run.status != RunStatus::Failed {
            self.entry(
                &run.task_id,
                TaskEntry::new(
                    "daemon",
                    "rearmed",
                    "recurring task is pending again, waiting for its next turn",
                ),
            )
            .await;
        }

        // The task mirrors the newest run, not the union of every run: an
        // attempt that succeeded must not leave the previous one's error
        // standing next to its own result -- nor its failure, nor a close
        // record from before it ran (`#122`).
        let failed = run.status == RunStatus::Failed;
        let patch = TaskPatch {
            status: Some(status),
            result: run.result.clone(),
            routed_to: run.routed_to.clone(),
            clear_result: run.result.is_none(),
            clear_routed_to: run.routed_to.is_none(),
            error: run.error.clone(),
            clear_error: run.error.is_none(),
            failure: failed.then(|| TaskFailure {
                kind: run.fail_kind,
                run_id: Some(run.id.clone()),
                attempt: Some(run.attempt),
                at: run.ended_at.unwrap_or_else(Utc::now),
            }),
            clear_failure: !failed,
            clear_closure: true,
            ..Default::default()
        };
        if let Ok(task) = self.store.update(&run.task_id, &patch).await {
            self.bus.publish(Event::TaskUpdated { task });
        }
    }

    /// A scheduled task whose failure nothing will retry stays blocked on it
    /// (`#122`) -- `mirror_to_task` already put it there -- and this says
    /// so. Its schedule keeps firing: the intent still stands, and the next
    /// run that succeeds clears the block. It is never left in Scheduled
    /// looking like a task that works.
    async fn block_on_failure(&self, task_id: &str, why: &str) {
        self.entry(
            task_id,
            TaskEntry::new(
                "daemon",
                "blocked_on_failure",
                format!("{why}; blocked until someone looks -- the schedule keeps firing, and a run that succeeds clears it"),
            ),
        )
        .await;
    }

    /// A run that will never report back. `kind` is the reason as a fact --
    /// every caller has to name one, which is what keeps "why do runs fail"
    /// answerable without reading `why`'s prose back apart.
    pub async fn fail_run(self: &Arc<Self>, run_id: &str, kind: FailKind, why: &str) {
        let Ok(run) = self.require_run(run_id).await else {
            return;
        };
        self.close_session(&run).await;
        self.entry(
            &run.task_id,
            TaskEntry::new("daemon", "failed", why.to_string()).in_run(run_id),
        )
        .await;
        let _ = self
            .finish_run(
                run_id,
                RunStatus::Failed,
                RunPatch {
                    error: Some(why.to_string()),
                    fail_kind: Some(kind),
                    ..Default::default()
                },
                why,
            )
            .await;
        self.record_workflow_task_state(&run.task_id).await;
        self.record_bench_task_state(&run.task_id).await;
    }

    /// Keep the last of what the agent saw, then let the session go. Every
    /// path that ends a run -- a terminal report, a cancel, a task deleted
    /// out from under an active run, or the watchdog giving up on it --
    /// comes through here, which is what makes this the one place that
    /// actually knows a run is over rather than guessing from one caller's
    /// reason for closing it.
    pub(crate) async fn close_session(&self, run: &Run) {
        // First and unconditional, ahead of the early `return` below for a
        // run that never got as far as a session (a dispatch failure) --
        // `power.release` is the counterpart to `dispatch`'s own
        // `power.acquire`, and every run that reaches this function reaches
        // it regardless of whether it ever had a session to close.
        self.power.release(&run.id).await;

        // The guide file, if this run's harness wrote one, is named after the
        // task rather than the run and nothing else removes it. It cannot be
        // deleted right after launch: a harness may read its configured
        // instruction file after launch, not only at startup (opencode
        // resolves instruction paths from config, and claude may read the
        // file after `herdr agent start` returns), so the file has to outlive
        // the launch and is removed only once the run is over. One that
        // carried the guide as inline text or not at all (`codex`, `shell`)
        // never wrote a file, so this is a harmless no-op for those, and a
        // run whose session never even came up still gets whatever
        // `launch_spec` managed to write before it failed cleaned up.
        let guide = run_guide_path(&self.factory_snapshot().guides_dir(), &run.task_id);
        let _ = std::fs::remove_file(guide);
        // Same story for the upstream-outputs file (`ShellAgent` writes it
        // and exports its path as `FACTORY_UPSTREAM_FILE`; a harness agent
        // never writes one at all, since it renders the same data inline
        // instead): named after the task, nothing else removes it, harmless
        // to remove when this run never wrote one.
        let upstream = upstream_output_path(&self.factory_snapshot().guides_dir(), &run.task_id);
        let _ = std::fs::remove_file(upstream);
        // And the knowledge-hints file, the same way for the same reader.
        let knowledge = knowledge_hints_path(&self.factory_snapshot().guides_dir(), &run.task_id);
        let _ = std::fs::remove_file(knowledge);
        // The shell agent's generated wrapper script, keyed by *run* id
        // rather than task id (see `run_shell_script_path`'s own comment) --
        // a retry's fresh run must never lose its script to this cleanup of
        // an earlier attempt's. Unlinking a file the pane's shell is still
        // sourcing is safe on Unix: the shell holds the file open, so
        // removing the directory entry does not disturb it, and a shell
        // reads a sourced file's content in rather than re-opening it line
        // by line, so there is no window where this could cut a run off
        // mid-script.
        let script = run_shell_script_path(&self.factory_snapshot().guides_dir(), &run.id);
        let _ = std::fs::remove_file(script);
        // And the harness settings file carrying a `claude` run's turn-end
        // hooks, keyed by run id for the same reason. Claude Code has
        // already read it at startup; a hook that fires as the session
        // closes runs from what it loaded then, and finds no run to end.
        let hooks = run_hook_settings_path(&self.factory_snapshot().guides_dir(), &run.id);
        let _ = std::fs::remove_file(hooks);

        let Some(session) = &run.session else { return };
        if let Ok(runtime) = self.registry.runtime(&session.runtime) {
            if let Ok(text) = runtime.read(session, 400).await {
                if !text.trim().is_empty() {
                    self.entry(
                        &run.task_id,
                        TaskEntry::new("daemon", "transcript", "final terminal output")
                            .in_run(&run.id)
                            .with_data(serde_json::json!({ "text": text })),
                    )
                    .await;
                }
            }
            // The last reading, while the session is still there to ask.
            self.snapshot_usage(run, factory_core::usage::SnapshotPoint::RunEnd)
                .await;
            let _ = runtime.stop(session).await;
        }
    }

    /// Terminal output for a run: live while it is running, the transcript kept
    /// at the end once it is not, and an empty string in the moment between a
    /// run starting and its session existing. Never an error -- a view that
    /// polls this should show a blank pane, not a red one.
    pub async fn output(&self, run: &Run, lines: u32) -> String {
        if let Some(session) = &run.session {
            if let Ok(runtime) = self.registry.runtime(&session.runtime) {
                if let Ok(text) = runtime.read(session, lines).await {
                    return text;
                }
            }
        }
        let entries = self.store.run_entries(&run.id, 500).await.unwrap_or_default();
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

    pub async fn due_now(&self) -> Result<Vec<Task>> {
        // The sqlite store already passes over a paused schedule; an
        // out-of-process store may predate the field and not know to, so
        // the rule is held here as well rather than trusted to every
        // adapter.
        let mut due = self.store.due(Utc::now()).await?;
        due.retain(|t| !t.schedule_paused && t.fires());
        Ok(due)
    }

    pub async fn active_runs(&self) -> Result<Vec<Run>> {
        self.store.active_runs().await
    }

    /// Every task still stored as `failed` -- written before `#122`, when a
    /// failed run closed its task -- moved to `Blocked` on that failure, so
    /// the board shows it where a person has to act instead of in a column
    /// it no longer has. The failure is read off the newest run; a task
    /// that never got a run (dispatch refused) has none to read, and is
    /// blocked on a dispatch failure, which is the only way that happened.
    ///
    /// Blocked, not closed as not planned: the rule this exists for is that
    /// a failure is an open item until a person disposes of it, and a bulk
    /// close would be the daemon disposing of every one at once. A
    /// scheduled one picks its schedule up from now -- its slots stopped
    /// firing the moment it failed, and must not come back as a burst.
    ///
    /// Idempotent and cheap: once a row is moved, nothing writes `failed`
    /// again, so later starts find none. Returns how many were moved.
    pub async fn migrate_failed_tasks(&self) -> usize {
        let filter = TaskFilter { status: Some(TaskStatus::Failed), ..Default::default() };
        let Ok(tasks) = self.store.list(&filter).await else {
            return 0;
        };
        let mut moved = 0;
        for task in tasks {
            let newest = self.store.runs(&task.id, 1).await.ok().and_then(|r| r.into_iter().next());
            let failure = match &newest {
                Some(run) => TaskFailure {
                    kind: run.fail_kind,
                    run_id: Some(run.id.clone()),
                    attempt: Some(run.attempt),
                    at: run.ended_at.unwrap_or(task.updated_at),
                },
                None => TaskFailure {
                    kind: Some(FailKind::DispatchFailed),
                    run_id: None,
                    attempt: None,
                    at: task.updated_at,
                },
            };
            let next_run_at = match &task.schedule {
                Some(s) if !task.schedule_paused => schedule::next_after(s, Utc::now()).ok(),
                _ => None,
            };
            let patch = TaskPatch {
                status: Some(TaskStatus::Blocked),
                failure: Some(failure),
                next_run_at,
                ..Default::default()
            };
            match self.store.update(&task.id, &patch).await {
                Ok(updated) => {
                    moved += 1;
                    self.entry(
                        &task.id,
                        TaskEntry::new(
                            "daemon",
                            "migrated",
                            "stored as failed before failures stopped closing tasks (#122): now blocked on that \
                             failure until someone runs it again or closes it with a reason",
                        ),
                    )
                    .await;
                    self.bus.publish(Event::TaskUpdated { task: updated });
                }
                Err(e) => tracing::warn!(task = %task.id, error = %e, "could not move a failed task to blocked"),
            }
        }
        if moved > 0 {
            tracing::info!(moved, "moved tasks stored as failed to blocked (#122)");
        }
        moved
    }

    /// Move a scheduled task's next firing forward so it is not picked up twice
    /// while it runs.
    ///
    /// The next firing is computed from now, not from the slot that is
    /// firing, so a task that was still running through later slots -- or a
    /// daemon that was down through them -- fires once, not once per slot.
    /// Those passed-over slots are written down here, as one
    /// `schedule_skipped` entry, because this is the only place that knows
    /// they existed: nothing else would ever say the 03:00 run did not
    /// happen.
    pub async fn advance_schedule(&self, task: &Task) -> Result<()> {
        let Some(s) = &task.schedule else {
            return Ok(());
        };
        let now = Utc::now();
        if let Some(slot) = task.next_run_at {
            let tick = chrono::Duration::seconds(self.factory_snapshot().config.daemon.tick_seconds.max(1) as i64);
            if let Some(skipped) = schedule::skipped_beyond_tick(s, slot, now, tick) {
                let reason = self.skip_reason(task, skipped.first).await;
                self.entry(
                    &task.id,
                    TaskEntry::new(
                        "scheduler",
                        "schedule_skipped",
                        format!(
                            "{} slot{} between {} and {} passed without firing ({}); \
                             firing once for {} instead of catching up",
                            skipped.count,
                            if skipped.count == 1 { "" } else { "s" },
                            skipped.first.to_rfc3339(),
                            skipped.last.to_rfc3339(),
                            reason.describe(),
                            slot.to_rfc3339(),
                        ),
                    )
                    .with_data(serde_json::json!({
                        "count": skipped.count,
                        "first": skipped.first,
                        "last": skipped.last,
                        "capped": skipped.capped,
                        "fired_slot": slot,
                        "reason": reason.as_str(),
                    })),
                )
                .await;
            }
        }
        let next = schedule::next_after(s, now)?;
        let updated = self
            .store
            .update(
                &task.id,
                &TaskPatch {
                    next_run_at: Some(next),
                    ..Default::default()
                },
            )
            .await?;
        self.bus.publish(Event::TaskUpdated { task: updated });
        Ok(())
    }

    /// The `Due` for a scheduled firing (`scheduled: true`, `at` is the slot)
    /// or a queued retry (`at` is when its backoff ran out): due at `at`,
    /// queued from the moment it could first have been dispatched -- see
    /// `dispatchable_from`. Read before `advance_schedule` or
    /// `resume_from_retry` moves `next_run_at` on, and before the new run
    /// exists, so the newest run is the previous one.
    pub async fn due_for(&self, task: &Task, at: chrono::DateTime<Utc>, scheduled: bool) -> Due {
        let previous = self.store.runs(&task.id, 1).await.ok().and_then(|r| r.into_iter().next());
        let mut due = if scheduled { Due::slot(at) } else { Due::retry(at) };
        due.queued_at = dispatchable_from(at, self.booted_at, previous.as_ref());
        due
    }

    /// Why the slots from `first` on passed without firing: the task's own
    /// previous run was still going at `first`, or it was not -- in which
    /// case the daemon was not there to fire it (asleep, stopped, or its
    /// tick running late). Read off the newest run's own times, the only
    /// record either way.
    async fn skip_reason(&self, task: &Task, first: chrono::DateTime<Utc>) -> SkipReason {
        let newest = self.store.runs(&task.id, 1).await.ok().and_then(|r| r.into_iter().next());
        skip_reason_of(newest.as_ref(), first)
    }

    /// Move a queued retry's task back onto the regular slot its streak is
    /// standing in front of, before dispatching the retry attempt itself --
    /// the retry's own version of what `advance_schedule` does for a regular
    /// firing above, and for the same reason: a dispatch slower than the
    /// backoff must not pick the task up a second time.
    ///
    /// Deliberately does not recompute the schedule the way `advance_schedule`
    /// does: `resume_at` was captured once, in `queue_or_end_retry`, when this
    /// streak began, and must survive however many retries happen before it
    /// ends. Recomputing from `now` here instead would be redundant at best
    /// for a `Cron` schedule (grid-aligned, so it would usually land on the
    /// same slot anyway) and actively wrong for an `Every` schedule, which
    /// has no grid at all -- each recompute would push the regular firing
    /// further out, which is exactly the displacement the scheduler's own
    /// comment on `advance_schedule` running before dispatch warns against.
    ///
    /// Dispatches this retry unconditionally, even if the task's policy was
    /// edited to `retry: none` after this streak began: a retry already
    /// queued and due is one the daemon committed to when it queued it, and
    /// pulling a run back out from under a dispatch already under way would
    /// be its own kind of surprise. `queue_or_end_retry` is where the new
    /// policy actually takes effect -- honoured on this attempt's outcome,
    /// not retroactively on the attempt itself.
    pub async fn resume_from_retry(&self, task: &Task) -> Result<()> {
        let Some(retry) = &task.pending_retry else {
            return Ok(());
        };
        let updated = self
            .store
            .update(
                &task.id,
                &TaskPatch {
                    next_run_at: Some(retry.resume_at),
                    ..Default::default()
                },
            )
            .await?;
        self.bus.publish(Event::TaskUpdated { task: updated });
        Ok(())
    }

    /// One frame of a run's session. `None` once the run has ended and its
    /// session is released -- what is left then is the transcript.
    pub async fn run_screen(&self, run: &Run) -> Result<Option<Screen>> {
        let Some(session) = &run.session else {
            return Ok(None);
        };
        let runtime = self.registry.runtime(&session.runtime)?;
        runtime.screen(session).await
    }

    pub async fn session_status(&self, run: &Run) -> RuntimeStatus {
        let Some(session) = &run.session else {
            return RuntimeStatus::Unknown;
        };
        match self.registry.runtime(&session.runtime) {
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
        match self.registry.runtime(&session.runtime) {
            Ok(rt) => rt.status_report(session).await.unwrap_or(unknown),
            Err(_) => unknown,
        }
    }

    // -- small helpers ------------------------------------------------------

    pub(crate) async fn require(&self, id: &str) -> Result<Task> {
        self.store
            .get(id)
            .await?
            .ok_or_else(|| FactoryError::TaskNotFound(id.to_string()))
    }

    pub(crate) async fn require_run(&self, id: &str) -> Result<Run> {
        self.store
            .get_run(id)
            .await?
            .ok_or_else(|| FactoryError::TaskNotFound(format!("run {id}")))
    }

    pub(crate) async fn publish_task(&self, id: &str) {
        if let Ok(Some(task)) = self.store.get(id).await {
            self.bus.publish(Event::TaskUpdated { task });
        }
    }

    /// `pub(crate)`: `occupancy::record_run_liveness` journals a hook-reported
    /// block or unblock the same way any other daemon-caused change is
    /// journaled here.
    pub(crate) async fn entry(&self, task_id: &str, entry: TaskEntry) {
        if let Err(e) = self.store.append_entry(task_id, &entry).await {
            tracing::warn!(task = task_id, "could not record journal entry: {e}");
        }
        self.bus.publish(Event::TaskEntry {
            id: task_id.to_string(),
            entry,
        });
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let head: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

impl Engine {
    /// The knowledge pages a run of `task` is handed, when the task asks for
    /// them: one search over its title and instructions, from its own scope,
    /// at most `KNOWLEDGE_HINTS_LIMIT` pages. What was handed over is
    /// written to the run's journal, so the prompt can be explained after
    /// the vault has moved on. A search that fails is journaled too and the
    /// run goes ahead without hints -- they are a help, never a reason not
    /// to start.
    async fn knowledge_hints(
        &self,
        task: &Task,
        run_id: &str,
    ) -> Option<factory_core::adapter::KnowledgeHints> {
        if !task.knowledge_hints {
            return None;
        }
        let (root, name) = {
            let factory = self.factory_snapshot();
            (factory.root, factory.config.daemon.knowledge_provider)
        };
        let query = factory_core::adapter::KnowledgeQuery {
            text: format!("{}\n{}", task.title, task.instructions),
            tags: Vec::new(),
            scope: Some(task.scope.clone()),
            limit: factory_core::adapter::knowledge::KNOWLEDGE_HINTS_LIMIT,
        };
        let searched = match self.registry.knowledge(&name) {
            Ok(provider) => provider.search(&root, &query).await,
            Err(e) => Err(e),
        };
        let entry = match &searched {
            Ok(hits) if hits.is_empty() => {
                TaskEntry::new("daemon", "knowledge", format!("no knowledge page matched ({name})"))
            }
            Ok(hits) => TaskEntry::new(
                "daemon",
                "knowledge",
                format!(
                    "handed {} knowledge page(s) ({name}): {}",
                    hits.len(),
                    hits.iter().map(|h| h.page.as_str()).collect::<Vec<_>>().join(", ")
                ),
            )
            .with_data(serde_json::json!({ "provider": name, "hits": hits })),
            Err(e) => TaskEntry::new(
                "daemon",
                "knowledge",
                format!("knowledge search failed, so this run has no hints ({name}): {e}"),
            ),
        };
        self.entry(&task.id, entry.in_run(run_id)).await;
        let hits = searched.ok().filter(|hits| !hits.is_empty())?;
        Some(factory_core::adapter::KnowledgeHints {
            vault: factory_core::knowledge::vault_root(&root).display().to_string(),
            hits,
        })
    }

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
        let result = match self.registry.knowledge(&name) {
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
    pub(crate) async fn fail_task_for_test(self: &Arc<Self>, task_id: &str, kind: FailKind) -> Task {
        let run = self
            .store
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
        self.fail_run(&run.id, kind, "the remediation run failed").await;
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
                // what the tests in *this* file need from `engine.power` is
                // only the bookkeeping (`active_count`), which is identical
                // whichever backend sits behind it.
                power_assertion: false,
                ..DaemonConfig::default()
            },
            roles: Default::default(),
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
                roles: Default::default(),
                policies: Default::default(),
                quality: Default::default(),
                dependencies: Default::default(),
            }],
            infrastructure: Default::default(),
            plugins_dir: None,
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

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("factory-engine-test-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn runtime_diagnostics_group_scopes_and_isolate_a_missing_adapter_as_data() {
        let scope_dir = temp_dir("runtime-diagnostic");
        let engine = test_engine(scope_dir.clone());
        {
            let mut factory = engine.factory.write().unwrap();
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
            let mut factory = engine.factory.write().unwrap();
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
            let mut factory = engine.factory.write().unwrap();
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
            vec![("cli", None), ("http", Some(crate::interfaces::http::DEFAULT_BIND))],
            "the default interfaces, with the address http falls back to"
        );
        assert!(daemon.started_at <= Utc::now());
        assert_eq!(host.arch.as_deref(), Some(std::env::consts::ARCH));

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
            let mut factory = engine.factory.write().unwrap();
            factory.config.scopes[0].agents.push(ScopeAgent {
                name: Some("boxed".into()),
                harness: "shell".into(),
                lifetime: Lifetime::Task,
                role: Role::default(),
                autostart: None,
                args: Vec::new(),
                sandbox: Sandbox::Docker,
                provider: None,
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
            let mut factory = engine.factory.write().unwrap();
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
            let mut factory = engine.factory.write().unwrap();
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
        engine.factory.write().unwrap().root.clone_from(&root);
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

        let hints = engine.knowledge_hints(&task, "run-1").await.expect("something matched");
        assert_eq!(hints.vault, root.join(".factory/knowledge").display().to_string());
        assert_eq!(hints.hits.len(), factory_core::adapter::knowledge::KNOWLEDGE_HINTS_LIMIT);
        assert_eq!(hints.hits[0].page, "acme", "tag matches all score alike, so page id decides");

        let entries = engine.store.entries(&task.id, 50).await.unwrap();
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
        assert!(engine.knowledge_hints(&task, "run-1").await.is_none());
        let entries = engine.store.entries(&task.id, 50).await.unwrap();
        assert!(!entries.iter().any(|e| e.kind == "knowledge"));

        // Turned on by an edit, it takes effect on the next run; a task whose
        // words match nothing says so rather than staying silent.
        let patch = TaskPatch { knowledge_hints: Some(true), title: Some("unrelated".into()), ..Default::default() };
        let task = match engine.handle_request(Request::TaskUpdate { id: task.id.clone(), patch, reason: None }).await {
            Response::Ok { data: Payload::Task { task } } => task,
            other => panic!("expected a task: {other:?}"),
        };
        assert!(task.knowledge_hints);
        assert!(engine.knowledge_hints(&task, "run-2").await.is_none());
        let entries = engine.store.entries(&task.id, 50).await.unwrap();
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
        engine.factory.write().unwrap().config.daemon.knowledge_provider = "gone".into();
        assert!(engine.knowledge_hints(&task, "run-1").await.is_none());
        let entries = engine.store.entries(&task.id, 50).await.unwrap();
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
            .registry
            .add_knowledge(stub.clone(), "test");
        engine.factory.write().unwrap().config.daemon.knowledge_provider = "stub".into();

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
            let mut factory = engine.factory.write().unwrap();
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
            let mut factory = engine.factory.write().unwrap();
            factory.config.scopes[0].agents.push(ScopeAgent {
                name: Some("builder".into()),
                harness: "claude-code".into(),
                lifetime: Lifetime::Task,
                role: Role::default(),
                autostart: None,
                args: vec!["--model".into(), "opus".into(), "--api-key".into(), "s3cret".into()],
                sandbox: Sandbox::None,
                provider: None,
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

        engine.start_run(&task.id, Trigger::Manual).await;

        let runs = engine.store.runs(&task.id, 10).await.unwrap();
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

        let entries = engine.store.run_entries(&run.id, 50).await.unwrap();
        assert!(
            entries.iter().any(|e| e.message.contains("not a git repository")),
            "the journal gets git's complaint too"
        );

        // `start_run`'s own error handling ran this all the way through
        // `fail_run` -> `close_session`, so the assertion `dispatch` took
        // out for this run when its row was made must be back down to zero
        // by now -- see issue #61.
        assert_eq!(
            engine.power.active_count().await,
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

        assert_eq!(engine.power.active_count().await, 0, "nothing has dispatched yet");

        // Not a git repository, so this fails inside `place_run` -- after
        // the run row (and so the acquire) but before a session. Calling
        // `dispatch` directly rather than through `start_run` is what lets
        // this test see the assertion still held: `start_run` would carry
        // the same error straight into `fail_run` and release it again.
        let err = engine.dispatch(&task.id, Trigger::Manual, Due::now()).await.unwrap_err();
        assert!(err.to_string().contains("not a git repository"), "got: {err}");

        assert_eq!(
            engine.power.active_count().await,
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
            .store
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
            .store
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

        engine.fail_run(&run.id, FailKind::AgentFailed, "nobody ever answered").await;

        let failed = engine.store.get_run(&run.id).await.unwrap().unwrap();
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
        let unchanged = engine.store.get(&task.id).await.unwrap().unwrap();
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
            .store
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
            .store
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

        let failed = engine.store.get_run(&run.id).await.unwrap().unwrap();
        assert_eq!(failed.status, RunStatus::Failed, "not left for the timeout to find");
        let error = failed.error.unwrap();
        assert!(error.contains("API error (server_error)"), "{error}");
        assert!(error.contains("I think that is everything."), "{error}");
        let entries = engine.store.entries(&task.id, 50).await.unwrap();
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
        let held = engine.store.get_run(&run.id).await.unwrap().unwrap();
        assert_eq!(held.status, RunStatus::Running, "still running");
        assert!(held.turn_ended_at.is_some());
        let why = held.turn_end_reason.as_deref().unwrap();
        assert!(why.contains("turn ended without reporting") && why.contains("Stop hook"), "{why}");

        engine
            .report(
                &task.id,
                TaskReport {
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
        let resumed = engine.store.get_run(&run.id).await.unwrap().unwrap();
        assert!(resumed.turn_ended_at.is_none(), "the agent talking is a turn that did not end");
        assert!(resumed.turn_end_reason.is_none());
    }

    #[tokio::test]
    async fn a_run_that_ends_takes_its_held_stop_with_it() {
        let engine = test_engine(temp_dir("turn-end-finish"));
        let (task, run) = running_run(&engine, RunStatus::Running).await;
        send_turn_end(&engine, &task.id, stop(Some("tok"), 0)).await;

        engine.fail_run(&run.id, FailKind::AgentFailed, "for some other reason").await;
        let failed = engine.store.get_run(&run.id).await.unwrap().unwrap();
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
            let still = engine.store.get_run(&run.id).await.unwrap().unwrap();
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
            .report(
                &task.id,
                TaskReport {
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
        let before = engine.store.entries(&task.id, 50).await.unwrap().len();

        // Pinned rather than narrated: this is what every successful
        // claude-code run's last Stop hook actually gets back.
        match send_turn_end(&engine, &task.id, stop(Some("tok"), 0)).await {
            Response::Error { code, .. } => assert_eq!(code, "denied"),
            other => panic!("a finished run's token is nobody's: {other:?}"),
        }
        engine.turn_ended(&task.id, stop(Some("tok"), 0)).await.unwrap();

        let done = engine.store.get_run(&run.id).await.unwrap().unwrap();
        assert_eq!(done.status, RunStatus::Done);
        assert!(done.error.is_none());
        assert_eq!(engine.store.entries(&task.id, 50).await.unwrap().len(), before, "nothing journalled");
    }

    #[tokio::test]
    async fn a_blocked_run_or_one_with_background_work_is_left_to_go_on() {
        let engine = test_engine(temp_dir("turn-end-waiting"));

        let (task, run) = running_run(&engine, RunStatus::Blocked).await;
        send_turn_end(&engine, &task.id, stop(Some("tok"), 0)).await;
        let still = engine.store.get_run(&run.id).await.unwrap().unwrap();
        assert_eq!(still.status, RunStatus::Blocked, "it asked for a human and is waiting for one");
        assert!(still.turn_ended_at.is_none(), "and nothing is held against it");

        let (task, run) = running_run(&engine, RunStatus::Running).await;
        send_turn_end(&engine, &task.id, stop(Some("tok"), 2)).await;
        let still = engine.store.get_run(&run.id).await.unwrap().unwrap();
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

        engine.fail_run(&run.id, FailKind::AgentFailed, "the audit script exited 1").await;

        let task = engine.store.get(&task.id).await.unwrap().unwrap();
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

        let entries = engine.store.entries(&task.id, 20).await.unwrap();
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
        engine.fail_run(&first.id, FailKind::AgentFailed, "the audit script exited 1").await;
        let mid = engine.store.get(&task.id).await.unwrap().unwrap();
        assert!(mid.error.is_some(), "sanity: the failure is on the mirror before the retry runs");
        assert!(mid.pending_retry.is_some(), "sanity: a retry is queued before it runs");
        assert_eq!(mid.status, TaskStatus::Pending, "a queued retry keeps the task scheduled (#122)");
        assert!(mid.failure.is_some(), "and it says what it is retrying after");

        // The scheduler's own retry pickup (`resume_from_retry`) would run
        // here in production; a fresh run row is all `finish_run` needs.
        let retry = run_for(&engine, &task.id, Trigger::Retry).await;
        engine
            .finish_run(
                &retry.id,
                RunStatus::Done,
                RunPatch { result: Some("all clear".into()), ..Default::default() },
                "attempt ended",
            )
            .await
            .unwrap();

        let task = engine.store.get(&task.id).await.unwrap().unwrap();
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
        engine.fail_run(&first.id, FailKind::AgentFailed, "attempt 1 failed").await;
        let mid = engine.store.get(&task.id).await.unwrap().unwrap();
        assert_eq!(mid.pending_retry.map(|p| p.attempts), Some(1), "the one allowed retry is queued");

        let retry = run_for(&engine, &task.id, Trigger::Retry).await;
        engine.fail_run(&retry.id, FailKind::AgentFailed, "attempt 2 failed too").await;

        let task = engine.store.get(&task.id).await.unwrap().unwrap();
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

        let entries = engine.store.entries(&task.id, 20).await.unwrap();
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
        engine.fail_run(&run.id, FailKind::AgentFailed, "boom").await;

        let task = engine.store.get(&task.id).await.unwrap().unwrap();
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
        let entries = engine.store.entries(&task.id, 20).await.unwrap();
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
            .store
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

        engine.resume_from_retry(&task).await.unwrap();

        let task = engine.store.get(&task.id).await.unwrap().unwrap();
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
            .store
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

        let (cwd, run) = engine.place_run(&task, run, &scope_dir).await.unwrap();
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

        engine.worktree_caps.lock().unwrap().clear();
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
            .store
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

        let (cwd, run) = engine.place_run(&task, run, &scope_dir).await.unwrap();
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
        engine.fail_run(&run.id, FailKind::RunTimeout, "ran for longer than 60s").await;
        let failed = engine.store.get_run(&run.id).await.unwrap().unwrap();
        assert_eq!(failed.fail_kind, Some(FailKind::RunTimeout));
        assert_eq!(failed.error.as_deref(), Some("ran for longer than 60s"), "the prose stays beside it");

        let run = run_for(&engine, &task.id, Trigger::Manual).await;
        engine
            .report(
                &task.id,
                TaskReport {
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
        let reported = engine.store.get_run(&run.id).await.unwrap().unwrap();
        assert_eq!(reported.fail_kind, Some(FailKind::AgentFailed));

        let run = run_for(&engine, &task.id, Trigger::Manual).await;
        let response = engine.handle_request(Request::TaskCancel { id: task.id.clone(), reason: None, run: None }).await;
        assert!(matches!(response, Response::Ok { .. }), "{response:?}");
        let cancelled = engine.store.get_run(&run.id).await.unwrap().unwrap();
        assert_eq!(cancelled.status, RunStatus::Cancelled);
        assert_eq!(
            cancelled.fail_kind,
            Some(FailKind::CancelledByPerson),
            "a cancel that came in with no token came in as the owner"
        );

        let done = run_for(&engine, &task.id, Trigger::Manual).await;
        engine
            .report(
                &task.id,
                TaskReport {
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
        let done = engine.store.get_run(&done.id).await.unwrap().unwrap();
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

        engine.start_run_due(&task.id, Trigger::Schedule, Due::slot(slot)).await;

        let run = engine.store.runs(&task.id, 1).await.unwrap().remove(0);
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
            .store
            .update(&task.id, &TaskPatch { next_run_at: Some(slot), ..Default::default() })
            .await
            .unwrap();

        engine.advance_schedule(&task).await.unwrap();

        let entries = engine.store.entries(&task.id, 20).await.unwrap();
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
        let task = engine.store.get(&task.id).await.unwrap().unwrap();
        let on_time = engine
            .store
            .update(&task.id, &TaskPatch { next_run_at: Some(Utc::now()), ..Default::default() })
            .await
            .unwrap();
        engine.advance_schedule(&on_time).await.unwrap();
        let entries = engine.store.entries(&task.id, 20).await.unwrap();
        assert_eq!(entries.iter().filter(|e| e.kind == "schedule_skipped").count(), 1);

        std::fs::remove_dir_all(&scope_dir).ok();
    }

    #[test]
    fn a_skip_is_blamed_on_the_previous_run_only_if_it_was_open_at_the_first_skipped_slot() {
        let t0 = Utc::now();
        let mut run = Run {
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
            token: None,
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

    #[tokio::test]
    async fn a_paused_schedule_is_kept_skipped_by_due_and_resumes_from_now() {
        let scope_dir = temp_dir("pause");
        let engine = test_engine(scope_dir.clone());
        let task = weekly_task(&engine, None).await;
        let schedule = task.schedule.clone();

        // Mid-streak, and overdue: the case a pause has to hold back.
        let run = run_for(&engine, &task.id, Trigger::Schedule).await;
        engine.fail_run(&run.id, FailKind::AgentFailed, "exit 1").await;
        let long_ago = Utc::now() - chrono::Duration::hours(2);
        engine
            .store
            .update(&task.id, &TaskPatch { next_run_at: Some(long_ago), ..Default::default() })
            .await
            .unwrap();
        assert!(engine.due_now().await.unwrap().iter().any(|t| t.id == task.id), "sanity: due before the pause");

        let paused = engine
            .update(&task.id, TaskPatch { schedule_paused: Some(true), ..Default::default() }, None)
            .await
            .unwrap();
        assert!(paused.schedule_paused);
        assert_eq!(paused.schedule, schedule, "a paused task keeps its schedule");
        assert!(!engine.due_now().await.unwrap().iter().any(|t| t.id == task.id), "nothing fires while paused");

        let resumed = engine
            .update(&task.id, TaskPatch { schedule_paused: Some(false), ..Default::default() }, None)
            .await
            .unwrap();
        assert!(!resumed.schedule_paused);
        assert!(resumed.next_run_at.unwrap() > Utc::now(), "recomputed from now: no catch-up firing");
        assert!(resumed.pending_retry.is_none(), "the interrupted retry streak is over");
        assert!(!engine.due_now().await.unwrap().iter().any(|t| t.id == task.id));

        let kinds: Vec<String> = engine.store.entries(&task.id, 50).await.unwrap().into_iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&"schedule_paused".to_string()), "{kinds:?}");
        assert!(kinds.contains(&"schedule_resumed".to_string()), "{kinds:?}");

        // Saying it again changes nothing and journals nothing.
        engine
            .update(&task.id, TaskPatch { schedule_paused: Some(false), ..Default::default() }, None)
            .await
            .unwrap();
        let resumes = engine
            .store
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
        let kinds: Vec<String> = engine.store.entries(&task.id, 50).await.unwrap().into_iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&"schedule_pause_cleared".to_string()), "{kinds:?}");

        let rescheduled = engine
            .update(&task.id, TaskPatch { schedule: Some(Schedule::Every { seconds: 60 }), ..Default::default() }, None)
            .await
            .unwrap();
        assert!(!rescheduled.schedule_paused);
        // Due once its first slot comes round.
        engine
            .store
            .update(&task.id, &TaskPatch { next_run_at: Some(Utc::now() - chrono::Duration::seconds(1)), ..Default::default() })
            .await
            .unwrap();
        assert!(engine.due_now().await.unwrap().iter().any(|t| t.id == task.id), "the new schedule fires");

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
            token: None,
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
            engine.fail_run(&run.id, kind, "it went wrong").await;

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
            .store
            .update(&task.id, &TaskPatch { agent: Some("no-such-agent".into()), ..Default::default() })
            .await
            .unwrap();

        engine.start_run_due(&task.id, Trigger::Manual, Due::now()).await;

        let task = engine.require(&task.id).await.unwrap();
        assert!(engine.store.runs(&task.id, 5).await.unwrap().is_empty(), "sanity: no run was made");
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
        engine.fail_run(&first.id, FailKind::AgentFailed, "boom").await;

        let second = run_for(&engine, &task.id, Trigger::Manual).await;
        let mid = engine.require(&task.id).await.unwrap();
        assert_eq!(mid.status, TaskStatus::Dispatching);
        assert!(mid.failure.is_none(), "a new attempt is newer than the failure it follows");

        engine
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
            .store
            .update(&task.id, &TaskPatch { blocked_timeout_seconds: Some(1), ..Default::default() })
            .await
            .unwrap();
        let run = run_for(&engine, &task.id, Trigger::Manual).await;
        let long_ago = Utc::now() - chrono::Duration::hours(1);
        engine
            .store
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
        engine.fail_run(&run.id, FailKind::BlockedTimeout, "nobody answered").await;

        let task = engine.require(&task.id).await.unwrap();
        assert!(task.blocked_by_failure());
        assert_eq!(task.failure.as_ref().and_then(|f| f.kind), Some(FailKind::BlockedTimeout));
        let active = engine.active_runs().await.unwrap();
        assert!(
            active.iter().all(|r| r.task_id != task.id),
            "no blocked run stands in for the failure, so nothing is left for the timeout to find: {active:?}"
        );
        assert_eq!(engine.store.runs(&task.id, 10).await.unwrap().len(), 1, "and no new run was made to show it");
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

        let entries = engine.store.entries(&task.id, 20).await.unwrap();
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
        engine.fail_run(&run.id, FailKind::AgentFailed, "boom").await;

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
        let entries = engine.store.entries(&task.id, 20).await.unwrap();
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
        engine.fail_run(&run.id, FailKind::AgentFailed, "boom").await;
        task_of(engine.handle_request(close(&task.id, factory_core::task::CloseReason::NotPlanned)).await);

        let reopened = task_of(
            engine.handle_request(Request::TaskReopen { id: task.id.clone(), reason: Some("worth another go".into()) }).await,
        );
        assert_eq!(reopened.status, TaskStatus::Pending);
        assert!(reopened.closure.is_none());
        assert!(reopened.failure.is_none(), "a pending task must not read as one mid-retry");
        assert_eq!(reopened.close_reason(), None);
        let entries = engine.store.entries(&task.id, 20).await.unwrap();
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
        engine.fail_run(&run.id, FailKind::AgentFailed, "boom").await;
        let closed = weekly_task(&engine, None).await;
        task_of(engine.handle_request(close(&closed.id, factory_core::task::CloseReason::NotPlanned)).await);
        let asking = weekly_task(&engine, None).await;
        let run = run_for(&engine, &asking.id, Trigger::Schedule).await;
        engine
            .store
            .update_run(&run.id, &RunPatch { status: Some(RunStatus::Blocked), ..Default::default() })
            .await
            .unwrap();
        let asking_now = engine.require(&asking.id).await.unwrap();
        engine.mirror_to_task(&engine.require_run(&run.id).await.unwrap()).await;
        assert_eq!(engine.require(&asking.id).await.unwrap().status, TaskStatus::Blocked, "sanity");
        let _ = asking_now;

        let past = Utc::now() - chrono::Duration::minutes(1);
        for id in [&blocked.id, &closed.id, &asking.id] {
            engine.store.update(id, &TaskPatch { next_run_at: Some(past), ..Default::default() }).await.unwrap();
        }

        let due: Vec<String> = engine.due_now().await.unwrap().into_iter().map(|t| t.id).collect();
        assert_eq!(due, vec![blocked.id.clone()], "the failure keeps firing; the close and the question do not");

        // Closed between `due_now` and the scheduler's locked re-read: not fired.
        let seen = engine.require(&blocked.id).await.unwrap();
        assert!(engine.still_due(&seen).await.is_some());
        task_of(engine.handle_request(close(&blocked.id, factory_core::task::CloseReason::NotPlanned)).await);
        assert!(engine.still_due(&seen).await.is_none(), "closed in between, so nothing fires it");
        std::fs::remove_dir_all(&scope_dir).ok();
    }

    #[tokio::test]
    async fn reopening_a_scheduled_task_picks_its_schedule_up_from_now() {
        let scope_dir = temp_dir("reopen-scheduled");
        let engine = test_engine(scope_dir.clone());
        let task = weekly_task(&engine, None).await;
        task_of(engine.handle_request(close(&task.id, factory_core::task::CloseReason::NotPlanned)).await);
        let long_ago = Utc::now() - chrono::Duration::days(30);
        engine.store.update(&task.id, &TaskPatch { next_run_at: Some(long_ago), ..Default::default() }).await.unwrap();

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
            .store
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
        engine.store.update(&old.id, &TaskPatch { status: Some(TaskStatus::Failed), ..Default::default() }).await.unwrap();
        // One that never got a run, scheduled, its slot long past.
        let never = weekly_task(&engine, None).await;
        let long_ago = Utc::now() - chrono::Duration::days(30);
        engine
            .store
            .update(&never.id, &TaskPatch { status: Some(TaskStatus::Failed), next_run_at: Some(long_ago), ..Default::default() })
            .await
            .unwrap();
        let fine = one_off_task(&engine, "untouched").await;

        assert_eq!(engine.migrate_failed_tasks().await, 2);

        let old = engine.require(&old.id).await.unwrap();
        assert!(old.blocked_by_failure());
        assert_eq!(old.failure.as_ref().and_then(|f| f.kind), Some(FailKind::RunTimeout));
        assert_eq!(old.failure.as_ref().and_then(|f| f.run_id.clone()), Some(run.id.clone()));
        let entries = engine.store.entries(&old.id, 20).await.unwrap();
        assert!(entries.iter().any(|e| e.kind == "migrated"), "{entries:?}");

        let never = engine.require(&never.id).await.unwrap();
        assert!(never.blocked_by_failure());
        assert_eq!(never.failure.as_ref().and_then(|f| f.kind), Some(FailKind::DispatchFailed));
        assert!(never.next_run_at.unwrap() > Utc::now(), "a migrated schedule does not fire a burst");

        assert_eq!(engine.require(&fine.id).await.unwrap().status, TaskStatus::Pending);
        assert_eq!(engine.migrate_failed_tasks().await, 0, "nothing writes failed any more, so a second start finds none");
        std::fs::remove_dir_all(&scope_dir).ok();
    }
}
