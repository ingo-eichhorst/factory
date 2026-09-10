//! `factory_task::complete`'s own test suite: endings, and the two statuses
//! that are not endings (design §2.4, §5; backlog §7).
//!
//! See `tests/deliver.rs`'s module docs for why the fixtures below seed rows
//! directly with raw SQL rather than going through `assign` or `deliver` —
//! same reasoning, same convention as `tests/create.rs`.

use factory_paths::CanonicalPath;
use factory_store::Store;
use factory_task::complete::{
    CompleteError, acknowledge_cancellation, blocked, blocked_from_observation, done, fail,
};
use factory_task::events::{EventType, for_task};
use factory_task::{BlockedReason, TaskError, TaskStatus};

/// A deterministic, distinct, syntactically valid UUID — mirrors
/// `factory-task/tests/create.rs::uid`.
fn uid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

/// Insert a scope row directly, bypassing `factory-registry` — mirrors
/// `factory-task/tests/create.rs::seed_scope`.
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
/// `dir` — mirrors `factory-task/tests/create.rs::seed_running_session`.
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

/// Seed a `running` task with `assigned_session_id` set — mirrors
/// `factory-task/tests/create.rs::seed_running_task`.
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

/// Seed a `running` task whose `cancel_requested_at` is already set — the
/// state `create::cancel`'s `Running` arm leaves a task in.
fn seed_running_task_with_cancel_requested(
    store: &mut Store,
    seed: u32,
    scope_id: uuid::Uuid,
    session_id: uuid::Uuid,
) -> uuid::Uuid {
    let task_id = uid(seed);
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO tasks (id, target_scope_id, assigned_session_id, prompt, status, cancel_requested_at) \
         VALUES (?1, ?2, ?3, 'do it', 'running', CURRENT_TIMESTAMP)",
        (
            task_id.to_string(),
            scope_id.to_string(),
            session_id.to_string(),
        ),
    )
    .expect("insert running task with a cancellation request");
    tx.commit().expect("commit");
    task_id
}

/// Seed a `blocked` task directly, with a valid reason — mirrors
/// `factory-task/tests/create.rs::seed_blocked_task`.
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

/// Seed a task in a terminal status directly — mirrors
/// `factory-task/tests/create.rs::seed_terminal_task`.
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

fn observation(
    confidence: factory_adapter::Confidence,
    task_signal: factory_adapter::TaskSignal,
) -> factory_adapter::Observation {
    factory_adapter::Observation {
        pane: factory_adapter::PaneId("pane-1".to_string()),
        harness_state: "whatever the harness said".to_string(),
        confidence,
        session_alive: true,
        task_signal,
        transcript_path: None,
        harness_session_id: None,
    }
}

// done / fail --------------------------------------------------------------

#[test]
fn done_with_a_summary_moves_a_running_task_to_done_and_stores_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_running_task(&mut store, 3, scope_id, session_id);

    done(&mut store, task_id, Some("all done"), None).expect("done");

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(task.status, TaskStatus::Done);
    assert_eq!(task.result_summary.as_deref(), Some("all done"));
    assert_eq!(task.blocked_reason, None);
}

#[test]
fn fail_with_artifact_paths_moves_a_running_task_to_failed_and_stores_the_json_array() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_running_task(&mut store, 3, scope_id, session_id);

    let paths = vec!["out/log.txt".to_string(), "out/diff.patch".to_string()];
    fail(&mut store, task_id, None, Some(&paths)).expect("fail");

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(task.status, TaskStatus::Failed);
    let stored = task
        .result_artifact_paths
        .as_deref()
        .expect("artifact paths stored");
    let parsed: Vec<String> = serde_json::from_str(stored).expect("valid JSON array");
    assert_eq!(parsed, paths);
}

#[test]
fn done_with_neither_summary_nor_paths_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_running_task(&mut store, 3, scope_id, session_id);

    let err = done(&mut store, task_id, None, None).expect_err("backlog §7 requires one");
    assert!(matches!(err, CompleteError::NoResult(id) if id == task_id));

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(
        task.status,
        TaskStatus::Running,
        "a refused call must not touch the row"
    );
}

/// An explicitly-empty summary or artifact-path list is, in substance, no
/// result at all — `Some("")` and `Some(&[])` must be refused exactly like
/// `None`, not accepted as satisfying backlog §7's "a result summary or
/// artifact paths."
#[test]
fn done_with_an_empty_summary_and_an_empty_artifact_list_is_also_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_running_task(&mut store, 3, scope_id, session_id);

    let empty_paths: Vec<String> = Vec::new();
    let err = done(&mut store, task_id, Some(""), Some(&empty_paths))
        .expect_err("an empty summary and an empty path list are still no result");
    assert!(matches!(err, CompleteError::NoResult(id) if id == task_id));

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.result_summary, None);
    assert_eq!(task.result_artifact_paths, None);
}

/// This task's report mutation 4's direct target: byte, not char, counting.
/// A multi-byte string at exactly the byte limit is accepted; one byte over
/// is refused. `"é"` is 2 bytes in UTF-8 but 1 `char`, so a char-counting
/// check would accept a string this test proves must be refused.
#[test]
fn a_multibyte_summary_is_counted_in_bytes_not_chars() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");

    // "é" is 2 bytes (U+00E9 in UTF-8: 0xC3 0xA9) and 1 char. Build a
    // summary of exactly MAX bytes using only "é" characters (an odd max
    // would leave one stray byte; the crate's MAX is a round power-of-two
    // multiple of 1024, so it is even, and this string is exactly at the
    // limit), then one that is exactly one byte over.
    let max = factory_task::MAX_RESULT_SUMMARY_BYTES;
    assert_eq!(max % 2, 0, "the byte limit must be even for this fixture");
    let at_limit: String = "é".repeat(max / 2);
    assert_eq!(
        at_limit.len(),
        max,
        "fixture must be exactly at the byte limit"
    );
    assert_eq!(
        at_limit.chars().count(),
        max / 2,
        "the same string is far fewer chars than bytes — this is the point"
    );

    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_running_task(&mut store, 3, scope_id, session_id);
    done(&mut store, task_id, Some(&at_limit), None)
        .expect("exactly at the byte limit is accepted");

    let over_limit: String = format!("{at_limit}x");
    assert_eq!(over_limit.len(), max + 1);
    let session_id2 = seed_running_session(&mut store, dir.path(), 4, scope_id);
    let task_id2 = seed_running_task(&mut store, 5, scope_id, session_id2);
    let err = done(&mut store, task_id2, Some(&over_limit), None)
        .expect_err("one byte over the limit must be refused");
    assert!(matches!(
        err,
        CompleteError::Task(TaskError::ResultSummaryTooLarge { len, max: m })
            if len == max + 1 && m == max
    ));

    // Overflow is refused, never truncated: the row must be untouched.
    let task2 = factory_task::create::show(&store, task_id2).expect("show");
    assert_eq!(task2.status, TaskStatus::Running);
    assert_eq!(
        task2.result_summary, None,
        "a refused result must not be truncated and stored"
    );
}

#[test]
fn oversized_artifact_paths_are_refused_without_truncation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_running_task(&mut store, 3, scope_id, session_id);

    let huge_path = "x".repeat(factory_task::MAX_RESULT_ARTIFACT_PATHS_BYTES);
    let paths = vec![huge_path];
    let err = fail(&mut store, task_id, None, Some(&paths)).expect_err("oversized paths refused");
    assert!(matches!(
        err,
        CompleteError::Task(TaskError::ResultArtifactPathsTooLarge { .. })
    ));

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.result_artifact_paths, None);
}

#[test]
fn done_of_a_terminal_task_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = seed_terminal_task(&mut store, 3, scope_id, "cancelled");

    let err = done(&mut store, task_id, Some("late"), None).expect_err("terminal");
    assert!(matches!(
        err,
        CompleteError::Task(TaskError::AlreadyTerminal {
            id,
            status: TaskStatus::Cancelled
        }) if id == task_id
    ));
}

// blocked --------------------------------------------------------------

#[test]
fn blocked_moves_a_running_task_to_blocked_with_the_given_reason() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_running_task(&mut store, 3, scope_id, session_id);

    blocked(&mut store, task_id, BlockedReason::Permission).expect("blocked");

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(task.status, TaskStatus::Blocked);
    assert_eq!(task.blocked_reason, Some(BlockedReason::Permission));
}

/// `blocked` is not terminal (design §2.4) — but it is still not its own
/// valid target (`lib.rs`'s `valid_targets(Blocked)` is `[Queued, Failed,
/// Cancelled]`, no `Blocked`). A second `blocked` call on an already-blocked
/// task is therefore refused as an ordinary `InvalidTransition`, never as
/// `AlreadyTerminal` — that distinction is the direct evidence that
/// `blocked` reads as non-terminal all the way through this module, not
/// merely in `TaskStatus::is_terminal`'s own return value.
#[test]
fn a_second_blocked_call_on_an_already_blocked_task_is_an_invalid_transition_not_terminal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = seed_blocked_task(&mut store, 3, scope_id);

    let err = blocked(&mut store, task_id, BlockedReason::Permission)
        .expect_err("blocked is not its own target");
    assert!(
        matches!(
            err,
            CompleteError::Task(TaskError::InvalidTransition {
                id,
                from: TaskStatus::Blocked,
                to: TaskStatus::Blocked,
                ..
            }) if id == task_id
        ),
        "must be InvalidTransition, never AlreadyTerminal: {err:?}"
    );
}

/// `done`/`fail` on a `blocked` task: `lib.rs`'s table has no `blocked →
/// done` edge, so this must refuse as `InvalidTransition`, and the row must
/// be left completely untouched.
#[test]
fn done_of_a_blocked_task_is_an_invalid_transition_and_leaves_the_row_untouched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = seed_blocked_task(&mut store, 3, scope_id);

    let err = done(&mut store, task_id, Some("premature"), None)
        .expect_err("blocked cannot go straight to done");
    assert!(matches!(
        err,
        CompleteError::Task(TaskError::InvalidTransition {
            id,
            from: TaskStatus::Blocked,
            to: TaskStatus::Done,
            ..
        }) if id == task_id
    ));

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(task.status, TaskStatus::Blocked);
    assert_eq!(task.result_summary, None);
}

#[test]
fn blocked_of_a_terminal_task_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = seed_terminal_task(&mut store, 3, scope_id, "done");

    let err = blocked(&mut store, task_id, BlockedReason::External).expect_err("terminal");
    assert!(matches!(
        err,
        CompleteError::Task(TaskError::AlreadyTerminal {
            id,
            status: TaskStatus::Done
        }) if id == task_id
    ));
}

// blocked_from_observation -------------------------------------------------

#[test]
fn an_authoritative_blocked_observation_blocks_the_task() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_running_task(&mut store, 3, scope_id, session_id);

    let obs = observation(
        factory_adapter::Confidence::Authoritative,
        factory_adapter::TaskSignal::Blocked(factory_adapter::BlockedReason::Permission),
    );
    let reason = blocked_from_observation(&mut store, task_id, &obs).expect("authoritative blocks");
    assert_eq!(reason, Some(BlockedReason::Permission));

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(task.status, TaskStatus::Blocked);
    assert_eq!(task.blocked_reason, Some(BlockedReason::Permission));
}

/// This task's report mutation 3's direct target, exercised through this
/// module's own path (not only `lib.rs`'s unit tests): `Degraded` must never
/// produce a blocked reason, so the task must be left completely untouched.
#[test]
fn a_degraded_blocked_observation_leaves_the_task_untouched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_running_task(&mut store, 3, scope_id, session_id);

    let obs = observation(
        factory_adapter::Confidence::Degraded,
        factory_adapter::TaskSignal::Blocked(factory_adapter::BlockedReason::Clarification),
    );
    let reason = blocked_from_observation(&mut store, task_id, &obs)
        .expect("Degraded never errors — it is a normal, non-exceptional outcome");
    assert_eq!(reason, None, "Degraded must never produce a blocked reason");

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(
        task.status,
        TaskStatus::Running,
        "a Degraded observation must leave the task exactly as it was"
    );
    assert_eq!(task.blocked_reason, None);
}

#[test]
fn an_unavailable_observation_leaves_the_task_untouched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_running_task(&mut store, 3, scope_id, session_id);

    let obs = observation(
        factory_adapter::Confidence::Unavailable,
        factory_adapter::TaskSignal::NoChange,
    );
    let reason = blocked_from_observation(&mut store, task_id, &obs).expect("no error");
    assert_eq!(reason, None);

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(task.status, TaskStatus::Running);
}

// acknowledge_cancellation ---------------------------------------------

/// The heart of the acknowledged-cancellation feature: a running task with a
/// recorded `cancel_requested_at` reaches `cancelled` once acknowledged.
#[test]
fn acknowledge_cancellation_moves_a_requested_running_task_to_cancelled() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_running_task_with_cancel_requested(&mut store, 3, scope_id, session_id);

    acknowledge_cancellation(&mut store, task_id).expect("acknowledge");

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(task.status, TaskStatus::Cancelled);
}

/// This task's report mutation 5's direct target: the guard. A running task
/// with **no** recorded cancellation request must not reach `cancelled`
/// through this function — dropping the guard is exactly what would let it.
#[test]
fn acknowledge_cancellation_without_a_recorded_request_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_running_task(&mut store, 3, scope_id, session_id);

    let err = acknowledge_cancellation(&mut store, task_id)
        .expect_err("no cancel_requested_at was ever recorded");
    assert!(matches!(
        err,
        CompleteError::CancellationNotRequested(id) if id == task_id
    ));

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(
        task.status,
        TaskStatus::Running,
        "a running task with no recorded request must not reach cancelled"
    );
}

#[test]
fn acknowledge_cancellation_of_a_queued_task_is_refused_as_not_running() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id =
        factory_task::create::create(&mut store, uid(3), None, scope_id, None, None, "do it", &[])
            .expect("create");

    let err = acknowledge_cancellation(&mut store, task_id).expect_err("not running");
    assert!(matches!(
        err,
        CompleteError::NotRunning {
            id,
            status: TaskStatus::Queued
        } if id == task_id
    ));
}

#[test]
fn acknowledge_cancellation_of_a_terminal_task_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = seed_terminal_task(&mut store, 3, scope_id, "failed");

    let err = acknowledge_cancellation(&mut store, task_id).expect_err("terminal");
    assert!(matches!(
        err,
        CompleteError::Task(TaskError::AlreadyTerminal {
            id,
            status: TaskStatus::Failed
        }) if id == task_id
    ));
}

#[test]
fn acknowledge_cancellation_of_a_nonexistent_task_is_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let err = acknowledge_cancellation(&mut store, uid(999)).expect_err("not found");
    assert!(matches!(
        err,
        CompleteError::Task(TaskError::NotFound(id)) if id == uid(999)
    ));
}

// events (station 11, `crate`'s own decision 3) ---------------------------

/// `done` writes exactly one `done` event, saying that a result was reported
/// and **not** repeating it.
///
/// The summary itself lives in `tasks.result_summary`, one join away by
/// `task_id`. `task_events` is append-only and is never corrected or deleted,
/// so a copy written here would outlive any later edit of the task's own, and
/// a result summary can be 16 KiB of agent-authored free text. Crate decision
/// 6 asks for short structured facts in a payload; this is the fact.
///
/// Mutation caught: putting the text back. The assertion below fails if
/// `result_summary` reappears as a payload key.
#[test]
fn done_writes_one_done_event_that_records_a_result_without_copying_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_running_task(&mut store, 3, scope_id, session_id);

    done(&mut store, task_id, Some("all done"), None).expect("done");

    let events = for_task(&store, task_id).expect("read events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, EventType::Done);
    assert_eq!(events[0].author_session_id, None);
    let payload: serde_json::Value =
        serde_json::from_str(events[0].payload.as_deref().expect("payload present"))
            .expect("valid JSON");
    assert_eq!(
        payload["result_summary_present"], true,
        "the event records that a result exists"
    );
    assert!(
        payload.get("result_summary").is_none(),
        "the summary text itself must not be copied into the append-only log; \
         it is in tasks.result_summary, one join away"
    );
}

/// `fail` with only artifact paths (no summary) writes a `failed` event with
/// no payload — "a failed event carries a short summary **if one exists**";
/// this is the case where one does not.
#[test]
fn fail_with_only_artifact_paths_writes_a_failed_event_with_no_payload() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_running_task(&mut store, 3, scope_id, session_id);

    let paths = vec!["out/log.txt".to_string()];
    fail(&mut store, task_id, None, Some(&paths)).expect("fail");

    let events = for_task(&store, task_id).expect("read events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, EventType::Failed);
    assert_eq!(
        events[0].payload, None,
        "no summary was given, so nothing is recorded — not the artifact paths, \
         which are not what the brief asks a failed event to carry"
    );
}

/// The mutation this test catches: a refused completion (already terminal)
/// leaves no event behind. `done` returns `TaskError::AlreadyTerminal`
/// before it ever reaches a write.
#[test]
fn done_of_a_terminal_task_writes_no_event() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = seed_terminal_task(&mut store, 3, scope_id, "cancelled");

    done(&mut store, task_id, Some("late"), None).expect_err("terminal");

    let events = for_task(&store, task_id).expect("read events");
    assert_eq!(
        events.len(),
        0,
        "this task was seeded directly by raw SQL, not through `create`, so it \
         starts with no events, and the refused call must add none"
    );
}

/// `blocked` writes exactly one `blocked` event, carrying the reason — the
/// brief's own words: "a blocked event carries its reason."
#[test]
fn blocked_writes_exactly_one_blocked_event_with_the_reason() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_running_task(&mut store, 3, scope_id, session_id);

    blocked(&mut store, task_id, BlockedReason::Permission).expect("blocked");

    let events = for_task(&store, task_id).expect("read events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, EventType::Blocked);
    let payload: serde_json::Value =
        serde_json::from_str(events[0].payload.as_deref().expect("payload present"))
            .expect("valid JSON");
    assert_eq!(payload["reason"], "permission");
}

/// `blocked_from_observation` reaches the same one `blocked` call as
/// `blocked` itself, so an authoritative observation must write the
/// identical event, not a second kind.
#[test]
fn an_authoritative_blocked_observation_writes_a_blocked_event() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_running_task(&mut store, 3, scope_id, session_id);

    let obs = observation(
        factory_adapter::Confidence::Authoritative,
        factory_adapter::TaskSignal::Blocked(factory_adapter::BlockedReason::Interrupted),
    );
    blocked_from_observation(&mut store, task_id, &obs).expect("blocked");

    let events = for_task(&store, task_id).expect("read events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, EventType::Blocked);
}

/// The mutation this test catches: a refused transition (`blocked` is not
/// its own valid target) leaves no additional event behind — the exact
/// scenario `a_second_blocked_call_on_an_already_blocked_task_is_an_invalid_transition_not_terminal`
/// above proves for the status column; this is the same proof for events.
#[test]
fn a_second_blocked_call_on_an_already_blocked_task_writes_no_additional_event() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_running_task(&mut store, 3, scope_id, session_id);

    blocked(&mut store, task_id, BlockedReason::Permission).expect("first block succeeds");
    blocked(&mut store, task_id, BlockedReason::Permission)
        .expect_err("blocked is not its own valid target");

    let events = for_task(&store, task_id).expect("read events");
    assert_eq!(
        events.len(),
        1,
        "only the first, successful call may have written an event"
    );
}

/// `acknowledge_cancellation` writes exactly one `cancelled` event, in the
/// same transaction as the status write.
#[test]
fn acknowledge_cancellation_writes_exactly_one_cancelled_event() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_running_task_with_cancel_requested(&mut store, 3, scope_id, session_id);

    acknowledge_cancellation(&mut store, task_id).expect("acknowledge");

    let events = for_task(&store, task_id).expect("read events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, EventType::Cancelled);
    assert_eq!(events[0].payload, None);
}

/// The mutation this test catches: the cooperative-cancellation guard
/// refusing (no recorded request) must leave no event behind.
#[test]
fn acknowledge_cancellation_without_a_recorded_request_writes_no_event() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_running_task(&mut store, 3, scope_id, session_id);

    acknowledge_cancellation(&mut store, task_id).expect_err("no request was ever recorded");

    let events = for_task(&store, task_id).expect("read events");
    assert_eq!(
        events.len(),
        0,
        "this task was seeded directly by raw SQL, not through `create`, so it \
         starts with no events, and the refused call must add none"
    );
}
