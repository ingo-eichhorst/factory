//! Backlog §6's interrupted-handling rule: when a temporary agent's session
//! dies while its task is not in a terminal state, `interrupt` releases the
//! lease and blocks the task as `interrupted`, and Factory never
//! automatically redelivers.
//!
//! [`the_schema_rejects_deleting_a_session_that_a_lease_still_references`] is
//! the measured evidence, against the real schema through this crate's own
//! `begin_start`, for why `interrupt` moves the session to `Failed` rather
//! than issuing `DELETE FROM sessions` — see the module docs' "why 'the
//! session record is removed' is `Failed`" section.

mod common;

use factory_session::{SessionError, begin_start, interrupt, mark_disconnected, mark_running};
use factory_store::Store;

/// `interrupt` moves a `running` session straight to `failed` and releases
/// its lease — proved the same way the rest of this crate's tests prove a
/// release: a fresh `begin_start` on the same workspace, previously blocked,
/// now succeeds.
///
/// Mutation: change `interrupt`'s target from `SessionState::Failed` to
/// `SessionState::Disconnected` (still a legal `Running` target, so this
/// compiles and the transition succeeds) — measured (see the task report):
/// this test fails at `assert_eq!(session_state, "failed")` with
/// `left: "disconnected", right: "failed"`, and two other tests in this file
/// fail alongside it (`interrupt_does_not_touch_an_already_terminal_task`,
/// `interrupt_also_accepts_a_session_already_disconnected`) because they
/// assert the same post-interrupt state.
#[test]
fn interrupt_moves_a_running_session_directly_to_failed_and_releases_its_lease() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir(&workspace_dir).expect("create");
    let workspace = common::resolve(&workspace_dir);
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());
    let session = common::uid(1);
    begin_start(&mut store, session, scope_id, "agent", 10, &workspace).expect("start");
    mark_running(&mut store, session).expect("readiness observed");

    let task = common::seed_task(&mut store, 1, scope_id, "running", None, Some(session));

    interrupt(&mut store, session, task).expect("interrupt a running, non-terminal session");

    assert_eq!(common::session_state(&store, session), "failed");
    assert!(
        !common::lease_is_open(&store, session),
        "interrupt must release the lease"
    );
    begin_start(
        &mut store,
        common::uid(2),
        scope_id,
        "agent",
        10,
        &workspace,
    )
    .expect("the workspace must be free once the interrupted session's lease is released");
}

/// The same call also blocks the task as `interrupted` — checked
/// independently of the session-side effect above.
///
/// Mutation: change `'interrupted'` to some other value (or drop the
/// `UPDATE tasks` statement) — this test starts failing. See the task report.
#[test]
fn interrupt_blocks_the_task_as_interrupted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir(&workspace_dir).expect("create");
    let workspace = common::resolve(&workspace_dir);
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());
    let session = common::uid(1);
    begin_start(&mut store, session, scope_id, "agent", 10, &workspace).expect("start");
    mark_running(&mut store, session).expect("readiness observed");
    let task = common::seed_task(&mut store, 1, scope_id, "running", None, Some(session));

    interrupt(&mut store, session, task).expect("interrupt");

    assert_eq!(
        common::task_row(&store, task),
        ("blocked".to_string(), Some("interrupted".to_string()))
    );
}

/// Backlog §6's sharpest rule, from the task side: a task already `blocked`
/// for another reason is *not* exempt from being rewritten to
/// `blocked: interrupted`. If it were exempt, that would mean `blocked` was
/// being treated as terminal — the exact mistake backlog §6 warns "reads like
/// an ending."
///
/// Mutation: add `'blocked'` to `interrupt`'s
/// `WHERE status NOT IN (...)` list — this test starts failing, because the
/// task's `blocked_reason` then stays `clarification`. See the task report.
#[test]
fn interrupt_overwrites_an_existing_blocked_reason_because_blocked_is_not_terminal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir(&workspace_dir).expect("create");
    let workspace = common::resolve(&workspace_dir);
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());
    let session = common::uid(1);
    begin_start(&mut store, session, scope_id, "agent", 10, &workspace).expect("start");
    mark_running(&mut store, session).expect("readiness observed");
    // The task is already blocked, awaiting clarification — a non-terminal,
    // in-flight state, per design §2.4.
    let task = common::seed_task(
        &mut store,
        1,
        scope_id,
        "blocked",
        Some("clarification"),
        Some(session),
    );

    interrupt(&mut store, session, task)
        .expect("a session dying mid-clarification is still interrupted");

    assert_eq!(
        common::task_row(&store, task),
        ("blocked".to_string(), Some("interrupted".to_string())),
        "an existing `clarification` reason must be overwritten by `interrupted`, \
         proving `blocked` was treated as non-terminal, not skipped"
    );
}

/// The complementary guard: a task that has already reached a terminal
/// status before its session's death is confirmed must never be rewritten.
/// The session-side cleanup (lease release) still happens regardless — a
/// dead session's resources are always reclaimed — but the finished task's
/// own record is left alone.
#[test]
fn interrupt_does_not_touch_an_already_terminal_task() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir(&workspace_dir).expect("create");
    let workspace = common::resolve(&workspace_dir);
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());
    let session = common::uid(1);
    begin_start(&mut store, session, scope_id, "agent", 10, &workspace).expect("start");
    mark_running(&mut store, session).expect("readiness observed");
    // An unusual race, but a real one: the task already finished, and the
    // session's death is only confirmed afterward.
    let task = common::seed_task(&mut store, 1, scope_id, "done", None, Some(session));

    interrupt(&mut store, session, task).expect("the session side still needs cleanup");

    assert_eq!(
        common::task_row(&store, task),
        ("done".to_string(), None),
        "a task that already finished must never be rewritten to blocked:interrupted"
    );
    assert_eq!(
        common::session_state(&store, session),
        "failed",
        "the dead session is still reclaimed regardless of the task's own status"
    );
}

/// `interrupt` accepts a session that was already `disconnected` when its
/// death is confirmed (not only one still recorded `running`) — the same
/// `Failed` destination either way.
#[test]
fn interrupt_also_accepts_a_session_already_disconnected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir(&workspace_dir).expect("create");
    let workspace = common::resolve(&workspace_dir);
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());
    let session = common::uid(1);
    begin_start(&mut store, session, scope_id, "agent", 10, &workspace).expect("start");
    mark_running(&mut store, session).expect("readiness observed");
    mark_disconnected(&mut store, session).expect("Factory loses sight of the session");
    let task = common::seed_task(&mut store, 1, scope_id, "running", None, Some(session));

    interrupt(&mut store, session, task).expect("interrupt from disconnected");

    assert_eq!(common::session_state(&store, session), "failed");
    assert_eq!(
        common::task_row(&store, task).0,
        "blocked",
        "the task must still be blocked as interrupted"
    );
}

/// Defensive: calling `interrupt` on a session that has no lease to release
/// (already `stopped`) is rejected, not silently accepted — a caller
/// interrupting an already-cleaned-up session is a bug worth surfacing.
#[test]
fn interrupt_on_an_already_stopped_session_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir(&workspace_dir).expect("create");
    let workspace = common::resolve(&workspace_dir);
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());
    let session = common::uid(1);
    begin_start(&mut store, session, scope_id, "agent", 10, &workspace).expect("start");
    mark_running(&mut store, session).expect("readiness observed");
    factory_session::stop(
        &mut store,
        session,
        "already stopped for an unrelated reason",
    )
    .expect("stop");
    let task = common::seed_task(&mut store, 1, scope_id, "running", None, Some(session));

    let err = interrupt(&mut store, session, task)
        .expect_err("nothing left to interrupt on an already-stopped session");
    assert!(matches!(err, SessionError::InvalidTransition { .. }));
}

/// A `task_id` that does not exist is rejected before either write, not
/// silently ignored: this is the same defensive shape as
/// `on_task_terminal`'s `TaskNotFound`, and here it also protects the
/// session — an unchecked `UPDATE tasks ... WHERE id = ?` matching zero rows
/// would otherwise let the call return `Ok` while still releasing the
/// session's lease on the strength of a task id that names nothing.
#[test]
fn interrupt_rejects_a_nonexistent_task_and_leaves_the_session_untouched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir(&workspace_dir).expect("create");
    let workspace = common::resolve(&workspace_dir);
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());
    let session = common::uid(1);
    begin_start(&mut store, session, scope_id, "agent", 10, &workspace).expect("start");
    mark_running(&mut store, session).expect("readiness observed");

    let nonexistent_task = common::uid(99);
    let err =
        interrupt(&mut store, session, nonexistent_task).expect_err("no task exists with this id");
    assert!(matches!(
        &err,
        SessionError::TaskNotFound(id) if *id == nonexistent_task
    ));

    assert_eq!(
        common::session_state(&store, session),
        "running",
        "a rejected call must not touch the session either"
    );
    assert!(common::lease_is_open(&store, session));
}

/// Pins "Factory never automatically retries" (module docs, `interrupt`'s
/// doc comment) by assertion rather than mutation: there is no retry call to
/// delete and watch a test fail, so this instead checks the two durable facts
/// that a hypothetical auto-retry would falsify. `delivery_attempts` exists
/// specifically to journal each delivery attempt (`schema.rs`), so "zero new
/// rows for this task" is what "no redelivery" means, made checkable.
#[test]
fn interrupt_leaves_delivery_attempts_untouched_and_creates_no_replacement_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir(&workspace_dir).expect("create");
    let workspace = common::resolve(&workspace_dir);
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());
    let session = common::uid(1);
    begin_start(&mut store, session, scope_id, "agent", 10, &workspace).expect("start");
    mark_running(&mut store, session).expect("readiness observed");
    let task = common::seed_task(&mut store, 1, scope_id, "running", None, Some(session));

    // Record the one delivery that already happened, the way Slice 7 would
    // have before this task ever reached `running` — so "no new row" is a
    // meaningful before/after, not just "always zero."
    store
        .connection()
        .execute(
            "INSERT INTO delivery_attempts (task_id, session_id, outcome) VALUES (?1, ?2, NULL)",
            (task.to_string(), session.to_string()),
        )
        .expect("record the original delivery attempt");
    let before = common::delivery_attempt_count(&store, task);
    assert_eq!(before, 1);

    interrupt(&mut store, session, task).expect("interrupt");

    assert_eq!(
        common::delivery_attempt_count(&store, task),
        before,
        "interrupt must journal no new delivery attempt — it never redelivers"
    );
    assert_eq!(
        common::session_count_for_agent(&store, scope_id, "agent"),
        1,
        "interrupt must not create a replacement session for this agent"
    );
}

/// Measured, not assumed evidence for why `interrupt` moves the session to
/// `Failed` instead of `DELETE FROM sessions WHERE id = ?`: `begin_start`
/// always leaves a `workspace_leases` row referencing the session (NOT NULL,
/// no `ON DELETE` action), and ADR 0012 decision 3 sets
/// `PRAGMA foreign_keys = ON` on every connection, so the database itself
/// refuses the delete. Run directly against the real schema and this crate's
/// own `begin_start`, not a hand-retyped schema fragment that could drift
/// from the real one.
///
/// This is not a bug the foreign key is protecting against by accident: see
/// the module docs' rejected alternative (deleting the `workspace_leases` row
/// first) for why that path is refused on audit-trail grounds even though it
/// is mechanically possible.
#[test]
fn the_schema_rejects_deleting_a_session_that_a_lease_still_references() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir(&workspace_dir).expect("create");
    let workspace = common::resolve(&workspace_dir);
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());
    let session = common::uid(1);
    begin_start(&mut store, session, scope_id, "agent", 10, &workspace).expect("start");

    let err = store
        .connection()
        .execute("DELETE FROM sessions WHERE id = ?1", [session.to_string()])
        .expect_err(
            "workspace_leases.session_id still references this session; the foreign key \
             must reject the delete rather than silently orphan the lease journal",
        );
    assert!(
        err.to_string().contains("FOREIGN KEY constraint failed"),
        "expected a foreign key violation, got: {err}"
    );
}
