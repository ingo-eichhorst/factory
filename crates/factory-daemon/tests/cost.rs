//! Backlog §11's acceptance criterion: "A run stores model identifier, token
//! counts, and duration when its adapter reports them, and remains valid
//! when the adapter reports none of them." ADR 0021 decisions 6 and 7.
//!
//! Every scenario below is best-effort by construction — see
//! `ops::task::sample_cost_best_effort` and `record_cost_at_terminal` — so
//! the tests here are less about a number appearing and more about what must
//! survive when the adapter, or the write itself, cannot supply one.

mod common;

use factory_daemon::envelope::{CommandRequest, QueryRequest};
use factory_daemon::{FactoryHandler, Handler};

fn uid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

fn command(scope_id: uuid::Uuid, command: &str, payload: serde_json::Value) -> CommandRequest {
    CommandRequest {
        request_id: uid(9000),
        scope_id,
        command: command.to_string(),
        payload,
        expected_revision: None,
    }
}

fn query(scope_id: uuid::Uuid, query: &str, payload: serde_json::Value) -> QueryRequest {
    QueryRequest {
        request_id: uid(9001),
        scope_id,
        query: query.to_string(),
        payload,
    }
}

/// A `factory_adapter::CostSample` shaped like a Claude Code / Irrlicht
/// reading — cumulative-to-date, per `factory_adapter::claude::ClaudeAdapter`'s
/// own doc comment. Every test below uses one `CostSource` consistently
/// between its baseline and terminal samples: `CostSample::since` refuses to
/// subtract two samples from different adapters, on purpose, and that is not
/// what any of these tests are checking.
fn claude_sample(
    input_tokens: u64,
    output_tokens: u64,
    duration_ms: Option<u64>,
    context_utilization_percent: Option<f64>,
) -> factory_adapter::CostSample {
    factory_adapter::CostSample {
        source: factory_adapter::CostSource::ClaudeCode,
        model: Some("claude-test".to_string()),
        input_tokens,
        output_tokens,
        duration_ms,
        context_utilization_percent,
    }
}

/// `agent.start` then `task.send` against `session_id`/`task_id`, exactly
/// `lifecycle.rs`'s own `agent_start_task_send_delivers_and_marks_running`
/// shape — the common setup nearly every test below needs before it can
/// configure a cost sample against the resulting pane.
fn start_and_send(
    handler: &FactoryHandler,
    scope_id: uuid::Uuid,
    session_id: uuid::Uuid,
    task_id: uuid::Uuid,
) {
    handler
        .handle_command(command(
            scope_id,
            "agent.start",
            serde_json::json!({ "session_id": session_id.to_string(), "agent_name": "agent" }),
        ))
        .expect("agent.start succeeds");

    let sent = handler
        .handle_command(command(
            scope_id,
            "task.send",
            serde_json::json!({
                "task_id": task_id.to_string(),
                "prompt": "do it",
                "target_session_id": session_id.to_string(),
            }),
        ))
        .expect("task.send succeeds");
    assert_eq!(sent.result["status"], "running");
}

#[test]
fn cumulative_baseline_and_terminal_store_the_difference() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();
    let handler = FactoryHandler::new(store, adapter.clone(), fixture.instance_root());

    let session_id = common::seed(1);
    let task_id = common::seed(2);
    let pane = format!("pane-{session_id}");

    // Queued before `task.send`, so it answers the baseline sample delivery
    // takes.
    adapter.queue_cost_sample(
        &pane,
        Some(claude_sample(1_000, 400, Some(10_000), Some(12.5))),
    );
    start_and_send(&handler, scope_id, session_id, task_id);

    // Queued before `task.done`, so it answers the terminal sample.
    adapter.queue_cost_sample(
        &pane,
        Some(claude_sample(1_350, 470, Some(25_000), Some(58.0))),
    );
    let done = handler
        .handle_command(command(
            scope_id,
            "task.done",
            serde_json::json!({ "task_id": task_id.to_string(), "result_summary": "ok" }),
        ))
        .expect("task.done succeeds");
    assert_eq!(done.result["status"], "done");

    let shown = handler
        .handle_query(query(
            scope_id,
            "task.show",
            serde_json::json!({ "task_id": task_id.to_string() }),
        ))
        .expect("task.show succeeds");

    assert_eq!(shown.result["cost_model"].as_str(), Some("claude-test"));
    assert_eq!(
        shown.result["cost_input_tokens"].as_i64(),
        Some(350),
        "1350 - 1000: the difference, not the raw terminal reading (1350) or the baseline (1000)"
    );
    assert_eq!(shown.result["cost_output_tokens"].as_i64(), Some(70));
    assert_eq!(shown.result["cost_duration_ms"].as_i64(), Some(15_000));
    assert_eq!(
        shown.result["context_utilization_percent"].as_f64(),
        Some(58.0),
        "the terminal reading's own context pressure, never diffed (ADR 0021 decision 7)"
    );
}

#[test]
fn cost_baseline_survives_a_simulated_daemon_restart() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let session_id = common::seed(1);
    let task_id = common::seed(2);
    let pane = format!("pane-{session_id}");

    {
        // "Before the restart": one daemon process, one `FactoryHandler`, one
        // `FakeAdapter` instance.
        let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
        let adapter = common::FakeAdapter::new();
        let handler = FactoryHandler::new(store, adapter.clone(), fixture.instance_root());

        adapter.queue_cost_sample(&pane, Some(claude_sample(1_000, 400, None, Some(10.0))));
        start_and_send(&handler, scope_id, session_id, task_id);
        // `handler` and `adapter` are dropped here — nothing about the
        // baseline sample above survives anywhere in this process's memory
        // past this point. Only `tasks.cost_baseline` can still answer for
        // it.
    }

    {
        // "After the restart": a fresh `Store::open` against the same
        // database file, and a brand new, unrelated `FakeAdapter` that has
        // never heard of the baseline queued above.
        let store = factory_store::Store::open(fixture.instance_root()).expect("reopen store");
        let adapter = common::FakeAdapter::new();
        let handler = FactoryHandler::new(store, adapter.clone(), fixture.instance_root());

        adapter.queue_cost_sample(&pane, Some(claude_sample(1_600, 900, None, Some(80.0))));
        let done = handler
            .handle_command(command(
                scope_id,
                "task.done",
                serde_json::json!({ "task_id": task_id.to_string(), "result_summary": "ok" }),
            ))
            .expect("task.done succeeds after the simulated restart");
        assert_eq!(done.result["status"], "done");

        let shown = handler
            .handle_query(query(
                scope_id,
                "task.show",
                serde_json::json!({ "task_id": task_id.to_string() }),
            ))
            .expect("task.show succeeds");
        assert_eq!(
            shown.result["cost_input_tokens"].as_i64(),
            Some(600),
            "1600 - 1000: the baseline came back from `tasks.cost_baseline`, not from the \
             first handler's memory, which this block can no longer reach"
        );
        assert_eq!(shown.result["cost_output_tokens"].as_i64(), Some(500));
    }
}

#[test]
fn cost_sample_ok_none_still_completes_with_null_columns() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    // Unconfigured: every `cost_sample` call answers `Ok(None)`.
    let adapter = common::FakeAdapter::new();
    let handler = FactoryHandler::new(store, adapter.clone(), fixture.instance_root());

    let session_id = common::seed(1);
    let task_id = common::seed(2);
    start_and_send(&handler, scope_id, session_id, task_id);

    let done = handler
        .handle_command(command(
            scope_id,
            "task.done",
            serde_json::json!({ "task_id": task_id.to_string(), "result_summary": "ok" }),
        ))
        .expect("task.done succeeds even though the adapter reports no cost data at all");
    assert_eq!(done.result["status"], "done");

    let shown = handler
        .handle_query(query(
            scope_id,
            "task.show",
            serde_json::json!({ "task_id": task_id.to_string() }),
        ))
        .expect("task.show succeeds");
    for field in [
        "cost_model",
        "cost_input_tokens",
        "cost_output_tokens",
        "cost_duration_ms",
        "context_utilization_percent",
    ] {
        assert!(
            shown.result[field].is_null(),
            "{field} must stay NULL when the adapter reports Ok(None) throughout"
        );
    }
}

#[test]
fn cost_sample_err_still_completes() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();
    adapter.set_cost_sample_fail(true);
    let handler = FactoryHandler::new(store, adapter.clone(), fixture.instance_root());

    let session_id = common::seed(1);
    let task_id = common::seed(2);
    start_and_send(&handler, scope_id, session_id, task_id);

    let done = handler
        .handle_command(command(
            scope_id,
            "task.done",
            serde_json::json!({ "task_id": task_id.to_string(), "result_summary": "ok" }),
        ))
        .expect("task.done must succeed even though cost_sample errors at the terminal");
    assert_eq!(done.result["status"], "done");

    let shown = handler
        .handle_query(query(
            scope_id,
            "task.show",
            serde_json::json!({ "task_id": task_id.to_string() }),
        ))
        .expect("task.show succeeds");
    assert!(shown.result["cost_input_tokens"].is_null());
}

#[test]
fn task_with_no_assigned_session_completes() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();
    let handler = FactoryHandler::new(store, adapter.clone(), fixture.instance_root());

    // No `agent.start` at all: there is no idle session for `task.send` to
    // assign, so this task stays `queued` with `assigned_session_id` NULL.
    let task_id = common::seed(1);
    let sent = handler
        .handle_command(command(
            scope_id,
            "task.send",
            serde_json::json!({
                "task_id": task_id.to_string(),
                "prompt": "do it",
                "agent_name": "agent",
            }),
        ))
        .expect("task.send succeeds even with no idle session to assign");
    assert_eq!(sent.result["status"], "queued");
    assert_eq!(sent.result["assignment"]["kind"], "deferred");

    let cancelled = handler
        .handle_command(command(
            scope_id,
            "task.cancel",
            serde_json::json!({ "task_id": task_id.to_string() }),
        ))
        .expect("task.cancel succeeds for a never-assigned task");
    assert_eq!(cancelled.result["status"], "cancelled");

    // Never assigned, so never a pane to ask — the adapter must never even
    // be called.
    assert!(
        adapter.cost_sample_calls().is_empty(),
        "a task with no assigned session has no pane to sample cost from"
    );

    let shown = handler
        .handle_query(query(
            scope_id,
            "task.show",
            serde_json::json!({ "task_id": task_id.to_string() }),
        ))
        .expect("task.show succeeds");
    assert_eq!(shown.result["status"], "cancelled");
    assert!(shown.result["cost_input_tokens"].is_null());
}

#[test]
fn context_utilization_survives_a_token_diff_that_underflows() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();
    let handler = FactoryHandler::new(store, adapter.clone(), fixture.instance_root());

    let session_id = common::seed(1);
    let task_id = common::seed(2);
    let pane = format!("pane-{session_id}");

    adapter.queue_cost_sample(&pane, Some(claude_sample(5_000, 2_000, None, Some(90.0))));
    start_and_send(&handler, scope_id, session_id, task_id);

    // A compaction between the two samples: the terminal reading's own
    // counters are *lower* than the baseline's, so `CostSample::since`
    // cannot subtract them and returns `Ok(None)`. `context_utilization_percent`
    // is never diffed in the first place, so it must still be recorded —
    // `CostSample::since`'s own doc comment: "the caller still holds `self`,
    // so the run's own context-pressure reading is not lost even when the
    // token delta is."
    adapter.queue_cost_sample(&pane, Some(claude_sample(1_200, 500, None, Some(15.0))));
    let done = handler
        .handle_command(command(
            scope_id,
            "task.done",
            serde_json::json!({ "task_id": task_id.to_string(), "result_summary": "ok" }),
        ))
        .expect("task.done succeeds even though the token delta underflows");
    assert_eq!(done.result["status"], "done");

    let shown = handler
        .handle_query(query(
            scope_id,
            "task.show",
            serde_json::json!({ "task_id": task_id.to_string() }),
        ))
        .expect("task.show succeeds");
    assert!(
        shown.result["cost_input_tokens"].is_null(),
        "the token counters could not be subtracted, so they must stay NULL"
    );
    assert!(shown.result["cost_output_tokens"].is_null());
    assert_eq!(
        shown.result["cost_model"].as_str(),
        Some("claude-test"),
        "the model is the terminal sample's own reading, not diffed"
    );
    assert_eq!(
        shown.result["context_utilization_percent"].as_f64(),
        Some(15.0),
        "context pressure is the terminal reading, never diffed, and must survive even when \
         the token delta could not be computed"
    );
}

#[test]
fn cost_sample_runs_before_session_teardown() {
    let fixture = common::build(&[common::ScopeSpec::new("temp", "temp").temporary()]);
    let scope_id = fixture.scope("temp");
    let db_path = fixture
        .instance_root()
        .join(".factory")
        .join("factory.sqlite");
    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();
    let handler = FactoryHandler::new(store, adapter.clone(), fixture.instance_root());

    let session_id = common::seed(1);
    let task_id = common::seed(2);
    start_and_send(&handler, scope_id, session_id, task_id);

    // Set only *after* `task.send`, so it captures the terminal call alone —
    // the baseline call above already ran, unobserved, before this hook
    // existed.
    let observed_states = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let observed_states_clone = observed_states.clone();
    adapter.set_cost_sample_hook(move |_pane| {
        // A second, read-only connection to the same file — exactly the
        // technique `delivery_journal_order.rs`'s `JournalCheckingAdapter`
        // already uses to observe intermediate database state mid-call.
        let read_only = factory_store::Store::open_read_only(&db_path)
            .expect("open a second, read-only connection to the same database");
        let session = factory_session::show(&read_only, session_id)
            .expect("the session row still exists at sample time");
        observed_states_clone
            .lock()
            .unwrap()
            .push(session.state.to_string());
    });

    let done = handler
        .handle_command(command(
            scope_id,
            "task.done",
            serde_json::json!({ "task_id": task_id.to_string(), "result_summary": "ok" }),
        ))
        .expect("task.done succeeds");
    assert_eq!(done.result["status"], "done");

    let states = observed_states.lock().unwrap();
    assert_eq!(
        states.len(),
        1,
        "the terminal cost sample must be taken exactly once"
    );
    assert_eq!(
        states[0], "running",
        "cost_sample must run before `teardown_session_if_terminal` commits \
         `sessions.state = 'stopped'` for a temporary agent's session — sampling after it \
         would read a session the run no longer owns"
    );
}

#[test]
fn cost_write_failure_at_terminal_does_not_undo_the_completion() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let db_path = fixture
        .instance_root()
        .join(".factory")
        .join("factory.sqlite");

    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    // Fail fast on contention rather than waiting the production 5 s
    // default — this test forces a real `SQLITE_BUSY`, not a mock.
    store
        .connection()
        .pragma_update(None, "busy_timeout", 50_i64)
        .expect("lower busy_timeout for this test");

    let adapter = common::FakeAdapter::new();
    let handler = FactoryHandler::new(store, adapter.clone(), fixture.instance_root());

    let session_id = common::seed(1);
    let task_id = common::seed(2);
    let pane = format!("pane-{session_id}");

    // A real baseline, queued *before* delivery, so `tasks.cost_baseline` is
    // actually recorded — without one, `record_cost_at_terminal` returns
    // before ever reaching the write this test means to fail, and the
    // mutation this test exists to catch would pass it vacuously.
    adapter.queue_cost_sample(&pane, Some(claude_sample(600, 250, None, Some(20.0))));
    start_and_send(&handler, scope_id, session_id, task_id);

    adapter.queue_cost_sample(&pane, Some(claude_sample(1_000, 400, None, Some(50.0))));

    // Hold the write lock on a second, real connection from the exact
    // moment the terminal sample is taken — after `factory_task::complete::done`'s
    // own transaction has already committed (that already happened, inside
    // `handle_command` above `on_terminal`), and just before
    // `record_cost_result` tries to open its own. Its `BEGIN IMMEDIATE` must
    // then fail with `SQLITE_BUSY`, proving the two writes are not one
    // transaction.
    let held = std::sync::Arc::new(std::sync::Mutex::new(None::<rusqlite::Connection>));
    let held_clone = held.clone();
    let db_path_clone = db_path.clone();
    adapter.set_cost_sample_hook(move |_pane| {
        let conn =
            rusqlite::Connection::open(&db_path_clone).expect("open a contending connection");
        conn.execute_batch("BEGIN IMMEDIATE;")
            .expect("take the write lock");
        *held_clone.lock().unwrap() = Some(conn);
    });

    let done = handler
        .handle_command(command(
            scope_id,
            "task.done",
            serde_json::json!({ "task_id": task_id.to_string(), "result_summary": "ok" }),
        ))
        .expect("task.done must still succeed even though the cost write fails under contention");
    assert_eq!(done.result["status"], "done");

    // Release the contending lock (rolls back — nothing was ever committed
    // on it).
    drop(held.lock().unwrap().take());

    let shown = handler
        .handle_query(query(
            scope_id,
            "task.show",
            serde_json::json!({ "task_id": task_id.to_string() }),
        ))
        .expect("task.show succeeds");
    assert_eq!(
        shown.result["status"], "done",
        "the completion must stand even though its cost write failed"
    );
    assert!(
        shown.result["cost_input_tokens"].is_null(),
        "the write itself failed under contention, so nothing was recorded — if \
         `record_cost_result` shared the completion's own transaction, `task.done` above \
         would have failed instead of the write silently losing"
    );
}

#[test]
fn cost_write_failure_at_delivery_does_not_undo_the_delivery() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let db_path = fixture
        .instance_root()
        .join(".factory")
        .join("factory.sqlite");

    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    store
        .connection()
        .pragma_update(None, "busy_timeout", 50_i64)
        .expect("lower busy_timeout for this test");

    let adapter = common::FakeAdapter::new();
    let handler = FactoryHandler::new(store, adapter.clone(), fixture.instance_root());

    let session_id = common::seed(1);
    handler
        .handle_command(command(
            scope_id,
            "agent.start",
            serde_json::json!({ "session_id": session_id.to_string(), "agent_name": "agent" }),
        ))
        .expect("agent.start succeeds");

    let pane = format!("pane-{session_id}");
    adapter.queue_cost_sample(&pane, Some(claude_sample(100, 50, None, None)));

    // Hold the write lock from the exact moment the *baseline* sample is
    // taken — set before `task.send`, unlike the terminal test above, since
    // there is only ever one `cost_sample` call before this test's own
    // assertion runs.
    let held = std::sync::Arc::new(std::sync::Mutex::new(None::<rusqlite::Connection>));
    let held_clone = held.clone();
    let db_path_clone = db_path.clone();
    adapter.set_cost_sample_hook(move |_pane| {
        let conn =
            rusqlite::Connection::open(&db_path_clone).expect("open a contending connection");
        conn.execute_batch("BEGIN IMMEDIATE;")
            .expect("take the write lock");
        *held_clone.lock().unwrap() = Some(conn);
    });

    let task_id = common::seed(2);
    let sent = handler
        .handle_command(command(
            scope_id,
            "task.send",
            serde_json::json!({
                "task_id": task_id.to_string(),
                "prompt": "do it",
                "target_session_id": session_id.to_string(),
            }),
        ))
        .expect("task.send must still succeed even though recording the cost baseline fails");
    assert_eq!(sent.result["status"], "running");
    assert_eq!(sent.result["delivery"]["sent"], true);

    drop(held.lock().unwrap().take());

    let shown = handler
        .handle_query(query(
            scope_id,
            "task.show",
            serde_json::json!({ "task_id": task_id.to_string() }),
        ))
        .expect("task.show succeeds");
    assert_eq!(
        shown.result["status"], "running",
        "delivery must stand even though writing the cost baseline failed under contention"
    );
    assert_eq!(shown.result["assigned_session_id"], session_id.to_string());
}
