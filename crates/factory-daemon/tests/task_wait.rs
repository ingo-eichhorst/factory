//! `task.wait` is bounded, not indefinite, and expiring must change nothing:
//! waiting is a read, so a timeout leaves the task exactly as it found it —
//! still queued, still running, still delegated. This test snapshots every
//! mutable column on `tasks` (plus the `delivery_attempts` count) before and
//! after a timed-out wait and requires them identical, not just `status`:
//! `updated_at` is what would catch a re-queue landing on the same status,
//! and the attempt count is what would catch a resend.

mod common;

use factory_daemon::Handler as _;
use factory_daemon::envelope::{CommandRequest, QueryRequest};

fn uid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

#[derive(Debug, PartialEq, Eq)]
struct TaskSnapshot {
    status: String,
    blocked_reason: Option<String>,
    assigned_session_id: Option<String>,
    cancel_requested_at: Option<String>,
    authorised_deliveries: i64,
    updated_at: String,
    delivery_attempt_count: i64,
}

fn snapshot(store: &factory_store::Store, task_id: uuid::Uuid) -> TaskSnapshot {
    let conn = store.connection();
    let (
        status,
        blocked_reason,
        assigned_session_id,
        cancel_requested_at,
        authorised_deliveries,
        updated_at,
    ): (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        i64,
        String,
    ) = conn
        .query_row(
            "SELECT status, blocked_reason, assigned_session_id, cancel_requested_at, \
             authorised_deliveries, updated_at FROM tasks WHERE id = ?1",
            [task_id.to_string()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .expect("task row exists");
    let delivery_attempt_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM delivery_attempts WHERE task_id = ?1",
            [task_id.to_string()],
            |row| row.get(0),
        )
        .expect("count delivery_attempts");

    TaskSnapshot {
        status,
        blocked_reason,
        assigned_session_id,
        cancel_requested_at,
        authorised_deliveries,
        updated_at,
        delivery_attempt_count,
    }
}

#[test]
fn a_timed_out_wait_leaves_the_task_completely_unchanged() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");

    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();
    let handler = factory_daemon::FactoryHandler::new(store, adapter, fixture.instance_root());

    let session_id = uid(5001);
    handler
        .handle_command(CommandRequest {
            request_id: uid(1),
            scope_id,
            command: "agent.start".to_string(),
            payload: serde_json::json!({ "session_id": session_id.to_string(), "agent_name": "agent" }),
            expected_revision: None,
        })
        .expect("agent.start succeeds");

    let task_id = uid(5002);
    let sent = handler
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
    assert_eq!(sent.result["status"], "running");

    let before = {
        let store = factory_store::Store::open(fixture.instance_root()).expect("reopen");
        snapshot(&store, task_id)
    };

    // A short timeout: the task stays `running` throughout (nothing here
    // ever calls `task.done`/`task.fail`/`task.block`), so this must expire.
    let waited = handler
        .handle_query(QueryRequest {
            request_id: uid(3),
            scope_id,
            query: "task.wait".to_string(),
            payload: serde_json::json!({ "task_id": task_id.to_string(), "timeout_ms": 300 }),
        })
        .expect("task.wait succeeds");
    assert_eq!(waited.result["timed_out"], true);
    assert_eq!(waited.result["status"], "running");

    let after = {
        let store = factory_store::Store::open(fixture.instance_root()).expect("reopen");
        snapshot(&store, task_id)
    };

    assert_eq!(
        before, after,
        "a timed-out wait is a read: it must leave every mutable column on `tasks`, and the \
         delivery_attempts count, byte-for-byte unchanged"
    );
}
