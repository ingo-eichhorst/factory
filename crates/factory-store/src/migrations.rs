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
///
/// Migration 3 (backlog §7, `schema::V3_SCHEMA`) carries the same
/// `.foreign_key_check()` for the same reason: it rebuilds `tasks`, which
/// `task_delegation_chain` and `delivery_attempts` reference.
fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(schema::V1_SCHEMA),
        M::up(schema::V2_SCHEMA).foreign_key_check(),
        M::up(schema::V3_SCHEMA).foreign_key_check(),
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
        // Was `2` before migration 3 (backlog §7) was appended; a fresh
        // database now runs migrations 1, 2, and 3, landing on `user_version
        // = 3`.
        assert_eq!(fresh.schema_version().unwrap(), 3);

        let migrated_path = dir.path().join("migrated.sqlite");
        seed_schema_1_database(&migrated_path, "");
        let migrated = Store::open_at(&migrated_path).expect("open migrated store");
        assert_eq!(migrated.schema_version().unwrap(), 3);

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
        // Was `2` before migration 3 (backlog §7) was appended; opening this
        // schema-1 database now also runs migration 3, landing on
        // `user_version = 3`.
        assert_eq!(store.schema_version().unwrap(), 3);

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

    /// Builds a schema-2 database directly from `schema::V1_SCHEMA` followed
    /// by `schema::V2_SCHEMA`, bypassing `Store` (and therefore migration 3)
    /// entirely, then pins it at `user_version = 2`.
    ///
    /// Mirrors [`seed_schema_1_database`]'s own reasoning, one migration
    /// later: a fresh database runs migrations 1, 2, and 3 back to back
    /// inside one shared transaction (`to_latest`'s `goto_up`), so it never
    /// exercises migration 3 rebuilding a `tasks` table it did not just
    /// create moments earlier in the same script. Seeding a real schema-2
    /// file here — with `tasks` in its pre-migration-3 shape, with no
    /// `assigned_session_id` or `cancel_requested_at` column at all — and
    /// then opening it is what actually exercises the create/copy/drop/
    /// rename dance the way an upgrade of a database left behind by an
    /// earlier release would. Calling `Store::open_at` twice would not: the
    /// first call already carries a fresh database straight to `user_version
    /// = 3` in one transaction, so a second call has nothing pending to
    /// apply.
    fn seed_schema_2_database(path: &std::path::Path, extra_sql: &str) {
        let conn = Connection::open(path).expect("open seed database");
        conn.execute_batch(schema::V1_SCHEMA)
            .expect("apply V1 schema");
        conn.execute_batch(schema::V2_SCHEMA)
            .expect("apply V2 schema");
        if !extra_sql.is_empty() {
            conn.execute_batch(extra_sql).expect("seed extra rows");
        }
        conn.pragma_update(None, "user_version", 2_i64)
            .expect("pin schema-2 database at user_version 2");
    }

    /// The same equivalence migration 2's own
    /// `fresh_and_migrated_from_v1_have_identical_schema` proves, one
    /// migration later: migration 3 must reach the same schema whether a
    /// database is created fresh or migrated forward from a real schema-2
    /// database.
    #[test]
    fn fresh_and_migrated_from_v2_have_identical_schema() {
        let dir = tempfile::tempdir().expect("tempdir");

        let fresh = Store::open_at(dir.path().join("fresh.sqlite")).expect("open fresh store");
        assert_eq!(fresh.schema_version().unwrap(), 3);

        let migrated_path = dir.path().join("migrated.sqlite");
        seed_schema_2_database(&migrated_path, "");
        let migrated = Store::open_at(&migrated_path).expect("open migrated store");
        assert_eq!(migrated.schema_version().unwrap(), 3);

        assert_eq!(
            schema_snapshot(&fresh),
            schema_snapshot(&migrated),
            "a database created fresh and one migrated from schema 2 must end \
             up with the same schema"
        );
    }

    /// An empty database proves nothing about migration 3's foreign-key
    /// safety, since `tasks` there has no referencing rows to break. This
    /// seeds a scope, a session, a `queued` task, a `task_delegation_chain`
    /// row, and a `delivery_attempts` row — the exact shapes
    /// `task_delegation_chain.task_id REFERENCES tasks (id)` and
    /// `delivery_attempts.task_id REFERENCES tasks (id)` protect — before
    /// running migration 3's drop/rename, and confirms the data, the new
    /// columns, and both references survive.
    #[test]
    fn migration_3_preserves_referencing_rows_and_stays_foreign_key_clean() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("with_tasks.sqlite");

        let scope_id = "5d8f599e-381a-42a1-929b-628ab6ecded1";
        let session_id = "b6f77e8e-3437-4946-bf47-9d3d35e8aa32";
        let task_id = "b3f0a7d0-3f6c-4b8a-9b7a-6f2f2c9a6a9a";
        let seed = format!(
            "INSERT INTO scopes (id, name, declared_path, canonical_path) \
             VALUES ('{scope_id}', 'irrlicht', '/instance', '/instance');\n\
             INSERT INTO sessions (id, scope_id, agent_name, workspace_path, state) \
             VALUES ('{session_id}', '{scope_id}', 'agent', '/instance', 'running');\n\
             INSERT INTO tasks (id, target_scope_id, target_session_id, prompt, status) \
             VALUES ('{task_id}', '{scope_id}', '{session_id}', 'do it', 'queued');\n\
             INSERT INTO task_delegation_chain (task_id, position, scope_id) \
             VALUES ('{task_id}', 0, '{scope_id}');\n\
             INSERT INTO delivery_attempts (task_id, session_id, outcome) \
             VALUES ('{task_id}', '{session_id}', NULL);"
        );
        seed_schema_2_database(&path, &seed);

        let store = Store::open_at(&path).expect("migration 3 must succeed against real rows");
        assert_eq!(store.schema_version().unwrap(), 3);

        // The queued row survives untouched, and the two new columns exist,
        // reading back NULL (schema 2 never wrote them).
        let (status, assigned_session_id, cancel_requested_at): (
            String,
            Option<String>,
            Option<String>,
        ) = store
            .conn
            .query_row(
                "SELECT status, assigned_session_id, cancel_requested_at FROM tasks WHERE id = ?1",
                [task_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("task row survived the rebuild");
        assert_eq!(status, "queued");
        assert_eq!(assigned_session_id, None);
        assert_eq!(cancel_requested_at, None);

        // task_delegation_chain.task_id and delivery_attempts.task_id still
        // resolve: both foreign keys survived the drop/rename.
        let chain_scope: String = store
            .conn
            .query_row(
                "SELECT scope_id FROM task_delegation_chain WHERE task_id = ?1",
                [task_id],
                |row| row.get(0),
            )
            .expect("delegation chain row survived the rebuild");
        assert_eq!(chain_scope, scope_id);

        let delivery_session: String = store
            .conn
            .query_row(
                "SELECT session_id FROM delivery_attempts WHERE task_id = ?1",
                [task_id],
                |row| row.get(0),
            )
            .expect("delivery attempt row survived the rebuild");
        assert_eq!(delivery_session, session_id);

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

        // The brief requires `tasks_target_scope_id` and `tasks_status` to
        // survive byte for byte, and migration 3 adds a third,
        // `tasks_one_running_per_session`. `DROP TABLE tasks` drops every
        // index on it; the `CREATE INDEX` statements after the rename are
        // what bring them back. Neither `fresh_and_migrated_from_v2_have_
        // identical_schema` nor the CHECK/index behavioural tests in
        // `tests/constraints.rs` can catch a forgotten one here: the schema
        // comparison would be symmetrically missing it on both sides (both
        // come from the same `V3_SCHEMA`), and the behavioural tests only
        // exercise `tasks_one_running_per_session`. This reads
        // `sqlite_master` directly instead.
        let mut index_names_stmt = store
            .conn
            .prepare(
                "SELECT name FROM sqlite_master \
                 WHERE type = 'index' AND tbl_name = 'tasks' ORDER BY name",
            )
            .expect("prepare index listing");
        let index_names = index_names_stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("list tasks indexes")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect tasks indexes");
        assert_eq!(
            index_names,
            vec![
                // SQLite's own autoindex backing `id TEXT PRIMARY KEY`
                // (a TEXT primary key is not the rowid alias, unlike an
                // `INTEGER PRIMARY KEY`, so SQLite creates one implicitly).
                "sqlite_autoindex_tasks_1".to_string(),
                "tasks_one_running_per_session".to_string(),
                "tasks_status".to_string(),
                "tasks_target_scope_id".to_string(),
            ],
            "all three explicit tasks indexes, plus SQLite's own primary-key \
             autoindex, must survive the drop/rename"
        );
    }

    /// The landmine this migration must not leave for an operator upgrading
    /// with a task mid-flight: a schema-2 `running` row has no
    /// `assigned_session_id` to copy forward (the column did not exist yet),
    /// so copying it unchanged would produce `('running', NULL)` — exactly
    /// what the new CHECK on `tasks_v3` rejects — and migration 3 would fail
    /// outright rather than complete. This proves the migration instead
    /// rewrites that row to `blocked: interrupted`, per design §5's own
    /// remedy for ambiguous running work, and that the row still carries no
    /// `assigned_session_id` afterward (there is still nothing to assign it
    /// to).
    ///
    /// It also proves `updated_at`'s half of the same rewrite. `updated_at`
    /// is the one column an operator reads to find what an upgrade touched;
    /// copying it verbatim on a rewritten row would leave it pointing at
    /// whenever the task was last worked on, not at the migration that just
    /// changed its status. So a rewritten row must get a new `updated_at`
    /// (the `moved` assertion below), while the `queued` task alongside it —
    /// which the migration only copies, never rewrites — must keep its own
    /// (the `untouched` assertion). Both halves matter: without the second,
    /// a migration that stamped every row with a fresh `updated_at`
    /// regardless of whether it changed anything would still pass, and the
    /// timestamp would stop being a signal of what the upgrade actually
    /// touched.
    #[test]
    fn migration_3_rewrites_a_schema_2_running_task_to_blocked_interrupted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("with_running_task.sqlite");

        let scope_id = "5d8f599e-381a-42a1-929b-628ab6ecded1";
        let session_id = "b6f77e8e-3437-4946-bf47-9d3d35e8aa32";
        let task_id = "b3f0a7d0-3f6c-4b8a-9b7a-6f2f2c9a6a9a";
        let queued_task_id = "1c2d3e4f-5a6b-4c8d-9e0f-1a2b3c4d5e6f";
        // Deliberately far in the past, so no plausible clock skew could make
        // `CURRENT_TIMESTAMP` collide with either value.
        let running_updated_at = "2000-01-01 00:00:00";
        let queued_updated_at = "2000-01-02 00:00:00";
        let seed = format!(
            "INSERT INTO scopes (id, name, declared_path, canonical_path) \
             VALUES ('{scope_id}', 'irrlicht', '/instance', '/instance');\n\
             INSERT INTO sessions (id, scope_id, agent_name, workspace_path, state) \
             VALUES ('{session_id}', '{scope_id}', 'agent', '/instance', 'running');\n\
             INSERT INTO tasks (id, target_scope_id, target_session_id, prompt, status, updated_at) \
             VALUES ('{task_id}', '{scope_id}', '{session_id}', 'do it', 'running', \
                     '{running_updated_at}');\n\
             INSERT INTO tasks (id, target_scope_id, prompt, status, updated_at) \
             VALUES ('{queued_task_id}', '{scope_id}', 'wait', 'queued', '{queued_updated_at}');"
        );
        seed_schema_2_database(&path, &seed);

        let store = Store::open_at(&path)
            .expect("migration 3 must not abort on a pre-existing running row");
        assert_eq!(store.schema_version().unwrap(), 3);

        let (status, blocked_reason, assigned_session_id): (
            String,
            Option<String>,
            Option<String>,
        ) = store
            .conn
            .query_row(
                "SELECT status, blocked_reason, assigned_session_id FROM tasks WHERE id = ?1",
                [task_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("task row survived the rebuild");
        assert_eq!(status, "blocked");
        assert_eq!(blocked_reason.as_deref(), Some("interrupted"));
        assert_eq!(
            assigned_session_id, None,
            "there is still nothing to assign an ambiguous pre-migration row to"
        );

        // `updated_at` is the one column an operator reads to find what an
        // upgrade touched. A rewritten row must move it; a row the migration
        // only copied must not, or the signal becomes noise on every row.
        let moved: String = store
            .conn
            .query_row(
                "SELECT updated_at FROM tasks WHERE id = ?1",
                [task_id],
                |row| row.get(0),
            )
            .expect("rewritten task row");
        assert_ne!(moved, running_updated_at);

        let untouched: String = store
            .conn
            .query_row(
                "SELECT updated_at FROM tasks WHERE id = ?1",
                [queued_task_id],
                |row| row.get(0),
            )
            .expect("queued task row survived the rebuild");
        assert_eq!(untouched, queued_updated_at);
    }
}
