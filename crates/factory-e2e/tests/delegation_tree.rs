//! Drill 2: design §6's one delegation rule, exercised against a registry
//! projected from a real `config.yaml` rather than scope rows written by hand.
//! Every kinship the rule can meet is present in the fixture's tree, and every
//! sender here is a *session*, never a scope id — an agent that could name its
//! own sender scope would have no rule left to break.

mod common;

use factory_delegation::queue::{queue_from_human, queue_from_session};
use factory_delegation::rule::DelegationError;
use factory_paths::CanonicalPath;

fn start_session(
    store: &mut factory_store::Store,
    seed: u32,
    scope: uuid::Uuid,
    agent: &str,
    workspace: &CanonicalPath,
) -> uuid::Uuid {
    let id = common::uid(seed);
    factory_session::begin_start(store, id, scope, agent, 2, workspace).expect("begin_start");
    factory_session::mark_running(store, id).expect("mark_running");
    id
}

fn row_counts(store: &factory_store::Store) -> (i64, i64, i64) {
    let one = |sql: &str| {
        store
            .connection()
            .query_row(sql, [], |row| row.get::<_, i64>(0))
            .expect("count")
    };
    (
        one("SELECT COUNT(*) FROM tasks"),
        one("SELECT COUNT(*) FROM task_delegation_chain"),
        one("SELECT COUNT(*) FROM delivery_attempts"),
    )
}

#[test]
fn the_whole_delegation_rule_holds_against_a_real_registry() {
    let mut inst = common::build();
    let (alpha, beta, gamma, root) = (inst.alpha, inst.beta, inst.gamma, inst.root);
    let ws_alpha = inst.workspace("ws-alpha");
    let ws_beta = inst.workspace("ws-beta");
    let alpha_session = start_session(&mut inst.store, 10, alpha, "alpha-agent", &ws_alpha);
    let beta_session = start_session(&mut inst.store, 11, beta, "beta-agent", &ws_beta);

    // Legal: a parent reaches its own child, and a scope reaches its sibling.
    queue_from_session(
        &mut inst.store,
        common::uid(20),
        alpha_session,
        beta,
        None,
        None,
        "to my child",
    )
    .expect("alpha -> beta is a descendant");
    queue_from_session(
        &mut inst.store,
        common::uid(21),
        alpha_session,
        gamma,
        None,
        None,
        "to my sibling",
    )
    .expect("alpha -> gamma is a sibling");

    let before = row_counts(&inst.store);

    // Refused: an ancestor, itself, and an uncle. Design §6 routes the last
    // one through gamma's parent instead — the root — which is exactly the
    // "keeps a scope informed that work is happening in its own subtree"
    // argument, read from beta's end.
    for (target, what) in [
        (root, "its own ancestor"),
        (beta, "itself"),
        (gamma, "an uncle, which is Unrelated"),
    ] {
        let err = queue_from_session(
            &mut inst.store,
            common::uid(30),
            beta_session,
            target,
            None,
            None,
            "should never be queued",
        )
        .expect_err(&format!("beta must not target {what}"));
        assert!(
            matches!(err, DelegationError::NotEligible { .. }),
            "expected NotEligible for {what}, got {err:?}"
        );
    }

    assert_eq!(
        row_counts(&inst.store),
        before,
        "a refusal leaves no task, no chain row, and no delivery record"
    );
}

#[test]
fn a_two_step_cycle_is_refused_after_the_chain_is_read_back_from_the_database() {
    let mut inst = common::build();
    let (alpha, gamma) = (inst.alpha, inst.gamma);
    let ws_alpha = inst.workspace("ws-alpha");
    let ws_gamma = inst.workspace("ws-gamma");
    let alpha_session = start_session(&mut inst.store, 10, alpha, "alpha-agent", &ws_alpha);
    let gamma_session = start_session(&mut inst.store, 11, gamma, "gamma-agent", &ws_gamma);

    // A human starts the work at alpha; alpha hands it to its sibling gamma.
    let first = common::uid(40);
    queue_from_human(&mut inst.store, first, alpha, None, None, "start here")
        .expect("human may target alpha");
    let second = common::uid(41);
    queue_from_session(
        &mut inst.store,
        second,
        alpha_session,
        gamma,
        None,
        None,
        "your turn",
    )
    .expect("alpha -> gamma is a sibling");
    let _ = first;

    // gamma is now running that second task, so its chain is what gamma
    // inherits — and alpha is already in it.
    factory_task::assign::assign(&mut inst.store, second, "gamma-agent", 1).expect("assign");
    let mut writer = common::RecordingWriter::default();
    factory_task::deliver::deliver(&mut inst.store, second, &mut writer).expect("deliver");
    factory_task::deliver::mark_running(&mut inst.store, second).expect("mark_running");

    let before = row_counts(&inst.store);
    let err = queue_from_session(
        &mut inst.store,
        common::uid(42),
        gamma_session,
        alpha,
        None,
        None,
        "back to you",
    )
    .expect_err("gamma -> alpha closes a two-step cycle");
    assert!(
        matches!(err, DelegationError::AlreadyInChain { target } if target == alpha),
        "expected AlreadyInChain, got {err:?}"
    );
    assert_eq!(
        row_counts(&inst.store),
        before,
        "a refused cycle writes nothing"
    );

    // The chain that refused it is durable, not a fact held in memory: it is
    // read back out of the database, which is what makes a loop refusable
    // after a restart (backlog §8).
    assert_eq!(
        factory_task::create::delegation_chain_of(&inst.store, second).expect("chain"),
        vec![alpha, inst.gamma],
    );
}
