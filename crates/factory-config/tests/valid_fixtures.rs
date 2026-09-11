//! Every `fixtures/valid/*.yaml` loads, and the resulting `InstanceConfig` is
//! checked field by field. Per `docs/slice-1-error-corpus.md`'s "Cases that
//! must load" table, as amended by ADR 0015.

use std::path::{Path, PathBuf};

use factory_config::{Agent, Harness, Instance, Lifetime, Location, ScopeEntry};
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

    assert_eq!(config.version, 1);
    assert_eq!(
        config.instance,
        Instance {
            id: uuid("f72da97f-140d-4f88-839b-685424441ed7"),
            name: "root-shorthand-instance".to_string(),
        }
    );
    assert_eq!(
        config.scopes,
        vec![ScopeEntry {
            id: uuid("8444ec60-970a-483d-9fcd-ef10938a1581"),
            name: "root-shorthand".to_string(),
            path: PathBuf::from("projects/root-shorthand"),
            git: None,
            agents: vec![Agent {
                name: "Solo".to_string(),
                harness: Harness::Pi,
                max_sessions: 3,
                lifetime: Lifetime::Permanent,
                model: None,
            }],
            id_location: Location {
                file: path.clone(),
                line: 8,
                column: 9,
            },
            path_location: Location {
                file: path.clone(),
                line: 10,
                column: 11,
            },
        }]
    );
    assert_eq!(config.origin, path);
}

#[test]
fn multi_agent_list_yields_several_in_order() {
    let path = fixture("multi-agent.yaml");
    let config = factory_config::load(&path).expect("fixture is valid");

    assert_eq!(config.scopes.len(), 1);
    assert_eq!(
        config.scopes[0].agents,
        vec![
            Agent {
                name: "First".to_string(),
                harness: Harness::Pi,
                max_sessions: 1,
                lifetime: Lifetime::Permanent,
                model: None,
            },
            Agent {
                name: "Second".to_string(),
                harness: Harness::ClaudeCode,
                max_sessions: 4,
                lifetime: Lifetime::Temporary,
                model: None,
            },
            Agent {
                name: "Third".to_string(),
                harness: Harness::Opencode,
                max_sessions: 1,
                lifetime: Lifetime::Permanent,
                model: None,
            },
        ]
    );
    assert_eq!(
        config.scopes[0].id_location,
        Location {
            file: path,
            line: 8,
            column: 9,
        }
    );
}

/// The `agent:` shorthand and the `agents:` list produce the same shape for
/// an *equivalent* agent — the distinction is a spelling in the file, not a
/// difference in the model. Same instance and scope, same single agent's
/// fields, one spelled each way: the resulting `agents` vectors must be
/// equal, not just similar.
#[test]
fn shorthand_and_list_produce_the_same_agent_shape() {
    let shorthand = factory_config::parse(
        "version: 1\n\
         instance:\n  id: 8a9ea91e-32af-46c3-9809-26d770b1e922\n  name: same\n\
         scopes:\n  - id: 8a9ea91e-32af-46c3-9809-26d770b1e923\n    name: same\n    path: p\n    agent:\n      name: Solo\n      harness: pi\n      max_sessions: 3\n",
        "shorthand.yaml",
    )
    .expect("shorthand form parses");
    let listed = factory_config::parse(
        "version: 1\n\
         instance:\n  id: 8a9ea91e-32af-46c3-9809-26d770b1e922\n  name: same\n\
         scopes:\n  - id: 8a9ea91e-32af-46c3-9809-26d770b1e923\n    name: same\n    path: p\n    agents:\n      - name: Solo\n        harness: pi\n        max_sessions: 3\n",
        "listed.yaml",
    )
    .expect("list form parses");

    assert_eq!(shorthand.scopes[0].agents, listed.scopes[0].agents);
}

#[test]
fn unknown_toplevel_runtime_block_is_ignored_not_rejected() {
    let path = fixture("unknown-toplevel.yaml");
    let config = factory_config::load(&path).expect(
        "a `runtime:` block must be ignored per ADR 0009 rule 2 -- \
         if this fails, unknown top-level keys have become unloadable",
    );

    assert_eq!(config.version, 1);
    assert_eq!(config.scopes.len(), 1);
    assert_eq!(
        config.scopes[0].id,
        uuid("57a4ca60-28a4-486b-a073-99de5782e32e")
    );
    assert_eq!(config.scopes[0].name, "assistant-fixture");
    assert_eq!(config.scopes[0].path, PathBuf::from("assistant"));
    // Explicit, on top of the full-value equality below: this fixture's
    // top-level `runtime:` block carries its own nested, indented `agents:`
    // list (matching the live `assistant` scope's corrected shape, including
    // reusing the same two agent names). That must contribute zero agents to
    // this scope -- a scan or parse that got greedy and picked it up would
    // silently change this count rather than fail loudly where the mistake
    // was made. Asserting exactly 2 here (not "at least 2") is what makes a
    // leak of the nested block's own entries visible instead of masked by a
    // name collision.
    assert_eq!(config.scopes[0].agents.len(), 2);
    assert_eq!(
        config.scopes[0].agents,
        vec![
            Agent {
                name: "assistant".to_string(),
                harness: Harness::Pi,
                max_sessions: 1,
                lifetime: Lifetime::Permanent,
                model: None,
            },
            Agent {
                name: "assistant-chat".to_string(),
                harness: Harness::Pi,
                max_sessions: 1,
                lifetime: Lifetime::Permanent,
                model: None,
            }
        ]
    );
    assert_eq!(config.origin, path.clone());
    // This fixture's nine-line comment header pushes the scope entry's `id`
    // well past line 8 (where it sits in the simpler fixtures), so this is
    // real coverage that the span comes from the file rather than a
    // constant.
    assert_eq!(
        config.scopes[0].id_location,
        Location {
            file: path,
            line: 17,
            column: 9,
        }
    );
}

#[test]
fn omitted_max_sessions_and_lifetime_take_their_defaults() {
    let path = fixture("defaults.yaml");
    let config = factory_config::load(&path).expect("fixture is valid");

    assert_eq!(
        config.scopes[0].agents,
        vec![Agent {
            name: "Defaulted".to_string(),
            harness: Harness::Pi,
            max_sessions: 1,
            lifetime: Lifetime::Permanent,
            model: None,
        }]
    );
    assert_eq!(
        config.scopes[0].id_location,
        Location {
            file: path,
            line: 8,
            column: 9,
        }
    );
}

/// ADR 0015: `git` marks a scope as its own repository; its absence means
/// the project lives in the instance's own repository. Both forms are valid
/// in the same file.
#[test]
fn git_present_and_absent_are_both_valid() {
    let path = fixture("git-optional.yaml");
    let config = factory_config::load(&path).expect("fixture is valid");

    assert_eq!(config.scopes.len(), 2);
    assert_eq!(
        config.scopes[0].git,
        Some("https://github.com/example/has-git.git".to_string())
    );
    assert_eq!(config.scopes[1].git, None);
}

/// A realistic instance registering several scopes at once, modelled on the
/// shape of the seven live Factory scopes (fresh UUIDs throughout).
#[test]
fn realistic_multi_scope_instance_loads_every_scope() {
    let path = fixture("multi-scope-instance.yaml");
    let config = factory_config::load(&path).expect("fixture is valid");

    assert_eq!(config.instance.name, "business-factory");
    let names: Vec<&str> = config.scopes.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "assistant",
            "irrlicht",
            "awesome-herdr",
            "model-lab",
            "ingo",
            "factory",
            "sandbox",
        ]
    );

    // Mixed `agent:`/`agents:`, mixed `git` presence, and every accepted
    // harness are all exercised in one file.
    assert_eq!(config.scopes[5].agents.len(), 2, "factory uses `agents:`");
    assert_eq!(
        config.scopes[1].git,
        Some("https://github.com/example/irrlicht.git".to_string())
    );
    assert_eq!(config.scopes[3].git, None, "model-lab has no `git`");
    assert_eq!(config.scopes[3].agents[0].harness, Harness::Opencode);
    assert_eq!(config.scopes[2].agents[0].harness, Harness::ClaudeCode);
    assert_eq!(config.scopes[6].agents[0].lifetime, Lifetime::Temporary);

    // Every scope ID and path is unique -- this fixture must load cleanly
    // through the same duplicate checks the invalid fixtures trigger.
    let mut ids: Vec<Uuid> = config.scopes.iter().map(|s| s.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), config.scopes.len());
}

/// `InstanceConfig::scopes` documents that it may be empty: a freshly
/// initialized instance has no scopes yet, and nothing about the shape
/// requires at least one. `scopes:` itself stays a required key -- an empty
/// *list* is valid, but the key must still be present, so a typo'd key still
/// fails loudly rather than silently producing an empty instance.
#[test]
fn an_instance_may_register_no_scopes_yet() {
    let config = factory_config::parse(
        "version: 1\n\
         instance:\n  id: 11111111-1111-4111-8111-111111111111\n  name: fresh\n\
         scopes: []\n",
        "fresh-instance.yaml",
    )
    .expect("an empty `scopes:` list is valid");

    assert_eq!(config.instance.name, "fresh");
    assert!(config.scopes.is_empty());
}
