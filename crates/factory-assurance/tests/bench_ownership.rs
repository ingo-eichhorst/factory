//! The L5 owner must work without importing the upper configuration facade.
use factory_assurance::{
    bench::{config_hash, derive_config, BenchOrigin},
    benchmark::{configurations, ConfigurationInput, ConfiguredAgent},
};

fn input(scope: &str, agent: &str, secret: &str) -> ConfigurationInput {
    ConfigurationInput::new(
        "shell".into(),
        vec![
            "--model".into(),
            "model".into(),
            "--api-key".into(),
            secret.into(),
        ],
        "none".into(),
        ConfiguredAgent {
            scope: scope.into(),
            agent: agent.into(),
            lifetime: "task".into(),
            declared: true,
        },
    )
}

#[test]
fn redacted_collision_never_combines_different_configuration_arguments() {
    let configs = configurations(&[
        input("demo", "a", "first-secret"),
        input("demo", "b", "second-secret"),
    ]);
    assert_eq!(
        configs.len(),
        2,
        "full arguments, not rendered flags, are the grouping key"
    );
    assert_eq!(configs[0].flags, configs[1].flags);
    let json = serde_json::to_string(&configs).unwrap();
    assert!(!json.contains("first-secret") && !json.contains("second-secret"));
    assert_ne!(
        config_hash("shell", &["first-secret".into()], "none"),
        config_hash("shell", &["second-secret".into()], "none")
    );
}

#[test]
fn grouping_retains_scope_metadata_and_is_stable_for_reordered_inputs() {
    let first = configurations(&[input("z", "b", "same"), input("a", "a", "same")]);
    let second = configurations(&[input("a", "a", "same"), input("z", "b", "same")]);
    assert_eq!(first, second);
    assert_eq!(first.len(), 1);
    assert_eq!(
        first[0]
            .agents
            .iter()
            .map(|a| a.scope.as_str())
            .collect::<Vec<_>>(),
        ["a", "z"]
    );
    assert_eq!(first[0].model.as_deref(), Some("model"));
    assert_eq!(first[0].model_source.as_deref(), Some("args"));
    assert!(!first[0].pinned);
}

#[test]
fn the_benchmark_owner_preserves_and_decodes_l4s_opaque_legacy_reference() {
    let origin = BenchOrigin {
        bench_run_id: "r".into(),
        case_id: "c".into(),
        agent: "a".into(),
        attempt: 2,
    };
    let reference: factory_process::origin::OriginRef = origin.clone().into();
    assert_eq!(
        serde_json::to_value(&reference).unwrap(),
        serde_json::to_value(&origin).unwrap()
    );
    assert_eq!(BenchOrigin::try_from(&reference).unwrap(), origin);
    assert!(
        BenchOrigin::try_from(&factory_process::origin::OriginRef::opaque("unsupported")).is_err()
    );
    let snapshot = derive_config("shell", &["--api-key".into(), "private".into()], "none");
    assert!(!serde_json::to_string(&snapshot)
        .unwrap()
        .contains("private"));
}
