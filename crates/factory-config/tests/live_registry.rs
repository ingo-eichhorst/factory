//! Loads the real company-root registry, not a fixture copy.
//!
//! Every other test in this crate is deliberately hermetic: `no_writes.rs`
//! copies fixtures into a temp dir precisely so nothing here ever touches a
//! file outside the crate. This file is the one exception, and the exception
//! is load-bearing, not a shortcut.
//!
//! A fixture copy of the live `assistant` scope cannot do the one thing this
//! test exists for: fail when the *real* file drifts back to the bug it
//! checks for. `fixtures/valid/unknown-toplevel.yaml` already asserts the
//! parsed *shape* of an assistant-like entry, but nothing ties that fixture to
//! `<company-root>/.factory/config.yaml` staying correct -- someone could edit
//! the live file back to the single-agent `agent:` shorthand and every
//! fixture-based test would keep passing. `docs/implementation-backlog.md`'s
//! "Measured, not assumed" precedent already ran the loader against the seven
//! live files by hand during Slice 1; this is that same measurement, kept
//! running instead of a one-off.
//!
//! The path is `CARGO_MANIFEST_DIR/../../../../.factory/config.yaml`: this
//! crate lives at `<company-root>/projects/factory/crates/factory-config`,
//! four directories below the company root. `projects/factory` is also
//! published as its own subtree remote (see the `factory` scope's `git:`
//! comment in the registry itself), where that path does not exist -- so a
//! missing file skips this test rather than failing it. A checkout that *can*
//! see the company root and still fails here has a real regression.
use std::path::PathBuf;

use factory_config::{Agent, Harness, Lifetime};

fn live_registry_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../.factory/config.yaml")
}

#[test]
fn live_registry_loads_and_the_assistant_scope_names_both_real_agents() {
    let path = live_registry_path();
    if !path.exists() {
        eprintln!(
            "skipping: {} not present (this checkout cannot see the company root)",
            path.display()
        );
        return;
    }

    let config = factory_config::load(&path).expect("the live registry must load cleanly");

    let assistant = config
        .scopes
        .iter()
        .find(|scope| scope.name == "assistant")
        .expect("the live registry registers an `assistant` scope");

    // The defect this test guards: modelled as one agent with
    // `max_sessions: 2`, the two runtimes that
    // `assistant/.factory/config.yaml`'s `runtime:` block actually starts --
    // `assistant` and `assistant-chat`, different agents, not two sessions of
    // one -- were indistinguishable to Factory. Reverting the registry entry
    // to the old shorthand reproduces exactly that: this assertion fails on
    // `agents.len()` (1, not 2) rather than on names or `max_sessions`.
    assert_eq!(
        assistant.agents,
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
            },
        ],
        "the assistant scope must declare exactly the two agents \
         assistant/.factory/config.yaml's runtime: block actually starts, \
         each capped at its own max_sessions: 1"
    );
}
