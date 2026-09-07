//! Slice 2 acceptance criterion: initialization can be run repeatedly
//! without changing a valid existing configuration.

mod common;

use factory_store::Store;

#[test]
fn opening_twice_preserves_data_and_schema_version() {
    let dir = tempfile::tempdir().expect("tempdir");

    let mut store = Store::open(dir.path()).expect("first open");
    let version_after_first_open = store.schema_version().expect("schema_version");
    assert_eq!(
        version_after_first_open, 1,
        "version 1 is a single migration"
    );

    {
        let tx = store.transaction().expect("begin");
        common::insert_scope(&tx, "scope-1", "root", "/company/root").expect("insert scope");
        tx.commit().expect("commit");
    }
    drop(store);

    // Re-open against the same root, as `Store::open` is documented to
    // support (idempotent initialization).
    let mut store_again = Store::open(dir.path()).expect("second open");

    let version_after_second_open = store_again.schema_version().expect("schema_version");
    assert_eq!(
        version_after_second_open, version_after_first_open,
        "schema_version must be stable across repeated opens"
    );

    let tx = store_again.transaction().expect("begin read");
    let count = common::row_count(&tx, "scopes").expect("count scopes");
    tx.commit().expect("commit read-only transaction");
    assert_eq!(
        count, 1,
        "data inserted before the second open must survive it"
    );
}

#[test]
fn opening_twice_does_not_duplicate_the_database_file() {
    let dir = tempfile::tempdir().expect("tempdir");

    let store = Store::open(dir.path()).expect("first open");
    let db_path = store.path().to_path_buf();
    drop(store);

    let store_again = Store::open(dir.path()).expect("second open");
    assert_eq!(store_again.path(), db_path, "same database file both times");
    assert!(db_path.exists());
}
