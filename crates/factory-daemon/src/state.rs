//! `Engine`'s state, grouped by the level that owns it (#193 phase 6, slice 1).
//!
//! A pure regrouping: every field is the same type with the same meaning as before. A group is a
//! place for state, not a service: the methods still live on `Engine`, and moving them is later work.
//! `Shared` is what no level owns: the live configuration snapshot, the adapter registry, the
//! observer bus and the page-side caches.

use crate::engine::CapacityEvent;
use crate::worktree;
use chrono::Utc;
use factory_core::adapter::runtime::RuntimeStatus;
use factory_core::adapter::TaskStore;
use factory_core::config::Factory;
use factory_core::event::EventBus;
use factory_plugins::registry::Registry;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// State no level owns.
pub struct Shared {
    /// The instance settings are stable, while a successful scope-config edit
    /// replaces the affected scope in this snapshot. Readers clone it before
    /// awaiting so no filesystem or runtime operation holds the lock.
    pub(crate) factory: std::sync::RwLock<Factory>,
    /// Serializes read-modify-write edits to local scope config files.
    pub(crate) configuration_edit: std::sync::Mutex<()>,
    pub(crate) registry: Registry,
    pub(crate) bus: EventBus,
    pub(crate) factory_bin: PathBuf,
    pub(crate) started: Instant,
    /// The wall-clock moment this daemon came up. A slot that fell before it
    /// could not have been dispatched before it -- see `Engine::due_for`.
    pub(crate) booted_at: chrono::DateTime<Utc>,
    pub(crate) interfaces: Vec<String>,
    /// The last walk of each scope's directory, with when it was taken. The
    /// site view asks for a scope's size on every run and agent event now, and
    /// a repository does not change size between two of them -- see
    /// `site::WALK_TTL`.
    pub(crate) site_walks:
        std::sync::Mutex<std::collections::HashMap<String, (Instant, crate::site::Measured)>>,
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
}

/// L1 Infrastructure: backup, running environments, renewal behaviour, host power.
pub struct L1State {
    /// What happened to backups -- see `backup::BackupStore`. The archives
    /// themselves are the destination's, listed fresh on every request.
    pub(crate) backups: crate::backup::BackupStore,
    /// Held for the whole of a backup, verification or restore, so the job and a
    /// person can never run two at once over one destination. Taken with
    /// `try_lock`: a second request is refused, never queued.
    pub(crate) backup_busy: tokio::sync::Mutex<()>,
    /// Deployments, releases and health samples -- see
    /// `environments::EnvironmentStore` (`#185`).
    pub(crate) environments: crate::environments::EnvironmentStore,
    /// Deployment transitions are read-modify-write operations. Keep starts,
    /// supersession and finishes ordered, including post-deploy verification.
    pub(crate) deployment_edit: tokio::sync::Mutex<()>,
    /// Serializes approved remote effects without holding a deployment/health lock.
    pub(crate) deployment_mirror_busy: tokio::sync::Mutex<()>,
    /// `#156`: the due slot and reason a verification drill last skipped for
    /// (an encrypted newest snapshot with no identity), so the job logs it
    /// once per slot rather than on every tick -- the same skip can recur
    /// for as long as the newest snapshot stays encrypted and unverified.
    /// Lost on restart, like `seen_status`: the first tick after one is
    /// genuinely new information. Never read by `backup_report`'s own
    /// `verify_skipped`, which is a plain projection of the live facts
    /// instead -- see `backup::report`'s own comment.
    pub(crate) verify_drill_skip: std::sync::Mutex<Option<(chrono::DateTime<Utc>, String)>>,
    /// Keeps the host awake for as long as any run is active -- see
    /// `crate::power` and issue #61. Acquired once a run's row exists
    /// (`dispatch`), released for every run `close_session` ever sees,
    /// terminal outcome or not.
    pub(crate) power: crate::power::PowerAssertions,
    /// The host's macOS power mode -- see `crate::host_power` and issue
    /// #260. No runner at all in tests or off macOS.
    pub(crate) host_power: crate::host_power::HostPower,
    pub(crate) infrastructure_expiries: crate::renewals::store::InfrastructureExpiryStore,
    pub(crate) renewal_declaration_cache: factory_infrastructure::renewal_declarations::DeclarationCache,
    pub(crate) promotion_lock: tokio::sync::Mutex<()>,
}

/// L2 Environment: sandbox provisioning and credential expiry.
pub struct L2State {
    /// `sandbox: openshell` prerequisites, kept in the background (`#234`).
    pub(crate) provision: crate::provision::Provisioner,
    pub(crate) credential_expiries: crate::renewals::store::CredentialExpiryStore,
}

/// L3 Agent: harness health.
pub struct L3State {
    /// Whether each harness binary starts, probed before a dispatch and
    /// cached -- see `crate::harness_health` and issue #131.
    pub(crate) harness: crate::harness_health::HarnessHealth,
}

/// L4 Process: tasks, runs, workflows, dispatch admission and scheduling.
pub struct L4State {
    pub(crate) store: Arc<dyn TaskStore>,
    pub(crate) workflows: crate::workflows::WorkflowStore,
    pub(crate) workflow_edit: tokio::sync::Mutex<()>,
    /// L4 owns run-step evidence and immutable artifact provenance separately
    /// from L6's policy receipts. Both stores use the existing instance database.
    pub(crate) run_evidence: factory_process::evidence_store::RunEvidenceStore,
    /// Serializes a run's usage read-modify-write -- append a snapshot, read
    /// them all back, write the derived `usage` -- so two snapshots landing
    /// together never leave the older sum on the run (`costs.rs`).
    pub(crate) usage_edit: tokio::sync::Mutex<()>,
    /// Turn-ended reads begin in chronological order but runtime calls may
    /// finish out of order. Keep their observation times until each call has
    /// settled so only the earliest outstanding turn can journal the one
    /// first-turn re-estimate (`costs.rs`).
    pub(crate) turn_usage_pending: std::sync::Mutex<
        std::collections::HashMap<String, std::collections::BTreeSet<chrono::DateTime<Utc>>>,
    >,
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
    /// The last liveness we wrote down for each session, so a poll that finds
    /// no change writes nothing. Lost on restart, which is right: after a
    /// restart the first observation is genuinely new information.
    pub(crate) seen_status: std::sync::Mutex<std::collections::HashMap<String, RuntimeStatus>>,
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
    /// Serializes the two things that move a schedule's next slot on a
    /// person's or the clock's account: the scheduler firing a due task, and
    /// `task.skip_next`. Each re-reads the task under it, so a slot is
    /// either fired or skipped, never both (`#106`).
    pub(crate) schedule_lock: tokio::sync::Mutex<()>,
    /// Serializes a `max_sessions` admission decision with the run it gates
    /// (`#179`): read the live counts, decide, `create_run` -- all under this
    /// one lock, so two dispatches racing for the last slot cannot both take
    /// it. Held only across that read-decide-write; never across the slower,
    /// fallible steps dispatch takes afterward (the worktree, the harness's
    /// own launch).
    pub(crate) admission_lock: tokio::sync::Mutex<()>,
    pub(crate) workspaces: worktree::Owner,
    pub(crate) run_lifecycle_locks: std::sync::Mutex<std::collections::HashMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>>,
    /// Serializes an intake receipt's identity lookup with its create
    /// (`#167`): whether an item with the same `(kind, provider, reference)`
    /// already exists, and minting a new one if not, all under one lock --
    /// the same shape `admission_lock` uses for a dispatch's read-decide-write.
    /// GitHub's poller and a relayed email/chat's `intake_add` both go
    /// through `Engine::receive_intake`, so the rule lives in one place.
    pub(crate) intake_receipt_lock: tokio::sync::Mutex<()>,
    /// A run ending, or the scheduler tick, sends a [`CapacityEvent`] here so
    /// one worker admits waiting tasks one event at a time -- without the
    /// `&self` sites that notice a run end (`report`, `cancel_task_run`)
    /// needing an `Arc<Self>` of their own, the same shape
    /// `verify_tx`/`spawn_verifier` already use for exactly that reason, and
    /// without a release and the tick's own sweep ever running at once: both
    /// read `waiting_tasks()` and admit from it, and `dispatch` has no
    /// existing-run guard of its own to fall back on if two admission
    /// attempts for the same waiting task overlapped.
    pub(crate) capacity_release_tx: tokio::sync::mpsc::UnboundedSender<CapacityEvent>,
    pub(crate) capacity_release_rx: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<CapacityEvent>>>,
}

/// L5 Improvement: benchmarks, datasets, suggestions, quality and signposts.
pub struct L5State {
    pub(crate) bench: crate::bench::BenchStore,
    /// `#275`: filed suggestions, append-only -- see
    /// `crate::suggestions`/`factory_assurance::suggestion_store`.
    pub(crate) suggestions: crate::suggestions::SuggestionStore,
    /// Serializes a bench run's own read-modify-write: choosing which
    /// pending attempts to start, and recomputing the run's own status once
    /// every attempt has settled. Coarse -- one lock for every run, the same
    /// trade `workflow_edit` already makes -- rather than one per run.
    pub(crate) bench_edit: tokio::sync::Mutex<()>,
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
    /// L5's signpost fact provider owns its short cache.
    pub(crate) signpost_cache: factory_assurance::signposts::Cache,
}

/// L6 Direction: policy receipts, goals and renewal push receipts.
pub struct L6State {
    /// The attestations audit trail -- see `policies::PolicyStore`. Nothing
    /// else in a policy request is stateful: the catalogues are read fresh
    /// off disk on every call, like `.factory/knowledge/` and
    /// `.factory/datasets/`.
    pub(crate) policies: crate::policies::PolicyStore,
    /// The check-ins audit trail -- see `goals::GoalsStore`. Nothing else in
    /// a goals request is stateful: the direction and cycle catalogues are
    /// read fresh off disk on every call, like the policy catalogues.
    pub(crate) goals: crate::goals::GoalsStore,
    pub(crate) renewal_alerts: crate::renewals::store::AlertStore,
}
