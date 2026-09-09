//! ADR 0014's own open item: "a restarted daemon must treat 'sessions to
//! re-adopt' as normal rather than as a cold start." `startup::build_handler`
//! is where that happens — it must run `factory_recovery::restore::reconcile`
//! before handing back a `FactoryHandler`, so a session left `running` (with
//! its task `running`) by a daemon that never shut down cleanly is found
//! `disconnected` (task `blocked: interrupted`) the moment the next daemon
//! looks, not still claiming to be live.

mod common;

use factory_daemon::Handler as _;
use factory_daemon::envelope::{CommandRequest, QueryRequest};

fn uid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

#[test]
fn build_handler_reconciles_a_session_a_prior_run_left_running() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");

    let session_id = uid(4001);
    let task_id = uid(4002);

    // Simulate a daemon that started a session and a task, then vanished
    // without a clean shutdown (no restore ever ran against this database).
    {
        let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
        let adapter = common::FakeAdapter::new();
        let handler = factory_daemon::FactoryHandler::new(store, adapter, fixture.instance_root());

        handler
            .handle_command(CommandRequest {
                request_id: uid(1),
                scope_id,
                command: "agent.start".to_string(),
                payload: serde_json::json!({ "session_id": session_id.to_string(), "agent_name": "agent" }),
                expected_revision: None,
            })
            .expect("agent.start succeeds");

        handler
            .handle_command(CommandRequest {
                request_id: uid(2),
                scope_id,
                command: "task.send".to_string(),
                payload: serde_json::json!({
                    "task_id": task_id.to_string(),
                    "prompt": "do it",
                    "target_session_id": session_id.to_string(),
                }),
                expected_revision: None,
            })
            .expect("task.send succeeds");
    }

    // Confirm the precondition directly against the database, independent of
    // any handler: the session is `running` and its task is `running`.
    {
        let store = factory_store::Store::open(fixture.instance_root()).expect("reopen store");
        let session = factory_session::show(&store, session_id).expect("session exists");
        assert_eq!(session.state, factory_session::SessionState::Running);
        let task = factory_task::create::show(&store, task_id).expect("task exists");
        assert_eq!(task.status, factory_task::TaskStatus::Running);
    }

    // "Restart the daemon": build a fresh handler over the same database
    // through the real startup path.
    let adapter = common::FakeAdapter::new();
    let handler = factory_daemon::startup::build_handler(fixture.instance_root(), adapter)
        .expect("build_handler succeeds");

    let session_status = handler
        .handle_query(QueryRequest {
            request_id: uid(3),
            scope_id,
            query: "agent.status".to_string(),
            payload: serde_json::json!({ "session_id": session_id.to_string() }),
        })
        .expect("agent.status succeeds");
    assert_eq!(
        session_status.result["session"]["state"], "disconnected",
        "a restart must find this session `disconnected`, never still `running`"
    );

    let task_status = handler
        .handle_query(QueryRequest {
            request_id: uid(4),
            scope_id,
            query: "task.show".to_string(),
            payload: serde_json::json!({ "task_id": task_id.to_string() }),
        })
        .expect("task.show succeeds");
    assert_eq!(task_status.result["status"], "blocked");
    assert_eq!(task_status.result["blocked_reason"], "interrupted");
}
