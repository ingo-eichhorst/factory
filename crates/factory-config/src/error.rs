//! Translating `serde_saphyr::Error` into `ConfigError`.
//!
//! `serde-saphyr` renders its own source as `<input>`, never a real path (see
//! the crate root docs), so every error it returns is wrapped here rather than
//! forwarded — this is the one place that happens.
//!
//! Only the variants Slice 1's corpus specifies get a bespoke message
//! (`SerdeMissingField`, `SerdeUnknownField`). Everything else falls back to a
//! generic wrapper: still located at the real file, but not corpus-specified
//! wording, since no fixture exercises it.

use std::path::Path;

use crate::raw::mapping_name_for_fields;
use crate::{ConfigError, Location};

/// Turn a raw parse failure into a `ConfigError` naming `origin`.
pub(crate) fn wrap_parse_error(err: &serde_saphyr::Error, origin: &Path) -> ConfigError {
    match unwrap_snippet(err) {
        serde_saphyr::Error::SerdeMissingField { field, location } => ConfigError {
            summary: format!("missing field `{field}`"),
            location: to_location(origin, *location),
            notes: Vec::new(),
            help: missing_field_help(field),
        },
        serde_saphyr::Error::SerdeUnknownField {
            field,
            expected,
            location,
        } => {
            let context = mapping_name_for_fields(expected);
            let mut sorted_expected = expected.clone();
            sorted_expected.sort_unstable();
            ConfigError {
                summary: format!("unknown field `{field}` in `{context}`"),
                location: to_location(origin, *location),
                notes: Vec::new(),
                help: unknown_field_help(field, &sorted_expected),
            }
        }
        other => {
            // No corpus case specifies this path; still name the real file
            // and give the caller *something* actionable rather than a bare
            // "<input>"-rendered message.
            let location = other
                .location()
                .map(|loc| to_location(origin, loc))
                .unwrap_or(Location {
                    file: origin.to_path_buf(),
                    line: 1,
                    column: 1,
                });
            let summary = other
                .to_string()
                .lines()
                .next()
                .unwrap_or("invalid configuration")
                .to_string();
            ConfigError {
                summary,
                location,
                notes: Vec::new(),
                help: "see docs/slice-1-error-corpus.md for the supported configuration shape"
                    .to_string(),
            }
        }
    }
}

/// `serde-saphyr` renders snippets by wrapping the real, structured error in
/// `Error::WithSnippet { error: Box<Error>, .. }` before returning it from
/// `from_str`. This is not documented and was found by testing: matching
/// directly on `Error::SerdeMissingField` / `Error::SerdeUnknownField`
/// against what `from_str` actually returns never matches, because the
/// top-level value is always `WithSnippet`. Unwrap it (recursively, in case
/// of nested wrapping) to reach the variant carrying the actual field name.
fn unwrap_snippet(err: &serde_saphyr::Error) -> &serde_saphyr::Error {
    match err {
        serde_saphyr::Error::WithSnippet { error, .. } => unwrap_snippet(error),
        other => other,
    }
}

pub(crate) fn to_location(origin: &Path, loc: serde_saphyr::Location) -> Location {
    Location {
        file: origin.to_path_buf(),
        line: loc.line() as usize,
        column: loc.column() as usize,
    }
}

fn missing_field_help(field: &str) -> String {
    match field {
        "instance" => "add an `instance:` block with an `id` and a `name`".to_string(),
        "scopes" => "add a `scopes:` list with at least one scope entry".to_string(),
        "version" => "add a `version:` key; this build of Factory supports version 1".to_string(),
        "path" => {
            "add a `path:` key naming where the scope's files live, relative to the instance root"
                .to_string()
        }
        _ => format!("add a `{field}:` key"),
    }
}

fn unknown_field_help(field: &str, sorted_expected: &[&'static str]) -> String {
    let list = sorted_expected
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut help = format!("expected one of {list}");
    if let Some(suggestion) = closest_match(field, sorted_expected) {
        help.push_str(&format!("; `{field}` looks like a typo for `{suggestion}`"));
    }
    help
}

/// A tight edit-distance check against the field names of the mapping being
/// parsed — not a general-purpose fuzzy matcher. Keeping the threshold at 2
/// (and requiring the candidate not be tiny) means a typo close to a real
/// field name gets a hint, and everything else gets none rather than a
/// nonsense suggestion.
fn closest_match(field: &str, candidates: &[&'static str]) -> Option<&'static str> {
    candidates
        .iter()
        .map(|candidate| (*candidate, levenshtein(field, candidate)))
        .filter(|(candidate, distance)| *distance > 0 && *distance <= 2 && candidate.len() >= 3)
        .min_by_key(|(_, distance)| *distance)
        .map(|(candidate, _)| candidate)
}

/// Classic Wagner–Fischer edit distance. `factory-config` has no dependency on
/// a string-distance crate, and the inputs here are short field names, so a
/// plain O(n*m) table is simplest.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();

    for i in 1..=a.len() {
        let mut prev_diag = row[0];
        row[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let deletion = row[j] + 1;
            let insertion = row[j - 1] + 1;
            let substitution = prev_diag + cost;
            prev_diag = row[j];
            row[j] = deletion.min(insertion).min(substitution);
        }
    }

    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typo_within_distance_two_is_suggested() {
        assert_eq!(
            closest_match(
                "max_sesions",
                &["harness", "lifetime", "max_sessions", "model", "name"]
            ),
            Some("max_sessions")
        );
    }

    #[test]
    fn unrelated_name_gets_no_invented_suggestion() {
        assert_eq!(
            closest_match(
                "kind",
                &["harness", "lifetime", "max_sessions", "model", "name"]
            ),
            None
        );
    }

    #[test]
    fn exact_match_is_not_a_typo() {
        assert_eq!(
            closest_match(
                "name",
                &["harness", "lifetime", "max_sessions", "model", "name"]
            ),
            None
        );
    }
}
