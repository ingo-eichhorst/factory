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
//! [`tick`] does three things for the instant it is given:
//!
//! 1. Ask [`factory_task::schedule::due`] which enabled schedules match this
//!    minute. This module never parses cron itself — decision 3a puts the
//!    one home of that predicate in `factory_task::schedule`, and the
//!    `factory schedule list` preview and this dispatcher must never
//!    disagree about what "due" means.
//! 2. For each due schedule, create a run from its template, tag it with
//!    `schedule_id` / `fired_for_minute` / `triggered_by = 'cron'`, and call
//!    [`factory_task::schedule::mark_fired`]. See "The two-transaction gap"
//!    below for exactly what "in one transaction" means here and where it
//!    falls short of decision 3's ideal.
//! 3. Record the tick in `dispatcher_state`, unconditionally — even a tick
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
//! `fired_for_minute` string. [`is_already_fired_violation`] is where that
//! is told apart from every other way the tagging `UPDATE` in
//! [`tag_and_mark_fired`] can fail — see that function's own doc comment for
//! why matching the SQLite error code alone is not enough.
//!
//! # The two-transaction gap
//!
//! ADR 0021 decision 3 wants the run insert and `schedules.last_fired_at`
//! written in **one** transaction. `factory_task::create::create_from_template`
//! cannot be that transaction: it opens and commits its own
//! (`Store::transaction` takes `&mut Store` for the `Transaction`'s whole
//! lifetime, so a caller already holding one open transaction cannot pass
//! the same `&mut Store` into a function that wants to open a second), and
//! `factory_task::events::append` — the crate's own "one place that INSERTs
//! into [`task_events`]" — is `pub(crate)` to `factory_task`, unreachable
//! from here. Writing `tasks` or `task_events` rows directly from this crate
//! would be exactly the second-home problem decision 3 already names for
//! `last_fired_at`, one layer further in, and duplicating `insert_task`'s
//! INSERT here would silently drop the run's `created` event that crate
//! decision 3 requires.
//!
//! So [`fire`] does the closest available thing in two steps: a pre-check
//! read for an existing `(schedule_id, fired_for_minute)` row (so an
//! ordinary repeated tick never calls `create_from_template` a second time),
//! then `create_from_template` on its own, then [`tag_and_mark_fired`] —
//! **one** transaction covering the `UPDATE` that tags the new row and the
//! `mark_fired` call, so the harm decision 3 actually names cannot happen: a
//! rejected tag rolls back `last_fired_at` with it. What is not closed is a
//! narrower one decision 3 does not name: a crash between
//! `create_from_template`'s commit and `tag_and_mark_fired`'s leaves an
//! untagged, `triggered_by = 'manual'` orphan row, invisible to the
//! pre-check on every later tick because it carries no `schedule_id`. See
//! this task's report for the exact API change that closes it —
//! `create_from_template` (or a sibling) taking a caller's own
//! `&rusqlite::Transaction` plus `schedule_id` / `fired_for_minute` /
//! `triggered_by`.
//!
//! # A template with no target scope
//!
//! `factory_task::template::Template::target_scope_id` is `Option` — "`None`
//! means 'no target' — design §11's run that 'remains queued for the
//! central agent to assign.'" `tasks.target_scope_id` is `NOT NULL`, and
//! `create_from_template`'s own `target_scope_id` parameter is a plain
//! `uuid::Uuid`, not an `Option`. There is today no way to create a run for
//! such a template at all. [`fire`] reports [`FireOutcome::NoTargetScope`]
//! rather than guessing at a placeholder scope; see this task's report for
//! why that is a defect in `factory_task`, not something this module can
//! close.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};

use factory_task::schedule::Schedule;

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
    /// A new run was created, tagged, and `schedules.last_fired_at` was
    /// stamped in the same transaction as the tag.
    Fired { task_id: uuid::Uuid },
    /// A run for this schedule and this local minute already exists — the
    /// ordinary autumn-clock-change outcome ADR 0021 decision 3a names, not
    /// a fault. Detected either by this function's own pre-check or by
    /// [`is_already_fired_violation`]; either way nothing was written.
    AlreadyFired,
    /// The schedule's template has `target_scope_id = NULL`. See this
    /// module's doc comment, "A template with no target scope" — no run was
    /// created, because `tasks.target_scope_id` cannot hold one.
    NoTargetScope,
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
/// module's doc comment for the three things it does.
///
/// Never panics, and one schedule's failure never stops the rest — the same
/// stance `observe::reconcile_once` takes for one session's failure, for the
/// same reason: a bad row or a transient store error must not turn into a
/// silent stall for every schedule after it.
pub fn tick(store: &mut factory_store::Store, instant: DateTime<Utc>) -> Vec<ScheduleReport> {
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
        let outcome = fire(store, &schedule, instant);
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
    schedule: &Schedule,
    instant: DateTime<Utc>,
) -> FireOutcome {
    match try_fire(store, schedule, instant) {
        Ok(outcome) => outcome,
        Err(e) => FireOutcome::Failed(e.to_string()),
    }
}

fn try_fire(
    store: &mut factory_store::Store,
    schedule: &Schedule,
    instant: DateTime<Utc>,
) -> Result<FireOutcome, DispatchError> {
    let local_minute = factory_task::schedule::local_minute_string(&schedule.timezone, instant)?;

    let template = factory_task::template::get_by_id(store, schedule.template_id)?;
    let Some(target_scope_id) = template.target_scope_id else {
        return Ok(FireOutcome::NoTargetScope);
    };

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
    match factory_task::create::create_from_schedule(
        store,
        task_id,
        schedule.id,
        schedule.template_id,
        target_scope_id,
        template.target_agent_name.as_deref(),
        &template.prompt,
        &local_minute,
        instant,
    )? {
        factory_task::create::ScheduleFire::Fired(task_id) => Ok(FireOutcome::Fired { task_id }),
        factory_task::create::ScheduleFire::AlreadyFired => Ok(FireOutcome::AlreadyFired),
    }
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
/// tick — never inside [`tag_and_mark_fired`]'s transaction, so a rolled-back
/// fire can never roll back the tick record with it. Silently does nothing
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
/// has no adapter to fake, so the signal comes from the loop itself instead
/// of from a double standing in for a dependency.
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
                let _ = tick(&mut store, Utc::now());
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
