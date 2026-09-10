//! Station 11's cron dispatcher (ADR 0021 decisions 2, 3, 3a, 10;
//! `crates/factory-daemon/src/dispatch.rs`).
//!
//! Every "one tick" test drives [`dispatch::tick`] directly against an
//! explicit `instant` — never `Utc::now()` — so nothing here depends on
//! wall-clock time or the minute the suite happens to run in. The loop tests
//! at the bottom follow `crates/factory-e2e/tests/observe_loop_drill.rs`'s
//! own deterministic style: a signal channel proves the loop ticks on its
//! interval, never a sleep, and a long interval plus a bounded join proves a
//! stop is prompt rather than merely eventual.
//!
//! `tasks.schedule_id`, `.fired_for_minute`, and `.triggered_by` are not on
//! `factory_task::create::Task` — `dispatch.rs`'s own doc comment names this
//! as part of the gap this task's report describes. Tests that need them
//! read the columns directly through `Store::connection()`, the same
//! sanctioned form `record_pane` uses in `observe_loop_drill.rs` for a
//! column with no dedicated public reader.

mod common;

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use factory_daemon::dispatch::{self, FireOutcome};

fn uid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

/// The one instant every "one tick" test fires against: 2026-09-10 09:00:00
/// UTC. Schedules in this file use `"UTC"` as their own timezone, so
/// [`factory_task::schedule::local_minute_string`] renders this exact
/// instant as `"2026-09-10T09:00"` — asserted directly in
/// [`a_due_schedule_creates_exactly_one_run`].
fn instant() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 10, 9, 0, 0).unwrap()
}

/// A cron expression that matches only [`instant`]'s exact minute — narrow
/// on purpose, so a test asserting "this schedule fires" or "this one does
/// not" can never be fooled by the suite running in the same minute as its
/// own fixed instant.
const MATCHING_CRON: &str = "0 9 10 9 *";

/// A cron expression for the same day and minute of hour, one hour later —
/// never matches [`instant`].
const NON_MATCHING_CRON: &str = "0 10 10 9 *";

fn create_template(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    name: &str,
    target_scope_id: uuid::Uuid,
) -> uuid::Uuid {
    factory_task::template::create(store, id, name, target_scope_id, None, "do it", None)
        .expect("create template")
}

/// A template row whose `target_scope_id` is NULL, written by raw SQL.
///
/// `factory_task::template::create` will not produce one: a run's own
/// `tasks.target_scope_id` is NOT NULL, so a template without a scope could
/// never fire, and version 1 refuses to create one rather than let an
/// operator make a schedule that silently never runs. The column itself is
/// still nullable, so a hand edit or a foreign restore can put one there —
/// which is exactly the state this helper builds, and exactly what
/// `FireOutcome::NoTargetScope` exists to name.
fn create_template_with_no_scope(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    name: &str,
) -> uuid::Uuid {
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO task_templates (id, name, prompt) VALUES (?1, ?2, 'do it')",
        (id.to_string(), name),
    )
    .expect("insert a scopeless template by hand");
    tx.commit().expect("commit");
    id
}

fn create_schedule(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    template_id: uuid::Uuid,
    cron: &str,
) -> uuid::Uuid {
    factory_task::schedule::create(store, id, template_id, cron, "UTC").expect("create schedule")
}

/// The run columns `factory_task::create::Task` does not expose — see this
/// file's own module doc comment.
fn run_columns(store: &factory_store::Store, task_id: uuid::Uuid) -> (String, String, String) {
    store
        .connection()
        .query_row(
            "SELECT schedule_id, fired_for_minute, triggered_by FROM tasks WHERE id = ?1",
            [task_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read schedule_id/fired_for_minute/triggered_by")
}

fn dispatcher_state_row_count(store: &factory_store::Store) -> i64 {
    store
        .connection()
        .query_row("SELECT COUNT(*) FROM dispatcher_state", [], |row| {
            row.get(0)
        })
        .expect("count dispatcher_state")
}

fn dispatcher_state_last_tick_at(store: &factory_store::Store) -> String {
    store
        .connection()
        .query_row(
            "SELECT last_tick_at FROM dispatcher_state WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .expect("read dispatcher_state.last_tick_at")
}

// One tick, driven directly -------------------------------------------------

/// The core acceptance path: a due schedule creates exactly one run, tagged
/// as this tick's own cron firing, carrying the template's version frozen at
/// creation (ADR 0021 decision 2). Mutation caught: dropping the tag
/// `UPDATE` in `tag_and_mark_fired`, or passing the wrong `template_id` /
/// `template_version` into `create_from_template`.
#[test]
fn a_due_schedule_creates_exactly_one_run() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let mut store = factory_store::Store::open(fixture.instance_root()).expect("open store");

    let template_id = create_template(&mut store, uid(10), "tmpl-a", scope_id);
    let schedule_id = create_schedule(&mut store, uid(11), template_id, MATCHING_CRON);

    let reports = dispatch::tick(&mut store, instant());

    assert_eq!(
        reports.len(),
        1,
        "exactly one schedule was due: {reports:?}"
    );
    assert_eq!(reports[0].schedule_id, schedule_id);
    let task_id = match reports[0].outcome {
        FireOutcome::Fired { task_id } => task_id,
        ref other => panic!("expected Fired, got {other:?}"),
    };

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(task.target_scope_id, scope_id);
    assert_eq!(task.template_id, Some(template_id));
    assert_eq!(
        task.template_version,
        Some(1),
        "the template's version at creation must be frozen onto the run"
    );
    assert_eq!(task.status, factory_task::TaskStatus::Queued);

    let (db_schedule_id, fired_for_minute, triggered_by) = run_columns(&store, task_id);
    assert_eq!(db_schedule_id, schedule_id.to_string());
    assert_eq!(fired_for_minute, "2026-09-10T09:00");
    assert_eq!(triggered_by, "cron");
}

/// A schedule whose cron does not match this minute is left alone entirely
/// — not even attempted. Mutation caught: calling `fire` for every enabled
/// schedule regardless of `factory_task::schedule::due`'s own answer.
#[test]
fn a_schedule_that_does_not_match_this_minute_creates_nothing() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let mut store = factory_store::Store::open(fixture.instance_root()).expect("open store");

    let template_id = create_template(&mut store, uid(20), "tmpl-b", scope_id);
    create_schedule(&mut store, uid(21), template_id, NON_MATCHING_CRON);

    let reports = dispatch::tick(&mut store, instant());

    assert!(
        reports.is_empty(),
        "a non-matching schedule must not even be reported: {reports:?}"
    );
    assert!(factory_task::create::list(&store).expect("list").is_empty());
}

/// A disabled schedule whose cron *would* match this minute still creates
/// nothing. The cron matching on purpose is what makes this test mean
/// something: `factory_task::schedule::due` already filters `enabled = 1` in
/// SQL, so a schedule that also fails to match would pass this test even if
/// the dispatcher ignored `enabled` entirely. Mutation caught: a dispatcher
/// that iterated every schedule in the table instead of going through
/// `due`.
#[test]
fn a_disabled_schedule_creates_nothing_even_when_its_cron_matches() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let mut store = factory_store::Store::open(fixture.instance_root()).expect("open store");

    let template_id = create_template(&mut store, uid(30), "tmpl-c", scope_id);
    let schedule_id = create_schedule(&mut store, uid(31), template_id, MATCHING_CRON);
    factory_task::schedule::disable(&mut store, schedule_id).expect("disable");

    let reports = dispatch::tick(&mut store, instant());

    assert!(
        reports.is_empty(),
        "a disabled schedule must not be reported at all: {reports:?}"
    );
    assert!(factory_task::create::list(&store).expect("list").is_empty());
}

/// Backlog §11's own acceptance criterion: no more than one run for the same
/// local minute, "including after dispatcher restarts" — running the tick
/// twice for the same instant is how a restart looks from the database's
/// side. The second call must report `AlreadyFired`, not an error, and
/// exactly one run must exist afterward. Mutation caught: dropping the
/// pre-check in `try_fire` (which would leave an untagged orphan row behind
/// a second `create_from_template` call — see `dispatch.rs`'s "The
/// two-transaction gap") or mis-classifying the tag `UPDATE`'s rejection as
/// a real error.
#[test]
fn the_same_tick_run_twice_creates_exactly_one_run_and_the_second_is_not_an_error() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let mut store = factory_store::Store::open(fixture.instance_root()).expect("open store");

    let template_id = create_template(&mut store, uid(40), "tmpl-d", scope_id);
    create_schedule(&mut store, uid(41), template_id, MATCHING_CRON);

    let first = dispatch::tick(&mut store, instant());
    assert!(
        matches!(first[0].outcome, FireOutcome::Fired { .. }),
        "first tick must fire: {first:?}"
    );

    let second = dispatch::tick(&mut store, instant());
    assert_eq!(second.len(), 1);
    assert_eq!(
        second[0].outcome,
        FireOutcome::AlreadyFired,
        "a repeated tick for the same minute must report AlreadyFired, not an error or a second Fired"
    );

    let tasks = factory_task::create::list(&store).expect("list");
    assert_eq!(
        tasks.len(),
        1,
        "exactly one run must exist after firing the same minute twice: {tasks:?}"
    );
}

/// `schedules.last_fired_at` is stamped exactly once, on the tick that
/// actually created a run — never touched by the rejected duplicate.
/// Mutation caught: calling `mark_fired` before the pre-check, or outside
/// `tag_and_mark_fired`'s transaction so a rolled-back tag leaves the stamp
/// behind anyway (the harm ADR 0021 decision 3 names directly).
#[test]
fn last_fired_at_is_stamped_on_a_successful_fire_and_not_on_a_rejected_duplicate() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let mut store = factory_store::Store::open(fixture.instance_root()).expect("open store");

    let template_id = create_template(&mut store, uid(50), "tmpl-e", scope_id);
    let schedule_id = create_schedule(&mut store, uid(51), template_id, MATCHING_CRON);

    let before = factory_task::schedule::get(&store, schedule_id).expect("get");
    assert_eq!(before.last_fired_at, None);

    dispatch::tick(&mut store, instant());
    let after_first = factory_task::schedule::get(&store, schedule_id).expect("get");
    assert!(
        after_first.last_fired_at.is_some(),
        "last_fired_at must be stamped on a successful fire"
    );

    dispatch::tick(&mut store, instant());
    let after_second = factory_task::schedule::get(&store, schedule_id).expect("get");
    assert_eq!(
        after_second.last_fired_at, after_first.last_fired_at,
        "a rejected duplicate must leave last_fired_at exactly as the first fire left it"
    );
}

/// `dispatcher_state` is written by every tick, and reads as entirely absent
/// before the first one — `factory_store::schema`'s own comment on the
/// table: "no row at all is a distinct answer from a stale row." Mutation
/// caught: writing `dispatcher_state` only when a schedule fires (a tick
/// with nothing due would then look identical to a dispatcher that has
/// never run).
#[test]
fn dispatcher_state_is_absent_before_the_first_tick_and_present_after() {
    let fixture = common::build(&[]);
    let mut store = factory_store::Store::open(fixture.instance_root()).expect("open store");

    assert_eq!(
        dispatcher_state_row_count(&store),
        0,
        "no dispatcher has ticked against this database yet"
    );

    let when = instant();
    let reports = dispatch::tick(&mut store, when);
    assert!(reports.is_empty(), "no schedules exist in this fixture");

    assert_eq!(dispatcher_state_row_count(&store), 1);
    assert_eq!(dispatcher_state_last_tick_at(&store), when.to_rfc3339());
}

/// One schedule's outcome never depends on, or blocks, another's in the same
/// tick. Two schedules are due in the same tick here: one whose template has
/// a registered target scope (fires normally) and one whose template does
/// not (see "A template with no target scope" below) — chosen because this
/// schema's foreign keys make a genuine `TaskError`/`StoreError` hard to
/// provoke without first breaking an invariant `factory_task::schedule::create`
/// or `factory_task::template::create` themselves refuse to let a test set
/// up (a bad `template_id` or `target_scope_id` is rejected by a `REFERENCES`
/// constraint at the moment a test tries to create the schedule or template
/// that would carry it, not later). `NoTargetScope` reaches `tick`'s per
/// schedule loop through the exact same branch `FireOutcome::Failed` would —
/// `try_fire` returning something other than `Fired` for one schedule and the
/// loop moving on to the next — so this is a faithful proof of isolation
/// even though it is not literally a `Failed` outcome.
#[test]
fn one_schedules_outcome_does_not_stop_the_others_in_the_same_tick() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let mut store = factory_store::Store::open(fixture.instance_root()).expect("open store");

    let ok_template = create_template(&mut store, uid(60), "tmpl-f-ok", scope_id);
    let ok_schedule = create_schedule(&mut store, uid(61), ok_template, MATCHING_CRON);

    let broken_template = create_template_with_no_scope(&mut store, uid(62), "tmpl-f-broken");
    let broken_schedule = create_schedule(&mut store, uid(63), broken_template, MATCHING_CRON);

    let mut reports = dispatch::tick(&mut store, instant());
    reports.sort_by_key(|r| r.schedule_id);
    let mut expected_order = [ok_schedule, broken_schedule];
    expected_order.sort();

    assert_eq!(reports.len(), 2);
    for report in &reports {
        if report.schedule_id == ok_schedule {
            assert!(
                matches!(report.outcome, FireOutcome::Fired { .. }),
                "the schedule with a real target scope must still fire: {report:?}"
            );
        } else if report.schedule_id == broken_schedule {
            assert_eq!(report.outcome, FireOutcome::NoTargetScope);
        } else {
            panic!("unexpected schedule id in report: {report:?}");
        }
    }

    let tasks = factory_task::create::list(&store).expect("list");
    assert_eq!(
        tasks.len(),
        1,
        "only the schedule with a target scope may have produced a run: {tasks:?}"
    );
}

/// A template row with `target_scope_id = NULL` names no run it could ever
/// produce, and the dispatcher says so rather than inventing a scope.
///
/// `factory_task::template::create` cannot make one: design §11's run that
/// "remains queued for the central agent to assign" is one whose target
/// **agent** is unspecified, and a run's own `tasks.target_scope_id` is NOT
/// NULL. So version 1 requires a scope on the template, and an operator
/// cannot create a schedule that silently never fires.
///
/// The column is still nullable, so a hand edit or a restore from something
/// that was not Factory can put such a row there. This test builds exactly
/// that state, and asserts the dispatcher names the schedule instead of
/// failing the tick or quietly skipping it.
#[test]
fn a_hand_written_template_with_no_target_scope_is_named_not_skipped() {
    let fixture = common::build(&[]);
    let mut store = factory_store::Store::open(fixture.instance_root()).expect("open store");

    let template_id = create_template_with_no_scope(&mut store, uid(70), "tmpl-g");
    let schedule_id = create_schedule(&mut store, uid(71), template_id, MATCHING_CRON);

    let reports = dispatch::tick(&mut store, instant());

    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].schedule_id, schedule_id);
    assert_eq!(reports[0].outcome, FireOutcome::NoTargetScope);
    assert!(factory_task::create::list(&store).expect("list").is_empty());
}

// The loop itself -------------------------------------------------------

/// Runs `join` on a helper thread and reports back over a channel, so the
/// caller can bound how long it waits — mirrors
/// `observe_loop_drill.rs::join_within` exactly, for the identical reason: a
/// direct `JoinHandle::join()` would hang the whole test binary, not just
/// fail one assertion, against the exact mutation (an ignored stop signal)
/// this drill exists to catch.
fn join_within(join: std::thread::JoinHandle<()>, timeout: Duration) -> bool {
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = join.join();
        let _ = done_tx.send(());
    });
    done_rx.recv_timeout(timeout).is_ok()
}

/// Three separate ticks, each proven by its own message on the channel
/// `dispatch::spawn` returns — not "the loop ran for N milliseconds," which
/// would only ever be a guess about how many ticks that implies. A short
/// interval only bounds how long this drill takes, never whether it passes.
#[test]
fn the_threaded_loop_ticks_on_its_interval() {
    let fixture = common::build(&[]);
    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();
    let handler = Arc::new(factory_daemon::FactoryHandler::new(
        store,
        adapter,
        fixture.instance_root(),
    ));

    let (join, stop_tx, ticked_rx) = dispatch::spawn(handler, Duration::from_millis(30));

    for i in 0..3 {
        ticked_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_else(|_| panic!("expected dispatcher tick #{i} within 2s"));
    }

    stop_tx.send(()).expect("send stop");
    assert!(
        join_within(join, Duration::from_secs(2)),
        "the dispatcher loop must stop promptly once told to"
    );
}

/// A long interval (30s): if stopping ever had to wait for it, this drill
/// would itself take 30 seconds, or the bounded join below would time out
/// and fail loudly rather than hang. `dispatch::spawn`'s loop body runs its
/// first tick before it ever waits on the interval, so the first message on
/// `ticked_rx` is guaranteed to arrive quickly, not "usually."
#[test]
fn stopping_the_dispatcher_loop_is_prompt_not_eventually() {
    let fixture = common::build(&[]);
    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();
    let handler = Arc::new(factory_daemon::FactoryHandler::new(
        store,
        adapter,
        fixture.instance_root(),
    ));

    let (join, stop_tx, ticked_rx) = dispatch::spawn(handler, Duration::from_secs(30));

    ticked_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the first tick must happen immediately, not after the 30s interval");

    let stopped_at = std::time::Instant::now();
    stop_tx.send(()).expect("send stop");
    let stopped = join_within(join, Duration::from_secs(2));
    let elapsed = stopped_at.elapsed();

    assert!(
        stopped,
        "the dispatcher loop must stop within 2s of being told to, not after its 30s interval \
         elapses (it did not stop at all within the bound)"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "stopping took {elapsed:?} — nowhere near prompt against a 30s interval"
    );
}
