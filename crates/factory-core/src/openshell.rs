//! Compatibility path; the implementation is owned by factory-environment.
pub use factory_environment::openshell::*;

#[cfg(test)]
mod integration_tests {
    use super::*;
    use std::path::PathBuf;

    /// The awesome-herdr block the README and the PR hand to a person to
    /// paste is a block this build loads -- not just one that looks right.
    #[test]
    fn the_shipped_awesome_herdr_example_loads_and_validates() {
        let doc: serde_yaml_ng::Value = serde_yaml_ng::from_str(include_str!(
            "../../../examples/openshell/awesome-herdr.config.yaml"
        ))
        .unwrap();
        let mut scope: crate::config::Scope =
            serde_yaml_ng::from_value(doc["scope"].clone()).unwrap();
        scope.path = PathBuf::from("projects/awesome-herdr");
        let agent = scope.declared_agents().remove(0);
        assert_eq!(agent.name(), "awesome-herdr-curator");
        assert_eq!(agent.sandbox, crate::config::Sandbox::Openshell);
        crate::config::refuse_bad_openshell(&scope, &agent).unwrap();
        let block = agent.openshell.unwrap();
        assert!(
            block.fast_forward
                && block.download == Transfer::None
                && block.upload == Transfer::Workdir
        );
        assert_eq!(
            block.image, None,
            "Factory builds the curator's image (#234)"
        );
        let names: Vec<&str> = block.providers.iter().map(ProviderDecl::name).collect();
        assert_eq!(names, ["factory-claude", "factory-github"]);
        let [ProviderDecl::Managed(claude), ProviderDecl::Managed(github)] =
            block.providers.as_slice()
        else {
            panic!("both providers are the daemon's: {:?}", block.providers)
        };
        assert_eq!(claude.kind, "claude-code-oauth");
        assert_eq!(
            claude.credential,
            ProviderCredential::Secret("claude-oauth-token".into()),
            "declared once, in the catalogue (#244)"
        );
        assert_eq!(github.kind, "github-publish");
        assert_eq!(
            github.credential,
            ProviderCredential::Secret("github-gh-login".into())
        );
        let target = block.callback_target(Some("192.168.188.92:8791")).unwrap();
        let policy: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(&block.policy_yaml(&target).unwrap()).unwrap();
        for rule in [
            "github_read",
            "awesome_herdr_publish",
            "sources",
            CALLBACK_RULE,
        ] {
            assert!(
                policy["network_policies"].get(rule).is_some(),
                "{rule} missing"
            );
        }
        assert_eq!(
            policy["network_policies"][CALLBACK_RULE]["endpoints"][0]["host"],
            serde_yaml_ng::Value::from("192.168.188.92")
        );
        // The open web is read-only, inspected, and curl's alone (#218):
        // the audit verifies articles and finds new ones, but may write
        // nowhere but its own repository.
        let web = &policy["network_policies"]["web_read"];
        assert_eq!(web["endpoints"][0]["host"], serde_yaml_ng::Value::from("**.*.*"));
        for endpoint in web["endpoints"].as_sequence().unwrap() {
            assert_eq!(endpoint["access"], serde_yaml_ng::Value::from("read-only"), "{endpoint:?}");
            assert_eq!(endpoint["enforcement"], serde_yaml_ng::Value::from("enforce"), "{endpoint:?}");
        }
        assert_eq!(web["binaries"].as_sequence().unwrap().len(), 1);
        assert_eq!(web["binaries"][0]["path"], serde_yaml_ng::Value::from("/usr/bin/curl"));
    }
}
