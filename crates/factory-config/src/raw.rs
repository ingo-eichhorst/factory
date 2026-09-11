//! Serde-facing shapes for the Factory instance's `.factory/config.yaml`.
//!
//! These mirror the file structurally but carry none of the validation rules
//! themselves — `crate::validate` turns a [`RawDocument`] into a
//! [`crate::InstanceConfig`] or a [`crate::ConfigError`].
//!
//! Two modelling choices are load-bearing, both fixed by ADR 0009 and carried
//! forward unchanged by ADR 0015:
//!
//! - [`RawDocument`] does **not** derive `deny_unknown_fields`: rule 2 requires
//!   unknown top-level keys (the `runtime:` block owned by
//!   `ensure_assistant_agents.py`) to pass through silently. [`RawScopeEntry`]
//!   *does* derive it: a scope entry is a Factory-owned mapping (it lives
//!   inside the Factory-owned `scopes:` list), so an ad-hoc extra key there is
//!   rejected the same way an unknown field inside `agent:` is.
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
    pub instance: RawInstance,
    /// May be empty — a freshly initialized instance has no scopes yet — but
    /// the key itself is required, so a typo'd `scope:` or `scopess:` fails
    /// loudly instead of silently producing an empty instance.
    pub scopes: Vec<Spanned<RawScopeEntry>>,
}

/// ADR 0015. Factory-owned, so unknown fields here are rejected (ADR 0009
/// rule 3) — a typo in `instance:` should fail as loudly as one in `agent:`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawInstance {
    pub id: Spanned<String>,
    pub name: String,
}

/// One entry of the instance's `scopes:` list, per ADR 0015. Factory-owned,
/// so — like [`RawInstance`] and [`RawAgent`] — unknown fields are rejected.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawScopeEntry {
    pub id: Spanned<String>,
    pub name: String,
    pub path: Spanned<String>,
    /// Present when the project is its own repository; absent when it lives
    /// in the instance's own repository. ADR 0015 validates only
    /// presence-or-absence, so a plain `String` is enough — no `Spanned`
    /// location is ever needed for it.
    pub git: Option<String>,
    pub agent: Option<Spanned<RawAgent>>,
    pub agents: Option<Vec<Spanned<RawAgent>>>,
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
    /// The model this agent's harness should be started with. Optional and
    /// opaque: Factory never resolves it, never checks it against a
    /// catalogue, and has no default of its own — see ADR 0024.
    pub model: Option<Spanned<String>>,
}

/// The field names of a Factory-owned mapping, for two purposes:
///
/// 1. Turning `serde_saphyr::Error::SerdeUnknownField`'s `expected` list back
///    into an `instance` vs `scope` vs `agent` context name for the "unknown
///    field `x` in `y`" message (case 10) — `serde-saphyr` reports the flat
///    field list but not which struct it came from.
/// 2. Rendering that same list, alphabetised, in the `help:` line.
pub(crate) fn mapping_name_for_fields(expected: &[&'static str]) -> &'static str {
    let mut sorted = expected.to_vec();
    sorted.sort_unstable();
    match sorted.as_slice() {
        ["id", "name"] => "instance",
        ["agent", "agents", "git", "id", "name", "path"] => "scope",
        ["harness", "lifetime", "max_sessions", "model", "name"] => "agent",
        _ => "configuration",
    }
}
