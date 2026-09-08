//! Shared helpers for factory-session's integration tests.

#![allow(dead_code)]

use std::path::Path;

use factory_paths::CanonicalPath;
use factory_store::Store;

/// A deterministic, distinct, syntactically valid UUID. `uuid` is pinned
/// workspace-wide without the `v4` feature (see
/// `crates/factory-registry/tests/registry.rs`'s `uid` helper, which this
/// mirrors), so tests build UUIDs by hand from a seed rather than generating
/// them.
pub fn uid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

/// Resolve `path`, panicking with the test's own context on failure — every
/// caller has just created the directory itself, so a failure here means the
/// test fixture is broken, not that the behaviour under test failed.
pub fn resolve(path: &Path) -> CanonicalPath {
    CanonicalPath::resolve(path).expect("resolve a directory this test just created")
}

/// Insert a scope row directly, bypassing `factory-registry` (which this
/// crate does not depend on), so tests have a `scope_id` to satisfy
/// `sessions.scope_id REFERENCES scopes (id)`. Returns the id it used.
pub fn seed_scope(store: &mut Store, seed: u32, name: &str, canonical_path: &Path) -> uuid::Uuid {
    let id = uid(seed);
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO scopes (id, name, declared_path, canonical_path) VALUES (?1, ?2, ?3, ?3)",
        (
            id.to_string(),
            name,
            canonical_path.to_string_lossy().into_owned(),
        ),
    )
    .expect("insert scope");
    tx.commit().expect("commit");
    id
}

/// Insert a task row directly, bypassing the not-yet-built task-queue crate
/// (Slice 7), so `tests/lifetime.rs` and `tests/interrupted.rs` have a task
/// to point [`factory_session::on_task_terminal`] and
/// [`factory_session::interrupt`] at. Returns the id it used.
///
/// `status` and `blocked_reason` are taken as plain strings rather than a
/// typed enum this crate does not own (the vocabulary belongs to
/// `factory-store`'s schema, per design §2.4) — mirroring `seed_scope`'s own
/// choice to write raw SQL rather than depend on a crate that owns the
/// concept.
pub fn seed_task(
    store: &mut Store,
    seed: u32,
    target_scope_id: uuid::Uuid,
    status: &str,
    blocked_reason: Option<&str>,
    target_session_id: Option<uuid::Uuid>,
) -> uuid::Uuid {
    let id = uid(seed);
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO tasks (id, target_scope_id, target_session_id, prompt, status, blocked_reason) \
         VALUES (?1, ?2, ?3, 'do the thing', ?4, ?5)",
        (
            id.to_string(),
            target_scope_id.to_string(),
            target_session_id.map(|s| s.to_string()),
            status,
            blocked_reason,
        ),
    )
    .expect("insert task");
    tx.commit().expect("commit");
    id
}

/// The current `sessions.state` for `id`, read directly rather than through
/// any `factory_session` accessor — this crate deliberately exposes no
/// "read a session's state" function of its own (every real caller learns a
/// session's outcome from the `Result` a transition returns), so tests read
/// the row the same way [`the_schema_rejects_deleting_a_session_that_a_lease_still_references`][1]
/// does: directly.
///
/// [1]: ../../tests/interrupted.rs
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

/// `(status, blocked_reason)` for `id`'s task row, read directly for the same
/// reason as [`session_state`].
pub fn task_row(store: &Store, id: uuid::Uuid) -> (String, Option<String>) {
    store
        .connection()
        .query_row(
            "SELECT status, blocked_reason FROM tasks WHERE id = ?1",
            [id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("task row exists")
}

/// Whether `session_id`'s most recent `workspace_leases` row is still open
/// (`released_at IS NULL`) — the direct, table-level check that a rejected
/// `begin_start` (say, on `max_sessions`) left an *existing* session's lease
/// completely undisturbed, rather than inferring it indirectly from whether a
/// later `begin_start` on the same workspace succeeds.
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

/// Count of rows in `sessions` for `(scope_id, agent_name)`, regardless of
/// state — used to assert that a rejected `begin_start` inserted nothing, and
/// that [`factory_session::interrupt`] leaves exactly the one (now `failed`)
/// row behind rather than creating a replacement.
pub fn session_count_for_agent(store: &Store, scope_id: uuid::Uuid, agent_name: &str) -> i64 {
    store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE scope_id = ?1 AND agent_name = ?2",
            (scope_id.to_string(), agent_name),
            |row| row.get(0),
        )
        .expect("count query succeeds")
}

/// Count of `delivery_attempts` rows for `task_id` — used to pin "Factory
/// never redelivers" (module docs, `interrupt`'s doc comment): a rule with no
/// line to delete and therefore no mutation to catch, so it is asserted
/// directly instead. Zero before and after `interrupt` *is* "no redelivery
/// happened," made durable and checkable.
pub fn delivery_attempt_count(store: &Store, task_id: uuid::Uuid) -> i64 {
    store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM delivery_attempts WHERE task_id = ?1",
            [task_id.to_string()],
            |row| row.get(0),
        )
        .expect("count query succeeds")
}
