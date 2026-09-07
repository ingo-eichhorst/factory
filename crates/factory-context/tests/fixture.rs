//! Compiles the real Slice 1/3 registration fixture
//! (`fixtures/registration/`) as a two-scope chain: the company root and its
//! one registered child. Read-only input -- this test must never write into
//! it.
//!
//! `cargo test` sets the working directory to this package's root, not the
//! workspace root, so the fixture path is anchored on `CARGO_MANIFEST_DIR`
//! rather than assumed relative to the process's cwd.

use factory_context::{ScopeContext, compile};
use std::path::PathBuf;

fn fixture_root() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/registration"
    ))
}

#[test]
fn the_registration_fixture_compiles_as_a_two_scope_chain() {
    let root = fixture_root();
    let scopes = vec![
        ScopeContext {
            scope_name: "fixture-co".to_string(),
            context_file: root.join("AGENTS.md"),
        },
        ScopeContext {
            scope_name: "example-project".to_string(),
            context_file: root.join("projects/example-project/AGENTS.md"),
        },
    ];

    let agent = factory_config::Agent {
        name: "example-project".to_string(),
        harness: factory_config::Harness::Pi,
        max_sessions: 1,
        lifetime: factory_config::Lifetime::Permanent,
    };

    let compiled = compile(&scopes, &agent, Some("Review the open pull request."))
        .expect("the fixture's AGENTS.md files are readable");

    // Both scope headings are present...
    assert!(compiled.text.contains("Context: fixture-co"));
    assert!(compiled.text.contains("Context: example-project"));

    // ...and each scope's own distinctive text made it in, verbatim.
    assert!(
        compiled
            .text
            .contains("This is a test fixture, not a real company.")
    );
    assert!(
        compiled.text.contains(
            "This is the project-level scope for Example Project, a child of the Fixture"
        )
    );

    let root_pos = compiled
        .text
        .find("This is a test fixture, not a real company.")
        .expect("company root text present");
    let leaf_pos = compiled
        .text
        .find("This is the project-level scope for Example Project")
        .expect("child scope text present");
    assert!(
        root_pos < leaf_pos,
        "the company root's content must precede the child scope's"
    );
}
