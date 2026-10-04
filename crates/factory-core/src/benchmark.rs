//! External compatibility bridge: scope/foreman declarations are projected
//! into narrow L5 inputs; grouping and argument redaction are owned by L5.
use crate::config::{ForemanConfig, Scope};
pub use factory_assurance::benchmark::{Configuration, ConfigurationInput, ConfiguredAgent};

pub fn configurations(scopes: &[Scope], foreman: &ForemanConfig) -> Vec<Configuration> {
    let mut inputs = Vec::new();
    for scope in scopes {
        let declared_names: std::collections::HashSet<String> =
            scope.declared_agents().iter().map(|a| a.name()).collect();
        for agent in scope.agents_with(foreman) {
            let metadata = ConfiguredAgent {
                scope: scope.name.clone(),
                agent: agent.name(),
                lifetime: agent.lifetime.as_str().to_string(),
                declared: declared_names.contains(&agent.name()),
            };
            inputs.push(ConfigurationInput::new(
                agent.harness,
                agent.args,
                agent.sandbox.as_str().to_string(),
                metadata,
            ));
        }
    }
    factory_assurance::benchmark::configurations(&inputs)
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::config::Sandbox;
    fn scope(yaml: &str) -> Scope {
        serde_yaml_ng::from_str(yaml).unwrap()
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
    fn a_sandbox_only_tie_sorts_the_same_way_across_many_calls() {
        // `builder` and `runner` share harness, model and (therefore)
        // rendered `flags`, and differ only in `sandbox` -- two distinct
        // `HashMap` groups that the old comparator could not order between
        // itself, leaving it to whatever order the map happened to iterate
        // in. Calling `configurations` repeatedly on the same input, rather
        // than once, is the point: a `HashMap`'s iteration order is fixed
        // for the lifetime of one process, so a single call would not have
        // caught a comparator that silently depended on it.
        let a = scope(
            "name: demo\npath: .\nagents:\n  - name: builder\n    harness: claude-code\n    sandbox: docker\n    args: [--model, opus]\n  - name: runner\n    harness: claude-code\n    sandbox: none\n    args: [--model, opus]\n",
        );
        let foreman = ForemanConfig::default();
        let first = configurations(std::slice::from_ref(&a), &foreman);
        assert_eq!(first.len(), 2);
        let expected: Vec<String> = first.iter().map(|c| c.sandbox.clone()).collect();
        assert_eq!(expected, vec!["docker".to_string(), "none".to_string()]);
        for _ in 0..50 {
            let configs = configurations(std::slice::from_ref(&a), &foreman);
            let sandboxes: Vec<String> = configs.iter().map(|c| c.sandbox.clone()).collect();
            assert_eq!(
                sandboxes, expected,
                "sort order must be stable across calls"
            );
        }
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
