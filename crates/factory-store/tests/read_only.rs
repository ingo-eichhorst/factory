//! ADR 0018 decision 1: `Store::open_read_only` is a second door into the
//! database that applies pragmas and reports the schema version, but never
//! migrates and never accepts a write — in fact, because the SQLite
//! connection itself is opened `SQLITE_OPEN_READ_ONLY`, not by convention.

mod common;

use factory_store::Store;

/// The mutation this guards against: `open_read_only` opening the
/// connection with ordinary (read-write) flags instead of
/// `SQLITE_OPEN_READ_ONLY`. If that flag is dropped, this write succeeds
/// instead of failing at the driver.
#[test]
fn a_write_through_the_read_only_door_fails_at_the_driver() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("factory.sqlite");
    {
        Store::open_at(&db_path).expect("create and migrate the database first");
    }

    let store = Store::open_read_only(&db_path).expect("open_read_only");

    let err = store
        .connection()
        .execute(
            "INSERT INTO scopes (id, name, declared_path, canonical_path) \
             VALUES ('scope-1', 'root', '/root', '/root')",
            [],
        )
        .expect_err("a write issued through the read-only door must fail");

    match err {
        rusqlite::Error::SqliteFailure(inner, _) => {
            assert_eq!(
                inner.code,
                rusqlite::ErrorCode::ReadOnly,
                "expected SQLITE_READONLY, got {inner:?}"
            );
        }
        other => panic!("expected a SqliteFailure(ReadOnly, ..), got {other:?}"),
    }

    // No row must have been written despite the attempt.
    let count: i64 = store
        .connection()
        .query_row("SELECT COUNT(*) FROM scopes", [], |row| row.get(0))
        .expect("count scopes");
    assert_eq!(count, 0, "the rejected write must not have persisted");
}

/// The mutation this guards against: `open_read_only` calling
/// `migrations::apply` (or otherwise mutating `user_version`) the way
/// `open_at` does. A snapshot's whole value is that inspecting it does not
/// change it (ADR 0018's motivating example: the backup drill migrating the
/// very snapshot it was meant to inspect).
#[test]
fn open_read_only_does_not_migrate_an_old_snapshot() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("factory.sqlite");

    {
        let store = Store::open_at(&db_path).expect("create and migrate to latest");
        assert_eq!(store.schema_version().expect("schema_version"), 5);
    }

    // Simulate an old snapshot: roll `user_version` back by hand, bypassing
    // `factory_store` entirely, the way a snapshot taken before the last
    // migration would genuinely be found.
    {
        let raw = rusqlite::Connection::open(&db_path).expect("reopen raw");
        raw.pragma_update(None, "user_version", 1_i64)
            .expect("roll user_version back to simulate an old snapshot");
    }

    let store = Store::open_read_only(&db_path).expect("open_read_only");
    assert_eq!(
        store.schema_version().expect("schema_version"),
        1,
        "open_read_only must report the version it found, not migrate to latest"
    );

    // And the file on disk must genuinely be unchanged, not just the
    // in-memory Store's view of it.
    let raw = rusqlite::Connection::open(&db_path).expect("reopen raw to verify");
    let version: i64 = raw
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("read back user_version");
    assert_eq!(
        version, 1,
        "the file itself must be left at the version it was found at"
    );
}

/// `open_read_only` never creates a database — that's `Store::open`'s job,
/// and it requires write access `open_read_only` does not have.
#[test]
fn open_read_only_does_not_create_a_missing_database() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("does-not-exist.sqlite");

    let err = Store::open_read_only(&db_path)
        .map(|_| ())
        .expect_err("open_read_only must refuse a path with no database");
    assert!(
        matches!(err, factory_store::StoreError::Sqlite(_)),
        "expected StoreError::Sqlite, got {err:?}"
    );
    assert!(
        !db_path.exists(),
        "open_read_only must not create the file it failed to open"
    );
}

/// The positive case: reads that must keep working through the read-only
/// door — `schema_version` and `integrity_check` — against a real,
/// populated, WAL-mode database.
#[test]
fn open_read_only_reads_a_live_wal_database_correctly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    {
        let tx = store.transaction().expect("begin");
        common::insert_scope(&tx, "scope-1", "root", "/company/root").expect("insert scope");
        tx.commit().expect("commit");
    }

    let db_path = store.path().to_path_buf();
    // Keep `store` alive: this also proves open_read_only works against a
    // database with a live writer still holding it open, not only after a
    // clean close.
    let read_only = Store::open_read_only(&db_path).expect("open_read_only while store is open");

    assert_eq!(read_only.schema_version().expect("schema_version"), 5);
    assert_eq!(read_only.integrity_check().expect("integrity_check"), "ok");

    let count: i64 = read_only
        .connection()
        .query_row("SELECT COUNT(*) FROM scopes", [], |row| row.get(0))
        .expect("count scopes");
    assert_eq!(
        count, 1,
        "the read-only door must see committed data from the live writer"
    );
}
