//! Every `fixtures/invalid/*.yaml` fails, and the exact rendered `Display`
//! string is asserted against `docs/slice-1-error-corpus.md`.
//!
//! Every fixture here was constructed to hit the corpus's stated line/column
//! exactly, and does -- including case 1, which recovers the `agent:` /
//! `agents:` key's own line by scanning the source (see
//! `validate::toplevel_key_location`), since `Spanned<T>` can only report
//! where a key's *value* begins.

use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/invalid")
        .join(name)
}

fn load_err(name: &str) -> factory_config::ConfigError {
    let path = fixture(name);
    factory_config::load(&path).expect_err("fixture is invalid")
}

/// Case 1. The fixture's `agent:` key is on line 7 and `agents:` on line 11;
/// both locations must be the key's own line, recovered by
/// `validate::toplevel_key_location` scanning the source, not the one-line-off
/// value-node location `Spanned<T>` reports directly.
#[test]
fn agent_and_agents_both_set() {
    let path = fixture("agent-and-agents-both-set.yaml");
    let err = load_err("agent-and-agents-both-set.yaml");
    assert_eq!(
        err.to_string(),
        format!(
            "error: `agent` and `agents` are both set; a scope uses one or the other\n\
             \x20\x20--> {p}:7:1\n\
             \x20\x20note: `agent` shorthand defined here\n\
             \x20\x20note: `agents` list defined at {p}:11:1\n\
             \x20\x20help: keep `agents:` and delete the `agent:` block, or keep `agent:` and delete `agents:`",
            p = path.display()
        )
    );
}

/// The fallback's defensive half, exercised end to end (the unit tests in
/// `validate::key_recovery_tests` exercise the scan function in isolation): a
/// comment mentioning `agent:` sits right above the real key and must not
/// shift, duplicate, or corrupt the reported location.
#[test]
fn agent_and_agents_both_set_with_a_decoy_comment() {
    let path = fixture("agent-and-agents-both-set-with-comment.yaml");
    let err = load_err("agent-and-agents-both-set-with-comment.yaml");
    assert_eq!(
        err.to_string(),
        format!(
            "error: `agent` and `agents` are both set; a scope uses one or the other\n\
             \x20\x20--> {p}:9:1\n\
             \x20\x20note: `agent` shorthand defined here\n\
             \x20\x20note: `agents` list defined at {p}:13:1\n\
             \x20\x20help: keep `agents:` and delete the `agent:` block, or keep `agent:` and delete `agents:`",
            p = path.display()
        )
    );
}

/// The hazard the coordinator found in production: the live `assistant`
/// scope (and `fixtures/valid/unknown-toplevel.yaml`, which copies its shape)
/// has a `runtime:` block owned by a different tool that itself contains a
/// nested, indented `agents:` key. This fixture is genuinely both-set *and*
/// carries that same nested `agents:` decoy at line 13, ahead of the real
/// top-level `agents:` at line 17. The scan's "first character is not
/// whitespace" rule must skip the indented decoy and report the real key --
/// a scan that got that wrong would silently point at the wrong line rather
/// than fail to detect the problem (serde already detects both-set; the scan
/// only chooses where to point at it).
#[test]
fn agent_and_agents_both_set_with_a_nested_agents_decoy() {
    let path = fixture("agent-and-agents-both-set-with-nested-agents-decoy.yaml");
    let err = load_err("agent-and-agents-both-set-with-nested-agents-decoy.yaml");
    assert_eq!(
        err.to_string(),
        format!(
            "error: `agent` and `agents` are both set; a scope uses one or the other\n\
             \x20\x20--> {p}:7:1\n\
             \x20\x20note: `agent` shorthand defined here\n\
             \x20\x20note: `agents` list defined at {p}:17:1\n\
             \x20\x20help: keep `agents:` and delete the `agent:` block, or keep `agent:` and delete `agents:`",
            p = path.display()
        )
    );
}

#[test]
fn neither_agent_nor_agents() {
    let path = fixture("no-agent.yaml");
    let err = load_err("no-agent.yaml");
    assert_eq!(
        err.to_string(),
        format!(
            "error: no agent is defined; a scope configures at least one agent\n\
             \x20\x20--> {p}:1:1\n\
             \x20\x20help: add an `agent:` block, or an `agents:` list with at least one entry",
            p = path.display()
        )
    );
}

#[test]
fn two_agents_sharing_a_name() {
    let path = fixture("duplicate-agent-name.yaml");
    let err = load_err("duplicate-agent-name.yaml");
    assert_eq!(
        err.to_string(),
        format!(
            "error: two agents in this scope are both named `assistant`\n\
             \x20\x20--> {p}:8:5\n\
             \x20\x20note: first defined at {p}:5:5\n\
             \x20\x20help: agent names address a recipient in `factory task send`, so they must be unique within a scope; rename one",
            p = path.display()
        )
    );
}

#[test]
fn unsupported_lifetime() {
    let path = fixture("lifetime-unsupported.yaml");
    let err = load_err("lifetime-unsupported.yaml");
    assert_eq!(
        err.to_string(),
        format!(
            "error: unsupported lifetime `ephemeral`\n\
             \x20\x20--> {p}:7:15\n\
             \x20\x20help: supported lifetimes are `permanent` and `temporary`; `permanent` is the default and may be omitted",
            p = path.display()
        )
    );
}

#[test]
fn unsupported_harness() {
    let path = fixture("harness-unsupported.yaml");
    let err = load_err("harness-unsupported.yaml");
    assert_eq!(
        err.to_string(),
        format!(
            "error: unsupported harness `bash`\n\
             \x20\x20--> {p}:6:12\n\
             \x20\x20help: supported harnesses are `claude-code`, `opencode`, and `pi`",
            p = path.display()
        )
    );
}

#[test]
fn harness_claude_near_miss() {
    let path = fixture("harness-near-miss.yaml");
    let err = load_err("harness-near-miss.yaml");
    assert_eq!(
        err.to_string(),
        format!(
            "error: unsupported harness `claude`\n\
             \x20\x20--> {p}:6:12\n\
             \x20\x20help: did you mean `claude-code`? supported harnesses are `claude-code`, `opencode`, and `pi`",
            p = path.display()
        )
    );
}

#[test]
fn invalid_uuid() {
    let path = fixture("uuid-invalid.yaml");
    let err = load_err("uuid-invalid.yaml");
    assert_eq!(
        err.to_string(),
        format!(
            "error: `scope.id` is not a UUID: `not-a-uuid`\n\
             \x20\x20--> {p}:3:7\n\
             \x20\x20help: generate one with `uuidgen`; the scope ID is permanent identity and must not be reused between scopes",
            p = path.display()
        )
    );
}

#[test]
fn unsupported_version() {
    let path = fixture("version-unsupported.yaml");
    let err = load_err("version-unsupported.yaml");
    assert_eq!(
        err.to_string(),
        format!(
            "error: unsupported configuration version `2`\n\
             \x20\x20--> {p}:1:10\n\
             \x20\x20help: this build of Factory supports version 1",
            p = path.display()
        )
    );
}

#[test]
fn missing_required_field() {
    let path = fixture("missing-scope.yaml");
    let err = load_err("missing-scope.yaml");
    assert_eq!(
        err.to_string(),
        format!(
            "error: missing field `scope`\n\
             \x20\x20--> {p}:1:1\n\
             \x20\x20help: add a `scope:` block with an `id` and a `name`",
            p = path.display()
        )
    );
}

#[test]
fn unknown_field_inside_agent() {
    let path = fixture("unknown-field-agent.yaml");
    let err = load_err("unknown-field-agent.yaml");
    assert_eq!(
        err.to_string(),
        format!(
            "error: unknown field `max_sesions` in `agent`\n\
             \x20\x20--> {p}:3:3\n\
             \x20\x20help: expected one of `harness`, `lifetime`, `max_sessions`, `name`; `max_sesions` looks like a typo for `max_sessions`",
            p = path.display()
        )
    );
}

#[test]
fn max_sessions_zero() {
    let path = fixture("max-sessions-zero.yaml");
    let err = load_err("max-sessions-zero.yaml");
    assert_eq!(
        err.to_string(),
        format!(
            "error: `max_sessions` is 0, so this agent could never start a session\n\
             \x20\x20--> {p}:6:17\n\
             \x20\x20help: use at least 1, or remove the agent",
            p = path.display()
        )
    );
}

#[test]
fn empty_file() {
    let path = fixture("empty.yaml");
    let err = load_err("empty.yaml");
    assert_eq!(
        err.to_string(),
        format!(
            "error: the file is empty\n\
             \x20\x20--> {p}:1:1\n\
             \x20\x20help: a scope configuration needs at least `version`, `scope`, and one agent",
            p = path.display()
        )
    );
}

#[test]
fn duplicate_scope_ids_across_files() {
    let path_a = fixture("duplicate-id-a.yaml");
    let path_b = fixture("duplicate-id-b.yaml");
    let config_a = factory_config::load(&path_a).expect("fixture is valid on its own");
    let config_b = factory_config::load(&path_b).expect("fixture is valid on its own");

    // `validate_unique_ids` points `-->` at the *later* config in the slice
    // and notes the earlier one -- passing `b` then `a` here puts the arrow on
    // `duplicate-id-a.yaml`, matching the corpus's `<file-a>` / `<file-b>`
    // shape.
    let err = factory_config::validate_unique_ids(&[config_b, config_a])
        .expect_err("both scopes share an ID");

    assert_eq!(
        err.to_string(),
        format!(
            "error: two scopes share the ID `22243942-ae4e-474d-a837-5a49c5eb7a7c`\n\
             \x20\x20--> {a}:3:7\n\
             \x20\x20note: also used by scope `scope-b` at {b}:3:7\n\
             \x20\x20help: a scope ID is permanent identity; if this file was copied, generate a new ID with `uuidgen`",
            a = path_a.display(),
            b = path_b.display(),
        )
    );
}
