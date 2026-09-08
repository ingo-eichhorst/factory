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

/// Migration 4 (backlog §9 / `factory-recovery`'s crate docs, "Migration 4 —
/// an authorised delivery is recorded, not passed"): `tasks` gains a counter
/// that turns "a human authorised one further delivery" into a durable fact
/// a restart can see, rather than a call argument that dies with the process
/// that received it.
///
/// # Plain `ALTER TABLE ADD COLUMN`, not migration 3's rebuild — measured,
/// not assumed
///
/// Migration 2 above rebuilt `scopes` because `ALTER TABLE ... ADD COLUMN`
/// cannot loosen `canonical_path`'s `UNIQUE`/`NOT NULL` constraints on an
/// *existing* table — a grammar limit, not a data one. Migration 3's own
/// reason is sharper than "the grammar disallows a two-column CHECK," and
/// this migration's own measurement 2 below is what actually proves it:
/// `tasks_v3`'s CHECK required `status = 'running' AND assigned_session_id
/// IS NOT NULL`, and a schema-2 database could already hold a `running` row
/// with no `assigned_session_id` at all (the column did not exist yet).
/// `ADD COLUMN` validates a self-referencing CHECK's backfilled default
/// against every existing row before it will commit (measurement 2), so
/// adding that CHECK by `ADD COLUMN` against such a row would have been
/// refused outright — there is no single `DEFAULT` expression that is
/// simultaneously `NOT NULL` for a `running` row and satisfies "non-NULL
/// whenever `status = 'running'`" for a row already sitting in that state.
/// Migration 3 needed the rebuild so its `INSERT ... SELECT` could rewrite
/// exactly those ambiguous rows to `blocked: interrupted` *before* the CHECK
/// had to hold — see `migration_3_rewrites_a_schema_2_running_task_to_
/// blocked_interrupted` in `migrations.rs` for the direct evidence. This
/// column needs none of that: one new column, with a CHECK naming only
/// itself, and `DEFAULT 1` that every existing row can satisfy unconditionally
/// — so before writing the rebuild dance a third time, the question was
/// measured rather than assumed from those two migrations' own comments:
/// does plain `ADD COLUMN` actually suffice here?
///
/// Measured on 2026-09-08 against the exact bundled engine this workspace
/// compiles (`rusqlite` 0.40.2 / `libsqlite3-sys` 0.38.2, ADR 0012
/// decision 1), with a throwaway in-memory database, in two steps:
///
/// 1. **Does `ADD COLUMN` accept a CHECK naming only the new column, and does
///    it bite?** Yes: `ALTER TABLE t ADD COLUMN authorised_deliveries
///    INTEGER NOT NULL DEFAULT 1 CHECK (authorised_deliveries >= 1)`
///    succeeds against a table with an existing row, and a subsequent
///    `UPDATE ... SET authorised_deliveries = 0` is rejected with `CHECK
///    constraint failed` — the same constraint-violation shape
///    `tests/constraints.rs` already asserts for other CHECKs in this
///    schema.
/// 2. **The actual discriminator: does `ADD COLUMN` validate the
///    `DEFAULT`-backfilled value against a CHECK installed in the very same
///    statement, for rows that already exist?** Yes. Against a table already
///    holding one row, `ALTER TABLE t ADD COLUMN x INTEGER NOT NULL DEFAULT 0
///    CHECK (x >= 1)` — a default deliberately chosen to violate its own
///    CHECK — is refused outright (`CHECK constraint failed`), and the
///    column is not added at all (confirmed by reading the table back from
///    `sqlite_master` afterwards: no trace of `x`). SQLite will not let a
///    migration silently leave a violating row behind the way a hand-rolled
///    backfill script could.
///
///    This is what actually justifies `DEFAULT 1` here, not merely "the
///    grammar allows it": `DEFAULT 1` satisfies the CHECK (requiring at
///    least `1`) by construction, so there is no row for a violation to hide
///    in — and measurement 2 confirms that even a mistake (say, a stray
///    `DEFAULT 0`) would fail loudly at migration time on any database
///    holding a task, rather than shipping a silently-violated invariant.
///
/// (The same throwaway database also measured a cross-column CHECK and a
/// `REFERENCES` clause each succeeding via `ADD COLUMN` against this engine
/// version — more than migrations 2 and 3's own doc comments above expect of
/// `ALTER TABLE`. That does not reopen either migration: both are released
/// and forward-only per ADR 0012 decision 2, so their rebuilds stand
/// regardless of what a newer SQLite now accepts. Recorded here only so the
/// next agent does not re-run the same measurement wondering whether this
/// one did.)
///
/// No `.foreign_key_check()` on this migration's entry in `migrations()`,
/// unlike migrations 2 and 3: `ADD COLUMN` neither drops nor recreates
/// `tasks`, so no reference to it from `task_delegation_chain` or
/// `delivery_attempts` is ever invalidated — there is nothing for
/// `PRAGMA foreign_key_check` to catch that this statement could have broken.
///
/// # A count, not a flag
///
/// `factory_task::deliver`'s guard (see that module) refuses once the number
/// of recorded `delivery_attempts` rows reaches this number — "at least the
/// authorised count," not slice 7's "any row at all." A boolean "resume
/// approved" flag could only ever re-authorise one further attempt and would
/// need to be cleared by the same call that consumes it, which is
/// indistinguishable, after a restart lands between the two writes, from
/// "never authorised." A monotonically increasing count needs no reset:
/// `factory_task::deliver::authorise_resume` increments it by exactly one per
/// human action, and the guard's arithmetic does the rest.
///
/// # Backfill
///
/// Every row that exists before this migration backfills to exactly `1`: a
/// task created under slice 7 already carries the authorisation slice 7's
/// guard assumed it had — one ordinary delivery, no resume yet. `DEFAULT 1`
/// *is* the backfill; there is no separate `INSERT ... SELECT` to write
/// because `ADD COLUMN` applies the default to every existing row as part of
/// the same statement (see measurement 2 above for the direct evidence that
/// this is not merely assumed).
pub(crate) const V4_SCHEMA: &str = r#"
ALTER TABLE tasks ADD COLUMN authorised_deliveries INTEGER NOT NULL DEFAULT 1
    CHECK (authorised_deliveries >= 1);
"#;

/// Migration 5 (backlog §9 / `factory-recovery`'s crate docs, "Migration 5 —
/// a session records how to find its harness again"): `sessions` gains two
/// nullable identity columns. Same reasoning as migration 4 for the form:
/// two new columns, neither a `REFERENCES` clause nor a CHECK spanning
/// another column, so plain `ADD COLUMN` suffices and no
/// `.foreign_key_check()` is needed on this migration's `migrations()` entry
/// either — nothing referencing `sessions` is ever dropped or recreated by
/// it.
///
/// # Why nullable, and why that is not merely "no default was chosen"
///
/// `grep -ic pane crates/factory-store/src/schema.rs` against every migration
/// before this one returns `0`: no session row has ever recorded a pane id or
/// a harness session id, so every pre-existing row backfills to `NULL` by
/// construction — the same situation `dev`, `ino`, and `parent_id` were left
/// in by migration 2, for the same reason (there is no historical value to
/// invent for a fact schema 1 through 4 never captured).
///
/// But `NULL` is also the only honest value for a session Factory has never
/// *since* observed, not only one it has not observed *yet*. ADR 0019
/// decision 3: "A snapshot may be hours or days old. The pane identifiers it
/// names may since have been reused by entirely different sessions, so a
/// pane that exists is not evidence that *this* session exists." A value in
/// either column is never a live claim that the pane or the harness session
/// still exists — only that it was observed once. Whether a recorded id is
/// still worth anything is `factory_recovery::evidence`'s question, not this
/// schema's: ADR 0019 decision 3 draws exactly that line ("this ADR says what
/// the database may claim on its own; Slice 9 says what live evidence is
/// allowed to change about that claim"), and `factory-recovery`'s crate docs
/// repeat it as the split this whole slice sits on.
///
/// # This migration ships with no writer, on purpose
///
/// Nothing in `factory-task` — the only other crate this agent may touch this
/// slice — writes to `sessions` at all. `factory_task::complete` already
/// receives a full `factory_adapter::Observation` (carrying `pane: PaneId`
/// and `harness_session_id: Option<String>`) in
/// [`factory_task::complete::blocked_from_observation`], and discards both
/// fields after reading only `task_signal` and `confidence` — but that
/// function updates `tasks`, never `sessions`, and has no session id in its
/// own signature to write one against. Writing to `sessions` at all is
/// `factory_session`'s exclusive job in this codebase (`begin_start`'s
/// `INSERT` and `transition_in_tx`'s `UPDATE` are the only two sites in the
/// whole workspace that touch the table), and `factory-session` is another
/// agent's crate this slice. Adding a third write site in `factory-task` that
/// reaches around `factory-session` into a table it does not own would be a
/// bigger, uninvited change than "add two columns," not a smaller one.
///
/// The natural first writer is `factory_recovery::reconnect` — the crate docs
/// name it as "the only place a live observation is permitted to move a
/// session at all," and a live observation is exactly what carries a pane id
/// and a harness session id. That module is empty on purpose, owned by
/// another agent this slice. A column nothing yet writes is recorded here as
/// a real, visible gap rather than hidden behind a writer bolted on in the
/// wrong crate.
///
/// **What `harness_session_id` holds, since nothing yet pins it.** It is the
/// bare UUID `factory_adapter::parse_harness_session_id` lifts out of a Pi
/// transcript filename (`<iso-timestamp>_<uuid>.jsonl`), not the path itself.
/// Slice 9 writes it and compares it, and both sides come from that one
/// extractor — so the comparison is self-consistent whatever the format is,
/// and no test in the workspace would notice if it changed. The first caller
/// that will actually care is slice 10's terminal attach, which has to turn a
/// stored value back into something a harness recognises. Recorded here so
/// that caller finds the answer rather than inferring it.
pub(crate) const V5_SCHEMA: &str = r#"
ALTER TABLE sessions ADD COLUMN herdr_pane_id TEXT;
ALTER TABLE sessions ADD COLUMN harness_session_id TEXT;
"#;
