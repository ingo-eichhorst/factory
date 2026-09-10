//! ADR 0018 decision 3: a `VACUUM INTO` snapshot is taken before any
//! migration that changes an *existing* database. Public-API-level
//! companion to the unit tests in `src/migrations.rs` (which also cover
//! this against real, seeded pre-release schemas and the read-only door).

use factory_store::{Store, StoreError, latest_schema_version};

/// Mutation target: delete the fresh-database exemption
/// (`from_version <= 0`) and this must fail — a brand-new database would
/// then get a pre-migration snapshot it has nothing to gain from.
#[test]
fn a_fresh_database_gets_no_pre_migration_snapshot() {
    let dir = tempfile::tempdir().expect("tempdir");

    let store = Store::open(dir.path()).expect("open a fresh company root");
    assert_eq!(
        store.schema_version().expect("schema_version"),
        latest_schema_version()
    );

    let backups_dir = dir.path().join(".factory").join("backups");
    assert!(
        !backups_dir.exists(),
        "a fresh database must not get a pre-migration snapshot, found: {backups_dir:?}"
    );
}

/// Reopening an already up-to-date database must not snapshot it again:
/// nothing is pending, so decision 3 does not apply.
#[test]
fn reopening_an_up_to_date_database_does_not_snapshot_again() {
    let dir = tempfile::tempdir().expect("tempdir");

    {
        Store::open(dir.path()).expect("first open");
    }
    {
        Store::open(dir.path()).expect("second open: nothing pending");
    }

    let backups_dir = dir.path().join(".factory").join("backups");
    assert!(
        !backups_dir.exists(),
        "an up-to-date database must not be snapshotted again, found: {backups_dir:?}"
    );
}

/// Mutation target: delete the "if the snapshot fails, refuse the
/// migration" branch (e.g. by ignoring the snapshot step's `Result`
/// instead of propagating it) and this must fail. Blocks the snapshot by
/// occupying `.factory/backups`' own path with a plain file, so the
/// directory it needs cannot be created.
///
/// This test's fixture rolls a real, fully-migrated database's
/// `user_version` back by hand (this crate's own per-schema SQL is private
/// to `src/schema.rs`, so a genuinely older schema is not reachable through
/// the public API) — which means the migration this would otherwise trigger
/// is itself doomed to fail on a duplicate-column error once it starts,
/// since the real table shape is already at schema 5. That failure alone
/// would also leave `user_version` unmoved, which is why this asserts the
/// *error type* rather than the version: with the guard in place, `apply`
/// returns `StoreError::Io` from the blocked snapshot *before* attempting
/// anything. With the guard deleted, the error changes to
/// `StoreError::Migration` (or another variant) from the migration attempt
/// itself — proving the migration was in fact attempted, which is exactly
/// what "the migration does not run" forbids. The stronger, unconfounded
/// version of this same guard — using a real pre-release schema so a
/// bypassed guard would let the migration *succeed* outright — lives in
/// `src/migrations.rs`'s
/// `migration_does_not_run_when_its_pre_migration_snapshot_cannot_be_written`.
#[test]
fn migration_does_not_run_when_its_pre_migration_snapshot_cannot_be_written() {
    let dir = tempfile::tempdir().expect("tempdir");

    let db_path = dir.path().join(".factory").join("factory.sqlite");
    {
        Store::open(dir.path()).expect("first open migrates to latest");
    }
    {
        let raw = rusqlite::Connection::open(&db_path).expect("reopen raw");
        raw.pragma_update(None, "user_version", 4_i64)
            .expect("roll user_version back so one migration looks pending");
    }

    std::fs::write(dir.path().join(".factory").join("backups"), b"occupied")
        .expect("occupy the backups path with a file instead of a directory");

    let err = Store::open(dir.path()).map(|_| ()).expect_err(
        "open must refuse to migrate when its pre-migration snapshot cannot be written",
    );
    assert!(
        matches!(err, StoreError::Io { .. }),
        "expected StoreError::Io from the blocked snapshot (proving the migration was never \
         attempted), got {err:?}"
    );
}
