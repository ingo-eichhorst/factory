//! The set of harnesses a scope may name, and the near-misses worth a hint.
//!
//! # Why this is a table and not a match arm
//!
//! Design §2.2 lists the initial harness values as exactly `pi` and
//! `claude-code`. Surveying the seven live `.factory/config.yaml` files on
//! 2026-09-08 found two that a literal reading rejects:
//!
//! | Scope | File | `harness:` |
//! |---|---|---|
//! | `awesome-herdr` | `projects/awesome-herdr/.factory/config.yaml` | `claude` |
//! | `model-lab` | `projects/model-lab/.factory/config.yaml` | `opencode` |
//!
//! ADR 0009 surveyed those same files and recorded the `runtime:` block, but not
//! this. A validation library that rejects two of seven production configs is
//! not useful, so version 1 resolves it as follows:
//!
//! - `opencode` is **accepted**. Design §2.2 says "*initial* harness values",
//!   which anticipates growth, and `opencode` is a real harness in real use.
//! - `claude` is **rejected with a suggestion**. Nothing by that name exists;
//!   `awesome-herdr` runs Claude Code, so this is a typo and the error says so.
//!
//! Accepting a harness here is not a claim that an adapter exists for it. Slice
//! 1 has no adapters at all; this table answers "is this a name Factory knows",
//! and Slice 5 and Slice 10 answer "can Factory drive it".
//!
//! **This is a decision taken without the user present and is meant to be easy
//! to reverse.** Everything above lives in this one file: narrowing the set back
//! to design §2.2 means deleting one row and one suggestion. It is recorded as
//! an open item in `docs/implementation-backlog.md` under Slice 1.

use std::fmt;

/// A harness a scope may name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Harness {
    ClaudeCode,
    Opencode,
    Pi,
}

/// Every accepted spelling, in the order errors list them.
///
/// Kept sorted so the "supported harnesses are …" help text is stable; a test
/// asserts the ordering rather than trusting it to survive an edit.
const HARNESS_TABLE: &[(&str, Harness)] = &[
    ("claude-code", Harness::ClaudeCode),
    ("opencode", Harness::Opencode),
    ("pi", Harness::Pi),
];

/// Names that are wrong but recognisably aimed at something real.
///
/// A suggestion is only worth making when it is almost certainly right. `claude`
/// is here because a live config uses it for a scope that demonstrably runs
/// Claude Code; this table is not a general-purpose fuzzy matcher.
const SUGGESTIONS: &[(&str, &str)] = &[("claude", "claude-code")];

impl Harness {
    /// Parse a `harness:` value, returning `None` if it is not a known name.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        HARNESS_TABLE
            .iter()
            .find(|(name, _)| *name == value)
            .map(|(_, harness)| *harness)
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        // An exhaustive match, not a table lookup. Adding a variant without a
        // spelling then fails to compile, where a lookup would panic the first
        // time anything formatted it — including inside an error message, which
        // is the worst possible place to discover it. `every_variant_round_trips`
        // cannot catch that case either, because it iterates the table and so
        // never visits a variant the table omits.
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Opencode => "opencode",
            Self::Pi => "pi",
        }
    }

    /// The accepted names, for error messages.
    #[must_use]
    pub fn supported_names() -> Vec<&'static str> {
        HARNESS_TABLE.iter().map(|(name, _)| *name).collect()
    }

    /// A correction for a known near-miss, if there is one.
    #[must_use]
    pub fn suggestion_for(value: &str) -> Option<&'static str> {
        SUGGESTIONS
            .iter()
            .find(|(wrong, _)| *wrong == value)
            .map(|(_, right)| *right)
    }
}

impl fmt::Display for Harness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_round_trips() {
        for (name, harness) in HARNESS_TABLE {
            assert_eq!(Harness::parse(name), Some(*harness));
            assert_eq!(harness.as_str(), *name);
        }
    }

    #[test]
    fn supported_names_are_sorted_so_help_text_is_stable() {
        let names = Harness::supported_names();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted, "HARNESS_TABLE must stay sorted by name");
    }

    #[test]
    fn the_two_live_configs_that_forced_this_table() {
        // `model-lab` sets `opencode`; accepted.
        assert_eq!(Harness::parse("opencode"), Some(Harness::Opencode));
        // `awesome-herdr` sets `claude`; rejected, but with a correction.
        assert_eq!(Harness::parse("claude"), None);
        assert_eq!(Harness::suggestion_for("claude"), Some("claude-code"));
    }

    #[test]
    fn an_unknown_name_gets_no_invented_suggestion() {
        assert_eq!(Harness::parse("bash"), None);
        assert_eq!(Harness::suggestion_for("bash"), None);
    }
}
