//! Schema versioning via `PRAGMA user_version` (ADR 0012 decision 2).
//!
//! `rusqlite_migration` tracks schema state in SQLite's own `user_version`
//! rather than a bookkeeping table of its own, so an operator can read the
//! schema version with the `sqlite3` CLI during an incident, with no Factory
//! code and no knowledge of a table layout (Slice 10's `factory doctor`).

use rusqlite::Connection;
use rusqlite_migration::{M, Migrations};

use crate::{StoreError, schema};

/// The full set of released migrations, in order.
///
/// Forward-only and append-only: once released, an entry here is never
/// edited. A schema change ships as a new entry appended after the last one,
/// never as an edit to an existing one. Version 1 is a single migration, so
/// a fresh database's `user_version` is `1`.
///
/// Migration 2 (ADR 0016 / Slice 3, `schema::V2_SCHEMA`) carries
/// `.foreign_key_check()`: it rebuilds `scopes`, which `sessions`, `tasks`,
/// and `task_delegation_chain` reference, so `PRAGMA foreign_key_check` is
/// asked to confirm nothing was left dangling before that migration's
/// transaction is allowed to commit.
fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(schema::V1_SCHEMA),
        M::up(schema::V2_SCHEMA).foreign_key_check(),
    ])
}

/// Bring `conn` to the latest schema.
///
/// Safe to call against a fresh database or one already at the latest
/// version: `rusqlite_migration` compares `user_version` against the
/// migration list and applies only what is missing, which is what makes
/// repeated `Store::open` calls idempotent (Slice 2's acceptance criterion).
///
/// `foreign_keys` is turned off for the duration of the batch and back on
/// immediately after, in both the success and failure path. This is
/// `rusqlite_migration`'s own documented pattern, not an improvisation: each
/// migration already runs inside its own transaction, and SQLite defines
/// `PRAGMA foreign_keys` as a no-op when set inside one — so a migration that
/// needs to rebuild a referenced table (migration 2 rebuilds `scopes`) can
/// only get a clean drop/recreate by having the *connection* enter the batch
/// with enforcement already off, never by asking the migration's own SQL to
/// turn it off.
pub(crate) fn apply(conn: &mut Connection) -> Result<(), StoreError> {
    conn.pragma_update(None, "foreign_keys", "OFF")?;
    let outcome = migrations().to_latest(conn);
    conn.pragma_update(None, "foreign_keys", "ON")?;
    outcome?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;
    use crate::Store;

    /// Builds a schema-1 database directly from `schema::V1_SCHEMA`, bypassing
    /// `Store` (and therefore migration 2) entirely, then pins it at
    /// `user_version = 1`.
    ///
    /// This is deliberately not the same as calling `Store::open_at` once on
    /// an empty file: `to_latest` runs every *pending* migration inside one
    /// shared transaction (see `rusqlite_migration`'s `goto_up`), so a fresh
    /// database goes through migrations 1 and 2 back to back in a single
    /// transaction and never exercises migration 2 rebuilding a `scopes`
    /// table it did not just create moments earlier in the same script.
    /// Seeding a real schema-1 file here, then opening it, is what actually
    /// exercises the create/copy/drop/rename dance the way an upgrade of a
    /// database left behind by an earlier release would.
    fn seed_schema_1_database(path: &std::path::Path, extra_sql: &str) {
        let conn = Connection::open(path).expect("open seed database");
        conn.execute_batch(schema::V1_SCHEMA)
            .expect("apply V1 schema");
        if !extra_sql.is_empty() {
            conn.execute_batch(extra_sql).expect("seed extra rows");
        }
        conn.pragma_update(None, "user_version", 1_i64)
            .expect("pin schema-1 database at user_version 1");
    }

    /// Every `(type, name, tbl_name, sql)` row from `sqlite_master`, sorted so
    /// the comparison does not depend on creation order.
    fn schema_snapshot(store: &Store) -> Vec<(String, String, String, Option<String>)> {
        let mut stmt = store
            .conn
            .prepare("SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name")
            .expect("prepare sqlite_master query");
        stmt.query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .expect("query sqlite_master")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect sqlite_master rows")
    }

    /// ADR 0016 requires migration 2 to reach the same schema whether a
    /// database is created fresh or migrated forward from schema 1 — the only
    /// way to know the create/copy/drop/rename dance in `schema::V2_SCHEMA`
    /// did not diverge from what a fresh `V1_SCHEMA` + `V2_SCHEMA` run
    /// produces.
    #[test]
    fn fresh_and_migrated_from_v1_have_identical_schema() {
        let dir = tempfile::tempdir().expect("tempdir");

        let fresh = Store::open_at(dir.path().join("fresh.sqlite")).expect("open fresh store");
        assert_eq!(fresh.schema_version().unwrap(), 2);

        let migrated_path = dir.path().join("migrated.sqlite");
        seed_schema_1_database(&migrated_path, "");
        let migrated = Store::open_at(&migrated_path).expect("open migrated store");
        assert_eq!(migrated.schema_version().unwrap(), 2);

        assert_eq!(
            schema_snapshot(&fresh),
            schema_snapshot(&migrated),
            "a database created fresh and one migrated from schema 1 must end \
             up with the same schema"
        );
    }

    /// An empty database proves nothing about migration 2's foreign-key
    /// safety, since `scopes` there has no referencing rows to break. This
    /// seeds a `scopes` row and a `sessions` row that references it — the
    /// exact shape `sessions.scope_id REFERENCES scopes (id)` protects —
    /// before running migration 2's drop/rename, and confirms both the data
    /// and the reference survive.
    #[test]
    fn migration_2_preserves_referencing_rows_and_stays_foreign_key_clean() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("with_data.sqlite");

        let scope_id = "5d8f599e-381a-42a1-929b-628ab6ecded1";
        let session_id = "b6f77e8e-3437-4946-bf47-9d3d35e8aa32";
        let seed = format!(
            "INSERT INTO scopes (id, name, canonical_path) VALUES ('{scope_id}', 'root', '/instance');\n\
             INSERT INTO sessions (id, scope_id, agent_name, workspace_path, state) \
             VALUES ('{session_id}', '{scope_id}', 'agent', '/instance', 'running');"
        );
        seed_schema_1_database(&path, &seed);

        let store = Store::open_at(&path).expect("migration 2 must succeed against real rows");
        assert_eq!(store.schema_version().unwrap(), 2);

        // declared_path backfills from schema 1's only path fact.
        let (declared_path, canonical_path, dev, ino, parent_id): (
            String,
            Option<String>,
            Option<i64>,
            Option<i64>,
            Option<String>,
        ) = store
            .conn
            .query_row(
                "SELECT declared_path, canonical_path, dev, ino, parent_id FROM scopes WHERE id = ?1",
                [scope_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("scope row survived the rebuild");
        assert_eq!(declared_path, "/instance");
        assert_eq!(canonical_path.as_deref(), Some("/instance"));
        assert_eq!(dev, None);
        assert_eq!(ino, None);
        assert_eq!(parent_id, None);

        // sessions.scope_id still resolves: the FK survived the drop/rename.
        let still_referenced: String = store
            .conn
            .query_row(
                "SELECT scope_id FROM sessions WHERE id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .expect("session row survived the rebuild");
        assert_eq!(still_referenced, scope_id);

        let mut fk_check = store
            .conn
            .prepare("PRAGMA foreign_key_check")
            .expect("prepare foreign_key_check");
        let violations = fk_check
            .query_map([], |row| row.get::<_, String>(0))
            .expect("run foreign_key_check")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect foreign_key_check rows");
        assert!(
            violations.is_empty(),
            "foreign_key_check reported violations: {violations:?}"
        );
    }
}
