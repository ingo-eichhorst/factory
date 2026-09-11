//! Raw parse tree to validated [`InstanceConfig`], per
//! `docs/slice-1-error-corpus.md` as amended by ADR 0015.
//!
//! Order of checks is documented on the crate root (`lib.rs`), since no
//! fixture exercises it — every fixture in the corpus triggers exactly one
//! problem.

use std::path::Path;

use serde_saphyr::Spanned;
use uuid::Uuid;

use crate::error::to_location;
use crate::harness::Harness;
use crate::raw::{RawAgent, RawDocument, RawScopeEntry};
use crate::{
    Agent, ConfigError, Instance, InstanceConfig, Lifetime, Location, Note, SUPPORTED_VERSION,
    ScopeEntry,
};

pub(crate) fn validate(
    raw: RawDocument,
    origin: &Path,
    source: &str,
) -> Result<InstanceConfig, ConfigError> {
    if raw.version.value != SUPPORTED_VERSION {
        return Err(ConfigError {
            summary: format!("unsupported configuration version `{}`", raw.version.value),
            location: to_location(origin, raw.version.defined),
            notes: Vec::new(),
            help: format!("this build of Factory supports version {SUPPORTED_VERSION}"),
        });
    }

    let instance_id_location = to_location(origin, raw.instance.id.defined);
    let instance_id = Uuid::parse_str(&raw.instance.id.value).map_err(|_| ConfigError {
        summary: format!("`instance.id` is not a UUID: `{}`", raw.instance.id.value),
        location: instance_id_location.clone(),
        notes: Vec::new(),
        help: "generate one with `uuidgen`; the instance ID is permanent identity and must not be reused between instances".to_string(),
    })?;

    // Every entry's own line, gathered before the entries are consumed below —
    // this borrow ends immediately, so it does not fight the `into_iter` that
    // follows. Used to bound each entry's key-recovery scan (see
    // `entry_line_range`) to that entry's own lines, which is what makes the
    // scan in `resolve_agent_items` immune to a sibling entry's `agent:` key
    // sitting at the very same indentation.
    let entry_start_lines: Vec<usize> = raw
        .scopes
        .iter()
        .map(|e| e.defined.line() as usize)
        .collect();
    let total_lines = source.lines().count().max(1);

    let mut scopes = Vec::with_capacity(raw.scopes.len());
    for (index, entry) in raw.scopes.into_iter().enumerate() {
        let key_indent = (entry.defined.column() as usize).saturating_sub(1);
        let line_range = (
            entry_start_lines[index],
            entry_start_lines
                .get(index + 1)
                .map_or(total_lines, |next| *next - 1),
        );
        scopes.push(validate_scope_entry(
            entry.value,
            key_indent,
            line_range,
            origin,
            source,
        )?);
    }

    check_unique_scope_ids(&scopes)?;
    check_unique_scope_paths(&scopes)?;

    Ok(InstanceConfig {
        version: raw.version.value,
        instance: Instance {
            id: instance_id,
            name: raw.instance.name,
        },
        scopes,
        origin: origin.to_path_buf(),
    })
}

fn validate_scope_entry(
    raw: RawScopeEntry,
    key_indent: usize,
    line_range: (usize, usize),
    origin: &Path,
    source: &str,
) -> Result<ScopeEntry, ConfigError> {
    let id_location = to_location(origin, raw.id.defined);
    let id = Uuid::parse_str(&raw.id.value).map_err(|_| ConfigError {
        summary: format!("scope `{}`'s `id` is not a UUID: `{}`", raw.name, raw.id.value),
        location: id_location.clone(),
        notes: Vec::new(),
        help: "generate one with `uuidgen`; a scope ID is permanent identity and must not be reused between scopes".to_string(),
    })?;

    let path_location = to_location(origin, raw.path.defined);
    let path = std::path::PathBuf::from(raw.path.value);

    let agent_items = resolve_agent_items(
        raw.agent, raw.agents, &raw.name, key_indent, line_range, origin, source,
    )?;

    let mut agents = Vec::with_capacity(agent_items.len());
    for item in &agent_items {
        agents.push(validate_agent(item, &raw.name, origin)?);
    }

    check_unique_names(&agent_items, &agents, &raw.name, origin)?;

    Ok(ScopeEntry {
        id,
        name: raw.name,
        path,
        git: raw.git,
        agents,
        id_location,
        path_location,
    })
}

/// Case 1 (both set) and case 2 (neither set) live here: the one place that
/// decides between the `agent:` shorthand and the `agents:` list, per ADR
/// 0009's "two optional fields plus post-parse validation" model.
fn resolve_agent_items(
    agent: Option<Spanned<RawAgent>>,
    agents: Option<Vec<Spanned<RawAgent>>>,
    scope_name: &str,
    key_indent: usize,
    line_range: (usize, usize),
    origin: &Path,
    source: &str,
) -> Result<Vec<Spanned<RawAgent>>, ConfigError> {
    match (agent, agents) {
        (Some(agent), Some(agents)) => {
            let agent_location =
                entry_key_location(source, "agent", key_indent, line_range, origin)
                    .unwrap_or_else(|| to_location(origin, agent.defined));
            let agents_location = entry_key_location(
                source, "agents", key_indent, line_range, origin,
            )
            .or_else(|| {
                agents
                    .first()
                    .map(|first| to_location(origin, first.defined))
            });
            let mut notes = vec![Note {
                message: "`agent` shorthand defined here".to_string(),
                location: None,
            }];
            if let Some(location) = agents_location {
                notes.push(Note {
                    message: "`agents` list defined".to_string(),
                    location: Some(location),
                });
            }
            Err(ConfigError {
                summary: format!(
                    "scope `{scope_name}` sets both `agent` and `agents`; a scope uses one or the other"
                ),
                location: agent_location,
                notes,
                help: "keep `agents:` and delete the `agent:` block, or keep `agent:` and delete `agents:`".to_string(),
            })
        }
        (Some(agent), None) => Ok(vec![agent]),
        (None, Some(agents)) if !agents.is_empty() => Ok(agents),
        (None, _) => Err(no_agent_defined(scope_name, key_indent, line_range, origin)),
    }
}

/// Recover a scope-entry-owned key's own line, for a diagnostic that is about
/// the key itself rather than its value.
///
/// `Spanned<T>` locates the spanned *value* node: for a key whose value is a
/// block mapping or a block sequence, that is the first nested line — one
/// below the key — and `serde-saphyr` exposes no way to ask for the key's own
/// position. Per ADR 0015, `agent:`/`agents:` now live inside a scope entry
/// rather than at the document's top level, so they are indented rather than
/// at column zero — the "first character is not whitespace" rule from before
/// the ADR would find nothing and silently fall back to the value-node
/// location for every case. The entry-relative replacement: within `source`,
/// restricted to `line_range` (this entry's own lines, 1-based and
/// inclusive — computed by the caller from consecutive entries' own starting
/// lines, so a sibling entry's `agent:` key at the very same indentation
/// cannot be mistaken for this entry's), find a line whose indentation
/// equals `key_indent` (this entry's own key indentation, derived from where
/// `serde-saphyr` reports the entry's mapping node starting) and which
/// begins `"{key}:"`.
///
/// Returns `None` — not a guess — unless the scan finds *exactly one* match
/// in range. Zero matches means the key was never really there as plain
/// entry-relative text (should not happen for a key we know is `Some`, but
/// this must not panic if it does); more than one means something else at the
/// same indentation within this entry's own lines began with the same text.
/// Callers fall back to the value-node location instead: a diagnostic one
/// line off is a papercut, one that points at an unrelated line because of a
/// false match is a defect.
fn entry_key_location(
    source: &str,
    key: &str,
    key_indent: usize,
    line_range: (usize, usize),
    origin: &Path,
) -> Option<Location> {
    find_entry_key_line(source, key, key_indent, line_range).map(|line| Location {
        file: origin.to_path_buf(),
        line,
        column: key_indent + 1,
    })
}

/// The scan itself, kept separate from [`entry_key_location`] so it can be
/// unit-tested against raw strings without needing a `Path`.
fn find_entry_key_line(
    source: &str,
    key: &str,
    key_indent: usize,
    (start, end): (usize, usize),
) -> Option<usize> {
    let prefix = format!("{key}:");
    let mut matches = source.lines().enumerate().filter(|(index, line)| {
        let line_no = index + 1;
        if line_no < start || line_no > end {
            return false;
        }
        let leading = line.len() - line.trim_start().len();
        leading == key_indent && line[leading..].starts_with(prefix.as_str())
    });
    let (first_index, _) = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(first_index + 1)
}

/// Points at the entry's own first key (its `id`, per the ADR 0015 shape),
/// not column 1: `line_range.0` is the entry's own starting line, but
/// column 1 on a `  - id: …` line lands on the dash, not on content. The
/// corpus documents elsewhere that a bare `1:1` for a whole-document problem
/// is "a property of the fixture, not a rule" rather than a fixed
/// requirement, so there is no compatibility reason to keep pointing this
/// per-entry case at a blank column.
fn no_agent_defined(
    scope_name: &str,
    key_indent: usize,
    line_range: (usize, usize),
    origin: &Path,
) -> ConfigError {
    ConfigError {
        summary: format!(
            "scope `{scope_name}` has no agent defined; a scope configures at least one agent"
        ),
        location: Location {
            file: origin.to_path_buf(),
            line: line_range.0,
            column: key_indent + 1,
        },
        notes: Vec::new(),
        help: "add an `agent:` block, or an `agents:` list with at least one entry".to_string(),
    }
}

fn validate_agent(
    item: &Spanned<RawAgent>,
    scope_name: &str,
    origin: &Path,
) -> Result<Agent, ConfigError> {
    let raw = &item.value;

    let harness = Harness::parse(&raw.harness.value).ok_or_else(|| {
        let value = &raw.harness.value;
        let supported = Harness::supported_names()
            .iter()
            .map(|name| format!("`{name}`"))
            .collect::<Vec<_>>();
        let help = match Harness::suggestion_for(value) {
            Some(suggestion) => format!(
                "did you mean `{suggestion}`? supported harnesses are {}",
                join_with_and(&supported)
            ),
            None => format!("supported harnesses are {}", join_with_and(&supported)),
        };
        ConfigError {
            summary: format!("scope `{scope_name}` sets unsupported harness `{value}`"),
            location: to_location(origin, raw.harness.defined),
            notes: Vec::new(),
            help,
        }
    })?;

    let max_sessions = match &raw.max_sessions {
        Some(spanned) if spanned.value == 0 => {
            return Err(ConfigError {
                summary: format!(
                    "scope `{scope_name}` sets `max_sessions` to 0, so this agent could never start a session"
                ),
                location: to_location(origin, spanned.defined),
                notes: Vec::new(),
                help: "use at least 1, or remove the agent".to_string(),
            });
        }
        Some(spanned) => spanned.value,
        None => 1,
    };

    let lifetime = match &raw.lifetime {
        Some(spanned) => match spanned.value.as_str() {
            "permanent" => Lifetime::Permanent,
            "temporary" => Lifetime::Temporary,
            other => {
                return Err(ConfigError {
                    summary: format!("scope `{scope_name}` sets unsupported lifetime `{other}`"),
                    location: to_location(origin, spanned.defined),
                    notes: Vec::new(),
                    help: "supported lifetimes are `permanent` and `temporary`; `permanent` is the default and may be omitted".to_string(),
                });
            }
        },
        None => Lifetime::default(),
    };

    // The only thing checked about a model is that it is not blank. What the
    // name means is the harness's question — but an empty string would be
    // passed to it as an empty `--model`, which every harness rejects with a
    // worse message than this one. ADR 0024.
    let model = match &raw.model {
        Some(spanned) if spanned.value.trim().is_empty() => {
            return Err(ConfigError {
                summary: format!("scope `{scope_name}` sets an empty `model`"),
                location: to_location(origin, spanned.defined),
                notes: Vec::new(),
                help: "name a model the harness knows, or remove the key to leave the choice to \
                       the harness's own configuration"
                    .to_string(),
            });
        }
        Some(spanned) => Some(spanned.value.clone()),
        None => None,
    };

    Ok(Agent {
        name: raw.name.clone(),
        harness,
        max_sessions,
        lifetime,
        model,
    })
}

/// Case 3: two agents in the same scope sharing a name. Names are how
/// `factory task send` addresses an agent, so a collision is reported against
/// both places it was defined, in file order.
fn check_unique_names(
    items: &[Spanned<RawAgent>],
    agents: &[Agent],
    scope_name: &str,
    origin: &Path,
) -> Result<(), ConfigError> {
    for later in 1..agents.len() {
        for earlier in 0..later {
            if agents[later].name == agents[earlier].name {
                return Err(ConfigError {
                    summary: format!(
                        "two agents in scope `{scope_name}` are both named `{}`",
                        agents[later].name
                    ),
                    location: to_location(origin, items[later].defined),
                    notes: vec![Note {
                        message: "first defined".to_string(),
                        location: Some(to_location(origin, items[earlier].defined)),
                    }],
                    help: "agent names address a recipient in `factory task send`, so they must be unique within a scope; rename one".to_string(),
                });
            }
        }
    }
    Ok(())
}

/// Case 13, now an intra-file check per ADR 0015: two scope entries in the
/// same instance file may not share an `id`. Runs after every entry has
/// already been individually validated, so both `id`s are known-good UUIDs.
fn check_unique_scope_ids(scopes: &[ScopeEntry]) -> Result<(), ConfigError> {
    for later in 1..scopes.len() {
        for earlier in 0..later {
            if scopes[later].id == scopes[earlier].id {
                return Err(ConfigError {
                    summary: format!("two scopes share the ID `{}`", scopes[later].id),
                    location: scopes[later].id_location.clone(),
                    notes: vec![Note {
                        message: format!("also used by scope `{}`", scopes[earlier].name),
                        location: Some(scopes[earlier].id_location.clone()),
                    }],
                    help: "a scope ID is permanent identity; if this file was copied, generate a new ID with `uuidgen`".to_string(),
                });
            }
        }
    }
    Ok(())
}

/// ADR 0015's companion to [`check_unique_scope_ids`]: two scope entries may
/// not share a `path` either. Sharing a path means two scopes claiming one
/// directory, which the Slice 6 lease would later have to reject anyway;
/// catching it at load names both entries instead of one lease failure.
///
/// This compares `path` exactly as written (see the crate root docs) — it is
/// a textual check, not a filesystem one, so `projects/x` and `./projects/x`
/// are not caught as the same path here.
fn check_unique_scope_paths(scopes: &[ScopeEntry]) -> Result<(), ConfigError> {
    for later in 1..scopes.len() {
        for earlier in 0..later {
            if scopes[later].path == scopes[earlier].path {
                return Err(ConfigError {
                    summary: format!(
                        "two scopes share the path `{}`",
                        scopes[later].path.display()
                    ),
                    location: scopes[later].path_location.clone(),
                    notes: vec![Note {
                        message: format!("also used by scope `{}`", scopes[earlier].name),
                        location: Some(scopes[earlier].path_location.clone()),
                    }],
                    help: "a scope path names the one directory it claims; if this scope was copied, point `path` at its own directory".to_string(),
                });
            }
        }
    }
    Ok(())
}

fn join_with_and(items: &[String]) -> String {
    match items.len() {
        0 => String::new(),
        1 => items[0].clone(),
        2 => format!("{} and {}", items[0], items[1]),
        _ => {
            let (last, rest) = items.split_last().expect("checked non-empty above");
            format!("{}, and {}", rest.join(", "), last)
        }
    }
}

#[cfg(test)]
mod key_recovery_tests {
    use super::find_entry_key_line;

    /// A scope entry's keys, once `id:` establishes the indentation via the
    /// "- " prefix, all align at that same column.
    const KEY_INDENT: usize = 4;

    #[test]
    fn finds_the_single_entry_relative_key() {
        let source = "scopes:\n  - id: a\n    name: n\n    agent:\n      name: A\n";
        assert_eq!(
            find_entry_key_line(source, "agent", KEY_INDENT, (2, 5)),
            Some(4)
        );
    }

    /// A comment at the entry's own indentation mentioning the key text can
    /// never satisfy the scan, because a YAML comment begins with `#`, not
    /// with the key text itself.
    #[test]
    fn a_comment_mentioning_the_key_is_not_a_match() {
        let source =
            "scopes:\n  - id: a\n    name: n\n    # see agent: below\n    agent:\n      name: A\n";
        assert_eq!(
            find_entry_key_line(source, "agent", KEY_INDENT, (2, 6)),
            Some(5)
        );
    }

    #[test]
    fn missing_key_yields_none() {
        let source = "scopes:\n  - id: a\n    name: n\n    agents:\n      - name: A\n";
        assert_eq!(
            find_entry_key_line(source, "agent", KEY_INDENT, (2, 5)),
            None
        );
    }

    /// `"agents:"` must not satisfy a scan for `"agent:"` (nor the reverse):
    /// one key name is a prefix of the other's text, but not of its `"key:"`
    /// form, since the character right after `agent` differs (`s` vs `:`).
    #[test]
    fn the_two_key_names_do_not_match_each_other() {
        let agents_only = "scopes:\n  - id: a\n    agents:\n      - name: A\n";
        assert_eq!(
            find_entry_key_line(agents_only, "agent", KEY_INDENT, (2, 4)),
            None
        );

        let agent_only = "scopes:\n  - id: a\n    agent:\n      name: A\n";
        assert_eq!(
            find_entry_key_line(agent_only, "agents", KEY_INDENT, (2, 4)),
            None
        );
    }

    /// A line at a *different* indentation that happens to contain the same
    /// text must not match — this is what makes the nested `runtime:` block's
    /// own `agents:` key (indented differently from a scope entry's own keys)
    /// safe by construction rather than by luck.
    #[test]
    fn a_line_at_a_different_indentation_is_not_a_match() {
        let source = "runtime:\n  agents:\n    - name: decoy\nscopes:\n  - id: a\n    name: n\n    agent:\n      name: A\n";
        // The decoy `  agents:` sits at indentation 2, two lines before the
        // entry's own range even starts; restricting the scan to the entry's
        // line range excludes it before indentation is even considered.
        assert_eq!(
            find_entry_key_line(source, "agent", KEY_INDENT, (5, 8)),
            Some(7)
        );
    }

    /// The defensive half of the rule: if the scan is ever ambiguous — for
    /// example because a sibling entry's key sits at the very same
    /// indentation and the caller's line range was computed wrongly — it must
    /// refuse to guess rather than pick one and risk pointing at the wrong
    /// entry.
    #[test]
    fn ambiguous_scan_falls_back_to_none_instead_of_guessing() {
        let source =
            "scopes:\n  - id: a\n    agent:\n      name: A\n  - id: b\n    agent:\n      name: B\n";
        // A range that (wrongly) spans both entries finds two matches.
        assert_eq!(
            find_entry_key_line(source, "agent", KEY_INDENT, (2, 7)),
            None
        );
    }
}
