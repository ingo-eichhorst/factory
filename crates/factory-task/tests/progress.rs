//! `factory_task::events::record_progress`'s own test suite: backlog §11's
//! "progress" verb.
//!
//! Every fixture below is duplicated from `tests/create.rs` rather than
//! shared through a `tests/common/mod.rs` — the same convention every other
//! test file in this crate already states for itself.

use factory_store::Store;
use factory_task::create::{create, show};
use factory_task::events::{EventType, ProgressError, for_task, record_progress};
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

#[test]
fn record_progress_writes_one_progress_event_with_the_note() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = seed_task(&mut store, 2, scope_id);

    record_progress(&mut store, task_id, None, "halfway through the migration").expect("record");

    let events = for_task(&store, task_id).expect("read events");
    assert_eq!(events.len(), 2, "created, then progress");
    assert_eq!(events[1].event_type, EventType::Progress);
    assert_eq!(events[1].author_session_id, None);
    let payload: serde_json::Value =
        serde_json::from_str(events[1].payload.as_deref().expect("payload present"))
            .expect("valid JSON");
    assert_eq!(payload["note"], "halfway through the migration");
}

/// A progress note is an annotation, not a transition — the same stance
/// `decisions::record` and `verify::record_verdict` both take toward their
/// own tables.
#[test]
fn record_progress_changes_no_column_of_tasks() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = seed_task(&mut store, 2, scope_id);

    let before = show(&store, task_id).expect("show before");
    record_progress(&mut store, task_id, None, "still working").expect("record");
    let after = show(&store, task_id).expect("show after");

    assert_eq!(
        before, after,
        "a progress note must touch no column of tasks"
    );
    assert_eq!(after.status, TaskStatus::Queued);
}

/// An empty note is refused — a progress note with no text is a log line,
/// not a note, the same reasoning `decisions::record` gives for an empty
/// rationale.
#[test]
fn an_empty_note_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = seed_task(&mut store, 2, scope_id);

    let err = record_progress(&mut store, task_id, None, "").expect_err("empty note refused");
    assert!(matches!(err, ProgressError::EmptyNote(id) if id == task_id));

    let events = for_task(&store, task_id).expect("read events");
    assert_eq!(
        events.len(),
        1,
        "only `created` — the refused note wrote nothing"
    );
}

/// A nonexistent task is refused by `task_events.task_id REFERENCES tasks
/// (id)` — the same backstop shape `tests/decisions.rs`'s own
/// `record_against_a_nonexistent_task_is_refused_by_the_foreign_key` proves
/// for `task_decisions`.
#[test]
fn record_against_a_nonexistent_task_is_refused_by_the_foreign_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let err = record_progress(&mut store, uid(999), None, "working on it")
        .expect_err("no such task exists");
    assert!(
        matches!(err, ProgressError::Task(TaskError::Store(_))),
        "expected the schema's own foreign key to reject this, got {err:?}"
    );
}

/// A progress note can be authored by a session — `task_events.author_session_id`
/// round-trips it, the same as every other event this crate writes.
#[test]
fn a_progress_note_can_be_authored_by_a_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = seed_task(&mut store, 2, scope_id);
    let session_id = uid(4);
    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir(&workspace_dir).expect("create workspace directory");
    let workspace = factory_paths::CanonicalPath::resolve(&workspace_dir).expect("resolve");
    factory_session::begin_start(&mut store, session_id, scope_id, "agent", 10, &workspace)
        .expect("start session");

    record_progress(&mut store, task_id, Some(session_id), "on track").expect("record");

    let events = for_task(&store, task_id).expect("read events");
    assert_eq!(events[1].author_session_id, Some(session_id));
}
