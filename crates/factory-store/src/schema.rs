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

/// Migration 3 (backlog §7 / design §2.4): `tasks` gains the two columns and
/// two rules that make single-session delivery a database-enforced fact
/// rather than a rule application code merely promises to keep.
///
/// Same create/copy/drop/rename recipe as migration 2, for the same
/// `ALTER TABLE` limitations (no way to add a `REFERENCES` clause to an
/// existing table), and the same three traps migration 2's doc comment above
/// already explains apply again here — read it before touching this one:
///
/// - the two new `REFERENCES sessions (id)` clauses below name the table's
///   *final* name (`tasks`), not `tasks_v3`. SQLite does not resolve a
///   foreign key target at `CREATE TABLE` time, and the closing
///   `ALTER TABLE ... RENAME TO tasks` below makes that name correct by the
///   time anything checks it.
/// - `PRAGMA foreign_keys` must be OFF for the connection while this batch
///   runs, since toggling it inside a transaction — which every migration is
///   — is a documented SQLite no-op; `migrations::apply` handles that around
///   the whole batch, not here.
/// - the tables that reference `tasks` — `task_delegation_chain.task_id` and
///   `delivery_attempts.task_id`, both `REFERENCES tasks (id)` — need no
///   special handling here: SQLite resolves that clause by name at reference
///   time, not by binding to a table identity that a rename or recreation
///   could invalidate.
///
/// This migration is declared with `.foreign_key_check()` (see
/// `migrations()`) for the same reason migration 2 is: rebuilding a table
/// that other tables reference means `PRAGMA foreign_key_check` must confirm
/// nothing was left dangling before the migration's transaction commits.
///
/// # The two new columns
///
/// - `assigned_session_id` is the session *Factory chose*. It is deliberately
///   not the same column as `target_session_id`, which is what the sender
///   *requested* ("A task may instead request a specific session or
///   workspace," design §2.4). This is the same split migration 2 made
///   between `declared_path` and `canonical_path` on `scopes`, and for the
///   same reason: after a restart, Slice 9 must be able to tell "the human
///   asked for this session" from "Factory happened to choose it," because a
///   requested session that died must not be silently re-assigned elsewhere,
///   while an untargeted task may be.
/// - `cancel_requested_at` stays NULL until an operator asks a *running* task
///   to stop. Design §2.4: "Task cancellation is cooperative once a task is
///   running." Recording the request here is not itself a status change —
///   the task stays `running` until the agent reports a terminal status; this
///   column is a request log, not a status.
///
/// # The two new rules are one mechanism, not two
///
/// `tasks_one_running_per_session` (below) enforces backlog §7's "each
/// session has at most one running task" the way
/// `sessions_one_live_lease_per_workspace` enforces the lease rule: a partial
/// unique index over exactly the rows that matter, rather than trusting
/// application code to remember to check before every write. But SQLite
/// treats every NULL in a unique index as distinct from every other NULL, so
/// without the CHECK constraint added to `tasks_v3` below, an unlimited
/// number of `running` tasks with a NULL `assigned_session_id` could coexist
/// and the index would never see a collision between them. The CHECK is what
/// makes the index actually bite.
///
/// # A pre-existing `running` row has no assignment to copy forward
///
/// Schema 1 and 2 never recorded `assigned_session_id` at all, so a database
/// migrated from either one may already hold a `running` task whose session
/// this migration has no way to know. Copying such a row forward unchanged
/// would leave `status = 'running'` with `assigned_session_id` still NULL —
/// exactly the row the new CHECK below exists to reject — so the migration's
/// transaction would abort for any operator who happens to upgrade with a
/// task mid-flight, which is worse than the ambiguity it is trying to guard
/// against. Design §5 already names this exact situation and its remedy:
/// "Factory does not automatically resend a possibly delivered prompt... the
/// task becomes `blocked: interrupted` for human review." A `running` row
/// surviving from before this migration existed is precisely "Factory can no
/// longer answer which session this belongs to," so the `INSERT ... SELECT`
/// below rewrites `status = 'running'` to `blocked` with
/// `blocked_reason = 'interrupted'` during the copy — the same outcome
/// `factory_session::interrupt` (a different crate this one does not depend
/// on) produces for a session confirmed dead mid-task, reached here directly
/// in SQL rather than by calling it. A `queued`, `blocked`, `done`, `failed`,
/// or `cancelled` row is untouched: none of those statuses claim a session is
/// currently working on them, so none of them are ambiguous.
pub(crate) const V3_SCHEMA: &str = r#"
CREATE TABLE tasks_v3 (
    id                    TEXT PRIMARY KEY,
    -- NULL means the sender is a human, not another scope.
    sender_scope_id       TEXT REFERENCES scopes (id),
    target_scope_id       TEXT NOT NULL REFERENCES scopes (id),
    -- A task may request a specific session or workspace instead of letting
    -- Factory choose any idle session of the target scope's agent.
    target_session_id     TEXT REFERENCES sessions (id),
    target_workspace_path TEXT,
    -- The session Factory *chose*, as opposed to `target_session_id` above,
    -- which is what the sender *requested*. Kept as its own column, never
    -- overwriting or reusing `target_session_id`, so that after a restart a
    -- requested session that died is never silently re-assigned elsewhere,
    -- while an untargeted task remains free to be reassigned to any idle
    -- session. Must be non-NULL whenever `status = 'running'` (see the CHECK
    -- below).
    assigned_session_id   TEXT REFERENCES sessions (id),
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
    -- NULL until an operator asks a *running* task to stop (design §2.4).
    -- Recording the request is not a status change by itself: the task stays
    -- `running` until the agent reports a terminal status.
    cancel_requested_at   TEXT,
    result_summary        TEXT,
    -- A JSON array of artifact paths. Kept as TEXT because this crate does
    -- not own artifact-path semantics (Slice 7 does); a JSON array is
    -- sufficient storage without a child table in version 1.
    result_artifact_paths TEXT,
    created_at            TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at            TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    -- A `running` task must name the session Factory assigned it to. Written
    -- with an explicit `IS NOT NULL`, guarding against the same
    -- three-valued-logic trap `blocked_reason`'s CHECK above documents: a
    -- bare comparison against NULL evaluates to NULL, not FALSE, and CHECK
    -- only rejects a row that evaluates to FALSE. This constraint is what
    -- makes `tasks_one_running_per_session` below actually enforce "at most
    -- one running task per session" — see that index's comment for why the
    -- index alone cannot.
    CHECK (
        (status = 'running' AND assigned_session_id IS NOT NULL) OR (status <> 'running')
    )
);

-- A schema-1/2 `running` row has no recorded `assigned_session_id` at all —
-- the column did not exist yet — so it is rewritten to `blocked: interrupted`
-- during the copy rather than carried forward as `running` with a NULL
-- assignment, which the CHECK above would reject outright. See this
-- migration's doc comment ("A pre-existing `running` row has no assignment
-- to copy forward") for why design §5 makes this the correct outcome, not
-- merely the one that satisfies the constraint.
INSERT INTO tasks_v3 (
    id, sender_scope_id, target_scope_id, target_session_id, target_workspace_path,
    prompt, status, blocked_reason, result_summary, result_artifact_paths,
    created_at, updated_at
)
SELECT
    id, sender_scope_id, target_scope_id, target_session_id, target_workspace_path,
    prompt,
    CASE WHEN status = 'running' THEN 'blocked' ELSE status END,
    CASE WHEN status = 'running' THEN 'interrupted' ELSE blocked_reason END,
    result_summary, result_artifact_paths,
    created_at,
    -- Only a row this migration actually rewrote gets a new `updated_at`.
    -- Copying it verbatim would leave the one column an operator reads to
    -- find what an upgrade touched pointing at the moment the task was last
    -- worked on, hours or days before the rewrite. Rows the migration only
    -- copies keep their timestamp, because nothing about them changed.
    CASE WHEN status = 'running' THEN CURRENT_TIMESTAMP ELSE updated_at END
FROM tasks;

DROP TABLE tasks;

ALTER TABLE tasks_v3 RENAME TO tasks;

CREATE INDEX tasks_target_scope_id ON tasks (target_scope_id);
CREATE INDEX tasks_status ON tasks (status);

-- Backlog §7: "each session has at most one running task." Enforced the way
-- `sessions_one_live_lease_per_workspace` enforces the lease rule: a partial
-- unique index over exactly the rows that matter, rather than trusting
-- application code to remember to check before every write. SQLite's unique
-- indexes treat every NULL as distinct from every other NULL, so without the
-- `running`-requires-`assigned_session_id` CHECK above (which guarantees
-- `assigned_session_id` is never NULL while `status = 'running'`), this index
-- alone would let any number of running tasks with a NULL
-- `assigned_session_id` coexist. The CHECK and this index are one mechanism:
-- the CHECK is what makes the index bite.
CREATE UNIQUE INDEX tasks_one_running_per_session
    ON tasks (assigned_session_id)
    WHERE status = 'running';
"#;
