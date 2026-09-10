//! `factory_task::schedule`'s own test suite: the matching predicate, the
//! next-run preview, and CRUD on `schedules` (design §11; ADR 0021 decisions
//! 3, 3a, 7).
//!
//! See `tests/template.rs`'s own module docs for why a fixture here creates
//! its `task_templates` row through `factory_task::template::create` rather
//! than this crate owning a second path to the same table.

use chrono::{TimeZone, Utc};
use factory_store::Store;
use factory_task::schedule::{self, ScheduleError};
use factory_task::template::create as create_template;

/// A deterministic, distinct, syntactically valid UUID — mirrors
/// `tests/template.rs::uid`. `uuid` is pinned workspace-wide without the
/// `v4` feature, so tests build UUIDs by hand from a seed.
fn uid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

/// A minimal, valid `task_templates` row for a schedule's `template_id` to
/// reference. What the template actually says is irrelevant to every test in
/// this file — `schedule` never reads a template's `prompt`.
fn seed_template(store: &mut Store, seed: u32, name: &str) -> uuid::Uuid {
    let id = uid(seed);
    create_template(store, id, name, None, None, "do it", None).expect("create template");
    id
}

// create: validation ------------------------------------------------------

/// A malformed expression is refused at create time, with a typed error that
/// carries croner's own message — worth showing an operator, per ADR 0021's
/// consequences section.
#[test]
fn create_refuses_a_malformed_cron_expression_with_croners_own_message() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let template_id = seed_template(&mut store, 1, "nightly-report");

    let err = schedule::create(&mut store, uid(50), template_id, "* * *", "UTC")
        .expect_err("a 3-field expression is not a valid five-field cron pattern");
    assert!(
        matches!(&err, ScheduleError::InvalidCron { cron, .. } if cron == "* * *"),
        "expected InvalidCron, got {err:?}"
    );
    assert!(
        err.to_string()
            .contains("Pattern must have between 5 and 7 fields"),
        "croner's own message must reach the operator; got {err}"
    );

    assert!(
        schedule::list(&store).expect("list").is_empty(),
        "a refused create must not have written a row"
    );
}

/// An unknown IANA timezone is refused at create time, the same way a
/// malformed cron expression is.
#[test]
fn create_refuses_an_unknown_iana_timezone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let template_id = seed_template(&mut store, 1, "nightly-report");

    let err = schedule::create(
        &mut store,
        uid(50),
        template_id,
        "30 2 * * *",
        "Mars/Colony",
    )
    .expect_err("Mars/Colony is not an IANA timezone");
    assert!(
        matches!(&err, ScheduleError::InvalidTimezone(tz) if tz == "Mars/Colony"),
        "expected InvalidTimezone, got {err:?}"
    );

    assert!(
        schedule::list(&store).expect("list").is_empty(),
        "a refused create must not have written a row"
    );
}

// matches -------------------------------------------------------------

#[test]
fn matches_the_right_minute_and_not_its_neighbours() {
    let target = Utc.with_ymd_and_hms(2027, 1, 1, 10, 15, 0).unwrap();
    let minute_before = Utc.with_ymd_and_hms(2027, 1, 1, 10, 14, 0).unwrap();
    let minute_after = Utc.with_ymd_and_hms(2027, 1, 1, 10, 16, 0).unwrap();

    assert!(schedule::matches("15 10 * * *", "UTC", target).expect("matches"));
    assert!(!schedule::matches("15 10 * * *", "UTC", minute_before).expect("matches"));
    assert!(!schedule::matches("15 10 * * *", "UTC", minute_after).expect("matches"));
}

#[test]
fn a_weekday_restricted_expression_does_not_match_a_weekend() {
    // 2027-01-04 is a Monday; 2027-01-09 is the following Saturday.
    let monday = Utc.with_ymd_and_hms(2027, 1, 4, 9, 0, 0).unwrap();
    let saturday = Utc.with_ymd_and_hms(2027, 1, 9, 9, 0, 0).unwrap();

    assert!(schedule::matches("0 9 * * 1-5", "UTC", monday).expect("matches"));
    assert!(!schedule::matches("0 9 * * 1-5", "UTC", saturday).expect("matches"));
}

/// A schedule fires on its own timezone's local time, not the machine
/// running the test's. Both instants below are computed by hand from a fixed
/// UTC offset (JST is UTC+9 year-round, no daylight saving), so the
/// assertion cannot depend on what timezone this machine happens to be in —
/// there is no `Local` anywhere in this test.
#[test]
fn a_schedule_fires_on_its_own_timezone_not_the_machines() {
    let nine_am_in_tokyo = Utc.with_ymd_and_hms(2027, 1, 4, 0, 0, 0).unwrap();
    let nine_am_in_utc = Utc.with_ymd_and_hms(2027, 1, 4, 9, 0, 0).unwrap();

    assert!(schedule::matches("0 9 * * *", "Asia/Tokyo", nine_am_in_tokyo).expect("matches"));
    assert!(
        !schedule::matches("0 9 * * *", "Asia/Tokyo", nine_am_in_utc).expect("matches"),
        "09:00 UTC is 18:00 in Tokyo, not 09:00"
    );
}

// Spring forward / autumn back — ADR 0021 decision 3a ----------------------

/// **The load-bearing test.** 2027-03-28 is Europe/Berlin's spring-forward
/// day: local time jumps from 02:00 straight to 03:00, so local 02:30 never
/// happens. `matches` must say no for every minute of that local day. A
/// scheduler built on `find_next_occurrence` instead would report a match at
/// 03:00 — this is the test that catches that swap.
#[test]
fn spring_forward_2027_03_28_never_matches_any_minute() {
    let cron = "30 2 * * *";
    let tz = "Europe/Berlin";

    // 23:00 UTC on 2027-03-27 through 23:59 UTC on 2027-03-28 comfortably
    // covers every UTC instant that can render as part of the local
    // calendar day 2027-03-28 CET/CEST.
    let start = Utc.with_ymd_and_hms(2027, 3, 27, 23, 0, 0).unwrap();
    for i in 0..(25 * 60) {
        let instant = start + chrono::Duration::minutes(i);
        assert!(
            !schedule::matches(cron, tz, instant).expect("matches"),
            "30 2 * * * must not match any instant on the spring-forward day, but matched {instant}"
        );
    }
}

/// The next-run preview walks past the spring-forward gap to the following
/// day's 02:30, not to 03:00 on the 28th — the same fact as the test above,
/// seen through `next_run` rather than `matches`.
#[test]
fn next_run_preview_skips_the_spring_forward_gap_to_the_following_day() {
    let after = Utc.with_ymd_and_hms(2027, 3, 27, 23, 0, 0).unwrap();
    let next = schedule::next_run("30 2 * * *", "Europe/Berlin", after)
        .expect("next_run")
        .expect("a match exists well within the horizon");
    assert_eq!(
        next.format("%Y-%m-%d %H:%M").to_string(),
        "2027-03-29 02:30",
        "the skipped 2027-03-28 02:30 must not become a 03:00 match on the 28th"
    );
}

/// **The other load-bearing test.** 2027-10-31 is Europe/Berlin's
/// autumn-back day: local 02:00-02:59 happens twice, an hour apart in UTC
/// (00:30 UTC is 02:30 CEST; 01:30 UTC is 02:30 CET). Both instants must
/// match, and both must render the identical `fired_for_minute` string —
/// that string equality, not any dispatcher logic, is what lets
/// `tasks_one_run_per_schedule_minute` admit exactly one run for the pair.
#[test]
fn autumn_back_2027_10_31_matches_both_instants_with_the_same_local_minute_string() {
    let cron = "30 2 * * *";
    let tz = "Europe/Berlin";
    let first = Utc.with_ymd_and_hms(2027, 10, 31, 0, 30, 0).unwrap();
    let second = Utc.with_ymd_and_hms(2027, 10, 31, 1, 30, 0).unwrap();
    assert_ne!(
        first, second,
        "these must be two distinct instants an hour apart"
    );

    assert!(schedule::matches(cron, tz, first).expect("matches"));
    assert!(schedule::matches(cron, tz, second).expect("matches"));

    let first_str = schedule::local_minute_string(tz, first).expect("local_minute_string");
    let second_str = schedule::local_minute_string(tz, second).expect("local_minute_string");
    assert_eq!(first_str, "2027-10-31T02:30");
    assert_eq!(
        first_str, second_str,
        "both instants must render the same fired_for_minute string, or the unique \
         index over (schedule_id, fired_for_minute) could not admit exactly one run"
    );
}

// next_run horizon ---------------------------------------------------------

/// An expression that can never fire (the 30th of February) returns "no run
/// inside the horizon" rather than hanging.
#[test]
fn next_run_preview_gives_up_at_the_horizon_without_hanging() {
    let after = Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap();
    let next = schedule::next_run("0 0 30 2 *", "UTC", after).expect("next_run");
    assert_eq!(
        next, None,
        "an expression that can never fire must report no run inside the horizon"
    );
}

// CRUD ----------------------------------------------------------------

#[test]
fn create_get_list_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let template_id = seed_template(&mut store, 1, "nightly-report");

    let schedule_id = uid(50);
    schedule::create(
        &mut store,
        schedule_id,
        template_id,
        "30 2 * * *",
        "Europe/Berlin",
    )
    .expect("create");

    let got = schedule::get(&store, schedule_id).expect("get");
    assert_eq!(got.id, schedule_id);
    assert_eq!(got.template_id, template_id);
    assert_eq!(got.cron, "30 2 * * *");
    assert_eq!(got.timezone, "Europe/Berlin");
    assert!(
        got.enabled,
        "schedules start enabled — the schema's own DEFAULT 1"
    );
    assert_eq!(got.last_fired_at, None);

    let all = schedule::list(&store).expect("list");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].id, schedule_id);
}

#[test]
fn get_of_a_nonexistent_schedule_is_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path()).expect("open");

    let err = schedule::get(&store, uid(999)).expect_err("no such schedule exists");
    assert!(matches!(err, ScheduleError::NotFound(id) if id == uid(999)));
}

/// The dispatcher's own query: only an *enabled* schedule whose expression
/// matches the given instant comes back. A disabled schedule that would
/// otherwise match, and an enabled schedule that does not match, are both
/// excluded.
#[test]
fn due_returns_only_enabled_schedules_matching_the_instant() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let template_id = seed_template(&mut store, 1, "nightly-report");

    let matching_enabled = uid(1);
    schedule::create(
        &mut store,
        matching_enabled,
        template_id,
        "15 10 * * *",
        "UTC",
    )
    .expect("create matching, enabled");

    let matching_disabled = uid(2);
    schedule::create(
        &mut store,
        matching_disabled,
        template_id,
        "15 10 * * *",
        "UTC",
    )
    .expect("create matching, to be disabled");
    schedule::disable(&mut store, matching_disabled).expect("disable");

    let non_matching = uid(3);
    schedule::create(&mut store, non_matching, template_id, "16 10 * * *", "UTC")
        .expect("create non-matching, enabled");

    let instant = Utc.with_ymd_and_hms(2027, 1, 1, 10, 15, 0).unwrap();
    let due = schedule::due(&store, instant).expect("due");
    let ids: Vec<uuid::Uuid> = due.iter().map(|s| s.id).collect();
    assert_eq!(
        ids,
        vec![matching_enabled],
        "only the enabled schedule whose expression matches this instant is due"
    );
}

/// Backlog §11: enable/disable "toggle a schedule without deleting its
/// history or affecting already-created runs." Only `enabled` (and
/// `updated_at`) may change; `cron`, `timezone`, `last_fired_at` and
/// `created_at` must read back exactly as they were.
#[test]
fn enable_disable_toggle_only_enabled_and_leave_everything_else_untouched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let template_id = seed_template(&mut store, 1, "nightly-report");

    let schedule_id = uid(50);
    schedule::create(
        &mut store,
        schedule_id,
        template_id,
        "30 2 * * *",
        "Europe/Berlin",
    )
    .expect("create");
    let before = schedule::get(&store, schedule_id).expect("get");

    schedule::disable(&mut store, schedule_id).expect("disable");
    let disabled = schedule::get(&store, schedule_id).expect("get");
    assert!(!disabled.enabled);
    assert_eq!(disabled.cron, before.cron);
    assert_eq!(disabled.timezone, before.timezone);
    assert_eq!(disabled.last_fired_at, before.last_fired_at);
    assert_eq!(disabled.created_at, before.created_at);

    schedule::enable(&mut store, schedule_id).expect("enable");
    let enabled = schedule::get(&store, schedule_id).expect("get");
    assert!(enabled.enabled);
    assert_eq!(enabled.cron, before.cron);
    assert_eq!(enabled.timezone, before.timezone);
    assert_eq!(enabled.last_fired_at, before.last_fired_at);
    assert_eq!(enabled.created_at, before.created_at);
}

#[test]
fn enable_of_a_nonexistent_schedule_is_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let err = schedule::enable(&mut store, uid(999)).expect_err("no such schedule exists");
    assert!(matches!(err, ScheduleError::NotFound(id) if id == uid(999)));
}

#[test]
fn disable_of_a_nonexistent_schedule_is_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let err = schedule::disable(&mut store, uid(999)).expect_err("no such schedule exists");
    assert!(matches!(err, ScheduleError::NotFound(id) if id == uid(999)));
}

// The horizon's contract ---------------------------------------------------

/// A leap-day schedule is an ordinary schedule, and the preview must answer
/// for it.
///
/// This is the case that moved [`schedule::NEXT_RUN_HORIZON_DAYS`] from 400
/// to 1500. From 2027-01-01 the next 29 February is 2028-02-29, 424 days out.
/// At the old bound `next_run` answered `None`, which `factory schedule list`
/// would render as a blank next-run column for a schedule that works
/// perfectly, with nothing to tell an operator that apart from a broken one.
///
/// Mutation caught: lowering the horizon back below 424 days.
#[test]
fn next_run_answers_for_a_leap_day_schedule_rather_than_giving_up() {
    let after = Utc
        .with_ymd_and_hms(2027, 1, 1, 12, 0, 0)
        .single()
        .expect("instant");

    let next = schedule::next_run("0 0 29 2 *", "Europe/Berlin", after)
        .expect("a valid expression must not error")
        .expect("29 February is a real date and the preview must find it");

    assert_eq!(
        next.format("%Y-%m-%d %H:%M").to_string(),
        "2028-02-29 00:00",
        "the next 29 February after 2027-01-01 is in 2028"
    );
}

/// The other half of the same contract, and the reason it is worth having:
/// after the horizon change, `None` means one thing only. `0 0 30 2 *` names
/// a date that does not exist in any year, so no horizon would help it.
///
/// Mutation caught: removing the horizon entirely, which turns this from an
/// answer into a hang.
#[test]
fn next_run_is_none_only_for_an_expression_that_can_never_fire() {
    let after = Utc
        .with_ymd_and_hms(2027, 1, 1, 12, 0, 0)
        .single()
        .expect("instant");

    assert_eq!(
        schedule::next_run("0 0 30 2 *", "Europe/Berlin", after).expect("must not error"),
        None,
        "30 February never happens, so the walk must give up and say so"
    );
}

// mark_fired ---------------------------------------------------------------

/// `mark_fired` takes the caller's own transaction, because ADR 0021
/// decision 3 requires the run insert and this column to commit together.
/// Proven here the way that matters: a transaction that is rolled back must
/// leave `last_fired_at` untouched. If this function opened its own
/// transaction instead, the stamp would survive a rolled-back run and record
/// a firing that did not happen.
#[test]
fn mark_fired_is_rolled_back_with_the_transaction_that_called_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let template_id = seed_template(&mut store, 1, "nightly-report");
    let schedule_id = uid(50);
    schedule::create(
        &mut store,
        schedule_id,
        template_id,
        "0 3 * * *",
        "Europe/Berlin",
    )
    .expect("create schedule");

    let fired_at = Utc
        .with_ymd_and_hms(2026, 9, 10, 1, 0, 0)
        .single()
        .expect("instant");
    {
        let tx = store.transaction().expect("begin");
        schedule::mark_fired(&tx, schedule_id, fired_at).expect("mark fired");
        // Deliberately no commit: `Transaction` rolls back when dropped.
    }

    let after = schedule::get(&store, schedule_id).expect("get");
    assert_eq!(
        after.last_fired_at, None,
        "a rolled-back transaction must leave the schedule looking like it never fired"
    );
}

/// The committed path, and the one column it may touch. `enable`/`disable`
/// have their own test for the same property; this is `mark_fired`'s.
#[test]
fn mark_fired_writes_only_last_fired_at() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let template_id = seed_template(&mut store, 1, "nightly-report");
    let schedule_id = uid(50);
    schedule::create(
        &mut store,
        schedule_id,
        template_id,
        "0 3 * * *",
        "Europe/Berlin",
    )
    .expect("create schedule");
    let before = schedule::get(&store, schedule_id).expect("get before");

    let fired_at = Utc
        .with_ymd_and_hms(2026, 9, 10, 1, 0, 0)
        .single()
        .expect("instant");
    {
        let tx = store.transaction().expect("begin");
        schedule::mark_fired(&tx, schedule_id, fired_at).expect("mark fired");
        tx.commit().expect("commit");
    }

    let after = schedule::get(&store, schedule_id).expect("get after");
    assert!(after.last_fired_at.is_some(), "the stamp must be recorded");
    assert_eq!(after.cron, before.cron, "cron must not move");
    assert_eq!(after.timezone, before.timezone, "timezone must not move");
    assert_eq!(after.enabled, before.enabled, "enabled must not move");
    assert_eq!(
        after.template_id, before.template_id,
        "template_id must not move"
    );
    assert_eq!(
        after.created_at, before.created_at,
        "created_at must not move"
    );
}

/// A stamp for a schedule that does not exist is a typed error, not a
/// silently ignored UPDATE that matched no rows.
#[test]
fn mark_fired_on_a_nonexistent_schedule_is_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let missing = uid(999);

    let tx = store.transaction().expect("begin");
    let err = schedule::mark_fired(&tx, missing, Utc::now())
        .expect_err("stamping a missing schedule must be an error");
    assert!(matches!(err, ScheduleError::NotFound(id) if id == missing));
}
