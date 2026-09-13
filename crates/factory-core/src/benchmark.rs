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

use crate::agent::Lifetime;
use crate::config::{ForemanConfig, Scope};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One agent that declares a `Configuration`, as the Benchmarks tab lists it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

/// Every configuration Factory can dispatch, one per distinct
/// harness + full `args` + sandbox, including the foreman `daemon.foreman`
/// synthesizes for a scope that has none of its own. Sorted by harness, then
/// model, then flags; each configuration's agents are sorted by scope, then
/// name.
pub fn configurations(scopes: &[Scope], foreman: &ForemanConfig) -> Vec<Configuration> {
    struct Group {
        harness: String,
        model: Option<String>,
        flags: Vec<String>,
        sandbox: String,
        agents: Vec<ConfiguredAgent>,
    }

    let mut groups: HashMap<(String, Vec<String>, String), Group> = HashMap::new();

    for scope in scopes {
        let declared_names: std::collections::HashSet<String> =
            scope.declared_agents().iter().map(|a| a.name()).collect();
        for agent in scope.agents_with(foreman) {
            let key = (
                agent.harness.clone(),
                agent.args.clone(),
                agent.sandbox.as_str().to_string(),
            );
            let group = groups.entry(key).or_insert_with(|| {
                let (model, flags) = analyze_args(&agent.args);
                Group {
                    harness: agent.harness.clone(),
                    model,
                    flags,
                    sandbox: agent.sandbox.as_str().to_string(),
                    agents: Vec::new(),
                }
            });
            group.agents.push(ConfiguredAgent {
                scope: scope.name.clone(),
                agent: agent.name(),
                lifetime: lifetime_str(agent.lifetime),
                declared: declared_names.contains(&agent.name()),
            });
        }
    }

    let mut out: Vec<Configuration> = groups
        .into_values()
        .map(|mut g| {
            g.agents
                .sort_by(|a, b| a.scope.cmp(&b.scope).then_with(|| a.agent.cmp(&b.agent)));
            let model_source = g.model.as_ref().map(|_| "args".to_string());
            let mut missing = vec!["harness version".to_string()];
            if g.model.is_none() {
                missing.push("model".to_string());
            }
            missing.push("tool surface".to_string());
            missing.push("context policy".to_string());
            missing.push("retry budget".to_string());
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

    out.sort_by(|a, b| {
        a.harness
            .cmp(&b.harness)
            .then_with(|| a.model.cmp(&b.model))
            .then_with(|| a.flags.cmp(&b.flags))
    });
    out
}

fn lifetime_str(lifetime: Lifetime) -> String {
    lifetime.as_str().to_string()
}

/// One pass over a declaration's `args`: pull out the model (`--model <v>`,
/// `--model=<v>`, or `-m <v>` -- the last occurrence wins, the same rule
/// harnesses themselves use for a repeated flag) and reduce everything else
/// to a flag with its value elided. The model's own flag never appears in
/// the returned `flags`, so a page showing both never repeats the value.
fn analyze_args(args: &[String]) -> (Option<String>, Vec<String>) {
    let mut model = None;
    let mut flags = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];

        if let Some(v) = a.strip_prefix("--model=") {
            model = Some(v.to_string());
            i += 1;
            continue;
        }
        if a == "--model" || a == "-m" {
            if let Some(next) = args.get(i + 1).filter(|n| !n.starts_with('-')) {
                model = Some(next.clone());
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

        if let Some(eq) = a.find('=') {
            if a.starts_with('-') {
                flags.push(format!("{}=…", &a[..eq]));
                i += 1;
                continue;
            }
        }
        if a.starts_with('-') && a.len() > 1 {
            if args.get(i + 1).filter(|n| !n.starts_with('-')).is_some() {
                flags.push(format!("{a} …"));
                i += 2;
            } else {
                flags.push(a.clone()); // a bare switch, e.g. --yolo
                i += 1;
            }
            continue;
        }
        // A positional that belongs to no flag before it -- rare, and not
        // named in any format this module otherwise expects, but still not
        // a value this module may repeat back verbatim.
        flags.push("…".to_string());
        i += 1;
    }
    (model, flags)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Sandbox;

    fn scope(yaml: &str) -> Scope {
        serde_yaml_ng::from_str(yaml).unwrap()
    }

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
    fn agents_that_share_harness_args_and_sandbox_are_one_configuration() {
        let a = scope(
            "name: demo\npath: .\nagents:\n  - name: builder\n    harness: claude-code\n    sandbox: none\n    args: [--model, opus]\n  - name: reviewer\n    harness: claude-code\n    sandbox: none\n    args: [--model, opus]\n",
        );
        let configs = configurations(&[a], &ForemanConfig::default());
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].agents.len(), 2);
        assert_eq!(
            configs[0]
                .agents
                .iter()
                .map(|a| a.agent.as_str())
                .collect::<Vec<_>>(),
            vec!["builder", "reviewer"],
            "agents are sorted by scope then name"
        );
    }

    #[test]
    fn two_agents_on_the_same_harness_with_different_args_are_two_configurations() {
        let a = scope(
            "name: demo\npath: .\nagents:\n  - name: builder\n    harness: claude-code\n    args: [--model, opus]\n  - name: reviewer\n    harness: claude-code\n    args: [--model, sonnet]\n",
        );
        let configs = configurations(&[a], &ForemanConfig::default());
        assert_eq!(
            configs.len(),
            2,
            "different args means different configurations: {configs:?}"
        );
    }

    #[test]
    fn every_configuration_is_unpinned_with_model_named_only_when_it_is_missing() {
        let a = scope(
            "name: demo\npath: .\nagents:\n  - name: builder\n    harness: claude-code\n    args: [--model, opus]\n  - name: bare\n    harness: shell\n",
        );
        let configs = configurations(&[a], &ForemanConfig::default());
        for c in &configs {
            assert!(!c.pinned);
            assert_eq!(c.missing.first(), Some(&"harness version".to_string()));
            assert_eq!(c.missing.last(), Some(&"retry budget".to_string()));
            assert_eq!(c.missing.contains(&"model".to_string()), c.model.is_none());
        }
        let builder = configs.iter().find(|c| c.harness == "claude-code").unwrap();
        assert_eq!(builder.model.as_deref(), Some("opus"));
        assert_eq!(builder.model_source.as_deref(), Some("args"));
        let bare = configs.iter().find(|c| c.harness == "shell").unwrap();
        assert_eq!(bare.model, None);
        assert_eq!(bare.model_source, None);
    }

    #[test]
    fn a_synthesized_foreman_is_included_and_marked_undeclared() {
        let a = scope("name: demo\npath: .\n");
        let cfg = ForemanConfig {
            enabled: true,
            ..Default::default()
        };
        let configs = configurations(&[a], &cfg);
        assert_eq!(configs.len(), 1);
        let foreman = &configs[0].agents[0];
        assert_eq!(foreman.agent, "foreman");
        assert!(
            !foreman.declared,
            "a synthesized foreman is not written into any scope's config"
        );
    }

    #[test]
    fn a_declared_agent_is_marked_declared() {
        let a =
            scope("name: demo\npath: .\nagents:\n  - name: builder\n    harness: claude-code\n");
        let configs = configurations(&[a], &ForemanConfig::default());
        assert!(configs[0].agents[0].declared);
    }

    #[test]
    fn configurations_sort_by_harness_then_model_then_flags() {
        let a = scope(
            "name: demo\npath: .\nagents:\n  - name: b\n    harness: pi\n    args: [--model, opus]\n  - name: c\n    harness: claude-code\n    args: [--model, sonnet]\n  - name: d\n    harness: claude-code\n    args: [--model, opus]\n",
        );
        let configs = configurations(&[a], &ForemanConfig::default());
        let harnesses: Vec<&str> = configs.iter().map(|c| c.harness.as_str()).collect();
        assert_eq!(harnesses, vec!["claude-code", "claude-code", "pi"]);
        assert_eq!(configs[0].model.as_deref(), Some("opus"));
        assert_eq!(configs[1].model.as_deref(), Some("sonnet"));
    }

    #[test]
    fn sandbox_is_part_of_the_grouping_key() {
        let a = scope(
            "name: demo\npath: .\nagents:\n  - name: boxed\n    harness: claude-code\n    sandbox: docker\n  - name: unboxed\n    harness: claude-code\n    sandbox: none\n",
        );
        let configs = configurations(&[a], &ForemanConfig::default());
        assert_eq!(configs.len(), 2);
        assert!(configs
            .iter()
            .any(|c| c.sandbox == Sandbox::Docker.as_str()));
        assert!(configs.iter().any(|c| c.sandbox == Sandbox::None.as_str()));
    }

    #[test]
    fn no_configuration_carries_the_raw_args_anywhere_serialized() {
        let a = scope(
            "name: demo\npath: .\nagents:\n  - name: builder\n    harness: claude-code\n    args: [--model, opus, --api-key, s3cret]\n",
        );
        let configs = configurations(&[a], &ForemanConfig::default());
        let json = serde_json::to_string(&configs).unwrap();
        assert!(!json.contains("s3cret"));
    }
}
