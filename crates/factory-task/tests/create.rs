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
use factory_task::create::{cancel, create, create_from_template, delegation_chain_of, list, show};
use factory_task::events::{EventType, for_task};
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

/// Insert a `task_templates` row directly, bypassing `factory_task::template`
/// (a sibling module this file does not otherwise depend on) so this file's
/// own tests of `create_from_template` stay self-contained the same way its
/// other fixtures bypass `factory-registry` and `factory-session`'s own
/// creation paths. Returns the template's id.
fn seed_template(store: &mut Store, seed: u32, name: &str, version: i64) -> uuid::Uuid {
    let id = uid(seed);
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO task_templates (id, name, prompt, version) VALUES (?1, ?2, 'do it', ?3)",
        (id.to_string(), name, version),
    )
    .expect("insert template");
    tx.commit().expect("commit");
    id
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
        &[],
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
    assert_eq!(
        task.template_id, None,
        "create never records a template — that is create_from_template's job"
    );
    assert_eq!(task.template_version, None);
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
        &[],
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
        &[],
    )
    .expect("create with a sender scope");

    let task = show(&store, task_id).expect("show");
    assert_eq!(task.sender_scope_id, Some(sender));
    assert_eq!(task.target_scope_id, target);
}

// create_from_template -------------------------------------------------

/// ADR 0021 decision 2, exercised directly at the `create` level rather than
/// through `template::revise` (that round trip lives in `tests/template.rs`):
/// the version recorded on the run is whatever `task_templates.version` held
/// at the moment of this INSERT. Seeding the template at version 5 here,
/// rather than 1, is deliberate — it is the version a fresh template could
/// never have on its own, so a test that only checked for `Some(1)` could
/// pass by accident (e.g. a bug that always wrote 1) without this failing.
#[test]
fn create_from_template_records_the_template_id_and_its_version_at_creation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let template_id = seed_template(&mut store, 2, "nightly-report", 5);

    let task_id = uid(50);
    create_from_template(
        &mut store,
        task_id,
        None,
        scope_id,
        None,
        None,
        "do it",
        &[scope_id],
        template_id,
    )
    .expect("create a run from the template");

    let task = show(&store, task_id).expect("show");
    assert_eq!(task.template_id, Some(template_id));
    assert_eq!(task.template_version, Some(5));
    assert_eq!(
        task.status,
        TaskStatus::Queued,
        "a template-backed run still starts out queued, same as any other"
    );
}

/// A `template_id` naming no real template is refused by
/// `tasks.template_id REFERENCES task_templates (id)` — the identical
/// backstop shape `a_failed_chain_insert_leaves_no_task_row_at_all` below
/// already proves for a chain entry naming an unregistered scope. Atomicity
/// holds the same way: no task row survives.
#[test]
fn create_from_template_rejects_a_template_id_that_names_no_real_template() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let never_registered = uid(999);

    let task_id = uid(50);
    let err = create_from_template(
        &mut store,
        task_id,
        None,
        scope_id,
        None,
        None,
        "do it",
        &[scope_id],
        never_registered,
    )
    .expect_err("a nonexistent template_id must be refused");
    assert!(
        matches!(err, TaskError::Store(_)),
        "expected the schema's own foreign key to reject the insert, got {err:?}"
    );

    let result = show(&store, task_id);
    assert!(
        matches!(result, Err(TaskError::NotFound(id)) if id == task_id),
        "the task row must not survive a failed template_id reference"
    );
}

// delegation_chain_of -------------------------------------------------------

#[test]
fn delegation_chain_of_an_empty_chain_is_empty() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
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
    .expect("create with an empty chain");

    let chain = delegation_chain_of(&store, task_id).expect("read chain");
    assert_eq!(chain, Vec::<uuid::Uuid>::new());
}

/// Round trip through `create` and back: a three-scope chain, 0-based with
/// the target last (per `delegation_chain_of`'s own doc comment), comes
/// back in exactly the order it was written.
///
/// The three scope ids are deliberately seeded so their lexicographic
/// (string) order — `hop_b (10) < target (20) < hop_a (30)` — differs from
/// their position order — `hop_a, hop_b, target`. This is deliberate, not
/// incidental: `delegation_chain_of`'s doc comment records that, without
/// `ORDER BY position`, SQLite answers this exact query (only `scope_id`
/// selected, filtered by `task_id`) from the `UNIQUE (task_id, scope_id)`
/// covering index instead of the `(task_id, position)` primary key, which
/// returns rows in `scope_id`'s lexicographic order. A chain whose scopes
/// happen to sort the same way they are positioned would pass with or
/// without that clause; this one cannot.
#[test]
fn delegation_chain_of_round_trips_a_three_scope_chain_in_position_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let hop_a = seed_scope(&mut store, 30, "hop-a", "/instance/hop-a");
    let hop_b = seed_scope(&mut store, 10, "hop-b", "/instance/hop-b");
    let target = seed_scope(&mut store, 20, "target", "/instance/target");
    let chain = [hop_a, hop_b, target];

    let task_id = uid(50);
    create(
        &mut store, task_id, None, target, None, None, "do it", &chain,
    )
    .expect("create with a three-scope chain");

    let read_back = delegation_chain_of(&store, task_id).expect("read chain");
    assert_eq!(
        read_back,
        vec![hop_a, hop_b, target],
        "must come back in position order, 0-based with the target last"
    );
}

/// Backlog §8: "the chain is recorded durably with the task, so a
/// delegation loop is reconstructable after a restart rather than only
/// detectable while running." A chain committed in a transaction separate
/// from the task row could, after a crash between the two commits, be read
/// back as a task that had travelled through no scope at all — this is the
/// direct proof that cannot happen. `create` writes the chain inside the
/// same transaction as the task row (see its own doc comment); forcing the
/// chain insert to fail — a chain entry naming a scope that was never
/// registered, so `task_delegation_chain.scope_id REFERENCES scopes (id)`
/// rejects it, the same defense-in-depth shape
/// `create_rejects_a_target_session_id_that_names_no_real_session` in
/// `tests/assign.rs` already proves works for `target_session_id` — must
/// leave **no task row at all**, not a task row with a partial or empty
/// chain.
///
/// Mutation target: move the chain-insert loop after `tx.commit()`, into
/// its own transaction. With that change the task row survives this exact
/// failure and the `show` assertion below dies.
#[test]
fn a_failed_chain_insert_leaves_no_task_row_at_all() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let target = seed_scope(&mut store, 1, "target", "/instance/target");
    let never_registered = uid(999);

    let task_id = uid(50);
    let err = create(
        &mut store,
        task_id,
        None,
        target,
        None,
        None,
        "do it",
        &[never_registered],
    )
    .expect_err("a chain entry naming a nonexistent scope must be refused");
    assert!(
        matches!(err, TaskError::Store(_)),
        "expected the schema's own foreign key to reject the chain row, got {err:?}"
    );

    let result = show(&store, task_id);
    assert!(
        matches!(result, Err(TaskError::NotFound(id)) if id == task_id),
        "the task row must not survive a failed chain insert — got {result:?}"
    );
}

/// `task_delegation_chain`'s `UNIQUE (task_id, scope_id)` is the database's
/// own last word on a looping chain (design §6: "a scope already in that
/// chain cannot be targeted again"; backlog §8: "[a] two-step cycle
/// (`A → B → A`)... [is] refused"). This is deliberately a *backstop* — the
/// typed refusal is `factory_delegation`'s job, a crate this one does not
/// depend on and cannot call into — so this test documents the database
/// constraint as exactly that: a backstop, not the primary enforcement.
/// `A → B → A` is modelled directly: `A` appears at position 0 and again at
/// position 2 (the target), which is precisely what re-targeting an
/// already-visited scope looks like on the wire.
#[test]
fn a_chain_that_targets_an_already_visited_scope_is_refused_by_the_database_uniqueness_backstop() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_a = seed_scope(&mut store, 1, "a", "/instance/a");
    let scope_b = seed_scope(&mut store, 2, "b", "/instance/b");

    let task_id = uid(50);
    let err = create(
        &mut store,
        task_id,
        None,
        scope_a,
        None,
        None,
        "do it",
        &[scope_a, scope_b, scope_a],
    )
    .expect_err("re-targeting an already-visited scope must be refused");
    assert!(
        matches!(err, TaskError::Store(_)),
        "expected the schema's UNIQUE (task_id, scope_id) to reject the repeat, got {err:?}"
    );

    let result = show(&store, task_id);
    assert!(
        matches!(result, Err(TaskError::NotFound(id)) if id == task_id),
        "atomicity holds here too: no task row should survive"
    );
}

// list / show --------------------------------------------------------------

#[test]
fn list_returns_every_task_oldest_first() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");

    let first = uid(10);
    let second = uid(11);
    create(&mut store, first, None, scope_id, None, None, "first", &[]).expect("create first");
    create(
        &mut store,
        second,
        None,
        scope_id,
        None,
        None,
        "second",
        &[],
    )
    .expect("create second");

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

// events (station 11, `crate`'s own decision 3) ---------------------------

/// `create` writes exactly one `created` event, with no payload and no
/// author session — the row itself already carries every fact about the
/// creation, and `prompt` is exactly what decision 6 forbids from a payload.
#[test]
fn create_writes_exactly_one_created_event_with_no_payload_and_no_author() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = uid(20);
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

    let events = for_task(&store, task_id).expect("read events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, EventType::Created);
    assert_eq!(events[0].author_session_id, None);
    assert_eq!(events[0].payload, None);
}

/// `create_from_template` reaches the same one-INSERT path as `create` (both
/// call the private `insert_task` helper), so it must write the identical
/// `created` event, not a second kind, and not zero.
#[test]
fn create_from_template_also_writes_exactly_one_created_event() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let template_id = seed_template(&mut store, 2, "nightly-report", 1);
    let task_id = uid(50);
    create_from_template(
        &mut store,
        task_id,
        None,
        scope_id,
        None,
        None,
        "do it",
        &[],
        template_id,
    )
    .expect("create a run from the template");

    let events = for_task(&store, task_id).expect("read events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, EventType::Created);
}

/// `cancel`'s immediate path (`queued` → `cancelled`) writes the matching
/// event, in the same transaction as the status write.
#[test]
fn cancel_of_a_queued_task_writes_exactly_one_cancelled_event() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = uid(20);
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

    cancel(&mut store, task_id).expect("cancel a queued task");

    let events = for_task(&store, task_id).expect("read events");
    assert_eq!(events.len(), 2, "created, then cancelled");
    assert_eq!(events[0].event_type, EventType::Created);
    assert_eq!(events[1].event_type, EventType::Cancelled);
    assert_eq!(events[1].payload, None);
}

/// `cancel`'s cooperative path on a `running` task changes no `tasks.status`
/// — it only records `cancel_requested_at` — and there is no
/// `cancel_requested` entry in the schema's twelve event types. This is the
/// regression guard against inventing one, or against reusing `cancelled` for
/// a request that has not actually moved the task there yet.
#[test]
fn cancel_of_a_running_task_writes_no_event() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let session_id = seed_running_session(&mut store, dir.path(), 2, scope_id);
    let task_id = seed_running_task(&mut store, 22, scope_id, session_id);

    cancel(&mut store, task_id).expect("cancel a running task");

    let events = for_task(&store, task_id).expect("read events");
    assert_eq!(
        events.len(),
        0,
        "this task was seeded directly by raw SQL, not through `create`, so it \
         starts with no events, and the cooperative request must add none"
    );
}

/// The mutation this test catches: a refused cancel (already terminal) must
/// leave no additional event behind. `cancel` returns
/// `TaskError::AlreadyTerminal` before it ever reaches a write.
#[test]
fn cancel_of_a_terminal_task_writes_no_additional_event() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = seed_terminal_task(&mut store, 23, scope_id, "done");

    cancel(&mut store, task_id).expect_err("a terminal task cannot be cancelled");

    let events = for_task(&store, task_id).expect("read events");
    assert_eq!(
        events.len(),
        0,
        "this task was seeded directly by raw SQL, not through `create`, so it \
         starts with no events, and the refused cancel must add none"
    );
}
