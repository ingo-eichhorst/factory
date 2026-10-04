//! What a benchmark result would need to be comparable, derived from what
//! Factory can already say about the agents it can dispatch. **v1 declares
//! and displays; it runs nothing and records no score.** `configurations`
//! groups every declared and synthesized agent by the tuple a result would
//! have to carry -- harness, full arguments, and sandbox -- and says which
//! of the fields a real benchmark needs are recorded today and which are
//! not. See the L5 Benchmarks tab and the issue that added it for the case
//! this makes.
//!
//! **No argument value but the extracted model ever leaves this module.** An
//! `args` entry can be a secret (`--api-key ...`), the same rule the Secrets
//! tab already lives by, so every other flag is reduced to its name with its
//! value elided before it is ever put on a `Configuration`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One agent that declares a `Configuration`, as the Benchmarks tab lists it.
/// `Ord` (by `scope`, then `agent`, then `lifetime`, then `declared`) exists
/// only so `configurations`'s own sort can use a configuration's full agent
/// list as its last tiebreaker -- see the comment there.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ConfiguredAgent {
    pub scope: String,
    pub agent: String,
    /// `permanent`, `temporary`, or `task`.
    pub lifetime: String,
    /// `false` for the foreman synthesized by `daemon.foreman` -- it is not
    /// written into any scope's config, so it cannot be compared against
    /// `declared_agents()` the way every other row here is.
    pub declared: bool,
}

/// The tuple a benchmark result would have to carry to be comparable to
/// another one, plus which of it Factory already records. Every agent that
/// shares a harness, full `args`, and sandbox is one configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Configuration {
    pub harness: String,
    /// Parsed out of `args`; `None` when the declaration never names one
    /// (the harness's own default applies, and this module does not know
    /// what that is).
    #[serde(default)]
    pub model: Option<String>,
    /// `"args"` when `model` came from a declared flag, else `None` --
    /// `model` and `model_source` are always both present or both absent.
    #[serde(default)]
    pub model_source: Option<String>,
    /// Every other argument, reduced to its flag with its value elided.
    /// `args` themselves never leave this module.
    pub flags: Vec<String>,
    /// `none`, `docker`, or `srt` -- declared, not enforced. See
    /// `config::Sandbox`.
    pub sandbox: String,
    /// Every agent that declares this exact configuration, sorted by scope
    /// then name.
    pub agents: Vec<ConfiguredAgent>,
    /// What a comparable score would still need, in a fixed order. Never
    /// empty today -- see `pinned`.
    pub missing: Vec<String>,
    /// `missing.is_empty()`. Always `false` in v1: `"harness version"` is
    /// never recorded by anything today.
    pub pinned: bool,
}

/// Resolved declaration command input. Raw arguments are private and this
/// type deliberately implements neither Serialize nor Debug: only the model,
/// redacted flags and hash may leave the L5 owner. Scope/foreman resolution
/// is performed by the caller, not by importing its upper configuration hub.
/// ```compile_fail
/// use factory_assurance::benchmark::{ConfigurationInput, ConfiguredAgent};
/// let input = ConfigurationInput::new("shell".into(), vec![], "none".into(), ConfiguredAgent {
///     scope: "demo".into(), agent: "shell".into(), lifetime: "task".into(), declared: true,
/// });
/// let _ = serde_json::to_value(input); // Raw input must never be serialized.
/// ```
#[derive(Clone)]
pub struct ConfigurationInput {
    harness: String,
    full_args: Vec<String>,
    sandbox: String,
    agent: ConfiguredAgent,
}
impl ConfigurationInput {
    pub fn new(
        harness: String,
        full_args: Vec<String>,
        sandbox: String,
        agent: ConfiguredAgent,
    ) -> Self {
        Self {
            harness,
            full_args,
            sandbox,
            agent,
        }
    }
}

/// Every configuration Factory can dispatch, one per distinct
/// harness + full `args` + sandbox, including the foreman `daemon.foreman`
/// synthesizes for a scope that has none of its own. Sorted by harness, then
/// model, then flags, then sandbox, then the full agent list (see the sort
/// call below for why the last two are there); each configuration's agents
/// are sorted by scope, then name.
pub fn configurations(inputs: &[ConfigurationInput]) -> Vec<Configuration> {
    struct Group {
        harness: String,
        model: Option<String>,
        flags: Vec<String>,
        sandbox: String,
        agents: Vec<ConfiguredAgent>,
    }

    let mut groups: HashMap<(String, Vec<String>, String), Group> = HashMap::new();

    for input in inputs {
        let key = (
            input.harness.clone(),
            input.full_args.clone(),
            input.sandbox.clone(),
        );
        let group = groups.entry(key).or_insert_with(|| {
            let (model, flags) = analyze_args(&input.full_args);
            Group {
                harness: input.harness.clone(),
                model,
                flags,
                sandbox: input.sandbox.clone(),
                agents: Vec::new(),
            }
        });
        group.agents.push(input.agent.clone());
    }

    let mut out: Vec<Configuration> = groups
        .into_values()
        .map(|mut g| {
            g.agents
                .sort_by(|a, b| a.scope.cmp(&b.scope).then_with(|| a.agent.cmp(&b.agent)));
            let model_source = g.model.as_ref().map(|_| "args".to_string());
            let missing = missing_fields(&g.model);
            let pinned = missing.is_empty();
            Configuration {
                harness: g.harness,
                model: g.model,
                model_source,
                flags: g.flags,
                sandbox: g.sandbox,
                agents: g.agents,
                missing,
                pinned,
            }
        })
        .collect();

    // `groups` is a `HashMap`, so its iteration order is not the same from
    // one process to the next -- two configurations that tie on harness,
    // model and flags but differ only in `sandbox` (grouping is keyed on
    // `sandbox` too, so they are genuinely two groups) would otherwise sort
    // however the map happened to hand them back. `sandbox` breaks that tie;
    // `agents` is the tiebreaker of last resort, since no two distinct
    // configurations can ever share the exact same sorted agent list, which
    // makes this comparator total rather than merely "usually enough".
    out.sort_by(|a, b| {
        a.harness
            .cmp(&b.harness)
            .then_with(|| a.model.cmp(&b.model))
            .then_with(|| a.flags.cmp(&b.flags))
            .then_with(|| a.sandbox.cmp(&b.sandbox))
            .then_with(|| a.agents.cmp(&b.agents))
    });
    out
}

/// What a comparable score still needs, for a given `model` outcome -- shared
/// by `configurations` above and `bench::derive_config`, which both start
/// from the same accounting of what nothing records yet. `"model"` is the
/// only entry that depends on the argument; the rest is always missing.
pub(crate) fn missing_fields(model: &Option<String>) -> Vec<String> {
    let mut missing = vec!["harness version".to_string()];
    if model.is_none() {
        missing.push("model".to_string());
    }
    missing.push("tool surface".to_string());
    missing.push("context policy".to_string());
    missing.push("retry budget".to_string());
    missing
}

/// The shape a token must have before this module will show any part of it.
/// Deliberately an allow-list rather than the `starts_with('-')` test this
/// replaced: that test decided a token was "a flag" by how it *started*,
/// which is exactly backwards when the thing that follows a flag can be an
/// arbitrary value that itself happens to start with `-`
/// (`--api-key -s3cretDASHvalue`). Under an allow-list, a token that fails
/// every recognised shape is never partially trusted -- the whole token is
/// elided, never just the part after its `=`.
enum FlagShape<'a> {
    /// `--name`, matching `^--[a-z][a-z0-9-]{0,39}$`. Safe to show as-is.
    LongBare(&'a str),
    /// `--name=value`, the name matching the same pattern. The name is safe
    /// to show; the value never is.
    LongWithValue(&'a str),
    /// `-x`, matching `^-[A-Za-z]$`. Safe to show as-is.
    Short,
    /// Everything else: prose, a secret, a malformed flag, a bare
    /// positional. Never shown, whole.
    Other,
}

/// `^[a-z][a-z0-9-]{0,39}$` -- the name portion of a long flag, with or
/// without an attached value. At most 40 characters so a single absurdly
/// long token cannot be shown just because its first 40-or-fewer characters
/// happen to look plausible.
fn is_safe_flag_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    name.len() <= 40 && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `^-[A-Za-z]$` -- a short flag, either case, and nothing else attached.
fn is_safe_short_flag(token: &str) -> bool {
    let b = token.as_bytes();
    b.len() == 2 && b[0] == b'-' && b[1].is_ascii_alphabetic()
}

fn classify_flag(token: &str) -> FlagShape<'_> {
    if let Some(rest) = token.strip_prefix("--") {
        return match rest.find('=') {
            Some(eq) if is_safe_flag_name(&rest[..eq]) => FlagShape::LongWithValue(&rest[..eq]),
            Some(_) => FlagShape::Other,
            None if is_safe_flag_name(rest) => FlagShape::LongBare(rest),
            None => FlagShape::Other,
        };
    }
    if is_safe_short_flag(token) {
        FlagShape::Short
    } else {
        FlagShape::Other
    }
}

/// An extracted model value that turned out to be empty (`--model=` or
/// `--model ""`) is the same as no model at all: there is nothing to show,
/// and nothing for a later increment to pin.
fn non_empty(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// Pushes `shown` -- a flag already known safe to display -- onto `flags`,
/// either bare or with a trailing elided value depending on whether the
/// token that follows it looks like a flag of its own (see `classify_flag`).
/// A flag with no attached `=` cannot tell, from its own text alone, whether
/// the next token is its value or an unrelated switch; not-a-recognised-flag
/// is the signal that it was consumed as this flag's value. Returns how many
/// argument slots were consumed (1 or 2) for the caller to advance by.
fn consume_flag(flags: &mut Vec<String>, shown: String, next: Option<&String>) -> usize {
    match next {
        Some(n) if matches!(classify_flag(n), FlagShape::Other) => {
            flags.push(format!("{shown} …"));
            2
        }
        _ => {
            flags.push(shown);
            1
        }
    }
}

/// One pass over a declaration's `args`: pull out the model (`--model <v>`,
/// `--model=<v>`, or `-m <v>` -- the last occurrence wins, the same rule
/// harnesses themselves use for a repeated flag) and reduce everything else
/// to a flag with its value elided. The model's own flag never appears in
/// the returned `flags`, so a page showing both never repeats the value.
pub(crate) fn analyze_args(args: &[String]) -> (Option<String>, Vec<String>) {
    let mut model = None;
    let mut flags = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];

        if let Some(v) = a.strip_prefix("--model=") {
            model = non_empty(v);
            i += 1;
            continue;
        }
        if a == "--model" || a == "-m" {
            if let Some(next) = args.get(i + 1).filter(|n| !n.starts_with('-')) {
                model = non_empty(next);
                i += 2;
            } else {
                // A model flag with nothing usable after it says less than
                // the truth if shown as a bare flag in `flags` -- it looks
                // like `--yolo`, a switch, when it is really an incomplete
                // attempt to set a value. Drop it rather than mislabel it.
                i += 1;
            }
            continue;
        }

        i += match classify_flag(a) {
            FlagShape::LongWithValue(name) => {
                flags.push(format!("--{name}=…"));
                1
            }
            FlagShape::LongBare(name) => {
                consume_flag(&mut flags, format!("--{name}"), args.get(i + 1))
            }
            FlagShape::Short => consume_flag(&mut flags, a.clone(), args.get(i + 1)),
            FlagShape::Other => {
                // Neither a flag nor (having reached here) a value already
                // claimed by one before it -- a bare positional, a
                // malformed flag, or a value with nowhere to attach. Never
                // repeated back verbatim.
                flags.push("…".to_string());
                1
            }
        };
    }
    (model, flags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_is_read_from_every_spelling_and_the_last_one_wins() {
        assert_eq!(
            analyze_args(&["--model".into(), "opus".into()]).0,
            Some("opus".into())
        );
        assert_eq!(
            analyze_args(&["--model=sonnet".into()]).0,
            Some("sonnet".into())
        );
        assert_eq!(
            analyze_args(&["-m".into(), "haiku".into()]).0,
            Some("haiku".into())
        );
        assert_eq!(
            analyze_args(&["--model".into(), "opus".into(), "-m".into(), "haiku".into()]).0,
            Some("haiku".into()),
            "a repeated flag is won by its last occurrence, same as the harnesses themselves"
        );
        assert_eq!(analyze_args(&["--yolo".into()]).0, None);
    }

    #[test]
    fn no_argument_value_but_the_model_ever_appears_in_flags() {
        let (model, flags) = analyze_args(&[
            "--model".into(),
            "opus".into(),
            "--api-key".into(),
            "s3cret".into(),
            "--permission-mode".into(),
            "bypassPermissions".into(),
            "--yolo".into(),
        ]);
        assert_eq!(model, Some("opus".into()));
        assert_eq!(flags, vec!["--api-key …", "--permission-mode …", "--yolo"]);
        for f in &flags {
            assert!(!f.contains("s3cret"), "{f}");
            assert!(!f.contains("bypassPermissions"), "{f}");
        }
    }

    #[test]
    fn a_bare_model_flag_with_nothing_after_it_is_dropped_rather_than_mislabeled() {
        let (model, flags) = analyze_args(&["--model".into()]);
        assert_eq!(model, None);
        assert!(flags.is_empty());
    }

    #[test]
    fn a_stray_positional_is_elided_too() {
        let (_, flags) = analyze_args(&["s3cret-standalone-value".into()]);
        assert_eq!(flags, vec!["…".to_string()]);
    }

    #[test]
    fn a_value_starting_with_a_dash_is_never_shown_as_a_flag_of_its_own() {
        // The bug this module existed to prevent, and the one the old
        // `starts_with('-')` test let through: a value that itself begins
        // with `-` is not a recognised flag shape, so it is elided and
        // folded into the flag before it rather than shown bare.
        let (_, flags) = analyze_args(&["--api-key".into(), "-s3cretDASHvalue".into()]);
        assert_eq!(flags, vec!["--api-key …".to_string()]);
        for f in &flags {
            assert!(!f.contains("s3cret"), "{f}");
        }
    }

    #[test]
    fn a_malformed_short_flag_between_two_real_flags_is_folded_into_the_one_before_it() {
        let (_, flags) = analyze_args(&["--api-key".into(), "-tok".into(), "--yolo".into()]);
        assert_eq!(flags, vec!["--api-key …".to_string(), "--yolo".to_string()]);
        for f in &flags {
            assert!(!f.contains("tok"), "{f}");
        }
    }

    #[test]
    fn a_mixed_case_flag_is_elided_whole_rather_than_partially_shown() {
        let (_, flags) = analyze_args(&["--Token=AbC".into()]);
        assert_eq!(flags, vec!["…".to_string()]);
        for f in &flags {
            assert!(!f.contains("Token") && !f.contains("AbC"), "{f}");
        }
    }

    #[test]
    fn a_short_flag_shaped_wrong_is_elided_not_shown() {
        let (_, flags) = analyze_args(&["-xyz".into()]);
        assert_eq!(flags, vec!["…".to_string()]);
    }

    #[test]
    fn a_sixty_character_flag_name_is_too_long_to_be_shown() {
        let long = format!("--{}", "a".repeat(58));
        assert_eq!(long.len(), 60);
        let (_, flags) = analyze_args(std::slice::from_ref(&long));
        assert_eq!(flags, vec!["…".to_string()]);
        for f in &flags {
            assert!(!f.contains(&"a".repeat(58)), "{f}");
        }
    }

    #[test]
    fn an_empty_model_value_is_treated_as_absent() {
        let (model, flags) = analyze_args(&["--model=".into()]);
        assert_eq!(model, None);
        assert!(flags.is_empty());

        let (model, flags) = analyze_args(&["--model".into(), "".into()]);
        assert_eq!(model, None);
        assert!(flags.is_empty());
    }

    #[test]
    fn a_valid_long_flag_with_value_and_a_bare_switch_still_render_normally() {
        let (_, flags) = analyze_args(&["--permission-mode=plan".into(), "--yolo".into()]);
        assert_eq!(
            flags,
            vec!["--permission-mode=…".to_string(), "--yolo".to_string()]
        );
    }
}
