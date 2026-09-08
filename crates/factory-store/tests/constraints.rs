//! Coverage for schema constraints not exercised by the other test files:
//! the `blocked_reason` CHECK (both directions plus the NULL case), the
//! `tasks.status` CHECK, the delegation-chain uniqueness that makes design
//! §6's "cannot be targeted again" rule enforceable, that
//! `foreign_keys` enforcement actually reaches DML rather than only reading
//! back as `1` from `PRAGMA foreign_keys` (which `pragma::tests` already
//! covers), and — added by migration 3 (backlog §7) — the
//! `running`-requires-`assigned_session_id` CHECK together with
//! `tasks_one_running_per_session`, which schema.rs's own comment on that
//! index describes as "one mechanism, not two": these tests are what proves
//! it, by exercising the CHECK and the index each on their own and together.

mod common;

use factory_store::Store;

fn assert_check_violation(err: rusqlite::Error) {
    assert!(
        matches!(
            err,
            rusqlite::Error::SqliteFailure(inner, _)
                if inner.code == rusqlite::ErrorCode::ConstraintViolation
        ),
        "expected a CHECK/FOREIGN KEY constraint violation, got {err:?}"
    );
}

#[test]
fn blocked_task_without_a_reason_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let tx = store.transaction().expect("begin");
    common::insert_scope(
        &tx,
        "scope-1",
        "irrlicht",
        "/company/root/projects/irrlicht",
    )
    .expect("insert scope");

    // SQLite's three-valued logic is the trap here: `blocked_reason IN
    // (...)` evaluates to NULL, not FALSE, when `blocked_reason` is NULL,
    // and a CHECK only rejects a row that evaluates to FALSE. Without an
    // explicit `IS NOT NULL` conjunct, `('blocked', NULL)` would pass
    // silently — this test is the guard against that regression.
    let err = tx
        .execute(
            "INSERT INTO tasks (id, target_scope_id, prompt, status, blocked_reason)
             VALUES ('task-1', 'scope-1', 'do it', 'blocked', NULL)",
            [],
        )
        .expect_err("a `blocked` task with no reason must be rejected");
    assert_check_violation(err);
}

#[test]
fn blocked_task_with_a_valid_reason_is_accepted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let tx = store.transaction().expect("begin");
    common::insert_scope(
        &tx,
        "scope-1",
        "irrlicht",
        "/company/root/projects/irrlicht",
    )
    .expect("insert scope");
    tx.execute(
        "INSERT INTO tasks (id, target_scope_id, prompt, status, blocked_reason)
         VALUES ('task-1', 'scope-1', 'do it', 'blocked', 'clarification')",
        [],
    )
    .expect("a `blocked` task with a reason from the vocabulary must be accepted");
    tx.commit().expect("commit");
}

#[test]
fn a_lingering_reason_on_a_non_blocked_task_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let tx = store.transaction().expect("begin");
    common::insert_scope(
        &tx,
        "scope-1",
        "irrlicht",
        "/company/root/projects/irrlicht",
    )
    .expect("insert scope");
    let err = tx
        .execute(
            "INSERT INTO tasks (id, target_scope_id, prompt, status, blocked_reason)
             VALUES ('task-1', 'scope-1', 'do it', 'done', 'clarification')",
            [],
        )
        .expect_err("a reason must not survive on a `done` task");
    assert_check_violation(err);
}

#[test]
fn task_status_outside_the_vocabulary_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let tx = store.transaction().expect("begin");
    common::insert_scope(
        &tx,
        "scope-1",
        "irrlicht",
        "/company/root/projects/irrlicht",
    )
    .expect("insert scope");
    let err = common::insert_task(&tx, "task-1", "scope-1", "do it", "in-progress").expect_err(
        "a status outside queued|running|blocked|done|failed|cancelled must be rejected",
    );
    assert_check_violation(err);
}

#[test]
fn a_scope_cannot_appear_twice_in_one_delegation_chain() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let tx = store.transaction().expect("begin");
    common::insert_scope(&tx, "scope-a", "a", "/a").expect("insert scope a");
    common::insert_scope(&tx, "scope-b", "b", "/b").expect("insert scope b");
    common::insert_task(&tx, "task-1", "scope-b", "do it", "queued").expect("insert task");

    tx.execute(
        "INSERT INTO task_delegation_chain (task_id, position, scope_id) VALUES ('task-1', 0, 'scope-a')",
        [],
    )
    .expect("first hop of the chain succeeds");

    // Design §6: "a scope already in that chain cannot be targeted again."
    // The chain's `UNIQUE(task_id, scope_id)` is what makes that a database
    // constraint rather than an application-level promise.
    let err = tx
        .execute(
            "INSERT INTO task_delegation_chain (task_id, position, scope_id) VALUES ('task-1', 1, 'scope-a')",
            [],
        )
        .expect_err("scope-a must not be insertable twice into the same task's chain");
    assert_check_violation(err);
}

#[test]
fn a_session_cannot_reference_a_nonexistent_scope() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let tx = store.transaction().expect("begin");
    // No scope with this ID has been inserted: `foreign_keys = ON` must make
    // this fail at DML time, not merely read back as `1` from a pragma
    // query. Without it, every FOREIGN KEY in this schema is decorative.
    let err = common::insert_session(
        &tx,
        "session-1",
        "no-such-scope",
        "agent",
        "/workspace",
        "running",
    )
    .expect_err("a session referencing a nonexistent scope must be rejected");
    assert_check_violation(err);
}

// Migration 3 (backlog §7) --------------------------------------------------
//
// `tasks.assigned_session_id`, the `running`-requires-`assigned_session_id`
// CHECK, and `tasks_one_running_per_session` below.

#[test]
fn running_task_without_an_assigned_session_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let tx = store.transaction().expect("begin");
    common::insert_scope(&tx, "scope-1", "irrlicht", "/instance").expect("insert scope");

    // The explicit `IS NOT NULL` in the CHECK is what makes this fail: a
    // bare comparison against NULL evaluates to NULL, not FALSE, and CHECK
    // only rejects a row that evaluates to FALSE (the same trap
    // `blocked_reason`'s CHECK documents above).
    let err = tx
        .execute(
            "INSERT INTO tasks (id, target_scope_id, prompt, status, assigned_session_id) \
             VALUES ('task-1', 'scope-1', 'do it', 'running', NULL)",
            [],
        )
        .expect_err("a `running` task with no assigned session must be rejected");
    assert_check_violation(err);
}

#[test]
fn running_task_with_an_assigned_session_is_accepted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let tx = store.transaction().expect("begin");
    common::insert_scope(&tx, "scope-1", "irrlicht", "/instance").expect("insert scope");
    common::insert_session(&tx, "session-1", "scope-1", "agent", "/instance", "running")
        .expect("insert session");

    tx.execute(
        "INSERT INTO tasks (id, target_scope_id, prompt, status, assigned_session_id) \
         VALUES ('task-1', 'scope-1', 'do it', 'running', 'session-1')",
        [],
    )
    .expect("a `running` task naming its assigned session must be accepted");
    tx.commit().expect("commit");
}

#[test]
fn a_second_running_task_on_the_same_assigned_session_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let tx = store.transaction().expect("begin");
    common::insert_scope(&tx, "scope-1", "irrlicht", "/instance").expect("insert scope");
    common::insert_session(&tx, "session-1", "scope-1", "agent", "/instance", "running")
        .expect("insert session");
    tx.execute(
        "INSERT INTO tasks (id, target_scope_id, prompt, status, assigned_session_id) \
         VALUES ('task-1', 'scope-1', 'do it', 'running', 'session-1')",
        [],
    )
    .expect("the first running task on this session must be accepted");

    // Backlog §7: "each session has at most one running task." This is what
    // `tasks_one_running_per_session` — a partial unique index over
    // `assigned_session_id` `WHERE status = 'running'` — exists to reject.
    let err = tx
        .execute(
            "INSERT INTO tasks (id, target_scope_id, prompt, status, assigned_session_id) \
             VALUES ('task-2', 'scope-1', 'do another thing', 'running', 'session-1')",
            [],
        )
        .expect_err("a second running task on an already-busy session must be rejected");
    assert_check_violation(err);
}

#[test]
fn two_non_running_tasks_may_share_one_assigned_session_id() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let tx = store.transaction().expect("begin");
    common::insert_scope(&tx, "scope-1", "irrlicht", "/instance").expect("insert scope");
    common::insert_session(&tx, "session-1", "scope-1", "agent", "/instance", "running")
        .expect("insert session");

    // `tasks_one_running_per_session` is a *partial* index, `WHERE status =
    // 'running'` — pinning that partiality, not just that some unique index
    // exists on `assigned_session_id`. Two `done` tasks recording the same
    // historical assignment must both be insertable; a plain (non-partial)
    // unique index would wrongly reject the second one and this test would
    // still pass if the `WHERE` clause were dropped, so it is the one
    // dropping that specific clause cannot get past.
    tx.execute(
        "INSERT INTO tasks (id, target_scope_id, prompt, status, assigned_session_id) \
         VALUES ('task-1', 'scope-1', 'do it', 'done', 'session-1')",
        [],
    )
    .expect("a finished task may still record its former assigned session");
    tx.execute(
        "INSERT INTO tasks (id, target_scope_id, prompt, status, assigned_session_id) \
         VALUES ('task-2', 'scope-1', 'do another thing', 'done', 'session-1')",
        [],
    )
    .expect("a second, unrelated finished task may record the same former session");
    tx.commit().expect("commit");
}
