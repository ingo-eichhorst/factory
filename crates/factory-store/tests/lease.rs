//! Slice 2's single most important test: the one-live-lease-per-workspace
//! constraint. Per ADR 0012 decision 5, `starting`, `running`, and
//! `disconnected` hold a workspace's lease; `stopped` and `failed` release
//! it. This is the constraint that stops two harnesses writing in one
//! directory.

mod common;

use factory_store::Store;

const WORKSPACE: &str = "/company/root/projects/irrlicht";

fn assert_unique_violation(err: rusqlite::Error) {
    assert!(
        matches!(
            err,
            rusqlite::Error::SqliteFailure(inner, _)
                if inner.code == rusqlite::ErrorCode::ConstraintViolation
        ),
        "expected a UNIQUE constraint violation from the partial lease index, got {err:?}"
    );
}

#[test]
fn a_second_lease_holding_session_on_the_same_workspace_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let tx = store.transaction().expect("begin");
    common::insert_scope(
        &tx,
        "scope-1",
        "irrlicht",
        "/company/root/projects/irrlicht",
    )
    .expect("insert scope");
    common::insert_session(
        &tx,
        "session-running",
        "scope-1",
        "agent",
        WORKSPACE,
        "running",
    )
    .expect("first session in a lease-holding state must succeed");
    tx.commit().expect("commit");

    // Each of the three lease-holding states must be rejected for the same
    // workspace while the first session still holds it.
    for (session_id, contending_state) in [
        ("session-starting", "starting"),
        ("session-running-2", "running"),
        ("session-disconnected", "disconnected"),
    ] {
        let tx = store.transaction().expect("begin");
        let err = common::insert_session(
            &tx,
            session_id,
            "scope-1",
            "agent",
            WORKSPACE,
            contending_state,
        )
        .expect_err(&format!(
            "a session in state {contending_state} must be rejected for a leased workspace"
        ));
        assert_unique_violation(err);
        tx.rollback().expect("rollback the rejected attempt");
    }
}

#[test]
fn a_released_lease_can_be_reacquired() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let tx = store.transaction().expect("begin");
    common::insert_scope(
        &tx,
        "scope-1",
        "irrlicht",
        "/company/root/projects/irrlicht",
    )
    .expect("insert scope");
    common::insert_session(&tx, "session-1", "scope-1", "agent", WORKSPACE, "running")
        .expect("first session holds the lease");
    tx.commit().expect("commit");

    // Inserting a second session in a releasing state for the *same*
    // workspace must succeed even while the first session's row still
    // exists, because `stopped`/`failed` never held the lease in the first
    // place.
    let tx = store.transaction().expect("begin");
    common::insert_session(&tx, "session-2", "scope-1", "agent", WORKSPACE, "stopped")
        .expect("a `stopped` session on the same workspace must be accepted");
    common::insert_session(&tx, "session-3", "scope-1", "agent", WORKSPACE, "failed")
        .expect("a `failed` session on the same workspace must be accepted");
    tx.commit().expect("commit");

    // Now release the original lease by transitioning its holder to
    // `failed`, and confirm a brand new lease-holding session on the same
    // workspace is then accepted.
    let tx = store.transaction().expect("begin");
    tx.execute(
        "UPDATE sessions SET state = 'failed' WHERE id = 'session-1'",
        [],
    )
    .expect("release the lease");
    common::insert_session(&tx, "session-4", "scope-1", "agent", WORKSPACE, "running")
        .expect("a new session may take the lease once the old holder releases it");
    tx.commit().expect("commit");
}

#[test]
fn duplicate_canonical_scope_paths_are_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let tx = store.transaction().expect("begin");
    common::insert_scope(
        &tx,
        "scope-1",
        "irrlicht",
        "/company/root/projects/irrlicht",
    )
    .expect("first registration succeeds");
    let err = common::insert_scope(
        &tx,
        "scope-2",
        "irrlicht-dup",
        "/company/root/projects/irrlicht",
    )
    .expect_err("a second scope at the same canonical path must be rejected");
    assert_unique_violation(err);
}
