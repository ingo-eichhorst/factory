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

#[test]
fn old_agent_and_runtime_paths_are_canonical_l3_and_l0_types() {
    let reference: task::SessionRef = factory_kernel::SessionRef {
        runtime: "runtime".into(),
        handle: "opaque".into(),
        meta: Default::default(),
    };
    let _: factory_kernel::SessionRef = reference;
    let health: factory_core::harness::HealthProbe =
        factory_agents::harness::HealthProbe::version("shell");
    assert_eq!(health.program(), "shell");
    let role: factory_core::role::Role = factory_agents::role::Role::worker();
    assert_eq!(role.as_str(), "worker");
    let usage: factory_core::usage::SessionUsage =
        factory_agents::usage::SessionUsage::parse("{\"schema\":1}").unwrap();
    assert!(usage.sessions.is_empty());
    // Trait-object coercion compiles only when both paths name the same seam.
    let old: Option<&dyn factory_core::adapter::AgentRuntime> = None;
    let canonical: Option<&dyn factory_agents::runtime::AgentRuntime> = old;
    assert!(canonical.is_none());
}
