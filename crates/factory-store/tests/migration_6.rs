//! Migration 6 (ADR 0021 / backlog §11): task templates, the run fields on
//! `tasks`, the append-only audit log, cron schedules, and the dispatcher's
//! one-row heartbeat.
//!
//! Every test here is written against one line of `V6_SCHEMA` it is meant to
//! catch. Where a mutation was actually run against a hand-written copy of
//! the relevant SQL in the scratchpad (not against this crate's own
//! `schema.rs`, which is off limits), the test's doc comment says so and
//! names what was seen. Where it was only reasoned about, the comment says
//! that instead.
//!
//! `tasks_one_run_per_schedule_minute` gets the most tests: backlog §11's
//! acceptance criterion is "one cron schedule creates no more than one run
//! for the same local minute," and ADR 0021 decision 3 is explicit that this
//! has to be a property of the database, not of the dispatcher's memory,
//! "including after dispatcher restarts."

mod common;

use rusqlite::Connection;

use factory_store::{Store, latest_schema_version};

/// Every CHECK, UNIQUE index, NOT NULL, and FOREIGN KEY violation this file
/// exercises surfaces through `rusqlite` as the same primary error code.
/// `constraints.rs` and `lease.rs` each define their own copy of this rather
/// than sharing one, since every file under `tests/` is compiled as its own
/// crate; this file follows the same convention.
fn assert_constraint_violation(err: rusqlite::Error) {
    assert!(
        matches!(
            err,
            rusqlite::Error::SqliteFailure(inner, _)
                if inner.code == rusqlite::ErrorCode::ConstraintViolation
        ),
        "expected a CHECK/UNIQUE/NOT NULL/FOREIGN KEY constraint violation, got {err:?}"
    );
}

/// Insert one `task_templates` row and one `schedules` row referencing it,
/// so a `tasks_one_run_per_schedule_minute` test only has to write the
/// `tasks` row it actually cares about.
fn insert_template_and_schedule(conn: &Connection, template_id: &str, schedule_id: &str) {
    conn.execute(
        "INSERT INTO task_templates (id, name, prompt) VALUES (?1, ?1, 'do it')",
        [template_id],
    )
    .expect("insert task template");
    conn.execute(
        "INSERT INTO schedules (id, template_id, cron, timezone) VALUES (?1, ?2, '* * * * *', 'UTC')",
        (schedule_id, template_id),
    )
    .expect("insert schedule");
}

// tasks_one_run_per_schedule_minute (ADR 0021 decision 3, backlog §11) -----
//
// Every task in this section is left `status = 'queued'` with no
// `assigned_session_id`, so `tasks_one_running_per_session` (migration 3)
// can never be the thing that rejects an insert here.

/// The acceptance criterion itself: "one cron schedule creates no more than
/// one run for the same local minute." Mutation caught: deleting
/// `tasks_one_run_per_schedule_minute` entirely, or loosening it so a
/// duplicate `(schedule_id, fired_for_minute)` pair is no longer unique.
/// Verified empirically in the scratchpad against a hand-written copy of
/// this table and index: the second insert fails with `UNIQUE constraint
/// failed: tasks.schedule_id, tasks.fired_for_minute`; dropping the index
/// lets it through.
#[test]
fn two_cron_runs_for_the_same_schedule_minute_are_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");

    common::insert_scope(&tx, "scope-1", "irrlicht", "/instance").expect("insert scope");
    insert_template_and_schedule(&tx, "tmpl-1", "sched-1");

    tx.execute(
        "INSERT INTO tasks (id, target_scope_id, prompt, status, schedule_id, fired_for_minute, triggered_by)
         VALUES ('task-1', 'scope-1', 'do it', 'queued', 'sched-1', '2026-09-10T09:00', 'cron')",
        [],
    )
    .expect("the first run for this schedule and minute must be accepted");

    let err = tx
        .execute(
            "INSERT INTO tasks (id, target_scope_id, prompt, status, schedule_id, fired_for_minute, triggered_by)
             VALUES ('task-2', 'scope-1', 'do it', 'queued', 'sched-1', '2026-09-10T09:00', 'cron')",
            [],
        )
        .expect_err("a second run for the same schedule and the same minute must be rejected");
    assert_constraint_violation(err);
}

/// The other half of the same acceptance criterion: a schedule firing once a
/// minute must be able to produce a new run every minute. Mutation caught:
/// an index or CHECK that ties uniqueness to `schedule_id` alone, ignoring
/// `fired_for_minute`, which would make every run after the first for a
/// schedule collide. Verified empirically in the scratchpad: both inserts
/// succeed against the real two-column index.
#[test]
fn two_cron_runs_for_the_same_schedule_different_minutes_are_accepted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");

    common::insert_scope(&tx, "scope-1", "irrlicht", "/instance").expect("insert scope");
    insert_template_and_schedule(&tx, "tmpl-1", "sched-1");

    tx.execute(
        "INSERT INTO tasks (id, target_scope_id, prompt, status, schedule_id, fired_for_minute, triggered_by)
         VALUES ('task-1', 'scope-1', 'do it', 'queued', 'sched-1', '2026-09-10T09:00', 'cron')",
        [],
    )
    .expect("the 09:00 run must be accepted");
    tx.execute(
        "INSERT INTO tasks (id, target_scope_id, prompt, status, schedule_id, fired_for_minute, triggered_by)
         VALUES ('task-2', 'scope-1', 'do it', 'queued', 'sched-1', '2026-09-10T09:01', 'cron')",
        [],
    )
    .expect("the 09:01 run for the same schedule must also be accepted");
    tx.commit().expect("commit");
}

/// Two unrelated schedules are allowed to both fire in the same minute; only
/// a duplicate for the *same* schedule is forbidden. Mutation caught: an
/// index built on `fired_for_minute` alone, dropping `schedule_id` from it.
/// Verified empirically in the scratchpad: with the index narrowed to
/// `(fired_for_minute)` (keeping the same `WHERE`), the two tests above
/// still both pass -- they never exercise a second schedule -- but this
/// test fails, because the narrowed index treats the two schedules' 09:01
/// runs as the same key.
#[test]
fn two_different_schedules_firing_the_same_minute_are_both_accepted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");

    common::insert_scope(&tx, "scope-1", "irrlicht", "/instance").expect("insert scope");
    insert_template_and_schedule(&tx, "tmpl-1", "sched-1");
    insert_template_and_schedule(&tx, "tmpl-2", "sched-2");

    tx.execute(
        "INSERT INTO tasks (id, target_scope_id, prompt, status, schedule_id, fired_for_minute, triggered_by)
         VALUES ('task-1', 'scope-1', 'do it', 'queued', 'sched-1', '2026-09-10T09:01', 'cron')",
        [],
    )
    .expect("schedule 1's 09:01 run must be accepted");
    tx.execute(
        "INSERT INTO tasks (id, target_scope_id, prompt, status, schedule_id, fired_for_minute, triggered_by)
         VALUES ('task-2', 'scope-1', 'do it', 'queued', 'sched-2', '2026-09-10T09:01', 'cron')",
        [],
    )
    .expect("a different schedule's own 09:01 run must also be accepted");
    tx.commit().expect("commit");
}

/// Every ordinary manual task has NULL in both `schedule_id` and
/// `fired_for_minute`, and there is no bound on how many of those may
/// exist. This is honestly a weaker guarantee than it looks: SQLite treats
/// every NULL in a unique index as distinct from every other NULL, so even
/// a plain (non-partial) `UNIQUE (schedule_id, fired_for_minute)` index,
/// with the `WHERE` clause deleted outright, still accepts every row here
/// -- verified empirically in the scratchpad. The `WHERE` clause is
/// therefore not what this test can prove is load-bearing.
///
/// What this test does catch: the "helpful" rewrite that replaces the
/// partial index with a plain expression index over
/// `(COALESCE(schedule_id, ''), COALESCE(fired_for_minute, ''))`, which is
/// how someone might try to express "ignore NULLs" without a `WHERE`
/// clause. That rewrite collapses every manual task onto the same `('',
/// '')` key. Verified empirically in the scratchpad: under that rewrite,
/// the second manual task ever inserted is rejected with `UNIQUE
/// constraint failed`.
#[test]
fn many_manual_tasks_with_null_schedule_and_minute_are_all_accepted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");

    common::insert_scope(&tx, "scope-1", "irrlicht", "/instance").expect("insert scope");

    for i in 0..5 {
        common::insert_task(&tx, &format!("task-{i}"), "scope-1", "do it", "queued")
            .unwrap_or_else(|err| panic!("manual task {i} must be accepted: {err}"));
    }
    tx.commit().expect("commit");
}

// triggered_by CHECK (ADR 0021's V6_SCHEMA doc comment, measurement 2) -----

/// Mutation caught: deleting `CHECK (triggered_by IN ('manual', 'cron'))`
/// from the `ADD COLUMN` statement. Verified empirically in the scratchpad:
/// against a table with the CHECK, this insert fails with `CHECK
/// constraint failed: triggered_by IN ('manual','cron')`; against a copy
/// with the CHECK removed, it succeeds.
#[test]
fn triggered_by_outside_the_vocabulary_is_rejected_on_insert() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");

    common::insert_scope(&tx, "scope-1", "irrlicht", "/instance").expect("insert scope");
    let err = tx
        .execute(
            "INSERT INTO tasks (id, target_scope_id, prompt, status, triggered_by)
             VALUES ('task-1', 'scope-1', 'do it', 'queued', 'webhook')",
            [],
        )
        .expect_err("a triggered_by outside manual|cron must be rejected on insert");
    assert_constraint_violation(err);
}

/// The same CHECK, exercised on `UPDATE` rather than `INSERT` -- schema.rs's
/// own doc comment on `V6_SCHEMA` cites this as measurement 2, "the CHECK
/// bites on the next write ... not merely on newly created tables." Mutation
/// caught: the same one as the insert test, from the other direction.
/// Verified empirically in the scratchpad alongside the insert case.
#[test]
fn triggered_by_outside_the_vocabulary_is_rejected_on_update() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");

    common::insert_scope(&tx, "scope-1", "irrlicht", "/instance").expect("insert scope");
    common::insert_task(&tx, "task-1", "scope-1", "do it", "queued").expect("insert task");

    let err = tx
        .execute(
            "UPDATE tasks SET triggered_by = 'webhook' WHERE id = 'task-1'",
            [],
        )
        .expect_err("updating triggered_by to a value outside manual|cron must be rejected");
    assert_constraint_violation(err);
}

/// Mutation caught: changing `DEFAULT 'manual'` to something else, or
/// dropping the default so an insert that omits the column fails instead
/// (a `NOT NULL` column with no default rejects an insert that never names
/// it). Verified empirically in the scratchpad: an insert that omits
/// `triggered_by` reads back `'manual'`.
///
/// The other half of "defaults to manual" -- a *pre-existing* schema-5 row
/// backfilling to `'manual'` when migration 6's `ADD COLUMN` runs, as
/// opposed to a fresh insert defaulting to it -- is
/// `a_schema_5_database_survives_migration_6_with_rows_intact`, below.
#[test]
fn triggered_by_defaults_to_manual_on_a_fresh_insert() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");

    common::insert_scope(&tx, "scope-1", "irrlicht", "/instance").expect("insert scope");
    common::insert_task(&tx, "task-1", "scope-1", "do it", "queued").expect("insert task");

    let triggered_by: String = tx
        .query_row(
            "SELECT triggered_by FROM tasks WHERE id = 'task-1'",
            [],
            |row| row.get(0),
        )
        .expect("read triggered_by back");
    assert_eq!(triggered_by, "manual");
}

// A pre-existing schema-5 database survives migration 6 -------------------

/// Builds a schema-5 database directly from raw SQL, bypassing `Store` (and
/// therefore migration 6) entirely, then pins it at `user_version = 5`.
///
/// This crate's own per-schema SQL constants (`schema::V1_SCHEMA` through
/// `V5_SCHEMA`) are `pub(crate)`, so they are not reachable from an
/// integration test in `tests/` -- see `pre_migration_snapshot.rs`'s own
/// note on this. What follows is the *effective* shape a real database
/// reaches after migrations 1 through 5: `scopes` and `tasks` in the shape
/// migrations 2 and 3's rebuilds leave them in, plus migration 4's
/// `authorised_deliveries` and migration 5's two `sessions` columns. The
/// migrations.rs unit tests (`fresh_and_migrated_from_v1_have_identical_
/// schema` through `..._from_v4_...`) are what establish that this really
/// is the shape a real upgrade reaches; this seed does not re-derive that,
/// it assumes it.
fn seed_schema_5_database(path: &std::path::Path, extra_sql: &str) {
    let conn = Connection::open(path).expect("open seed database");
    conn.execute_batch(
        r#"
CREATE TABLE scopes (
    id             TEXT PRIMARY KEY,
    name           TEXT NOT NULL,
    declared_path  TEXT NOT NULL,
    canonical_path TEXT UNIQUE,
    git            TEXT,
    dev            INTEGER,
    ino            INTEGER,
    parent_id      TEXT REFERENCES scopes (id),
    created_at     TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE sessions (
    id                 TEXT PRIMARY KEY,
    scope_id           TEXT NOT NULL REFERENCES scopes (id),
    agent_name         TEXT NOT NULL,
    workspace_path     TEXT NOT NULL,
    state              TEXT NOT NULL CHECK (
                           state IN ('stopped', 'starting', 'running', 'disconnected', 'failed')
                       ),
    created_at         TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at         TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    herdr_pane_id      TEXT,
    harness_session_id TEXT
);

CREATE INDEX sessions_scope_id ON sessions (scope_id);

CREATE UNIQUE INDEX sessions_one_live_lease_per_workspace
    ON sessions (workspace_path)
    WHERE state IN ('starting', 'running', 'disconnected');

CREATE TABLE workspace_leases (
    id                       INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id               TEXT NOT NULL REFERENCES sessions (id),
    canonical_workspace_path TEXT NOT NULL,
    acquired_at              TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    released_at              TEXT,
    release_reason           TEXT
);

CREATE INDEX workspace_leases_session_id ON workspace_leases (session_id);

CREATE TABLE tasks (
    id                    TEXT PRIMARY KEY,
    sender_scope_id       TEXT REFERENCES scopes (id),
    target_scope_id       TEXT NOT NULL REFERENCES scopes (id),
    target_session_id     TEXT REFERENCES sessions (id),
    target_workspace_path TEXT,
    assigned_session_id   TEXT REFERENCES sessions (id),
    prompt                TEXT NOT NULL,
    status                TEXT NOT NULL CHECK (
                              status IN ('queued', 'running', 'blocked', 'done', 'failed', 'cancelled')
                          ),
    blocked_reason        TEXT CHECK (
                              (status = 'blocked' AND blocked_reason IS NOT NULL AND blocked_reason IN
                                  ('clarification', 'permission', 'interrupted', 'external'))
                              OR (status <> 'blocked' AND blocked_reason IS NULL)
                          ),
    cancel_requested_at   TEXT,
    result_summary        TEXT,
    result_artifact_paths TEXT,
    created_at            TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at            TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    authorised_deliveries INTEGER NOT NULL DEFAULT 1 CHECK (authorised_deliveries >= 1),
    CHECK (
        (status = 'running' AND assigned_session_id IS NOT NULL) OR (status <> 'running')
    )
);

CREATE INDEX tasks_target_scope_id ON tasks (target_scope_id);
CREATE INDEX tasks_status ON tasks (status);

CREATE UNIQUE INDEX tasks_one_running_per_session
    ON tasks (assigned_session_id)
    WHERE status = 'running';

CREATE TABLE task_delegation_chain (
    task_id  TEXT NOT NULL REFERENCES tasks (id),
    position INTEGER NOT NULL,
    scope_id TEXT NOT NULL REFERENCES scopes (id),
    PRIMARY KEY (task_id, position),
    UNIQUE (task_id, scope_id)
);

CREATE TABLE delivery_attempts (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id      TEXT NOT NULL REFERENCES tasks (id),
    session_id   TEXT NOT NULL REFERENCES sessions (id),
    attempted_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    outcome      TEXT
);

CREATE INDEX delivery_attempts_task_id ON delivery_attempts (task_id);
"#,
    )
    .expect("apply the schema-5 shape");
    if !extra_sql.is_empty() {
        conn.execute_batch(extra_sql).expect("seed extra rows");
    }
    conn.pragma_update(None, "user_version", 5_i64)
        .expect("pin schema-5 database at user_version 5");
}

/// The core proof for "what to prove" item 3: a real database that
/// predates migration 6 keeps every row it had, and every column migration
/// 6 adds reads back exactly as `V6_SCHEMA`'s doc comment promises --
/// `NULL` for every run field except `triggered_by`, which backfills to
/// `'manual'` (the `ADD COLUMN ... DEFAULT 'manual'` half of "defaults to
/// manual"; the fresh-insert half is
/// `triggered_by_defaults_to_manual_on_a_fresh_insert`, above).
///
/// One row is seeded in each table migration 6 could plausibly disturb: a
/// scope, a session, a workspace lease, a task, a delegation-chain row, and
/// a delivery attempt. `PRAGMA foreign_key_check` at the end is what would
/// catch migration 6 silently breaking one of those references -- it never
/// should, since migration 6 only adds columns and tables, but this is the
/// same check `migration_4_preserves_...` and `migration_5_preserves_...`
/// run in `src/migrations.rs` for exactly that reason.
#[test]
fn a_schema_5_database_survives_migration_6_with_rows_intact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("with_data.sqlite");

    let seed = "\
        INSERT INTO scopes (id, name, declared_path, canonical_path) \
            VALUES ('scope-1', 'irrlicht', '/instance', '/instance');\n\
        INSERT INTO sessions (id, scope_id, agent_name, workspace_path, state) \
            VALUES ('session-1', 'scope-1', 'agent', '/instance', 'running');\n\
        INSERT INTO workspace_leases (session_id, canonical_workspace_path) \
            VALUES ('session-1', '/instance');\n\
        INSERT INTO tasks (id, target_scope_id, assigned_session_id, prompt, status) \
            VALUES ('task-1', 'scope-1', 'session-1', 'do it', 'running');\n\
        INSERT INTO task_delegation_chain (task_id, position, scope_id) \
            VALUES ('task-1', 0, 'scope-1');\n\
        INSERT INTO delivery_attempts (task_id, session_id, outcome) \
            VALUES ('task-1', 'session-1', 'sent');";
    seed_schema_5_database(&path, seed);

    let store = Store::open_at(&path).expect("migration 6 must succeed against real rows");
    assert_eq!(store.schema_version().unwrap(), latest_schema_version());

    // The pre-existing rows are all still there.
    let status: String = store
        .connection()
        .query_row("SELECT status FROM tasks WHERE id = 'task-1'", [], |row| {
            row.get(0)
        })
        .expect("task row survived migration 6");
    assert_eq!(status, "running");

    let lease_session: String = store
        .connection()
        .query_row(
            "SELECT session_id FROM workspace_leases WHERE session_id = 'session-1'",
            [],
            |row| row.get(0),
        )
        .expect("workspace_leases row survived migration 6");
    assert_eq!(lease_session, "session-1");

    let chain_scope: String = store
        .connection()
        .query_row(
            "SELECT scope_id FROM task_delegation_chain WHERE task_id = 'task-1'",
            [],
            |row| row.get(0),
        )
        .expect("delegation chain row survived migration 6");
    assert_eq!(chain_scope, "scope-1");

    let delivery_task: String = store
        .connection()
        .query_row(
            "SELECT task_id FROM delivery_attempts WHERE task_id = 'task-1'",
            [],
            |row| row.get(0),
        )
        .expect("delivery attempt row survived migration 6");
    assert_eq!(delivery_task, "task-1");

    // Every new run field on the pre-existing task reads NULL, except
    // `triggered_by`, which backfills to `'manual'`.
    let (template_id, template_version, triggered_by, schedule_id, fired_for_minute): (
        Option<String>,
        Option<i64>,
        String,
        Option<String>,
        Option<String>,
    ) = store
        .connection()
        .query_row(
            "SELECT template_id, template_version, triggered_by, schedule_id, fired_for_minute \
             FROM tasks WHERE id = 'task-1'",
            [],
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
        .expect("read the scheduling run fields back");
    assert_eq!(template_id, None);
    assert_eq!(template_version, None);
    assert_eq!(
        triggered_by, "manual",
        "a task that existed before migration 6 must backfill to 'manual', \
         the only trigger schema 5 could ever have meant"
    );
    assert_eq!(schedule_id, None);
    assert_eq!(fired_for_minute, None);

    let (reworks_task_id, rework_finding): (Option<String>, Option<String>) = store
        .connection()
        .query_row(
            "SELECT reworks_task_id, rework_finding FROM tasks WHERE id = 'task-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read the rework fields back");
    assert_eq!(reworks_task_id, None);
    assert_eq!(rework_finding, None);

    // Split across two queries (rather than one six-column tuple) to keep
    // each query's row-mapping closure a plain, readable tuple type.
    let (cost_model, cost_input_tokens, cost_output_tokens): (
        Option<String>,
        Option<i64>,
        Option<i64>,
    ) = store
        .connection()
        .query_row(
            "SELECT cost_model, cost_input_tokens, cost_output_tokens FROM tasks WHERE id = 'task-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read the token cost fields back");
    assert_eq!(cost_model, None);
    assert_eq!(cost_input_tokens, None);
    assert_eq!(cost_output_tokens, None);

    let (cost_duration_ms, cost_baseline, context_utilization_percent): (
        Option<i64>,
        Option<String>,
        Option<f64>,
    ) = store
        .connection()
        .query_row(
            "SELECT cost_duration_ms, cost_baseline, context_utilization_percent \
             FROM tasks WHERE id = 'task-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read the remaining cost fields back");
    assert_eq!(cost_duration_ms, None);
    assert_eq!(cost_baseline, None);
    assert_eq!(context_utilization_percent, None);

    let mut fk_check = store
        .connection()
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

    // The snapshot migration 6 took beforehand names the version this
    // database really was at. If the seed above were mis-pinned (say, at
    // `user_version = 6` already), migration 6 would never have run at
    // all and every assertion above would be checking a no-op rather than
    // a real migration.
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
    let expected_prefix = format!("pre-migration-5-to-{}-", latest_schema_version());
    assert!(
        name.starts_with(&expected_prefix) && name.ends_with(".sqlite"),
        "unexpected snapshot name: {name} (expected prefix {expected_prefix})"
    );
}

/// The migration-6 twin of `migrations.rs`'s own `fresh_and_migrated_from_
/// vN_have_identical_schema` series, which stops at schema 4. Nothing
/// elsewhere proves a schema-5 database lands on the same schema a fresh
/// one does, and `src/migrations.rs` is off limits, so this is that proof
/// from the public API.
///
/// `tasks`, `scopes`, and the rest of the schema-5 tables are compared by
/// column name and type, not by their `sql` text: this test's own
/// hand-written seed spells their `CREATE TABLE` statements differently
/// (no comments, different whitespace) than the real migration-2/3
/// rebuilds do, so a literal `sql` comparison would false-fail on a
/// difference that carries no meaning. The five tables and six indexes
/// migration 6 itself creates are compared by `sql` text, because both
/// paths execute the identical `V6_SCHEMA` string to create them --
/// nothing about how `tasks` got to schema 5 should be able to change what
/// that string produces.
#[test]
fn fresh_and_migrated_from_v5_have_identical_schema() {
    let dir = tempfile::tempdir().expect("tempdir");

    let fresh = Store::open_at(dir.path().join("fresh.sqlite")).expect("open fresh store");
    assert_eq!(fresh.schema_version().unwrap(), latest_schema_version());

    let migrated_path = dir.path().join("migrated.sqlite");
    seed_schema_5_database(&migrated_path, "");
    let migrated = Store::open_at(&migrated_path).expect("open migrated store");
    assert_eq!(migrated.schema_version().unwrap(), latest_schema_version());

    let object_set = |store: &Store| -> Vec<(String, String, String)> {
        let mut stmt = store
            .connection()
            .prepare("SELECT type, name, tbl_name FROM sqlite_master ORDER BY type, name")
            .expect("prepare sqlite_master query");
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("query sqlite_master")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect sqlite_master rows")
    };
    assert_eq!(
        object_set(&fresh),
        object_set(&migrated),
        "a database created fresh and one migrated from schema 5 must define \
         the same set of tables and indexes"
    );

    let task_columns = |store: &Store| -> Vec<(String, String)> {
        let mut stmt = store
            .connection()
            .prepare("PRAGMA table_info(tasks)")
            .expect("prepare table_info");
        stmt.query_map([], |row| Ok((row.get(1)?, row.get(2)?)))
            .expect("query table_info")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect table_info rows")
    };
    assert_eq!(
        task_columns(&fresh),
        task_columns(&migrated),
        "tasks must end up with the same columns, in the same order, \
         whichever path built the table"
    );

    let migration_6_objects = [
        "task_templates",
        "task_templates_name",
        "schedules",
        "schedules_template_id",
        "task_events",
        "task_events_task_id",
        "task_decisions",
        "task_decisions_task_id",
        "dispatcher_state",
        "tasks_template_id",
        "tasks_one_run_per_schedule_minute",
    ];
    for name in migration_6_objects {
        let sql_for = |store: &Store| -> Option<String> {
            store
                .connection()
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE name = ?1",
                    [name],
                    |row| row.get(0),
                )
                .expect("query object sql")
        };
        assert_eq!(
            sql_for(&fresh),
            sql_for(&migrated),
            "object {name} must have identical CREATE SQL on both paths"
        );
    }
}

// Foreign keys hold (ADR 0021's V6_SCHEMA, every new REFERENCES clause) ----

/// Mutation caught: dropping `task_id TEXT NOT NULL REFERENCES tasks (id)`'s
/// `REFERENCES` clause. Verified empirically in the scratchpad, with
/// `PRAGMA foreign_keys = ON` set on the connection (the same pragma
/// `pragma::apply` sets on every `Store::open`): the insert fails with
/// `FOREIGN KEY constraint failed`.
#[test]
fn task_event_referencing_a_nonexistent_task_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");

    let err = tx
        .execute(
            "INSERT INTO task_events (task_id, event_type) VALUES ('no-such-task', 'created')",
            [],
        )
        .expect_err("an event referencing a task that does not exist must be rejected");
    assert_constraint_violation(err);
}

/// The `task_decisions` twin of the test above. Mutation caught: dropping
/// `task_decisions.task_id`'s `REFERENCES tasks (id)` clause. Verified the
/// same way, in the scratchpad.
#[test]
fn task_decision_referencing_a_nonexistent_task_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");

    let err = tx
        .execute(
            "INSERT INTO task_decisions (id, task_id, decision, rationale)
             VALUES ('decision-1', 'no-such-task', 'do it', 'because')",
            [],
        )
        .expect_err("a decision referencing a task that does not exist must be rejected");
    assert_constraint_violation(err);
}

/// Design §11: "A schedule only creates a task run; it never executes an
/// untracked prompt" -- which only means something if `schedules.template_id`
/// is a real foreign key rather than a plain column. Mutation caught:
/// dropping `REFERENCES task_templates (id)`. Verified the same way, in the
/// scratchpad.
#[test]
fn schedule_referencing_a_nonexistent_task_template_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");

    let err = tx
        .execute(
            "INSERT INTO schedules (id, template_id, cron, timezone)
             VALUES ('sched-1', 'no-such-template', '* * * * *', 'UTC')",
            [],
        )
        .expect_err("a schedule referencing a task template that does not exist must be rejected");
    assert_constraint_violation(err);
}

// task_events.event_type CHECK ---------------------------------------------

/// Mutation caught: any value not in the current twelve-member vocabulary
/// slipping past the CHECK, e.g. from a typo or a future addition made
/// without updating the CHECK. Verified empirically in the scratchpad:
/// against the current CHECK, `'started'` is rejected with `CHECK
/// constraint failed`; with the CHECK removed entirely, it is accepted.
#[test]
fn task_event_type_outside_the_vocabulary_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");

    common::insert_scope(&tx, "scope-1", "irrlicht", "/instance").expect("insert scope");
    common::insert_task(&tx, "task-1", "scope-1", "do it", "queued").expect("insert task");

    let err = tx
        .execute(
            "INSERT INTO task_events (task_id, event_type) VALUES ('task-1', 'started')",
            [],
        )
        .expect_err("an event_type outside the vocabulary must be rejected");
    assert_constraint_violation(err);
}

/// The twelve-member vocabulary is `created, assigned, delivered, refused,
/// running, progress, blocked, done, failed, cancelled, verification,
/// rework` -- station 10 added `refused` between `delivered` and `running`
/// because a refusal (`Adapter::send` declining a busy session) is not a
/// delivery. Mutation caught: a CHECK narrowed to omit one of these, which
/// only naming every member here can catch. Verified empirically in the
/// scratchpad: a CHECK missing `refused` rejects it with `CHECK constraint
/// failed`, while the current, full list accepts all twelve.
#[test]
fn every_event_type_in_the_vocabulary_is_accepted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");

    common::insert_scope(&tx, "scope-1", "irrlicht", "/instance").expect("insert scope");
    common::insert_task(&tx, "task-1", "scope-1", "do it", "queued").expect("insert task");

    const EVENT_TYPES: [&str; 12] = [
        "created",
        "assigned",
        "delivered",
        "refused",
        "running",
        "progress",
        "blocked",
        "done",
        "failed",
        "cancelled",
        "verification",
        "rework",
    ];
    for event_type in EVENT_TYPES {
        tx.execute(
            "INSERT INTO task_events (task_id, event_type) VALUES ('task-1', ?1)",
            [event_type],
        )
        .unwrap_or_else(|err| panic!("event_type {event_type} must be accepted: {err}"));
    }
    tx.commit().expect("commit");
}

// task_templates.name is unique, and version >= 1 -------------------------

/// Design §11: a schedule names one template by its handle across the whole
/// instance, so a name that meant different templates in different places
/// would make that reference ambiguous. Mutation caught: dropping
/// `task_templates_name`. Verified empirically in the scratchpad: without
/// the index, both inserts succeed; with it, the second fails with `UNIQUE
/// constraint failed: task_templates.name`.
#[test]
fn task_template_name_must_be_unique() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");

    tx.execute(
        "INSERT INTO task_templates (id, name, prompt) VALUES ('tmpl-1', 'nightly-report', 'do it')",
        [],
    )
    .expect("the first template with this name must be accepted");

    let err = tx
        .execute(
            "INSERT INTO task_templates (id, name, prompt) VALUES ('tmpl-2', 'nightly-report', 'do it')",
            [],
        )
        .expect_err("a second template with the same name must be rejected");
    assert_constraint_violation(err);
}

/// §12.3's hook: version "starts at 1 and increases when the prompt or the
/// criteria change." Mutation caught: dropping `CHECK (version >= 1)`.
/// Verified empirically in the scratchpad: `version = 0` is rejected with
/// the CHECK in place, accepted without it.
#[test]
fn task_template_version_below_one_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");

    let err = tx
        .execute(
            "INSERT INTO task_templates (id, name, prompt, version) VALUES ('tmpl-1', 'x', 'do it', 0)",
            [],
        )
        .expect_err("version 0 must be rejected");
    assert_constraint_violation(err);
}

// dispatcher_state holds at most one row -----------------------------------

/// The ordinary case: one row, recording the dispatcher's last tick.
#[test]
fn dispatcher_state_accepts_its_one_row() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");

    tx.execute(
        "INSERT INTO dispatcher_state (id, last_tick_at) VALUES (1, '2026-09-10T09:00:00Z')",
        [],
    )
    .expect("the one legitimate dispatcher_state row must be accepted");
    tx.commit().expect("commit");
}

/// `id` is an `INTEGER PRIMARY KEY`, so it is a rowid alias: a second insert
/// that omits `id` gets `id = 2` from SQLite itself, without this test
/// naming it -- the same path a real dispatcher write would take. Mutation
/// caught: dropping `CHECK (id = 1)`. Verified empirically in the
/// scratchpad: without the CHECK, the second row (`id = 2`) is accepted;
/// with it, `CHECK constraint failed: id = 1`.
#[test]
fn dispatcher_state_rejects_a_second_row() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");

    tx.execute(
        "INSERT INTO dispatcher_state (last_tick_at) VALUES ('2026-09-10T09:00:00Z')",
        [],
    )
    .expect("the first row, taking rowid 1, must be accepted");

    let err = tx
        .execute(
            "INSERT INTO dispatcher_state (last_tick_at) VALUES ('2026-09-10T09:05:00Z')",
            [],
        )
        .expect_err("a second dispatcher_state row must be rejected");
    assert_constraint_violation(err);
}

// task_decisions.rationale is NOT NULL -------------------------------------

/// Design §11 asks for "nachvollziehbare Entscheidungen" -- traceable
/// decisions. A decision with no rationale is a log line, not a decision.
/// Mutation caught: dropping `rationale`'s `NOT NULL`. Verified empirically
/// in the scratchpad: with `NOT NULL`, the insert fails with `NOT NULL
/// constraint failed: task_decisions.rationale`; without it, `rationale`
/// reads back NULL.
#[test]
fn task_decision_without_a_rationale_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");

    common::insert_scope(&tx, "scope-1", "irrlicht", "/instance").expect("insert scope");
    common::insert_task(&tx, "task-1", "scope-1", "do it", "queued").expect("insert task");

    let err = tx
        .execute(
            "INSERT INTO task_decisions (id, task_id, decision, rationale)
             VALUES ('decision-1', 'task-1', 'retry with a smaller prompt', NULL)",
            [],
        )
        .expect_err("a decision with no rationale must be rejected");
    assert_constraint_violation(err);
}

// The CHECK that makes the partial index bite ------------------------------

/// The evasion a test agent found in migration 6 before this CHECK existed,
/// and the reason migration 3 pairs `tasks_one_running_per_session` with a
/// CHECK of its own.
///
/// A partial unique index indexes only the rows its `WHERE` admits. So a cron
/// row written with a NULL `fired_for_minute` is invisible to
/// `tasks_one_run_per_schedule_minute`: two of them for one schedule both
/// insert, and backlog §11's "no more than one run for the same local minute"
/// stops being a property of the database and becomes a property of the
/// dispatcher remembering to fill a column. Measured before the CHECK was
/// added: both rows inserted.
///
/// Mutation caught: delete the `triggered_by` CHECK, or weaken it back to
/// `triggered_by IN ('manual','cron')`.
#[test]
fn a_cron_row_without_a_fired_minute_is_refused_so_the_index_cannot_be_evaded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");

    common::insert_scope(&tx, "scope-1", "irrlicht", "/instance").expect("insert scope");
    insert_template_and_schedule(&tx, "tmpl-1", "sched-1");

    let err = tx
        .execute(
            "INSERT INTO tasks (id, target_scope_id, prompt, status, schedule_id, triggered_by)
             VALUES ('task-1', 'scope-1', 'do it', 'queued', 'sched-1', 'cron')",
            [],
        )
        .expect_err("a cron run with no fired minute must be refused at the source");
    assert_constraint_violation(err);
}

/// The same CHECK read the other way. A `manual` task may not carry a schedule
/// or a minute, so those columns cannot keep a stale schedule id on a row that
/// is no longer a cron run. Without this direction, a cron row could be
/// rewritten to `manual` and keep a `(schedule_id, fired_for_minute)` pair that
/// then occupies the index slot for a minute no schedule fired.
#[test]
fn a_manual_row_carrying_a_schedule_id_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");

    common::insert_scope(&tx, "scope-1", "irrlicht", "/instance").expect("insert scope");
    insert_template_and_schedule(&tx, "tmpl-1", "sched-1");

    let err = tx
        .execute(
            "INSERT INTO tasks (id, target_scope_id, prompt, status, schedule_id, fired_for_minute, triggered_by)
             VALUES ('task-1', 'scope-1', 'do it', 'queued', 'sched-1', '2026-09-10T09:00', 'manual')",
            [],
        )
        .expect_err("a manual task must not carry a schedule or a fired minute");
    assert_constraint_violation(err);

    // And the reverse rewrite is refused too: an existing cron row cannot be
    // demoted to `manual` while keeping its index slot.
    tx.execute(
        "INSERT INTO tasks (id, target_scope_id, prompt, status, schedule_id, fired_for_minute, triggered_by)
         VALUES ('task-2', 'scope-1', 'do it', 'queued', 'sched-1', '2026-09-10T09:00', 'cron')",
        [],
    )
    .expect("a proper cron row inserts");

    let err = tx
        .execute(
            "UPDATE tasks SET triggered_by = 'manual' WHERE id = 'task-2'",
            [],
        )
        .expect_err("demoting a cron row to manual must be refused while it keeps its pair");
    assert_constraint_violation(err);
}
