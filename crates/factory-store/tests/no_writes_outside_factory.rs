//! Design §4: "No Factory command writes to a file outside a `.factory/`
//! directory." This test snapshots a tempdir's full recursive listing,
//! exercises open, migrate, a transaction, and a backup, and asserts every
//! path that appears afterward is under `<root>/.factory/`.

mod common;

use std::path::Path;

use factory_store::Store;

#[test]
fn every_created_path_is_under_dot_factory() {
    let company_root = tempfile::tempdir().expect("company root tempdir");
    let root = company_root.path();

    let before = common::recursive_listing(root);
    assert!(before.is_empty(), "tempdir must start empty: {before:?}");

    // Open (creates `.factory/` and the database — schema and migrations
    // applied as part of open).
    let mut store = Store::open(root).expect("open");

    // A transaction that writes.
    {
        let tx = store.transaction().expect("begin");
        common::insert_scope(&tx, "scope-1", "root", root.to_str().expect("utf8 path"))
            .expect("insert scope");
        tx.commit().expect("commit");
    }

    // A backup. `.factory/backups/` is inside `.factory/`, so creating it is
    // permitted; `VACUUM INTO` does not create its own parent directory.
    let backups_dir = root.join(".factory").join("backups");
    std::fs::create_dir_all(&backups_dir).expect("create backups dir");
    store
        .backup_to(backups_dir.join("snapshot.sqlite"))
        .expect("backup_to");

    // Drop the store so SQLite's clean-close WAL checkpoint runs and any
    // `-wal`/`-shm` sidecars are finalized before the final listing, rather
    // than asserting about files that are about to disappear anyway.
    drop(store);

    let after = common::recursive_listing(root);
    let dot_factory = Path::new(".factory");
    for path in &after {
        assert!(
            path.starts_with(dot_factory),
            "found a path outside .factory/: {}",
            path.display()
        );
    }
    assert!(
        after.len() > before.len(),
        "the drill should have created at least the database and the backup"
    );
}
