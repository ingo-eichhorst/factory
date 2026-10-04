//! Compatibility path; the implementation is owned by factory-infrastructure.
pub use factory_infrastructure::renewals::*;

#[cfg(test)]
mod integration_tests {
    use super::*;

    #[test]
    fn root_and_scope_yaml_roundtrip_and_misplacement_is_refused() {
        let yaml = "instance: {id: test, name: test}\nrenewals:\n - {name: token, kind: credential, expires: never, lead: 30d}\nrenewals_notify: {command: notify-owner, timeout: 15s}\nscope:\n id: demo\n name: demo\n renewals:\n  - {name: licence, kind: licence, expires: 2090-01-01, affects: [demo]}\n";
        let config: crate::config::Config = serde_yaml_ng::from_str(yaml).unwrap();
        config.validate_instance().unwrap();
        assert!(config.renewals[0].no_expiry());
        assert_eq!(config.scope.as_ref().unwrap().renewals.len(), 1);
        let serialized = serde_yaml_ng::to_string(&config).unwrap();
        let restored: crate::config::Config = serde_yaml_ng::from_str(&serialized).unwrap();
        assert_eq!(restored.renewals, config.renewals);
        assert_eq!(
            restored.scope.as_ref().unwrap().renewals,
            config.scope.as_ref().unwrap().renewals
        );
        let doc = serde_yaml_ng::from_str(yaml).unwrap();
        assert!(crate::config::refuse_misplaced_scope_renewals(
            &doc,
            std::path::Path::new("/tmp/demo/.factory/config.yaml")
        )
        .is_err());
        let bad: crate::config::Config = serde_yaml_ng::from_str("instance: {id: test, name: test}\nrenewals: [{name: x, kind: credential, expires: tomorrow}]\n").unwrap();
        assert!(bad.validate_instance().is_err());
        assert!(
            serde_yaml_ng::from_str::<RenewalsNotify>("command: notify\ntimeout: 0s\n").is_err()
        );
    }
}
