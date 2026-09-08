//! Session state transitions and the lease effects that follow them (design
//! §2.3, ADR 0012 decision 5, backlog §6).
//!
//! `factory-store`'s own `tests/lease.rs` proves the database index rejects
//! an exact-string duplicate. This file proves the layer above it: that
//! `factory_session` records `starting` — and therefore holds the lease —
//! *before* any launch attempt, that only the documented transitions are
//! reachable, and that a lease is released exactly when a session leaves a
//! holding state and never otherwise.

mod common;

use factory_session::{
    SessionError, SessionState, begin_start, fail, mark_disconnected, mark_running, stop,
};
use factory_store::Store;

/// ADR 0012 decision 5's own justification for why `starting` must hold the
/// lease: "If the lease were taken only on `running`, two concurrent starts
/// would both pass the check and race for one workspace." This test is that
/// scenario, played out through `factory_session`'s API rather than raw SQL:
/// the second `begin_start` is attempted while the first session is still
/// `starting`, never having reached `running`.
#[test]
fn a_second_start_is_blocked_while_the_first_is_only_starting() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir(&workspace_dir).expect("create workspace");
    let workspace = common::resolve(&workspace_dir);
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());

    begin_start(
        &mut store,
        common::uid(1),
        scope_id,
        "agent",
        10,
        &workspace,
    )
    .expect("first start succeeds and records `starting`");

    let err = begin_start(
        &mut store,
        common::uid(2),
        scope_id,
        "agent",
        10,
        &workspace,
    )
    .expect_err(
        "a second start for the same workspace must be blocked while the first \
         is `starting`, not just once it reaches `running`",
    );
    assert!(
        matches!(
            err,
            SessionError::WorkspaceLeased {
                holder_state: SessionState::Starting,
                ..
            }
        ),
        "expected WorkspaceLeased naming the `starting` holder, got {err:?}"
    );
}

/// `starting` → `running` → `stopped` is the ordinary happy path, and each
/// step's lease effect is asserted independently rather than only checking
/// the end state: `running` still holds it (a third party is blocked), and
/// `stopped` releases it (a fresh start on the same workspace then
/// succeeds).
#[test]
fn happy_path_holds_through_running_and_releases_on_stop() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir(&workspace_dir).expect("create workspace");
    let workspace = common::resolve(&workspace_dir);
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());
    let session = common::uid(1);

    begin_start(&mut store, session, scope_id, "agent", 10, &workspace).expect("start");
    mark_running(&mut store, session).expect("readiness observed");

    let blocked = begin_start(
        &mut store,
        common::uid(2),
        scope_id,
        "agent",
        10,
        &workspace,
    )
    .expect_err("`running` still holds the lease");
    assert!(matches!(blocked, SessionError::WorkspaceLeased { .. }));

    stop(&mut store, session, "graceful shutdown").expect("stop releases the lease");

    begin_start(
        &mut store,
        common::uid(3),
        scope_id,
        "agent",
        10,
        &workspace,
    )
    .expect("once `stopped`, the workspace is free for a new session");
}

/// A launch that never reaches `running` must not leak the lease. Slice 6's
/// own acceptance criterion: "a failed start [must leave] no leaked lease."
#[test]
fn a_failed_start_releases_the_lease() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir(&workspace_dir).expect("create workspace");
    let workspace = common::resolve(&workspace_dir);
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());
    let session = common::uid(1);

    begin_start(&mut store, session, scope_id, "agent", 10, &workspace).expect("start");
    fail(
        &mut store,
        session,
        "harness exited before reporting readiness",
    )
    .expect("fail releases");

    begin_start(
        &mut store,
        common::uid(2),
        scope_id,
        "agent",
        10,
        &workspace,
    )
    .expect("a failed start must not leak the lease it took");
}

/// ADR 0012 decision 5's most consequential case: `disconnected` HOLDS the
/// lease. Losing sight of a session is not evidence its process is gone, and
/// a second harness starting in the same directory while the first may
/// still be writing to it is corruption, not a nuisance.
#[test]
fn disconnected_still_holds_the_lease() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir(&workspace_dir).expect("create workspace");
    let workspace = common::resolve(&workspace_dir);
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());
    let session = common::uid(1);

    begin_start(&mut store, session, scope_id, "agent", 10, &workspace).expect("start");
    mark_running(&mut store, session).expect("readiness observed");
    mark_disconnected(&mut store, session).expect("Factory loses sight of the session");

    let err = begin_start(
        &mut store,
        common::uid(2),
        scope_id,
        "agent",
        10,
        &workspace,
    )
    .expect_err("a `disconnected` session must still block a new start on its workspace");
    assert!(matches!(
        err,
        SessionError::WorkspaceLeased {
            holder_state: SessionState::Disconnected,
            ..
        }
    ));

    // The mechanism Slice 9's stale-lease recovery is expected to call:
    // `disconnected` can still resolve to `stopped`, releasing the lease.
    stop(
        &mut store,
        session,
        "operator: unrecoverable, clearing stale lease",
    )
    .expect("disconnected -> stopped is a valid recovery transition");
    begin_start(
        &mut store,
        common::uid(3),
        scope_id,
        "agent",
        10,
        &workspace,
    )
    .expect("the lease is free once the disconnected session is administratively stopped");
}

/// Only the transitions the module docs name are reachable. `running` cannot
/// jump back to `starting`, and a terminal state (`stopped`, `failed`)
/// cannot move at all.
#[test]
fn illegal_transitions_are_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir(&workspace_dir).expect("create workspace");
    let workspace = common::resolve(&workspace_dir);
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());
    let session = common::uid(1);
    begin_start(&mut store, session, scope_id, "agent", 10, &workspace).expect("start");
    mark_running(&mut store, session).expect("readiness observed");

    // running -> running (no-op reachability): rejected. There is no
    // identity transition in the table.
    let err = mark_running(&mut store, session)
        .expect_err("`running` -> `running` is not a listed transition");
    assert!(matches!(err, SessionError::InvalidTransition { .. }));

    stop(&mut store, session, "done").expect("running -> stopped");

    // stopped is terminal.
    let err = mark_running(&mut store, session)
        .expect_err("`stopped` must not be able to restart in place");
    assert!(matches!(
        err,
        SessionError::InvalidTransition {
            from: SessionState::Stopped,
            to: SessionState::Running,
            ..
        }
    ));
}

/// The permanent half of the crash-restart asymmetry the module docs
/// describe (see `interrupt` and its module-docs section in `src/lib.rs`): a
/// session that Factory loses sight of can also simply regain sight,
/// `disconnected -> running`, with no task-level consequence at all. This is
/// the path a **permanent** agent's crash recovery uses — restarted freely,
/// because it carries no in-flight delivery whose external effect is in
/// question — in direct contrast to `tests/interrupted.rs`'s temporary-agent
/// tests, which from the same `disconnected` starting point instead call
/// `interrupt` and never come back.
///
/// `mark_running`'s own doc comment already claims to serve both
/// `starting -> running` and `disconnected -> running` "because both leave
/// the lease held throughout" — this is the first test to actually exercise
/// the second half of that claim; every other test in this file only drives
/// `starting -> running`.
#[test]
fn disconnected_session_can_regain_sight_which_is_how_a_permanent_agent_recovers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir(&workspace_dir).expect("create workspace");
    let workspace = common::resolve(&workspace_dir);
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());
    let session = common::uid(1);

    begin_start(&mut store, session, scope_id, "agent", 10, &workspace).expect("start");
    mark_running(&mut store, session).expect("readiness observed");
    mark_disconnected(&mut store, session).expect("Factory loses sight of the session");

    // The lease was held throughout `disconnected`, per ADR 0012 decision 5,
    // so it was never available for anyone else in the meantime.
    mark_running(&mut store, session)
        .expect("disconnected -> running: Factory regains sight of the same session");
    assert_eq!(common::session_state(&store, session), "running");

    // Still held, unbroken, the whole way through — no release ever
    // happened, unlike `interrupt`.
    let err = begin_start(
        &mut store,
        common::uid(2),
        scope_id,
        "agent",
        10,
        &workspace,
    )
    .expect_err("the lease was never released across the whole disconnected/running cycle");
    assert!(matches!(err, SessionError::WorkspaceLeased { .. }));
}

/// `starting -> disconnected` is deliberately absent from the transition
/// table (see the module docs): a start that never reached an observed
/// `running` has no confirmed process to lose sight of, so a launch that
/// does not pan out is honestly `failed`.
#[test]
fn starting_cannot_go_directly_to_disconnected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir(&workspace_dir).expect("create workspace");
    let workspace = common::resolve(&workspace_dir);
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());
    let session = common::uid(1);
    begin_start(&mut store, session, scope_id, "agent", 10, &workspace).expect("start");

    let err = mark_disconnected(&mut store, session)
        .expect_err("`starting` -> `disconnected` must be rejected");
    assert!(matches!(
        err,
        SessionError::InvalidTransition {
            from: SessionState::Starting,
            to: SessionState::Disconnected,
            ..
        }
    ));
}
