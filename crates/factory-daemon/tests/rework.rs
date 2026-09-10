//! `task.rework` — station 11 gap 3, §12.2's hook:
//! `factory_task::create::create_rework` had no caller (commit `16d9dc9`)
//! until this operation. The domain rules themselves are proven once, at the
//! domain layer, in `factory-task`'s own `tests/rework.rs`; this file proves
//! the wiring on top of them: that the daemon operation actually reaches
//! those rules, surfaces their refusals with the right code and message, and
//! makes the two choices this station had to make about a rework that the
//! domain signature does not itself dictate — where the prompt comes from,
//! and where the new run's scope comes from.

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

/// Create a run in `scope_id`, deliver it to a fresh session, and report it
/// `done` — a plain, human-queued run with no template at all
/// (`task_templates` is never touched anywhere in this file), which is
/// exactly the ordinary case §12.2 asks a rework to handle.
fn finished_task(
    handler: &factory_daemon::FactoryHandler,
    scope_id: uuid::Uuid,
    task_id: uuid::Uuid,
    session_id: uuid::Uuid,
) {
    handler
        .handle_command(cmd(
            scope_id,
            "agent.start",
            serde_json::json!({ "session_id": session_id.to_string(), "agent_name": "agent" }),
        ))
        .expect("agent.start succeeds");
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
    handler
        .handle_command(cmd(
            scope_id,
            "task.done",
            serde_json::json!({ "task_id": task_id.to_string(), "result_summary": "shipped" }),
        ))
        .expect("task.done succeeds");
}

/// A rework of a run that has not finished is refused, and the message
/// names the status the run is actually in — the same daemon-level proof
/// `task_decision_with_an_empty_rationale_is_refused` gives for its own
/// refusal, one layer up from the domain test
/// (`factory-task/tests/rework.rs::reworking_a_queued_run_is_refused`).
#[test]
fn rework_of_a_non_terminal_run_is_refused_and_names_the_status() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let (handler, _adapter) = new_handler(&fixture);

    // Untargeted send with no idle session yet: stays `queued`, not terminal.
    let queued_task = uid(10001);
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

    let err = handler
        .handle_command(cmd(
            scope_id,
            "task.rework",
            serde_json::json!({
                "task_id": uid(10002).to_string(),
                "prompt": "redo it",
                "reworks_task_id": queued_task.to_string(),
                "rework_finding": "too early",
            }),
        ))
        .unwrap_err();
    assert_eq!(err.code, "conflict.rework_target_not_terminal");
    assert!(
        err.message.contains("queued"),
        "message must name the status: {}",
        err.message
    );

    // Refused before any row was written — the new run must not exist.
    let show_err = handler
        .handle_query(qry(
            scope_id,
            "task.show",
            serde_json::json!({ "task_id": uid(10002).to_string() }),
        ))
        .unwrap_err();
    assert_eq!(show_err.code, "not_found.task");
}

/// A run may not rework itself, and the refusal names that explicitly —
/// `validation.rework_self_reference`, distinct from `not_found.task`, which
/// is what the *same* payload would produce if the self-check were ever
/// removed (there would then be no row yet at `reworks_task_id` for the
/// status read to find). See this task's own report for why that
/// distinction is exactly what the self-rework mutation must be checked
/// against, not merely "some error was returned."
#[test]
fn a_run_may_not_rework_itself_and_the_message_says_so() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let (handler, _adapter) = new_handler(&fixture);

    let id = uid(10101);
    let err = handler
        .handle_command(cmd(
            scope_id,
            "task.rework",
            serde_json::json!({
                "task_id": id.to_string(),
                "prompt": "redo it",
                "reworks_task_id": id.to_string(),
                "rework_finding": "self-reference",
            }),
        ))
        .unwrap_err();
    assert_eq!(err.code, "validation.rework_self_reference");
    assert!(
        err.message.contains("cannot rework itself"),
        "message must say a run cannot rework itself: {}",
        err.message
    );
}

/// The referenced run's own row is untouched by a rework — the daemon-level
/// half of the criterion `factory-task/tests/rework.rs::
/// the_referenced_run_is_untouched_by_a_rework` already proves at the domain
/// layer, including its event list (no daemon query exposes the raw event
/// log, so `task.show`'s full JSON — every column `task_json` carries — is
/// the strongest equality this layer can assert).
///
/// This is also this station's proof that a run with no template at all
/// reworks exactly like any other finished run: `finished_task` never
/// touches `task_templates`, and `task.show` confirms `template_id` is
/// `null` on it both before and after — so the caller-supplies-the-prompt
/// choice (see `ops::task::rework`'s own doc comment) makes "no template"
/// a non-case rather than a refusal.
#[test]
fn reworking_a_run_leaves_its_row_unchanged_even_with_no_template() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let (handler, _adapter) = new_handler(&fixture);

    let old_task = uid(10201);
    let session_id = uid(10202);
    finished_task(&handler, scope_id, old_task, session_id);

    let before = handler
        .handle_query(qry(
            scope_id,
            "task.show",
            serde_json::json!({ "task_id": old_task.to_string() }),
        ))
        .expect("task.show before succeeds");
    assert!(
        before.result["template_id"].is_null(),
        "this fixture never registers a template; the referenced run must have none"
    );

    let new_task = uid(10203);
    let reworked = handler
        .handle_command(cmd(
            scope_id,
            "task.rework",
            serde_json::json!({
                "task_id": new_task.to_string(),
                "prompt": "redo it, handling the edge case this time",
                "reworks_task_id": old_task.to_string(),
                "rework_finding": "missed the edge case",
            }),
        ))
        .expect("reworking a finished, template-less run succeeds");
    assert_eq!(reworked.result["status"], "queued");

    let after = handler
        .handle_query(qry(
            scope_id,
            "task.show",
            serde_json::json!({ "task_id": old_task.to_string() }),
        ))
        .expect("task.show after succeeds");
    assert_eq!(
        before.result, after.result,
        "the referenced run's own row must be byte-for-byte unchanged"
    );

    let new_shown = handler
        .handle_query(qry(
            scope_id,
            "task.show",
            serde_json::json!({ "task_id": new_task.to_string() }),
        ))
        .expect("task.show of the new run succeeds");
    assert_eq!(new_shown.result["status"], "queued");
    assert_eq!(new_shown.result["reworks_task_id"], old_task.to_string());
    assert_eq!(new_shown.result["rework_finding"], "missed the edge case");
    assert_eq!(
        new_shown.result["prompt"],
        "redo it, handling the edge case this time"
    );
    assert!(
        new_shown.result["template_id"].is_null(),
        "a rework's prompt is caller-supplied, never derived from a template"
    );
}

/// The new run's target scope is the caller's, not the reworked run's own —
/// `ops::task::rework`'s own doc comment names this explicitly, as the
/// coordinator asked. A rework escalated to a different scope is a
/// deliberate, supported outcome, not a surprise this layer should refuse.
#[test]
fn rework_can_land_in_a_different_scope_than_the_run_it_reworks() {
    let fixture = common::build(&[
        common::ScopeSpec::new("alpha", "alpha"),
        common::ScopeSpec::new("beta", "beta"),
    ]);
    let alpha = fixture.scope("alpha");
    let beta = fixture.scope("beta");
    let (handler, _adapter) = new_handler(&fixture);

    let old_task = uid(10301);
    let session_id = uid(10302);
    finished_task(&handler, alpha, old_task, session_id);

    let new_task = uid(10303);
    handler
        .handle_command(cmd(
            beta,
            "task.rework",
            serde_json::json!({
                "task_id": new_task.to_string(),
                "prompt": "escalate this to beta",
                "reworks_task_id": old_task.to_string(),
                "rework_finding": "needs beta's own review",
            }),
        ))
        .expect("a rework naming a different scope than the run it reworks succeeds");

    let new_shown = handler
        .handle_query(qry(
            beta,
            "task.show",
            serde_json::json!({ "task_id": new_task.to_string() }),
        ))
        .expect("task.show of the new run succeeds");
    assert_eq!(new_shown.result["target_scope_id"], beta.to_string());
    assert_ne!(
        new_shown.result["target_scope_id"],
        alpha.to_string(),
        "the new run's scope must not be inherited from the run it reworks"
    );

    let old_shown = handler
        .handle_query(qry(
            alpha,
            "task.show",
            serde_json::json!({ "task_id": old_task.to_string() }),
        ))
        .expect("task.show of the reworked run succeeds");
    assert_eq!(
        old_shown.result["target_scope_id"],
        alpha.to_string(),
        "the reworked run's own scope must not move either"
    );
}
