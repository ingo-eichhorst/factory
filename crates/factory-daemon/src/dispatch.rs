//! Station 11's cron dispatcher — ADR 0021 decisions 2, 3, 3a, 10: a thread
//! the daemon owns, waking about once a minute, that turns a due
//! [`factory_task::schedule::Schedule`] into a run. `factory_daemon::observe`
//! is the model this follows almost line for line — "the dispatcher is the
//! same shape with a different period" (decision 2) — down to the spawned
//! thread, the stop channel, and the first pass running before the first
//! wait.
//!
//! # The tick
//!
//! [`tick`] does four things for the instant it is given:
//!
//! 1. Ask [`factory_task::schedule::due`] which enabled schedules match this
//!    minute. This module never parses cron itself — decision 3a puts the
//!    one home of that predicate in `factory_task::schedule`, and the
//!    `factory schedule list` preview and this dispatcher must never
//!    disagree about what "due" means.
//! 2. For each due schedule whose template is `open` (see "A paused or
//!    closed template is skipped, not failed" below), call
//!    [`factory_task::create::create_from_schedule`], which inserts the run
//!    already carrying `schedule_id` / `fired_for_minute` /
//!    `triggered_by = 'cron'`, appends its `created` event, and stamps
//!    `schedules.last_fired_at` — all in one transaction. See "One
//!    transaction, and why it had to move" below.
//! 3. For a run that was actually created (never for `AlreadyFired`, and
//!    only when the template names a target agent), attempt assignment and
//!    delivery — see "Attempting delivery after a fire" below. This can
//!    never turn a created run into a reported failure: once step 2 has
//!    committed the row, this tick's [`FireOutcome`] for that schedule stays
//!    [`FireOutcome::Fired`] no matter what step 3 does or does not manage
//!    to deliver.
//! 4. Record the tick in `dispatcher_state`, unconditionally — even a tick
//!    that found nothing due, and even one where every fire failed. That
//!    table answers one question only, "has the dispatcher run at all
//!    lately," and a schedule's own failure must not make the dispatcher
//!    itself look silent.
//!
//! `schedules.last_fired_at` is never read as a decision input here.
//! Whether a schedule is due comes from [`factory_task::schedule::due`]'s
//! cron match alone — the unique index is what prevents a double run, and
//! using a timestamp instead would reopen the restart hole decision 3
//! exists to close.
//!
//! # A unique-constraint violation is "already fired," not a fault
//!
//! On the autumn clock change a repeated local minute matches
//! [`factory_task::schedule::due`] twice, an hour apart, so this dispatcher
//! genuinely tries to fire the same schedule twice for the same rendered
//! `fired_for_minute` string. `factory_task::create::create_from_schedule`
//! answers [`factory_task::create::ScheduleFire::AlreadyFired`] for that,
//! and this module reports it as an ordinary outcome rather than a fault.
//!
//! Telling that violation apart from every other way the INSERT can fail
//! lives in `factory_task`, beside the INSERT itself, because that is the
//! only place that can match the constraint by name. This crate could only
//! have compared error text, and the same INSERT produces text of the same
//! shape for a CHECK failure and for a foreign-key failure — both real
//! faults that must never be swallowed as an ordinary duplicate.
//!
//! # One transaction, and why it had to move
//!
//! ADR 0021 decision 3 wants the run insert and `schedules.last_fired_at`
//! written in **one** transaction. This module cannot be that transaction.
//! `factory_task::create::create_from_template` opens and commits its own
//! (`Store::transaction` holds `&mut Store` for the `Transaction`'s whole
//! lifetime, so a caller already holding one open transaction cannot pass
//! the same `&mut Store` into a function that wants a second), and
//! `factory_task::events::append` — that crate's one place that INSERTs into
//! `task_events` — is `pub(crate)`, unreachable from here. Writing `tasks`
//! or `task_events` rows directly from this crate would be the second-home
//! problem decision 3 already names for `last_fired_at`, one layer further
//! in.
//!
//! An earlier version of this module assembled the fire from a committing
//! create plus a second tagging transaction. That leaves a real hole: a
//! crash between the two commits strands a run with `triggered_by =
//! 'manual'` and no `schedule_id`, which the unique index never sees and no
//! later tick can find — so the schedule fires again for the same minute,
//! defeating the one criterion the index exists to hold. A pre-check read
//! cannot close it either, because the orphan carries nothing to find it by.
//!
//! So the whole fire moved into `factory_task`, where one transaction can
//! cover all three writes. This module chooses *which* schedules fire and
//! reports what happened; it no longer assembles the write.
//!
//! # A paused or closed template is skipped, not failed
//!
//! `factory schedule create --template` already refuses a template whose
//! `state` is not `open` (ADR 0021 decision 11), but until this defect was
//! closed [`try_fire`] read only the template's scope and prompt and never
//! its `state` — so a schedule created while its template was `open` kept
//! firing every minute after an operator paused or closed it. "Paused" meant
//! one thing at creation and nothing afterwards; the gate and the dispatcher
//! must agree.
//!
//! [`try_fire`] now checks `state` before ever calling
//! `create_from_schedule`, and reports [`FireOutcome::TemplateNotOpen`] when
//! it is not `open`. This is deliberately not [`FireOutcome::Failed`]: the
//! operator asked for this, by pausing or closing the template, so
//! `log_failed_fires` stays silent for it exactly as it does for
//! [`FireOutcome::AlreadyFired`].
//!
//! # Attempting delivery after a fire
//!
//! Design §11: "[a] minute-level dispatcher creates idempotent cron runs and
//! attempts delivery only to an assigned idle agent; otherwise work stays
//! queued for central assignment." Before this defect was closed, `fire`
//! stopped at "creates" — nothing in this crate ever called
//! [`factory_task::assign::assign`] or [`factory_task::deliver::deliver`]
//! for a cron run, so every one sat `queued` forever.
//!
//! [`attempt_delivery`] is the fix, called once per newly `Fired` run, only
//! when `template.target_agent_name` is `Some` — a template with no named
//! agent is design §11's "run [that] remains queued for the central agent to
//! assign," and this dispatcher does not choose an agent on its own. It
//! mirrors `ops::task::send`'s own sequence exactly, because that handler is
//! this workspace's one other caller of `assign`/`deliver` and its rules are
//! not this module's to re-derive:
//!
//! 1. Resolve `max_sessions` from the instance configuration, the same way
//!    `ops::task::send` resolves it for its own `Some(agent_name)` branch —
//!    the branch this always is, since delivery is only attempted when
//!    `template.target_agent_name` is `Some`. A config that cannot be
//!    loaded, or an agent it no longer names (a template and the config can
//!    drift independently), is handled exactly like
//!    [`Assignment::Deferred`] below: silently, the run stays `queued`,
//!    nothing to report.
//! 2. [`factory_task::assign::assign`] against the run's untargeted branch —
//!    a cron run never carries `target_session_id` or
//!    `target_workspace_path` (`create_from_schedule` always passes `None`
//!    for both), so `assign` always takes the `assign_untargeted` path.
//!    Verified directly against `factory_task::assign`'s source: that path
//!    does not read `max_sessions` today (only `assign_requested_workspace`
//!    does, and a cron run never carries a workspace path either) — but that
//!    module's own doc comment names starting a session "within
//!    `max_sessions`" as exactly what a future widening of the untargeted
//!    branch would add, so the real, configured value is resolved in step 1
//!    rather than a placeholder that would silently disagree with the
//!    manual path the day that happens. [`Assignment::Deferred`] (no idle
//!    session, a requested session busy, an agent at capacity, …) and
//!    [`Assignment::StartSessionAt`] are both ordinary here — the run stays
//!    `queued`, and this dispatcher never starts a session itself.
//! 3. On [`Assignment::Assigned`], [`factory_task::deliver::deliver`] through
//!    the same [`crate::handler::deliver::AdapterPromptWriter`]
//!    `ops::task::send` uses — the one place `Adapter::send`'s refusal
//!    (ADR 0021 decision 4a: a refusal is not a delivery) is turned into a
//!    journalled, non-terminal outcome, reused here rather than
//!    reimplemented. `deliver` itself journals whichever of `sent`, `failed`,
//!    or `refused` happened, in its own transaction, before this function
//!    ever sees the result — so on any `Err` there is nothing further for
//!    this dispatcher to log without repeating what the event log already
//!    says.
//! 4. On success, [`factory_task::deliver::mark_running`], then a cost
//!    sample taken and recorded exactly where `ops::task::send` takes and
//!    records its own baseline: after `mark_running` has committed, in its
//!    own step, so a failure sampling cost can never undo a delivery that
//!    already happened (ADR 0021 decision 6). `ops::task::sample_cost_best_effort`
//!    is `private` to that module (owned by another agent on this station);
//!    this calls `Adapter::cost_sample` directly rather than duplicating that
//!    helper's body — see this task's own report.
//!
//! Every step here is best-effort with respect to the *tick*: a genuine
//! error partway through (a store error, a broken pane lookup) is logged to
//! stderr and this function simply returns, exactly as one schedule's own
//! `Failed` outcome already does not stop the schedules after it. The run
//! [`try_fire`] just created already exists and already stays `queued` if it
//! was not delivered — nothing here can lose it or mark it `failed`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};

use factory_task::schedule::Schedule;
use factory_task::template::TemplateState;

/// Everything [`fire`] and its helpers can fail with, before it is folded
/// into [`FireOutcome::Failed`]. A thin wrapper over the three crates this
/// module reads from, in the style of `factory_task`'s own per-module error
/// enums (`ScheduleError`, `TemplateError`) — this one has no callers
/// outside this file, so it stays private.
#[derive(Debug, thiserror::Error)]
enum DispatchError {
    #[error("store error: {0}")]
    Store(#[from] factory_store::StoreError),
    #[error("schedule error: {0}")]
    Schedule(#[from] factory_task::schedule::ScheduleError),
    #[error("template error: {0}")]
    Template(#[from] factory_task::template::TemplateError),
    #[error("task error: {0}")]
    Task(#[from] factory_task::TaskError),
}

/// What happened when [`tick`] tried to fire one due schedule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FireOutcome {
    /// A new run was created with its cron columns already set, its
    /// `created` event appended, and `schedules.last_fired_at` stamped —
    /// all in one transaction.
    Fired { task_id: uuid::Uuid },
    /// A run for this schedule and this local minute already exists — the
    /// ordinary autumn-clock-change outcome ADR 0021 decision 3a names, not
    /// a fault. Reported by
    /// [`factory_task::create::ScheduleFire::AlreadyFired`], which reads the
    /// unique index's own constraint violation; nothing was written.
    AlreadyFired,
    /// The schedule's template is `paused` or `closed`. See this module's
    /// doc comment, "A paused or closed template is skipped, not failed" —
    /// no run was created, and this is deliberately not [`FireOutcome::Failed`]:
    /// the operator asked for this by taking the template out of service
    /// (ADR 0021 decision 11).
    TemplateNotOpen { state: TemplateState },
    /// Anything else that stopped this one schedule from firing. Never
    /// aborts the tick — the schedules after this one are still attempted.
    Failed(String),
}

/// One schedule's outcome from one [`tick`] pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleReport {
    pub schedule_id: uuid::Uuid,
    pub outcome: FireOutcome,
}

/// One dispatcher pass for the instant `instant` falls in. See this
/// module's doc comment for the four things it does.
///
/// `adapter` is what a newly fired run is delivered through, and
/// `instance_root` is where its `max_sessions` is resolved from — see
/// "Attempting delivery after a fire" in this module's doc comment. Never
/// panics, and one schedule's failure never stops the rest — the same
/// stance `observe::reconcile_once` takes for one session's failure, for the
/// same reason: a bad row or a transient store error must not turn into a
/// silent stall for every schedule after it.
pub fn tick(
    store: &mut factory_store::Store,
    adapter: &(dyn factory_adapter::Adapter + Send + Sync),
    instance_root: &std::path::Path,
    instant: DateTime<Utc>,
) -> Vec<ScheduleReport> {
    // A store error here is reported, not swallowed. An earlier version used
    // `unwrap_or_default()`, which turned "the database could not be read"
    // into "no schedule was due" — indistinguishable, in the log and in
    // `dispatcher_state`, from a quiet minute.
    let due = match factory_task::schedule::due(store, instant) {
        Ok(due) => due,
        Err(e) => {
            record_tick(store, instant);
            return vec![ScheduleReport {
                schedule_id: uuid::Uuid::nil(),
                outcome: FireOutcome::Failed(format!("could not read schedules: {e}")),
            }];
        }
    };

    let mut reports = Vec::with_capacity(due.schedules.len() + due.unreadable.len());

    // A schedule whose `cron` or `timezone` cannot be read at all is named,
    // not dropped. `schedule::create` validates both, so such a row can only
    // arrive by a hand edit or a foreign restore, and an operator who did
    // that needs to be told which row it was.
    for bad in due.unreadable {
        reports.push(ScheduleReport {
            schedule_id: bad.id,
            outcome: FireOutcome::Failed(bad.reason),
        });
    }

    for schedule in due.schedules {
        let outcome = fire(store, adapter, instance_root, &schedule, instant);
        reports.push(ScheduleReport {
            schedule_id: schedule.id,
            outcome,
        });
    }

    record_tick(store, instant);

    reports
}

/// Try to fire one due schedule; [`try_fire`] does the work, this just folds
/// any [`DispatchError`] into [`FireOutcome::Failed`] so a caller never sees
/// this module's internal error type.
fn fire(
    store: &mut factory_store::Store,
    adapter: &(dyn factory_adapter::Adapter + Send + Sync),
    instance_root: &std::path::Path,
    schedule: &Schedule,
    instant: DateTime<Utc>,
) -> FireOutcome {
    match try_fire(store, adapter, instance_root, schedule, instant) {
        Ok(outcome) => outcome,
        Err(e) => FireOutcome::Failed(e.to_string()),
    }
}

fn try_fire(
    store: &mut factory_store::Store,
    adapter: &(dyn factory_adapter::Adapter + Send + Sync),
    instance_root: &std::path::Path,
    schedule: &Schedule,
    instant: DateTime<Utc>,
) -> Result<FireOutcome, DispatchError> {
    let local_minute = factory_task::schedule::local_minute_string(&schedule.timezone, instant)?;

    let template = factory_task::template::get_by_id(store, schedule.template_id)?;

    // ADR 0021 decision 11: the gate `factory schedule create --template`
    // applies at creation must hold afterwards too, or "paused" stops
    // meaning anything the moment the schedule already exists. Checked
    // before `create_from_schedule` is ever called, so a paused template
    // creates no run at all — not a run that then sits undelivered.
    if template.state != TemplateState::Open {
        return Ok(FireOutcome::TemplateNotOpen {
            state: template.state,
        });
    }

    let task_id = new_task_id();

    // One call, one transaction. `create_from_schedule` inserts the run with
    // its schedule and local minute, writes the `created` event, and stamps
    // `schedules.last_fired_at`, all inside a transaction it owns — ADR 0021
    // decision 3.
    //
    // This module used to assemble the same thing from `create_from_template`
    // plus a second transaction that tagged the row and stamped the schedule.
    // That closed the harm decision 3 names, but left a narrower hole: a
    // crash between the two commits stranded a run with
    // `triggered_by = 'manual'` and no `schedule_id`, which
    // `tasks_one_run_per_schedule_minute` never indexes and no later tick can
    // see, so the schedule would fire again for the same minute. A pre-check
    // read cannot close that either, because the orphan carries nothing to
    // find it by.
    //
    // Detecting the duplicate also moved with it. `factory-task` has
    // `rusqlite` as a direct dependency and matches
    // `ErrorCode::ConstraintViolation` against the index by name; this crate
    // does not, and could only have compared error text.
    let task_id = match factory_task::create::create_from_schedule(
        store,
        task_id,
        schedule.id,
        schedule.template_id,
        template.target_scope_id,
        template.target_agent_name.as_deref(),
        &template.prompt,
        &local_minute,
        instant,
    )? {
        factory_task::create::ScheduleFire::Fired(task_id) => task_id,
        // `AlreadyFired` is reported before `attempt_delivery` is ever
        // called — design §5's at-most-once delivery depends on this. A
        // repeated tick for the same minute must never reach `deliver` a
        // second time for a run it did not just create.
        factory_task::create::ScheduleFire::AlreadyFired => return Ok(FireOutcome::AlreadyFired),
    };

    // Design §11: "attempts delivery only to an assigned idle agent;
    // otherwise work stays queued for central assignment." A template with
    // no named agent is that "otherwise" — this dispatcher does not choose
    // one on its own, so there is nothing to attempt.
    if let Some(agent_name) = template.target_agent_name.as_deref() {
        attempt_delivery(
            store,
            adapter,
            instance_root,
            task_id,
            template.target_scope_id,
            agent_name,
        );
    }

    Ok(FireOutcome::Fired { task_id })
}

/// Best-effort assignment and delivery for the run [`try_fire`] just
/// created — design §5 steps 2-4, mirroring `ops::task::send`'s own sequence
/// (see this module's doc comment, "Attempting delivery after a fire", for
/// the full argument).
///
/// Returns nothing to `try_fire`: whatever happens here, `task_id` already
/// exists and already stays `queued` unless it is actually delivered. A busy
/// agent, no idle session, and a refusal are ordinary, silent outcomes; even
/// a genuine error (a store error, a broken pane lookup) only gets an
/// `eprintln!` and must never turn an already-created run into a reported
/// dispatcher failure — the same "never panics" stance [`record_tick`] takes
/// for its own write.
fn attempt_delivery(
    store: &mut factory_store::Store,
    adapter: &(dyn factory_adapter::Adapter + Send + Sync),
    instance_root: &std::path::Path,
    task_id: uuid::Uuid,
    scope_id: uuid::Uuid,
    agent_name: &str,
) {
    // Resolve the real `max_sessions`, the same way `ops::task::send` does
    // for its own `Some(agent_name)` branch (`ops/task.rs`: `load` the
    // config, then `find_agent(&config, scope_id, agent_name).max_sessions`)
    // — not `1`, which is `send`'s placeholder for a *different* branch (its
    // `target_session_id` case, where `assign`'s requested-session path
    // never reads either argument at all). `assign_untargeted` — the only
    // branch reachable here, since `create_from_schedule` hard-codes
    // `target_session_id`/`target_workspace_path` to `None` — does not read
    // `max_sessions` today either (checked directly against
    // `factory_task::assign`'s source: only `assign_requested_workspace`
    // does, and a cron run never reaches it). But that module's own doc
    // comment names starting a session "within `max_sessions`" as exactly
    // what a future widening of the untargeted branch would add, so this
    // resolves the real value now rather than a placeholder that would
    // silently disagree with the manual path the day that happens.
    let max_sessions = match crate::handler::config::load(instance_root).and_then(|config| {
        crate::handler::config::find_agent(&config, scope_id, agent_name)
            .map(|agent| agent.max_sessions)
    }) {
        Ok(max_sessions) => max_sessions,
        // The run already exists and stays `queued`, exactly as
        // `Assignment::Deferred` below leaves it — nothing here fails a run.
        //
        // But this is named, not silent. `Deferred` means "the agent is
        // busy", which the next tick may resolve on its own. This means the
        // configuration does not describe the agent this template asks for,
        // so **no tick will ever deliver this schedule until a human edits
        // one of the two**. A template and the configuration drift
        // independently: nothing revalidates a template after it is created,
        // and `agents:` can be edited at any time. Left silent, an operator
        // would see a schedule that fires every minute and a run that never
        // moves, with nothing anywhere saying why.
        //
        // It repeats at the tick rate, deliberately, for the reason
        // `log_failed_fires` gives for an unreadable schedule: a row nobody
        // has fixed is a finding that should keep being visible.
        Err(e) => {
            eprintln!(
                "factory: dispatcher: task {task_id} names agent `{agent_name}`, which this \
                 instance's configuration does not describe ({}: {}); the run stays queued \
                 until the template or the configuration is corrected",
                e.code, e.message
            );
            return;
        }
    };

    let assignment = match factory_task::assign::assign(store, task_id, agent_name, max_sessions) {
        Ok(assignment) => assignment,
        Err(e) => {
            eprintln!("factory: dispatcher: task {task_id} could not be assigned: {e}");
            return;
        }
    };

    let session_id = match assignment {
        factory_task::assign::Assignment::Assigned(session_id) => session_id,
        // No idle session, a requested session busy, an agent at capacity —
        // every one of these is design §11's "work stays queued for central
        // assignment," not a dispatcher problem to report.
        factory_task::assign::Assignment::Deferred(_)
        | factory_task::assign::Assignment::StartSessionAt(_) => return,
    };

    let pane = match crate::handler::pane::read(store, session_id) {
        Ok((pane, _)) => pane.map(factory_adapter::PaneId),
        Err(e) => {
            eprintln!(
                "factory: dispatcher: task {task_id} pane lookup failed: {}",
                e.message
            );
            return;
        }
    };

    let mut writer =
        crate::handler::deliver::AdapterPromptWriter::new(adapter, task_id, pane.clone());
    // `deliver` journals whichever of `sent`, `failed`, or `refused`
    // happened — including a refusal, ADR 0021 decision 4a's "a refusal is
    // an event, because it is not a delivery" — before this function ever
    // sees the result. There is nothing to add here on `Err`: the run stays
    // `queued`, and the reason already lives in the event `deliver` wrote.
    if factory_task::deliver::deliver(store, task_id, &mut writer).is_err() {
        return;
    }

    if let Err(e) = factory_task::deliver::mark_running(store, task_id) {
        eprintln!(
            "factory: dispatcher: task {task_id} was delivered but could not be marked running: {e}"
        );
        return;
    }

    // ADR 0021 decision 6, mirroring `ops::task::send`'s own comment: the
    // baseline is the cost sample taken at delivery, written *after*
    // `mark_running` has already committed, in its own step, so a failure
    // here can never undo a delivery that already happened.
    //
    // `ops::task::sample_cost_best_effort` is the one other place this exact
    // shape lives, and it is `fn`, not `pub(crate)` — private to that module,
    // which another agent owns on this station. Rather than duplicate its
    // body under a second name, this calls `Adapter::cost_sample` directly;
    // see this task's own report, which recommends the coordinator widen
    // that helper's visibility instead.
    let baseline = pane
        .as_ref()
        .and_then(|p| adapter.cost_sample(p).ok().flatten());
    let baseline_json = baseline.as_ref().map(ToString::to_string);
    let _ = factory_task::deliver::record_cost_baseline(store, task_id, baseline_json.as_deref());
}

/// Upsert the dispatcher's own heartbeat. Nothing else in this workspace
/// writes `dispatcher_state` (`factory_store::schema`'s own comment on the
/// table), so this is not the second-home problem the rest of this module's
/// doc comment is careful about — there is no other home to defer to.
///
/// `INSERT OR REPLACE` rather than a separate `UPDATE`-then-`INSERT`
/// dance: `dispatcher_state` has exactly one legitimate row
/// (`CHECK (id = 1)`), so replacing whatever is at `id = 1` is always the
/// right write, first tick or the thousandth.
///
/// Runs in its own transaction, after every schedule's own attempt in this
/// tick — never inside a fire's transaction, so a rolled-back fire can never
/// roll back the tick record with it. Silently does nothing
/// on a store error, the same "never panics" stance [`tick`] itself takes:
/// a tick that could not even write its own heartbeat has nothing further
/// useful to do, and the next tick tries again a minute later.
fn record_tick(store: &mut factory_store::Store, instant: DateTime<Utc>) {
    let Ok(tx) = store.transaction() else {
        return;
    };
    let _ = tx.execute(
        "INSERT OR REPLACE INTO dispatcher_state (id, last_tick_at) VALUES (1, ?1)",
        [instant.to_rfc3339()],
    );
    let _ = tx.commit();
}

/// Mint a fresh task id. `uuid` is pinned workspace-wide without the `v4` or
/// `v7` feature (`factory_cli::ids`'s own doc comment explains why, for the
/// identical reason), and this crate has no dependency on `factory-cli` to
/// reuse its minting helper — depending "up" from the daemon into its own
/// CLI client would invert the layering ADR 0014 draws between them. This is
/// a smaller copy of the same technique, through
/// `uuid::Builder::from_unix_timestamp_millis`, which is not feature-gated:
/// a real UUIDv7, with a process-local counter written into the
/// counter/random bytes so two ids minted in this process can never
/// collide, regardless of what the millisecond timestamp does. Unlike
/// `factory_cli::ids::new_id`, this has no need to be unpredictable across
/// processes — a task id is a primary key, not a security token — so it
/// skips that module's OS-seeded mixing.
fn new_task_id() -> uuid::Uuid {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut counter_random_bytes = [0u8; 10];
    counter_random_bytes[..8].copy_from_slice(&counter.to_be_bytes());
    uuid::Builder::from_unix_timestamp_millis(millis, &counter_random_bytes).into_uuid()
}

/// Run [`tick`] on `interval` until `stop` is signalled (or dropped).
/// Mirrors [`crate::observe::spawn`]'s shape — ADR 0021 decision 2: "the
/// same shape with a different period" — a spawned thread, a stop channel, a
/// first pass before the first wait, and a prompt stop.
///
/// Returns the join handle, the stop sender, and a third channel this loop
/// has that `observe::spawn` does not need: a receiver that gets one message
/// after every completed tick. `observe_loop_drill.rs` proves its loop ticks
/// on schedule with a fake `Adapter` that signals on every call; this loop
/// signals from itself instead, because `tick` runs to completion
/// synchronously and there is no separate confirmation step to wait on the
/// way `observe` waits on an `Observation`.
pub fn spawn(
    handler: Arc<crate::handler::FactoryHandler>,
    interval: Duration,
) -> (JoinHandle<()>, Sender<()>, Receiver<()>) {
    let (stop_tx, stop_rx) = mpsc::channel();
    let (ticked_tx, ticked_rx) = mpsc::channel();

    let join = std::thread::spawn(move || {
        loop {
            {
                let mut store = handler.lock_store();
                let reports = tick(
                    &mut store,
                    handler.adapter(),
                    handler.instance_root(),
                    Utc::now(),
                );
                log_failed_fires(&reports);
            }
            let _ = ticked_tx.send(());
            match stop_rx.recv_timeout(interval) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    });

    (join, stop_tx, ticked_rx)
}

/// Print the outcomes from one [`tick`] that mean a schedule did not fire
/// when it should have.
///
/// `eprintln!`, not a return value a caller must remember to check: this
/// runs inside the spawned thread, where the only way out is already stderr
/// or silence. Living here, next to [`tick`] itself, rather than in whatever
/// called [`spawn`], means every caller gets this visibility for free and
/// never has to remember to add it — a failed fire is this module's own
/// domain information (`FireOutcome` is this module's type), not something a
/// generic caller should have to interpret. `factory start`
/// (`factory-cli`'s `open_log_pair`) redirects the whole daemon process's
/// stderr to `<root>/.factory/daemon.log`, so this reaches a real,
/// operator-readable file without this crate knowing anything about log
/// files — stderr is stderr, wherever the process's stderr ends up.
///
/// Logs [`FireOutcome::Failed`] only. That already covers every entry
/// [`factory_task::schedule::due`] puts in `Due::unreadable`: [`tick`] folds
/// each one into a `ScheduleReport` whose outcome is
/// `FireOutcome::Failed(bad.reason)` before this function ever sees the
/// list, so there is no separate `unreadable` case to add here.
///
/// Silent on purpose for [`FireOutcome::AlreadyFired`], [`FireOutcome::Fired`],
/// and [`FireOutcome::TemplateNotOpen`]. At `DISPATCH_INTERVAL`'s cadence
/// (roughly three ticks a minute — see `factory-cli`'s own comment on that
/// constant), `AlreadyFired` is the *normal* outcome for a healthy instance
/// almost every tick; logging it would put several lines a minute in the log
/// for an instance where nothing is wrong. `TemplateNotOpen` is silent for a
/// different reason (ADR 0021 decision 11): an operator paused or closed
/// the template on purpose, so a schedule skipping it is not a finding to
/// surface at all, not merely one to rate-limit.
///
/// A schedule that stays broken (bad cron on a hand-edited row) repeats its
/// line once every tick, forever. That repetition is deliberate, not a bug
/// to fix here: telling "still broken" apart from "broken again" needs state
/// this loop does not keep — it would have to remember which schedule ids it
/// already logged and when — and a row nobody has fixed yet is exactly the
/// kind of finding an operator should keep seeing on every tick, not one
/// that quietly falls silent after its first mention.
fn log_failed_fires(reports: &[ScheduleReport]) {
    for report in reports {
        match &report.outcome {
            FireOutcome::Failed(reason) => eprintln!(
                "factory: dispatcher: schedule {} failed to fire: {reason}",
                report.schedule_id
            ),
            FireOutcome::Fired { .. }
            | FireOutcome::AlreadyFired
            | FireOutcome::TemplateNotOpen { .. } => {}
        }
    }
}
