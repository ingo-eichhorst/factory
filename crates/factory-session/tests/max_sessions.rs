//! `max_sessions` is evaluated per *agent*, not per scope (design §2.2,
//! `factory_config::Agent::max_sessions`; backlog §6: "`max_sessions`
//! prevents further starts without changing existing sessions, and is
//! evaluated per agent rather than per scope").
//!
//! [`max_sessions_is_evaluated_per_agent_not_per_scope`] is the load-bearing
//! test: it is the one a mutation dropping `count_live_sessions`'s
//! `agent_name` predicate breaks (see the task report), because a per-scope
//! count would see agent-a's session and wrongly reject agent-b's first
//! start. [`reaching_max_sessions_blocks_a_further_start...`] is the
//! complementary half of the same acceptance criterion: the cap must reject
//! *and* leave the session already running completely alone.

mod common;

use factory_session::{SessionError, begin_start, mark_running};
use factory_store::Store;

/// Two agents sharing one scope get independent budgets. Agent-a already has
/// one live (`running`) session at its `max_sessions = 1`; agent-b, with the
/// same limit, must still be able to take its *first* session, because the
/// bound is per `(scope_id, agent_name)`, not per `scope_id`. A count that
/// forgot to filter by `agent_name` would see agent-a's session and reject
/// this.
#[test]
fn max_sessions_is_evaluated_per_agent_not_per_scope() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());

    let workspace_a = dir.path().join("workspace-a");
    std::fs::create_dir(&workspace_a).expect("create");
    let workspace_a = common::resolve(&workspace_a);
    let session_a = common::uid(1);
    begin_start(&mut store, session_a, scope_id, "agent-a", 1, &workspace_a)
        .expect("agent-a's first session, within its own budget of 1");
    mark_running(&mut store, session_a).expect("readiness observed");

    let workspace_b = dir.path().join("workspace-b");
    std::fs::create_dir(&workspace_b).expect("create");
    let workspace_b = common::resolve(&workspace_b);
    begin_start(
        &mut store,
        common::uid(2),
        scope_id,
        "agent-b",
        1,
        &workspace_b,
    )
    .expect(
        "agent-b has its own independent budget of 1 and has zero live \
         sessions of its own; agent-a's session must not count against it",
    );
}

/// The cap rejects a further start once reached, and — the second half of
/// the same acceptance criterion — must not disturb the session already
/// running: its state is unchanged, and its lease row is still open.
#[test]
fn reaching_max_sessions_blocks_a_further_start_without_disturbing_the_existing_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());

    let workspace_a = dir.path().join("workspace-a");
    std::fs::create_dir(&workspace_a).expect("create");
    let workspace_a = common::resolve(&workspace_a);
    let session_a = common::uid(1);
    begin_start(&mut store, session_a, scope_id, "agent-a", 1, &workspace_a)
        .expect("agent-a's first session");
    mark_running(&mut store, session_a).expect("readiness observed");

    // A second, different workspace, so any rejection here can only be the
    // max_sessions cap — never the unrelated WorkspaceLeased check, since the
    // two workspaces do not alias.
    let workspace_b = dir.path().join("workspace-b");
    std::fs::create_dir(&workspace_b).expect("create");
    let workspace_b = common::resolve(&workspace_b);
    let err = begin_start(
        &mut store,
        common::uid(2),
        scope_id,
        "agent-a",
        1,
        &workspace_b,
    )
    .expect_err("agent-a is already at its max_sessions of 1");
    assert!(
        matches!(
            &err,
            SessionError::MaxSessionsReached {
                agent_name,
                max_sessions: 1,
                live_count: 1,
            } if agent_name == "agent-a"
        ),
        "expected MaxSessionsReached for agent-a (1 of 1), got {err:?}"
    );

    // "Without changing existing sessions" (backlog §6), checked directly:
    // session A's state and lease are exactly as they were before the
    // rejected attempt, and no row was inserted for the rejected one.
    assert_eq!(common::session_state(&store, session_a), "running");
    assert!(
        common::lease_is_open(&store, session_a),
        "the rejected second start must not touch session A's lease"
    );
    assert_eq!(
        common::session_count_for_agent(&store, scope_id, "agent-a"),
        1,
        "a rejected start must insert no row"
    );
}

/// A `stopped` session does not count toward the limit — [`holds_lease`]
/// governs both the lease and `max_sessions`' notion of "live," on purpose
/// (see the module docs), so once a session is stopped, its agent has room
/// again.
///
/// [`holds_lease`]: factory_session::SessionState::holds_lease
#[test]
fn a_stopped_session_does_not_count_toward_max_sessions() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());

    let workspace_a = dir.path().join("workspace-a");
    std::fs::create_dir(&workspace_a).expect("create");
    let workspace_a = common::resolve(&workspace_a);
    let session_a = common::uid(1);
    begin_start(&mut store, session_a, scope_id, "agent-a", 1, &workspace_a)
        .expect("agent-a's first session");
    mark_running(&mut store, session_a).expect("readiness observed");
    factory_session::stop(&mut store, session_a, "task finished").expect("stop");

    let workspace_b = dir.path().join("workspace-b");
    std::fs::create_dir(&workspace_b).expect("create");
    let workspace_b = common::resolve(&workspace_b);
    begin_start(
        &mut store,
        common::uid(2),
        scope_id,
        "agent-a",
        1,
        &workspace_b,
    )
    .expect("agent-a has zero *live* sessions now, so a new one fits within max_sessions = 1");
}
