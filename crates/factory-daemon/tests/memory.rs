//! `memory.add`, `memory.list` (ADR 0022, decision 12 in particular).

mod common;

use factory_daemon::envelope::{CommandRequest, QueryRequest};
use factory_daemon::{FactoryHandler, Handler};

fn uid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

fn cmd(scope_id: uuid::Uuid, command: &str, payload: serde_json::Value) -> CommandRequest {
    CommandRequest {
        request_id: uid(9800),
        scope_id,
        command: command.to_string(),
        payload,
        expected_revision: None,
    }
}

fn qry(scope_id: uuid::Uuid, query: &str, payload: serde_json::Value) -> QueryRequest {
    QueryRequest {
        request_id: uid(9801),
        scope_id,
        query: query.to_string(),
        payload,
    }
}

fn new_handler(fixture: &common::Fixture) -> FactoryHandler {
    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();
    FactoryHandler::new(store, adapter, fixture.instance_root())
}

#[test]
fn add_then_list_round_trips() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let alpha = fixture.scope("alpha");
    let handler = new_handler(&fixture);

    let entry_id = uid(500);
    let added = handler
        .handle_command(cmd(
            alpha,
            "memory.add",
            serde_json::json!({ "entry_id": entry_id.to_string(), "entry": "remember this" }),
        ))
        .expect("memory.add succeeds");
    assert_eq!(added.result["scope"], "alpha");
    assert_eq!(added.result["id"], entry_id.to_string());
    // Station 12 drill, defect 2: a stamped `created_at` is second
    // resolution, matching every other timestamp this system stamps
    // (SQLite's own `CURRENT_TIMESTAMP`). The mutation this assertion
    // exists to kill is reverting `ops::memory::add`'s `to_rfc3339_opts`
    // back to bare `to_rfc3339`, which reintroduces microseconds.
    let created_at = added.result["created_at"]
        .as_str()
        .expect("created_at is a string");
    chrono::DateTime::parse_from_rfc3339(created_at).expect("created_at is RFC3339");
    assert!(
        !created_at.contains('.'),
        "created_at must be second resolution, not sub-second: {created_at}"
    );

    let listed = handler
        .handle_query(qry(alpha, "memory.list", serde_json::json!({})))
        .expect("memory.list succeeds");
    let entries = listed.result["entries"]
        .as_array()
        .expect("entries is an array");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["id"], entry_id.to_string());
    assert_eq!(entries[0]["text"], "remember this");

    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let scope_id_col: String = store
        .connection()
        .query_row(
            "SELECT scope_id FROM durable_writes WHERE kind = 'memory' AND name = ?1",
            [entry_id.to_string()],
            |row| row.get(0),
        )
        .expect("row exists");
    assert_eq!(scope_id_col, alpha.to_string());
}

#[test]
fn an_empty_entry_is_refused() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let alpha = fixture.scope("alpha");
    let handler = new_handler(&fixture);

    let err = handler
        .handle_command(cmd(
            alpha,
            "memory.add",
            serde_json::json!({ "entry_id": uid(501).to_string(), "entry": "   \n " }),
        ))
        .unwrap_err();
    assert_eq!(err.code, "validation.empty_entry");
}

/// Mutation 9's own target: `memory add` must write only under the calling
/// scope's own directory, never another scope's — proved by checking both
/// directions, not just that the call succeeded.
#[test]
fn add_writes_only_under_the_calling_scopes_own_directory() {
    let fixture = common::build(&[
        common::ScopeSpec::new("alpha", "alpha"),
        common::ScopeSpec::new("beta", "beta"),
    ]);
    let alpha = fixture.scope("alpha");
    let handler = new_handler(&fixture);

    handler
        .handle_command(cmd(
            alpha,
            "memory.add",
            serde_json::json!({ "entry_id": uid(502).to_string(), "entry": "alpha's own memory" }),
        ))
        .expect("memory.add succeeds");

    let memory_root = fixture.instance_root().join(".factory").join("memory");
    let alpha_dir = memory_root.join("alpha");
    let beta_dir = memory_root.join("beta");

    let alpha_files: Vec<_> = std::fs::read_dir(&alpha_dir)
        .expect("alpha's memory directory exists")
        .collect();
    assert_eq!(
        alpha_files.len(),
        1,
        "the entry must land under alpha's own directory"
    );

    assert!(
        !beta_dir.exists() || std::fs::read_dir(&beta_dir).unwrap().next().is_none(),
        "beta's memory directory must not receive alpha's entry"
    );
}

/// §12's own acceptance criterion: adding a new entry leaves every existing
/// one untouched.
#[test]
fn add_leaves_existing_entries_unmodified() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let alpha = fixture.scope("alpha");
    let handler = new_handler(&fixture);

    handler
        .handle_command(cmd(
            alpha,
            "memory.add",
            serde_json::json!({ "entry_id": uid(503).to_string(), "entry": "first entry" }),
        ))
        .expect("first add succeeds");

    let memory_root = fixture.instance_root().join(".factory").join("memory");
    let alpha_dir = memory_root.join("alpha");
    let first_file = std::fs::read_dir(&alpha_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let first_contents_before = std::fs::read_to_string(&first_file).unwrap();

    handler
        .handle_command(cmd(
            alpha,
            "memory.add",
            serde_json::json!({ "entry_id": uid(504).to_string(), "entry": "second entry" }),
        ))
        .expect("second add succeeds");

    let first_contents_after = std::fs::read_to_string(&first_file).unwrap();
    assert_eq!(
        first_contents_before, first_contents_after,
        "the first entry's file must be byte-identical after a second, unrelated add"
    );

    let entry_count = std::fs::read_dir(&alpha_dir).unwrap().count();
    assert_eq!(entry_count, 2);
}

/// Mutation 7's own target, for `memory.add`: `scopes.name` carries no
/// UNIQUE constraint, so two registered scopes can share a name, and a
/// directory keyed by name would merge their entries unrecoverably.
#[test]
fn add_refuses_an_ambiguous_scope_name_and_names_the_collision() {
    // Two scopes, same name, different paths — `scopes.name` has no UNIQUE
    // constraint, so the registry accepts both.
    let fixture = common::build(&[
        common::ScopeSpec::new("dup", "dup-a"),
        common::ScopeSpec::new("dup", "dup-b"),
    ]);
    // `common::build` mints each scope's id as `uid(2 + index)` — see that
    // module's own doc comment; `fixture.scope("dup")` cannot be used here
    // since the fixture's own lookup table can only keep one of the two.
    let first = uid(2);
    let second = uid(3);
    let handler = new_handler(&fixture);

    let err = handler
        .handle_command(cmd(
            first,
            "memory.add",
            serde_json::json!({ "entry_id": uid(505).to_string(), "entry": "whichever" }),
        ))
        .unwrap_err();
    assert_eq!(err.code, "conflict.ambiguous_scope_name");
    assert!(err.message.contains("dup"), "{}", err.message);
    assert!(
        err.message.contains(&first.to_string()) && err.message.contains(&second.to_string()),
        "the refusal must name which scopes collided: {}",
        err.message
    );

    // Nothing must have been written for either colliding scope.
    let memory_root = fixture.instance_root().join(".factory").join("memory");
    assert!(!memory_root.join("dup").exists());
}

/// The same refusal, reached through `memory.list` — decision 12 covers
/// both operations, and a mutation that only guards `add` still leaves the
/// read path free to merge two scopes' entries in its own output.
#[test]
fn list_refuses_an_ambiguous_scope_name_too() {
    let fixture = common::build(&[
        common::ScopeSpec::new("dup", "dup-a"),
        common::ScopeSpec::new("dup", "dup-b"),
    ]);
    let second = uid(3);
    let handler = new_handler(&fixture);

    let err = handler
        .handle_query(qry(second, "memory.list", serde_json::json!({})))
        .unwrap_err();
    assert_eq!(err.code, "conflict.ambiguous_scope_name");
}

#[test]
fn an_unregistered_scope_is_not_found() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let handler = new_handler(&fixture);

    let err = handler
        .handle_command(cmd(
            uid(9999),
            "memory.add",
            serde_json::json!({ "entry_id": uid(506).to_string(), "entry": "text" }),
        ))
        .unwrap_err();
    assert_eq!(err.code, "not_found.scope");
}
