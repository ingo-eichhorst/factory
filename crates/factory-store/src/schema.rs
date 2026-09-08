//! The version-1 schema, as raw SQL.
//!
//! Design primitives: `.specs/design.md` §2.1 (scope), §2.3 (session), and
//! §2.4 (task). ADR 0012 settles migration tooling (decision 2), locking
//! (decision 3), and which session states hold a workspace lease
//! (decision 5).
//!
//! Migrations are forward-only and append-only (ADR 0012 decision 2): once
//! released, this file is never edited. A schema change ships as a new
//! migration appended after it in [`crate::migrations`], never as an edit
//! here. The two halves of a future schema also follow different rules per
//! ADR 0012: a migration touching an event store may only add, never rewrite
//! history, while a migration touching a projection may drop and rebuild it.
//! Version 1 has no event store yet, so every table below is a plain
//! read/write table, not a projection.

/// The complete version-1 schema, applied as a single migration so that a
/// fresh database's `PRAGMA user_version` is `1`.
pub(crate) const V1_SCHEMA: &str = r#"
-- Scopes ---------------------------------------------------------------
--
-- A registered directory with a stable ID (design §2.1). `canonical_path` is
-- the *stored* identity per ADR 0009 §3a: it is what this UNIQUE constraint
-- indexes, and it is re-canonicalized by the caller before every comparison
-- rather than string-matched as read from the database. Runtime aliasing
-- checks compare (st_dev, st_ino) instead of this string and are Slice 6's
-- concern; this crate stores whatever canonical string it is given and does
-- not canonicalize paths itself.
CREATE TABLE scopes (
    id             TEXT PRIMARY KEY,
    name           TEXT NOT NULL,
    canonical_path TEXT NOT NULL UNIQUE,
    created_at     TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Sessions ---------------------------------------------------------------
--
-- One live instance of an agent: one harness process in one workspace
-- (design §2.3). `state` is constrained to the design's exact vocabulary.
CREATE TABLE sessions (
    id             TEXT PRIMARY KEY,
    scope_id       TEXT NOT NULL REFERENCES scopes (id),
    agent_name     TEXT NOT NULL,
    workspace_path TEXT NOT NULL,
    state          TEXT NOT NULL CHECK (
                       state IN ('stopped', 'starting', 'running', 'disconnected', 'failed')
                   ),
    created_at     TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at     TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX sessions_scope_id ON sessions (scope_id);

-- The lease constraint: only one *live* session may hold a canonical
-- workspace path at a time (design §2.3). Per ADR 0012 decision 5, the
-- lease-holding states are `starting`, `running`, and `disconnected`;
-- `stopped` and `failed` release the workspace. This partial unique index
-- makes SQLite enforce that rule directly on every insert and update,
-- rather than trusting application code to remember which states hold a
-- lease every time it writes a row. This is the constraint that stops two
-- harnesses writing in one directory.
CREATE UNIQUE INDEX sessions_one_live_lease_per_workspace
    ON sessions (workspace_path)
    WHERE state IN ('starting', 'running', 'disconnected');

-- Workspace leases -------------------------------------------------------
--
-- A journal of lease acquisition and release, kept for audit and for
-- Slice 9's mandatory stale-lease recovery action: because `disconnected`
-- holds its lease (ADR 0012 decision 5), an operator clearing a lease whose
-- session cannot be recovered needs a durable record of what was held and
-- when. This table is NOT the authority for "is this workspace leased right
-- now" — that question is answered by `sessions_one_live_lease_per_workspace`
-- above, against the live `sessions` row. A second uniqueness rule here
-- (e.g. an "active" flag with its own unique index) would create two sources
-- of truth for the same fact that could drift out of sync; this table
-- records history instead and is written in the same transaction as the
-- `sessions` state change that it journals.
CREATE TABLE workspace_leases (
    id                       INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id               TEXT NOT NULL REFERENCES sessions (id),
    canonical_workspace_path TEXT NOT NULL,
    acquired_at              TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    -- NULL while the lease named by `session_id`'s current state is held.
    released_at              TEXT,
    release_reason           TEXT
);

CREATE INDEX workspace_leases_session_id ON workspace_leases (session_id);

-- Tasks --------------------------------------------------------------------
--
-- The only durable unit of work exchanged between humans and agents, or
-- between agents (design §2.4). `status` is constrained to the design's
-- exact vocabulary.
CREATE TABLE tasks (
    id                    TEXT PRIMARY KEY,
    -- NULL means the sender is a human, not another scope.
    sender_scope_id       TEXT REFERENCES scopes (id),
    target_scope_id       TEXT NOT NULL REFERENCES scopes (id),
    -- A task may request a specific session or workspace instead of letting
    -- Factory choose any idle session of the target scope's agent.
    target_session_id     TEXT REFERENCES sessions (id),
    target_workspace_path TEXT,
    prompt                TEXT NOT NULL,
    status                TEXT NOT NULL CHECK (
                              status IN ('queued', 'running', 'blocked', 'done', 'failed', 'cancelled')
                          ),
    -- A `blocked` task always carries a reason from the design's exact
    -- vocabulary (design §2.4); every other status must carry none, so a
    -- stale reason cannot survive a later status change. Both halves of the
    -- CHECK matter: a one-sided version would silently allow a reason to
    -- linger on a `done` task. The explicit `IS NOT NULL` in the first half
    -- is load-bearing, not redundant: SQLite's `IN` against a NULL operand
    -- evaluates to NULL rather than FALSE, and a CHECK only rejects a row
    -- when it evaluates to FALSE — so without it, `('blocked', NULL)` would
    -- pass silently (`TRUE AND NULL` is NULL, which CHECK treats as pass).
    blocked_reason        TEXT CHECK (
                              (status = 'blocked' AND blocked_reason IS NOT NULL AND blocked_reason IN
                                  ('clarification', 'permission', 'interrupted', 'external'))
                              OR (status <> 'blocked' AND blocked_reason IS NULL)
                          ),
    result_summary        TEXT,
    -- A JSON array of artifact paths. Kept as TEXT because this crate does
    -- not own artifact-path semantics (Slice 7 does); a JSON array is
    -- sufficient storage without a child table in version 1.
    result_artifact_paths TEXT,
    created_at            TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at            TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX tasks_target_scope_id ON tasks (target_scope_id);
CREATE INDEX tasks_status ON tasks (status);

-- Delegation chain -----------------------------------------------------
--
-- "Every scope the task has passed through" (design §2.4), in order. Kept as
-- its own table, one row per hop, rather than a serialized list on `tasks`,
-- specifically so the UNIQUE constraint below lets the database enforce
-- design §6 directly: "a scope already in that chain cannot be targeted
-- again."
CREATE TABLE task_delegation_chain (
    task_id  TEXT NOT NULL REFERENCES tasks (id),
    position INTEGER NOT NULL,
    scope_id TEXT NOT NULL REFERENCES scopes (id),
    PRIMARY KEY (task_id, position),
    UNIQUE (task_id, scope_id)
);

-- Delivery attempts ------------------------------------------------------
--
-- Journals each delivery attempt before a prompt is written to a session's
-- terminal (design §5 / Slice 7: "record the delivery attempt before
-- writing to the PTY"). `outcome` is left unconstrained because this crate
-- does not own the delivery-outcome vocabulary (Slice 7 does); it is
-- nullable so a row can be written before the attempt's outcome is known,
-- which is the point of journalling before the write rather than after.
CREATE TABLE delivery_attempts (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id      TEXT NOT NULL REFERENCES tasks (id),
    session_id   TEXT NOT NULL REFERENCES sessions (id),
    attempted_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    outcome      TEXT
);

CREATE INDEX delivery_attempts_task_id ON delivery_attempts (task_id);
"#;

/// Migration 2 (ADR 0016 / Slice 3): `scopes` gains the registry projection's
/// columns.
///
/// SQLite's `ALTER TABLE` cannot add a `REFERENCES` clause, and cannot drop or
/// loosen a `NOT NULL`/`UNIQUE` constraint, so widening `scopes` — and making
/// `canonical_path` nullable, since a declared path that does not currently
/// exist is a reportable state (ADR 0016), not an unrepresentable one — takes
/// the create-new-table / copy / drop / rename dance from the SQLite manual's
/// "Making Other Kinds Of Table Schema Changes". Three points matter here:
///
/// - `declared_path` backfills from the only value schema 1 ever had for a
///   path — `canonical_path` — since that is the closest available fact about
///   what schema 1 rows were declared as. `dev`, `ino`, and `parent_id` start
///   `NULL`: schema 1 never recorded identity or parentage, and inventing
///   values here would be a guess dressed as data.
/// - `parent_id REFERENCES scopes (id)` names the table's own *final* name,
///   not `scopes_v2`. SQLite does not resolve a foreign key target at
///   `CREATE TABLE` time, and the closing `ALTER TABLE ... RENAME TO scopes`
///   below makes that name correct by the time anything checks it.
/// - Foreign keys against `scopes` — from `sessions`, `tasks`, and
///   `task_delegation_chain` — are untouched by this migration and need no
///   special handling: `DROP TABLE scopes` followed by renaming the new table
///   back to `scopes` leaves every other table's `REFERENCES scopes (id)`
///   resolving correctly again, because SQLite resolves that clause by name
///   at reference time, not by binding to a table identity that a rename or
///   recreation could invalidate.
///
/// This migration is declared with `.foreign_key_check()` (see
/// `migrations()`), so `PRAGMA foreign_key_check` runs automatically before
/// the migration's transaction commits — and `PRAGMA foreign_keys` itself
/// must be OFF for the connection while this runs, since SQLite documents
/// that toggling it inside a transaction (which every migration is) is a
/// no-op; `migrations::apply` handles that around the whole batch, not here.
pub(crate) const V2_SCHEMA: &str = r#"
CREATE TABLE scopes_v2 (
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

INSERT INTO scopes_v2 (id, name, declared_path, canonical_path, created_at)
SELECT id, name, canonical_path, canonical_path, created_at FROM scopes;

DROP TABLE scopes;

ALTER TABLE scopes_v2 RENAME TO scopes;
"#;
