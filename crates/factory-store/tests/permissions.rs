//! Slice 2 acceptance criterion: "Database file permissions are
//! restrictive." Also checks (and reports, since the task calls for it) the
//! mode SQLite gives the `-wal`/`-shm` sidecar files it creates alongside
//! the database — a 0600 database beside a world-readable WAL file would
//! defeat the point.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use factory_store::Store;

#[test]
fn database_file_is_mode_0600() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path()).expect("open");

    let mode = std::fs::metadata(store.path())
        .expect("stat db file")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "database file must be mode 0600, got {mode:o}");
}

#[test]
fn wal_and_shm_sidecar_modes_are_checked() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    // Force a write so the sidecars are populated, not merely present.
    let tx = store.transaction().expect("begin");
    common::insert_scope(&tx, "scope-1", "root", "/company/root").expect("insert scope");
    tx.commit().expect("commit");

    let db_path = store.path().to_path_buf();
    let wal_path = PathBuf::from(format!("{}-wal", db_path.display()));
    let shm_path = PathBuf::from(format!("{}-shm", db_path.display()));

    // Stat while the `Store` (and its connection) is still open: SQLite
    // removes both sidecars on a clean close, so checking after the store
    // is dropped would find nothing.
    for (name, path) in [("-wal", &wal_path), ("-shm", &shm_path)] {
        let metadata = std::fs::metadata(path)
            .unwrap_or_else(|e| panic!("{name} sidecar must exist while the store is open: {e}"));
        let mode = metadata.permissions().mode() & 0o777;
        // Reported to the crate's report rather than asserted to a specific
        // value here: SQLite, not this crate, decides the sidecars' mode,
        // and that decision is worth recording rather than silently trusting.
        eprintln!("sidecar {name} mode = {mode:o}");
        assert_eq!(
            mode, 0o600,
            "sidecar {name} must not be more permissive than the database file it belongs to"
        );
    }
}
