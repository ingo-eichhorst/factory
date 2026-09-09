//! The full backup drill from ADR 0012 decision 4: "a backup is not verified
//! until it has been restored." Checks integrity, `user_version`, row
//! counts, and — the step that actually matters — that the snapshot opens
//! read-write and accepts a write.
//!
//! ADR 0018 decision 1 moves the first three (inspection) steps through
//! `Store::open_read_only`: inspecting a snapshot must not be the thing that
//! migrates it, which opening it through `Store::open_at` — as this test did
//! before that ADR — would silently do. Only the fourth step, which exists
//! specifically to prove the snapshot also works as a live database, opens
//! read-write.

mod common;

use factory_store::{Store, StoreError};

const TABLES: &[&str] = &["scopes", "sessions", "workspace_leases", "tasks"];

fn row_counts(store: &mut Store, tables: &[&str]) -> Vec<i64> {
    let tx = store.transaction().expect("begin read");
    let counts = tables
        .iter()
        .map(|table| common::row_count(&tx, table).expect("count"))
        .collect();
    tx.commit().expect("commit read-only transaction");
    counts
}

/// The same row-count check, but through a borrowed connection rather than
/// [`Store::transaction`] — the only thing a [`Store::open_read_only`]
/// handle can offer, since a write-lock transaction is unreachable through
/// it by design.
fn row_counts_read_only(store: &Store, tables: &[&str]) -> Vec<i64> {
    tables
        .iter()
        .map(|table| common::row_count(store.connection(), table).expect("count"))
        .collect()
}

#[test]
fn backup_and_restore_drill() {
    let company_root = tempfile::tempdir().expect("company root tempdir");
    let mut store = Store::open(company_root.path()).expect("open");

    // Populate every table the drill checks row counts for.
    {
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
            "/company/root/projects/irrlicht",
            "running",
        )
        .expect("insert session");
        tx.execute(
            "INSERT INTO workspace_leases (session_id, canonical_workspace_path)
             VALUES ('session-1', '/company/root/projects/irrlicht')",
            [],
        )
        .expect("insert lease journal row");
        common::insert_task(&tx, "task-1", "scope-1", "do the thing", "queued")
            .expect("insert task");
        tx.commit().expect("commit fixture data");
    }

    let source_version = store.schema_version().expect("source schema_version");
    let source_counts = row_counts(&mut store, TABLES);
    assert_eq!(
        source_counts,
        vec![1, 1, 1, 1],
        "fixture data must be exactly one row per table"
    );

    // The scratch directory for the snapshot must be outside the company
    // root: `VACUUM INTO` writes a `-wal`/`-shm`-free single file, but
    // opening *that* file as a `Store` below applies WAL again and would
    // otherwise create sidecars inside the company root, which the
    // no-writes-outside-.factory test (correctly) forbids elsewhere.
    let backup_scratch = tempfile::tempdir().expect("backup scratch tempdir");
    let backup_path = backup_scratch.path().join("factory-backup.sqlite");

    store.backup_to(&backup_path).expect("backup_to");

    // Decision 4: refuses an existing destination rather than overwriting.
    let err = store
        .backup_to(&backup_path)
        .expect_err("a second backup to the same destination must be refused");
    match err {
        StoreError::BackupExists { path, help } => {
            assert_eq!(path, backup_path);
            assert!(!help.is_empty(), "help must state an action");
        }
        other => panic!("expected StoreError::BackupExists, got {other:?}"),
    }

    // The drill: step 1, integrity — through the read-only door (ADR 0018
    // decision 1). Inspecting a snapshot must not be the act that migrates
    // it: `Store::open_at` would apply every pending migration here, and an
    // old snapshot opened later to inspect it would be silently rewritten.
    let inspected = Store::open_read_only(&backup_path).expect("open the snapshot read-only");
    assert_eq!(inspected.integrity_check().expect("integrity_check"), "ok");

    // Step 2, schema version matches the source — and is the version the
    // snapshot was actually taken at, since open_read_only never migrates.
    assert_eq!(
        inspected.schema_version().expect("restored schema_version"),
        source_version
    );

    // Step 3, row counts match the source.
    let restored_counts = row_counts_read_only(&inspected, TABLES);
    assert_eq!(restored_counts, source_counts);
    drop(inspected);

    // Step 4, the snapshot opens read-write and accepts a write. A snapshot
    // that only passes an integrity check can still be unusable as a
    // working database; this is the step that actually exercises that. This
    // is the one place a snapshot may be migrated, and it comes after the
    // user_version comparison above, so that comparison still saw the
    // version the snapshot was taken at.
    let mut restored = Store::open_at(&backup_path).expect("open the snapshot read-write");
    let tx = restored
        .transaction()
        .expect("begin write on the restored snapshot");
    common::insert_scope(&tx, "scope-2", "post-restore", "/somewhere/else")
        .expect("the restored snapshot must accept a write");
    tx.commit().expect("commit on the restored snapshot");
}
