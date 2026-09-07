//! Byte-stability requires that a human's editor leaving or omitting a
//! trailing newline never changes the compiled bytes (see `compile`'s doc
//! comment and `src/format.rs`'s `normalize_trailing_newline`). This checks
//! that decision end-to-end, through an actual file read, rather than only at
//! the unit level.

mod support;

use factory_context::{ScopeContext, compile};

#[test]
fn trailing_newline_presence_does_not_affect_compiled_bytes() {
    let temp = tempfile::tempdir().expect("can create a temp dir");
    let path = temp.path().join("AGENTS.md");
    let agent = support::sample_agent();
    let scopes = |context_file: std::path::PathBuf| {
        vec![ScopeContext {
            scope_name: "company".to_string(),
            context_file,
        }]
    };

    support::write_file(&path, "same body content");
    let without_newline =
        compile(&scopes(path.clone()), &agent, None).expect("compiles without a trailing newline");

    support::write_file(&path, "same body content\n");
    let with_newline =
        compile(&scopes(path.clone()), &agent, None).expect("compiles with a trailing newline");

    assert_eq!(
        without_newline.text, with_newline.text,
        "a trailing newline in the source file must not change the compiled bytes"
    );
}

#[test]
fn crlf_line_endings_are_normalised_to_lf_when_read_from_disk() {
    let temp = tempfile::tempdir().expect("can create a temp dir");
    let path = temp.path().join("AGENTS.md");
    // Written directly, not through `support::write_file`, because the point
    // here is the literal bytes on disk, CRLF included.
    std::fs::write(&path, "line one\r\nline two\r\n").expect("can write the CRLF fixture");
    let agent = support::sample_agent();

    let compiled = compile(
        &[ScopeContext {
            scope_name: "company".to_string(),
            context_file: path,
        }],
        &agent,
        None,
    )
    .expect("compiles");

    assert!(compiled.text.contains("line one\nline two\n"));
    assert!(
        !compiled.text.contains('\r'),
        "no carriage return may survive into the compiled text"
    );
}

#[test]
fn an_entirely_empty_context_file_compiles_deterministically() {
    let temp = tempfile::tempdir().expect("can create a temp dir");
    let path = temp.path().join("AGENTS.md");
    support::write_file(&path, "");
    let agent = support::sample_agent();

    let compiled = compile(
        &[ScopeContext {
            scope_name: "company".to_string(),
            context_file: path,
        }],
        &agent,
        None,
    )
    .expect("an empty file is still a readable, if content-free, source");

    // The scope's section header is present, and an empty file normalises to
    // a single `\n` body (see `normalize_trailing_newline`'s doc comment):
    // the header's own blank line, then that one-newline body, then the next
    // section's rule line -- three newlines in a row at the seam, not two
    // and not four.
    assert!(compiled.text.contains("Context: company\n"));
    let rule = "=".repeat(80);
    let expected_seam = format!("\n\n\n{rule}\nAgent definition\n");
    assert!(
        compiled.text.contains(&expected_seam),
        "expected the empty-body seam {expected_seam:?} in {:?}",
        compiled.text
    );
}
