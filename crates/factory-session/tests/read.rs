//! Read-only inspection for sessions and their workspace-lease journal —
//! backlog §8's closing acceptance criterion: "[t]he operator can inspect
//! session, task, lease, and compiled-context records without relying on
//! terminal scrollback." `factory_task::create::{list, show}` and
//! `factory_context::compile` already existed when this criterion was
//! written; `factory_session::{list, show, leases_of_session}` — this file's
//! subject — did not.
//!
//! # Why `list`'s fixture inserts rows out of chronological order
//!
//! `sessions.created_at` defaults to `CURRENT_TIMESTAMP`, which has
//! second-level resolution. A fixture that calls [`begin_start`] three times
//! in a loop cannot reliably produce rows whose *insertion* order disagrees
//! with their *chronological* order — the whole loop typically finishes
//! inside one wall-clock second, ties every row on `created_at`, and would
//! leave `list`'s `ORDER BY created_at, id` and a mutated, order-free
//! `SELECT` (which falls back to SQLite's natural, insertion-order scan of a
//! rowid table with no other applicable index) producing the *same*
//! sequence — exactly the "a test that cannot fail is worse than no test"
//! trap the backlog warns about elsewhere for lease tests. `seed_session_at`
//! below writes `created_at` explicitly and is called with the *newest*
//! session first, so insertion order and chronological order are guaranteed
//! to disagree; see [`list_returns_every_session_oldest_first`].

mod common;

use factory_session::{
    SessionError, SessionState, begin_start, leases_of_session, list, mark_running, show, stop,
};
use factory_store::Store;

/// Insert a `sessions` row directly with an explicit `created_at`, bypassing
/// [`begin_start`] for the reason given in the module docs. Mirrors
/// `common::seed_scope`/`seed_task`'s own stance on writing raw SQL for a
/// fixture the real write path cannot otherwise produce. `state` is always
/// `'running'`: nothing here exercises the state machine, only the columns
/// [`list`] and [`show`] read back.
fn seed_session_at(
    store: &mut Store,
    seed: u32,
    scope_id: uuid::Uuid,
    agent_name: &str,
    created_at: &str,
) -> uuid::Uuid {
    let id = common::uid(seed);
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO sessions \
         (id, scope_id, agent_name, workspace_path, state, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, 'running', ?5, ?5)",
        (
            id.to_string(),
            scope_id.to_string(),
            agent_name,
            format!("/instance/workspace-{seed}"),
            created_at,
        ),
    )
    .expect("insert session");
    tx.commit().expect("commit");
    id
}

// list -----------------------------------------------------------------

/// The load-bearing ordering test — see the module docs for why the fixture
/// inserts newest-first. If `list`'s `ORDER BY created_at, id` were dropped,
/// this must observe the reverse sequence, not the correct one by
/// coincidence.
#[test]
fn list_returns_every_session_oldest_first() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());

    let newest = seed_session_at(
        &mut store,
        1,
        scope_id,
        "agent-newest",
        "2026-01-03T00:00:00",
    );
    let middle = seed_session_at(
        &mut store,
        2,
        scope_id,
        "agent-middle",
        "2026-01-02T00:00:00",
    );
    let oldest = seed_session_at(
        &mut store,
        3,
        scope_id,
        "agent-oldest",
        "2026-01-01T00:00:00",
    );

    let sessions = list(&store).expect("list");
    let ids: Vec<uuid::Uuid> = sessions.iter().map(|s| s.id).collect();
    assert_eq!(
        ids,
        vec![oldest, middle, newest],
        "list must return sessions ordered oldest-created first; the fixture \
         inserted them newest-first, so a wrong or absent ORDER BY would \
         show up here as the reverse sequence, not as this one by accident"
    );
}

/// `created_at`'s second resolution means two sessions can legitimately tie
/// on it; `id` is what keeps `list`'s ordering total rather than merely
/// "usually right." Both rows here share one `created_at` and differ only in
/// `id`. Measured, not assumed: a first version of this test inserted the
/// lower-id row first, which a mutation dropping the `, id` tiebreaker
/// (`ORDER BY created_at` alone) survived — with only one applicable index, a
/// bare `ORDER BY created_at` scan happens to visit tied rows in the same
/// insertion order they were written in, so ties that are already inserted
/// in ascending-id order stay in ascending-id order even with the tiebreaker
/// gone. The fixture below inserts the *higher*-id row (`second`) first and
/// the lower-id row (`first`) second, so insertion order and the promised
/// `id`-ascending order disagree; only then does dropping `, id` change the
/// observed result.
#[test]
fn list_breaks_a_created_at_tie_by_id() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());

    // Inserted higher-id-first (reverse of the promised output order) — see
    // the doc comment above for why insertion order must disagree with
    // `id` order for this fixture to discriminate anything.
    let second = seed_session_at(&mut store, 2, scope_id, "agent-b", "2026-01-01T00:00:00");
    let first = seed_session_at(&mut store, 1, scope_id, "agent-a", "2026-01-01T00:00:00");

    let sessions = list(&store).expect("list");
    let ids: Vec<uuid::Uuid> = sessions.iter().map(|s| s.id).collect();
    assert_eq!(
        ids,
        vec![first, second],
        "created_at is tied; list must still break the tie by ascending id, \
         not by insertion order"
    );
}

// show -------------------------------------------------------------------

/// The mutation this test exists to kill: a `show` that ignores its `id`
/// argument and returns the first row regardless (for example, dropping the
/// `WHERE id = ?1` clause and adding `LIMIT 1`). Two sessions are seeded so
/// that "the first one" and "the one actually named" are provably different
/// rows.
#[test]
fn show_returns_the_named_session_not_the_first_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());

    let first = seed_session_at(
        &mut store,
        1,
        scope_id,
        "agent-first",
        "2026-01-01T00:00:00",
    );
    let second = seed_session_at(
        &mut store,
        2,
        scope_id,
        "agent-second",
        "2026-01-02T00:00:00",
    );

    let session = show(&store, second).expect("show the second session by its own id");
    assert_eq!(session.id, second);
    assert_eq!(session.agent_name, "agent-second");
    assert_eq!(session.state, SessionState::Running);
    assert_ne!(
        session.id, first,
        "show(second) must not silently return the first-inserted row"
    );
}

#[test]
fn show_of_a_nonexistent_session_is_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path()).expect("open");

    let err = show(&store, common::uid(999)).expect_err("no such session exists");
    assert!(matches!(err, SessionError::NotFound(id) if id == common::uid(999)));
}

// leases_of_session --------------------------------------------------------

/// The mutation this test exists to kill: a `leases_of_session` that adds
/// `AND released_at IS NULL` (or otherwise filters to only-open leases). A
/// stopped session's one lease row is released — this test asserts it still
/// comes back, with its `released_at` and `release_reason` populated, which
/// is exactly what "the operator can inspect... lease... records" (backlog
/// §8) means beyond "is a lease held right now."
#[test]
fn leases_of_session_returns_a_released_lease_not_just_open_ones() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());

    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir(&workspace_dir).expect("create");
    let workspace = common::resolve(&workspace_dir);

    let session_id = common::uid(1);
    begin_start(&mut store, session_id, scope_id, "agent-a", 10, &workspace)
        .expect("begin_start records the one lease this test reads back");
    mark_running(&mut store, session_id).expect("readiness observed");
    stop(&mut store, session_id, "operator stopped it for this test").expect("stop releases it");

    let leases = leases_of_session(&store, session_id).expect("leases_of_session");
    assert_eq!(
        leases.len(),
        1,
        "expected exactly the one lease begin_start recorded, released or not"
    );
    let lease = &leases[0];
    assert_eq!(
        lease.canonical_workspace_path,
        workspace.as_path().to_string_lossy()
    );
    assert!(
        !lease.acquired_at.is_empty(),
        "acquired_at must be recorded"
    );
    assert!(
        lease.released_at.is_some(),
        "a stopped session's lease must show as released, not silently dropped"
    );
    assert_eq!(
        lease.release_reason.as_deref(),
        Some("operator stopped it for this test")
    );
}

/// The filter (`WHERE session_id = ?1`) is not incidental: two sessions each
/// get their own lease, and asking for one must never surface the other's.
#[test]
fn leases_of_session_never_returns_another_sessions_lease() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());

    let workspace_a = common::resolve(&{
        let p = dir.path().join("workspace-a");
        std::fs::create_dir(&p).expect("create");
        p
    });
    let workspace_b = common::resolve(&{
        let p = dir.path().join("workspace-b");
        std::fs::create_dir(&p).expect("create");
        p
    });

    let session_a = common::uid(1);
    let session_b = common::uid(2);
    begin_start(&mut store, session_a, scope_id, "agent-a", 10, &workspace_a).expect("start a");
    begin_start(&mut store, session_b, scope_id, "agent-b", 10, &workspace_b).expect("start b");

    let leases_a = leases_of_session(&store, session_a).expect("leases for a");
    assert_eq!(leases_a.len(), 1);
    assert_eq!(
        leases_a[0].canonical_workspace_path,
        workspace_a.as_path().to_string_lossy()
    );

    let leases_b = leases_of_session(&store, session_b).expect("leases for b");
    assert_eq!(leases_b.len(), 1);
    assert_eq!(
        leases_b[0].canonical_workspace_path,
        workspace_b.as_path().to_string_lossy()
    );
}

/// A `session_id` with no `workspace_leases` rows at all (here, one seeded
/// by raw SQL rather than [`begin_start`]) produces an empty history, not an
/// error — mirrors `factory_task::create::delegation_chain_of`'s own stance
/// on an id that names no rows in its journal table.
#[test]
fn leases_of_session_for_a_session_with_no_lease_rows_is_empty() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());
    let session_id = seed_session_at(&mut store, 1, scope_id, "agent-a", "2026-01-01T00:00:00");

    let leases = leases_of_session(&store, session_id).expect("leases_of_session");
    assert!(leases.is_empty());
}
