//! Slice 2 acceptance criterion: a failed mutation rolls back completely.

mod common;

use factory_store::Store;

#[test]
fn explicit_rollback_leaves_no_trace() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    {
        let tx = store.transaction().expect("begin immediate");
        common::insert_scope(&tx, "scope-1", "root", "/company/root").expect("insert scope");
        tx.rollback().expect("rollback");
    }

    let tx = store.transaction().expect("begin read");
    let count = common::row_count(&tx, "scopes").expect("count scopes");
    tx.commit().expect("commit read-only transaction");
    assert_eq!(count, 0, "a rolled-back insert must not persist");
}

#[test]
fn a_transaction_that_errors_midway_leaves_no_partial_state() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    {
        let tx = store.transaction().expect("begin immediate");
        common::insert_scope(&tx, "scope-1", "root", "/company/root").expect("insert scope");

        // A session state outside the design's exact vocabulary must be
        // rejected by the CHECK constraint — this is the "errors midway"
        // step, deliberately triggered rather than simulated.
        let err = common::insert_session(
            &tx,
            "session-1",
            "scope-1",
            "agent",
            "/company/root",
            "not-a-real-state",
        )
        .expect_err("CHECK constraint must reject an unknown state");
        assert!(
            matches!(
                err,
                rusqlite::Error::SqliteFailure(inner, _)
                    if inner.code == rusqlite::ErrorCode::ConstraintViolation
            ),
            "expected a CHECK constraint violation, got {err:?}"
        );

        // Neither an explicit rollback nor a commit is called: dropping the
        // transaction here rolls it back, exercising the "never commit
        // after an error" path rather than the "we remembered to roll back"
        // path.
    }

    let tx = store.transaction().expect("begin read");
    let scopes = common::row_count(&tx, "scopes").expect("count scopes");
    let sessions = common::row_count(&tx, "sessions").expect("count sessions");
    tx.commit().expect("commit read-only transaction");

    assert_eq!(
        scopes, 0,
        "the scope inserted earlier in the failed transaction must not survive"
    );
    assert_eq!(sessions, 0, "the rejected session must not survive either");
}
