//! `factory_task::create::create_rework`'s own test suite: §12.2's hook — "a
//! run can reference the run it reworks together with the finding that
//! caused it, without modifying the referenced run."
//!
//! Every fixture below is duplicated from `tests/create.rs` rather than
//! shared through a `tests/common/mod.rs` — the same convention every other
//! test file in this crate already states for itself.

use factory_store::Store;
use factory_task::create::{create, create_rework, show};
use factory_task::events::{EventType, for_task};
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

fn seed_task(store: &mut Store, seed: u32, scope_id: uuid::Uuid) -> uuid::Uuid {
    create(store, uid(seed), None, scope_id, None, None, "do it", &[]).expect("create")
}

/// Move a task straight to `done` with raw SQL — this test file does not
/// need `factory_task::complete`'s own validation, only a terminal row to
/// rework against.
fn force_done(store: &mut Store, task_id: uuid::Uuid) {
    let tx = store.transaction().expect("begin");
    tx.execute(
        "UPDATE tasks SET status = 'done' WHERE id = ?1",
        [task_id.to_string()],
    )
    .expect("force done");
    tx.commit().expect("commit");
}

/// The ordinary case: a run reworking an already-`done` run records the link
/// on itself and writes a `rework` event against its own id.
#[test]
fn a_rework_names_the_finished_run_and_records_a_rework_event_on_itself() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let old_task = seed_task(&mut store, 2, scope_id);
    force_done(&mut store, old_task);

    let new_task = uid(3);
    create_rework(
        &mut store,
        new_task,
        None,
        scope_id,
        None,
        None,
        "redo it",
        &[],
        old_task,
        "missed the edge case",
    )
    .expect("rework a finished run");

    let task = show(&store, new_task).expect("show new task");
    assert_eq!(task.reworks_task_id, Some(old_task));
    assert_eq!(task.rework_finding.as_deref(), Some("missed the edge case"));

    // The event lands on the *new* run, never the old one.
    let new_events = for_task(&store, new_task).expect("read new events");
    assert_eq!(new_events.len(), 2, "created, then rework");
    assert_eq!(new_events[1].event_type, EventType::Rework);
    let payload: serde_json::Value =
        serde_json::from_str(new_events[1].payload.as_deref().expect("payload present"))
            .expect("valid JSON");
    assert_eq!(payload["reworks_task_id"], old_task.to_string());
    assert_eq!(payload["finding"], "missed the edge case");
}

/// The criterion's own words, tested hardest: the referenced run's entire
/// `tasks` row and its entire event list are byte-for-byte identical before
/// and after a rework names it. `before == after` on the whole `Task` struct
/// covers every column, not a hand-picked subset — the same shape
/// `verify.rs`'s own `a_verdict_changes_no_column_of_tasks` uses for the
/// identical claim about a different table.
#[test]
fn the_referenced_run_is_untouched_by_a_rework() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let old_task = seed_task(&mut store, 2, scope_id);
    force_done(&mut store, old_task);

    let task_before = show(&store, old_task).expect("show before");
    let events_before = for_task(&store, old_task).expect("events before");

    create_rework(
        &mut store,
        uid(3),
        None,
        scope_id,
        None,
        None,
        "redo it",
        &[],
        old_task,
        "missed the edge case",
    )
    .expect("rework");

    let task_after = show(&store, old_task).expect("show after");
    let events_after = for_task(&store, old_task).expect("events after");

    assert_eq!(
        task_before, task_after,
        "the referenced run's own row must not change"
    );
    assert_eq!(
        events_before, events_after,
        "the referenced run's own event list must not gain the rework event"
    );
}

/// A run may not rework a run that has not finished — §12.2's own analogy is
/// a finding carried back from an inspection, and there is no finding until
/// the referenced run has stopped moving.
#[test]
fn reworking_a_queued_run_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let queued_task = seed_task(&mut store, 2, scope_id);

    let err = create_rework(
        &mut store,
        uid(3),
        None,
        scope_id,
        None,
        None,
        "redo it",
        &[],
        queued_task,
        "too early",
    )
    .expect_err("a queued run has not finished");
    assert!(matches!(
        err,
        TaskError::ReworkTargetNotTerminal { id, status: TaskStatus::Queued } if id == queued_task
    ));
}

/// `blocked` reads like an ending in §12.2's own prose but
/// `TaskStatus::is_terminal` deliberately excludes it — a blocked run can
/// still return to `queued` and finish, so reworking it would leave two live
/// attempts on the same piece of work. See `create_rework`'s own doc comment
/// for the full reasoning; this is the direct test of it.
#[test]
fn reworking_a_blocked_run_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let blocked_task = seed_task(&mut store, 2, scope_id);
    {
        let tx = store.transaction().expect("begin");
        tx.execute(
            "UPDATE tasks SET status = 'blocked', blocked_reason = 'clarification' WHERE id = ?1",
            [blocked_task.to_string()],
        )
        .expect("force blocked");
        tx.commit().expect("commit");
    }

    let err = create_rework(
        &mut store,
        uid(3),
        None,
        scope_id,
        None,
        None,
        "redo it",
        &[],
        blocked_task,
        "too early",
    )
    .expect_err("a blocked run has not finished, even though it reads like an ending");
    assert!(matches!(
        err,
        TaskError::ReworkTargetNotTerminal { id, status: TaskStatus::Blocked } if id == blocked_task
    ));
}

/// A run cannot rework itself. `reworks_task_id TEXT REFERENCES tasks (id)`
/// does not catch this on its own — SQLite's foreign-key check runs after
/// the statement, by which point the self-referencing row already exists —
/// so this is a pure Rust guard, asserted directly rather than by whatever
/// error a foreign-key failure or a not-found read would otherwise produce.
#[test]
fn a_run_cannot_rework_itself() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let id = uid(2);

    let err = create_rework(
        &mut store,
        id,
        None,
        scope_id,
        None,
        None,
        "redo it",
        &[],
        id,
        "self-reference",
    )
    .expect_err("self-rework is refused");
    assert!(matches!(err, TaskError::SelfRework(i) if i == id));

    // Nothing was written at all — not even the row itself.
    assert!(matches!(show(&store, id), Err(TaskError::NotFound(i)) if i == id));
}

/// A rework naming no finding is refused — design §11 / §12.2: the finding
/// is what makes the link an audit trail rather than an unexplained
/// reference. Mirrors `decisions::record`'s own `NoRationale` guard.
#[test]
fn a_rework_with_an_empty_finding_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let old_task = seed_task(&mut store, 2, scope_id);
    force_done(&mut store, old_task);

    let new_task = uid(3);
    let err = create_rework(
        &mut store,
        new_task,
        None,
        scope_id,
        None,
        None,
        "redo it",
        &[],
        old_task,
        "",
    )
    .expect_err("an empty finding is refused");
    assert!(matches!(err, TaskError::ReworkFindingRequired(id) if id == new_task));

    assert!(matches!(
        show(&store, new_task),
        Err(TaskError::NotFound(id)) if id == new_task
    ));
}

/// A rework naming a task id that does not exist at all is refused as
/// `NotFound`, distinct from `ReworkTargetNotTerminal`.
#[test]
fn reworking_a_nonexistent_task_is_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");

    let err = create_rework(
        &mut store,
        uid(2),
        None,
        scope_id,
        None,
        None,
        "redo it",
        &[],
        uid(999),
        "a finding",
    )
    .expect_err("no such task to rework");
    assert!(matches!(err, TaskError::NotFound(id) if id == uid(999)));
}

/// Chains are ordinary provenance, not a defect: C may rework B, which
/// reworked A. Each link is recorded independently and each referenced run
/// stays untouched by the run downstream of it.
#[test]
fn a_chain_of_reworks_is_allowed_and_each_link_leaves_its_predecessor_untouched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");

    let task_a = seed_task(&mut store, 2, scope_id);
    force_done(&mut store, task_a);

    let task_b = uid(3);
    create_rework(
        &mut store,
        task_b,
        None,
        scope_id,
        None,
        None,
        "redo a",
        &[],
        task_a,
        "a's finding",
    )
    .expect("b reworks a");
    force_done(&mut store, task_b);
    let task_a_after_b = show(&store, task_a).expect("show a");

    let task_c = uid(4);
    create_rework(
        &mut store,
        task_c,
        None,
        scope_id,
        None,
        None,
        "redo b",
        &[],
        task_b,
        "b's finding",
    )
    .expect("c reworks b");

    let task_a_after_c = show(&store, task_a).expect("show a again");
    assert_eq!(
        task_a_after_b, task_a_after_c,
        "a's own row must not move again once b is reworked in turn"
    );

    let task_c_shown = show(&store, task_c).expect("show c");
    assert_eq!(task_c_shown.reworks_task_id, Some(task_b));
    let task_b_shown = show(&store, task_b).expect("show b");
    assert_eq!(task_b_shown.reworks_task_id, Some(task_a));
}
