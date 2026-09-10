//! Broad coverage of the remaining operations: `scope.add` (refusal),
//! `scope.reconcile`, `scope.list`, `agent.stop`, `agent.attach_command`,
//! `context.show`, `daemon.status`, `task.cancel`, `task.block`,
//! `task.resume`, and the validation/dispatch error paths.

mod common;

use factory_daemon::Handler as _;
use factory_daemon::envelope::{CommandRequest, QueryRequest};

fn uid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

fn cmd(scope_id: uuid::Uuid, command: &str, payload: serde_json::Value) -> CommandRequest {
    CommandRequest {
        request_id: uid(1),
        scope_id,
        command: command.to_string(),
        payload,
        expected_revision: None,
    }
}

fn qry(scope_id: uuid::Uuid, query: &str, payload: serde_json::Value) -> QueryRequest {
    QueryRequest {
        request_id: uid(2),
        scope_id,
        query: query.to_string(),
        payload,
    }
}

fn new_handler(fixture: &common::Fixture) -> (factory_daemon::FactoryHandler, common::FakeAdapter) {
    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();
    let handler =
        factory_daemon::FactoryHandler::new(store, adapter.clone(), fixture.instance_root());
    (handler, adapter)
}

#[test]
fn scope_add_always_refuses_with_a_named_remedy() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let (handler, _adapter) = new_handler(&fixture);
    let err = handler
        .handle_command(cmd(
            fixture.scope("alpha"),
            "scope.add",
            serde_json::json!({}),
        ))
        .unwrap_err();
    assert_eq!(err.code, "internal.scope_add_unsupported");
    assert!(err.message.contains("scope.reconcile"));
}

#[test]
fn scope_reconcile_reports_clean_after_the_fixture_already_applied() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let (handler, _adapter) = new_handler(&fixture);
    let result = handler
        .handle_command(cmd(
            fixture.scope("alpha"),
            "scope.reconcile",
            serde_json::json!({}),
        ))
        .expect("scope.reconcile succeeds");
    assert_eq!(result.result["clean"], true);
    assert_eq!(result.result["drift"].as_array().unwrap().len(), 0);
}

#[test]
fn scope_list_reports_every_registered_scope() {
    let fixture = common::build(&[
        common::ScopeSpec::new("alpha", "alpha"),
        common::ScopeSpec::new("beta", "beta"),
    ]);
    let (handler, _adapter) = new_handler(&fixture);
    let result = handler
        .handle_query(qry(
            fixture.scope("alpha"),
            "scope.list",
            serde_json::json!({}),
        ))
        .expect("scope.list succeeds");
    let scopes = result.result["scopes"].as_array().unwrap();
    // company (root) + alpha + beta
    assert_eq!(scopes.len(), 3);
    let names: Vec<&str> = scopes.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"alpha"));
    assert!(names.contains(&"beta"));
    assert!(names.contains(&"company"));
}

#[test]
fn agent_stop_without_a_running_task_stops_cleanly() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let (handler, adapter) = new_handler(&fixture);

    let session_id = uid(6001);
    handler
        .handle_command(cmd(
            scope_id,
            "agent.start",
            serde_json::json!({ "session_id": session_id.to_string(), "agent_name": "agent" }),
        ))
        .expect("agent.start succeeds");

    let stopped = handler
        .handle_command(cmd(
            scope_id,
            "agent.stop",
            serde_json::json!({ "session_id": session_id.to_string() }),
        ))
        .expect("agent.stop succeeds");
    assert_eq!(stopped.result["state"], "stopped");
    assert_eq!(
        adapter.stop_calls(),
        vec![format!("pane-{session_id}")],
        "Adapter::stop must be called before the session-side write"
    );
}

#[test]
fn agent_stop_with_a_running_task_interrupts_it() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let (handler, _adapter) = new_handler(&fixture);

    let session_id = uid(6002);
    handler
        .handle_command(cmd(
            scope_id,
            "agent.start",
            serde_json::json!({ "session_id": session_id.to_string(), "agent_name": "agent" }),
        ))
        .expect("agent.start succeeds");

    let task_id = uid(6003);
    handler
        .handle_command(cmd(
            scope_id,
            "task.send",
            serde_json::json!({
                "task_id": task_id.to_string(),
                "prompt": "do it",
                "target_session_id": session_id.to_string(),
            }),
        ))
        .expect("task.send succeeds");

    let stopped = handler
        .handle_command(cmd(
            scope_id,
            "agent.stop",
            serde_json::json!({ "session_id": session_id.to_string() }),
        ))
        .expect("agent.stop succeeds");
    assert_eq!(stopped.result["state"], "failed");
    assert_eq!(stopped.result["interrupted_task_id"], task_id.to_string());

    let shown = handler
        .handle_query(qry(
            scope_id,
            "task.show",
            serde_json::json!({ "task_id": task_id.to_string() }),
        ))
        .expect("task.show succeeds");
    assert_eq!(shown.result["status"], "blocked");
    assert_eq!(shown.result["blocked_reason"], "interrupted");
}

#[test]
fn agent_attach_command_returns_argv_naming_the_pane() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let (handler, _adapter) = new_handler(&fixture);

    let session_id = uid(6004);
    handler
        .handle_command(cmd(
            scope_id,
            "agent.start",
            serde_json::json!({ "session_id": session_id.to_string(), "agent_name": "agent" }),
        ))
        .expect("agent.start succeeds");

    let attach = handler
        .handle_query(qry(
            scope_id,
            "agent.attach_command",
            serde_json::json!({ "session_id": session_id.to_string() }),
        ))
        .expect("agent.attach_command succeeds");
    let argv: Vec<String> = attach.result["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(argv.iter().any(|a| a.contains(&session_id.to_string())));
}

#[test]
fn context_show_compiles_company_and_scope_agents_md() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let (handler, _adapter) = new_handler(&fixture);

    let shown = handler
        .handle_query(qry(
            scope_id,
            "context.show",
            serde_json::json!({ "agent_name": "agent" }),
        ))
        .expect("context.show succeeds");
    let text = shown.result["text"].as_str().unwrap();
    assert!(text.contains("company root"));
    assert!(text.contains("alpha"));
    let sources = shown.result["sources"].as_array().unwrap();
    assert!(
        sources.len() >= 3,
        "company + scope + agent definition sections: {sources:?}"
    );
}

#[test]
fn daemon_status_reports_schema_and_paths() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let (handler, _adapter) = new_handler(&fixture);

    let status = handler
        .handle_query(qry(scope_id, "daemon.status", serde_json::json!({})))
        .expect("daemon.status succeeds");
    assert!(status.result["schema_version"].as_i64().unwrap() >= 1);
    assert!(
        status.result["socket_path"]
            .as_str()
            .unwrap()
            .ends_with("factory.sock")
    );
}

#[test]
fn task_cancel_immediate_path_for_queued_and_cooperative_path_for_running() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let (handler, _adapter) = new_handler(&fixture);

    // Queued: no idle session exists yet, so `task.send` defers.
    let queued_task = uid(7001);
    let sent = handler
        .handle_command(cmd(
            scope_id,
            "task.send",
            serde_json::json!({
                "task_id": queued_task.to_string(),
                "prompt": "do it",
                "agent_name": "agent",
            }),
        ))
        .expect("task.send succeeds");
    assert_eq!(sent.result["status"], "queued");

    let cancelled = handler
        .handle_command(cmd(
            scope_id,
            "task.cancel",
            serde_json::json!({ "task_id": queued_task.to_string() }),
        ))
        .expect("task.cancel succeeds");
    assert_eq!(cancelled.result["status"], "cancelled");

    // Running: cooperative — stays running, records cancel_requested_at.
    let session_id = uid(7002);
    handler
        .handle_command(cmd(
            scope_id,
            "agent.start",
            serde_json::json!({ "session_id": session_id.to_string(), "agent_name": "agent" }),
        ))
        .expect("agent.start succeeds");
    let running_task = uid(7003);
    handler
        .handle_command(cmd(
            scope_id,
            "task.send",
            serde_json::json!({
                "task_id": running_task.to_string(),
                "prompt": "do it",
                "target_session_id": session_id.to_string(),
            }),
        ))
        .expect("task.send succeeds");

    let cancelled = handler
        .handle_command(cmd(
            scope_id,
            "task.cancel",
            serde_json::json!({ "task_id": running_task.to_string() }),
        ))
        .expect("task.cancel succeeds");
    assert_eq!(cancelled.result["status"], "running");

    let shown = handler
        .handle_query(qry(
            scope_id,
            "task.show",
            serde_json::json!({ "task_id": running_task.to_string() }),
        ))
        .expect("task.show succeeds");
    assert!(shown.result["cancel_requested_at"].is_string());
    assert_eq!(shown.result["status"], "running");
}

#[test]
fn task_block_does_not_tear_down_the_session_and_task_resume_requeues() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let (handler, _adapter) = new_handler(&fixture);

    let session_id = uid(7101);
    handler
        .handle_command(cmd(
            scope_id,
            "agent.start",
            serde_json::json!({ "session_id": session_id.to_string(), "agent_name": "agent" }),
        ))
        .expect("agent.start succeeds");
    let task_id = uid(7102);
    handler
        .handle_command(cmd(
            scope_id,
            "task.send",
            serde_json::json!({
                "task_id": task_id.to_string(),
                "prompt": "do it",
                "target_session_id": session_id.to_string(),
            }),
        ))
        .expect("task.send succeeds");

    let blocked = handler
        .handle_command(cmd(
            scope_id,
            "task.block",
            serde_json::json!({ "task_id": task_id.to_string(), "reason": "clarification" }),
        ))
        .expect("task.block succeeds");
    assert_eq!(blocked.result["status"], "blocked");
    assert_eq!(blocked.result["blocked_reason"], "clarification");

    let status = handler
        .handle_query(qry(
            scope_id,
            "agent.status",
            serde_json::json!({ "session_id": session_id.to_string() }),
        ))
        .expect("agent.status succeeds");
    assert_eq!(
        status.result["session"]["state"], "running",
        "task.block must never tear down the session — blocked is not terminal"
    );

    let resumed = handler
        .handle_command(cmd(
            scope_id,
            "task.resume",
            serde_json::json!({ "task_id": task_id.to_string() }),
        ))
        .expect("task.resume succeeds");
    assert_eq!(resumed.result["status"], "queued");
}

#[test]
fn unknown_operation_and_missing_field_are_validation_errors() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let (handler, _adapter) = new_handler(&fixture);

    let err = handler
        .handle_command(cmd(scope_id, "task.frobnicate", serde_json::json!({})))
        .unwrap_err();
    assert_eq!(err.code, "validation.unknown_operation");

    // Untargeted task.send with neither `target_session_id` nor `agent_name`.
    let err = handler
        .handle_command(cmd(
            scope_id,
            "task.send",
            serde_json::json!({ "task_id": uid(8001).to_string(), "prompt": "x" }),
        ))
        .unwrap_err();
    assert_eq!(err.code, "validation.missing_field");
    assert_eq!(err.details["field"], "agent_name");

    // An unknown field must be a malformed-payload error, not silently
    // ignored — this is the client/server payload-contract mismatch guard.
    let err = handler
        .handle_command(cmd(
            scope_id,
            "task.cancel",
            serde_json::json!({ "task_id": uid(8002).to_string(), "oops": true }),
        ))
        .unwrap_err();
    assert_eq!(err.code, "validation.malformed_payload");
}

// --- Station 11 gap 2: task.assign, task.progress, task.decision, task.verify

#[test]
fn task_assign_chooses_the_idle_session_started_after_the_task_was_queued() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let (handler, _adapter) = new_handler(&fixture);

    // Untargeted send with no idle session yet: stays queued.
    let task_id = uid(9001);
    let sent = handler
        .handle_command(cmd(
            scope_id,
            "task.send",
            serde_json::json!({
                "task_id": task_id.to_string(),
                "prompt": "do it",
                "agent_name": "agent",
            }),
        ))
        .expect("task.send succeeds");
    assert_eq!(sent.result["status"], "queued");

    // Now an idle session exists; `task.assign` finds it without a second
    // `task.send`.
    let session_id = uid(9002);
    handler
        .handle_command(cmd(
            scope_id,
            "agent.start",
            serde_json::json!({ "session_id": session_id.to_string(), "agent_name": "agent" }),
        ))
        .expect("agent.start succeeds");

    let assigned = handler
        .handle_command(cmd(
            scope_id,
            "task.assign",
            serde_json::json!({ "task_id": task_id.to_string(), "agent_name": "agent" }),
        ))
        .expect("task.assign succeeds");
    assert_eq!(assigned.result["assignment"]["kind"], "assigned");
    assert_eq!(
        assigned.result["assignment"]["session_id"],
        session_id.to_string()
    );

    let shown = handler
        .handle_query(qry(
            scope_id,
            "task.show",
            serde_json::json!({ "task_id": task_id.to_string() }),
        ))
        .expect("task.show succeeds");
    // `assign` never delivers, so `status` stays `queued` even though
    // `assigned_session_id` is now set — the same distinction
    // `factory_task::assign`'s own module docs draw.
    assert_eq!(shown.result["status"], "queued");
    assert_eq!(shown.result["assigned_session_id"], session_id.to_string());
}

/// `task.assign` on a task that is not `queued` is refused with the same
/// code `factory_task::assign::AssignError::NotQueued` already maps to
/// (`errors::assign_error`) — this is the real boundary of the command, and
/// exactly the case a second `task.assign` after a first one already
/// succeeded would hit.
#[test]
fn task_assign_on_an_already_assigned_task_is_refused() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let (handler, _adapter) = new_handler(&fixture);

    let session_id = uid(9101);
    handler
        .handle_command(cmd(
            scope_id,
            "agent.start",
            serde_json::json!({ "session_id": session_id.to_string(), "agent_name": "agent" }),
        ))
        .expect("agent.start succeeds");

    let task_id = uid(9102);
    handler
        .handle_command(cmd(
            scope_id,
            "task.send",
            serde_json::json!({
                "task_id": task_id.to_string(),
                "prompt": "do it",
                "target_session_id": session_id.to_string(),
            }),
        ))
        .expect("task.send succeeds");

    // `task.send` above already delivered, so the task is now `running` —
    // not `queued` — and a second `task.assign` must be refused.
    let shown = handler
        .handle_query(qry(
            scope_id,
            "task.show",
            serde_json::json!({ "task_id": task_id.to_string() }),
        ))
        .expect("task.show succeeds");
    assert_eq!(shown.result["status"], "running");

    let err = handler
        .handle_command(cmd(
            scope_id,
            "task.assign",
            serde_json::json!({ "task_id": task_id.to_string(), "agent_name": "agent" }),
        ))
        .unwrap_err();
    assert_eq!(err.code, "conflict.task_not_queued");
}

/// `handler.rs`'s own rule for `mutated`: `false` for "a command that turned
/// out to be a no-op." A deferred `task.assign` (no idle session yet) writes
/// nothing to `tasks`, so it must not advance `event_cursor` — the same
/// no-mutation contract `task.wait`'s own timeout path already honours.
#[test]
fn task_assign_deferred_does_not_advance_the_event_cursor() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let (handler, _adapter) = new_handler(&fixture);

    let task_id = uid(9601);
    let sent = handler
        .handle_command(cmd(
            scope_id,
            "task.send",
            serde_json::json!({
                "task_id": task_id.to_string(),
                "prompt": "do it",
                "agent_name": "agent",
            }),
        ))
        .expect("task.send succeeds");
    let cursor_after_send = sent.event_cursor;

    // No idle session exists, so this must defer without writing anything.
    let assigned = handler
        .handle_command(cmd(
            scope_id,
            "task.assign",
            serde_json::json!({ "task_id": task_id.to_string(), "agent_name": "agent" }),
        ))
        .expect("task.assign succeeds");
    assert_eq!(assigned.result["assignment"]["kind"], "deferred");
    assert_eq!(
        assigned.event_cursor, cursor_after_send,
        "a deferred assign must not advance the cursor"
    );
}

#[test]
fn task_progress_records_a_note_and_changes_no_status() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let (handler, _adapter) = new_handler(&fixture);

    let task_id = uid(9201);
    handler
        .handle_command(cmd(
            scope_id,
            "task.send",
            serde_json::json!({
                "task_id": task_id.to_string(),
                "prompt": "do it",
                "agent_name": "agent",
            }),
        ))
        .expect("task.send succeeds");

    let progressed = handler
        .handle_command(cmd(
            scope_id,
            "task.progress",
            serde_json::json!({ "task_id": task_id.to_string(), "note": "halfway there" }),
        ))
        .expect("task.progress succeeds");
    assert_eq!(progressed.result["recorded"], true);

    let shown = handler
        .handle_query(qry(
            scope_id,
            "task.show",
            serde_json::json!({ "task_id": task_id.to_string() }),
        ))
        .expect("task.show succeeds");
    assert_eq!(
        shown.result["status"], "queued",
        "a progress note must not change status"
    );
}

/// design §11: "a decision without a rationale is a log line, not a
/// decision" — `task_decisions.rationale` is `NOT NULL` on purpose, and this
/// is the daemon-level proof that an empty `rationale` is refused before any
/// row is written, not merely a domain-level one.
#[test]
fn task_decision_with_an_empty_rationale_is_refused() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let (handler, _adapter) = new_handler(&fixture);

    let task_id = uid(9301);
    handler
        .handle_command(cmd(
            scope_id,
            "task.send",
            serde_json::json!({
                "task_id": task_id.to_string(),
                "prompt": "do it",
                "agent_name": "agent",
            }),
        ))
        .expect("task.send succeeds");

    let err = handler
        .handle_command(cmd(
            scope_id,
            "task.decision",
            serde_json::json!({
                "decision_id": uid(9302).to_string(),
                "task_id": task_id.to_string(),
                "decision": "use approach B",
                "rationale": "",
            }),
        ))
        .unwrap_err();
    assert_eq!(err.code, "validation.no_rationale");
}

#[test]
fn task_decision_records_alternatives_and_consequences() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let (handler, _adapter) = new_handler(&fixture);

    let task_id = uid(9401);
    handler
        .handle_command(cmd(
            scope_id,
            "task.send",
            serde_json::json!({
                "task_id": task_id.to_string(),
                "prompt": "do it",
                "agent_name": "agent",
            }),
        ))
        .expect("task.send succeeds");

    let decision_id = uid(9402);
    let recorded = handler
        .handle_command(cmd(
            scope_id,
            "task.decision",
            serde_json::json!({
                "decision_id": decision_id.to_string(),
                "task_id": task_id.to_string(),
                "decision": "use approach B",
                "rationale": "approach A needed a schema change we don't own",
                "alternatives": "approach A",
                "consequences": "slower, no cross-crate coordination",
            }),
        ))
        .expect("task.decision succeeds");
    assert_eq!(recorded.result["decision_id"], decision_id.to_string());
}

/// ADR 0021 decision 4's whole point, proven through the wire: recording a
/// verdict must leave `task.show`'s own `status` exactly as it was.
#[test]
fn task_verify_records_a_verdict_and_leaves_status_unchanged() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let (handler, _adapter) = new_handler(&fixture);

    let task_id = uid(9501);
    handler
        .handle_command(cmd(
            scope_id,
            "task.send",
            serde_json::json!({
                "task_id": task_id.to_string(),
                "prompt": "do it",
                "agent_name": "agent",
            }),
        ))
        .expect("task.send succeeds");

    let before = handler
        .handle_query(qry(
            scope_id,
            "task.show",
            serde_json::json!({ "task_id": task_id.to_string() }),
        ))
        .expect("task.show before succeeds");

    let verified = handler
        .handle_command(cmd(
            scope_id,
            "task.verify",
            serde_json::json!({
                "task_id": task_id.to_string(),
                "verdict": "pass",
                "note": "looks correct",
            }),
        ))
        .expect("task.verify succeeds");
    assert_eq!(verified.result["recorded"], true);

    let after = handler
        .handle_query(qry(
            scope_id,
            "task.show",
            serde_json::json!({ "task_id": task_id.to_string() }),
        ))
        .expect("task.show after succeeds");
    assert_eq!(
        before.result["status"], after.result["status"],
        "a verdict must not transition the run"
    );
}
