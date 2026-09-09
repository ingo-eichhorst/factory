//! ADR 0018 decision 2: `DatabaseTooFarAhead` is Factory's contract, not an
//! accident of the `rusqlite_migration` dependency. This is the
//! public-API-level twin of the unit test in `src/migrations.rs` — opening
//! through `Store::open_at`, which runs the real, private `migrations()`
//! set, not a synthetic stand-in.

use factory_store::{Store, StoreError};

#[test]
fn opening_a_database_too_far_ahead_fails_and_leaves_the_file_untouched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("factory.sqlite");

    // A real database, migrated all the way to what this build knows: 5.
    {
        let store = Store::open_at(&db_path).expect("open");
        assert_eq!(store.schema_version().expect("schema_version"), 5);
    }

    // Simulate a newer build having migrated this database further than
    // this one's `migrations()` set goes — measured directly against
    // `rusqlite_migration` 2.6.0 in ADR 0018.
    {
        let raw = rusqlite::Connection::open(&db_path).expect("reopen raw");
        raw.pragma_update(None, "user_version", 6_i64)
            .expect("bump user_version beyond what this build's migrations() defines");
    }
    let before_bytes = std::fs::read(&db_path).expect("read db bytes before reopen");

    let err = Store::open_at(&db_path)
        .map(|_| ())
        .expect_err("opening a database ahead of this build's schema must fail");

    let message = err.to_string();
    assert!(
        message.contains('5') && message.contains('6'),
        "the error message must name both schema numbers, got: {message}"
    );
    let lower = message.to_lowercase();
    assert!(
        lower.contains("install") || lower.contains("restore"),
        "ADR 0018 line 108: a message that says only \"database\" is not enough — \
         it must tell an operator what to do. got: {message}"
    );

    match err {
        StoreError::DatabaseTooFarAhead {
            built_schema,
            database_schema,
        } => {
            assert_eq!(built_schema, 5, "this build knows exactly 5 migrations");
            assert_eq!(
                database_schema, 6,
                "the database's own version must be named"
            );
        }
        other => panic!("expected StoreError::DatabaseTooFarAhead, got {other:?}"),
    }

    let after_bytes = std::fs::read(&db_path).expect("read db bytes after reopen");
    assert_eq!(
        before_bytes, after_bytes,
        "a database too far ahead must be left byte-for-byte untouched"
    );
}
