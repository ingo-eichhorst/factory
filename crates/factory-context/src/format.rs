//! Section rendering: the exact bytes each part of `compile`'s output
//! contributes.
//!
//! Kept out of `lib.rs` because the formatting rules (the rule line, the
//! blank line after each header, newline normalisation) are pure text
//! transforms with no dependency on `factory_config` or the filesystem, and
//! are the part of this crate most worth unit-testing in isolation from the
//! `Result`/`ContextError` plumbing.

/// The section separator. Chosen because it cannot be confused with Markdown
/// a source file might contain, since every source *is* Markdown.
pub(crate) const RULE: &str =
    "================================================================================";

/// Normalise a piece of text so it ends in exactly one `\n`, regardless of
/// what its source did.
///
/// Two decisions this makes, both required for byte-stability (design §2.5):
///
/// - **CRLF is collapsed to LF.** A file's line-ending style is an artifact of
///   whatever editor or platform last saved it, not part of its content, and
///   the compiled context must not change bytes depending on which one
///   touched the file last. A lone `\r` not preceded by `\n` is left alone:
///   it is not a line ending Factory needs to normalise, and inventing a rule
///   for it would be guessing.
/// - **An empty input normalises to a single `\n`.** "Ends in exactly one
///   newline" is well-defined even for zero bytes of content: the section's
///   blank line still separates the header from an (empty) body, rather than
///   the next section's rule line running into this one.
///
/// A consequence, not a separate rule: a file already ending in several
/// blank lines (`"text\n\n\n"`) is also collapsed to one trailing `\n`, since
/// "ends in exactly one newline" describes the output, not merely "add one if
/// missing".
pub(crate) fn normalize_trailing_newline(content: &str) -> String {
    let unified = content.replace("\r\n", "\n");
    let trimmed = unified.trim_end_matches('\n');
    format!("{trimmed}\n")
}

/// Render one scope's `AGENTS.md` section.
///
/// `content` must already be read from `path`; this function does no I/O, so
/// it can be unit-tested without a filesystem.
pub(crate) fn scope_section(scope_name: &str, path: &std::path::Path, content: &str) -> String {
    let body = normalize_trailing_newline(content);
    format!(
        "{RULE}\nContext: {scope_name}\nSource: {}\n{RULE}\n\n{body}",
        path.display()
    )
}

/// Render the agent-definition section.
///
/// This text is synthesised, not read from a file, so it is built to already
/// satisfy the trailing-newline rule rather than being passed through
/// [`normalize_trailing_newline`].
pub(crate) fn agent_section(agent: &factory_config::Agent) -> String {
    let lifetime = match agent.lifetime {
        factory_config::Lifetime::Permanent => "permanent",
        factory_config::Lifetime::Temporary => "temporary",
    };
    format!(
        "{RULE}\nAgent definition\n{RULE}\n\nname: {}\nharness: {}\nlifetime: {lifetime}\nmax_sessions: {}\n",
        agent.name, agent.harness, agent.max_sessions
    )
}

/// Render the task-prompt section.
///
/// The prompt goes through the same [`normalize_trailing_newline`] as file
/// content, for the same reason: two logically identical prompts that differ
/// only in a trailing newline must compile to identical bytes.
pub(crate) fn task_section(prompt: &str) -> String {
    let body = normalize_trailing_newline(prompt);
    format!("{RULE}\nTask\n{RULE}\n\n{body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_is_exactly_eighty_characters() {
        assert_eq!(RULE.len(), 80);
        assert!(RULE.chars().all(|c| c == '='));
    }

    #[test]
    fn missing_trailing_newline_is_added() {
        assert_eq!(normalize_trailing_newline("hello"), "hello\n");
    }

    #[test]
    fn existing_single_trailing_newline_is_preserved() {
        assert_eq!(normalize_trailing_newline("hello\n"), "hello\n");
    }

    #[test]
    fn multiple_trailing_newlines_collapse_to_one() {
        assert_eq!(normalize_trailing_newline("hello\n\n\n"), "hello\n");
    }

    #[test]
    fn empty_content_normalises_to_a_single_newline() {
        assert_eq!(normalize_trailing_newline(""), "\n");
    }

    #[test]
    fn crlf_is_collapsed_to_lf() {
        assert_eq!(normalize_trailing_newline("one\r\ntwo\r\n"), "one\ntwo\n");
    }

    #[test]
    fn lone_cr_is_left_alone() {
        assert_eq!(normalize_trailing_newline("a\rb\n"), "a\rb\n");
    }

    #[test]
    fn scope_section_matches_the_documented_format() {
        let path = std::path::Path::new("/repo/AGENTS.md");
        let rendered = scope_section("company", path, "hello");
        assert_eq!(
            rendered,
            format!("{RULE}\nContext: company\nSource: /repo/AGENTS.md\n{RULE}\n\nhello\n")
        );
    }

    #[test]
    fn agent_section_matches_the_documented_format() {
        let agent = factory_config::Agent {
            name: "example-project".to_string(),
            harness: factory_config::Harness::Pi,
            max_sessions: 3,
            lifetime: factory_config::Lifetime::Temporary,
            model: None,
        };
        let rendered = agent_section(&agent);
        assert_eq!(
            rendered,
            format!(
                "{RULE}\nAgent definition\n{RULE}\n\nname: example-project\nharness: pi\nlifetime: temporary\nmax_sessions: 3\n"
            )
        );
    }

    #[test]
    fn task_section_matches_the_documented_format() {
        let rendered = task_section("do the thing");
        assert_eq!(rendered, format!("{RULE}\nTask\n{RULE}\n\ndo the thing\n"));
    }
}
