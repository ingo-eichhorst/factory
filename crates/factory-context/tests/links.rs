//! ADR 0013 decision 4: version 1 follows no `[[links]]` when compiling.
//! Context is exactly the `AGENTS.md` chain, the agent definition, and the
//! task prompt -- nothing a link points at.

mod support;

use factory_context::{ScopeContext, compile};

#[test]
fn double_bracket_links_survive_as_literal_text_and_are_never_followed() {
    let temp = tempfile::tempdir().expect("can create a temp dir");
    let agents_path = temp.path().join("AGENTS.md");
    let note_path = temp.path().join("some-note.md");

    support::write_file(&agents_path, "See [[some-note]] for background.\n");
    // A note that genuinely exists right next to `AGENTS.md`. If `compile`
    // ever started resolving `[[links]]`, this distinctive text would leak
    // into the compiled output; today it must never be opened at all.
    support::write_file(&note_path, "NOTE-BODY-THAT-MUST-NEVER-APPEAR");

    let agent = support::sample_agent();
    let compiled = compile(
        &[ScopeContext {
            scope_name: "company".to_string(),
            context_file: agents_path,
        }],
        &agent,
        None,
    )
    .expect("compiles");

    assert!(
        compiled.text.contains("[[some-note]]"),
        "the literal link text must survive verbatim in the compiled output"
    );
    assert!(
        !compiled.text.contains("NOTE-BODY-THAT-MUST-NEVER-APPEAR"),
        "compile must never open the file a link names"
    );
}
