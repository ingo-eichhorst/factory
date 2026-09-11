//! Every `fixtures/invalid/*.yaml` fails, and the exact rendered `Display`
//! string is asserted against `docs/slice-1-error-corpus.md`, as amended by
//! ADR 0015.
//!
//! Every fixture here was constructed to hit the corpus's stated line/column
//! exactly, and does -- including case 1, which recovers the `agent:` /
//! `agents:` key's own line by scanning the source *within the failing
//! scope entry's own line range* (see `validate::entry_key_location`), since
//! `Spanned<T>` can only report where a key's *value* begins, and ADR 0015
//! moved these keys from the document's top level into an indented scope
//! entry.

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

/// Case 1. The fixture's `agent:` key is on line 11 and `agents:` on line 15;
/// both locations must be the key's own line, recovered by
/// `validate::entry_key_location` scanning the source within this entry's own
/// line range, not the one-line-off value-node location `Spanned<T>` reports
/// directly. Per ADR 0015's amendment, the summary also names the scope.
#[test]
fn agent_and_agents_both_set() {
    let path = fixture("agent-and-agents-both-set.yaml");
    let err = load_err("agent-and-agents-both-set.yaml");
    assert_eq!(
        err.to_string(),
        format!(
            "error: scope `both-set` sets both `agent` and `agents`; a scope uses one or the other\n\
             \x20\x20--> {p}:11:5\n\
             \x20\x20note: `agent` shorthand defined here\n\
             \x20\x20note: `agents` list defined at {p}:15:5\n\
             \x20\x20help: keep `agents:` and delete the `agent:` block, or keep `agent:` and delete `agents:`",
            p = path.display()
        )
    );
}

/// The fallback's defensive half, exercised end to end (the unit tests in
/// `validate::key_recovery_tests` exercise the scan function in isolation): a
/// comment mentioning `agent:` at the entry's own indentation sits right
/// above the real key and must not shift, duplicate, or corrupt the reported
/// location.
#[test]
fn agent_and_agents_both_set_with_a_decoy_comment() {
    let path = fixture("agent-and-agents-both-set-with-comment.yaml");
    let err = load_err("agent-and-agents-both-set-with-comment.yaml");
    assert_eq!(
        err.to_string(),
        format!(
            "error: scope `both-set-with-comment` sets both `agent` and `agents`; a scope uses one or the other\n\
             \x20\x20--> {p}:13:5\n\
             \x20\x20note: `agent` shorthand defined here\n\
             \x20\x20note: `agents` list defined at {p}:17:5\n\
             \x20\x20help: keep `agents:` and delete the `agent:` block, or keep `agent:` and delete `agents:`",
            p = path.display()
        )
    );
}

/// The hazard the coordinator found in production before ADR 0015: the live
/// `assistant` scope's own file (and `fixtures/valid/unknown-toplevel.yaml`,
/// which copies its shape) has a `runtime:` block owned by a different tool
/// that itself contains a nested, indented `agents:` key. Since a scope entry
/// is now `deny_unknown_fields`, that block can no longer live *inside* the
/// entry the way it could in the old per-scope file, so this fixture places
/// it as an unrelated top-level sibling of `instance`/`scopes` instead --
/// entirely outside the failing entry's own line range. The scan must still
/// land on the entry's real `agent:`/`agents:` keys rather than being
/// confused by the decoy sitting earlier in the same file.
#[test]
fn agent_and_agents_both_set_with_a_nested_agents_decoy() {
    let path = fixture("agent-and-agents-both-set-with-nested-agents-decoy.yaml");
    let err = load_err("agent-and-agents-both-set-with-nested-agents-decoy.yaml");
    assert_eq!(
        err.to_string(),
        format!(
            "error: scope `both-set-with-nested-agents-decoy` sets both `agent` and `agents`; a scope uses one or the other\n\
             \x20\x20--> {p}:24:5\n\
             \x20\x20note: `agent` shorthand defined here\n\
             \x20\x20note: `agents` list defined at {p}:28:5\n\
             \x20\x20help: keep `agents:` and delete the `agent:` block, or keep `agent:` and delete `agents:`",
            p = path.display()
        )
    );
}

/// The hazard ADR 0015 introduces that did not exist before: a *sibling*
/// scope entry's own `agent:` key sits at exactly the same indentation as the
/// failing entry's, since both are list items of the same `scopes:` list. The
/// entry-relative scan must bound its search to the failing entry's own line
/// range, or the sibling's `agent:` would create a second match and wrongly
/// degrade to the value-node fallback instead of reporting the failing
/// entry's own, correct key locations.
#[test]
fn agent_and_agents_both_set_with_a_sibling_entry_decoy() {
    let path = fixture("agent-and-agents-both-set-with-sibling-entry-decoy.yaml");
    let err = load_err("agent-and-agents-both-set-with-sibling-entry-decoy.yaml");
    assert_eq!(
        err.to_string(),
        format!(
            "error: scope `sibling-b-both-set` sets both `agent` and `agents`; a scope uses one or the other\n\
             \x20\x20--> {p}:24:5\n\
             \x20\x20note: `agent` shorthand defined here\n\
             \x20\x20note: `agents` list defined at {p}:28:5\n\
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
            "error: scope `no-agent` has no agent defined; a scope configures at least one agent\n\
             \x20\x20--> {p}:8:5\n\
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
            "error: two agents in scope `dup-scope` are both named `assistant`\n\
             \x20\x20--> {p}:15:9\n\
             \x20\x20note: first defined at {p}:12:9\n\
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
            "error: scope `X` sets unsupported lifetime `ephemeral`\n\
             \x20\x20--> {p}:13:19\n\
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
            "error: scope `harness-bad` sets unsupported harness `bash`\n\
             \x20\x20--> {p}:12:16\n\
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
            "error: scope `harness-near-miss` sets unsupported harness `claude`\n\
             \x20\x20--> {p}:12:16\n\
             \x20\x20help: did you mean `claude-code`? supported harnesses are `claude-code`, `opencode`, and `pi`",
            p = path.display()
        )
    );
}

#[test]
fn invalid_scope_uuid() {
    let path = fixture("uuid-invalid.yaml");
    let err = load_err("uuid-invalid.yaml");
    assert_eq!(
        err.to_string(),
        format!(
            "error: scope `uuid-bad`'s `id` is not a UUID: `not-a-uuid`\n\
             \x20\x20--> {p}:8:9\n\
             \x20\x20help: generate one with `uuidgen`; a scope ID is permanent identity and must not be reused between scopes",
            p = path.display()
        )
    );
}

/// New under ADR 0015: `instance.id` is a second place a UUID can be
/// invalid, distinct from a scope entry's own `id` -- the summary and help
/// name the instance rather than a scope, since there is no scope to name.
#[test]
fn invalid_instance_uuid() {
    let path = fixture("instance-uuid-invalid.yaml");
    let err = load_err("instance-uuid-invalid.yaml");
    assert_eq!(
        err.to_string(),
        format!(
            "error: `instance.id` is not a UUID: `not-a-uuid`\n\
             \x20\x20--> {p}:4:7\n\
             \x20\x20help: generate one with `uuidgen`; the instance ID is permanent identity and must not be reused between instances",
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

/// The new document shape's analogue of the old "missing `scope:`" case: a
/// file with only `version: 1` is missing the required `instance:` block.
/// `serde-saphyr` reports the first missing required field it processed,
/// which for `RawDocument`'s field order is `instance` before `scopes`.
#[test]
fn missing_required_field() {
    let path = fixture("missing-instance.yaml");
    let err = load_err("missing-instance.yaml");
    assert_eq!(
        err.to_string(),
        format!(
            "error: missing field `instance`\n\
             \x20\x20--> {p}:1:1\n\
             \x20\x20help: add an `instance:` block with an `id` and a `name`",
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
             \x20\x20--> {p}:4:7\n\
             \x20\x20help: expected one of `harness`, `lifetime`, `max_sessions`, `model`, `name`; `max_sesions` looks like a typo for `max_sessions`",
            p = path.display()
        )
    );
}

/// `raw::mapping_name_for_fields` gained new match arms for `instance` and
/// `scope` under ADR 0015's document shape. Verified against a real fixture
/// rather than trusted as an assumption: an unmatched arm would silently
/// render `` in `configuration` `` instead, which no other test would catch.
#[test]
fn unknown_field_inside_instance() {
    let path = fixture("unknown-field-instance.yaml");
    let err = load_err("unknown-field-instance.yaml");
    assert_eq!(
        err.to_string(),
        format!(
            "error: unknown field `nam` in `instance`\n\
             \x20\x20--> {p}:6:3\n\
             \x20\x20help: expected one of `id`, `name`; `nam` looks like a typo for `name`",
            p = path.display()
        )
    );
}

/// The `scope` counterpart of the same regression, for a scope entry rather
/// than the `instance:` block.
#[test]
fn unknown_field_inside_scope_entry() {
    let path = fixture("unknown-field-scope.yaml");
    let err = load_err("unknown-field-scope.yaml");
    assert_eq!(
        err.to_string(),
        format!(
            "error: unknown field `pathh` in `scope`\n\
             \x20\x20--> {p}:11:5\n\
             \x20\x20help: expected one of `agent`, `agents`, `git`, `id`, `name`, `path`; `pathh` looks like a typo for `path`",
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
            "error: scope `zero-sessions` sets `max_sessions` to 0, so this agent could never start a session\n\
             \x20\x20--> {p}:12:21\n\
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
             \x20\x20help: an instance configuration needs at least `version`, `instance`, and `scopes`",
            p = path.display()
        )
    );
}

/// Case 13, now an intra-file check per ADR 0015: both entries are in the
/// same instance file and the check runs on every load, not only when a
/// caller remembers to call a separate cross-file function.
#[test]
fn duplicate_scope_ids_in_one_file() {
    let path = fixture("duplicate-scope-ids.yaml");
    let err = load_err("duplicate-scope-ids.yaml");
    assert_eq!(
        err.to_string(),
        format!(
            "error: two scopes share the ID `73995414-d129-456f-82fe-c3f7f657be59`\n\
             \x20\x20--> {p}:17:9\n\
             \x20\x20note: also used by scope `scope-a` at {p}:10:9\n\
             \x20\x20help: a scope ID is permanent identity; if this file was copied, generate a new ID with `uuidgen`",
            p = path.display(),
        )
    );
}

/// ADR 0015's companion check: two scopes may not share a `path` either,
/// with the same shape of message, naming both entries.
#[test]
fn duplicate_scope_paths_in_one_file() {
    let path = fixture("duplicate-scope-paths.yaml");
    let err = load_err("duplicate-scope-paths.yaml");
    assert_eq!(
        err.to_string(),
        format!(
            "error: two scopes share the path `projects/shared`\n\
             \x20\x20--> {p}:19:11\n\
             \x20\x20note: also used by scope `scope-a` at {p}:12:11\n\
             \x20\x20help: a scope path names the one directory it claims; if this scope was copied, point `path` at its own directory",
            p = path.display(),
        )
    );
}
