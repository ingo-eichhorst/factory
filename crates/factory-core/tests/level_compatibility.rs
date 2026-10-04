//! Public core paths remain the identical owning-crate types, not copies.
use factory_core::{adapter::agent, config, error, task};
use factory_kernel::{LaunchKind, LaunchSpec, Schedule, Span};
use serde_json::json;

#[test]
fn compatibility_values_are_the_canonical_kernel_types() {
    let launch: agent::LaunchSpec = LaunchSpec {
        kind: LaunchKind::Named("shell".into()),
        args: vec![],
        env: Default::default(),
    };
    let _: LaunchSpec = launch;
    let schedule: task::Schedule = Schedule::Every { seconds: 300 };
    let _: Schedule = schedule;
    let span: factory_core::environments::Span = Span::try_from("5m".to_string()).unwrap();
    let _: Span = span;
    let error: error::FactoryError = factory_kernel::FactoryError::Denied("no grant".into());
    assert_eq!(error.code(), "denied");
}

#[test]
fn dependency_config_is_the_same_l2_declaration_with_the_same_json() {
    let wire = json!({"services": [{
        "name": "api", "transport": "network", "endpoints": ["api.test:443"],
        "effects": ["read"]
    }]});
    let old: config::DependenciesConfig = serde_json::from_value(wire.clone()).unwrap();
    let owned: factory_environment::declarations::DependenciesConfig = old;
    assert_eq!(serde_json::to_value(owned).unwrap(), wire);
}
