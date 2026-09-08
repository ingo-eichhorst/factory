//! `factory_task::create`'s own test suite: creation, read-only inspection,
//! and cooperative cancellation (design §2.4, §5; backlog §7).
//!
//! # A note on the fixtures below
//!
//! `assign` and `deliver` — the two modules that would, in a complete Slice
//! 7, actually choose a session and mark a task `running` — stay untouched
//! placeholders in this task (another agent owns them). `create` itself only
//! ever writes `queued`. So the tests here that need a `running` task (to
//! exercise `cancel`'s cooperative path, and the fixtures the task report's
//! required mutations target) seed one directly with raw SQL, the same way
//! `factory-session`'s own test suite seeds tasks it does not own the
//! creation of. [`seed_running_task`] is the load-bearing fixture for the
//! task report's mutation 2 ("in whatever code path sets a task running,
//! write NULL into `assigned_session_id`"): since no real "assign" code path
//! exists yet to mutate, writing NULL into this helper's insert is the
//! substitution that says the same thing — the schema's CHECK rejects the
//! insert outright, and every test built on top of the helper fails as a
//! result.

use factory_paths::CanonicalPath;
use factory_store::Store;
use factory_task::create::{cancel, create, list, show};
use factory_task::{TaskError, TaskStatus};

/// A deterministic, distinct, syntactically valid UUID. `uuid` is pinned
/// workspace-wide without the `v4` feature, so tests build UUIDs by hand
/// from a seed — mirrors `factory-session/tests/common/mod.rs::uid`.
fn uid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

/// Insert a scope row directly, bypassing `factory-registry` (which this
/// crate does not depend on) — mirrors `factory-session`'s own `seed_scope`.
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

/// Start and mark `running` a fresh session, in a fresh workspace directory
/// under `dir` named uniquely by `seed`. Returns its id.
fn seed_running_session(
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
    factory_session::mark_running(store, session_id).expect("readiness observed");
    session_id
}

/// Seed a `running` task directly, with `assigned_session_id` set to
/// `session_id` — see the module docs for why this bypasses `create` (which
/// only ever writes `queued`) and why it is the fixture the task report's
/// mutation 2 targets.
fn seed_running_task(
    store: &mut Store,
    seed: u32,
    scope_id: uuid::Uuid,
    session_id: uuid::Uuid,
) -> uuid::Uuid {
    let task_id = uid(seed);
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO tasks (id, target_scope_id, assigned_session_id, prompt, status) \
         VALUES (?1, ?2, ?3, 'do it', 'running')",
        (
            task_id.to_string(),
            scope_id.to_string(),
            session_id.to_string(),
        ),
    )
    .expect("insert running task");
    tx.commit().expect("commit");
    task_id
}

/// Seed a `blocked` task directly, with a valid reason — bypasses `create`
/// (which only ever writes `queued`) so `cancel`'s `blocked → cancelled`
/// path has a fixture to exercise.
fn seed_blocked_task(store: &mut Store, seed: u32, scope_id: uuid::Uuid) -> uuid::Uuid {
    let task_id = uid(seed);
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO tasks (id, target_scope_id, prompt, status, blocked_reason) \
         VALUES (?1, ?2, 'do it', 'blocked', 'clarification')",
        (task_id.to_string(), scope_id.to_string()),
    )
    .expect("insert blocked task");
    tx.commit().expect("commit");
    task_id
}

/// Seed a task in a terminal status directly.
fn seed_terminal_task(
    store: &mut Store,
    seed: u32,
    scope_id: uuid::Uuid,
    status: &str,
) -> uuid::Uuid {
    let task_id = uid(seed);
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO tasks (id, target_scope_id, prompt, status) VALUES (?1, ?2, 'do it', ?3)",
        (task_id.to_string(), scope_id.to_string(), status),
    )
    .expect("insert terminal task");
    tx.commit().expect("commit");
    task_id
}

// create -----------------------------------------------------------------

/// Design §5 step 1: `create` commits `queued`, in its own transaction,
/// before anything talks to a terminal — this test is the direct evidence,
/// reading the row straight back through `show`.
#[test]
fn create_commits_a_queued_row_and_returns_its_id() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");

    let task_id = uid(50);
    let returned = create(
        &mut store,
        task_id,
        None,
        scope_id,
        None,
        None,
        "do the thing",
    )
    .expect("create a queued task");
    assert_eq!(returned, task_id, "create returns the id it was given");

    let task = show(&store, task_id).expect("show the created task");
    assert_eq!(task.status, TaskStatus::Queued);
    assert_eq!(task.prompt, "do the thing");
    assert_eq!(task.sender_scope_id, None, "no sender scope was given");
    assert_eq!(task.target_scope_id, scope_id);
    assert_eq!(task.target_session_id, None);
    assert_eq!(
        task.assigned_session_id, None,
        "create never assigns a session — that is assign's job, not this one's"
    );
    assert_eq!(task.blocked_reason, None);
    assert_eq!(task.cancel_requested_at, None);
}

/// `target_session_id` (what the sender requested) and `assigned_session_id`
/// (what Factory chose) are deliberately distinct columns — `create` records
/// the former from its caller and never touches the latter, because nothing
/// has been assigned yet.
#[test]
fn create_records_a_requested_session_without_assigning_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let requested_session = seed_running_session(&mut store, dir.path(), 2, scope_id);

    let task_id = uid(51);
    create(
        &mut store,
        task_id,
        None,
        scope_id,
        Some(requested_session),
        None,
        "do it",
    )
    .expect("create a task that requests a specific session");

    let task = show(&store, task_id).expect("show");
    assert_eq!(task.target_session_id, Some(requested_session));
    assert_eq!(
        task.assigned_session_id, None,
        "a requested session is not automatically the assigned one"
    );
}

#[test]
fn create_records_the_sender_scope_when_given() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let sender = seed_scope(&mut store, 1, "sender", "/instance/sender");
    let target = seed_scope(&mut store, 2, "target", "/instance/target");

    let task_id = uid(52);
    create(
        &mut store,
        task_id,
        Some(sender),
        target,
        None,
        None,
        "do it",
    )
    .expect("create with a sender scope");

    let task = show(&store, task_id).expect("show");
    assert_eq!(task.sender_scope_id, Some(sender));
    assert_eq!(task.target_scope_id, target);
}

// list / show --------------------------------------------------------------

#[test]
fn list_returns_every_task_oldest_first() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");

    let first = uid(10);
    let second = uid(11);
    create(&mut store, first, None, scope_id, None, None, "first").expect("create first");
    create(&mut store, second, None, scope_id, None, None, "second").expect("create second");

    let tasks = list(&store).expect("list");
    let ids: Vec<uuid::Uuid> = tasks.iter().map(|t| t.id).collect();
    assert_eq!(
        ids,
        vec![first, second],
        "list must return every task, ordered oldest first"
    );
}

#[test]
fn show_of_a_nonexistent_task_is_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path()).expect("open");

    let err = show(&store, uid(999)).expect_err("no such task exists");
    assert!(matches!(err, TaskError::NotFound(id) if id == uid(999)));
}

// cancel ---------------------------------------------------------------

#[test]
fn cancel_a_queued_task_moves_it_straight_to_cancelled() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = uid(20);
    create(&mut store, task_id, None, scope_id, None, None, "do it").expect("create");

    let outcome = cancel(&mut store, task_id).expect("cancel a queued task");
    assert_eq!(outcome, TaskStatus::Cancelled);

    let task = show(&store, task_id).expect("show");
    assert_eq!(task.status, TaskStatus::Cancelled);
    assert_eq!(task.blocked_reason, None);
}

#[test]
fn cancel_a_blocked_task_moves_it_straight_to_cancelled_and_clears_the_reason() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = seed_blocked_task(&mut store, 21, scope_id);

    let outcome = cancel(&mut store, task_id).expect("cancel a blocked task");
    assert_eq!(outcome, TaskStatus::Cancelled);

    let task = show(&store, task_id).expect("show");
    assert_eq!(task.status, TaskStatus::Cancelled);
    assert_eq!(
        task.blocked_reason, None,
        "a lingering blocked_reason on a cancelled task would violate the \
         schema's own blocked_reason CHECK"
    );
}

/// The heart of the cooperative-cancellation rule (design §2.4): cancelling
/// *running* work must not jump straight to `cancelled`. It records the
/// request and leaves the task exactly as it was, `running`, waiting for the
/// agent to notice and report a terminal status.
///
/// Mutation target (task report mutation 4): make `cancel`'s `Running` arm
/// set `status = 'cancelled'` instead of recording the request — this test
/// fails at the `outcome` assertion.
#[test]
fn cancel_a_running_task_records_a_request_and_leaves_status_running() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_running_task(&mut store, 22, scope_id, session_id);

    let outcome = cancel(&mut store, task_id).expect("cancel a running task");
    assert_eq!(
        outcome,
        TaskStatus::Running,
        "cancelling running work is cooperative — it must not jump straight to cancelled"
    );

    let task = show(&store, task_id).expect("show");
    assert_eq!(task.status, TaskStatus::Running);
    assert!(
        task.cancel_requested_at.is_some(),
        "the request must be durably recorded even though status does not change"
    );
}

/// `cancel` on running work must not touch the session at all — neither its
/// state nor its workspace lease — because it is not the same operation as
/// `factory_session::interrupt`, which models a session that *died*. This is
/// the direct, data-visible proof: the session stays `running` and its lease
/// stays open.
#[test]
fn cancel_a_running_task_does_not_touch_the_session_its_state_or_its_lease() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_running_task(&mut store, 22, scope_id, session_id);

    cancel(&mut store, task_id).expect("cancel a running task");

    let state: String = store
        .connection()
        .query_row(
            "SELECT state FROM sessions WHERE id = ?1",
            [session_id.to_string()],
            |row| row.get(0),
        )
        .expect("session row exists");
    assert_eq!(
        state, "running",
        "cancel must not touch the session's state"
    );

    let released_at: Option<String> = store
        .connection()
        .query_row(
            "SELECT released_at FROM workspace_leases WHERE session_id = ?1",
            [session_id.to_string()],
            |row| row.get(0),
        )
        .expect("lease row exists");
    assert!(
        released_at.is_none(),
        "cancel must not release the running session's workspace lease"
    );
}

#[test]
fn cancel_a_terminal_task_is_refused_and_leaves_the_row_untouched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = seed_terminal_task(&mut store, 23, scope_id, "done");

    let err = cancel(&mut store, task_id).expect_err("a terminal task cannot be cancelled");
    assert!(matches!(
        err,
        TaskError::AlreadyTerminal {
            id,
            status: TaskStatus::Done
        } if id == task_id
    ));

    let task = show(&store, task_id).expect("show");
    assert_eq!(
        task.status,
        TaskStatus::Done,
        "a refused cancel must not touch the row"
    );
}

#[test]
fn cancel_of_a_nonexistent_task_is_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let err = cancel(&mut store, uid(999)).expect_err("no such task exists");
    assert!(matches!(err, TaskError::NotFound(id) if id == uid(999)));
}
