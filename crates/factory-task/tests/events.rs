//! `factory_task::events`'s own test suite: the append-only audit log
//! (design §11's `task_events`; ADR 0021 decision 3, and `crate`'s own
//! decision 3, which this module implements).
//!
//! Each wired transition's own event — its type, payload, and same-
//! transaction placement — is proven where that transition lives
//! (`tests/create.rs`, `tests/assign.rs`, `tests/deliver.rs`,
//! `tests/complete.rs`). This file is the cross-cutting proof: a whole
//! lifecycle's events read back in the right order, and one task's events
//! never leak into another's, even when their ids interleave.
//!
//! Every fixture below is duplicated from `tests/create.rs` rather than
//! shared through a `tests/common/mod.rs` — the same convention every other
//! test file in this crate already states for itself.

use factory_paths::CanonicalPath;
use factory_store::Store;
use factory_task::assign::assign;
use factory_task::create::create;
use factory_task::deliver::{PromptWriteError, PromptWriter, deliver, mark_running};
use factory_task::events::{EventType, for_task};

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

/// Start and mark `running` a fresh session in a fresh workspace under
/// `dir` — mirrors `tests/deliver.rs::seed_running_session`.
fn seed_idle_session(
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

/// A writer that always succeeds — mirrors `tests/deliver.rs::RecordingWriter`,
/// minus the recording (nothing here reads its calls back).
struct AcceptingWriter;

impl PromptWriter for AcceptingWriter {
    fn write_prompt(
        &mut self,
        _session_id: uuid::Uuid,
        _prompt: &str,
    ) -> Result<(), PromptWriteError> {
        Ok(())
    }
}

/// The full lifecycle, through the ordinary public API end to end: create,
/// assign, deliver, observe running, complete. `events::for_task` must read
/// back exactly the five events this sequence writes, in the exact order
/// they were written — [`Event::id`] ascending, per the module's own doc
/// comment on why `id`, not `created_at`, is the ordering.
#[test]
fn a_full_lifecycle_writes_the_matching_events_in_append_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_idle_session(&mut store, dir.path(), 2, scope_id);

    let task_id = uid(50);
    create(
        &mut store,
        task_id,
        None,
        scope_id,
        None,
        None,
        "do it",
        &[],
    )
    .expect("create");
    assign(&mut store, task_id, "agent", 10).expect("assign");
    let mut writer = AcceptingWriter;
    deliver(&mut store, task_id, &mut writer).expect("deliver");
    mark_running(&mut store, task_id).expect("mark running");
    factory_task::complete::done(&mut store, task_id, Some("all done"), None).expect("done");

    let events = for_task(&store, task_id).expect("read events");
    let types: Vec<EventType> = events.iter().map(|e| e.event_type).collect();
    assert_eq!(
        types,
        vec![
            EventType::Created,
            EventType::Assigned,
            EventType::Delivered,
            EventType::Running,
            EventType::Done,
        ]
    );

    let ids: Vec<i64> = events.iter().map(|e| e.id).collect();
    let mut sorted_ids = ids.clone();
    sorted_ids.sort_unstable();
    assert_eq!(ids, sorted_ids, "already in append order");
    assert_eq!(
        ids.len(),
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        "no id is repeated"
    );

    for event in &events {
        assert_eq!(event.task_id, task_id);
    }

    // The one idle session in this fixture is also the one `assign` and
    // `mark_running` each name in their own payload — the direct evidence
    // that both wrote about the same session `assign` actually chose.
    for event in [&events[1], &events[3]] {
        let payload: serde_json::Value =
            serde_json::from_str(event.payload.as_deref().expect("payload present"))
                .expect("valid JSON");
        assert_eq!(payload["session_id"], session_id.to_string());
    }
}

/// Two tasks' events never leak into each other, even when their ids
/// interleave — the direct proof that `for_task`'s `WHERE task_id = ?1`
/// clause, not merely the order the events happened to be written in, is
/// what keeps them apart. Sequence: create(a), create(b), cancel(a),
/// cancel(b) — so task `a`'s events are ids 1 and 3, task `b`'s are 2 and 4.
#[test]
fn events_are_scoped_to_their_own_task_even_when_ids_interleave() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");

    let task_a = uid(50);
    create(&mut store, task_a, None, scope_id, None, None, "a", &[]).expect("create a");
    let task_b = uid(51);
    create(&mut store, task_b, None, scope_id, None, None, "b", &[]).expect("create b");

    factory_task::create::cancel(&mut store, task_a).expect("cancel a");
    factory_task::create::cancel(&mut store, task_b).expect("cancel b");

    let events_a = for_task(&store, task_a).expect("read a's events");
    let events_b = for_task(&store, task_b).expect("read b's events");

    assert_eq!(
        events_a.len(),
        2,
        "a's own created and cancelled, nothing of b's"
    );
    assert_eq!(
        events_b.len(),
        2,
        "b's own created and cancelled, nothing of a's"
    );

    for event in events_a.iter().chain(events_b.iter()) {
        assert!(
            event.task_id == task_a || event.task_id == task_b,
            "every returned event must belong to the task it was read for"
        );
    }
    assert!(events_a.iter().all(|e| e.task_id == task_a));
    assert!(events_b.iter().all(|e| e.task_id == task_b));

    // The interleaving itself: a's ids are the odd ones out (1, 3), b's are
    // the even ones (2, 4) — this is what proves the filter, not the write
    // order, is doing the separating.
    assert_eq!(events_a[0].id, 1);
    assert_eq!(events_a[1].id, 3);
    assert_eq!(events_b[0].id, 2);
    assert_eq!(events_b[1].id, 4);
}

/// A task with no events at all reads back as an empty list, not an error —
/// this task was seeded directly by raw SQL, never through `create`.
#[test]
fn for_task_of_a_task_with_no_events_is_an_empty_list() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = uid(50);
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO tasks (id, target_scope_id, prompt, status) VALUES (?1, ?2, 'do it', 'queued')",
        (task_id.to_string(), scope_id.to_string()),
    )
    .expect("insert task");
    tx.commit().expect("commit");

    let events = for_task(&store, task_id).expect("read events");
    assert_eq!(events, vec![]);
}
