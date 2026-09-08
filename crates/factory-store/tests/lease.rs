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

/// `workspace_path` is declared `TEXT` with no `COLLATE NOCASE`, so SQLite's
/// default `BINARY` collation treats `/x/Workspace` and `/x/workspace` as two
/// distinct strings — the partial unique index above does not, and cannot,
/// see a case-variant alias by itself.
///
/// This is not a bug in the index; it is the reason `factory-session`'s
/// `begin_start` runs its own `(st_dev, st_ino)` scan (ADR 0009's
/// 2026-09-08 correction) before ever reaching this index. Without this test
/// a reader has no way to tell whether that Rust-level scan is load-bearing
/// or decorative — this proves it is load-bearing, by showing what the
/// database alone accepts.
#[test]
fn the_database_index_alone_does_not_see_case_variant_paths() {
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
        "session-1",
        "scope-1",
        "agent",
        "/company/root/projects/irrlicht-case-probe/Workspace",
        "running",
    )
    .expect("first spelling holds the lease");
    // A second session naming the *same directory on a case-insensitive
    // volume*, spelled differently, is accepted by the index alone — BINARY
    // collation compares bytes, not filesystem identity.
    common::insert_session(
        &tx,
        "session-2",
        "scope-1",
        "agent",
        "/company/root/projects/irrlicht-case-probe/workspace",
        "running",
    )
    .expect(
        "a case-variant spelling of the same directory is NOT rejected by \
         the index alone; this is the gap factory-session's (st_dev, st_ino) \
         scan exists to close",
    );
    tx.commit().expect("commit");
}

/// `sessions_one_live_lease_per_workspace` is what rejects an exact-string
/// duplicate — not application code that could be deleted. Proved by
/// removing the index at runtime and showing the identical insert that
/// `a_second_lease_holding_session_on_the_same_workspace_is_rejected` proves
/// fails now succeeds without it.
#[test]
fn dropping_the_lease_index_allows_an_exact_duplicate() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path()).expect("open");

    store
        .connection()
        .execute("DROP INDEX sessions_one_live_lease_per_workspace", [])
        .expect("drop the index that normally enforces exclusivity");

    common::insert_scope(
        store.connection(),
        "scope-1",
        "irrlicht",
        "/company/root/projects/irrlicht",
    )
    .expect("insert scope");
    common::insert_session(
        store.connection(),
        "session-1",
        "scope-1",
        "agent",
        WORKSPACE,
        "running",
    )
    .expect("first session holds the lease");
    common::insert_session(
        store.connection(),
        "session-2",
        "scope-1",
        "agent",
        WORKSPACE,
        "running",
    )
    .expect(
        "with the index gone, an exact-string duplicate lease-holder on the \
         same workspace is wrongly accepted — proving the index, not Rust, \
         is what normally rejects it",
    );
}
