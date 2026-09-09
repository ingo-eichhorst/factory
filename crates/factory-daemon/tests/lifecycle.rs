//! End-to-end coverage of the operations most other tests build on:
//! `agent.start`, `task.send` (assign + deliver in one call), `task.done`
//! (which tears down a `temporary` agent's session), `task.list`/`task.show`,
//! and `agent.status`.

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

#[test]
fn agent_start_task_send_delivers_and_marks_running() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();
    let handler = FactoryHandler::new(store, adapter.clone(), fixture.instance_root());

    let session_id = common::seed(1);
    let start = handler
        .handle_command(command(
            scope_id,
            "agent.start",
            serde_json::json!({ "session_id": session_id.to_string(), "agent_name": "agent" }),
        ))
        .expect("agent.start succeeds");
    assert_eq!(start.result["state"], "running");

    let task_id = common::seed(2);
    let sent = handler
        .handle_command(command(
            scope_id,
            "task.send",
            serde_json::json!({
                "task_id": task_id.to_string(),
                "prompt": "do the thing",
                "agent_name": "agent",
            }),
        ))
        .expect("task.send succeeds");
    assert_eq!(sent.result["status"], "running");
    assert_eq!(sent.result["assignment"]["kind"], "assigned");
    assert_eq!(sent.result["delivery"]["sent"], true);

    let calls = adapter.send_calls();
    assert_eq!(calls.len(), 1, "adapter.send must be called exactly once");
    assert_eq!(calls[0].1, task_id);
    assert!(
        calls[0].2.contains(&task_id.to_string()),
        "the rendered prompt must contain the task id: {}",
        calls[0].2
    );

    let shown = handler
        .handle_query(query(
            scope_id,
            "task.show",
            serde_json::json!({ "task_id": task_id.to_string() }),
        ))
        .expect("task.show succeeds");
    assert_eq!(shown.result["status"], "running");
    assert_eq!(shown.result["assigned_session_id"], session_id.to_string());
}

#[test]
fn task_done_tears_down_a_temporary_agents_session_but_not_a_permanent_ones() {
    let fixture = common::build(&[
        common::ScopeSpec::new("perm", "perm"),
        common::ScopeSpec::new("temp", "temp").temporary(),
    ]);

    for (scope_name, expect_stopped) in [("perm", false), ("temp", true)] {
        let scope_id = fixture.scope(scope_name);
        let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
        let adapter = common::FakeAdapter::new();
        let handler = FactoryHandler::new(store, adapter, fixture.instance_root());

        let session_id = uuid::Uuid::parse_str(&format!(
            "00000000-0000-4000-8000-2000000000{:02}",
            if expect_stopped { 1 } else { 0 }
        ))
        .unwrap();
        handler
            .handle_command(command(
                scope_id,
                "agent.start",
                serde_json::json!({ "session_id": session_id.to_string(), "agent_name": "agent" }),
            ))
            .expect("agent.start succeeds");

        let task_id = uuid::Uuid::parse_str(&format!(
            "00000000-0000-4000-8000-3000000000{:02}",
            if expect_stopped { 1 } else { 0 }
        ))
        .unwrap();
        handler
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

        let done = handler
            .handle_command(command(
                scope_id,
                "task.done",
                serde_json::json!({ "task_id": task_id.to_string(), "result_summary": "ok" }),
            ))
            .expect("task.done succeeds");
        assert_eq!(done.result["status"], "done");

        let status = handler
            .handle_query(query(
                scope_id,
                "agent.status",
                serde_json::json!({ "session_id": session_id.to_string() }),
            ))
            .expect("agent.status succeeds");
        let state = status.result["session"]["state"].as_str().unwrap();
        if expect_stopped {
            assert_eq!(
                state, "stopped",
                "a temporary agent's session must be torn down"
            );
        } else {
            assert_eq!(state, "running", "a permanent agent's session must survive");
        }
    }
}
