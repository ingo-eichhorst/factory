//! `factory_task::assign`'s own test suite (design §2.4, §5; backlog §7).
//!
//! Every fixture below is duplicated from `tests/create.rs` rather than
//! shared through a `tests/common/mod.rs`: this task's file list is exactly
//! `crates/factory-task/src/assign.rs` and `crates/factory-task/tests/assign.rs`
//! (new), and each integration-test file already compiles as its own crate,
//! so a shared module would be a third file this task does not own creating.

use std::path::PathBuf;

use factory_paths::CanonicalPath;
use factory_store::Store;
use factory_task::TaskStatus;
use factory_task::assign::{AssignError, Assignment, DeferReason, assign};
use factory_task::create::{create, show};

/// A deterministic, distinct, syntactically valid UUID — mirrors
/// `tests/create.rs::uid` and `factory-session/tests/common/mod.rs::uid`.
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

/// Start a fresh session (left `starting`, the state a lease-holding but
/// not-yet-idle session needs) in a fresh workspace directory under `dir`
/// named uniquely by `seed`. Returns its id and canonical workspace path.
fn start_session(
    store: &mut Store,
    dir: &std::path::Path,
    seed: u32,
    scope_id: uuid::Uuid,
    agent_name: &str,
    max_sessions: u32,
) -> (uuid::Uuid, PathBuf) {
    let session_id = uid(seed);
    let workspace_dir = dir.join(format!("workspace-{seed}"));
    std::fs::create_dir(&workspace_dir).expect("create workspace directory");
    let workspace = CanonicalPath::resolve(&workspace_dir).expect("resolve workspace");
    factory_session::begin_start(
        store,
        session_id,
        scope_id,
        agent_name,
        max_sessions,
        &workspace,
    )
    .expect("start session");
    (session_id, workspace.into_path_buf())
}

/// Start and mark `running` a fresh session — an idle session, so long as no
/// task is assigned to it.
fn seed_idle_session(
    store: &mut Store,
    dir: &std::path::Path,
    seed: u32,
    scope_id: uuid::Uuid,
    agent_name: &str,
    max_sessions: u32,
) -> (uuid::Uuid, PathBuf) {
    let (session_id, path) = start_session(store, dir, seed, scope_id, agent_name, max_sessions);
    factory_session::mark_running(store, session_id).expect("readiness observed");
    (session_id, path)
}

/// Seed a task directly, bypassing `create` (which only ever writes `queued`
/// with no `assigned_session_id`), so a fixture can set `status` and
/// `assigned_session_id` (and, for `blocked`, a reason) to whatever a test
/// needs — mirrors `tests/create.rs`'s own `seed_running_task` /
/// `seed_blocked_task`.
#[allow(clippy::too_many_arguments)]
fn seed_task(
    store: &mut Store,
    seed: u32,
    scope_id: uuid::Uuid,
    assigned_session_id: Option<uuid::Uuid>,
    status: &str,
    blocked_reason: Option<&str>,
) -> uuid::Uuid {
    let task_id = uid(seed);
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO tasks (id, target_scope_id, assigned_session_id, prompt, status, blocked_reason) \
         VALUES (?1, ?2, ?3, 'do it', ?4, ?5)",
        (
            task_id.to_string(),
            scope_id.to_string(),
            assigned_session_id.map(|s| s.to_string()),
            status,
            blocked_reason,
        ),
    )
    .expect("insert task");
    tx.commit().expect("commit");
    task_id
}

// Untargeted -----------------------------------------------------------

#[test]
fn untargeted_assign_picks_the_only_idle_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let (session_id, _) = seed_idle_session(&mut store, dir.path(), 2, scope_id, "agent", 10);

    let task_id = uid(50);
    create(&mut store, task_id, None, scope_id, None, None, "do it").expect("create");

    let outcome = assign(&mut store, task_id, "agent", 10).expect("assign");
    assert_eq!(outcome, Assignment::Assigned(session_id));

    let task = show(&store, task_id).expect("show");
    assert_eq!(task.assigned_session_id, Some(session_id));
    assert_eq!(
        task.status,
        TaskStatus::Queued,
        "assign never sets status to running — delivery owns that"
    );
}

#[test]
fn untargeted_assign_defers_when_no_session_exists_at_all() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");

    let task_id = uid(50);
    create(&mut store, task_id, None, scope_id, None, None, "do it").expect("create");

    let outcome = assign(&mut store, task_id, "agent", 10).expect("assign");
    assert_eq!(
        outcome,
        Assignment::Deferred(DeferReason::NoIdleSession {
            agent_name: "agent".to_string()
        })
    );

    let task = show(&store, task_id).expect("show");
    assert_eq!(task.assigned_session_id, None);
    assert_eq!(task.status, TaskStatus::Queued);
}

/// Mutation 1's target: widening `is_idle`'s state check from `== running` to
/// `holds_lease()` would let this `starting` session — which holds its
/// workspace lease but has never been confirmed ready — absorb the task.
/// With the predicate as written, it must not: the only session that exists
/// is not idle, so the task stays deferred.
#[test]
fn a_starting_session_holds_the_lease_but_is_not_idle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    // Left `starting` deliberately: no `mark_running` call.
    let _ = start_session(&mut store, dir.path(), 2, scope_id, "agent", 10);

    let task_id = uid(50);
    create(&mut store, task_id, None, scope_id, None, None, "do it").expect("create");

    let outcome = assign(&mut store, task_id, "agent", 10).expect("assign");
    assert_eq!(
        outcome,
        Assignment::Deferred(DeferReason::NoIdleSession {
            agent_name: "agent".to_string()
        }),
        "a `starting` session must not be treated as idle"
    );
}

/// Mutation 2's target, specifically with a `blocked` task (per the task
/// brief: "it must be a test about a `blocked` task, not only about a
/// `running` one"). The session itself is `running` — only the
/// no-non-terminal-task half of the idle predicate is what must reject it.
#[test]
fn a_session_whose_task_is_blocked_is_not_idle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let (session_id, _) = seed_idle_session(&mut store, dir.path(), 2, scope_id, "agent", 10);
    seed_task(
        &mut store,
        20,
        scope_id,
        Some(session_id),
        "blocked",
        Some("clarification"),
    );

    let task_id = uid(50);
    create(&mut store, task_id, None, scope_id, None, None, "do it").expect("create");

    let outcome = assign(&mut store, task_id, "agent", 10).expect("assign");
    assert_eq!(
        outcome,
        Assignment::Deferred(DeferReason::NoIdleSession {
            agent_name: "agent".to_string()
        }),
        "`blocked` is not terminal — a session whose task is blocked is not idle"
    );
}

/// The database's `tasks_one_running_per_session` unique index only covers
/// `status = 'running'` rows, so nothing in the schema stops a second
/// `assign` call from choosing the same session while the first assignment
/// is still sitting `queued`, undelivered. The in-transaction idle check is
/// the only thing that prevents it — this is the direct evidence.
#[test]
fn a_second_untargeted_assign_does_not_double_book_the_only_idle_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let (session_id, _) = seed_idle_session(&mut store, dir.path(), 2, scope_id, "agent", 10);

    let task_a = uid(50);
    let task_b = uid(51);
    create(&mut store, task_a, None, scope_id, None, None, "a").expect("create a");
    create(&mut store, task_b, None, scope_id, None, None, "b").expect("create b");

    let outcome_a = assign(&mut store, task_a, "agent", 10).expect("assign a");
    assert_eq!(outcome_a, Assignment::Assigned(session_id));

    let outcome_b = assign(&mut store, task_b, "agent", 10).expect("assign b");
    assert_eq!(
        outcome_b,
        Assignment::Deferred(DeferReason::NoIdleSession {
            agent_name: "agent".to_string()
        }),
        "the session is now busy with task_a, still only `queued` — not `running`"
    );

    assert_eq!(
        show(&store, task_a).expect("show a").assigned_session_id,
        Some(session_id)
    );
    assert_eq!(
        show(&store, task_b).expect("show b").assigned_session_id,
        None
    );
}

/// Selection among several idle sessions is deterministic: `rowid` order
/// (insertion order), lowest wins.
#[test]
fn untargeted_assign_picks_the_earliest_created_idle_session_deterministically() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let (first, _) = seed_idle_session(&mut store, dir.path(), 2, scope_id, "agent", 10);
    let (_second, _) = seed_idle_session(&mut store, dir.path(), 3, scope_id, "agent", 10);

    let task_id = uid(50);
    create(&mut store, task_id, None, scope_id, None, None, "do it").expect("create");

    let outcome = assign(&mut store, task_id, "agent", 10).expect("assign");
    assert_eq!(outcome, Assignment::Assigned(first));
}

// Requested session ------------------------------------------------------

#[test]
fn requested_session_is_used_when_idle_even_if_another_idle_session_exists() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let (_other_idle, _) = seed_idle_session(&mut store, dir.path(), 2, scope_id, "agent", 10);
    let (requested, _) = seed_idle_session(&mut store, dir.path(), 3, scope_id, "agent", 10);

    let task_id = uid(50);
    create(
        &mut store,
        task_id,
        None,
        scope_id,
        Some(requested),
        None,
        "do it",
    )
    .expect("create");

    let outcome = assign(&mut store, task_id, "agent", 10).expect("assign");
    assert_eq!(
        outcome,
        Assignment::Assigned(requested),
        "the sender asked for this session; assign must never silently substitute another"
    );
}

#[test]
fn requested_session_that_is_busy_is_deferred_not_substituted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let (other_idle, _) = seed_idle_session(&mut store, dir.path(), 2, scope_id, "agent", 10);
    let (requested, _) = seed_idle_session(&mut store, dir.path(), 3, scope_id, "agent", 10);
    seed_task(&mut store, 20, scope_id, Some(requested), "running", None);

    let task_id = uid(50);
    create(
        &mut store,
        task_id,
        None,
        scope_id,
        Some(requested),
        None,
        "do it",
    )
    .expect("create");

    let outcome = assign(&mut store, task_id, "agent", 10).expect("assign");
    assert_eq!(
        outcome,
        Assignment::Deferred(DeferReason::SessionNotIdle {
            session_id: requested,
            state: "running".to_string(),
        }),
        "must defer, and must not fall back to the other idle session ({other_idle})"
    );
}

/// `DeferReason::RequestedSessionNotFound` exists in `assign.rs` for
/// defence in depth — `assign_requested_session` re-reads the row rather
/// than assuming it is there, the same "never trust a bare lookup" stance
/// `factory_session::on_task_terminal` takes toward `tasks.status` — but it
/// is unreachable through this crate's own writing surface: `create` was
/// first tried here with a `target_session_id` that names no real session,
/// expecting `assign` to report `RequestedSessionNotFound`. It cannot: the
/// schema's own `target_session_id TEXT REFERENCES sessions (id)` foreign
/// key, enforced on every connection (`PRAGMA foreign_keys = ON`, ADR 0012
/// decision 3), rejects the `create` call itself, before `assign` ever runs.
/// This is the direct evidence, and arguably a stronger guarantee than a
/// unit test of the defensive branch would have been: a task can never
/// durably request a session that was never real in the first place. Session
/// rows are also never deleted once created (`factory_session`'s module
/// docs: "the session record is removed" means `Failed`, never
/// `DELETE FROM sessions`), so a *previously* valid `target_session_id`
/// cannot go missing later either — every way a requested session becomes
/// unusable narrows to a `state` change, which
/// `requested_session_that_is_busy_is_deferred_not_substituted` and its
/// siblings already cover via `DeferReason::SessionNotIdle`.
#[test]
fn create_rejects_a_target_session_id_that_names_no_real_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let missing = uid(99);

    let task_id = uid(50);
    let err = create(
        &mut store,
        task_id,
        None,
        scope_id,
        Some(missing),
        None,
        "do it",
    )
    .expect_err("a task cannot durably request a session that was never real");
    assert!(
        matches!(err, factory_task::TaskError::Store(_)),
        "expected the schema's own foreign key to reject this, got {err:?}"
    );
}

// Requested workspace ----------------------------------------------------

#[test]
fn requested_workspace_reuses_its_idle_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let (session_id, path) = seed_idle_session(&mut store, dir.path(), 2, scope_id, "agent", 10);

    let task_id = uid(50);
    create(
        &mut store,
        task_id,
        None,
        scope_id,
        None,
        Some(path.to_str().expect("utf8 path")),
        "do it",
    )
    .expect("create");

    let outcome = assign(&mut store, task_id, "agent", 10).expect("assign");
    assert_eq!(outcome, Assignment::Assigned(session_id));
}

#[test]
fn requested_workspace_with_a_busy_occupant_is_deferred() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let (session_id, path) = seed_idle_session(&mut store, dir.path(), 2, scope_id, "agent", 10);
    seed_task(&mut store, 20, scope_id, Some(session_id), "running", None);

    let task_id = uid(50);
    create(
        &mut store,
        task_id,
        None,
        scope_id,
        None,
        Some(path.to_str().expect("utf8 path")),
        "do it",
    )
    .expect("create");

    let outcome = assign(&mut store, task_id, "agent", 10).expect("assign");
    assert_eq!(
        outcome,
        Assignment::Deferred(DeferReason::SessionNotIdle {
            session_id,
            state: "running".to_string(),
        })
    );
}

/// Found by mutating `SessionState::holds_lease` during acceptance review:
/// dropping `Starting` from the lease-holding set broke four tests in
/// `factory-session` and none here, even though this path is the one that
/// hands out a *new* session for a workspace.
///
/// A `starting` occupant is not busy the way the test above is busy — it has
/// no task at all. It is a launch in progress, and ADR 0012 decision 5 makes
/// it hold the workspace lease for exactly this reason: if `assign` read the
/// workspace as unoccupied it would answer `StartSessionAt`, and a caller
/// acting on that answer puts a second harness in a directory the first one
/// is already opening. That is the corruption the lease exists to prevent,
/// reached through a door the lease's own tests do not watch.
#[test]
fn a_workspace_whose_occupant_is_still_starting_is_not_offered_for_a_new_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    // Left `starting` deliberately: no `mark_running` call, and no task.
    let (session_id, path) = start_session(&mut store, dir.path(), 2, scope_id, "agent", 10);

    let task_id = uid(50);
    create(
        &mut store,
        task_id,
        None,
        scope_id,
        None,
        Some(path.to_str().expect("utf8 path")),
        "do it",
    )
    .expect("create");

    let outcome = assign(&mut store, task_id, "agent", 10).expect("assign");
    assert_eq!(
        outcome,
        Assignment::Deferred(DeferReason::SessionNotIdle {
            session_id,
            state: "starting".to_string(),
        }),
        "a workspace held by a starting session must never be offered as free"
    );
}

/// Found during acceptance review: `sessions_one_live_lease_per_workspace`
/// indexes `workspace_path` alone, so a lease is global, but the lookup
/// filtered by scope as well and reported another scope's workspace as free.
/// `assign` then answered `StartSessionAt` for a directory where starting is
/// impossible — the unique index would have refused it. Nothing could have
/// corrupted a directory; the defect was an answer a caller cannot act on.
#[test]
fn a_workspace_leased_by_another_scope_is_never_offered_as_free() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_a = seed_scope(&mut store, 1, "irrlicht", "/instance/a");
    let scope_b = seed_scope(&mut store, 2, "model-lab", "/instance/b");
    let (holder, path) = seed_idle_session(&mut store, dir.path(), 3, scope_a, "agent", 10);

    let task_id = uid(50);
    create(
        &mut store,
        task_id,
        None,
        scope_b,
        None,
        Some(path.to_str().expect("utf8 path")),
        "do it",
    )
    .expect("create");

    let outcome = assign(&mut store, task_id, "agent", 10).expect("assign");
    assert_eq!(
        outcome,
        Assignment::Deferred(DeferReason::WorkspaceLeasedByAnotherScope {
            session_id: holder,
            scope_id: scope_a,
        }),
        "a lease is per workspace path, not per scope"
    );
}

/// The second half of the `max_sessions` de-duplication, and the reason it
/// needs its own test: the capacity fixture below it marks its session
/// `running`, so it never exercises a `starting` session against the bound.
/// A `starting` session is precisely the case ADR 0012 decision 5 exists for
/// — recorded before launch, so two concurrent starts cannot both pass the
/// check — and without this, `count_live_sessions` could regress to a
/// private copy of the rule and nothing here would notice.
#[test]
fn a_starting_session_still_spends_a_max_sessions_slot() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    // Left `starting` deliberately, at a different workspace than the one the
    // task asks for, so only the capacity check can reject this.
    let _ = start_session(&mut store, dir.path(), 2, scope_id, "agent", 1);

    let workspace_dir = dir.path().join("fresh-workspace");
    std::fs::create_dir(&workspace_dir).expect("create workspace directory");

    let task_id = uid(50);
    create(
        &mut store,
        task_id,
        None,
        scope_id,
        None,
        Some(workspace_dir.to_str().expect("utf8 path")),
        "do it",
    )
    .expect("create");

    let outcome = assign(&mut store, task_id, "agent", 1).expect("assign");
    assert_eq!(
        outcome,
        Assignment::Deferred(DeferReason::AgentAtCapacity {
            agent_name: "agent".to_string(),
            max_sessions: 1,
        }),
        "a launch in progress already occupies its agent's slot"
    );
}

#[test]
fn requested_workspace_that_does_not_exist_is_deferred_not_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let missing_path = dir.path().join("never-created");

    let task_id = uid(50);
    create(
        &mut store,
        task_id,
        None,
        scope_id,
        None,
        Some(missing_path.to_str().expect("utf8 path")),
        "do it",
    )
    .expect("create");

    let outcome = assign(&mut store, task_id, "agent", 10).expect("assign");
    // `canonicalize` may rewrite the path (e.g. resolving `/tmp` to
    // `/private/tmp` on macOS) before failing to find it, so compare only
    // that the deferral fires, not the exact `PathBuf`.
    assert!(
        matches!(
            outcome,
            Assignment::Deferred(DeferReason::WorkspaceNotFound { .. })
        ),
        "expected WorkspaceNotFound, got {outcome:?}"
    );
}

#[test]
fn requested_workspace_with_no_occupant_and_room_starts_a_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let workspace_dir = dir.path().join("fresh-workspace");
    std::fs::create_dir(&workspace_dir).expect("create workspace directory");
    let canonical = CanonicalPath::resolve(&workspace_dir).expect("resolve");

    let task_id = uid(50);
    create(
        &mut store,
        task_id,
        None,
        scope_id,
        None,
        Some(workspace_dir.to_str().expect("utf8 path")),
        "do it",
    )
    .expect("create");

    let outcome = assign(&mut store, task_id, "agent", 5).expect("assign");
    assert_eq!(
        outcome,
        Assignment::StartSessionAt(canonical.into_path_buf())
    );

    let task = show(&store, task_id).expect("show");
    assert_eq!(
        task.assigned_session_id, None,
        "StartSessionAt writes nothing — assign never starts a session itself"
    );
    assert_eq!(task.status, TaskStatus::Queued);
}

/// Mutation 4's target: the `max_sessions` comparison must be strict `<`.
/// One session already lives at a *different* workspace than the one this
/// task requests, so the requested workspace has no occupant to reuse — this
/// isolates the capacity check itself, not workspace reuse.
#[test]
fn requested_workspace_defers_when_the_agent_is_already_at_max_sessions() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let _existing = seed_idle_session(&mut store, dir.path(), 2, scope_id, "agent", 1);

    let new_workspace = dir.path().join("second-workspace");
    std::fs::create_dir(&new_workspace).expect("create workspace directory");

    let task_id = uid(50);
    create(
        &mut store,
        task_id,
        None,
        scope_id,
        None,
        Some(new_workspace.to_str().expect("utf8 path")),
        "do it",
    )
    .expect("create");

    let outcome = assign(&mut store, task_id, "agent", 1).expect("assign");
    assert_eq!(
        outcome,
        Assignment::Deferred(DeferReason::AgentAtCapacity {
            agent_name: "agent".to_string(),
            max_sessions: 1,
        })
    );
}

// Precedence when both are requested -------------------------------------

/// Nothing in the schema forbids a task from carrying both
/// `target_session_id` and `target_workspace_path`; `assign` documents that
/// the session wins as the more specific ask. Session and workspace point at
/// two different sessions here so the outcome is unambiguous about which
/// path was actually taken.
#[test]
fn a_requested_session_takes_precedence_over_a_requested_workspace() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let (workspace_occupant, workspace_path) =
        seed_idle_session(&mut store, dir.path(), 2, scope_id, "agent", 10);
    let (requested_session, _) =
        seed_idle_session(&mut store, dir.path(), 3, scope_id, "agent", 10);

    let task_id = uid(50);
    create(
        &mut store,
        task_id,
        None,
        scope_id,
        Some(requested_session),
        Some(workspace_path.to_str().expect("utf8 path")),
        "do it",
    )
    .expect("create");

    let outcome = assign(&mut store, task_id, "agent", 10).expect("assign");
    assert_eq!(
        outcome,
        Assignment::Assigned(requested_session),
        "session ({requested_session}) must win over workspace occupant ({workspace_occupant})"
    );
}

// Status guard and not-found ----------------------------------------------

/// Mutation 3's target: `assign` must refuse a task that is not `queued`.
/// Setup makes the two possible outcomes unambiguous: the task's own
/// (already-`running`) session is busy — because this very task is assigned
/// to it — but a second, genuinely idle session of the same agent exists.
/// Removing the queued-guard would fall through to the untargeted search,
/// find the second session idle, and silently reassign the running task to
/// it; the guard's job is to refuse before any of that runs.
#[test]
fn assign_refuses_a_task_that_is_not_queued() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let (busy_session, _) = seed_idle_session(&mut store, dir.path(), 2, scope_id, "agent", 10);
    let (_other_idle, _) = seed_idle_session(&mut store, dir.path(), 3, scope_id, "agent", 10);
    let task_id = seed_task(
        &mut store,
        50,
        scope_id,
        Some(busy_session),
        "running",
        None,
    );

    let err =
        assign(&mut store, task_id, "agent", 10).expect_err("a running task cannot be assigned");
    assert!(matches!(
        err,
        AssignError::NotQueued {
            id,
            status: TaskStatus::Running
        } if id == task_id
    ));

    let task = show(&store, task_id).expect("show");
    assert_eq!(
        task.assigned_session_id,
        Some(busy_session),
        "a refused assign must not touch the row"
    );
}

#[test]
fn assign_of_a_blocked_task_is_also_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let task_id = seed_task(
        &mut store,
        50,
        scope_id,
        None,
        "blocked",
        Some("clarification"),
    );

    let err =
        assign(&mut store, task_id, "agent", 10).expect_err("a blocked task cannot be assigned");
    assert!(matches!(
        err,
        AssignError::NotQueued {
            id,
            status: TaskStatus::Blocked
        } if id == task_id
    ));
}

#[test]
fn assign_of_a_nonexistent_task_is_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let err = assign(&mut store, uid(999), "agent", 10).expect_err("no such task exists");
    assert!(matches!(err, AssignError::TaskNotFound(id) if id == uid(999)));
}
