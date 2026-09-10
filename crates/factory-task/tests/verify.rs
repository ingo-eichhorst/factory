//! `factory_task::verify`'s own test suite: verification verdicts (design
//! §12.1's hook; ADR 0021 decision 4, and `crate`'s own decision 4, which
//! this module implements byte for byte).
//!
//! Every fixture below is duplicated from `tests/create.rs` rather than
//! shared through a `tests/common/mod.rs` — the same convention every other
//! test file in this crate already states for itself.

use factory_paths::CanonicalPath;
use factory_store::Store;
use factory_task::create::{create, show};
use factory_task::events::{EventType, for_task};
use factory_task::verify::{VerifyError, record_verdict};
use factory_task::{TaskError, TaskStatus};

/// A deterministic, distinct, syntactically valid UUID — mirrors
/// `tests/create.rs::uid`.
fn uid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

/// Insert a scope row directly, bypassing `factory-registry` — mirrors
/// `tests/create.rs::seed_scope`.
fn seed_scope(store: &mut Store, seed: u32, name: &str, canonical_path: &str) -> uuid::Uuid {
    let id = uid(seed);
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO scopes (id, name, declared_path, canonical_path) VALUES (?1, ?2, ?3, ?3)",
        (id.to_string(), name, canonical_path),
    )
    .expect("insert scope");
    tx.commit().expect("commit");
    id
}

/// Start a fresh session of `scope_id`, in a fresh workspace under `dir` —
/// left `starting` deliberately: `verify` only ever reads `sessions.scope_id`,
/// which is set at creation, so this fixture does not need to mark it
/// `running` the way an idle-session fixture would.
fn seed_session(
    store: &mut Store,
    dir: &std::path::Path,
    seed: u32,
    scope_id: uuid::Uuid,
) -> uuid::Uuid {
    let session_id = uid(seed);
    let workspace_dir = dir.join(format!("workspace-{seed}"));
    std::fs::create_dir(&workspace_dir).expect("create workspace directory");
    let workspace = CanonicalPath::resolve(&workspace_dir).expect("resolve workspace");
    factory_session::begin_start(store, session_id, scope_id, "agent", 10, &workspace)
        .expect("start session");
    session_id
}

fn seed_task(store: &mut Store, seed: u32, scope_id: uuid::Uuid) -> uuid::Uuid {
    create(store, uid(seed), None, scope_id, None, None, "do it", &[]).expect("create")
}

/// §12.1's ordinary case: a session in a *different* scope than the run's
/// `target_scope_id` may verify it.
#[test]
fn a_verdict_by_a_session_of_a_different_scope_is_accepted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let target_scope = seed_scope(&mut store, 1, "target", "/instance/target");
    let other_scope = seed_scope(&mut store, 2, "other", "/instance/other");
    let task_id = seed_task(&mut store, 3, target_scope);
    let verifier = seed_session(&mut store, dir.path(), 4, other_scope);

    record_verdict(&mut store, task_id, Some(verifier), "pass").expect("independent verdict");

    let events = for_task(&store, task_id).expect("read events");
    assert_eq!(events.len(), 2, "created, then verification");
    assert_eq!(events[1].event_type, EventType::Verification);
    assert_eq!(events[1].author_session_id, Some(verifier));
    let payload: serde_json::Value =
        serde_json::from_str(events[1].payload.as_deref().expect("payload present"))
            .expect("valid JSON");
    assert_eq!(payload["verdict"], "pass");
}

/// §12.1's core rule: a *sibling* session of the run's own scope is refused,
/// even though it is not the session the run was assigned to. This is the
/// direct evidence the guard compares scopes, not sessions — a
/// session-level guard (comparing against `assigned_session_id`) would let
/// this sibling through, since it is a different session id.
#[test]
fn a_verdict_by_a_sibling_session_of_the_runs_own_scope_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let assigned_session = seed_session(&mut store, dir.path(), 2, scope_id);
    let sibling_session = seed_session(&mut store, dir.path(), 3, scope_id);
    assert_ne!(assigned_session, sibling_session);

    let task_id = seed_task(&mut store, 4, scope_id);
    {
        let tx = store.transaction().expect("begin");
        tx.execute(
            "UPDATE tasks SET assigned_session_id = ?2 WHERE id = ?1",
            (task_id.to_string(), assigned_session.to_string()),
        )
        .expect("assign");
        tx.commit().expect("commit");
    }

    let err = record_verdict(&mut store, task_id, Some(sibling_session), "pass")
        .expect_err("a sibling session of the same scope is not independent");
    assert!(matches!(
        err,
        VerifyError::NotIndependent { session_id, task_id: t }
        if session_id == sibling_session && t == task_id
    ));
}

/// The hole test (`crate`'s own decision 4, and this task's brief by name):
/// a task whose `assigned_session_id` is NULL — this one was never assigned
/// at all, the simplest of the three cases ADR 0021 decision 4 names ("one a
/// human closed, one cancelled while queued, one whose only delivery
/// attempt was refused") — but whose `target_scope_id` is the author's own
/// scope must still be refused. A guard comparing against
/// `assigned_session_id` instead would find `session_id != NULL` trivially
/// true for *any* session and let this through regardless of scope; this is
/// exactly the mutation that would pass every other test in this file while
/// failing this one.
#[test]
fn the_hole_test_a_never_assigned_task_still_refuses_a_same_scope_verifier() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = seed_task(&mut store, 2, scope_id);
    let task_before = show(&store, task_id).expect("show");
    assert_eq!(
        task_before.assigned_session_id, None,
        "the fixture's own precondition: `create` never sets this column"
    );
    let verifier = seed_session(&mut store, dir.path(), 3, scope_id);

    let err = record_verdict(&mut store, task_id, Some(verifier), "pass").expect_err(
        "target_scope_id matches the verifier's scope, independent of assigned_session_id",
    );
    assert!(matches!(
        err,
        VerifyError::NotIndependent { session_id, task_id: t }
        if session_id == verifier && t == task_id
    ));

    let events = for_task(&store, task_id).expect("read events");
    assert_eq!(
        events.len(),
        1,
        "only `created` — the refused verdict wrote nothing"
    );
}

/// A verdict with no author session is a human's, and §12.1's independence
/// rule has nothing to say about a human — always allowed, regardless of
/// scope.
#[test]
fn a_verdict_with_no_author_session_is_always_accepted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = seed_task(&mut store, 2, scope_id);

    record_verdict(&mut store, task_id, None, "pass").expect("a human's verdict is always allowed");

    let events = for_task(&store, task_id).expect("read events");
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].event_type, EventType::Verification);
    assert_eq!(events[1].author_session_id, None);
}

/// Decision 4's whole point: a verdict annotates, it never transitions. The
/// entire `tasks` row — read back through the crate's own `Task` type, so
/// every column is covered, not a hand-picked subset — must be identical
/// before and after a successful verdict.
#[test]
fn a_verdict_changes_no_column_of_tasks() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let target_scope = seed_scope(&mut store, 1, "target", "/instance/target");
    let other_scope = seed_scope(&mut store, 2, "other", "/instance/other");
    let task_id = seed_task(&mut store, 3, target_scope);
    let verifier = seed_session(&mut store, dir.path(), 4, other_scope);

    let before = show(&store, task_id).expect("show before");
    record_verdict(&mut store, task_id, Some(verifier), "pass").expect("verdict");
    let after = show(&store, task_id).expect("show after");

    assert_eq!(before, after, "a verdict must touch no column of tasks");
}

/// ADR 0021 decision 5's own framing: a verdict is "an event recorded
/// against an already-finished run." Recording one against a `done` task
/// must succeed and must not resurrect its status.
#[test]
fn a_verdict_can_be_recorded_against_an_already_done_task() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let target_scope = seed_scope(&mut store, 1, "target", "/instance/target");
    let other_scope = seed_scope(&mut store, 2, "other", "/instance/other");
    let task_id = seed_task(&mut store, 3, target_scope);
    let session_id = seed_session(&mut store, dir.path(), 4, target_scope);
    factory_session::mark_running(&mut store, session_id).expect("readiness observed");
    {
        let tx = store.transaction().expect("begin");
        tx.execute(
            "UPDATE tasks SET assigned_session_id = ?2, status = 'running' WHERE id = ?1",
            (task_id.to_string(), session_id.to_string()),
        )
        .expect("move to running directly, bypassing deliver for this fixture");
        tx.commit().expect("commit");
    }
    factory_task::complete::done(&mut store, task_id, Some("finished"), None).expect("done");

    let verifier = seed_session(&mut store, dir.path(), 5, other_scope);
    record_verdict(&mut store, task_id, Some(verifier), "pass")
        .expect("verdict against a done run");

    let task = show(&store, task_id).expect("show");
    assert_eq!(
        task.status,
        TaskStatus::Done,
        "the verdict must not resurrect the status"
    );
}

#[test]
fn a_verdict_against_a_nonexistent_task_is_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let err = record_verdict(&mut store, uid(999), None, "pass").expect_err("no such task");
    assert!(matches!(err, VerifyError::Task(TaskError::NotFound(id)) if id == uid(999)));
}

/// An author session that names no real row is a caller bug worth
/// surfacing as its own typed error, distinct from `TaskError::NotFound` —
/// resolved before the task itself is even read (see `record_verdict`'s own
/// doc comment on the ordering).
#[test]
fn a_verdict_by_a_nonexistent_session_is_a_session_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = seed_task(&mut store, 2, scope_id);

    let err = record_verdict(&mut store, task_id, Some(uid(999)), "pass")
        .expect_err("no such session exists");
    assert!(matches!(
        err,
        VerifyError::Session(factory_session::SessionError::NotFound(id)) if id == uid(999)
    ));
}
