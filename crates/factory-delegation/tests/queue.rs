//! Integration tests for `factory_delegation::queue`'s two entry points —
//! the trust rule (design §6; crate docs decisions 3, 5) and the acceptance
//! criteria backlog §8 states for it.
//!
//! # The fixture: one scope tree, every test builds its own copy
//!
//! ```text
//!               root
//!              /    \
//!        parent_a    parent_b
//!        /  |   \        \
//!   child_a1 a2  a3     child_b1
//! ```
//!
//! [`build_tree`] is the one place this shape is declared; every test below
//! calls it fresh against its own temp database, mirroring
//! `factory-registry`'s own tests' "[e]very scenario builds its own tiny
//! instance" stance. One tree serves every scenario the task brief lists:
//! `parent_a`/`child_a1` is the parent-child pair, `child_a1`/`child_a2` (and
//! `a3`) are mutual siblings for the sibling and cycle tests, `parent_a` ->
//! `child_b1` is the nephew case, and `child_a1` -> `child_b1` is the cousin
//! case (children of siblings `parent_a`/`parent_b`).
//!
//! # Why scopes and sessions are seeded with raw SQL, not through
//! `factory_registry` / `factory_session`
//!
//! This crate does not own scope registration or session lifecycle, the same
//! stance `factory-task`'s own test suite takes toward scopes (see that
//! crate's `tests/create.rs` module docs: "bypasses `factory-registry`
//! (which this crate does not depend on)"). `factory_session` is also
//! deliberately not exercised here: this crate's `Cargo.toml` depends on it
//! for `scope_of_session`, but not on `factory_paths`, which
//! `factory_session::begin_start` requires a real `CanonicalPath` for — and
//! this crate's rule only ever reads `sessions.scope_id` and
//! `tasks.(assigned_session_id, status)`, neither of which requires a real
//! workspace lease to exist. A row inserted directly is exactly as valid an
//! input to `factory_delegation::queue` as one `factory_session` would have
//! produced.
//!
//! # "Leaves nothing behind," proven against a non-empty database
//!
//! Every refusal test below seeds a baseline successful task, its chain row,
//! and a `delivery_attempts` row *before* taking its "before" snapshot, so
//! the row-count assertion after the refused call is checking that nothing
//! changed, not merely that everything is still zero (a check that would
//! also pass if the refusal path wrote and then rolled back sloppily, or
//! never got the chance to write at all).

use factory_delegation::queue::{queue_from_human, queue_from_session};
use factory_delegation::rule::DelegationError;
use factory_store::Store;
use uuid::Uuid;

// ---------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------

/// A deterministic, distinct, syntactically valid UUID. `uuid` is pinned
/// workspace-wide without the `v4` feature, so tests build UUIDs by hand
/// from a seed — mirrors `factory-session`'s and `factory-task`'s own test
/// helpers of the same name.
fn uid(seed: u32) -> Uuid {
    Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

/// Insert a scope row directly (see module docs for why). `declared_path`
/// and `canonical_path` are set to the same throwaway string —
/// `factory-registry`'s own resolution is not under test here, and nothing
/// in this crate's rule reads either column.
fn seed_scope(store: &mut Store, seed: u32, name: &str, parent_id: Option<Uuid>) -> Uuid {
    let id = uid(seed);
    let path = format!("/instance/scope-{seed}");
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO scopes (id, name, declared_path, canonical_path, parent_id) \
         VALUES (?1, ?2, ?3, ?3, ?4)",
        (id.to_string(), name, path, parent_id.map(|p| p.to_string())),
    )
    .expect("insert scope");
    tx.commit().expect("commit");
    id
}

/// Insert a `running` session row directly, in `scope_id`, at a unique
/// throwaway workspace path — see module docs for why this bypasses
/// `factory_session::begin_start`. `state` plays no role in this crate's
/// rule (only `sessions.scope_id` is ever read from this table), so a fixed
/// `running` is used purely for readability, not because any check here
/// depends on it.
fn seed_session(store: &mut Store, seed: u32, scope_id: Uuid) -> Uuid {
    let id = uid(seed);
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO sessions (id, scope_id, agent_name, workspace_path, state) \
         VALUES (?1, ?2, 'agent', ?3, 'running')",
        (
            id.to_string(),
            scope_id.to_string(),
            format!("/instance/workspace-{seed}"),
        ),
    )
    .expect("insert session");
    tx.commit().expect("commit");
    id
}

/// Move `task_id` to `running`, assigned to `session_id` — directly, the
/// same way `factory-task`'s own `tests/create.rs::seed_running_task`
/// bypasses `assign`/`deliver` (neither of which this crate is allowed to
/// touch, and neither of which this test needs: only the resulting
/// `(assigned_session_id, status)` pair matters to
/// `factory_task::assign::running_task_of_session`, which is what
/// `queue_from_session` calls to find it). Satisfies the schema's own
/// `CHECK (status = 'running' AND assigned_session_id IS NOT NULL) OR
/// (status <> 'running')` and `tasks_one_running_per_session`'s partial
/// unique index, so long as no two tasks in one test share a session.
fn mark_task_running(store: &mut Store, task_id: Uuid, session_id: Uuid) {
    let tx = store.transaction().expect("begin");
    tx.execute(
        "UPDATE tasks SET status = 'running', assigned_session_id = ?2 WHERE id = ?1",
        (task_id.to_string(), session_id.to_string()),
    )
    .expect("mark task running");
    tx.commit().expect("commit");
}

/// Insert one `delivery_attempts` row directly, for the "leaves nothing
/// behind" baseline described in the module docs.
fn seed_delivery_attempt(store: &mut Store, task_id: Uuid, session_id: Uuid) {
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO delivery_attempts (task_id, session_id) VALUES (?1, ?2)",
        (task_id.to_string(), session_id.to_string()),
    )
    .expect("insert delivery attempt");
    tx.commit().expect("commit");
}

/// `(tasks, task_delegation_chain, delivery_attempts)` row counts — the
/// three tables backlog §8 says a refusal must leave untouched.
fn table_counts(store: &Store) -> (i64, i64, i64) {
    let conn = store.connection();
    let count = |table: &str| -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap_or_else(|e| panic!("count {table}: {e}"))
    };
    (
        count("tasks"),
        count("task_delegation_chain"),
        count("delivery_attempts"),
    )
}

/// The scope tree every test below builds — see the module docs' diagram.
#[allow(dead_code)]
struct Tree {
    root: Uuid,
    parent_a: Uuid,
    parent_b: Uuid,
    child_a1: Uuid,
    child_a2: Uuid,
    child_a3: Uuid,
    child_b1: Uuid,
}

fn build_tree(store: &mut Store) -> Tree {
    let root = seed_scope(store, 1, "root", None);
    let parent_a = seed_scope(store, 2, "parent-a", Some(root));
    let parent_b = seed_scope(store, 3, "parent-b", Some(root));
    let child_a1 = seed_scope(store, 4, "child-a1", Some(parent_a));
    let child_a2 = seed_scope(store, 5, "child-a2", Some(parent_a));
    let child_a3 = seed_scope(store, 6, "child-a3", Some(parent_a));
    let child_b1 = seed_scope(store, 7, "child-b1", Some(parent_b));
    Tree {
        root,
        parent_a,
        parent_b,
        child_a1,
        child_a2,
        child_a3,
        child_b1,
    }
}

/// Seed one baseline successful human-queued task (a `tasks` row and its
/// one-entry `task_delegation_chain` row) plus a `delivery_attempts` row
/// against it, so a refusal test's "before" snapshot is taken against a
/// non-empty database — see the module docs.
fn seed_baseline(store: &mut Store, target: Uuid) {
    let baseline_task = queue_from_human(store, uid(800), target, None, None, "baseline")
        .expect("baseline task must succeed");
    let baseline_session = seed_session(store, 800, target);
    seed_delivery_attempt(store, baseline_task, baseline_session);
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

/// Design §6: "a human may target any registered scope." Exercises decision
/// 5's human chain: exactly `[target]`.
#[test]
fn human_can_queue_to_any_registered_scope() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tree = build_tree(&mut store);

    let task_id = uid(100);
    let returned = queue_from_human(
        &mut store,
        task_id,
        tree.child_b1,
        None,
        None,
        "do the thing",
    )
    .expect("a human may target any registered scope");
    assert_eq!(returned, task_id);

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(
        task.sender_scope_id, None,
        "sender_scope_id is NULL for a human sender"
    );
    assert_eq!(task.target_scope_id, tree.child_b1);

    let chain = factory_task::create::delegation_chain_of(&store, task_id).expect("chain");
    assert_eq!(chain, vec![tree.child_b1]);
}

/// The word "registered" in design §6 / backlog §8's "a human may target
/// any **registered** scope" is load-bearing: a human sender is exempt from
/// kinship, never from existence. `rule::check` enforces this by asking
/// `factory_registry::kinship(target, target)` purely for its existence
/// check (see that function's doc comment on `rule::check`) — an id that
/// names no row must surface as this crate's own typed
/// `DelegationError::Registry(RegistryError::UnknownScope)`, not the raw
/// foreign-key violation `factory_task::create::create` would otherwise
/// produce.
#[test]
fn human_to_unregistered_scope_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tree = build_tree(&mut store);

    seed_baseline(&mut store, tree.child_b1);

    let unregistered = uid(999);
    let before = table_counts(&store);

    let result = queue_from_human(
        &mut store,
        uid(802),
        unregistered,
        None,
        None,
        "target a made-up scope",
    );

    match result {
        Err(DelegationError::Registry(factory_registry::RegistryError::UnknownScope { id })) => {
            assert_eq!(id, unregistered);
        }
        other => panic!(
            "expected DelegationError::Registry(UnknownScope), got {other:?} — a bare foreign-key \
             violation here would mean the existence check was skipped for a human sender"
        ),
    }

    assert_eq!(table_counts(&store), before);
}

/// The mirror of [`human_to_unregistered_scope_is_refused`] for an agent
/// sender. `factory_registry::kinship`'s own doc comment says both ids'
/// existence is checked unconditionally, before the `SameScope`
/// short-circuit — so this should already work via the ordinary kinship
/// call `rule::check` makes for [`Sender::Agent`], with no extra code. This
/// test is the proof: nothing before it in this file sends an unregistered
/// id through `queue_from_session`, so nothing else demonstrates this path.
#[test]
fn scope_to_unregistered_target_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tree = build_tree(&mut store);

    seed_baseline(&mut store, tree.child_b1);

    let sender_session = seed_session(&mut store, 10, tree.parent_a);
    let unregistered = uid(999);
    let before = table_counts(&store);

    let result = queue_from_session(
        &mut store,
        uid(802),
        sender_session,
        unregistered,
        None,
        None,
        "target a made-up scope",
    );

    match result {
        Err(DelegationError::Registry(factory_registry::RegistryError::UnknownScope { id })) => {
            assert_eq!(id, unregistered);
        }
        other => panic!("expected DelegationError::Registry(UnknownScope), got {other:?}"),
    }

    assert_eq!(table_counts(&store), before);
}

/// Design §6: "[a]n agent may target a registered descendant." Exercises
/// the *other* half of decision 5: a session running no task gets chain
/// `[sender, target]`.
#[test]
fn parent_can_queue_directly_to_child() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tree = build_tree(&mut store);

    let parent_session = seed_session(&mut store, 10, tree.parent_a);

    let task_id = uid(100);
    let returned = queue_from_session(
        &mut store,
        task_id,
        parent_session,
        tree.child_a1,
        None,
        None,
        "do it",
    )
    .expect("parent -> child must succeed");
    assert_eq!(returned, task_id);

    let task = factory_task::create::show(&store, task_id).expect("show");
    assert_eq!(task.sender_scope_id, Some(tree.parent_a));
    assert_eq!(task.target_scope_id, tree.child_a1);

    let chain = factory_task::create::delegation_chain_of(&store, task_id).expect("chain");
    assert_eq!(
        chain,
        vec![tree.parent_a, tree.child_a1],
        "session ran no task, so its chain is [sender, target] (decision 5)"
    );
}

/// Design §6: "never an ancestor." `child_a1`'s existing chain (`[child_a1]`,
/// from the task it is running) does not already contain `parent_a`, so this
/// refusal is attributable to the kinship gate alone — required for the
/// crate report's "allow Ancestor" mutation to unambiguously kill only this
/// test.
#[test]
fn child_to_parent_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tree = build_tree(&mut store);

    seed_baseline(&mut store, tree.child_b1);

    let running_task = queue_from_human(&mut store, uid(801), tree.child_a1, None, None, "seed")
        .expect("seed task");
    let child_session = seed_session(&mut store, 10, tree.child_a1);
    mark_task_running(&mut store, running_task, child_session);

    let before = table_counts(&store);

    let result = queue_from_session(
        &mut store,
        uid(802),
        child_session,
        tree.parent_a,
        None,
        None,
        "escalate",
    );

    match result {
        Err(DelegationError::NotEligible {
            sender,
            target,
            kinship,
            ..
        }) => {
            assert_eq!(sender, tree.child_a1);
            assert_eq!(target, tree.parent_a);
            assert_eq!(kinship, factory_registry::Kinship::Ancestor);
        }
        other => panic!("expected DelegationError::NotEligible (Ancestor), got {other:?}"),
    }

    assert_eq!(
        table_counts(&store),
        before,
        "a refusal must leave tasks, task_delegation_chain, and delivery_attempts unchanged"
    );
}

/// Design §6: "never itself."
#[test]
fn scope_targeting_itself_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tree = build_tree(&mut store);

    seed_baseline(&mut store, tree.child_b1);

    let session = seed_session(&mut store, 10, tree.child_a1);
    let before = table_counts(&store);

    let result = queue_from_session(
        &mut store,
        uid(802),
        session,
        tree.child_a1,
        None,
        None,
        "loop",
    );

    match result {
        Err(DelegationError::NotEligible {
            sender,
            target,
            kinship,
            ..
        }) => {
            assert_eq!(sender, tree.child_a1);
            assert_eq!(target, tree.child_a1);
            assert_eq!(kinship, factory_registry::Kinship::SameScope);
        }
        other => panic!("expected DelegationError::NotEligible (SameScope), got {other:?}"),
    }

    assert_eq!(table_counts(&store), before);
}

/// Design §6: "[a]n agent may target a registered descendant or sibling."
#[test]
fn scope_can_target_its_sibling() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tree = build_tree(&mut store);

    let session_a1 = seed_session(&mut store, 10, tree.child_a1);

    let task_id = uid(100);
    let returned = queue_from_session(
        &mut store,
        task_id,
        session_a1,
        tree.child_a2,
        None,
        None,
        "peer work",
    )
    .expect("sibling delegation must succeed");
    assert_eq!(returned, task_id);

    let chain = factory_task::create::delegation_chain_of(&store, task_id).expect("chain");
    assert_eq!(chain, vec![tree.child_a1, tree.child_a2]);
}

/// Design §6: "[a] nephew or cousin scope is not directly targetable; the
/// work goes through that scope's parent." `parent_a` -> `child_b1` (a
/// child of `parent_a`'s sibling `parent_b`) is the nephew shape:
/// `factory_registry::kinship` resolves it to `Unrelated` (crate docs,
/// decision 3's sibling rule only ever matches an *immediate* shared
/// parent).
#[test]
fn scope_to_nephew_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tree = build_tree(&mut store);

    seed_baseline(&mut store, tree.child_a3);

    let parent_a_session = seed_session(&mut store, 10, tree.parent_a);
    let before = table_counts(&store);

    let result = queue_from_session(
        &mut store,
        uid(802),
        parent_a_session,
        tree.child_b1,
        None,
        None,
        "reach into my sibling's subtree",
    );

    match result {
        Err(DelegationError::NotEligible {
            sender,
            target,
            kinship,
            ..
        }) => {
            assert_eq!(sender, tree.parent_a);
            assert_eq!(target, tree.child_b1);
            assert_eq!(kinship, factory_registry::Kinship::Unrelated);
        }
        other => panic!("expected DelegationError::NotEligible (Unrelated), got {other:?}"),
    }

    assert_eq!(table_counts(&store), before);
}

/// The other shape `Unrelated` refuses: `child_a1` -> `child_b1`, children
/// of siblings `parent_a`/`parent_b` — cousins.
#[test]
fn scope_to_cousin_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tree = build_tree(&mut store);

    seed_baseline(&mut store, tree.child_a3);

    let child_a1_session = seed_session(&mut store, 10, tree.child_a1);
    let before = table_counts(&store);

    let result = queue_from_session(
        &mut store,
        uid(802),
        child_a1_session,
        tree.child_b1,
        None,
        None,
        "reach my cousin directly",
    );

    match result {
        Err(DelegationError::NotEligible {
            sender,
            target,
            kinship,
            ..
        }) => {
            assert_eq!(sender, tree.child_a1);
            assert_eq!(target, tree.child_b1);
            assert_eq!(kinship, factory_registry::Kinship::Unrelated);
        }
        other => panic!("expected DelegationError::NotEligible (Unrelated), got {other:?}"),
    }

    assert_eq!(table_counts(&store), before);
}

/// Design §6: "[t]he delegation chain replaces a guarantee that the
/// earlier descendant-only rule provided for free... `irrlicht -> model-lab
/// -> irrlicht` is otherwise legal." `child_a1` and `child_a2` are mutual
/// siblings, so kinship allows *both* hops in this cycle — only the chain
/// gate can refuse the second one, which is why this scenario (not a
/// parent/child pair) is what the task brief asks for.
#[test]
fn two_step_cycle_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tree = build_tree(&mut store);

    seed_baseline(&mut store, tree.child_a3);

    // Human -> child_a1 (chain [A]).
    let task_a = queue_from_human(&mut store, uid(810), tree.child_a1, None, None, "start")
        .expect("seed task A");
    let session_a = seed_session(&mut store, 10, tree.child_a1);
    mark_task_running(&mut store, task_a, session_a);

    // A (sibling) -> B, chain [A, B].
    let task_b = queue_from_session(
        &mut store,
        uid(811),
        session_a,
        tree.child_a2,
        None,
        None,
        "hand off to my sibling",
    )
    .expect("A -> B is a legal sibling hop");
    assert_eq!(
        factory_task::create::delegation_chain_of(&store, task_b).expect("chain"),
        vec![tree.child_a1, tree.child_a2]
    );
    let session_b = seed_session(&mut store, 11, tree.child_a2);
    mark_task_running(&mut store, task_b, session_b);

    let before = table_counts(&store);

    // B (sibling) -> A: kinship allows it (siblings are symmetric); only
    // the chain gate — A is already at position 0 — can refuse it.
    let result = queue_from_session(
        &mut store,
        uid(812),
        session_b,
        tree.child_a1,
        None,
        None,
        "hand back",
    );

    match result {
        Err(DelegationError::AlreadyInChain { target }) => {
            assert_eq!(target, tree.child_a1);
        }
        other => panic!(
            "expected DelegationError::AlreadyInChain — if this is instead an `Err` wrapping a \
             rusqlite constraint error, the chain-membership check was removed and only the \
             database's UNIQUE (task_id, scope_id) backstop caught it; got {other:?}"
        ),
    }

    assert_eq!(table_counts(&store), before);
}

/// The longer cycle: `child_a1`, `child_a2`, `child_a3` are three mutual
/// siblings (all children of `parent_a`), so kinship allows every one of
/// A -> B -> C -> A's three hops; only the chain gate stops the last one.
#[test]
fn three_step_cycle_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tree = build_tree(&mut store);

    seed_baseline(&mut store, tree.child_b1);

    // Human -> A (chain [A]).
    let task_a = queue_from_human(&mut store, uid(820), tree.child_a1, None, None, "start")
        .expect("seed task A");
    let session_a = seed_session(&mut store, 20, tree.child_a1);
    mark_task_running(&mut store, task_a, session_a);

    // A -> B, chain [A, B].
    let task_b = queue_from_session(
        &mut store,
        uid(821),
        session_a,
        tree.child_a2,
        None,
        None,
        "A to B",
    )
    .expect("A -> B is a legal sibling hop");
    let session_b = seed_session(&mut store, 21, tree.child_a2);
    mark_task_running(&mut store, task_b, session_b);

    // B -> C, chain [A, B, C].
    let task_c = queue_from_session(
        &mut store,
        uid(822),
        session_b,
        tree.child_a3,
        None,
        None,
        "B to C",
    )
    .expect("B -> C is a legal sibling hop");
    assert_eq!(
        factory_task::create::delegation_chain_of(&store, task_c).expect("chain"),
        vec![tree.child_a1, tree.child_a2, tree.child_a3]
    );
    let session_c = seed_session(&mut store, 22, tree.child_a3);
    mark_task_running(&mut store, task_c, session_c);

    let before = table_counts(&store);

    // C -> A: kinship allows it (C and A are siblings); the chain gate is
    // the only thing that can refuse it, since A is already at position 0.
    let result = queue_from_session(
        &mut store,
        uid(823),
        session_c,
        tree.child_a1,
        None,
        None,
        "C back to A",
    );

    match result {
        Err(DelegationError::AlreadyInChain { target }) => {
            assert_eq!(target, tree.child_a1);
        }
        other => panic!(
            "expected DelegationError::AlreadyInChain, got {other:?} (see \
             two_step_cycle_is_refused for what a missing chain check looks like instead)"
        ),
    }

    assert_eq!(table_counts(&store), before);
}

// ---------------------------------------------------------------------
// Station 12 drill, defect 3: the refusal text must read as a sentence.
// ---------------------------------------------------------------------

/// The station 12 drill's own observation:
/// `"scope …0001 may not target scope …0001 — …0001 is itself of …0001"`
/// and its `Unrelated` sibling read as nonsense, because the old `Display`
/// template appended `" of {sender}"` onto a `reason` that already said
/// everything needed. This asserts the exact rendered sentence for all
/// three refused kinships — the mutation this test exists to kill is
/// putting that trailing clause back.
///
/// Calls `rule::check` directly rather than routing through `queue`: the
/// wording lives entirely in `DelegationError`'s `Display`, which `queue`
/// does not touch, so a task/session detour would test nothing extra.
#[test]
fn not_eligible_display_reads_as_a_grammatical_sentence_for_every_kinship() {
    use factory_delegation::rule::{Sender, check};

    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let tree = build_tree(&mut store);

    let same_scope = check(
        store.connection(),
        Sender::Agent {
            scope_id: tree.child_a1,
        },
        tree.child_a1,
        &[],
    )
    .unwrap_err();
    assert_eq!(
        same_scope.to_string(),
        format!(
            "scope {a} may not target scope {a} — {a} is the sender itself\n  help: design §6: an agent may target only a registered descendant or sibling scope; a nephew or cousin is reached through its parent",
            a = tree.child_a1,
        )
    );

    let ancestor = check(
        store.connection(),
        Sender::Agent {
            scope_id: tree.child_a1,
        },
        tree.parent_a,
        &[],
    )
    .unwrap_err();
    assert_eq!(
        ancestor.to_string(),
        format!(
            "scope {sender} may not target scope {target} — {target} is an ancestor of the sender\n  help: design §6: an agent may target only a registered descendant or sibling scope; a nephew or cousin is reached through its parent",
            sender = tree.child_a1,
            target = tree.parent_a,
        )
    );

    let unrelated = check(
        store.connection(),
        Sender::Agent {
            scope_id: tree.child_a1,
        },
        tree.child_b1,
        &[],
    )
    .unwrap_err();
    assert_eq!(
        unrelated.to_string(),
        format!(
            "scope {sender} may not target scope {target} — {target} is unrelated to the sender (a nephew or cousin scope — design §6 says such work goes through the target's parent)\n  help: design §6: an agent may target only a registered descendant or sibling scope; a nephew or cousin is reached through its parent",
            sender = tree.child_a1,
            target = tree.child_b1,
        )
    );
}
