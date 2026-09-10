//! `schedule.create`, `schedule.list`, `schedule.enable`, `schedule.disable`
//! (design §7, §11; ADR 0021 decisions 1, 3, 3a, 11).

mod common;

use factory_daemon::Handler as _;
use factory_daemon::envelope::{CommandRequest, QueryRequest};

fn uid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

fn cmd(scope_id: uuid::Uuid, command: &str, payload: serde_json::Value) -> CommandRequest {
    CommandRequest {
        request_id: uid(9500),
        scope_id,
        command: command.to_string(),
        payload,
        expected_revision: None,
    }
}

fn qry(scope_id: uuid::Uuid, query: &str, payload: serde_json::Value) -> QueryRequest {
    QueryRequest {
        request_id: uid(9501),
        scope_id,
        query: query.to_string(),
        payload,
    }
}

fn new_handler(fixture: &common::Fixture) -> factory_daemon::FactoryHandler {
    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();
    factory_daemon::FactoryHandler::new(store, adapter, fixture.instance_root())
}

/// A `schedule.create` command for the `--task "<prompt>" --name
/// <template-name>` form.
fn create_with_new_template(
    schedule_id: uuid::Uuid,
    template_id: uuid::Uuid,
    name: &str,
    cron: &str,
    timezone: &str,
) -> serde_json::Value {
    serde_json::json!({
        "schedule_id": schedule_id.to_string(),
        "template_id": template_id.to_string(),
        "name": name,
        "task": "do it",
        "cron": cron,
        "timezone": timezone,
    })
}

// --- create: the two forms ------------------------------------------------

#[test]
fn create_with_task_and_name_creates_the_template_and_schedule_together() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let handler = new_handler(&fixture);

    let schedule_id = uid(1001);
    let template_id = uid(1002);
    let created = handler
        .handle_command(cmd(
            scope_id,
            "schedule.create",
            create_with_new_template(
                schedule_id,
                template_id,
                "nightly-report",
                "0 9 * * 1-5",
                "Europe/Berlin",
            ),
        ))
        .expect("schedule.create succeeds");

    assert_eq!(created.result["id"], schedule_id.to_string());
    assert_eq!(created.result["template_id"], template_id.to_string());
    assert_eq!(created.result["template_name"], "nightly-report");
    assert_eq!(created.result["cron"], "0 9 * * 1-5");
    assert_eq!(created.result["timezone"], "Europe/Berlin");
    assert_eq!(created.result["enabled"], true);
}

#[test]
fn create_with_template_uses_the_existing_open_template() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let handler = new_handler(&fixture);

    let template_id = uid(1102);
    handler
        .handle_command(cmd(
            scope_id,
            "schedule.create",
            create_with_new_template(uid(1101), template_id, "weekly-status", "0 9 * * 1", "UTC"),
        ))
        .expect("first schedule.create succeeds");

    let second_schedule_id = uid(1103);
    let created = handler
        .handle_command(cmd(
            scope_id,
            "schedule.create",
            serde_json::json!({
                "schedule_id": second_schedule_id.to_string(),
                "template_name": "weekly-status",
                "cron": "0 10 * * 3",
                "timezone": "UTC",
            }),
        ))
        .expect("schedule.create against --template succeeds");

    assert_eq!(created.result["id"], second_schedule_id.to_string());
    assert_eq!(created.result["template_id"], template_id.to_string());
    assert_eq!(created.result["template_name"], "weekly-status");
    assert_eq!(created.result["cron"], "0 10 * * 3");
}

#[test]
fn create_refuses_both_forms_together_and_neither_form() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let handler = new_handler(&fixture);

    let both = handler
        .handle_command(cmd(
            scope_id,
            "schedule.create",
            serde_json::json!({
                "schedule_id": uid(1201).to_string(),
                "cron": "0 9 * * *",
                "timezone": "UTC",
                "template_name": "whatever",
                "template_id": uid(1202).to_string(),
                "name": "n",
                "task": "t",
            }),
        ))
        .unwrap_err();
    assert_eq!(both.code, "validation.conflicting_fields");

    let neither = handler
        .handle_command(cmd(
            scope_id,
            "schedule.create",
            serde_json::json!({
                "schedule_id": uid(1203).to_string(),
                "cron": "0 9 * * *",
                "timezone": "UTC",
            }),
        ))
        .unwrap_err();
    assert_eq!(neither.code, "validation.missing_field");
}

/// ADR 0021 decision 11's own reasoning, proved through the command: if the
/// schedule insert fails, the template insert this call made first, in the
/// same transaction, must not survive either.
#[test]
fn create_with_new_template_is_atomic_when_the_schedule_insert_fails() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let handler = new_handler(&fixture);

    let existing_schedule_id = uid(1301);
    handler
        .handle_command(cmd(
            scope_id,
            "schedule.create",
            create_with_new_template(
                existing_schedule_id,
                uid(1302),
                "existing-template",
                "0 9 * * *",
                "UTC",
            ),
        ))
        .expect("seed an existing schedule to collide with");

    // Reuse `existing_schedule_id`: the new template inserts first inside
    // the same transaction, then the schedule insert collides on
    // `schedules`' own primary key and the whole transaction rolls back.
    let err = handler
        .handle_command(cmd(
            scope_id,
            "schedule.create",
            create_with_new_template(
                existing_schedule_id,
                uid(1303),
                "should-not-survive",
                "0 10 * * *",
                "UTC",
            ),
        ))
        .unwrap_err();
    assert_eq!(err.code, "internal.store_error");

    // The template must not have been left behind: naming it by
    // `--template` must fail as "no such template", never succeed.
    let lookup = handler
        .handle_command(cmd(
            scope_id,
            "schedule.create",
            serde_json::json!({
                "schedule_id": uid(1304).to_string(),
                "template_name": "should-not-survive",
                "cron": "0 11 * * *",
                "timezone": "UTC",
            }),
        ))
        .unwrap_err();
    assert_eq!(lookup.code, "not_found.template");
}

#[test]
fn a_refused_cron_expression_and_a_refused_timezone_each_name_what_was_wrong() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let handler = new_handler(&fixture);

    let bad_cron = handler
        .handle_command(cmd(
            scope_id,
            "schedule.create",
            create_with_new_template(uid(1401), uid(1402), "t1", "* * *", "UTC"),
        ))
        .unwrap_err();
    assert_eq!(bad_cron.code, "validation.invalid_cron");
    assert!(
        bad_cron.message.contains("* * *"),
        "the message must name the bad expression: {}",
        bad_cron.message
    );

    let bad_tz = handler
        .handle_command(cmd(
            scope_id,
            "schedule.create",
            create_with_new_template(uid(1403), uid(1404), "t2", "0 9 * * *", "Not/AZone"),
        ))
        .unwrap_err();
    assert_eq!(bad_tz.code, "validation.invalid_timezone");
    assert!(
        bad_tz.message.contains("Not/AZone"),
        "the message must name the bad timezone: {}",
        bad_tz.message
    );
}

/// The same refusal, reached through the **other** form of `create`.
///
/// Found by mutation, not by review: deleting `validate` from
/// `create_with_new_template` alone killed
/// `a_refused_cron_expression_and_a_refused_timezone_each_name_what_was_wrong`
/// above, but deleting it from `create_for_named_template` alone killed
/// nothing in the whole workspace. Both forms reach the same one home for
/// the rule, and a test that only walks one of the two roads cannot say so.
///
/// The cost of the gap is not theoretical: an unvalidated expression is
/// stored, `schedule::due` then fails to read that row on every tick, and
/// `log_failed_fires` names it every twenty seconds — while the operator who
/// typed it got a success back.
#[test]
fn a_refused_cron_expression_is_refused_for_an_existing_template_too() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let handler = new_handler(&fixture);

    handler
        .handle_command(cmd(
            scope_id,
            "schedule.create",
            create_with_new_template(uid(1451), uid(1452), "hourly-sweep", "0 * * * *", "UTC"),
        ))
        .expect("the first schedule.create succeeds");

    let bad_cron = handler
        .handle_command(cmd(
            scope_id,
            "schedule.create",
            serde_json::json!({
                "schedule_id": uid(1453).to_string(),
                "template_name": "hourly-sweep",
                "cron": "* * *",
                "timezone": "UTC",
            }),
        ))
        .unwrap_err();
    assert_eq!(bad_cron.code, "validation.invalid_cron");
    assert!(
        bad_cron.message.contains("* * *"),
        "the message must name the bad expression: {}",
        bad_cron.message
    );

    let bad_tz = handler
        .handle_command(cmd(
            scope_id,
            "schedule.create",
            serde_json::json!({
                "schedule_id": uid(1454).to_string(),
                "template_name": "hourly-sweep",
                "cron": "0 9 * * *",
                "timezone": "Not/AZone",
            }),
        ))
        .unwrap_err();
    assert_eq!(bad_tz.code, "validation.invalid_timezone");
    assert!(
        bad_tz.message.contains("Not/AZone"),
        "the message must name the bad timezone: {}",
        bad_tz.message
    );
}

/// The coordinator's own gate on top of ADR 0021 decision 11 (see
/// `factory_task::schedule::ScheduleError::TemplateNotOpen`'s doc comment):
/// `--template` against a template that is not `open` is refused, naming
/// the state.
///
/// Pauses the template directly through `factory_task::template::set_state`
/// rather than through a daemon command — no `template.*` operation exists
/// in this station's own scope, so this is the only way to reach the state.
#[test]
fn a_template_that_is_not_open_is_refused_and_the_message_names_its_state() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let handler = new_handler(&fixture);

    let template_id = uid(1501);
    handler
        .handle_command(cmd(
            scope_id,
            "schedule.create",
            create_with_new_template(
                uid(1502),
                template_id,
                "paused-template",
                "0 9 * * *",
                "UTC",
            ),
        ))
        .expect("seed a template through the new-template form");

    {
        let mut store = factory_store::Store::open(fixture.instance_root()).expect("open store");
        factory_task::template::set_state(
            &mut store,
            template_id,
            factory_task::template::TemplateState::Paused,
        )
        .expect("pause it");
    }

    let err = handler
        .handle_command(cmd(
            scope_id,
            "schedule.create",
            serde_json::json!({
                "schedule_id": uid(1503).to_string(),
                "template_name": "paused-template",
                "cron": "0 10 * * *",
                "timezone": "UTC",
            }),
        ))
        .unwrap_err();
    assert_eq!(err.code, "conflict.template_not_open");
    assert!(
        err.message.contains("paused"),
        "the message must name the state an operator would have to fix: {}",
        err.message
    );
}

// --- list ------------------------------------------------------------------

fn find_row(schedules: &[serde_json::Value], id: uuid::Uuid) -> &serde_json::Value {
    schedules
        .iter()
        .find(|s| s["id"] == id.to_string())
        .unwrap_or_else(|| panic!("schedule {id} not in `schedule.list`'s own result"))
}

#[test]
fn list_shows_the_next_run_and_something_readable_for_a_disabled_schedule() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let handler = new_handler(&fixture);

    let enabled_id = uid(1601);
    handler
        .handle_command(cmd(
            scope_id,
            "schedule.create",
            create_with_new_template(enabled_id, uid(1602), "enabled-one", "0 9 * * *", "UTC"),
        ))
        .expect("create enabled schedule");

    let disabled_id = uid(1603);
    handler
        .handle_command(cmd(
            scope_id,
            "schedule.create",
            create_with_new_template(disabled_id, uid(1604), "disabled-one", "0 9 * * *", "UTC"),
        ))
        .expect("create schedule to disable");
    handler
        .handle_command(cmd(
            scope_id,
            "schedule.disable",
            serde_json::json!({ "schedule_id": disabled_id.to_string() }),
        ))
        .expect("disable");

    let listed = handler
        .handle_query(qry(scope_id, "schedule.list", serde_json::json!({})))
        .expect("schedule.list succeeds");
    let schedules = listed.result["schedules"].as_array().unwrap();

    let enabled_row = find_row(schedules, enabled_id);
    assert_eq!(enabled_row["next_run_state"], "scheduled");
    assert!(
        enabled_row["next_run"].is_string(),
        "an enabled schedule must show a concrete next run: {enabled_row:?}"
    );

    let disabled_row = find_row(schedules, disabled_id);
    assert_eq!(disabled_row["next_run_state"], "disabled");
    assert!(
        disabled_row["next_run"].is_null(),
        "a disabled schedule must not report a fabricated next run: {disabled_row:?}"
    );
}

/// Mutation 3's own target: `next_run` must never echo the stored
/// `last_fired_at`, and it must actually compute a future instant for a
/// schedule that has already fired once.
#[test]
fn list_never_reports_last_fired_at_where_the_next_run_belongs() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let handler = new_handler(&fixture);

    let schedule_id = uid(1701);
    handler
        .handle_command(cmd(
            scope_id,
            "schedule.create",
            create_with_new_template(schedule_id, uid(1702), "fires-daily", "0 9 * * *", "UTC"),
        ))
        .expect("create");

    // Stamp `last_fired_at` in the past — production only ever does this
    // from `dispatch::tick`, but this test only needs the column set, not a
    // real fire.
    let fired_at = chrono::Utc::now() - chrono::Duration::days(1);
    {
        let mut store = factory_store::Store::open(fixture.instance_root()).expect("open store");
        let tx = store.transaction().expect("begin");
        factory_task::schedule::mark_fired(&tx, schedule_id, fired_at).expect("mark_fired");
        tx.commit().expect("commit");
    }

    let listed = handler
        .handle_query(qry(scope_id, "schedule.list", serde_json::json!({})))
        .expect("schedule.list succeeds");
    let row = find_row(listed.result["schedules"].as_array().unwrap(), schedule_id);

    let last_fired_at = row["last_fired_at"].as_str().expect("last_fired_at is set");
    let next_run = row["next_run"].as_str().expect("next_run is set");
    assert_ne!(
        last_fired_at, next_run,
        "next_run must never echo the stored last_fired_at: {row:?}"
    );
    let next_run_parsed = chrono::DateTime::parse_from_rfc3339(next_run).expect("valid RFC3339");
    assert!(
        next_run_parsed > chrono::Utc::now(),
        "the next run of a daily 09:00 UTC schedule must be in the future: {next_run}"
    );
}

// --- enable / disable --------------------------------------------------

/// Backlog §11, first half: toggling `enabled` "must not delete history."
#[test]
fn disable_then_enable_does_not_touch_history() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let handler = new_handler(&fixture);

    let schedule_id = uid(1801);
    handler
        .handle_command(cmd(
            scope_id,
            "schedule.create",
            create_with_new_template(
                schedule_id,
                uid(1802),
                "toggled-history",
                "0 9 * * *",
                "UTC",
            ),
        ))
        .expect("create");

    let fired_at = chrono::Utc::now() - chrono::Duration::hours(3);
    {
        let mut store = factory_store::Store::open(fixture.instance_root()).expect("open store");
        let tx = store.transaction().expect("begin");
        factory_task::schedule::mark_fired(&tx, schedule_id, fired_at).expect("mark_fired");
        tx.commit().expect("commit");
    }

    let before = handler
        .handle_query(qry(scope_id, "schedule.list", serde_json::json!({})))
        .expect("list before toggling");
    let before_row = find_row(before.result["schedules"].as_array().unwrap(), schedule_id).clone();

    handler
        .handle_command(cmd(
            scope_id,
            "schedule.disable",
            serde_json::json!({ "schedule_id": schedule_id.to_string() }),
        ))
        .expect("disable");
    handler
        .handle_command(cmd(
            scope_id,
            "schedule.enable",
            serde_json::json!({ "schedule_id": schedule_id.to_string() }),
        ))
        .expect("enable");

    let after = handler
        .handle_query(qry(scope_id, "schedule.list", serde_json::json!({})))
        .expect("list after toggling");
    let after_row = find_row(after.result["schedules"].as_array().unwrap(), schedule_id);

    assert_eq!(
        after_row["last_fired_at"], before_row["last_fired_at"],
        "toggling enabled twice must not touch last_fired_at"
    );
    assert_eq!(after_row["cron"], before_row["cron"]);
    assert_eq!(after_row["timezone"], before_row["timezone"]);
    assert_eq!(after_row["template_id"], before_row["template_id"]);
}

/// Backlog §11, second half: toggling `enabled` "must not affect runs that
/// already exist."
#[test]
fn disable_then_enable_does_not_touch_any_existing_run() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let handler = new_handler(&fixture);

    let schedule_id = uid(1901);
    let template_id = uid(1902);
    handler
        .handle_command(cmd(
            scope_id,
            "schedule.create",
            create_with_new_template(schedule_id, template_id, "toggled-runs", "0 9 * * *", "UTC"),
        ))
        .expect("create");

    // A real run, created exactly the way the dispatcher creates one.
    let run_id = uid(1903);
    let fired_at = chrono::Utc::now();
    {
        let mut store = factory_store::Store::open(fixture.instance_root()).expect("open store");
        let local_minute =
            factory_task::schedule::local_minute_string("UTC", fired_at).expect("local minute");
        factory_task::create::create_from_schedule(
            &mut store,
            run_id,
            schedule_id,
            template_id,
            scope_id,
            None,
            "do it",
            &local_minute,
            fired_at,
        )
        .expect("create_from_schedule");
    }

    let run_before = handler
        .handle_query(qry(
            scope_id,
            "task.show",
            serde_json::json!({ "task_id": run_id.to_string() }),
        ))
        .expect("task.show before toggling");

    handler
        .handle_command(cmd(
            scope_id,
            "schedule.disable",
            serde_json::json!({ "schedule_id": schedule_id.to_string() }),
        ))
        .expect("disable");
    handler
        .handle_command(cmd(
            scope_id,
            "schedule.enable",
            serde_json::json!({ "schedule_id": schedule_id.to_string() }),
        ))
        .expect("enable");

    let run_after = handler
        .handle_query(qry(
            scope_id,
            "task.show",
            serde_json::json!({ "task_id": run_id.to_string() }),
        ))
        .expect("task.show after toggling");

    assert_eq!(
        run_before.result, run_after.result,
        "toggling a schedule must not change a single field of a run it already produced"
    );
}

/// `factory_task::schedule::due` is the dispatcher's own query and the one
/// home of "is this schedule due right now." Checking it here, against a
/// store read back after `schedule.disable`, is what proves the command and
/// `due`'s own `enabled = 1` filter cannot drift apart — a `schedule.disable`
/// that silently did nothing would still pass a check that never looked at
/// `due` at all.
#[test]
fn a_disabled_schedule_does_not_fire() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let handler = new_handler(&fixture);

    let schedule_id = uid(2001);
    handler
        .handle_command(cmd(
            scope_id,
            "schedule.create",
            create_with_new_template(schedule_id, uid(2002), "every-minute", "* * * * *", "UTC"),
        ))
        .expect("create");

    handler
        .handle_command(cmd(
            scope_id,
            "schedule.disable",
            serde_json::json!({ "schedule_id": schedule_id.to_string() }),
        ))
        .expect("disable through the command under test");

    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let due = factory_task::schedule::due(&store, chrono::Utc::now()).expect("due");
    assert!(
        !due.schedules.iter().any(|s| s.id == schedule_id),
        "a disabled schedule must not be due, even one matching every minute"
    );
}
