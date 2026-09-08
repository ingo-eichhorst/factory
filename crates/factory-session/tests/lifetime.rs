//! Design §2.2's lifetime rule, applied by `on_task_terminal`: a `permanent`
//! agent's session survives between tasks; a `temporary` agent's session is
//! torn down once its task reaches `done`, `failed`, or `cancelled` —
//! `blocked` explicitly excluded, because "`blocked` is NOT terminal" is
//! backlog §6's own words for "the rule most likely to be got wrong, because
//! 'blocked' reads like an ending."
//!
//! [`on_task_terminal_refuses_to_tear_down_a_session_for_a_blocked_task`] and
//! [`on_task_terminal_refuses_a_session_whose_task_is_merely_running`] are
//! the direct tests of that refusal: `on_task_terminal` reads `tasks.status`
//! itself rather than trusting an enum the caller constructed, specifically
//! so a mutation widening the accepted status set has something to break
//! (see the module docs' "lifetimes" section for why the enum-only design was
//! rejected).

mod common;

use factory_config::Lifetime;
use factory_session::{SessionError, begin_start, mark_running, on_task_terminal};
use factory_store::Store;

/// `Lifetime::Permanent` is a pure no-op: the session stays exactly as it
/// was, still holding its lease, even though its task is genuinely `done`.
#[test]
fn permanent_agent_session_survives_a_terminal_task() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir(&workspace_dir).expect("create");
    let workspace = common::resolve(&workspace_dir);
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());
    let session = common::uid(1);
    begin_start(&mut store, session, scope_id, "agent", 10, &workspace).expect("start");
    mark_running(&mut store, session).expect("readiness observed");

    let task = common::seed_task(&mut store, 1, scope_id, "done", None, Some(session));

    on_task_terminal(&mut store, session, task, Lifetime::Permanent)
        .expect("permanent lifetime is always Ok, whatever the task's status");

    assert_eq!(common::session_state(&store, session), "running");
    let err = begin_start(
        &mut store,
        common::uid(2),
        scope_id,
        "agent",
        10,
        &workspace,
    )
    .expect_err("the permanent session's lease must still be held");
    assert!(matches!(err, SessionError::WorkspaceLeased { .. }));
}

/// `Lifetime::Temporary` tears the session down once its task is confirmed
/// `done`: the session moves to `stopped` and its lease is released.
#[test]
fn temporary_agent_session_is_torn_down_once_its_task_is_done() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir(&workspace_dir).expect("create");
    let workspace = common::resolve(&workspace_dir);
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());
    let session = common::uid(1);
    begin_start(&mut store, session, scope_id, "agent", 10, &workspace).expect("start");
    mark_running(&mut store, session).expect("readiness observed");

    let task = common::seed_task(&mut store, 1, scope_id, "done", None, Some(session));

    on_task_terminal(&mut store, session, task, Lifetime::Temporary)
        .expect("the task is genuinely done, so teardown proceeds");

    assert_eq!(common::session_state(&store, session), "stopped");
    begin_start(
        &mut store,
        common::uid(2),
        scope_id,
        "agent",
        10,
        &workspace,
    )
    .expect("the lease was released by teardown, so a fresh session fits here");
}

/// Backlog §6's sharpest rule, tested explicitly and directly: a temporary
/// agent's task that is merely `blocked` (here: awaiting clarification) must
/// not have its session torn down. `on_task_terminal` must refuse, and the
/// session and its lease must be completely untouched.
///
/// Mutation: widen the `matches!` set in `on_task_terminal` to also accept
/// `"blocked"` — measured (see the task report): this test fails at
/// `.expect_err(...)` on the call itself, because teardown now proceeds and
/// returns `Ok(())` instead of `TaskNotTerminal`.
#[test]
fn on_task_terminal_refuses_to_tear_down_a_session_for_a_blocked_task() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir(&workspace_dir).expect("create");
    let workspace = common::resolve(&workspace_dir);
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());
    let session = common::uid(1);
    begin_start(&mut store, session, scope_id, "agent", 10, &workspace).expect("start");
    mark_running(&mut store, session).expect("readiness observed");

    let task = common::seed_task(
        &mut store,
        1,
        scope_id,
        "blocked",
        Some("clarification"),
        Some(session),
    );

    let err = on_task_terminal(&mut store, session, task, Lifetime::Temporary).expect_err(
        "a task awaiting clarification is not terminal and must not tear down its session",
    );
    assert!(
        matches!(
            &err,
            SessionError::TaskNotTerminal { status, .. } if status == "blocked"
        ),
        "expected TaskNotTerminal naming `blocked`, got {err:?}"
    );

    assert_eq!(
        common::session_state(&store, session),
        "running",
        "the session must be exactly as it was before the refused call"
    );
    let err = begin_start(
        &mut store,
        common::uid(2),
        scope_id,
        "agent",
        10,
        &workspace,
    )
    .expect_err("the lease must still be held — nothing about the refused call may release it");
    assert!(matches!(err, SessionError::WorkspaceLeased { .. }));
}

/// The general case behind the specific `blocked` one above: a task that is
/// merely `running` (not yet even blocked) must equally refuse teardown.
/// This guards against a narrower, wrong fix that special-cases `blocked`
/// alone instead of checking for the three terminal statuses.
#[test]
fn on_task_terminal_refuses_a_session_whose_task_is_merely_running() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir(&workspace_dir).expect("create");
    let workspace = common::resolve(&workspace_dir);
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());
    let session = common::uid(1);
    begin_start(&mut store, session, scope_id, "agent", 10, &workspace).expect("start");
    mark_running(&mut store, session).expect("readiness observed");

    let task = common::seed_task(&mut store, 1, scope_id, "running", None, Some(session));

    let err = on_task_terminal(&mut store, session, task, Lifetime::Temporary)
        .expect_err("a task still `running` has not finished");
    assert!(matches!(
        &err,
        SessionError::TaskNotTerminal { status, .. } if status == "running"
    ));
}
