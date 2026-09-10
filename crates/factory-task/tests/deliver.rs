//! `factory_task::deliver`'s own test suite: design §5 steps 3 and 4, backlog
//! §7's "the delivery attempt is recorded before input; every prompt
//! contains the task UUID."
//!
//! # A note on the fixtures below
//!
//! `assign` (the module that would, in a complete Slice 7, choose a session
//! and set `assigned_session_id`) stays an untouched placeholder in this
//! task — another agent owns it. So every test here that needs a `queued`
//! task with an assigned session seeds one directly with raw SQL, the same
//! way `factory-task/tests/create.rs` seeds a `running` task for `cancel`'s
//! cooperative path, and the same way `factory-session`'s own test suite
//! seeds tasks it does not own the creation of.

use rusqlite::OptionalExtension;

use factory_paths::CanonicalPath;
use factory_store::Store;
use factory_task::TaskError;
use factory_task::TaskStatus;
use factory_task::deliver::{
    DeliverError, OperatorPromptWriter, PromptWriteError, PromptWriter, authorise_resume, deliver,
    mark_running,
};
use factory_task::events::{EventType, for_task};

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

/// Seed a `queued` task with `assigned_session_id` already set — the state
/// `assign` would leave a task in, immediately before `deliver` runs.
fn seed_assigned_task(
    store: &mut Store,
    seed: u32,
    scope_id: uuid::Uuid,
    session_id: uuid::Uuid,
    prompt: &str,
) -> uuid::Uuid {
    let task_id = uid(seed);
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO tasks (id, target_scope_id, assigned_session_id, prompt, status) \
         VALUES (?1, ?2, ?3, ?4, 'queued')",
        (
            task_id.to_string(),
            scope_id.to_string(),
            session_id.to_string(),
            prompt,
        ),
    )
    .expect("insert assigned task");
    tx.commit().expect("commit");
    task_id
}

/// A plain `queued` task with no assigned session, as `create::create`
/// alone leaves it.
fn seed_unassigned_queued_task(store: &mut Store, seed: u32, scope_id: uuid::Uuid) -> uuid::Uuid {
    factory_task::create::create(store, uid(seed), None, scope_id, None, None, "do it", &[])
        .expect("create a queued task")
}

fn delivery_attempts_for(store: &Store, task_id: uuid::Uuid) -> Vec<(i64, Option<String>)> {
    let mut stmt = store
        .connection()
        .prepare("SELECT id, outcome FROM delivery_attempts WHERE task_id = ?1 ORDER BY id")
        .expect("prepare");
    stmt.query_map([task_id.to_string()], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect")
}

/// A writer that always succeeds and records what it was called with.
struct RecordingWriter {
    calls: Vec<(uuid::Uuid, String)>,
}

impl RecordingWriter {
    fn new() -> Self {
        Self { calls: Vec::new() }
    }
}

impl PromptWriter for RecordingWriter {
    fn write_prompt(
        &mut self,
        session_id: uuid::Uuid,
        prompt: &str,
    ) -> Result<(), PromptWriteError> {
        self.calls.push((session_id, prompt.to_string()));
        Ok(())
    }
}

/// A writer that always fails.
struct FailingWriter;

impl PromptWriter for FailingWriter {
    fn write_prompt(
        &mut self,
        _session_id: uuid::Uuid,
        _prompt: &str,
    ) -> Result<(), PromptWriteError> {
        Err(PromptWriteError::new("the operator's terminal is gone"))
    }
}

/// A writer that refuses before writing anything — the shape
/// `AdapterPromptWriter` takes when the adapter reports `SessionBusy`.
struct RefusingWriter {
    calls: usize,
}

impl PromptWriter for RefusingWriter {
    fn write_prompt(
        &mut self,
        _session_id: uuid::Uuid,
        _prompt: &str,
    ) -> Result<(), PromptWriteError> {
        self.calls += 1;
        Err(PromptWriteError::refused(
            "the session is working; nothing was submitted",
        ))
    }
}

/// What the row looked like, from the writer's own point of view, the
/// instant it was called — read through a *separate* connection to the same
/// database file, not through the `Store` `deliver` is using, so this is
/// genuine evidence of what had already been committed by the time the
/// writer ran, not merely what the test asserts afterwards.
#[derive(Debug, PartialEq, Eq)]
enum ObservedAtCallTime {
    NoRowYet,
    RowWithNullOutcome,
    RowWithOutcome(String),
}

/// A writer that, before doing anything else, opens its own connection to
/// the database and reads back the `delivery_attempts` row for `task_id` —
/// proving the row (and its `NULL` outcome) was committed *before* the
/// writer was ever invoked. This is the direct evidence for design §5 step
/// 3 ("record the delivery attempt before writing to the PTY") and this
/// task's report mutation 1 (move the insert-and-commit to after the writer
/// call): under that mutation, this writer observes `NoRowYet` instead.
struct SpyWriter {
    db_path: std::path::PathBuf,
    task_id: uuid::Uuid,
    fail: bool,
    observed_at_call: Option<ObservedAtCallTime>,
    row_count_at_call: Option<i64>,
}

impl SpyWriter {
    fn new(db_path: std::path::PathBuf, task_id: uuid::Uuid, fail: bool) -> Self {
        Self {
            db_path,
            task_id,
            fail,
            observed_at_call: None,
            row_count_at_call: None,
        }
    }
}

impl PromptWriter for SpyWriter {
    fn write_prompt(
        &mut self,
        _session_id: uuid::Uuid,
        prompt: &str,
    ) -> Result<(), PromptWriteError> {
        assert!(
            prompt.contains(&self.task_id.to_string()),
            "the prompt handed to the writer must contain the task UUID"
        );

        let conn = rusqlite::Connection::open(&self.db_path).expect("open a separate connection");
        let outcome: Option<Option<String>> = conn
            .query_row(
                "SELECT outcome FROM delivery_attempts WHERE task_id = ?1",
                [self.task_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .expect("query delivery_attempts");
        self.observed_at_call = Some(match outcome {
            None => ObservedAtCallTime::NoRowYet,
            Some(None) => ObservedAtCallTime::RowWithNullOutcome,
            Some(Some(value)) => ObservedAtCallTime::RowWithOutcome(value),
        });
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM delivery_attempts WHERE task_id = ?1",
                [self.task_id.to_string()],
                |row| row.get(0),
            )
            .expect("count delivery_attempts");
        self.row_count_at_call = Some(count);

        if self.fail {
            Err(PromptWriteError::new("boom"))
        } else {
            Ok(())
        }
    }
}

// deliver --------------------------------------------------------------

/// The core ordering proof (design §5 steps 3–4; this task's report mutation
/// 1): the `delivery_attempts` row exists, with `outcome` still `NULL`, and
/// there is exactly one such row, at the exact moment the writer runs —
/// observed through a connection `deliver` itself has no way to influence.
#[test]
fn the_delivery_attempt_row_is_committed_with_null_outcome_before_the_writer_runs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_assigned_task(&mut store, 3, scope_id, session_id, "do it");

    let mut writer = SpyWriter::new(store.path().to_path_buf(), task_id, false);
    deliver(&mut store, task_id, &mut writer).expect("deliver succeeds");

    assert_eq!(
        writer.observed_at_call,
        Some(ObservedAtCallTime::RowWithNullOutcome),
        "the row must already be committed, with outcome NULL, by the time the writer is called"
    );
    assert_eq!(
        writer.row_count_at_call,
        Some(1),
        "exactly one delivery_attempts row must exist at call time"
    );

    // After a successful deliver, the outcome is recorded as sent.
    let rows = delivery_attempts_for(&store, task_id);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1.as_deref(), Some("sent"));
}

/// The failure twin of the test above (this task's report mutation 1, other
/// half): a writer that errors still leaves the row that was committed
/// before it ran — the row is not rolled back or deleted because the writer
/// failed — and its outcome is updated to reflect the failure once
/// `deliver` returns.
#[test]
fn a_failing_writer_still_leaves_the_pre_committed_row_and_records_the_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_assigned_task(&mut store, 3, scope_id, session_id, "do it");

    let mut writer = SpyWriter::new(store.path().to_path_buf(), task_id, true);
    let err = deliver(&mut store, task_id, &mut writer).expect_err("writer fails");
    assert!(matches!(err, DeliverError::WriteFailed { id, .. } if id == task_id));

    assert_eq!(
        writer.observed_at_call,
        Some(ObservedAtCallTime::RowWithNullOutcome),
        "the row was committed before the (failing) writer ran"
    );
    assert_eq!(writer.row_count_at_call, Some(1));

    let rows = delivery_attempts_for(&store, task_id);
    assert_eq!(
        rows.len(),
        1,
        "a writer error must not delete or roll back the pre-committed row"
    );
    assert_eq!(
        rows[0].1.as_deref(),
        Some("failed"),
        "the outcome must be updated to reflect the writer's failure"
    );

    // A writer failure must not move the task's status at all.
    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(
        task.status,
        TaskStatus::Queued,
        "deliver never writes tasks.status; a failed write leaves the task queued, \
         exactly the ambiguous state ADR 0019 decision 2's restore reconciliation expects"
    );
}

/// Backlog §7: "every prompt contains the task UUID." Direct evidence
/// through a `RecordingWriter` rather than through `SpyWriter`'s inline
/// assertion, so this requirement has its own named test independent of the
/// ordering proof above.
#[test]
fn the_prompt_handed_to_the_writer_contains_the_task_uuid() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_assigned_task(&mut store, 3, scope_id, session_id, "please do the thing");

    let mut writer = RecordingWriter::new();
    deliver(&mut store, task_id, &mut writer).expect("deliver succeeds");

    assert_eq!(writer.calls.len(), 1);
    let (delivered_session, delivered_prompt) = &writer.calls[0];
    assert_eq!(*delivered_session, session_id);
    assert!(
        delivered_prompt.contains(&task_id.to_string()),
        "the prompt must contain the task UUID: {delivered_prompt:?}"
    );
    assert!(
        delivered_prompt.contains("please do the thing"),
        "the prompt must still contain the task's own text: {delivered_prompt:?}"
    );
}

/// This task's report mutation 2: the at-most-once guard. A second
/// `deliver` call for the same task, after a first successful one, must be
/// refused — and must leave the original row untouched (no second row).
#[test]
fn a_second_delivery_attempt_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_assigned_task(&mut store, 3, scope_id, session_id, "do it");

    let mut first = RecordingWriter::new();
    deliver(&mut store, task_id, &mut first).expect("first delivery succeeds");

    let mut second = RecordingWriter::new();
    let err = deliver(&mut store, task_id, &mut second).expect_err("a second attempt is refused");
    assert!(matches!(err, DeliverError::AlreadyAttempted(id) if id == task_id));
    assert!(
        second.calls.is_empty(),
        "the writer must never even be called for a refused second attempt"
    );

    let rows = delivery_attempts_for(&store, task_id);
    assert_eq!(
        rows.len(),
        1,
        "a refused second attempt must not add a second delivery_attempts row"
    );
}

/// The same guard, exercised after an *ambiguous* (failed-writer) first
/// attempt: design §5 forbids a resend "after an ambiguous failure," and the
/// task report explicitly calls this the exact situation the guard exists
/// for.
#[test]
fn a_second_delivery_attempt_after_a_failed_first_one_is_also_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_assigned_task(&mut store, 3, scope_id, session_id, "do it");

    let mut failing = FailingWriter;
    deliver(&mut store, task_id, &mut failing).expect_err("first delivery fails");

    let mut second = RecordingWriter::new();
    let err =
        deliver(&mut store, task_id, &mut second).expect_err("a second attempt is still refused");
    assert!(matches!(err, DeliverError::AlreadyAttempted(id) if id == task_id));
    assert!(second.calls.is_empty());
}

#[test]
fn deliver_of_a_task_that_is_not_queued_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_assigned_task(&mut store, 3, scope_id, session_id, "do it");
    let mut setup_writer = RecordingWriter::new();
    deliver(&mut store, task_id, &mut setup_writer).expect("deliver once, to set up the fixture");
    mark_running(&mut store, task_id).expect("mark running for the test");
    // Now confirm the now-`running` task's second `deliver` is refused for
    // being not-queued (a distinct refusal from the at-most-once guard,
    // exercised separately above).

    let mut writer = RecordingWriter::new();
    let err = deliver(&mut store, task_id, &mut writer).expect_err("not queued");
    assert!(matches!(
        err,
        DeliverError::NotQueued {
            id,
            status: TaskStatus::Running
        } if id == task_id
    ));
}

#[test]
fn deliver_of_an_unassigned_queued_task_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = seed_unassigned_queued_task(&mut store, 2, scope_id);

    let mut writer = RecordingWriter::new();
    let err = deliver(&mut store, task_id, &mut writer).expect_err("no assigned session");
    assert!(matches!(err, DeliverError::NotAssigned(id) if id == task_id));
    assert!(writer.calls.is_empty());
}

#[test]
fn deliver_of_a_nonexistent_task_is_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let mut writer = RecordingWriter::new();
    let err = deliver(&mut store, uid(999), &mut writer).expect_err("no such task");
    assert!(matches!(err, DeliverError::NotFound(id) if id == uid(999)));
}

/// `deliver` must never write `tasks.status`, whether it succeeds or fails —
/// the direct evidence for this file's own doc comment on why a writer
/// failure does not move the task to `failed`.
#[test]
fn a_successful_deliver_leaves_the_task_queued() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_assigned_task(&mut store, 3, scope_id, session_id, "do it");

    let mut writer = RecordingWriter::new();
    deliver(&mut store, task_id, &mut writer).expect("deliver succeeds");

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(
        task.status,
        TaskStatus::Queued,
        "deliver marks nothing running — mark_running is the only place that happens"
    );
}

// mark_running -----------------------------------------------------------

#[test]
fn mark_running_moves_a_delivered_task_to_running() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_assigned_task(&mut store, 3, scope_id, session_id, "do it");

    let mut writer = RecordingWriter::new();
    deliver(&mut store, task_id, &mut writer).expect("deliver succeeds");

    mark_running(&mut store, task_id).expect("mark running");

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.assigned_session_id, Some(session_id));
}

/// `mark_running` requires a `delivery_attempts` row (any outcome, even
/// `NULL`) but does not require `outcome = 'sent'` — a NULL-outcome row is
/// exactly the "attempt made, result unknown" case an external confirmation
/// (a real observation) can still resolve to `running`.
#[test]
fn mark_running_accepts_a_delivery_attempt_whose_outcome_is_still_unknown() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_assigned_task(&mut store, 3, scope_id, session_id, "do it");

    // Seed a delivery_attempts row directly, with outcome left NULL — the
    // exact state a crash between deliver's two transactions would leave.
    {
        let tx = store.transaction().expect("begin");
        tx.execute(
            "INSERT INTO delivery_attempts (task_id, session_id, outcome) VALUES (?1, ?2, NULL)",
            (task_id.to_string(), session_id.to_string()),
        )
        .expect("insert delivery attempt");
        tx.commit().expect("commit");
    }

    mark_running(&mut store, task_id).expect("mark running despite an unknown outcome");
    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(task.status, TaskStatus::Running);
}

#[test]
fn mark_running_of_an_undelivered_task_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_assigned_task(&mut store, 3, scope_id, session_id, "do it");

    let err = mark_running(&mut store, task_id).expect_err("never delivered");
    assert!(matches!(err, DeliverError::NotDelivered(id) if id == task_id));

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(
        task.status,
        TaskStatus::Queued,
        "a refused call must not touch the row"
    );
}

/// `OperatorPromptWriter` is the shipped implementation: it must actually
/// render the prompt somewhere a human can read it, including the task
/// UUID, and never touch a PTY.
#[test]
fn operator_prompt_writer_renders_the_prompt_and_session_id() {
    let mut out: Vec<u8> = Vec::new();
    let mut writer = OperatorPromptWriter::new(&mut out);
    let session_id = uid(1);
    let task_id = uid(2);
    let prompt = format!("[task {task_id}]\ndo the thing");

    writer
        .write_prompt(session_id, &prompt)
        .expect("writing to an in-memory buffer cannot fail");

    let rendered = String::from_utf8(out).expect("utf8");
    assert!(rendered.contains(&session_id.to_string()));
    assert!(rendered.contains(&task_id.to_string()));
    assert!(rendered.contains("do the thing"));
}

// authorise_resume -------------------------------------------------------

fn authorised_deliveries_of(store: &Store, task_id: uuid::Uuid) -> i64 {
    store
        .connection()
        .query_row(
            "SELECT authorised_deliveries FROM tasks WHERE id = ?1",
            [task_id.to_string()],
            |row| row.get(0),
        )
        .expect("read authorised_deliveries")
}

/// Move a task all the way to `blocked` through the ordinary path — assign,
/// deliver, mark running, then block it via an authoritative observation —
/// mirroring how a real clarification request would arrive mid-task, so
/// `authorise_resume`'s own tests exercise it against a task that got there
/// the way slice 7's guard was designed for, not a hand-built row.
fn seed_blocked_task(
    store: &mut Store,
    seed: u32,
    scope_id: uuid::Uuid,
    session_id: uuid::Uuid,
    prompt: &str,
) -> uuid::Uuid {
    let task_id = seed_assigned_task(store, seed, scope_id, session_id, prompt);
    let mut writer = RecordingWriter::new();
    deliver(store, task_id, &mut writer).expect("deliver succeeds");
    mark_running(store, task_id).expect("mark running");
    factory_task::complete::blocked(store, task_id, factory_task::BlockedReason::Clarification)
        .expect("block the task");
    task_id
}

/// The whole operator action in one call: `blocked → queued`,
/// `assigned_session_id` cleared, `authorised_deliveries` incremented by
/// exactly one — this task's report mutation 3 target (backfill vs. increment
/// arithmetic already covered by the `deliver` suite above; this is the
/// `authorise_resume`-specific half).
#[test]
fn authorise_resume_moves_a_blocked_task_to_queued_and_grants_one_more_delivery() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_blocked_task(&mut store, 3, scope_id, session_id, "do it");

    assert_eq!(authorised_deliveries_of(&store, task_id), 1);

    authorise_resume(&mut store, task_id).expect("authorise resume succeeds");

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(task.status, TaskStatus::Queued);
    assert_eq!(
        task.assigned_session_id, None,
        "resume clears the assignment so ordinary re-assignment can choose again"
    );
    assert_eq!(
        task.blocked_reason, None,
        "blocked_reason must clear alongside the status, or the CHECK constraint \
         would have refused this very write"
    );
    assert_eq!(
        authorised_deliveries_of(&store, task_id),
        2,
        "one human action grants exactly one further delivery"
    );
}

/// The direct evidence that the authorisation this grants actually unlocks a
/// second delivery, closing the loop `a_second_delivery_attempt_is_refused`
/// (above) opened: without `authorise_resume`, a second `deliver` call on
/// this same task is refused; after it — and a fresh assignment, since resume
/// deliberately clears the old one — it succeeds and leaves a second
/// `delivery_attempts` row.
#[test]
fn a_task_can_be_delivered_again_after_authorise_resume_grants_a_second_delivery() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_blocked_task(&mut store, 3, scope_id, session_id, "do it");

    authorise_resume(&mut store, task_id).expect("authorise resume succeeds");

    // `authorise_resume` clears `assigned_session_id` on purpose (see its own
    // doc comment); re-assignment is `assign`'s job, another agent's module,
    // so this test re-assigns with raw SQL, the same way every other fixture
    // in this file stands in for it.
    {
        let tx = store.transaction().expect("begin");
        tx.execute(
            "UPDATE tasks SET assigned_session_id = ?2 WHERE id = ?1",
            (task_id.to_string(), session_id.to_string()),
        )
        .expect("re-assign");
        tx.commit().expect("commit");
    }

    let mut writer = RecordingWriter::new();
    deliver(&mut store, task_id, &mut writer)
        .expect("the second, authorised delivery must succeed");

    let rows = delivery_attempts_for(&store, task_id);
    assert_eq!(
        rows.len(),
        2,
        "two delivery_attempts rows now exist: the original attempt and the \
         authorised resend"
    );

    // The property design §5 actually turns on: one human action authorises
    // exactly *one* further delivery, not an open-ended number. Re-assign
    // again (same stand-in for `assign` as above) and attempt a *third*
    // delivery — with only one `authorise_resume` call behind it,
    // `authorised_deliveries` is `2`, two attempts are already recorded, and
    // `attempted_count >= authorised_deliveries` must refuse this one. A test
    // that stopped at the second delivery succeeding could not tell "one
    // further delivery" from "unlimited" apart — this is the assertion that
    // can.
    {
        let tx = store.transaction().expect("begin");
        tx.execute(
            "UPDATE tasks SET assigned_session_id = ?2 WHERE id = ?1",
            (task_id.to_string(), session_id.to_string()),
        )
        .expect("re-assign");
        tx.commit().expect("commit");
    }

    let mut third_writer = RecordingWriter::new();
    let err = deliver(&mut store, task_id, &mut third_writer)
        .expect_err("a third attempt, with only one authorised resume behind it, is refused");
    assert!(
        matches!(err, DeliverError::AlreadyAttempted(id) if id == task_id),
        "expected AlreadyAttempted, got {err:?}"
    );
    assert!(
        third_writer.calls.is_empty(),
        "the writer must never even be called for a refused third attempt"
    );

    let rows = delivery_attempts_for(&store, task_id);
    assert_eq!(
        rows.len(),
        2,
        "a refused third attempt must not add a third delivery_attempts row"
    );
}

/// This task's report mutation 3 target: a task that is `running` — not
/// `blocked` — has nothing for a human to authorise past.
#[test]
fn authorise_resume_of_a_running_task_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_assigned_task(&mut store, 3, scope_id, session_id, "do it");
    let mut writer = RecordingWriter::new();
    deliver(&mut store, task_id, &mut writer).expect("deliver succeeds");
    mark_running(&mut store, task_id).expect("mark running");

    let err = authorise_resume(&mut store, task_id).expect_err("a running task is refused");
    assert!(matches!(
        err,
        TaskError::NotBlocked {
            id,
            status: TaskStatus::Running
        } if id == task_id
    ));

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(
        task.status,
        TaskStatus::Running,
        "a refused call must not touch the row"
    );
    assert_eq!(authorised_deliveries_of(&store, task_id), 1);
}

#[test]
fn authorise_resume_of_a_queued_task_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = seed_unassigned_queued_task(&mut store, 2, scope_id);

    let err = authorise_resume(&mut store, task_id).expect_err("a queued task is refused");
    assert!(matches!(
        err,
        TaskError::NotBlocked {
            id,
            status: TaskStatus::Queued
        } if id == task_id
    ));
}

#[test]
fn authorise_resume_of_a_terminal_task_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = seed_unassigned_queued_task(&mut store, 2, scope_id);
    // `queued → failed` is a valid transition (delivery itself could not be
    // completed); `queued → done` is not (only a `running` task reports a
    // result), so `fail` is what reaches a terminal status from here in one
    // step.
    factory_task::complete::fail(&mut store, task_id, Some("could not deliver"), None)
        .expect("mark failed");

    let err = authorise_resume(&mut store, task_id).expect_err("a failed task is refused");
    assert!(matches!(
        err,
        TaskError::NotBlocked {
            id,
            status: TaskStatus::Failed
        } if id == task_id
    ));
}

#[test]
fn authorise_resume_of_a_nonexistent_task_is_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let err = authorise_resume(&mut store, uid(999)).expect_err("no such task");
    assert!(matches!(err, TaskError::NotFound(id) if id == uid(999)));
}

/// A refusal is not a delivery, so it must not spend the task's one
/// authorised delivery.
///
/// The writer established that nothing reached the terminal — that is what
/// separates `PromptWriteError::refused` from `PromptWriteError::new`. Design
/// §5 refuses to resend after an *ambiguous* failure; a refusal is not
/// ambiguous. Counting it would send an operator to `task resume` to
/// authorise a second delivery of a prompt that was never sent once, and an
/// authorisation performed routinely stops being read.
#[test]
fn a_refused_delivery_does_not_spend_the_tasks_authorised_delivery() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_assigned_task(&mut store, 3, scope_id, session_id, "do it");

    let mut refusing = RefusingWriter { calls: 0 };
    deliver(&mut store, task_id, &mut refusing).expect_err("a refusal is still an error");
    assert_eq!(refusing.calls, 1);

    // The refusal is journalled — design §5 records what happened — but it is
    // recorded as `refused`, which every "was this possibly delivered?"
    // question excludes.
    let rows = delivery_attempts_for(&store, task_id);
    assert_eq!(rows.len(), 1, "the refusal is still journalled");
    assert_eq!(rows[0].1.as_deref(), Some("refused"));

    // And so the next attempt goes through, with no human authorisation.
    let mut second = RecordingWriter::new();
    deliver(&mut store, task_id, &mut second)
        .expect("a refusal left the authorised delivery unspent");
    assert_eq!(second.calls.len(), 1, "the prompt reaches the writer");
}

/// The other half, and the one that must not regress: an *ambiguous* failure
/// still spends the authorisation. Without this test, making every failure a
/// refusal would pass the test above.
#[test]
fn an_ambiguous_failure_still_spends_the_tasks_authorised_delivery() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_assigned_task(&mut store, 3, scope_id, session_id, "do it");

    let mut failing = FailingWriter;
    deliver(&mut store, task_id, &mut failing).expect_err("first delivery fails");

    let rows = delivery_attempts_for(&store, task_id);
    assert_eq!(rows[0].1.as_deref(), Some("failed"));

    let mut second = RecordingWriter::new();
    let err = deliver(&mut store, task_id, &mut second)
        .expect_err("an ambiguous failure must still block a resend");
    assert!(matches!(err, DeliverError::AlreadyAttempted(id) if id == task_id));
    assert!(second.calls.is_empty());
}

/// `mark_running` insists the task was actually delivered. A refusal is not a
/// delivery, so a task whose only attempt was refused is not markable.
#[test]
fn mark_running_refuses_a_task_whose_only_attempt_was_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_assigned_task(&mut store, 3, scope_id, session_id, "do it");

    let mut refusing = RefusingWriter { calls: 0 };
    deliver(&mut store, task_id, &mut refusing).expect_err("refused");

    let err = mark_running(&mut store, task_id)
        .expect_err("a refused attempt is not evidence a prompt was received");
    assert!(matches!(err, DeliverError::NotDelivered(id) if id == task_id));
}

// events (station 11, `crate`'s own decision 3) ---------------------------

/// A writer that opens its own connection and reads `task_events` for
/// `task_id` at the instant it is called — the `SpyWriter` pattern above,
/// aimed at the event log instead of `delivery_attempts`. Proves the
/// `delivered`/`refused` event is written in transaction 2 (after the writer
/// runs), not transaction 1 (before it): if it were ever moved earlier, this
/// writer would observe a row that already exists.
struct EventSpyWriter {
    db_path: std::path::PathBuf,
    task_id: uuid::Uuid,
    fail: bool,
    event_count_at_call: Option<i64>,
}

impl PromptWriter for EventSpyWriter {
    fn write_prompt(
        &mut self,
        _session_id: uuid::Uuid,
        _prompt: &str,
    ) -> Result<(), PromptWriteError> {
        let conn = rusqlite::Connection::open(&self.db_path).expect("open a separate connection");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM task_events WHERE task_id = ?1",
                [self.task_id.to_string()],
                |row| row.get(0),
            )
            .expect("count task_events");
        self.event_count_at_call = Some(count);
        if self.fail {
            Err(PromptWriteError::new("boom"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn the_delivered_event_is_written_after_the_writer_runs_not_before() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_assigned_task(&mut store, 3, scope_id, session_id, "do it");

    let mut writer = EventSpyWriter {
        db_path: store.path().to_path_buf(),
        task_id,
        fail: false,
        event_count_at_call: None,
    };
    deliver(&mut store, task_id, &mut writer).expect("deliver succeeds");

    assert_eq!(
        writer.event_count_at_call,
        Some(0),
        "no event exists yet at the instant the writer runs — it is written in \
         transaction 2, after the writer returns"
    );

    let events = for_task(&store, task_id).expect("read events");
    assert_eq!(
        events.len(),
        1,
        "exactly one event once deliver has returned"
    );
    assert_eq!(events[0].event_type, EventType::Delivered);
}

/// A successful delivery writes exactly one `delivered` event, naming the
/// `delivery_attempts` row it describes — "the attempt identity", per this
/// task's brief.
#[test]
fn a_successful_delivery_writes_exactly_one_delivered_event_with_the_attempt_identity() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_assigned_task(&mut store, 3, scope_id, session_id, "do it");

    let mut writer = RecordingWriter::new();
    deliver(&mut store, task_id, &mut writer).expect("deliver succeeds");

    let events = for_task(&store, task_id).expect("read events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, EventType::Delivered);
    assert_eq!(events[0].author_session_id, None);
    let rows = delivery_attempts_for(&store, task_id);
    let payload: serde_json::Value =
        serde_json::from_str(events[0].payload.as_deref().expect("payload present"))
            .expect("valid JSON");
    assert_eq!(payload["delivery_attempt_id"], rows[0].0);
    assert_eq!(payload["outcome"], "sent");
}

/// The exact case ADR 0021 decision 4a exists to name: `refused`, never
/// `delivered`, and the task is still `queued` once `deliver` returns.
#[test]
fn a_refused_delivery_writes_refused_not_delivered_and_the_task_stays_queued() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_assigned_task(&mut store, 3, scope_id, session_id, "do it");

    let mut refusing = RefusingWriter { calls: 0 };
    deliver(&mut store, task_id, &mut refusing).expect_err("a refusal is still an error");

    let events = for_task(&store, task_id).expect("read events");
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].event_type,
        EventType::Refused,
        "a refusal must never be recorded as delivered — station 10's whole point"
    );

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(
        task.status,
        TaskStatus::Queued,
        "nothing reached a terminal, so the task stays queued"
    );
}

/// The deliberate choice this task's report calls out: an *ambiguous*
/// writer failure (not a refusal) is grouped with a successful send, both as
/// `delivered` — `ATTEMPT_MAY_HAVE_REACHED_THE_TERMINAL` already treats them
/// the same way for the at-most-once budget, and decision 4a's own reasoning
/// ("a task sitting queued with nothing in the log explaining why" is the
/// wrong answer) applies just as much to a silently-unlogged failure as to a
/// silently-unlogged refusal. The payload's `outcome` field is what keeps
/// this from reading as a claim that the prompt definitely arrived.
#[test]
fn an_ambiguous_failed_delivery_also_writes_a_delivered_event_with_outcome_failed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_assigned_task(&mut store, 3, scope_id, session_id, "do it");

    let mut failing = FailingWriter;
    deliver(&mut store, task_id, &mut failing).expect_err("an ambiguous failure is still an error");

    let events = for_task(&store, task_id).expect("read events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, EventType::Delivered);
    let payload: serde_json::Value =
        serde_json::from_str(events[0].payload.as_deref().expect("payload present"))
            .expect("valid JSON");
    assert_eq!(payload["outcome"], "failed");
}

/// The mutation this test catches: a refused transition leaves no event
/// behind. `deliver` on a task that is not `queued` fails its own
/// `NotQueued` guard in transaction 1, before either transaction 2 or an
/// event write could ever run.
#[test]
fn deliver_of_a_task_that_is_not_queued_writes_no_event() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_assigned_task(&mut store, 3, scope_id, session_id, "do it");
    let mut setup_writer = RecordingWriter::new();
    deliver(&mut store, task_id, &mut setup_writer).expect("first delivery");
    mark_running(&mut store, task_id).expect("mark running, so status is no longer queued");

    let events_before = for_task(&store, task_id).expect("read events").len();

    let mut writer = RecordingWriter::new();
    let err = deliver(&mut store, task_id, &mut writer).expect_err("already running, not queued");
    assert!(matches!(err, DeliverError::NotQueued { .. }));

    let events_after = for_task(&store, task_id).expect("read events").len();
    assert_eq!(
        events_before, events_after,
        "a refused deliver call must add no event"
    );
}

/// `mark_running` writes exactly one `running` event, naming the session,
/// in the same transaction as the status write.
#[test]
fn mark_running_writes_exactly_one_running_event_naming_the_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_assigned_task(&mut store, 3, scope_id, session_id, "do it");

    let mut writer = RecordingWriter::new();
    deliver(&mut store, task_id, &mut writer).expect("deliver succeeds");
    mark_running(&mut store, task_id).expect("mark running");

    let events = for_task(&store, task_id).expect("read events");
    // `delivered`, then this function's own `running`.
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].event_type, EventType::Running);
    assert_eq!(events[1].author_session_id, None);
    let payload: serde_json::Value =
        serde_json::from_str(events[1].payload.as_deref().expect("payload present"))
            .expect("valid JSON");
    assert_eq!(payload["session_id"], session_id.to_string());
}

/// `authorise_resume` moves `blocked → queued`, which design §11 counts as a
/// material status transition and therefore something the log must carry.
///
/// It did not, at first. The audit log was wired before the vocabulary had a
/// type for this edge, so the transition an operator most wants to find later
/// — a human authorising one further delivery after a restart — was the one
/// the log did not record. `resumed` was added for it.
///
/// The author is NULL because this edge has no session author by
/// construction: a human authorises it, per ADR 0019.
///
/// Compared before and after rather than against an absolute count, because
/// [`seed_blocked_task`] itself runs the task through `deliver`,
/// `mark_running` and `complete::blocked`, each already writing its own
/// event, before `authorise_resume` ever sees it.
#[test]
fn authorise_resume_writes_one_resumed_event_authored_by_no_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_blocked_task(&mut store, 3, scope_id, session_id, "do it");

    let before = for_task(&store, task_id).expect("read events").len();

    authorise_resume(&mut store, task_id).expect("resume a blocked task");

    let after = for_task(&store, task_id).expect("read events");
    assert_eq!(
        after.len(),
        before + 1,
        "authorise_resume must add exactly one event"
    );
    let last = after.last().expect("at least one event");
    assert_eq!(last.event_type, EventType::Resumed);
    assert_eq!(
        last.author_session_id, None,
        "a human authorises a resume, so no session authored it"
    );
    assert_eq!(
        last.payload, None,
        "the edge itself is the whole fact; there is nothing else to carry"
    );
}
