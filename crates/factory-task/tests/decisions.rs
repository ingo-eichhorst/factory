//! `factory_task::decisions`'s own test suite: structured decisions
//! (`task_decisions`; design §11, "strukturiert abrufbar").
//!
//! Every fixture below is duplicated from `tests/create.rs` rather than
//! shared through a `tests/common/mod.rs` — the same convention every other
//! test file in this crate already states for itself.

use factory_store::Store;
use factory_task::create::create;
use factory_task::decisions::{Decision, DecisionError, for_task, record};
use factory_task::events;
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
fn record_and_read_back_round_trips_every_field() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = seed_task(&mut store, 2, scope_id);
    let decision_id = uid(3);

    record(
        &mut store,
        decision_id,
        task_id,
        None,
        "use approach B",
        "approach A needed a schema change we don't own",
        Some("approach A; approach C"),
        Some("slower, but no cross-crate coordination"),
    )
    .expect("record");

    let decisions = for_task(&store, task_id).expect("read decisions");
    assert_eq!(decisions.len(), 1);
    assert_eq!(
        decisions[0],
        Decision {
            id: decision_id,
            task_id,
            author_session_id: None,
            decision: "use approach B".to_string(),
            rationale: "approach A needed a schema change we don't own".to_string(),
            alternatives: Some("approach A; approach C".to_string()),
            consequences: Some("slower, but no cross-crate coordination".to_string()),
            created_at: decisions[0].created_at.clone(),
        }
    );
}

/// Design §11's own reason this table exists at all: "a decision without a
/// rationale is a log line, not a decision."
#[test]
fn record_with_an_empty_rationale_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = seed_task(&mut store, 2, scope_id);

    let err = record(
        &mut store,
        uid(3),
        task_id,
        None,
        "use approach B",
        "",
        None,
        None,
    )
    .expect_err("an empty rationale is refused");
    assert!(matches!(err, DecisionError::NoRationale(id) if id == task_id));

    let decisions = for_task(&store, task_id).expect("read decisions");
    assert_eq!(decisions, vec![], "a refused record must write nothing");
}

/// `alternatives` and `consequences` are genuinely optional — `None` round
/// trips as `None`, not as an empty string.
#[test]
fn alternatives_and_consequences_are_optional_and_round_trip_as_none() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = seed_task(&mut store, 2, scope_id);

    record(
        &mut store,
        uid(3),
        task_id,
        None,
        "use approach B",
        "it was simpler",
        None,
        None,
    )
    .expect("record");

    let decisions = for_task(&store, task_id).expect("read decisions");
    assert_eq!(decisions[0].alternatives, None);
    assert_eq!(decisions[0].consequences, None);
}

/// A decision authored by a session round-trips its `author_session_id` —
/// `task_decisions.author_session_id` is nullable for the identical reason
/// `task_events.author_session_id` is: `None` means a human.
#[test]
fn a_decision_can_be_authored_by_a_session() {
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

    record(
        &mut store,
        uid(5),
        task_id,
        Some(session_id),
        "use approach B",
        "it was simpler",
        None,
        None,
    )
    .expect("record");

    let decisions = for_task(&store, task_id).expect("read decisions");
    assert_eq!(decisions[0].author_session_id, Some(session_id));
}

/// Two decisions on one task read back oldest first.
#[test]
fn two_decisions_on_one_task_read_back_oldest_first() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = seed_task(&mut store, 2, scope_id);

    record(
        &mut store,
        uid(3),
        task_id,
        None,
        "first decision",
        "rationale one",
        None,
        None,
    )
    .expect("record first");
    record(
        &mut store,
        uid(4),
        task_id,
        None,
        "second decision",
        "rationale two",
        None,
        None,
    )
    .expect("record second");

    let decisions = for_task(&store, task_id).expect("read decisions");
    assert_eq!(decisions.len(), 2);
    assert_eq!(decisions[0].decision, "first decision");
    assert_eq!(decisions[1].decision, "second decision");
}

/// A decision naming a nonexistent task is refused by
/// `task_decisions.task_id REFERENCES tasks (id)` — the same backstop shape
/// `tests/create.rs::a_failed_chain_insert_leaves_no_task_row_at_all` already
/// proves for `task_delegation_chain`.
#[test]
fn record_against_a_nonexistent_task_is_refused_by_the_foreign_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let err = record(
        &mut store,
        uid(3),
        uid(999),
        None,
        "use approach B",
        "rationale",
        None,
        None,
    )
    .expect_err("no such task exists");
    assert!(
        matches!(err, DecisionError::Task(TaskError::Store(_))),
        "expected the schema's own foreign key to reject this, got {err:?}"
    );
}

/// `task_decisions` is a second table, not a second name for `task_events` —
/// design §11's own reason for splitting them. Recording a decision must
/// write no `task_events` row at all.
#[test]
fn recording_a_decision_writes_no_task_event() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = seed_task(&mut store, 2, scope_id);
    let events_before = events::for_task(&store, task_id)
        .expect("read events")
        .len();

    record(
        &mut store,
        uid(3),
        task_id,
        None,
        "use approach B",
        "rationale",
        None,
        None,
    )
    .expect("record");

    let events_after = events::for_task(&store, task_id)
        .expect("read events")
        .len();
    assert_eq!(
        events_before, events_after,
        "a decision is not an event — the two tables are independent"
    );

    // And the task row itself is untouched — a decision annotates, it does
    // not transition, the same stance `verify::record_verdict` takes.
    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(task.status, TaskStatus::Queued);
}
