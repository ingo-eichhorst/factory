//! Serde-facing shapes for `.factory/config.yaml`.
//!
//! These mirror the file structurally but carry none of the validation rules
//! themselves — `crate::validate` turns a [`RawDocument`] into a
//! [`crate::ScopeConfig`] or a [`crate::ConfigError`].
//!
//! Two modelling choices are load-bearing, both fixed by ADR 0009:
//!
//! - [`RawDocument`] does **not** derive `deny_unknown_fields`: rule 2 requires
//!   unknown top-level keys (the `runtime:` block owned by
//!   `ensure_assistant_agents.py`) to pass through silently.
//! - `agent` and `agents` are two independent optional fields, never an
//!   untagged enum. `serde-saphyr` cannot carry span information through an
//!   untagged enum variant, and an untagged enum could not report the
//!   both-set/neither-set diagnostics Slice 1 requires. `crate::validate`
//!   decides between them after parsing.
//!
//! Every field that a diagnostic must be able to point at is wrapped in
//! [`Spanned`] so its source location survives into validation.

use serde::Deserialize;
use serde_saphyr::Spanned;

#[derive(Debug, Deserialize)]
pub(crate) struct RawDocument {
    pub version: Spanned<u32>,
    pub scope: RawScope,
    pub agent: Option<Spanned<RawAgent>>,
    pub agents: Option<Vec<Spanned<RawAgent>>>,
}

/// Design §2.1. Factory-owned, so unknown fields here are rejected (ADR 0009
/// rule 3) — a typo in `scope:` should fail as loudly as one in `agent:`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawScope {
    pub id: Spanned<String>,
    pub name: String,
}

/// Design §2.2. Shared by both the `agent:` shorthand and each element of an
/// `agents:` list — the two spellings produce the same shape, so they share
/// one raw struct as well as one validated one.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawAgent {
    pub name: String,
    pub harness: Spanned<String>,
    pub max_sessions: Option<Spanned<u32>>,
    pub lifetime: Option<Spanned<String>>,
}

/// The field names of a Factory-owned mapping, for two purposes:
///
/// 1. Turning `serde_saphyr::Error::SerdeUnknownField`'s `expected` list back
///    into a `scope` vs `agent` context name for the "unknown field `x` in
///    `y`" message (case 10) — `serde-saphyr` reports the flat field list but
///    not which struct it came from.
/// 2. Rendering that same list, alphabetised, in the `help:` line.
pub(crate) fn mapping_name_for_fields(expected: &[&'static str]) -> &'static str {
    let mut sorted = expected.to_vec();
    sorted.sort_unstable();
    match sorted.as_slice() {
        ["id", "name"] => "scope",
        ["harness", "lifetime", "max_sessions", "name"] => "agent",
        _ => "configuration",
    }
}
