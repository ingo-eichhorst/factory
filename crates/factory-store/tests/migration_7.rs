//! Migration 7 (backlog §12 / ADR 0022): `durable_writes`, the provenance log
//! for `factory knowledge write` and `factory memory add`.
//!
//! Every test here is written against one line of `V7_SCHEMA`, or one line of
//! `src/durable.rs`, that it is meant to catch. Where a mutation was actually
//! run against this crate's own source (reverted afterward) and the tests it
//! killed recorded, the test's doc comment says so. `migration_6.rs` is the
//! model this file follows.

mod common;

use rusqlite::Connection;

use factory_store::durable::{self, WriteKind};
use factory_store::{Store, StoreError, latest_schema_version};

/// Every CHECK, UNIQUE index, NOT NULL, and FOREIGN KEY violation this file
/// exercises surfaces through `rusqlite` as the same primary error code.
/// `migration_6.rs` and the other files under `tests/` each define their own
/// copy of this rather than sharing one, since every file under `tests/` is
/// compiled as its own crate; this file follows the same convention.
fn assert_constraint_violation(err: &rusqlite::Error) {
    assert!(
        matches!(
            err,
            rusqlite::Error::SqliteFailure(inner, _)
                if inner.code == rusqlite::ErrorCode::ConstraintViolation
        ),
        "expected a CHECK/UNIQUE/NOT NULL/FOREIGN KEY constraint violation, got {err:?}"
    );
}

/// The primary code alone (`assert_constraint_violation`) cannot tell a CHECK
/// apart from a FOREIGN KEY or a NOT NULL violation — every one of them is
/// `ConstraintViolation`. This asserts SQLite's own *extended* result code,
/// which does distinguish them, so a test proving "the foreign key on
/// `task_id` is what rejected this row" cannot pass because some unrelated
/// constraint fired first (the exact failure mode this project's acceptance
/// standard names).
fn assert_extended_code(err: &rusqlite::Error, expected: i32, what: &str) {
    match err {
        rusqlite::Error::SqliteFailure(inner, _) => {
            assert_eq!(
                inner.extended_code, expected,
                "expected {what} (extended code {expected}), got extended code \
                 {} ({err:?})",
                inner.extended_code
            );
        }
        other => panic!("expected a SqliteFailure carrying {what}, got {other:?}"),
    }
}

fn assert_foreign_key_violation(err: &rusqlite::Error) {
    assert_extended_code(
        err,
        rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY,
        "a FOREIGN KEY violation",
    );
}

/// A CHECK violation, further narrowed to the one whose failing expression
/// contains `needle`. SQLite reports an *unnamed* CHECK constraint's failure
/// as `CHECK constraint failed: <the expression text>` (verified in this
/// file's own test run, and already relied on by `migration_6.rs`'s doc
/// comments, e.g. "`CHECK constraint failed: triggered_by IN
/// ('manual','cron')`"), so matching a substring of that text is how two
/// different anonymous CHECK constraints on the same table are told apart.
fn assert_check_violation_naming(err: &rusqlite::Error, needle: &str) {
    assert_extended_code(
        err,
        rusqlite::ffi::SQLITE_CONSTRAINT_CHECK,
        "a CHECK violation",
    );
    let message = err.to_string();
    assert!(
        message.contains(needle),
        "expected the CHECK failure message to mention {needle:?}, got {message:?}"
    );
}

fn store_error_to_sqlite(err: StoreError) -> rusqlite::Error {
    match err {
        StoreError::Sqlite(inner) => inner,
        other => panic!("expected StoreError::Sqlite, got {other:?}"),
    }
}

/// Insert one scope and one task referencing it — `common::insert_task`
/// alone is not enough, since `tasks.target_scope_id` is `NOT NULL
/// REFERENCES scopes (id)`, and most tests below need a real task to hang a
/// `durable_writes.task_id` off of.
fn insert_scope_and_task(tx: &Connection, scope_id: &str, task_id: &str) {
    common::insert_scope(tx, scope_id, "irrlicht", "/instance").expect("insert scope");
    common::insert_task(tx, task_id, scope_id, "do it", "queued").expect("insert task");
}

// A pre-existing schema-6 database survives migration 7 --------------------

/// Builds a schema-6 database directly from raw SQL, bypassing `Store` (and
/// therefore migration 7) entirely, then pins it at `user_version = 6`.
///
/// `schema::V1_SCHEMA` through `V6_SCHEMA` are `pub(crate)`, unreachable from
/// an integration test — the same constraint `migration_6.rs`'s own
/// `seed_schema_5_database` names for itself. What follows is
/// `migration_6.rs`'s `seed_schema_5_database` (the schema-5 shape) with
/// migration 6's own tables and `ALTER TABLE` statements layered on top,
/// reproduced from `schema::V6_SCHEMA` exactly — column for column, statement
/// for statement, in the same order — so this really is the shape a real
/// upgrade through migration 6 leaves behind, not an approximation of it.
fn seed_schema_6_database(path: &std::path::Path, extra_sql: &str) {
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

-- Migration 6 (ADR 0021 / backlog §11), reproduced from `schema::V6_SCHEMA`.
CREATE TABLE task_templates (
    id                  TEXT PRIMARY KEY,
    name                TEXT NOT NULL,
    target_scope_id     TEXT NOT NULL REFERENCES scopes (id),
    target_agent_name   TEXT,
    prompt              TEXT NOT NULL,
    acceptance_criteria TEXT,
    version             INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    state               TEXT NOT NULL DEFAULT 'open' CHECK (
                            state IN ('open', 'paused', 'closed')
                        ),
    created_at          TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at          TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE UNIQUE INDEX task_templates_name ON task_templates (name);

CREATE TABLE schedules (
    id            TEXT PRIMARY KEY,
    template_id   TEXT NOT NULL REFERENCES task_templates (id),
    cron          TEXT NOT NULL,
    timezone      TEXT NOT NULL,
    enabled       INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    last_fired_at TEXT,
    created_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX schedules_template_id ON schedules (template_id);

CREATE TABLE task_events (
    id                INTEGER PRIMARY KEY,
    task_id           TEXT NOT NULL REFERENCES tasks (id),
    event_type        TEXT NOT NULL CHECK (
                          event_type IN (
                              'created', 'assigned', 'delivered', 'refused',
                              'running', 'progress', 'blocked', 'done',
                              'failed', 'cancelled', 'resumed', 'verification',
                              'rework'
                          )
                      ),
    author_session_id TEXT REFERENCES sessions (id),
    payload           TEXT,
    created_at        TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX task_events_task_id ON task_events (task_id, id);

CREATE TABLE task_decisions (
    id                TEXT PRIMARY KEY,
    task_id           TEXT NOT NULL REFERENCES tasks (id),
    author_session_id TEXT REFERENCES sessions (id),
    decision          TEXT NOT NULL,
    rationale         TEXT NOT NULL,
    alternatives      TEXT,
    consequences      TEXT,
    created_at        TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX task_decisions_task_id ON task_decisions (task_id);

CREATE TABLE dispatcher_state (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    last_tick_at TEXT NOT NULL
);

ALTER TABLE tasks ADD COLUMN template_id TEXT REFERENCES task_templates (id);
ALTER TABLE tasks ADD COLUMN template_version INTEGER;
ALTER TABLE tasks ADD COLUMN schedule_id TEXT REFERENCES schedules (id);
ALTER TABLE tasks ADD COLUMN fired_for_minute TEXT;
ALTER TABLE tasks ADD COLUMN triggered_by TEXT NOT NULL DEFAULT 'manual' CHECK (
    (triggered_by = 'manual' AND schedule_id IS NULL AND fired_for_minute IS NULL)
    OR (triggered_by = 'cron' AND schedule_id IS NOT NULL AND fired_for_minute IS NOT NULL)
);
ALTER TABLE tasks ADD COLUMN reworks_task_id TEXT REFERENCES tasks (id);
ALTER TABLE tasks ADD COLUMN rework_finding TEXT;
ALTER TABLE tasks ADD COLUMN cost_model TEXT;
ALTER TABLE tasks ADD COLUMN cost_input_tokens INTEGER;
ALTER TABLE tasks ADD COLUMN cost_output_tokens INTEGER;
ALTER TABLE tasks ADD COLUMN cost_duration_ms INTEGER;
ALTER TABLE tasks ADD COLUMN cost_baseline TEXT;
ALTER TABLE tasks ADD COLUMN context_utilization_percent REAL;

CREATE INDEX tasks_template_id ON tasks (template_id);

CREATE UNIQUE INDEX tasks_one_run_per_schedule_minute
    ON tasks (schedule_id, fired_for_minute)
    WHERE schedule_id IS NOT NULL AND fired_for_minute IS NOT NULL;
"#,
    )
    .expect("apply the schema-6 shape");
    if !extra_sql.is_empty() {
        conn.execute_batch(extra_sql).expect("seed extra rows");
    }
    conn.pragma_update(None, "user_version", 6_i64)
        .expect("pin schema-6 database at user_version 6");
}

/// What to prove, item 1: a real database that predates migration 7 keeps
/// every row it had, across every table migration 7 could plausibly disturb
/// if it were the drop-and-rebuild kind rather than the additive kind — a
/// scope, a session, a task, and a task event. This is the test that would
/// catch a migration 7 that rebuilt a table it should not touch: a rebuild
/// changes `sqlite_master.sql` for that table and risks losing rows in the
/// copy, neither of which a purely additive `CREATE TABLE` can do.
#[test]
fn a_schema_6_database_survives_migration_7_with_rows_intact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("with_data.sqlite");

    let seed = "\
        INSERT INTO scopes (id, name, declared_path, canonical_path) \
            VALUES ('scope-1', 'irrlicht', '/instance', '/instance');\n\
        INSERT INTO sessions (id, scope_id, agent_name, workspace_path, state) \
            VALUES ('session-1', 'scope-1', 'agent', '/instance', 'running');\n\
        INSERT INTO tasks (id, target_scope_id, prompt, status) \
            VALUES ('task-1', 'scope-1', 'do it', 'queued');\n\
        INSERT INTO task_events (task_id, event_type, author_session_id, payload) \
            VALUES ('task-1', 'created', 'session-1', '{\"note\":\"seeded\"}');";
    seed_schema_6_database(&path, seed);

    let store = Store::open_at(&path).expect("migration 7 must succeed against real rows");
    assert_eq!(store.schema_version().unwrap(), latest_schema_version());

    let scope_name: String = store
        .connection()
        .query_row("SELECT name FROM scopes WHERE id = 'scope-1'", [], |row| {
            row.get(0)
        })
        .expect("scope row survived migration 7");
    assert_eq!(scope_name, "irrlicht");

    let session_state: String = store
        .connection()
        .query_row(
            "SELECT state FROM sessions WHERE id = 'session-1'",
            [],
            |row| row.get(0),
        )
        .expect("session row survived migration 7");
    assert_eq!(session_state, "running");

    let task_status: String = store
        .connection()
        .query_row("SELECT status FROM tasks WHERE id = 'task-1'", [], |row| {
            row.get(0)
        })
        .expect("task row survived migration 7");
    assert_eq!(task_status, "queued");

    let (event_type, event_payload): (String, Option<String>) = store
        .connection()
        .query_row(
            "SELECT event_type, payload FROM task_events WHERE task_id = 'task-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("task_events row survived migration 7 -- proof it was not rebuilt");
    assert_eq!(event_type, "created");
    assert_eq!(event_payload.as_deref(), Some("{\"note\":\"seeded\"}"));

    // A purely additive migration cannot introduce a foreign key violation:
    // nothing it does could invalidate a `REFERENCES` clause that already
    // resolved before it ran.
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

    // `durable_writes` itself exists and starts empty -- the additive
    // promise `V7_SCHEMA`'s doc comment makes.
    let durable_write_count: i64 = store
        .connection()
        .query_row("SELECT COUNT(*) FROM durable_writes", [], |row| row.get(0))
        .expect("durable_writes table exists after migration 7");
    assert_eq!(durable_write_count, 0);
}

// What to prove, item 2: a fresh database ------------------------------------

/// A brand-new database reaches schema 7 (whatever `latest_schema_version()`
/// currently is) directly, in one `Store::open_at` call, with `durable_writes`
/// already present and empty.
#[test]
fn a_fresh_database_arrives_at_the_latest_schema_directly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open_at(dir.path().join("fresh.sqlite")).expect("open fresh store");
    assert_eq!(store.schema_version().unwrap(), latest_schema_version());

    let durable_write_count: i64 = store
        .connection()
        .query_row("SELECT COUNT(*) FROM durable_writes", [], |row| row.get(0))
        .expect("durable_writes table exists on a fresh database");
    assert_eq!(durable_write_count, 0);
}

/// No behavioural test above can catch a forgotten or misdirected index:
/// every query in this file still returns the right *rows* from a table
/// scan, just slower, so deleting `CREATE INDEX durable_writes_task_id` or
/// `durable_writes_scope_id` would not fail a single test that only checks
/// results. `migrations.rs`'s own
/// `migration_3_preserves_referencing_rows_and_stays_foreign_key_clean`
/// takes the same stance for `tasks`'s indexes, for the same reason: this
/// reads `sqlite_master` directly instead. `durable_writes.id` is an
/// `INTEGER PRIMARY KEY` (a rowid alias), so — unlike `tasks`'s `TEXT
/// PRIMARY KEY` — SQLite creates no autoindex for it, and this list is
/// exactly the two explicit indexes `V7_SCHEMA` creates.
///
/// Mutation caught: deleting either `CREATE INDEX` statement. Verified: with
/// `durable_writes_scope_id` deleted, this test's list drops to one name and
/// fails.
#[test]
fn durable_writes_has_exactly_its_two_named_indexes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open_at(dir.path().join("fresh.sqlite")).expect("open fresh store");

    let mut stmt = store
        .connection()
        .prepare(
            "SELECT name FROM sqlite_master \
             WHERE type = 'index' AND tbl_name = 'durable_writes' ORDER BY name",
        )
        .expect("prepare index listing");
    let index_names = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .expect("list durable_writes indexes")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect durable_writes indexes");
    assert_eq!(
        index_names,
        vec!["durable_writes_scope_id", "durable_writes_task_id"],
        "durable_writes must carry exactly the two indexes V7_SCHEMA creates, \
         no more and no fewer"
    );
}

// What to prove, item 3: migrating twice is a no-op --------------------------

/// Reopening an already-migrated database must not re-run migration 7 (or any
/// other migration) a second time -- `rusqlite_migration` only applies what
/// `PRAGMA user_version` says is pending, and a schema-7 database has none.
/// Mutation caught: anything that made `to_latest` re-run a migration already
/// reflected in `user_version` -- an appended duplicate `V7_SCHEMA` entry, or
/// a bug in the pending-migration count -- fails here (or, for a literal
/// duplicate list entry, at the *first* `open_at` already) because
/// `CREATE TABLE durable_writes` cannot run twice against the same database.
#[test]
fn migrating_twice_is_a_noop() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("factory.sqlite");

    let first = Store::open_at(&path).expect("first open migrates to latest");
    assert_eq!(first.schema_version().unwrap(), latest_schema_version());
    drop(first);

    let second = Store::open_at(&path).expect("second open: nothing pending");
    assert_eq!(second.schema_version().unwrap(), latest_schema_version());
}

// durable_writes.kind CHECK ---------------------------------------------

/// What to prove, item 4: the CHECK refuses a third `kind` string. Mutation
/// caught: deleting `CHECK (kind IN ('knowledge', 'memory'))`. This insert is
/// deliberately given a NULL `scope_id`, which the pairing CHECK on `scope_id`
/// (see `V7_SCHEMA`'s doc comment) also rejects for `kind = 'memory'` or
/// `kind = 'knowledge'` -- but not for a third string, since that CHECK is
/// written as two implications that only ever fire when `kind` is exactly one
/// of the two valid values. So this row is refused by the vocabulary CHECK
/// alone, and `assert_check_violation_naming` further confirms it is *that*
/// CHECK's own text in the failure message, not the pairing CHECK's.
#[test]
fn kind_outside_the_vocabulary_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");

    let err = tx
        .execute(
            "INSERT INTO durable_writes (kind, name, path) VALUES ('wiki', 'note-1', 'knowledge/note-1.md')",
            [],
        )
        .expect_err("a kind outside knowledge|memory must be rejected");
    assert_constraint_violation(&err);
    assert_check_violation_naming(&err, "kind IN");
}

/// Both members of the two-string vocabulary are accepted -- paired with a
/// `scope_id` that satisfies the pairing CHECK for that `kind`, so this test
/// is only exercising the vocabulary, not the pairing rule (which has its own
/// tests below).
#[test]
fn both_kinds_in_the_vocabulary_are_accepted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");
    common::insert_scope(&tx, "scope-1", "irrlicht", "/instance").expect("insert scope");

    tx.execute(
        "INSERT INTO durable_writes (kind, name, path) VALUES ('knowledge', 'note-1', 'knowledge/note-1.md')",
        [],
    )
    .expect("a knowledge write with no scope must be accepted");
    tx.execute(
        "INSERT INTO durable_writes (kind, name, path, scope_id) \
         VALUES ('memory', 'entry-1', 'memory/scope-1/entry-1.md', 'scope-1')",
        [],
    )
    .expect("a memory write naming its scope must be accepted");
    tx.commit().expect("commit");
}

// durable_writes.scope_id pairing CHECK -----------------------------------

/// A `memory` write with no scope is refused -- `memory add` always knows its
/// calling scope, so a row missing one is a bug, not a legitimate write.
/// Mutation caught: deleting the `kind <> 'memory' OR scope_id IS NOT NULL`
/// half of the pairing CHECK. Distinguished from the vocabulary CHECK the
/// same way `kind_outside_the_vocabulary_is_rejected` distinguishes it in the
/// other direction: `kind = 'memory'` here, so the vocabulary CHECK alone
/// would accept this row, and only the pairing CHECK rejects it.
#[test]
fn a_memory_write_with_no_scope_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");

    let err = tx
        .execute(
            "INSERT INTO durable_writes (kind, name, path) VALUES ('memory', 'entry-1', 'memory/x/entry-1.md')",
            [],
        )
        .expect_err("a memory write naming no scope must be rejected");
    assert_constraint_violation(&err);
    assert_check_violation_naming(&err, "scope_id IS NOT NULL");
}

/// The other half: a `knowledge` write naming a scope is refused -- knowledge
/// lives in the shared company root, never under a scope directory. Mutation
/// caught: deleting the `kind <> 'knowledge' OR scope_id IS NULL` half.
#[test]
fn a_knowledge_write_naming_a_scope_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");
    common::insert_scope(&tx, "scope-1", "irrlicht", "/instance").expect("insert scope");

    let err = tx
        .execute(
            "INSERT INTO durable_writes (kind, name, path, scope_id) \
             VALUES ('knowledge', 'note-1', 'knowledge/note-1.md', 'scope-1')",
            [],
        )
        .expect_err("a knowledge write naming a scope must be rejected");
    assert_constraint_violation(&err);
    assert_check_violation_naming(&err, "scope_id IS NULL");
}

// durable_writes.task_id is nullable, and its foreign key holds -----------

/// What to prove, item 5, first half: `task_id` really accepts NULL -- a
/// human at a terminal, running no task, must be able to write a note.
/// Mutation caught: making `task_id` `NOT NULL`. Reads the column back rather
/// than only checking the insert succeeded, since an insert that merely
/// defaults some other way would not prove NULL specifically made it to
/// disk.
#[test]
fn task_id_accepts_null() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");

    tx.execute(
        "INSERT INTO durable_writes (kind, name, path) VALUES ('knowledge', 'note-1', 'knowledge/note-1.md')",
        [],
    )
    .expect("a durable write naming no task must be accepted");

    let task_id: Option<String> = tx
        .query_row(
            "SELECT task_id FROM durable_writes WHERE name = 'note-1'",
            [],
            |row| row.get(0),
        )
        .expect("read task_id back");
    assert_eq!(
        task_id, None,
        "a human writing a note names no task, and NULL must be what is stored"
    );
}

/// What to prove, item 5, second half: a non-NULL `task_id` naming no real
/// task is refused by the foreign key. Mutation caught: dropping
/// `REFERENCES tasks (id)` from `task_id`. `assert_foreign_key_violation`
/// pins this to the *foreign key* specifically, not merely "some constraint",
/// so a mutation that replaced the FK with an unrelated CHECK that happened
/// to also reject this one row could not pass silently.
#[test]
fn task_id_naming_a_nonexistent_task_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");

    let err = tx
        .execute(
            "INSERT INTO durable_writes (kind, name, path, task_id) \
             VALUES ('knowledge', 'note-1', 'knowledge/note-1.md', 'no-such-task')",
            [],
        )
        .expect_err("a task_id naming a task that does not exist must be rejected");
    assert_constraint_violation(&err);
    assert_foreign_key_violation(&err);
}

/// A `task_id` naming a real task is accepted, and reads back correctly --
/// the positive half `task_id_naming_a_nonexistent_task_is_rejected` needs to
/// mean anything (a foreign key that rejected every non-NULL value would also
/// pass that test).
#[test]
fn task_id_naming_a_real_task_is_accepted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");
    insert_scope_and_task(&tx, "scope-1", "task-1");

    tx.execute(
        "INSERT INTO durable_writes (kind, name, path, task_id) \
         VALUES ('knowledge', 'note-1', 'knowledge/note-1.md', 'task-1')",
        [],
    )
    .expect("a task_id naming a real task must be accepted");

    let task_id: Option<String> = tx
        .query_row(
            "SELECT task_id FROM durable_writes WHERE name = 'note-1'",
            [],
            |row| row.get(0),
        )
        .expect("read task_id back");
    assert_eq!(task_id.as_deref(), Some("task-1"));
}

// scope_id's own foreign key ------------------------------------------------

/// The `scope_id` twin of `task_id_naming_a_nonexistent_task_is_rejected`.
/// Mutation caught: dropping `REFERENCES scopes (id)` from `scope_id`.
#[test]
fn scope_id_naming_a_nonexistent_scope_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");

    let err = tx
        .execute(
            "INSERT INTO durable_writes (kind, name, path, scope_id) \
             VALUES ('memory', 'entry-1', 'memory/x/entry-1.md', 'no-such-scope')",
            [],
        )
        .expect_err("a scope_id naming a scope that does not exist must be rejected");
    assert_constraint_violation(&err);
    assert_foreign_key_violation(&err);
}

// src/durable.rs's `append` --------------------------------------------

/// The ordinary path: `append` inserts a row that is still there once the
/// caller commits, and every field round-trips.
#[test]
fn append_persists_a_row_once_the_caller_commits() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");
    insert_scope_and_task(&tx, "scope-1", "task-1");
    common::insert_session(&tx, "session-1", "scope-1", "agent", "/instance", "running")
        .expect("insert session");

    durable::append(
        &tx,
        WriteKind::Memory,
        "entry-1",
        "memory/scope-1/entry-1.md",
        Some("scope-1"),
        Some("task-1"),
        Some("session-1"),
    )
    .expect("append must succeed against real rows");
    tx.commit().expect("commit");

    let (kind, name, path, scope_id, task_id, author_session_id): (
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = store
        .connection()
        .query_row(
            "SELECT kind, name, path, scope_id, task_id, author_session_id FROM durable_writes",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .expect("read the appended row back");
    assert_eq!(kind, "memory");
    assert_eq!(name, "entry-1");
    assert_eq!(path, "memory/scope-1/entry-1.md");
    assert_eq!(scope_id.as_deref(), Some("scope-1"));
    assert_eq!(task_id.as_deref(), Some("task-1"));
    assert_eq!(author_session_id.as_deref(), Some("session-1"));
}

/// What to prove, item 6, and the one that proves the signature does what its
/// doc comment claims: `append` writes nothing when the caller's transaction
/// is rolled back. `append` takes `&rusqlite::Transaction` and never opens or
/// commits one of its own, so the row it inserts has no way to become durable
/// except through the caller's own `tx.commit()` -- dropping `tx` here without
/// calling it rolls the whole transaction back, `append`'s insert included.
///
/// Mutation caught: changing `append` to commit on its own (see this crate's
/// task report for the exact edit run and reverted -- inserting the row, then
/// executing `COMMIT; BEGIN IMMEDIATE;` on the raw connection before
/// returning, which durably commits the insert out from under the caller
/// mid-transaction). Against the mutated version this test fails, because the
/// row survives the rollback that follows.
#[test]
fn append_writes_nothing_when_the_transaction_is_rolled_back() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    {
        let tx = store.transaction().expect("begin");
        insert_scope_and_task(&tx, "scope-1", "task-1");
        durable::append(
            &tx,
            WriteKind::Knowledge,
            "note-1",
            "knowledge/note-1.md",
            None,
            Some("task-1"),
            None,
        )
        .expect("append must succeed inside the transaction");
        // `tx` is dropped here without `.commit()` -- `rusqlite::Transaction`'s
        // own `Drop` impl rolls back, taking the scope insert, the task
        // insert, and `append`'s own insert with it.
    }

    let count: i64 = store
        .connection()
        .query_row("SELECT COUNT(*) FROM durable_writes", [], |row| row.get(0))
        .expect("query durable_writes after the rollback");
    assert_eq!(
        count, 0,
        "a row inserted by append must not survive its caller's transaction \
         being rolled back"
    );

    // The scope and task inserted in the same, rolled-back transaction must
    // also be gone -- confirming this is a real rollback of the shared
    // transaction, not a coincidence of `durable_writes` alone being empty.
    let scope_count: i64 = store
        .connection()
        .query_row("SELECT COUNT(*) FROM scopes", [], |row| row.get(0))
        .expect("query scopes after the rollback");
    assert_eq!(scope_count, 0);
}

/// `list_for_task` returns a task's durable writes in append order, and only
/// that task's own rows.
#[test]
fn list_for_task_returns_only_that_tasks_writes_in_append_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    {
        let tx = store.transaction().expect("begin");
        insert_scope_and_task(&tx, "scope-1", "task-1");
        common::insert_task(&tx, "task-2", "scope-1", "do it", "queued").expect("insert task-2");
        durable::append(
            &tx,
            WriteKind::Knowledge,
            "note-1",
            "knowledge/note-1.md",
            None,
            Some("task-1"),
            None,
        )
        .expect("append note-1");
        durable::append(
            &tx,
            WriteKind::Knowledge,
            "note-2",
            "knowledge/note-2.md",
            None,
            Some("task-2"),
            None,
        )
        .expect("append note-2 against a different task");
        durable::append(
            &tx,
            WriteKind::Knowledge,
            "note-3",
            "knowledge/note-3.md",
            None,
            Some("task-1"),
            None,
        )
        .expect("append note-3");
        tx.commit().expect("commit");
    }

    let writes = durable::list_for_task(&store, "task-1").expect("list_for_task");
    let names: Vec<&str> = writes.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["note-1", "note-3"],
        "only task-1's own writes, in append order"
    );
}

/// `list_for_scope` returns a scope's memory entries in append order, and
/// only that scope's own rows.
#[test]
fn list_for_scope_returns_only_that_scopes_writes_in_append_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    {
        let tx = store.transaction().expect("begin");
        common::insert_scope(&tx, "scope-1", "irrlicht", "/instance").expect("insert scope-1");
        common::insert_scope(&tx, "scope-2", "ingo", "/instance2").expect("insert scope-2");
        durable::append(
            &tx,
            WriteKind::Memory,
            "entry-1",
            "memory/scope-1/entry-1.md",
            Some("scope-1"),
            None,
            None,
        )
        .expect("append entry-1");
        durable::append(
            &tx,
            WriteKind::Memory,
            "entry-2",
            "memory/scope-2/entry-2.md",
            Some("scope-2"),
            None,
            None,
        )
        .expect("append entry-2 against a different scope");
        durable::append(
            &tx,
            WriteKind::Memory,
            "entry-3",
            "memory/scope-1/entry-3.md",
            Some("scope-1"),
            None,
            None,
        )
        .expect("append entry-3");
        tx.commit().expect("commit");
    }

    let writes = durable::list_for_scope(&store, "scope-1").expect("list_for_scope");
    let names: Vec<&str> = writes.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["entry-1", "entry-3"],
        "only scope-1's own writes, in append order"
    );
}

/// A silent guard against the mistake `store_error_to_sqlite` exists to
/// unwrap: `append` on a genuinely bad row (a `task_id` naming nothing)
/// returns `StoreError::Sqlite` carrying the same foreign key violation the
/// raw-SQL tests above see, not some other variant.
#[test]
fn append_surfaces_a_foreign_key_violation_as_store_error_sqlite() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tx = store.transaction().expect("begin");

    let err = durable::append(
        &tx,
        WriteKind::Knowledge,
        "note-1",
        "knowledge/note-1.md",
        None,
        Some("no-such-task"),
        None,
    )
    .expect_err("append against a nonexistent task must fail");
    let sqlite_err = store_error_to_sqlite(err);
    assert_constraint_violation(&sqlite_err);
    assert_foreign_key_violation(&sqlite_err);
}
