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

/// A template that names a target agent, for the delivery-attempt tests —
/// [`create_template`] always passes `None`, since most tests in this file
/// only care about the fire itself.
fn create_template_with_agent(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    name: &str,
    target_scope_id: uuid::Uuid,
    agent_name: &str,
) -> uuid::Uuid {
    factory_task::template::create(
        store,
        id,
        name,
        target_scope_id,
        Some(agent_name),
        "do it",
        None,
    )
    .expect("create template with a target agent")
}

fn create_schedule(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    template_id: uuid::Uuid,
    cron: &str,
) -> uuid::Uuid {
    factory_task::schedule::create(store, id, template_id, cron, "UTC").expect("create schedule")
}

/// A `running`, idle session for `agent_name` in `scope_id`, with `pane`
/// recorded as its `herdr_pane_id` when given — written directly with raw
/// SQL. `factory_session::begin_start`'s workspace-lease machinery is more
/// than these tests need: `assign::is_idle` only reads `sessions.state` and
/// whether any non-terminal task already names the session, and
/// `crate::handler::pane::record` is `pub(crate)` to `factory-daemon`, so a
/// raw insert is this crate's own sanctioned way to seed one — the same
/// convention `create_template_with_no_scope` (removed with defect 3) used
/// for `task_templates`.
///
/// `workspace_path` must be distinct per session in one test:
/// `sessions_one_live_lease_per_workspace` is a unique index over every
/// lease-holding state, `running` included.
fn create_idle_session(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    scope_id: uuid::Uuid,
    agent_name: &str,
    workspace_path: &str,
    pane: Option<&str>,
) -> uuid::Uuid {
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO sessions (id, scope_id, agent_name, workspace_path, state, herdr_pane_id) \
         VALUES (?1, ?2, ?3, ?4, 'running', ?5)",
        (
            id.to_string(),
            scope_id.to_string(),
            agent_name,
            workspace_path,
            pane,
        ),
    )
    .expect("insert idle session");
    tx.commit().expect("commit");
    id
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
    let adapter = common::FakeAdapter::new();

    let template_id = create_template(&mut store, uid(10), "tmpl-a", scope_id);
    let schedule_id = create_schedule(&mut store, uid(11), template_id, MATCHING_CRON);

    let reports = dispatch::tick(&mut store, &adapter, fixture.instance_root(), instant());

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
    let adapter = common::FakeAdapter::new();

    let template_id = create_template(&mut store, uid(20), "tmpl-b", scope_id);
    create_schedule(&mut store, uid(21), template_id, NON_MATCHING_CRON);

    let reports = dispatch::tick(&mut store, &adapter, fixture.instance_root(), instant());

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
    let adapter = common::FakeAdapter::new();

    let template_id = create_template(&mut store, uid(30), "tmpl-c", scope_id);
    let schedule_id = create_schedule(&mut store, uid(31), template_id, MATCHING_CRON);
    factory_task::schedule::disable(&mut store, schedule_id).expect("disable");

    let reports = dispatch::tick(&mut store, &adapter, fixture.instance_root(), instant());

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
    let adapter = common::FakeAdapter::new();

    let template_id = create_template(&mut store, uid(40), "tmpl-d", scope_id);
    create_schedule(&mut store, uid(41), template_id, MATCHING_CRON);

    let first = dispatch::tick(&mut store, &adapter, fixture.instance_root(), instant());
    assert!(
        matches!(first[0].outcome, FireOutcome::Fired { .. }),
        "first tick must fire: {first:?}"
    );

    let second = dispatch::tick(&mut store, &adapter, fixture.instance_root(), instant());
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
    let adapter = common::FakeAdapter::new();

    let template_id = create_template(&mut store, uid(50), "tmpl-e", scope_id);
    let schedule_id = create_schedule(&mut store, uid(51), template_id, MATCHING_CRON);

    let before = factory_task::schedule::get(&store, schedule_id).expect("get");
    assert_eq!(before.last_fired_at, None);

    dispatch::tick(&mut store, &adapter, fixture.instance_root(), instant());
    let after_first = factory_task::schedule::get(&store, schedule_id).expect("get");
    assert!(
        after_first.last_fired_at.is_some(),
        "last_fired_at must be stamped on a successful fire"
    );

    dispatch::tick(&mut store, &adapter, fixture.instance_root(), instant());
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
    let adapter = common::FakeAdapter::new();

    assert_eq!(
        dispatcher_state_row_count(&store),
        0,
        "no dispatcher has ticked against this database yet"
    );

    let when = instant();
    let reports = dispatch::tick(&mut store, &adapter, fixture.instance_root(), when);
    assert!(reports.is_empty(), "no schedules exist in this fixture");

    assert_eq!(dispatcher_state_row_count(&store), 1);
    assert_eq!(dispatcher_state_last_tick_at(&store), when.to_rfc3339());
}

/// One schedule's outcome never depends on, or blocks, another's in the same
/// tick. Two schedules are due in the same tick here: one whose template is
/// `open` (fires normally) and one whose template is `paused` (skipped, see
/// defect 2's own tests below) — chosen because this schema's foreign keys
/// make a genuine `TaskError`/`StoreError` hard to provoke without first
/// breaking an invariant `factory_task::schedule::create` or
/// `factory_task::template::create` themselves refuse to let a test set up.
/// `TemplateNotOpen` reaches `tick`'s per schedule loop through the exact
/// same branch `FireOutcome::Failed` would — `try_fire` returning something
/// other than `Fired` for one schedule and the loop moving on to the next —
/// so this is a faithful proof of isolation even though it is not literally
/// a `Failed` outcome.
#[test]
fn one_schedules_outcome_does_not_stop_the_others_in_the_same_tick() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let mut store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();

    let ok_template = create_template(&mut store, uid(60), "tmpl-f-ok", scope_id);
    let ok_schedule = create_schedule(&mut store, uid(61), ok_template, MATCHING_CRON);

    let paused_template = create_template(&mut store, uid(62), "tmpl-f-paused", scope_id);
    factory_task::template::set_state(
        &mut store,
        paused_template,
        factory_task::template::TemplateState::Paused,
    )
    .expect("pause");
    let paused_schedule = create_schedule(&mut store, uid(63), paused_template, MATCHING_CRON);

    let mut reports = dispatch::tick(&mut store, &adapter, fixture.instance_root(), instant());
    reports.sort_by_key(|r| r.schedule_id);
    let mut expected_order = [ok_schedule, paused_schedule];
    expected_order.sort();

    assert_eq!(reports.len(), 2);
    for report in &reports {
        if report.schedule_id == ok_schedule {
            assert!(
                matches!(report.outcome, FireOutcome::Fired { .. }),
                "the schedule whose template is open must still fire: {report:?}"
            );
        } else if report.schedule_id == paused_schedule {
            assert_eq!(
                report.outcome,
                FireOutcome::TemplateNotOpen {
                    state: factory_task::template::TemplateState::Paused
                }
            );
        } else {
            panic!("unexpected schedule id in report: {report:?}");
        }
    }

    let tasks = factory_task::create::list(&store).expect("list");
    assert_eq!(
        tasks.len(),
        1,
        "only the schedule whose template is open may have produced a run: {tasks:?}"
    );
}

// Defect 1: attempting delivery after a fire --------------------------------
//
// `create_from_schedule` never sets `target_session_id` or
// `target_workspace_path` on a cron run, so `assign` always takes the
// untargeted branch below — every fixture in this section only needs an
// idle session with the right `scope_id`/`agent_name`, never a targeted one.

/// The acceptance path design §11 names: a fired run reaches an idle session
/// and is delivered. Mutation caught: `try_fire` returning `Fired` without
/// ever calling `assign`/`deliver` (the defect this task closes).
#[test]
fn a_fired_run_with_an_idle_session_is_delivered() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let mut store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();

    let template_id = create_template_with_agent(&mut store, uid(80), "tmpl-h", scope_id, "agent");
    let schedule_id = create_schedule(&mut store, uid(81), template_id, MATCHING_CRON);
    let session_id = uid(82);
    create_idle_session(
        &mut store,
        session_id,
        scope_id,
        "agent",
        "/workspace-h",
        Some("pane-h"),
    );

    let reports = dispatch::tick(&mut store, &adapter, fixture.instance_root(), instant());

    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].schedule_id, schedule_id);
    let task_id = match reports[0].outcome {
        FireOutcome::Fired { task_id } => task_id,
        ref other => panic!("expected Fired, got {other:?}"),
    };

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(
        task.status,
        factory_task::TaskStatus::Running,
        "a delivered run must be marked running"
    );
    assert_eq!(task.assigned_session_id, Some(session_id));

    let sends = adapter.send_calls();
    assert_eq!(
        sends.len(),
        1,
        "the prompt must reach the adapter exactly once"
    );
    assert_eq!(sends[0].0, "pane-h");
    assert_eq!(sends[0].1, task_id);
}

/// Backlog §11: "Unassigned, busy, and stopped target agents do not lose
/// queued work." No session at all exists for the template's agent here, so
/// the run stays `queued` — and a second tick (`AlreadyFired`) must not
/// somehow change that either, since `attempt_delivery` is only ever called
/// from the branch that actually created a row.
#[test]
fn a_fired_run_with_no_idle_session_stays_queued_across_two_ticks() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let mut store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();

    let template_id = create_template_with_agent(&mut store, uid(90), "tmpl-i", scope_id, "agent");
    let schedule_id = create_schedule(&mut store, uid(91), template_id, MATCHING_CRON);

    let first = dispatch::tick(&mut store, &adapter, fixture.instance_root(), instant());
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].schedule_id, schedule_id);
    let task_id = match first[0].outcome {
        FireOutcome::Fired { task_id } => task_id,
        ref other => panic!("expected Fired, got {other:?}"),
    };

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(task.status, factory_task::TaskStatus::Queued);
    assert_eq!(task.assigned_session_id, None);

    let second = dispatch::tick(&mut store, &adapter, fixture.instance_root(), instant());
    assert_eq!(second.len(), 1);
    assert_eq!(
        second[0].outcome,
        FireOutcome::AlreadyFired,
        "the same minute must not fire a second run: {:?}",
        second[0].outcome
    );

    let task_after = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(
        task_after.status,
        factory_task::TaskStatus::Queued,
        "still queued after a second tick"
    );
    assert_eq!(task_after.assigned_session_id, None);
    assert!(
        adapter.send_calls().is_empty(),
        "with no idle session, delivery must never be attempted at all"
    );
}

/// ADR 0021 decision 4a: "a refusal is an event, because station 10 decided
/// it is not a delivery." `AdapterError::SessionBusy` is the one `send`
/// error `AdapterPromptWriter` turns into a refusal rather than an ordinary
/// failure — the run must stay `queued`, `assign` must still have recorded
/// its own choice, and a `refused` event must exist.
#[test]
fn a_refused_delivery_leaves_the_run_queued_and_records_a_refused_event() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let mut store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();
    adapter.set_send_busy(true);

    let template_id = create_template_with_agent(&mut store, uid(100), "tmpl-j", scope_id, "agent");
    let schedule_id = create_schedule(&mut store, uid(101), template_id, MATCHING_CRON);
    let session_id = uid(102);
    create_idle_session(
        &mut store,
        session_id,
        scope_id,
        "agent",
        "/workspace-j",
        Some("pane-j"),
    );

    let reports = dispatch::tick(&mut store, &adapter, fixture.instance_root(), instant());
    assert_eq!(reports[0].schedule_id, schedule_id);
    let task_id = match reports[0].outcome {
        FireOutcome::Fired { task_id } => task_id,
        ref other => panic!("expected Fired, got {other:?}"),
    };

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(
        task.status,
        factory_task::TaskStatus::Queued,
        "a refusal must never advance status past queued"
    );
    assert_eq!(
        task.assigned_session_id,
        Some(session_id),
        "assign still records its own choice even though delivery was refused"
    );

    let events = factory_task::events::for_task(&store, task_id).expect("read events");
    assert!(
        events
            .iter()
            .any(|e| e.event_type == factory_task::events::EventType::Refused),
        "a refusal must be journalled as its own event: {events:?}"
    );
}

/// ADR 0021 decision 6, exercised the same way `crates/factory-daemon/tests/cost.rs`
/// exercises `ops::task::send`'s own baseline: a delivered run must carry a
/// `cost_baseline`, sampled after `mark_running` commits.
#[test]
fn a_delivered_cron_run_has_a_cost_baseline() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let mut store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();
    adapter.queue_cost_sample(
        "pane-k",
        Some(factory_adapter::CostSample {
            source: factory_adapter::CostSource::ClaudeCode,
            model: Some("claude-test".to_string()),
            input_tokens: 100,
            output_tokens: 20,
            duration_ms: Some(1_000),
            context_utilization_percent: Some(5.0),
        }),
    );

    let template_id = create_template_with_agent(&mut store, uid(110), "tmpl-k", scope_id, "agent");
    let schedule_id = create_schedule(&mut store, uid(111), template_id, MATCHING_CRON);
    create_idle_session(
        &mut store,
        uid(112),
        scope_id,
        "agent",
        "/workspace-k",
        Some("pane-k"),
    );

    let reports = dispatch::tick(&mut store, &adapter, fixture.instance_root(), instant());
    assert_eq!(reports[0].schedule_id, schedule_id);
    let task_id = match reports[0].outcome {
        FireOutcome::Fired { task_id } => task_id,
        ref other => panic!("expected Fired, got {other:?}"),
    };

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(task.status, factory_task::TaskStatus::Running);
    assert!(
        task.cost_baseline.is_some(),
        "a delivered cron run must sample a cost baseline just like ops::task::send does"
    );
}

/// One schedule's *delivery* failure (as opposed to its *fire*, already
/// covered above) must not stop the next schedule in the same tick. `alpha`'s
/// idle session has no recorded pane at all, so `AdapterPromptWriter` refuses
/// before ever calling the adapter; `beta`'s has a real pane and succeeds.
/// Two scopes, each with their own "agent", so `assign` cannot cross-wire the
/// two sessions.
#[test]
fn one_schedules_delivery_failure_does_not_stop_the_next_schedule_in_the_same_tick() {
    let fixture = common::build(&[
        common::ScopeSpec::new("alpha", "alpha"),
        common::ScopeSpec::new("beta", "beta"),
    ]);
    let alpha = fixture.scope("alpha");
    let beta = fixture.scope("beta");
    let mut store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();

    let alpha_template =
        create_template_with_agent(&mut store, uid(120), "tmpl-l1", alpha, "agent");
    let alpha_schedule = create_schedule(&mut store, uid(121), alpha_template, MATCHING_CRON);
    create_idle_session(&mut store, uid(122), alpha, "agent", "/workspace-l1", None);

    let beta_template = create_template_with_agent(&mut store, uid(123), "tmpl-l2", beta, "agent");
    let beta_schedule = create_schedule(&mut store, uid(124), beta_template, MATCHING_CRON);
    create_idle_session(
        &mut store,
        uid(125),
        beta,
        "agent",
        "/workspace-l2",
        Some("pane-l2"),
    );

    let reports = dispatch::tick(&mut store, &adapter, fixture.instance_root(), instant());
    assert_eq!(reports.len(), 2);
    for report in &reports {
        assert!(
            matches!(report.outcome, FireOutcome::Fired { .. }),
            "both schedules must still fire regardless of what delivery does: {report:?}"
        );
    }

    let alpha_task_id = reports
        .iter()
        .find(|r| r.schedule_id == alpha_schedule)
        .map(|r| match r.outcome {
            FireOutcome::Fired { task_id } => task_id,
            _ => unreachable!(),
        })
        .expect("alpha schedule reported");
    let beta_task_id = reports
        .iter()
        .find(|r| r.schedule_id == beta_schedule)
        .map(|r| match r.outcome {
            FireOutcome::Fired { task_id } => task_id,
            _ => unreachable!(),
        })
        .expect("beta schedule reported");

    let alpha_task = factory_task::create::show(&store, alpha_task_id).expect("show alpha");
    assert_eq!(
        alpha_task.status,
        factory_task::TaskStatus::Queued,
        "the paneless session refuses delivery, so alpha's run stays queued"
    );

    let beta_task = factory_task::create::show(&store, beta_task_id).expect("show beta");
    assert_eq!(
        beta_task.status,
        factory_task::TaskStatus::Running,
        "beta's delivery must succeed independently of alpha's"
    );
}

// Defect 2: a paused template is skipped, not failed -------------------------

/// Backlog acceptance: a schedule whose template is `paused` creates no run
/// and is not reported as a failure, and starts firing again once the
/// template is set back to `open`. `last_fired_at` staying `None` across the
/// skipped tick pins that the skip happens *before* `create_from_schedule` —
/// not after, with the run then discarded.
#[test]
fn a_paused_templates_schedule_creates_no_run_and_fires_again_once_reopened() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let mut store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();

    let template_id = create_template(&mut store, uid(140), "tmpl-m", scope_id);
    let schedule_id = create_schedule(&mut store, uid(141), template_id, MATCHING_CRON);
    factory_task::template::set_state(
        &mut store,
        template_id,
        factory_task::template::TemplateState::Paused,
    )
    .expect("pause");

    let reports = dispatch::tick(&mut store, &adapter, fixture.instance_root(), instant());
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].schedule_id, schedule_id);
    assert_eq!(
        reports[0].outcome,
        FireOutcome::TemplateNotOpen {
            state: factory_task::template::TemplateState::Paused
        }
    );
    assert!(
        factory_task::create::list(&store).expect("list").is_empty(),
        "a paused template must create no run"
    );
    let after_skip = factory_task::schedule::get(&store, schedule_id).expect("get");
    assert_eq!(
        after_skip.last_fired_at, None,
        "a skipped fire must not stamp last_fired_at — the skip happens before \
         create_from_schedule, not as a discard afterwards"
    );

    factory_task::template::set_state(
        &mut store,
        template_id,
        factory_task::template::TemplateState::Open,
    )
    .expect("reopen");

    let reports_after_reopen =
        dispatch::tick(&mut store, &adapter, fixture.instance_root(), instant());
    assert_eq!(reports_after_reopen.len(), 1);
    assert!(
        matches!(reports_after_reopen[0].outcome, FireOutcome::Fired { .. }),
        "reopening the template must let the same due minute fire: {:?}",
        reports_after_reopen[0].outcome
    );
    assert_eq!(
        factory_task::create::list(&store).expect("list").len(),
        1,
        "exactly one run must exist once the template is reopened"
    );
}

/// A template naming an agent the instance configuration does not declare —
/// a template and the config can drift independently, since nothing
/// validates one against the other once the template exists. Resolving
/// `max_sessions` for delivery must fail no worse than
/// `Assignment::Deferred` does: the run stays `queued`, and — the point of
/// this test — the tick is not aborted, so an ordinary schedule due in the
/// same tick still fires and still gets delivered.
///
/// The unconfigured agent is given its own idle, paned session so that
/// staying `queued` cannot be a coincidence of "no idle session either way"
/// — `assign_untargeted` does not care whether a name is configured, only
/// whether a matching session row exists. Mutation caught: resolving
/// `max_sessions` via a hard-coded placeholder instead of loading the
/// config, which skips `find_agent` entirely and lets this session receive
/// the run.
#[test]
fn a_template_naming_an_unconfigured_agent_fires_but_is_not_delivered() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let mut store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();

    let ghost_template =
        create_template_with_agent(&mut store, uid(150), "tmpl-n1", scope_id, "ghost-agent");
    let ghost_schedule = create_schedule(&mut store, uid(151), ghost_template, MATCHING_CRON);
    // An idle session under the unconfigured name, written by raw SQL (the
    // same way `create_idle_session` always bypasses config) — deliberately
    // NOT gated by whether "ghost-agent" is declared anywhere. This is the
    // fixture that actually proves config resolution runs: `assign_untargeted`
    // would happily hand this session the task if delivery ever reached it,
    // so the run staying `queued` below can only be `find_agent` refusing
    // the name *before* `assign` is ever called — not an accidental "no idle
    // session" outcome that would hold even without resolving the config at
    // all.
    create_idle_session(
        &mut store,
        uid(155),
        scope_id,
        "ghost-agent",
        "/workspace-ghost",
        Some("pane-ghost"),
    );

    let ok_template =
        create_template_with_agent(&mut store, uid(152), "tmpl-n2", scope_id, "agent");
    let ok_schedule = create_schedule(&mut store, uid(153), ok_template, MATCHING_CRON);
    create_idle_session(
        &mut store,
        uid(154),
        scope_id,
        "agent",
        "/workspace-n",
        Some("pane-n"),
    );

    let reports = dispatch::tick(&mut store, &adapter, fixture.instance_root(), instant());
    assert_eq!(reports.len(), 2);
    for report in &reports {
        assert!(
            matches!(report.outcome, FireOutcome::Fired { .. }),
            "an unconfigured agent must not stop either schedule from firing: {report:?}"
        );
    }

    let ghost_task_id = reports
        .iter()
        .find(|r| r.schedule_id == ghost_schedule)
        .map(|r| match r.outcome {
            FireOutcome::Fired { task_id } => task_id,
            _ => unreachable!(),
        })
        .expect("ghost schedule reported");
    let ok_task_id = reports
        .iter()
        .find(|r| r.schedule_id == ok_schedule)
        .map(|r| match r.outcome {
            FireOutcome::Fired { task_id } => task_id,
            _ => unreachable!(),
        })
        .expect("ok schedule reported");

    let ghost_task = factory_task::create::show(&store, ghost_task_id).expect("show ghost");
    assert_eq!(
        ghost_task.status,
        factory_task::TaskStatus::Queued,
        "an agent the config does not declare leaves the run queued, not failed"
    );
    assert_eq!(ghost_task.assigned_session_id, None);
    assert!(
        adapter
            .send_calls()
            .iter()
            .all(|(pane, _, _)| pane != "pane-ghost"),
        "the ghost session must never receive the prompt: {:?}",
        adapter.send_calls()
    );

    let ok_task = factory_task::create::show(&store, ok_task_id).expect("show ok");
    assert_eq!(
        ok_task.status,
        factory_task::TaskStatus::Running,
        "the next schedule's delivery must succeed independently of the unconfigured one"
    );
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
