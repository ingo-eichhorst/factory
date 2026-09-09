//! Crate docs decision 6: when a task with `sender_scope_id.is_some()`
//! reaches a terminal state, the delegating scope's `running` session is
//! told, carrying nothing but the task id and its outcome.
//!
//! Built through `queue_from_session` (an agent-to-agent delegation), not
//! `queue_from_human` — a human-queued task's chain is `[target]`, non-empty
//! but sent by nobody a notice could reach, so a fixture built that way would
//! prove nothing about this feature (`sender_scope_id` is `None` for it).

mod common;

use factory_daemon::Handler as _;
use factory_daemon::envelope::CommandRequest;

fn uid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

fn command(scope_id: uuid::Uuid, command: &str, payload: serde_json::Value) -> CommandRequest {
    CommandRequest {
        request_id: uid(9999),
        scope_id,
        command: command.to_string(),
        payload,
        expected_revision: None,
    }
}

#[test]
fn a_delegated_tasks_completion_notifies_the_delegating_session() {
    // `alpha` and `beta` are both direct children of the fixture's root
    // scope, hence siblings — design §6 allows an agent to target a
    // registered sibling scope.
    let fixture = common::build(&[
        common::ScopeSpec::new("alpha", "alpha"),
        common::ScopeSpec::new("beta", "beta"),
    ]);
    let alpha = fixture.scope("alpha");
    let beta = fixture.scope("beta");

    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();
    let handler =
        factory_daemon::FactoryHandler::new(store, adapter.clone(), fixture.instance_root());

    let alpha_session = uid(3001);
    handler
        .handle_command(command(
            alpha,
            "agent.start",
            serde_json::json!({ "session_id": alpha_session.to_string(), "agent_name": "agent" }),
        ))
        .expect("agent.start (alpha) succeeds");

    let beta_session = uid(3002);
    handler
        .handle_command(command(
            beta,
            "agent.start",
            serde_json::json!({ "session_id": beta_session.to_string(), "agent_name": "agent" }),
        ))
        .expect("agent.start (beta) succeeds");

    let task_id = uid(3003);
    let sent = handler
        .handle_command(command(
            beta,
            "task.send",
            serde_json::json!({
                "task_id": task_id.to_string(),
                "prompt": "please do this for alpha",
                "sender_session_id": alpha_session.to_string(),
                "agent_name": "agent",
            }),
        ))
        .expect("task.send (delegated) succeeds");
    assert_eq!(sent.result["status"], "running");

    // Sanity: the task really is delegated (`sender_scope_id` is `Some`),
    // which is the predicate that actually matters — not merely "the chain
    // has more than one entry" (design §6: a human-queued task's chain is
    // already `[target]`, non-empty, but delegates nothing).
    let shown = handler
        .handle_query(factory_daemon::envelope::QueryRequest {
            request_id: uid(1),
            scope_id: beta,
            query: "task.show".to_string(),
            payload: serde_json::json!({ "task_id": task_id.to_string() }),
        })
        .expect("task.show succeeds");
    assert_eq!(shown.result["sender_scope_id"], alpha.to_string());

    // Clear the delivery calls recorded so far (the original prompt to
    // `beta_session`) so the assertion below is about the notice alone.
    let calls_before = adapter.send_calls().len();

    handler
        .handle_command(command(
            beta,
            "task.done",
            serde_json::json!({ "task_id": task_id.to_string(), "result_summary": "done for alpha" }),
        ))
        .expect("task.done succeeds");

    let calls_after = adapter.send_calls();
    let alpha_pane = format!("pane-{alpha_session}");
    let notice = calls_after
        .iter()
        .skip(calls_before)
        .find(|(pane, id, _)| pane == &alpha_pane && *id == task_id);

    assert!(
        notice.is_some(),
        "expected a notice sent to alpha's session (pane {alpha_pane}) about task {task_id}; \
         calls after task.done: {calls_after:?}"
    );
    let (_, _, text) = notice.unwrap();
    assert!(
        text.contains(&task_id.to_string()) && text.contains("done"),
        "the notice must carry the task id and its outcome, nothing else readable back: {text:?}"
    );
}
