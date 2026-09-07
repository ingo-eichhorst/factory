//! Every `fixtures/valid/*.yaml` loads, and the resulting `ScopeConfig` is
//! checked field by field. Per `docs/slice-1-error-corpus.md`'s "Cases that
//! must load" table.

use std::path::{Path, PathBuf};

use factory_config::{Agent, Harness, Lifetime, Location, Scope, ScopeConfig};
use uuid::Uuid;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/valid")
        .join(name)
}

fn uuid(s: &str) -> Uuid {
    Uuid::parse_str(s).expect("fixture UUIDs in this test are hand-verified valid")
}

#[test]
fn root_shorthand_yields_exactly_one_agent() {
    let path = fixture("root-shorthand.yaml");
    let config = factory_config::load(&path).expect("fixture is valid");

    assert_eq!(
        config,
        ScopeConfig {
            version: 1,
            scope: Scope {
                id: uuid("a0558c01-de66-4ad9-a6ca-43c7e5e1aa5d"),
                name: "root-shorthand".to_string(),
            },
            agents: vec![Agent {
                name: "Solo".to_string(),
                harness: Harness::Pi,
                max_sessions: 3,
                lifetime: Lifetime::Permanent,
            }],
            origin: path.clone(),
            scope_id_location: Location {
                file: path,
                line: 4,
                column: 7,
            },
        }
    );
}

#[test]
fn multi_agent_list_yields_several_in_order() {
    let path = fixture("multi-agent.yaml");
    let config = factory_config::load(&path).expect("fixture is valid");

    assert_eq!(
        config,
        ScopeConfig {
            version: 1,
            scope: Scope {
                id: uuid("1f11c010-c37e-4b1e-8e25-6f70fe91b043"),
                name: "multi-agent".to_string(),
            },
            agents: vec![
                Agent {
                    name: "First".to_string(),
                    harness: Harness::Pi,
                    max_sessions: 1,
                    lifetime: Lifetime::Permanent,
                },
                Agent {
                    name: "Second".to_string(),
                    harness: Harness::ClaudeCode,
                    max_sessions: 4,
                    lifetime: Lifetime::Temporary,
                },
                Agent {
                    name: "Third".to_string(),
                    harness: Harness::Opencode,
                    max_sessions: 1,
                    lifetime: Lifetime::Permanent,
                },
            ],
            origin: path.clone(),
            scope_id_location: Location {
                file: path,
                line: 4,
                column: 7,
            },
        }
    );
}

/// The `agent:` shorthand and the `agents:` list produce the same shape for
/// an *equivalent* agent — the distinction is a spelling in the file, not a
/// difference in the model (see `ScopeConfig`'s doc comment). Same scope,
/// same single agent's fields, one spelled each way: the resulting `agents`
/// vectors must be equal, not just similar.
#[test]
fn shorthand_and_list_produce_the_same_agent_shape() {
    let shorthand = factory_config::parse(
        "version: 1\n\
         scope:\n  id: 8a9ea91e-32af-46c3-9809-26d770b1e922\n  name: same\n\
         agent:\n  name: Solo\n  harness: pi\n  max_sessions: 3\n",
        "shorthand.yaml",
    )
    .expect("shorthand form parses");
    let listed = factory_config::parse(
        "version: 1\n\
         scope:\n  id: 8a9ea91e-32af-46c3-9809-26d770b1e922\n  name: same\n\
         agents:\n  - name: Solo\n    harness: pi\n    max_sessions: 3\n",
        "listed.yaml",
    )
    .expect("list form parses");

    assert_eq!(shorthand.agents, listed.agents);
}

#[test]
fn unknown_toplevel_runtime_block_is_ignored_not_rejected() {
    let path = fixture("unknown-toplevel.yaml");
    let config = factory_config::load(&path).expect(
        "a `runtime:` block must be ignored per ADR 0009 rule 2 -- \
         if this fails, two live scopes (assistant, model-lab) have become unloadable",
    );

    assert_eq!(config.version, 1);
    assert_eq!(
        config.scope,
        Scope {
            id: uuid("fb58a4a1-c7ac-4f53-9f1f-fd770eee4268"),
            name: "assistant-fixture".to_string(),
        }
    );
    // Explicit, on top of the full-value equality below: this fixture's
    // `runtime:` block carries its own nested, indented `agents:` list
    // (matching the live `assistant` scope's shape exactly). That must
    // contribute zero agents -- a scan or parse that got greedy and picked it
    // up would silently produce a second agent here rather than fail loudly
    // where the mistake was made.
    assert_eq!(config.agents.len(), 1);
    assert_eq!(
        config.agents,
        vec![Agent {
            name: "Assistant".to_string(),
            harness: Harness::Pi,
            max_sessions: 2,
            lifetime: Lifetime::Permanent,
        }]
    );
    assert_eq!(config.origin, path.clone());
    // This fixture's six-line comment header pushes `scope.id` well past
    // line 4 (where it sits in every other fixture here), so this is real
    // coverage that the span comes from the file rather than a constant.
    assert_eq!(
        config.scope_id_location,
        Location {
            file: path,
            line: 10,
            column: 7,
        }
    );
}

#[test]
fn omitted_max_sessions_and_lifetime_take_their_defaults() {
    let path = fixture("defaults.yaml");
    let config = factory_config::load(&path).expect("fixture is valid");

    assert_eq!(
        config.agents,
        vec![Agent {
            name: "Defaulted".to_string(),
            harness: Harness::Pi,
            max_sessions: 1,
            lifetime: Lifetime::Permanent,
        }]
    );
    assert_eq!(
        config.scope_id_location,
        Location {
            file: path,
            line: 4,
            column: 7,
        }
    );
}
