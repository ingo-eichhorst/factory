//! Slice 2 acceptance criterion: "a second supervisor/mutator is rejected or
//! serialized." ADR 0012 decision 3 chooses serialization via WAL,
//! `busy_timeout`, and `BEGIN IMMEDIATE` over outright rejection.
//!
//! This test cannot be made deterministic in *latency* — it deliberately
//! waits out the full `busy_timeout` — but it is deterministic in *outcome*:
//! Store A holds its immediate transaction open for the whole test and never
//! releases it, so Store B's `BEGIN IMMEDIATE` can end only one way, a clean
//! `SQLITE_BUSY` failure once the 5000ms timeout elapses. No threads, no
//! sleeps, no timing assertions — only the returned error code is asserted.

mod common;

use factory_store::{Store, StoreError};

#[test]
fn a_second_writer_is_serialized_to_a_clean_busy_error_never_corruption() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = {
        // Create the database and its schema up front via a throwaway
        // handle, then reopen it as two independent connections below —
        // `busy_timeout` and WAL locking only matter between separate
        // `Connection`s, not within one.
        let store = Store::open(dir.path()).expect("initial open");
        store.path().to_path_buf()
    };

    let mut store_a = Store::open_at(&db_path).expect("open A");
    let mut store_b = Store::open_at(&db_path).expect("open B");

    // A takes the write lock and holds it for the rest of the test.
    let tx_a = store_a.transaction().expect("A begins immediate");
    common::insert_scope(&tx_a, "scope-a", "a", "/a").expect("A inserts");
    // Deliberately not committed or rolled back: A is still an active writer.

    // B must wait up to `busy_timeout` (5000ms) and then fail cleanly,
    // never observe or produce a partially applied write.
    let result = store_b.transaction();
    let err = result.expect_err("B must not be able to write while A holds the lock");
    match err {
        StoreError::Sqlite(rusqlite::Error::SqliteFailure(inner, _)) => {
            assert_eq!(
                inner.code,
                rusqlite::ErrorCode::DatabaseBusy,
                "expected SQLITE_BUSY, got {inner:?}"
            );
        }
        other => panic!("expected a clean SQLITE_BUSY failure, got {other:?}"),
    }

    // A's uncommitted insert must not be visible anywhere, confirming there
    // was no partial application: rolling it back now and checking is the
    // only way to observe that from a test also bound by A's own lock.
    tx_a.rollback().expect("A rolls back");

    let tx_check = store_a.transaction().expect("post-rollback read");
    let count = common::row_count(&tx_check, "scopes").expect("count scopes");
    tx_check.commit().expect("commit read-only transaction");
    assert_eq!(
        count, 0,
        "A's own uncommitted write must not have persisted"
    );
}
