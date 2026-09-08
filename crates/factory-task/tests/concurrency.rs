//! Backlog §8's two concurrency demonstrations: "[t]wo sessions for one
//! scope can run separate tasks concurrently without shared workspace
//! leases or prompt multiplexing," and separately, two agents of one scope
//! running concurrently.
//!
//! **This is a demonstration, not new machinery.** Everything asserted here
//! is what Slice 6 (workspace leases, `count_live_sessions`,
//! `tasks_one_running_per_session`) and station 8's own `assign` /
//! `running_task_of_session` already built — this file wires the existing
//! public API (`create` → `assign` → `deliver` → `mark_running`) for two
//! sessions at once and reads the data-visible result back. No scheduler, no
//! runtime, and no threads: both "concurrent" tasks are driven from one
//! thread, one call at a time, exactly the way a real Factory supervisor
//! would drive them from its own single-writer event loop. What is proved is
//! that nothing in the schema or this crate's code forces the two tasks onto
//! the same session or the same lease — not that the calls literally overlap
//! in wall-clock time.
//!
//! # A note on the fixtures below
//!
//! Duplicated from `tests/assign.rs` and `tests/deliver.rs` rather than
//! shared through a `tests/common/mod.rs`, for the same reason those two
//! files give for duplicating from each other: each integration-test file
//! already compiles as its own crate, and a shared module would be a third,
//! fourth file this task does not own creating.

use std::path::PathBuf;

use factory_paths::CanonicalPath;
use factory_store::Store;
use factory_task::TaskStatus;
use factory_task::assign::{Assignment, assign, running_task_of_session};
use factory_task::create::{create, show};
use factory_task::deliver::{PromptWriteError, PromptWriter, deliver, mark_running};

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

/// Start and mark `running` a fresh session — an idle session, so long as
/// no task is assigned to it — in a fresh workspace directory under `dir`
/// named uniquely by `seed`. Returns its id and canonical workspace path.
/// Mirrors `tests/assign.rs::seed_idle_session`.
fn seed_idle_session(
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
    factory_session::mark_running(store, session_id).expect("readiness observed");
    (session_id, workspace.into_path_buf())
}

/// A writer that always succeeds and records exactly which session each
/// prompt went to — the data-visible proof of "no prompt multiplexing":
/// each recorded prompt carries its own task UUID (`deliver::render_prompt`
/// puts it there) and is addressed to exactly one session.
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

/// Whether `session_id` currently holds a live workspace lease
/// (`released_at IS NULL` — `workspace_leases`'s own doc comment: this
/// journal, not a second "active" flag, is what a caller consults for
/// history, but the live-or-not question for one known session reduces to
/// exactly this column).
fn holds_a_live_lease(store: &Store, session_id: uuid::Uuid) -> bool {
    let released_at: Option<String> = store
        .connection()
        .query_row(
            "SELECT released_at FROM workspace_leases WHERE session_id = ?1",
            [session_id.to_string()],
            |row| row.get(0),
        )
        .expect("lease row exists");
    released_at.is_none()
}

/// Run a task all the way from `queued` to `running`, the same three-call
/// sequence a real caller makes: `assign`, then `deliver`, then
/// `mark_running`. Returns the session it landed on.
fn assign_deliver_and_run(
    store: &mut Store,
    task_id: uuid::Uuid,
    agent_name: &str,
    max_sessions: u32,
    writer: &mut RecordingWriter,
) -> uuid::Uuid {
    let outcome = assign(store, task_id, agent_name, max_sessions).expect("assign");
    let Assignment::Assigned(session_id) = outcome else {
        panic!("expected an idle session to be assigned, got {outcome:?}");
    };
    deliver(store, task_id, writer).expect("deliver");
    mark_running(store, task_id).expect("mark_running");
    session_id
}

/// Backlog §8: "[t]wo sessions for one scope can run separate tasks
/// concurrently without shared workspace leases or prompt multiplexing."
///
/// Two idle sessions of the same scope and the same agent, in distinct
/// workspaces; two untargeted tasks, each carried through `assign` →
/// `deliver` → `mark_running`. The direct, data-visible proof: both tasks
/// end up `running`, on two different sessions, each holding its own live
/// workspace lease, and the writer recorded two separate calls — one per
/// session — rather than one call multiplexing both prompts into a shared
/// terminal.
///
/// Mutation target (task report mutation 4, "let one session hold two
/// running tasks"): widen `is_idle` (in `assign.rs`) so it no longer treats
/// a session with a non-terminal assigned task as busy — for example, drop
/// the `while` loop's early `return Ok(false)`. With exactly two idle
/// sessions and no other busy state, both tasks would then be assigned to
/// the same (lowest-`rowid`) session, and this test dies — either at the
/// distinct-`assigned_session_id` assertion below, or earlier, when the
/// second `mark_running` call hits `tasks_one_running_per_session` and
/// `.expect("mark_running")` panics on the resulting `Store` error. See the
/// task report for which.
#[test]
fn two_sessions_of_one_scope_run_separate_tasks_concurrently() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    let (session_a, workspace_a) =
        seed_idle_session(&mut store, dir.path(), 2, scope_id, "agent", 10);
    let (session_b, workspace_b) =
        seed_idle_session(&mut store, dir.path(), 3, scope_id, "agent", 10);
    assert_ne!(
        workspace_a, workspace_b,
        "fixture sanity: distinct workspaces"
    );

    let task_a = uid(50);
    let task_b = uid(51);
    create(
        &mut store,
        task_a,
        None,
        scope_id,
        None,
        None,
        "task a",
        &[],
    )
    .expect("create a");
    create(
        &mut store,
        task_b,
        None,
        scope_id,
        None,
        None,
        "task b",
        &[],
    )
    .expect("create b");

    let mut writer = RecordingWriter::new();
    let ran_on_a = assign_deliver_and_run(&mut store, task_a, "agent", 10, &mut writer);
    let ran_on_b = assign_deliver_and_run(&mut store, task_b, "agent", 10, &mut writer);

    // Distinct sessions — no shared session between the two tasks.
    assert_ne!(
        ran_on_a, ran_on_b,
        "each task must land on its own session, not share one"
    );
    let mut assigned_sessions = [ran_on_a, ran_on_b];
    assigned_sessions.sort();
    let mut seeded_sessions = [session_a, session_b];
    seeded_sessions.sort();
    assert_eq!(
        assigned_sessions, seeded_sessions,
        "the two seeded sessions, in some order, absorbed exactly the two tasks"
    );

    // Both tasks are `running` at once, each on its own assigned session.
    let task_a_row = show(&store, task_a).expect("show a");
    let task_b_row = show(&store, task_b).expect("show b");
    assert_eq!(task_a_row.status, TaskStatus::Running);
    assert_eq!(task_b_row.status, TaskStatus::Running);
    assert_eq!(task_a_row.assigned_session_id, Some(ran_on_a));
    assert_eq!(task_b_row.assigned_session_id, Some(ran_on_b));
    assert_ne!(
        task_a_row.assigned_session_id, task_b_row.assigned_session_id,
        "distinct assigned_session_ids"
    );

    // Each session's own view agrees: it is running exactly the task it was
    // handed, not the other one.
    assert_eq!(
        running_task_of_session(&store, ran_on_a).expect("read"),
        Some(task_a)
    );
    assert_eq!(
        running_task_of_session(&store, ran_on_b).expect("read"),
        Some(task_b)
    );

    // No shared workspace leases: both sessions still hold their own live
    // lease, independently.
    assert!(
        holds_a_live_lease(&store, session_a),
        "session a's lease must still be held"
    );
    assert!(
        holds_a_live_lease(&store, session_b),
        "session b's lease must still be held"
    );

    // No prompt multiplexing: the writer recorded two separate calls, one
    // per session, each carrying its own task's UUID.
    assert_eq!(writer.calls.len(), 2, "one delivery call per task");
    let calls_by_session: std::collections::HashMap<_, _> = writer
        .calls
        .iter()
        .map(|(session, prompt)| (*session, prompt.clone()))
        .collect();
    assert!(
        calls_by_session
            .get(&ran_on_a)
            .is_some_and(|p| p.contains(&task_a.to_string())),
        "the call to session a's prompt must carry task a's own UUID"
    );
    assert!(
        calls_by_session
            .get(&ran_on_b)
            .is_some_and(|p| p.contains(&task_b.to_string())),
        "the call to session b's prompt must carry task b's own UUID"
    );
}

/// Backlog §8's second demonstration: "two agents of one scope running
/// concurrently." Same scope, two different `agent_name`s, one idle session
/// each. `assign`'s existing `agent_name` parameter is what selects which
/// agent a task goes to — `tasks` carries no `target_agent_name` column
/// (backlog §8's own recorded schema gap: "[r]esolving this needs a
/// migration adding `target_agent_name` to `tasks`... [s]lice 7's `assign`
/// works around it by taking `agent_name` as a call parameter, which means
/// the choice is made at assignment time and never recorded, so after a
/// restart the task row cannot say which agent it was meant for"), so this
/// test — like every real caller today — supplies `agent_name` directly to
/// `assign` rather than reading it back off the task row. Station 8 does not
/// add that column; the gap stays exactly as recorded.
///
/// # The delivery order below is load-bearing — do not "tidy" it back
///
/// Acceptance review found that the original version of this test (deliver
/// agent-a's task, then agent-b's) passed even when `find_idle_session`'s
/// `agent_name = ?2` predicate was deleted outright
/// (`WHERE scope_id = ?1 AND agent_name = ?2` mutated to
/// `WHERE scope_id = ?1 AND ?2 IS NOT NULL`): agent-a's session is seeded
/// first, so it has the lower `rowid`, and `find_idle_session`'s own
/// `ORDER BY rowid` (see its doc comment) picks it whether or not the
/// `agent_name` filter is even applied — the assignment for agent-a "passes
/// for a reason that has nothing to do with the property it claims to
/// demonstrate" (acceptance finding), and by the time agent-b's task is
/// assigned, agent-a's session is no longer idle regardless of the filter,
/// so that assignment is order-safe too. **The mutation is invisible unless
/// the lower-`rowid` session's own agent is asked for *second*, while the
/// higher-`rowid` session is still idle.** So agent-b's task — whose
/// session (seeded second, higher `rowid`) is the one a broken filter would
/// skip past — is assigned first here, while agent-a's session is still
/// idle and available for a broken filter to wrongly prefer. Reverting this
/// order back to "a then b" restores a passing-for-the-wrong-reason test;
/// see [`untargeted_assign_never_picks_another_agents_idle_session`] in
/// `tests/assign.rs` for the same property proved head-on, independent of
/// any ordering.
#[test]
fn two_agents_of_one_scope_run_concurrently() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");
    // Seeded in this order deliberately: agent-a's session gets the lower
    // `rowid`, agent-b's the higher one — see the doc comment above.
    let (session_agent_a, _) =
        seed_idle_session(&mut store, dir.path(), 2, scope_id, "agent-a", 10);
    let (session_agent_b, _) =
        seed_idle_session(&mut store, dir.path(), 3, scope_id, "agent-b", 10);

    let task_for_a = uid(50);
    let task_for_b = uid(51);
    create(
        &mut store,
        task_for_a,
        None,
        scope_id,
        None,
        None,
        "for agent-a",
        &[],
    )
    .expect("create task for agent-a");
    create(
        &mut store,
        task_for_b,
        None,
        scope_id,
        None,
        None,
        "for agent-b",
        &[],
    )
    .expect("create task for agent-b");

    let mut writer = RecordingWriter::new();
    // agent-b first, while agent-a's lower-rowid session is still idle —
    // load-bearing order, see the doc comment above.
    let ran_on_b = assign_deliver_and_run(&mut store, task_for_b, "agent-b", 10, &mut writer);
    let ran_on_a = assign_deliver_and_run(&mut store, task_for_a, "agent-a", 10, &mut writer);

    assert_eq!(
        ran_on_b, session_agent_b,
        "the task aimed at agent-b must land on agent-b's session, not agent-a's lower-rowid one"
    );
    assert_eq!(
        ran_on_a, session_agent_a,
        "the task aimed at agent-a must land on agent-a's session"
    );
    assert_ne!(ran_on_a, ran_on_b);

    let task_a_row = show(&store, task_for_a).expect("show a");
    let task_b_row = show(&store, task_for_b).expect("show b");
    assert_eq!(task_a_row.status, TaskStatus::Running);
    assert_eq!(task_b_row.status, TaskStatus::Running);
    assert!(holds_a_live_lease(&store, session_agent_a));
    assert!(holds_a_live_lease(&store, session_agent_b));
}
