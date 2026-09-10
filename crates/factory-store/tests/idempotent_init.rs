//! Slice 2 acceptance criterion: initialization can be run repeatedly
//! without changing a valid existing configuration.

mod common;

use factory_store::{Store, latest_schema_version};

#[test]
fn opening_twice_preserves_data_and_schema_version() {
    let dir = tempfile::tempdir().expect("tempdir");

    let mut store = Store::open(dir.path()).expect("first open");
    let version_after_first_open = store.schema_version().expect("schema_version");
    assert_eq!(
        version_after_first_open,
        latest_schema_version(),
        "a first open must land on the latest released schema: 1 (Slice 2), \
         2, the registry projection columns on `scopes` (ADR 0016 / Slice 3), \
         3, the `tasks` columns and rules for single-session delivery \
         (backlog §7), 4, `tasks.authorised_deliveries` (backlog §9), 5, the \
         `sessions` pane/harness identity columns (backlog §9), and 6, task \
         templates, run fields, the audit log and cron schedules \
         (backlog §11 / ADR 0021). The count itself is pinned in \
         `database_too_far_ahead.rs`, deliberately in one place"
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
