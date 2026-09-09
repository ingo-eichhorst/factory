//! ADR 0014's Slice 5 prerequisite, and the rule that has held since slice 5
//! and must still hold: `factory_adapter::TaskSignal::{NoChange, Running,
//! Blocked}` is the whole vocabulary, and neither `idle` nor `done` — Herdr's
//! own words for harness state, carried verbatim in
//! `Observation::harness_state` — means a task finished. A task closes only
//! through the push path (`task.done`/`task.fail`), never through this loop.

mod common;

use factory_adapter::{Confidence, Observation, TaskSignal};
use factory_daemon::observe::reconcile_once;

fn uid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

/// Build a `running` session with a `running` task assigned, and an adapter
/// whose `observe()` reports the given `harness_state` string alongside
/// `TaskSignal::NoChange` — the harness went idle/reported done, but said
/// nothing in `factory_adapter`'s own narrower vocabulary about the task.
fn running_session_with_task(
    fixture: &common::Fixture,
    harness_state: &str,
) -> (
    factory_store::Store,
    common::FakeAdapter,
    uuid::Uuid,
    uuid::Uuid,
) {
    let scope_id = fixture.scope("alpha");
    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();
    let handler =
        factory_daemon::FactoryHandler::new(store, adapter.clone(), fixture.instance_root());

    let session_id = uid(2001);
    use factory_daemon::Handler as _;
    handler
        .handle_command(factory_daemon::envelope::CommandRequest {
            request_id: uid(1),
            scope_id,
            command: "agent.start".to_string(),
            payload: serde_json::json!({ "session_id": session_id.to_string(), "agent_name": "agent" }),
            expected_revision: None,
        })
        .expect("agent.start succeeds");

    let task_id = uid(2002);
    let sent = handler
        .handle_command(factory_daemon::envelope::CommandRequest {
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
    assert_eq!(
        sent.result["status"], "running",
        "fixture precondition: the task must actually be running before the loop runs"
    );

    let pane = format!("pane-{session_id}");
    adapter.set_observation(
        &pane,
        factory_adapter::Observation {
            pane: factory_adapter::PaneId(pane.clone()),
            harness_state: harness_state.to_string(),
            confidence: Confidence::Authoritative,
            session_alive: true,
            task_signal: TaskSignal::NoChange,
            transcript_path: None,
            harness_session_id: None,
        },
    );

    // Recover the store the handler was holding — `FactoryHandler` has no
    // teardown API, so open a fresh handle onto the same file for the
    // reconciliation pass under test.
    drop(handler);
    let store = factory_store::Store::open(fixture.instance_root()).expect("reopen store");
    (store, adapter, session_id, task_id)
}

#[test]
fn an_idle_observation_never_closes_a_running_task() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let (mut store, adapter, _session_id, task_id) = running_session_with_task(&fixture, "idle");

    reconcile_once(&mut store, &adapter);

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(
        task.status,
        factory_task::TaskStatus::Running,
        "an `idle` harness_state must never close a task — TaskSignal has no such vocabulary"
    );
}

#[test]
fn a_done_observation_never_closes_a_running_task() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let (mut store, adapter, _session_id, task_id) = running_session_with_task(&fixture, "done");

    reconcile_once(&mut store, &adapter);

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(
        task.status,
        factory_task::TaskStatus::Running,
        "a `done` harness_state must never close a task — a task may span many turns, and \
         completion is recorded only from the result an agent reports"
    );
}

/// A `running` session whose pane authoritatively reports the process gone
/// reuses `factory_recovery::harness::harness_crashed` — session moves to
/// `failed` (lease released) and its task to `blocked: interrupted`, never
/// straight to a terminal task status (see `harness.rs`'s own "`TaskSignal`
/// is evidence this module deliberately does not consult" section: crash
/// detection is orthogonal to what a task's own signal says).
#[test]
fn a_confirmed_crash_blocks_the_task_as_interrupted_never_closes_it() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();
    let handler =
        factory_daemon::FactoryHandler::new(store, adapter.clone(), fixture.instance_root());

    let session_id = uid(2101);
    use factory_daemon::Handler as _;
    handler
        .handle_command(factory_daemon::envelope::CommandRequest {
            request_id: uid(1),
            scope_id,
            command: "agent.start".to_string(),
            payload: serde_json::json!({ "session_id": session_id.to_string(), "agent_name": "agent" }),
            expected_revision: None,
        })
        .expect("agent.start succeeds");

    let task_id = uid(2102);
    handler
        .handle_command(factory_daemon::envelope::CommandRequest {
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

    let pane = format!("pane-{session_id}");
    adapter.set_observation(
        &pane,
        Observation {
            pane: factory_adapter::PaneId(pane.clone()),
            harness_state: "gone".to_string(),
            confidence: Confidence::Authoritative,
            session_alive: false,
            task_signal: TaskSignal::NoChange,
            transcript_path: None,
            harness_session_id: None,
        },
    );

    drop(handler);
    let mut store = factory_store::Store::open(fixture.instance_root()).expect("reopen store");
    reconcile_once(&mut store, &adapter);

    let session = factory_session::show(&store, session_id).expect("session exists");
    assert_eq!(session.state, factory_session::SessionState::Failed);

    let task = factory_task::create::show(&store, task_id).expect("task exists");
    assert_eq!(task.status, factory_task::TaskStatus::Blocked);
    assert_eq!(
        task.blocked_reason,
        Some(factory_task::BlockedReason::Interrupted)
    );
}
