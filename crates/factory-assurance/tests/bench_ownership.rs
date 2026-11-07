//! The L5 owner must work without importing the upper configuration facade.
use factory_assurance::{
    bench::{
        aggregate, compose, config_hash, derive_config, BenchAttempt, BenchOrigin, BenchRun,
        BenchRunStatus, Verdict,
    },
    bench_store::BenchStore,
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
        ..Default::default()
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

fn finished(id: &str, agent: &str, verdict: Verdict) -> BenchRun {
    let mut attempt = BenchAttempt::pending(format!("{id}-a1"), "c1".into(), agent.into(), 1);
    attempt.verdict = Some(verdict);
    attempt.gate_output = Some(format!("{agent} gate"));
    attempt.config = Some(derive_config(
        "shell",
        &["--model".into(), agent.into()],
        "none",
    ));
    BenchRun {
        id: id.into(),
        dataset: "finance".into(),
        dataset_revision: 1,
        cases: Vec::new(),
        case_bases: Default::default(),
        agents: vec![agent.into()],
        attempts_per_case: 1,
        concurrency: 1,
        status: BenchRunStatus::Done,
        attempts: vec![attempt],
        started_at: chrono::Utc::now(),
        ended_at: Some(chrono::Utc::now()),
        composed_from: Vec::new(),
        source_revisions: Default::default(),
        note: None,
    }
}

/// #388: composing runs is the L5 owner's own work -- the pure combination,
/// the store write and the results table, from the assurance crate alone.
/// Who may ask for it (the owner only, like `bench clean`) is the daemon's
/// one authorization check, `access.rs`, and is tested there
/// (`bench_compose_and_clean_are_the_owners_alone_even_when_a_role_has_every_grant`).
#[tokio::test]
async fn the_benchmark_owner_composes_finished_runs_without_the_upper_facade() {
    let store = BenchStore::in_memory().unwrap();
    let sources = [
        finished("qwen-run", "qwen", Verdict::Pass),
        finished("sonnet-run", "sonnet", Verdict::Fail),
    ];
    for source in &sources {
        store.put_run_with_attempts(source).await.unwrap();
    }
    let mut n = 0;
    let composed = compose("finance", &sources, &[], None, || {
        n += 1;
        format!("composed-{n}")
    })
    .unwrap();
    store.put_run_with_attempts(&composed).await.unwrap();

    let loaded = store.get_run(&composed.id).await.unwrap().unwrap();
    assert_eq!(loaded.composed_from, ["qwen-run", "sonnet-run"]);
    assert_eq!(
        aggregate(&loaded.attempts).len(),
        2,
        "one row per configuration"
    );
    for source in &sources {
        let reread = store.get_run(&source.id).await.unwrap().unwrap();
        assert_eq!(&reread, source, "a source run is read, never changed");
    }
}
