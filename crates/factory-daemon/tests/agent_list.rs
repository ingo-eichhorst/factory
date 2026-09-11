//! `agent.list` (ADR 0022 decisions 7, 8, 9).

mod common;

use factory_daemon::envelope::{CommandRequest, QueryRequest};
use factory_daemon::{FactoryHandler, Handler};

fn uid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

fn cmd(scope_id: uuid::Uuid, command: &str, payload: serde_json::Value) -> CommandRequest {
    CommandRequest {
        request_id: uid(9600),
        scope_id,
        command: command.to_string(),
        payload,
        expected_revision: None,
    }
}

fn qry(payload: serde_json::Value) -> QueryRequest {
    QueryRequest {
        request_id: uid(9601),
        scope_id: uuid::Uuid::nil(),
        query: "agent.list".to_string(),
        payload,
    }
}

fn new_handler(fixture: &common::Fixture) -> FactoryHandler {
    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();
    FactoryHandler::new(store, adapter, fixture.instance_root())
}

fn find_scope(scopes: &[serde_json::Value], id: uuid::Uuid) -> &serde_json::Value {
    scopes
        .iter()
        .find(|s| s["scope_id"] == id.to_string())
        .unwrap_or_else(|| panic!("scope {id} not in agent.list's own result: {scopes:?}"))
}

fn find_agent<'a>(scope_row: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    scope_row["agents"]
        .as_array()
        .expect("agents is an array")
        .iter()
        .find(|a| a["name"] == name)
        .unwrap_or_else(|| panic!("agent {name} not listed for scope {scope_row:?}"))
}

// --- decision 7: exactly one of --session / --scope ------------------------

#[test]
fn refuses_both_session_and_scope_together() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let alpha = fixture.scope("alpha");
    let handler = new_handler(&fixture);

    let session_id = common::seed(1);
    handler
        .handle_command(cmd(
            alpha,
            "agent.start",
            serde_json::json!({ "session_id": session_id.to_string(), "agent_name": "agent" }),
        ))
        .expect("agent.start succeeds");

    let err = handler
        .handle_query(qry(serde_json::json!({
            "session_id": session_id.to_string(),
            "scope_id": alpha.to_string(),
        })))
        .unwrap_err();
    assert_eq!(err.code, "validation.conflicting_fields");
}

#[test]
fn refuses_when_neither_session_nor_scope_is_given() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let handler = new_handler(&fixture);

    let err = handler
        .handle_query(qry(serde_json::json!({})))
        .unwrap_err();
    assert_eq!(err.code, "validation.missing_field");
}

/// ADR 0022 decision 7's own stated purpose: `--session` and `--scope` must
/// resolve the caller through the same identity, so the advisory listing
/// cannot drift from what `task send` will actually enforce. Proved here by
/// checking the two forms produce byte-identical results for the same
/// underlying scope.
#[test]
fn session_and_scope_forms_resolve_the_caller_identically() {
    let fixture = common::build(&[
        common::ScopeSpec::new("alpha", "alpha"),
        common::ScopeSpec::new("beta", "beta"),
    ]);
    let alpha = fixture.scope("alpha");
    let handler = new_handler(&fixture);

    let session_id = common::seed(1);
    handler
        .handle_command(cmd(
            alpha,
            "agent.start",
            serde_json::json!({ "session_id": session_id.to_string(), "agent_name": "agent" }),
        ))
        .expect("agent.start succeeds");

    let by_session = handler
        .handle_query(qry(
            serde_json::json!({ "session_id": session_id.to_string() }),
        ))
        .expect("agent.list by --session succeeds");
    let by_scope = handler
        .handle_query(qry(serde_json::json!({ "scope_id": alpha.to_string() })))
        .expect("agent.list by --scope succeeds");

    assert_eq!(
        by_session.result, by_scope.result,
        "the two forms must resolve to the same caller and therefore the same answer"
    );
}

// --- decision 8: targetable comes from the shared delegation rule ---------

/// Mutation 2's own target: deleting the `rule::check` call (or marking
/// everything targetable regardless) must die here. This fixture gives the
/// caller three different kinships to the scopes in the listing — itself,
/// its own ancestor, and a sibling — so a blanket "always true" cannot pass
/// by accident the way it could with only one other scope in play.
#[test]
fn targetable_reflects_kinship_computed_by_the_shared_rule() {
    let fixture = common::build(&[
        common::ScopeSpec::new("alpha", "alpha"),
        common::ScopeSpec::new("beta", "beta"),
    ]);
    let alpha = fixture.scope("alpha");
    let beta = fixture.scope("beta");
    let root = fixture.root_scope_id;
    let handler = new_handler(&fixture);

    let listed = handler
        .handle_query(qry(serde_json::json!({ "scope_id": alpha.to_string() })))
        .expect("agent.list succeeds");
    let scopes = listed.result["scopes"]
        .as_array()
        .expect("scopes is an array");

    let self_row = find_scope(scopes, alpha);
    assert_eq!(
        self_row["targetable"], false,
        "a scope may never target itself: {self_row:?}"
    );
    // Station 12 drill, defect 4: the exact wording, not merely a substring
    // — a `.contains("itself")` check here would have passed against the
    // drill's own broken text (`"…0001 is itself of …0001"`) just as
    // happily as against the fix, so it would not have caught the defect.
    assert_eq!(
        self_row["refusal_reason"]
            .as_str()
            .expect("refusal_reason is set"),
        "the calling scope itself",
    );

    let ancestor_row = find_scope(scopes, root);
    assert_eq!(
        ancestor_row["targetable"], false,
        "an agent may not target its own ancestor: {ancestor_row:?}"
    );
    assert_eq!(
        ancestor_row["refusal_reason"]
            .as_str()
            .expect("refusal_reason is set"),
        "an ancestor of the calling scope",
    );

    // The actual defect: raw scope ids in a field the reader has to
    // cross-reference against ids the same row already spells out as
    // names. Neither `alpha`'s nor `root`'s id may appear in either
    // message now that both are worded generically.
    for row in [self_row, ancestor_row] {
        let reason = row["refusal_reason"].as_str().unwrap();
        assert!(!reason.contains(&alpha.to_string()), "{row:?}");
        assert!(!reason.contains(&root.to_string()), "{row:?}");
    }

    let sibling_row = find_scope(scopes, beta);
    assert_eq!(
        sibling_row["targetable"], true,
        "a sibling is targetable: {sibling_row:?}"
    );
    assert!(sibling_row["refusal_reason"].is_null(), "{sibling_row:?}");
}

/// The third refused kinship `targetable_reflects_kinship_computed_by_the_shared_rule`
/// cannot exercise: `common::build`'s scopes are all direct children of the
/// fixture's root, so none of them are ever `Unrelated` to each other. A
/// nested `path` (`factory_registry::resolve` derives `parent_id` from
/// filesystem nesting, not from a field in `config.yaml`) builds the two
/// two-level branches needed for a cousin pair.
#[test]
fn unrelated_scope_is_worded_as_a_nephew_or_cousin_reached_through_its_parent() {
    let fixture = common::build(&[
        common::ScopeSpec::new("parent-a", "parent-a"),
        common::ScopeSpec::new("child-a1", "parent-a/child-a1"),
        common::ScopeSpec::new("parent-b", "parent-b"),
        common::ScopeSpec::new("child-b1", "parent-b/child-b1"),
    ]);
    let child_a1 = fixture.scope("child-a1");
    let child_b1 = fixture.scope("child-b1");
    let handler = new_handler(&fixture);

    let listed = handler
        .handle_query(qry(serde_json::json!({ "scope_id": child_a1.to_string() })))
        .expect("agent.list succeeds");
    let scopes = listed.result["scopes"]
        .as_array()
        .expect("scopes is an array");

    let cousin_row = find_scope(scopes, child_b1);
    assert_eq!(
        cousin_row["targetable"], false,
        "a cousin is not targetable: {cousin_row:?}"
    );
    assert_eq!(
        cousin_row["refusal_reason"]
            .as_str()
            .expect("refusal_reason is set"),
        "unrelated to the calling scope — a nephew or cousin, reached through its parent",
    );
    let reason = cousin_row["refusal_reason"].as_str().unwrap();
    assert!(!reason.contains(&child_a1.to_string()), "{cousin_row:?}");
    assert!(!reason.contains(&child_b1.to_string()), "{cousin_row:?}");
}

/// `agent.list` must call `rule::check` with `Sender::Agent { scope_id:
/// caller }`, never `Sender::Human` — a human sender is exempt from kinship
/// entirely (design §6), so a caller resolved as a human would see every
/// registered scope as targetable, hiding exactly the refusal this row
/// exists to report. `self_row` above already proves the sender is *some*
/// agent (a human sender would never refuse a same-scope target as
/// "itself" — `check`'s own `Sender::Human` arm never inspects kinship at
/// all), so this is only restated here for the mutation report, not a new
/// assertion.
#[test]
fn caller_is_never_treated_as_a_human_sender() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let alpha = fixture.scope("alpha");
    let handler = new_handler(&fixture);

    let listed = handler
        .handle_query(qry(serde_json::json!({ "scope_id": alpha.to_string() })))
        .expect("agent.list succeeds");
    let scopes = listed.result["scopes"]
        .as_array()
        .expect("scopes is an array");
    let self_row = find_scope(scopes, alpha);
    assert_eq!(self_row["targetable"], false);
}

// --- decision 9: availability -----------------------------------------

#[test]
fn every_registered_scope_and_agent_is_listed_even_with_no_live_session() {
    let fixture = common::build(&[
        common::ScopeSpec::new("alpha", "alpha"),
        common::ScopeSpec::new("beta", "beta"),
    ]);
    let alpha = fixture.scope("alpha");
    let beta = fixture.scope("beta");
    let root = fixture.root_scope_id;
    let handler = new_handler(&fixture);

    let listed = handler
        .handle_query(qry(serde_json::json!({ "scope_id": alpha.to_string() })))
        .expect("agent.list succeeds");
    let scopes = listed.result["scopes"]
        .as_array()
        .expect("scopes is an array");
    assert_eq!(
        scopes.len(),
        3,
        "every registered scope must be listed: {scopes:?}"
    );

    for (scope_id, agent_name) in [(alpha, "agent"), (beta, "agent"), (root, "company-agent")] {
        let row = find_scope(scopes, scope_id);
        let agent_row = find_agent(row, agent_name);
        assert_eq!(
            agent_row["availability"], "no_session",
            "an agent with no live session must still be listed, with its availability stated: {agent_row:?}"
        );
    }
}

#[test]
fn a_running_session_with_no_task_is_idle() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let alpha = fixture.scope("alpha");
    let handler = new_handler(&fixture);

    handler
        .handle_command(cmd(
            alpha,
            "agent.start",
            serde_json::json!({ "session_id": common::seed(1).to_string(), "agent_name": "agent" }),
        ))
        .expect("agent.start succeeds");

    let listed = handler
        .handle_query(qry(serde_json::json!({ "scope_id": alpha.to_string() })))
        .expect("agent.list succeeds");
    let scopes = listed.result["scopes"].as_array().unwrap();
    let agent_row = find_agent(find_scope(scopes, alpha), "agent");
    assert_eq!(agent_row["availability"], "idle");
}

#[test]
fn a_running_session_with_a_running_task_is_busy() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let alpha = fixture.scope("alpha");
    let handler = new_handler(&fixture);

    handler
        .handle_command(cmd(
            alpha,
            "agent.start",
            serde_json::json!({ "session_id": common::seed(1).to_string(), "agent_name": "agent" }),
        ))
        .expect("agent.start succeeds");
    handler
        .handle_command(cmd(
            alpha,
            "task.send",
            serde_json::json!({
                "task_id": common::seed(2).to_string(),
                "prompt": "do it",
                "agent_name": "agent",
            }),
        ))
        .expect("task.send succeeds");

    let listed = handler
        .handle_query(qry(serde_json::json!({ "scope_id": alpha.to_string() })))
        .expect("agent.list succeeds");
    let scopes = listed.result["scopes"].as_array().unwrap();
    let agent_row = find_agent(find_scope(scopes, alpha), "agent");
    assert_eq!(agent_row["availability"], "busy");
}

#[test]
fn a_starting_session_is_starting() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let alpha = fixture.scope("alpha");
    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();
    adapter.set_start_confidence(factory_adapter::Confidence::Degraded);
    let handler = FactoryHandler::new(store, adapter, fixture.instance_root());

    let start = handler
        .handle_command(cmd(
            alpha,
            "agent.start",
            serde_json::json!({ "session_id": common::seed(1).to_string(), "agent_name": "agent" }),
        ))
        .expect("agent.start succeeds");
    assert_eq!(start.result["state"], "starting");

    let listed = handler
        .handle_query(qry(serde_json::json!({ "scope_id": alpha.to_string() })))
        .expect("agent.list succeeds");
    let scopes = listed.result["scopes"].as_array().unwrap();
    let agent_row = find_agent(find_scope(scopes, alpha), "agent");
    assert_eq!(agent_row["availability"], "starting");
}

/// Mutation 3's own target: a `disconnected` session must never read as
/// `idle`. ADR 0017/ADR 0022 decision 9: the observer has lost sight of it,
/// and reporting `idle` would queue work behind a session that will not
/// free up.
#[test]
fn a_disconnected_session_is_unknown_never_idle() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let alpha = fixture.scope("alpha");
    let handler = new_handler(&fixture);

    let session_id = common::seed(1);
    handler
        .handle_command(cmd(
            alpha,
            "agent.start",
            serde_json::json!({ "session_id": session_id.to_string(), "agent_name": "agent" }),
        ))
        .expect("agent.start succeeds");

    {
        let mut store = factory_store::Store::open(fixture.instance_root()).expect("open store");
        factory_session::mark_disconnected(&mut store, session_id).expect("mark_disconnected");
    }

    let listed = handler
        .handle_query(qry(serde_json::json!({ "scope_id": alpha.to_string() })))
        .expect("agent.list succeeds");
    let scopes = listed.result["scopes"].as_array().unwrap();
    let agent_row = find_agent(find_scope(scopes, alpha), "agent");
    assert_eq!(
        agent_row["availability"], "unknown",
        "a disconnected session must never be reported as idle: {agent_row:?}"
    );
}

/// The defect the coordinator's rework caught: `busy` must never outrank
/// `idle`. An agent holding two live sessions at once — one working a task,
/// one sitting idle — has capacity right now, and reporting `busy` sends a
/// caller elsewhere while a free session waits: design §7's own guard
/// ("a caller does not queue work behind a session that will not free up"),
/// reproduced by the ranking that was meant to prevent it. Requires two
/// genuinely live sessions for one agent, each with its own leased
/// workspace (ADR 0012 decision 5: a lease is per directory, not per
/// session) — a fixture with only one session per agent cannot exercise
/// this at all.
#[test]
fn an_idle_session_outranks_a_busy_one_held_by_the_same_agent() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let alpha = fixture.scope("alpha");
    let handler = new_handler(&fixture);

    let busy_workspace = fixture.instance_root().join("busy-workspace");
    std::fs::create_dir_all(&busy_workspace).expect("create busy workspace dir");
    let idle_workspace = fixture.instance_root().join("idle-workspace");
    std::fs::create_dir_all(&idle_workspace).expect("create idle workspace dir");

    let busy_session = common::seed(1);
    handler
        .handle_command(cmd(
            alpha,
            "agent.start",
            serde_json::json!({
                "session_id": busy_session.to_string(),
                "agent_name": "agent",
                "workspace_path": busy_workspace.display().to_string(),
            }),
        ))
        .expect("start the session that will become busy");
    handler
        .handle_command(cmd(
            alpha,
            "task.send",
            serde_json::json!({
                "task_id": common::seed(2).to_string(),
                "prompt": "do it",
                "target_session_id": busy_session.to_string(),
            }),
        ))
        .expect("send a task straight to the busy session");

    let idle_session = common::seed(3);
    handler
        .handle_command(cmd(
            alpha,
            "agent.start",
            serde_json::json!({
                "session_id": idle_session.to_string(),
                "agent_name": "agent",
                "workspace_path": idle_workspace.display().to_string(),
            }),
        ))
        .expect("start a second, idle session for the same agent");

    let listed = handler
        .handle_query(qry(serde_json::json!({ "scope_id": alpha.to_string() })))
        .expect("agent.list succeeds");
    let scopes = listed.result["scopes"].as_array().unwrap();
    let agent_row = find_agent(find_scope(scopes, alpha), "agent");
    assert_eq!(
        agent_row["availability"], "idle",
        "a genuinely idle session must win over a busy one held by the same agent: {agent_row:?}"
    );
}
