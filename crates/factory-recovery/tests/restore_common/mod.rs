//! Shared fixtures for `restore.rs`'s own integration tests.
//!
//! Named `restore_common`, not `common`: another agent's reconnect tests
//! (`tests/supervisor_restart.rs`) already claim `tests/common/mod.rs` for
//! their own fixtures, and `tests/` is a flat namespace shared by every test
//! binary in this crate — a second `mod common;` pointing at the same path
//! would silently replace whichever file was written first. This module is
//! this file's own answer to that collision: a distinct subdirectory, so
//! neither agent's fixtures are edited by the other.
//!
//! Mirrors `factory-session/tests/common/mod.rs`'s own choices (a
//! deterministic `uid` built from a seed, raw-SQL scope seeding) for the same
//! reasons that file gives: `uuid` is pinned workspace-wide without the `v4`
//! feature, and this crate has no dependency on `factory-registry` to build a
//! scope through.
//!
//! Sessions are seeded with a *real, canonicalized, on-disk* workspace
//! directory rather than an arbitrary string, specifically so that lease
//! enforcement (`sessions_one_live_lease_per_workspace`,
//! `factory_session::begin_start`'s Rust-level aliasing scan) actually bites
//! in tests that need to prove a lease was kept rather than merely that a
//! `state` column reads `disconnected` — see `restore.rs`'s
//! `reconcile_disconnects_every_lease_holding_session_and_keeps_its_lease`,
//! which is the whole reason this module exists rather than a one-line raw
//! INSERT.

#![allow(dead_code)]

use std::path::Path;

use factory_paths::CanonicalPath;
use factory_store::Store;

pub fn uid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

/// Insert a scope row directly, bypassing `factory-registry` (which this
/// crate does not depend on), so tests have a `scope_id` to satisfy
/// `sessions.scope_id REFERENCES scopes (id)` and
/// `tasks.target_scope_id REFERENCES scopes (id)`.
pub fn seed_scope(store: &mut Store, seed: u32, canonical_path: &Path) -> uuid::Uuid {
    let id = uid(seed);
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO scopes (id, name, declared_path, canonical_path) VALUES (?1, 'scope', ?2, ?2)",
        (
            id.to_string(),
            canonical_path.to_string_lossy().into_owned(),
        ),
    )
    .expect("insert scope");
    tx.commit().expect("commit");
    id
}

/// Create `workspace` on disk, canonicalize it, and insert a `sessions` row
/// directly in `state` — plus, when `state` holds a lease
/// (`starting`/`running`/`disconnected`), the matching open
/// `workspace_leases` row a real `begin_start` would have left behind. This
/// is exactly the shape ADR 0019 describes a restored snapshot as carrying:
/// "`sessions` rows in `starting`, `running`, or `disconnected`... naming
/// panes that may not exist any more" plus "`workspace_leases` rows with
/// `released_at IS NULL`, held on behalf of those sessions."
///
/// Raw SQL, not `factory_session::begin_start` plus its transition
/// functions: this fixture needs to plant a session already sitting in an
/// arbitrary state, including states a snapshot restored as-is would never
/// have reached through a live sequence of calls in *this* process (the
/// snapshot's `starting`/`running` rows are relics of a process that no
/// longer exists), so building it through the real state machine would be
/// fighting the fixture's own premise.
pub fn seed_session(
    store: &mut Store,
    seed: u32,
    scope_id: uuid::Uuid,
    workspace: &Path,
    state: &str,
) -> uuid::Uuid {
    std::fs::create_dir(workspace).expect("create workspace dir");
    let canonical = CanonicalPath::resolve(workspace).expect("resolve workspace");
    let workspace_str = canonical.as_path().to_string_lossy().into_owned();

    let id = uid(seed);
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO sessions (id, scope_id, agent_name, workspace_path, state) \
         VALUES (?1, ?2, 'agent', ?3, ?4)",
        (id.to_string(), scope_id.to_string(), &workspace_str, state),
    )
    .expect("insert session");
    if matches!(state, "starting" | "running" | "disconnected") {
        tx.execute(
            "INSERT INTO workspace_leases (session_id, canonical_workspace_path) VALUES (?1, ?2)",
            (id.to_string(), &workspace_str),
        )
        .expect("insert lease");
    }
    tx.commit().expect("commit");
    id
}

/// Insert a `tasks` row directly with `status`/`assigned_session_id` set
/// exactly as given — mirrors `seed_session`'s reasoning: a restored
/// snapshot's `running` or `queued` rows are relics of a process this test
/// never runs, so the fixture plants the row's final shape rather than
/// re-deriving it by calling `factory-task`'s real `create`/`assign`/`deliver`
/// sequence (which this crate does not own and whose private `TaskStatus`
/// conversions it cannot reach in any case).
pub fn seed_task(
    store: &mut Store,
    seed: u32,
    target_scope_id: uuid::Uuid,
    status: &str,
    assigned_session_id: Option<uuid::Uuid>,
) -> uuid::Uuid {
    let id = uid(seed);
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO tasks (id, target_scope_id, assigned_session_id, prompt, status) \
         VALUES (?1, ?2, ?3, 'do the thing', ?4)",
        (
            id.to_string(),
            target_scope_id.to_string(),
            assigned_session_id.map(|s| s.to_string()),
            status,
        ),
    )
    .expect("insert task");
    tx.commit().expect("commit");
    id
}

/// Insert a `tasks` row already `blocked`, which needs a `blocked_reason`
/// the CHECK constraint requires — kept separate from [`seed_task`] rather
/// than adding an `Option<&str>` parameter there, since every other status
/// this crate's tests seed forbids a reason at all.
pub fn seed_blocked_task(
    store: &mut Store,
    seed: u32,
    target_scope_id: uuid::Uuid,
    blocked_reason: &str,
) -> uuid::Uuid {
    let id = uid(seed);
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO tasks (id, target_scope_id, prompt, status, blocked_reason) \
         VALUES (?1, ?2, 'do the thing', 'blocked', ?3)",
        (id.to_string(), target_scope_id.to_string(), blocked_reason),
    )
    .expect("insert task");
    tx.commit().expect("commit");
    id
}

/// Record a `delivery_attempts` row for `task_id` against `session_id`,
/// mirroring what `factory_task::deliver` commits *before* ever calling a
/// prompt writer (design §5 step 3) — the fact ADR 0019 decision 2's middle
/// row exists to recognise.
pub fn record_delivery_attempt(store: &mut Store, task_id: uuid::Uuid, session_id: uuid::Uuid) {
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO delivery_attempts (task_id, session_id, outcome) VALUES (?1, ?2, NULL)",
        (task_id.to_string(), session_id.to_string()),
    )
    .expect("insert delivery attempt");
    tx.commit().expect("commit");
}

pub fn session_state(store: &Store, id: uuid::Uuid) -> String {
    store
        .connection()
        .query_row(
            "SELECT state FROM sessions WHERE id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .expect("session row exists")
}

pub fn session_updated_at(store: &Store, id: uuid::Uuid) -> String {
    store
        .connection()
        .query_row(
            "SELECT updated_at FROM sessions WHERE id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .expect("session row exists")
}

pub fn lease_is_open(store: &Store, session_id: uuid::Uuid) -> bool {
    let released_at: Option<String> = store
        .connection()
        .query_row(
            "SELECT released_at FROM workspace_leases WHERE session_id = ?1",
            [session_id.to_string()],
            |row| row.get(0),
        )
        .expect("lease row exists");
    released_at.is_none()
}

/// `(status, blocked_reason, assigned_session_id)` for `id`'s task row.
pub fn task_row(store: &Store, id: uuid::Uuid) -> (String, Option<String>, Option<String>) {
    store
        .connection()
        .query_row(
            "SELECT status, blocked_reason, assigned_session_id FROM tasks WHERE id = ?1",
            [id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("task row exists")
}

pub fn task_updated_at(store: &Store, id: uuid::Uuid) -> String {
    store
        .connection()
        .query_row(
            "SELECT updated_at FROM tasks WHERE id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .expect("task row exists")
}
