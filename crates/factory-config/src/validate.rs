//! Raw parse tree to validated [`ScopeConfig`], per
//! `docs/slice-1-error-corpus.md`.
//!
//! Order of checks matters only in that each one must run on data the
//! previous checks have already proven sound (there is no point validating an
//! agent's `harness` in a document whose `version` this build does not
//! support). It does not matter beyond that: each fixture in the corpus
//! triggers exactly one problem.

use std::path::Path;

use serde_saphyr::Spanned;
use uuid::Uuid;

use crate::error::to_location;
use crate::harness::Harness;
use crate::raw::{RawAgent, RawDocument};
use crate::{Agent, ConfigError, Lifetime, Location, Note, SUPPORTED_VERSION, Scope, ScopeConfig};

pub(crate) fn validate(
    raw: RawDocument,
    origin: &Path,
    source: &str,
) -> Result<ScopeConfig, ConfigError> {
    if raw.version.value != SUPPORTED_VERSION {
        return Err(ConfigError {
            summary: format!("unsupported configuration version `{}`", raw.version.value),
            location: to_location(origin, raw.version.defined),
            notes: Vec::new(),
            help: format!("this build of Factory supports version {SUPPORTED_VERSION}"),
        });
    }

    let scope_id_location = to_location(origin, raw.scope.id.defined);
    let scope_id = Uuid::parse_str(&raw.scope.id.value).map_err(|_| ConfigError {
        summary: format!("`scope.id` is not a UUID: `{}`", raw.scope.id.value),
        location: scope_id_location.clone(),
        notes: Vec::new(),
        help: "generate one with `uuidgen`; the scope ID is permanent identity and must not be reused between scopes".to_string(),
    })?;

    let agent_items = resolve_agent_items(raw.agent, raw.agents, origin, source)?;

    let mut agents = Vec::with_capacity(agent_items.len());
    for item in &agent_items {
        agents.push(validate_agent(item, origin)?);
    }

    check_unique_names(&agent_items, &agents, origin)?;

    Ok(ScopeConfig {
        version: raw.version.value,
        scope: Scope {
            id: scope_id,
            name: raw.scope.name,
        },
        agents,
        origin: origin.to_path_buf(),
        scope_id_location,
    })
}

/// Case 1 (both set) and case 2 (neither set) live here: the one place that
/// decides between the `agent:` shorthand and the `agents:` list, per ADR
/// 0009's "two optional fields plus post-parse validation" model.
fn resolve_agent_items(
    agent: Option<Spanned<RawAgent>>,
    agents: Option<Vec<Spanned<RawAgent>>>,
    origin: &Path,
    source: &str,
) -> Result<Vec<Spanned<RawAgent>>, ConfigError> {
    match (agent, agents) {
        (Some(agent), Some(agents)) => {
            let agent_location = toplevel_key_location(source, "agent", origin)
                .unwrap_or_else(|| to_location(origin, agent.defined));
            let agents_location = toplevel_key_location(source, "agents", origin).or_else(|| {
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
                summary: "`agent` and `agents` are both set; a scope uses one or the other"
                    .to_string(),
                location: agent_location,
                notes,
                help: "keep `agents:` and delete the `agent:` block, or keep `agent:` and delete `agents:`".to_string(),
            })
        }
        (Some(agent), None) => Ok(vec![agent]),
        (None, Some(agents)) if !agents.is_empty() => Ok(agents),
        (None, _) => Err(no_agent_defined(origin)),
    }
}

/// Recover a Factory-owned top-level key's own line, for a diagnostic that is
/// about the key itself rather than its value.
///
/// `Spanned<T>` locates the spanned *value* node: for a key whose value is a
/// block mapping or a block sequence, that is the first nested line — one
/// below the key — and `serde-saphyr` exposes no way to ask for the key's own
/// position. Top-level Factory-owned keys are always at indentation zero, so
/// scanning the raw source for a line whose first character is not
/// whitespace and which begins `"{key}:"` is exact for the block-style YAML
/// every real and fixture config uses.
///
/// Returns `None` — not a guess — unless the scan finds *exactly one* match.
/// Zero matches means the key was never really there as plain top-level text
/// (should not happen for a key we know is `Some`, but this must not panic if
/// it does); more than one means something else on a zero-indent line began
/// with the same text — a comment cannot cause this (comments start with
/// `#`), but this must never assume that and guess. Callers fall back to the
/// value-node location instead: a diagnostic one line off is a papercut, one
/// that points at an unrelated line because of a false match is a defect.
fn toplevel_key_location(source: &str, key: &str, origin: &Path) -> Option<Location> {
    find_toplevel_key_line(source, key).map(|line| Location {
        file: origin.to_path_buf(),
        line,
        column: 1,
    })
}

/// The scan itself, kept separate from [`toplevel_key_location`] so it can be
/// unit-tested against raw strings without needing a `Path`.
fn find_toplevel_key_line(source: &str, key: &str) -> Option<usize> {
    let prefix = format!("{key}:");
    let mut matches = source
        .lines()
        .enumerate()
        .filter(|(_, line)| line.starts_with(prefix.as_str()));
    let (first_index, _) = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(first_index + 1)
}

fn no_agent_defined(origin: &Path) -> ConfigError {
    ConfigError {
        summary: "no agent is defined; a scope configures at least one agent".to_string(),
        location: Location {
            file: origin.to_path_buf(),
            line: 1,
            column: 1,
        },
        notes: Vec::new(),
        help: "add an `agent:` block, or an `agents:` list with at least one entry".to_string(),
    }
}

fn validate_agent(item: &Spanned<RawAgent>, origin: &Path) -> Result<Agent, ConfigError> {
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
            summary: format!("unsupported harness `{value}`"),
            location: to_location(origin, raw.harness.defined),
            notes: Vec::new(),
            help,
        }
    })?;

    let max_sessions = match &raw.max_sessions {
        Some(spanned) if spanned.value == 0 => {
            return Err(ConfigError {
                summary: "`max_sessions` is 0, so this agent could never start a session"
                    .to_string(),
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
                    summary: format!("unsupported lifetime `{other}`"),
                    location: to_location(origin, spanned.defined),
                    notes: Vec::new(),
                    help: "supported lifetimes are `permanent` and `temporary`; `permanent` is the default and may be omitted".to_string(),
                });
            }
        },
        None => Lifetime::default(),
    };

    Ok(Agent {
        name: raw.name.clone(),
        harness,
        max_sessions,
        lifetime,
    })
}

/// Case 3: two agents in the same scope sharing a name. Names are how
/// `factory task send` addresses an agent, so a collision is reported against
/// both places it was defined, in file order.
fn check_unique_names(
    items: &[Spanned<RawAgent>],
    agents: &[Agent],
    origin: &Path,
) -> Result<(), ConfigError> {
    for later in 1..agents.len() {
        for earlier in 0..later {
            if agents[later].name == agents[earlier].name {
                return Err(ConfigError {
                    summary: format!(
                        "two agents in this scope are both named `{}`",
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
    use super::find_toplevel_key_line;

    #[test]
    fn finds_the_single_toplevel_key() {
        let source = "version: 1\nagent:\n  name: A\n";
        assert_eq!(find_toplevel_key_line(source, "agent"), Some(2));
    }

    /// The scan requires the line to literally *begin* with `"{key}:"`. A
    /// YAML comment begins with `#`, so a comment that merely mentions the
    /// key text can never satisfy that — this is what makes the coordinator's
    /// "comment containing `agent:`" scenario safe by construction, not by
    /// luck.
    #[test]
    fn a_comment_mentioning_the_key_is_not_a_match() {
        let source = "# see agent: in the README\nversion: 1\nagent:\n  name: A\n";
        assert_eq!(find_toplevel_key_line(source, "agent"), Some(3));
    }

    #[test]
    fn missing_key_yields_none() {
        let source = "version: 1\nagents:\n  - name: A\n";
        assert_eq!(find_toplevel_key_line(source, "agent"), None);
    }

    /// `"agents:"` must not satisfy a scan for `"agent:"` (nor the reverse):
    /// one key name is a prefix of the other's text, but not of its
    /// `"key:"` form, since the character right after `agent` differs
    /// (`s` vs `:`).
    #[test]
    fn the_two_key_names_do_not_match_each_other() {
        let agents_only = "agents:\n  - name: A\n";
        assert_eq!(find_toplevel_key_line(agents_only, "agent"), None);

        let agent_only = "agent:\n  name: A\n";
        assert_eq!(find_toplevel_key_line(agent_only, "agents"), None);
    }

    /// The defensive half of the rule: if the scan is ever ambiguous, it must
    /// refuse to guess rather than pick one and risk pointing at the wrong
    /// line. Two lines beginning `agent:` at indentation zero cannot arise
    /// from a real document (a duplicate top-level key fails to parse before
    /// this scan ever runs), but the function must not assume that invariant
    /// and must degrade to `None` if it is ever violated.
    #[test]
    fn ambiguous_scan_falls_back_to_none_instead_of_guessing() {
        let source = "agent:\n  name: A\nagent:\n  name: B\n";
        assert_eq!(find_toplevel_key_line(source, "agent"), None);
    }
}
