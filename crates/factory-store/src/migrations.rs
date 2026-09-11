//! Schema versioning via `PRAGMA user_version` (ADR 0012 decision 2).
//!
//! `rusqlite_migration` tracks schema state in SQLite's own `user_version`
//! rather than a bookkeeping table of its own, so an operator can read the
//! schema version with the `sqlite3` CLI during an incident, with no Factory
//! code and no knowledge of a table layout (Slice 10's `factory doctor`).

use std::path::Path;

use rusqlite::Connection;
use rusqlite_migration::{Error as MigrationError, M, MigrationDefinitionError, Migrations};

use crate::{StoreError, schema, snapshot};

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
///
/// Migrations 4 and 5 (backlog §9, `schema::V4_SCHEMA` / `schema::V5_SCHEMA`)
/// carry no `.foreign_key_check()`: both are measured (see `schema.rs`'s doc
/// comment on `V4_SCHEMA`) to need nothing beyond plain `ALTER TABLE ADD
/// COLUMN`, which neither drops nor recreates the table it targets. There is
/// no reference to `tasks` or `sessions` that either statement could ever
/// invalidate, unlike migrations 2 and 3's drop/rename dance.
///
/// Migration 6 (backlog §11 / ADR 0021, `schema::V6_SCHEMA`) carries none for
/// the same reason, and it was given one by mistake first. Its shape is
/// migrations 4 and 5's, not 2 and 3's: it creates tables that start empty and
/// adds nullable columns to `tasks`, so every new reference it introduces
/// holds NULL on every pre-existing row and cannot dangle.
///
/// Removing it matters because `rusqlite_migration` runs `SELECT * FROM
/// pragma_foreign_key_check` with **no table argument** — read directly from
/// 2.6.0's `fk_check.rs`, not assumed. That is a whole-database scan. A
/// dangling reference left behind by any earlier schema would therefore make
/// migration 6 refuse the upgrade, blaming a migration that neither caused it
/// nor touches the table it is in. An operator would be told an unrelated true
/// thing at the least useful moment.
///
/// Migration 7 (backlog §12 / ADR 0022, `schema::V7_SCHEMA`) carries none
/// either, for a stronger version of migrations 4/5/6's reason: it creates
/// exactly one table — `durable_writes` — and touches no existing one at all.
/// An empty table has no rows yet, so it cannot be the dangling end of a
/// foreign key; there is nothing for `PRAGMA foreign_key_check`'s
/// whole-database scan to usefully confirm here that migrations 4 through 6
/// did not already confirm.
fn migrations() -> Migrations<'static> {
    Migrations::new(migration_list())
}

/// The migration list itself, so that [`latest_schema_version`] can count it
/// without a database connection.
fn migration_list() -> Vec<M<'static>> {
    vec![
        M::up(schema::V1_SCHEMA),
        M::up(schema::V2_SCHEMA).foreign_key_check(),
        M::up(schema::V3_SCHEMA).foreign_key_check(),
        M::up(schema::V4_SCHEMA),
        M::up(schema::V5_SCHEMA),
        M::up(schema::V6_SCHEMA),
        M::up(schema::V7_SCHEMA),
    ]
}

/// The highest schema version this build understands.
///
/// Counted from the migration list rather than written down beside it, so
/// there is no second place to update when a migration is added — and so a
/// caller outside this crate (`factory-doctor`, which reports whether
/// migrations are pending) does not have to keep its own copy of a number
/// only this module can know. `user_version` is 1-based and each migration
/// advances it by one, so the count *is* the version.
#[must_use]
pub fn latest_schema_version() -> i64 {
    i64::try_from(migration_list().len()).unwrap_or(i64::MAX)
}

/// Bring `conn` (backing `db_path`) to the latest schema.
///
/// Safe to call against a fresh database or one already at the latest
/// version: `rusqlite_migration` compares `user_version` against the
/// migration list and applies only what is missing, which is what makes
/// repeated `Store::open` calls idempotent (Slice 2's acceptance criterion).
///
/// Before anything else, this reads `user_version` and asks `rusqlite_migration`
/// how many migrations are pending — the one call this module makes into
/// `pending_migrations`, so `snapshot::snapshot_before_migration` (ADR 0018
/// decision 3) and the `DatabaseTooFarAhead` mapping below (ADR 0018
/// decision 2) both work from the same numbers `to_latest` is about to see,
/// rather than each recomputing them and risking disagreement. The identity
/// `from_version + pending == (the number of migrations this build defines)`
/// holds unconditionally — it is exactly `pending_migrations`'s own formula,
/// `self.ms.len() - user_version` — so it is also how this function learns
/// the schema version *this build* understands, without a second constant
/// that could drift from the `Vec` in [`migrations`].
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
pub(crate) fn apply(conn: &mut Connection, db_path: &Path) -> Result<(), StoreError> {
    let ms = migrations();
    let from_version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let pending = ms.pending_migrations(conn)?;
    let built_schema = from_version + i64::from(pending);

    // Decision 3: the only rollback an append-only migration will ever have.
    // If this fails, the `?` below returns before `to_latest` is ever
    // called — the migration must not run.
    snapshot::snapshot_before_migration(conn, db_path, from_version, pending)?;

    conn.pragma_update(None, "foreign_keys", "OFF")?;
    let outcome = ms.to_latest(conn);
    conn.pragma_update(None, "foreign_keys", "ON")?;
    outcome.map_err(|source| map_database_too_far_ahead(source, from_version, built_schema))
}

/// Map `rusqlite_migration`'s undifferentiated `DatabaseTooFarAhead` into
/// [`StoreError::DatabaseTooFarAhead`], which names both schema numbers and
/// what an operator can do about it (ADR 0018 decision 2). Every other
/// migration error passes through unchanged.
fn map_database_too_far_ahead(
    source: MigrationError,
    database_schema: i64,
    built_schema: i64,
) -> StoreError {
    if matches!(
        source,
        MigrationError::MigrationDefinition(MigrationDefinitionError::DatabaseTooFarAhead)
    ) {
        StoreError::DatabaseTooFarAhead {
            built_schema,
            database_schema,
        }
    } else {
        StoreError::Migration(source)
    }
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
        // Was `3` before migrations 4 and 5 (backlog §9) were appended, and
        // `5` before migration 6 (backlog §11). Asserted against
        // `latest_schema_version()` rather than a literal, because what this
        // test is about is the two databases *agreeing*, not the number.
        assert_eq!(fresh.schema_version().unwrap(), latest_schema_version());

        let migrated_path = dir.path().join("migrated.sqlite");
        seed_schema_1_database(&migrated_path, "");
        let migrated = Store::open_at(&migrated_path).expect("open migrated store");
        assert_eq!(migrated.schema_version().unwrap(), latest_schema_version());

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
        // Was `2`, then `3` (backlog §7); opening this schema-1 database now
        // and every migration appended since, landing on the latest schema.
        assert_eq!(store.schema_version().unwrap(), latest_schema_version());

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
        // Was `3` before migrations 4 and 5 (backlog §9), `5` before
        // migration 6 (backlog §11). See the note in
        // `fresh_and_migrated_from_v1_have_identical_schema`.
        assert_eq!(fresh.schema_version().unwrap(), latest_schema_version());

        let migrated_path = dir.path().join("migrated.sqlite");
        seed_schema_2_database(&migrated_path, "");
        let migrated = Store::open_at(&migrated_path).expect("open migrated store");
        assert_eq!(migrated.schema_version().unwrap(), latest_schema_version());

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
        // Was `3`; opening this schema-2 database now also runs migrations 4
        // and every migration appended since, landing on the latest schema.
        assert_eq!(store.schema_version().unwrap(), latest_schema_version());

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
                // Migration 6 (backlog §11).
                "tasks_one_run_per_schedule_minute".to_string(),
                "tasks_one_running_per_session".to_string(),
                "tasks_status".to_string(),
                "tasks_target_scope_id".to_string(),
                // Migration 6 (backlog §11).
                "tasks_template_id".to_string(),
            ],
            "migration 3's three explicit tasks indexes, plus SQLite's own \
             primary-key autoindex, must survive the drop/rename -- and every \
             later migration's indexes must be listed here deliberately, so \
             that adding one cannot hide dropping another"
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
        // Was `3`; opening this schema-2 database now also runs migrations 4
        // and every migration appended since, landing on the latest schema.
        assert_eq!(store.schema_version().unwrap(), latest_schema_version());

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

    // Migration 4 (backlog §9) -----------------------------------------

    /// Builds a schema-3 database directly from `schema::V1_SCHEMA` through
    /// `schema::V3_SCHEMA`, bypassing `Store` (and therefore migrations 4 and
    /// 5) entirely, then pins it at `user_version = 3`.
    ///
    /// Mirrors [`seed_schema_1_database`] and [`seed_schema_2_database`]'s
    /// own reasoning, two migrations later: a fresh database now runs
    /// migrations 1 through 5 back to back inside one shared transaction, so
    /// it never exercises migration 4 running `ADD COLUMN` against a `tasks`
    /// table it did not just create moments earlier in the same script.
    /// Seeding a real schema-3 file here — with `tasks` in its
    /// pre-migration-4 shape, with no `authorised_deliveries` column at all —
    /// and then opening it is what actually exercises `ADD COLUMN` the way an
    /// upgrade of a database left behind by slice 7 would.
    fn seed_schema_3_database(path: &std::path::Path, extra_sql: &str) {
        let conn = Connection::open(path).expect("open seed database");
        conn.execute_batch(schema::V1_SCHEMA)
            .expect("apply V1 schema");
        conn.execute_batch(schema::V2_SCHEMA)
            .expect("apply V2 schema");
        conn.execute_batch(schema::V3_SCHEMA)
            .expect("apply V3 schema");
        if !extra_sql.is_empty() {
            conn.execute_batch(extra_sql).expect("seed extra rows");
        }
        conn.pragma_update(None, "user_version", 3_i64)
            .expect("pin schema-3 database at user_version 3");
    }

    /// The same equivalence proved one migration at a time by every sibling
    /// above: migration 4 (and, since a fresh database runs every pending
    /// migration together, migration 5 alongside it) must reach the same
    /// schema whether a database is created fresh or migrated forward from a
    /// real schema-3 database.
    #[test]
    fn fresh_and_migrated_from_v3_have_identical_schema() {
        let dir = tempfile::tempdir().expect("tempdir");

        let fresh = Store::open_at(dir.path().join("fresh.sqlite")).expect("open fresh store");
        assert_eq!(fresh.schema_version().unwrap(), latest_schema_version());

        let migrated_path = dir.path().join("migrated.sqlite");
        seed_schema_3_database(&migrated_path, "");
        let migrated = Store::open_at(&migrated_path).expect("open migrated store");
        assert_eq!(migrated.schema_version().unwrap(), latest_schema_version());

        assert_eq!(
            schema_snapshot(&fresh),
            schema_snapshot(&migrated),
            "a database created fresh and one migrated from schema 3 must end \
             up with the same schema"
        );
    }

    /// An empty database proves nothing about migration 4's backfill or its
    /// foreign-key safety, since `tasks` there has no pre-existing row to
    /// backfill and no referencing row to break. This seeds a scope, a
    /// session, a `queued` task (schema 3's shape — no
    /// `authorised_deliveries` column exists yet to set), and a
    /// `delivery_attempts` row referencing it, then confirms after opening
    /// through `Store` (which also runs migration 5) that: the pre-existing
    /// task backfilled to exactly `authorised_deliveries = 1` (not `0`,
    /// not `NULL` — see `schema::V4_SCHEMA`'s doc comment for the measured
    /// evidence that `ADD COLUMN` performs this backfill in the same
    /// statement as the CHECK it installs); the `delivery_attempts` row
    /// still resolves to the same task; and `PRAGMA foreign_key_check`
    /// reports nothing — `ADD COLUMN` neither drops nor recreates `tasks`,
    /// so this is confirming a fact `ADD COLUMN` should never have been able
    /// to disturb, not probing a rebuild the way migrations 2 and 3's
    /// siblings do.
    #[test]
    fn migration_4_preserves_referencing_rows_and_backfills_authorised_deliveries_to_one() {
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
             INSERT INTO tasks (id, target_scope_id, assigned_session_id, prompt, status) \
             VALUES ('{task_id}', '{scope_id}', '{session_id}', 'do it', 'queued');\n\
             INSERT INTO delivery_attempts (task_id, session_id, outcome) \
             VALUES ('{task_id}', '{session_id}', 'sent');"
        );
        seed_schema_3_database(&path, &seed);

        let store = Store::open_at(&path).expect("migration 4 must succeed against real rows");
        assert_eq!(store.schema_version().unwrap(), latest_schema_version());

        let authorised_deliveries: i64 = store
            .conn
            .query_row(
                "SELECT authorised_deliveries FROM tasks WHERE id = ?1",
                [task_id],
                |row| row.get(0),
            )
            .expect("task row survived migration 4's ADD COLUMN");
        assert_eq!(
            authorised_deliveries, 1,
            "a task that existed before migration 4 must backfill to exactly \
             one authorised delivery — the same authorisation slice 7's guard \
             already assumed it had"
        );

        let delivery_task_id: String = store
            .conn
            .query_row(
                "SELECT task_id FROM delivery_attempts WHERE task_id = ?1",
                [task_id],
                |row| row.get(0),
            )
            .expect("delivery_attempts row survived migration 4 untouched");
        assert_eq!(delivery_task_id, task_id);

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

    // Migration 5 (backlog §9) -----------------------------------------

    /// Builds a schema-4 database directly from `schema::V1_SCHEMA` through
    /// `schema::V4_SCHEMA`, bypassing `Store` (and therefore migration 5)
    /// entirely, then pins it at `user_version = 4`. Mirrors
    /// [`seed_schema_3_database`]'s reasoning one migration later.
    fn seed_schema_4_database(path: &std::path::Path, extra_sql: &str) {
        let conn = Connection::open(path).expect("open seed database");
        conn.execute_batch(schema::V1_SCHEMA)
            .expect("apply V1 schema");
        conn.execute_batch(schema::V2_SCHEMA)
            .expect("apply V2 schema");
        conn.execute_batch(schema::V3_SCHEMA)
            .expect("apply V3 schema");
        conn.execute_batch(schema::V4_SCHEMA)
            .expect("apply V4 schema");
        if !extra_sql.is_empty() {
            conn.execute_batch(extra_sql).expect("seed extra rows");
        }
        conn.pragma_update(None, "user_version", 4_i64)
            .expect("pin schema-4 database at user_version 4");
    }

    /// The same equivalence one migration later again: migration 5 must
    /// reach the same schema whether a database is created fresh or migrated
    /// forward from a real schema-4 database.
    #[test]
    fn fresh_and_migrated_from_v4_have_identical_schema() {
        let dir = tempfile::tempdir().expect("tempdir");

        let fresh = Store::open_at(dir.path().join("fresh.sqlite")).expect("open fresh store");
        assert_eq!(fresh.schema_version().unwrap(), latest_schema_version());

        let migrated_path = dir.path().join("migrated.sqlite");
        seed_schema_4_database(&migrated_path, "");
        let migrated = Store::open_at(&migrated_path).expect("open migrated store");
        assert_eq!(migrated.schema_version().unwrap(), latest_schema_version());

        assert_eq!(
            schema_snapshot(&fresh),
            schema_snapshot(&migrated),
            "a database created fresh and one migrated from schema 4 must end \
             up with the same schema"
        );
    }

    /// The migration-5 twin of `migration_4_preserves_referencing_rows_and_
    /// backfills_authorised_deliveries_to_one`: seeds a schema-4 `sessions`
    /// row (no `herdr_pane_id` or `harness_session_id` column exists yet) and
    /// a `workspace_leases` row referencing it, then confirms after opening
    /// through `Store` that both new columns exist and read back `NULL` for
    /// the pre-existing row (there is no historical value to backfill —
    /// see `schema::V5_SCHEMA`'s doc comment), that the `workspace_leases`
    /// row still resolves to the same session, and that
    /// `PRAGMA foreign_key_check` reports nothing.
    #[test]
    fn migration_5_preserves_referencing_rows_and_backfills_pane_columns_to_null() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("with_sessions.sqlite");

        let scope_id = "5d8f599e-381a-42a1-929b-628ab6ecded1";
        let session_id = "b6f77e8e-3437-4946-bf47-9d3d35e8aa32";
        let seed = format!(
            "INSERT INTO scopes (id, name, declared_path, canonical_path) \
             VALUES ('{scope_id}', 'irrlicht', '/instance', '/instance');\n\
             INSERT INTO sessions (id, scope_id, agent_name, workspace_path, state) \
             VALUES ('{session_id}', '{scope_id}', 'agent', '/instance', 'running');\n\
             INSERT INTO workspace_leases (session_id, canonical_workspace_path) \
             VALUES ('{session_id}', '/instance');"
        );
        seed_schema_4_database(&path, &seed);

        let store = Store::open_at(&path).expect("migration 5 must succeed against real rows");
        assert_eq!(store.schema_version().unwrap(), latest_schema_version());

        let (herdr_pane_id, harness_session_id): (Option<String>, Option<String>) = store
            .conn
            .query_row(
                "SELECT herdr_pane_id, harness_session_id FROM sessions WHERE id = ?1",
                [session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("session row survived migration 5's ADD COLUMN");
        assert_eq!(
            herdr_pane_id, None,
            "a session that existed before migration 5 has never been \
             observed carrying a pane id; NULL is the only honest backfill"
        );
        assert_eq!(harness_session_id, None);

        let lease_session_id: String = store
            .conn
            .query_row(
                "SELECT session_id FROM workspace_leases WHERE session_id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .expect("workspace_leases row survived migration 5 untouched");
        assert_eq!(lease_session_id, session_id);

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

    // ADR 0018 decision 2: `DatabaseTooFarAhead` is Factory's contract --

    /// The probe ADR 0018 describes measuring against `rusqlite_migration`
    /// 2.6.0 directly, but against Factory's real `migrations()` set rather
    /// than the ADR's synthetic two-migration stand-in: a database whose
    /// `user_version` is above the highest release migration must be
    /// refused, and left completely untouched, by the real migration set —
    /// not by a fact about the dependency this test never checks.
    #[test]
    fn a_database_too_far_ahead_of_the_real_migrations_is_refused_and_left_untouched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("too_far_ahead.sqlite");

        // `DatabaseTooFarAhead` is a pure `user_version`-vs-migration-count
        // comparison inside `rusqlite_migration::goto` -- it runs before
        // any SQL touches a table, so the seed only needs a real,
        // non-trivial database (schema 4) pinned one past what this
        // build's `migrations()` defines, not an actual sixth migration.
        seed_schema_4_database(&path, "");
        // One past whatever this build defines, computed rather than written
        // down: a literal here would silently stop testing "too far ahead"
        // the moment the migration it names became a real migration.
        let ahead = latest_schema_version() + 1;
        {
            let conn = Connection::open(&path).expect("open seed database");
            conn.pragma_update(None, "user_version", ahead)
                .expect("pin the database one past the highest release migration");
        }
        let before_bytes = std::fs::read(&path).expect("read db bytes before the refused open");

        let mut conn = Connection::open(&path).expect("open raw connection");
        let err = apply(&mut conn, &path)
            .expect_err("a database ahead of this build's migrations() must be refused");
        match err {
            StoreError::DatabaseTooFarAhead {
                built_schema,
                database_schema,
            } => {
                assert_eq!(
                    built_schema,
                    latest_schema_version(),
                    "the error must name what this build understands"
                );
                assert_eq!(
                    database_schema, ahead,
                    "the database's own version must be named"
                );
            }
            other => panic!("expected StoreError::DatabaseTooFarAhead, got {other:?}"),
        }
        drop(conn);

        let after_bytes = std::fs::read(&path).expect("read db bytes after the refused open");
        assert_eq!(
            before_bytes, after_bytes,
            "a database too far ahead must be left byte-for-byte untouched"
        );
    }

    // ADR 0018 decision 3: pre-migration snapshots ----------------------

    /// Mutation target: delete the "if the snapshot fails, refuse the
    /// migration" branch in `apply` (e.g. by ignoring
    /// `snapshot::snapshot_before_migration`'s `Result` instead of
    /// propagating it with `?`) and this must fail. It seeds a *real*
    /// schema-1 database -- the same fixture
    /// `fresh_and_migrated_from_v1_have_identical_schema` proves migrates
    /// cleanly all the way to schema 5 -- then blocks the snapshot by
    /// occupying `backups`' path with a plain file, so
    /// `std::fs::create_dir_all` fails. With the guard in place, `apply`
    /// must return before ever calling `to_latest`, leaving the database at
    /// schema 1. With the guard deleted, this same fixture migrates
    /// successfully (proven elsewhere), so `user_version` would move to 5 --
    /// which is exactly what this test refuses to see.
    #[test]
    fn migration_does_not_run_when_its_pre_migration_snapshot_cannot_be_written() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("with_data.sqlite");
        seed_schema_1_database(&path, "");

        // Occupy the `backups` directory's own path with a regular file, so
        // `snapshot_before_migration`'s `create_dir_all` cannot create it.
        std::fs::write(dir.path().join("backups"), b"not a directory")
            .expect("occupy the backups path with a file");

        let mut conn = Connection::open(&path).expect("open raw connection");
        let err = apply(&mut conn, &path).expect_err(
            "apply must refuse to migrate when its pre-migration snapshot cannot be written",
        );
        assert!(
            matches!(err, StoreError::Io { .. }),
            "expected StoreError::Io from the blocked snapshot, got {err:?}"
        );
        drop(conn);

        let conn = Connection::open(&path).expect("reopen to verify nothing moved");
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("read back user_version");
        assert_eq!(
            version, 1,
            "a migration must not run at all when its snapshot could not be written"
        );
    }

    /// The positive half of the snapshot guard: seeds a real schema-4
    /// database, opens it through `Store::open_at` (which runs every
    /// migration appended since), and confirms the snapshot this produces is
    /// a genuinely usable rollback artifact: opened through the read-only
    /// door (ADR 0018 decision 1), it reports schema **4** -- the version the
    /// source database was at the moment before migrating, not the version it
    /// ended up at.
    ///
    /// One snapshot, not one per migration: `snapshot_before_migration` runs
    /// once for the whole pending run, so its name spans `4-to-<latest>`
    /// however many migrations that is. The name is asserted against
    /// `latest_schema_version()` for that reason -- a literal here would need
    /// editing with every migration, and an author who edited it without
    /// thinking would not notice if the snapshot had started being taken
    /// per-migration instead.
    #[test]
    fn pre_migration_snapshot_is_a_usable_schema_4_rollback_artifact() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("with_sessions.sqlite");
        seed_schema_4_database(&path, "");

        let store = crate::Store::open_at(&path).expect("migrating from 4 must succeed");
        assert_eq!(store.schema_version().unwrap(), latest_schema_version());
        drop(store);

        let backups_dir = dir.path().join("backups");
        let mut entries: Vec<_> = std::fs::read_dir(&backups_dir)
            .expect("backups dir must exist")
            .map(|entry| entry.expect("dir entry").file_name())
            .collect();
        entries.sort();
        assert_eq!(
            entries.len(),
            1,
            "exactly one pre-migration snapshot must be written, got {entries:?}"
        );
        let name = entries[0].to_str().expect("utf8 filename").to_string();
        let expected_prefix = format!("pre-migration-4-to-{}-", latest_schema_version());
        assert!(
            name.starts_with(&expected_prefix) && name.ends_with(".sqlite"),
            "unexpected snapshot name: {name} (expected prefix {expected_prefix})"
        );

        let snapshot_path = backups_dir.join(&name);
        let snapshot = crate::Store::open_read_only(&snapshot_path)
            .expect("the snapshot must open through the read-only door");
        assert_eq!(
            snapshot.schema_version().unwrap(),
            4,
            "the snapshot must preserve the schema version the source database \
             was at immediately before migrating, not the version it ended up at"
        );
    }

    /// Mutation target: delete the fresh-database exemption (`from_version
    /// <= 0`) in `snapshot::snapshot_before_migration` and this must fail.
    /// A brand-new database has nothing to lose, so opening it for the
    /// first time must not create `backups/` at all.
    #[test]
    fn a_fresh_database_gets_no_pre_migration_snapshot() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("fresh.sqlite");

        let store = crate::Store::open_at(&path).expect("open fresh store");
        assert_eq!(store.schema_version().unwrap(), latest_schema_version());

        let backups_dir = dir.path().join("backups");
        assert!(
            !backups_dir.exists(),
            "a fresh database must not get a pre-migration snapshot, found: {backups_dir:?}"
        );
    }

    /// Reopening an already up-to-date database (nothing pending) must not
    /// snapshot it again: `pending <= 0` is the second half of the
    /// exemption, distinct from the fresh-database half above.
    #[test]
    fn reopening_an_up_to_date_database_does_not_snapshot_again() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("factory.sqlite");

        crate::Store::open_at(&path).expect("first open migrates to latest");
        crate::Store::open_at(&path).expect("second open: nothing pending");

        let backups_dir = dir.path().join("backups");
        assert!(
            !backups_dir.exists(),
            "an up-to-date database must not be snapshotted again, found: {backups_dir:?}"
        );
    }
}
