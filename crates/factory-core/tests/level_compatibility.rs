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

#[test]
fn the_agent_seam_and_shared_dispatch_values_are_canonical() {
    let old: Option<&dyn factory_core::adapter::Agent> = None;
    let canonical: Option<&dyn factory_agents::adapter::Agent> = old;
    assert!(canonical.is_none());
    let hints: factory_core::adapter::KnowledgeHints = factory_kernel::KnowledgeHints {
        vault: "/tmp/vault".into(),
        hits: vec![],
    };
    let _: factory_kernel::KnowledgeHints = hints;
    let origin: task::WorkflowOrigin = factory_kernel::WorkflowOrigin {
        workflow_id: "w".into(),
        workflow_run_id: "wr".into(),
        node_id: "n".into(),
        workspace: Some(factory_kernel::WorkflowWorkspace {
            base_ref: "integration".into(),
        }),
    };
    let _: factory_kernel::WorkflowOrigin = origin;
    let assignment: factory_core::adapter::AssignedTask =
        factory_agents::assignment::AssignedTask::new("t", "title", "instructions");
    let _: factory_agents::assignment::AssignedTask = assignment;
}

#[test]
fn a_process_task_projects_to_an_independent_wire_compatible_dispatch_snapshot() {
    let mut task: task::Task = serde_json::from_value(json!({
        "id":"t1", "title":"title", "instructions":"instructions", "scope":"demo",
        "agent":"shell", "runtime":"herdr", "status":"blocked",
        "created_at":"2024-01-01T00:00:00Z", "updated_at":"2024-01-01T00:00:00Z",
        "schedule":{"every":{"seconds":300}}, "runs":4, "result":"old result",
        "error":"old error", "routed_to":"exit", "estimate_seconds":120,
        "labels":{"goal":"g", "arbitrary":"value"}, "worktree":true, "knowledge_hints":true,
        "after":["upstream"], "after_condition":"route", "depends_on":["dep"],
        "decomposition_part":"part", "parent_task_id":"parent", "category":"feature",
        "bench_origin":{"bench_run_id":"b", "case_id":"c", "agent":"shell", "attempt":2},
        "workflow_origin":{"workflow_id":"w", "workflow_run_id":"wr", "node_id":"n",
            "workspace":{"base_ref":"integration"}},
        "ack_timeout_seconds":30, "timeout_seconds":60, "blocked_timeout_seconds":90,
        "schedule_paused":true, "last_run_at":"2024-01-01T00:00:00Z",
        "next_run_at":"2024-01-02T00:00:00Z"
    }))
    .unwrap();
    let original = serde_json::to_value(&task).unwrap();
    let assignment = agent::AssignedTask::try_from(&task).unwrap();
    assert_eq!(serde_json::to_value(&assignment).unwrap(), original);
    task.title = "edited after dispatch".into();
    task.status = task::TaskStatus::Done;
    task.error = None;
    assert_eq!(serde_json::to_value(&assignment).unwrap(), original);
    let restored: task::Task =
        serde_json::from_value(serde_json::to_value(assignment).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(restored).unwrap(), original);
}

#[test]
fn process_and_plan_paths_are_canonical_without_a_benchmark_type_in_l4() {
    let old: Option<&dyn factory_core::adapter::TaskStore> = None;
    let canonical: Option<&dyn factory_process::store::TaskStore> = old;
    assert!(canonical.is_none());
    let plan: factory_core::control_plan::ControlPlan =
        factory_process::control_plan::ControlPlan::default();
    assert_eq!(plan.steps.len(), 0);
    let usage: factory_core::usage::RunUsage =
        factory_process::usage::RunUsage::unknown("unknown", 1);
    assert_eq!(usage.state, factory_process::usage::UsageState::Unknown);
    let task: Option<factory_core::task::Task> = None;
    let _: Option<factory_process::task::Task> = task;
    let run: Option<factory_core::run::Run> = None;
    let _: Option<factory_process::run::Run> = run;
    let workflow: Option<factory_core::workflow::WorkflowDefinition> = None;
    let _: Option<factory_process::workflow::WorkflowDefinition> = workflow;
    let intake: Option<factory_core::intake::Intake> = None;
    let _: Option<factory_process::intake::Intake> = intake;
    let requirement: Option<factory_core::control_plan::Requirement> = None;
    let _: Option<factory_assurance::control_plan::Requirement> = requirement;
}

#[test]
fn benchmark_owner_decodes_the_same_legacy_wire_reference_after_l4_roundtrip() {
    use factory_core::bench::BenchOrigin;
    let origin = BenchOrigin {
        bench_run_id: "b".into(),
        case_id: "c".into(),
        agent: "shell".into(),
        attempt: 2,
    };
    let legacy = serde_json::to_value(&origin).unwrap();
    let reference: factory_process::origin::OriginRef = origin.clone().into();
    assert_eq!(serde_json::to_value(&reference).unwrap(), legacy);
    assert_eq!(BenchOrigin::try_from(&reference).unwrap(), origin);
    let unknown = factory_process::origin::OriginRef::opaque("unsupported-reference");
    assert!(BenchOrigin::try_from(&unknown).is_err());
}

#[test]
fn checks_quality_and_receipts_are_canonical_owner_types() {
    let evidence: factory_core::policy::Evidence = factory_assurance::checks::Evidence::default();
    let _: factory_assurance::checks::Evidence = evidence;
    let check: factory_core::policy::Check = factory_assurance::checks::Check::Sandbox;
    let _: factory_assurance::checks::Check = check;
    let receipt: Option<factory_core::policy::Attestation> = None;
    let _: Option<factory_kernel::Attestation> = receipt;
    let control: factory_core::policy::ControlRef =
        factory_kernel::ControlRef::new("cra", "art-14");
    let _: factory_kernel::ControlRef = control;
    let mark: Option<factory_core::reporting_clock::ClockMark> = None;
    let _: Option<factory_kernel::ClockMark> = mark;
    let quality: Option<factory_core::quality::QualityTree> = None;
    let _: Option<factory_assurance::quality::QualityTree> = quality;
    let metric: factory_core::metrics::MetricId = "scrap_rate".parse().unwrap();
    let _: factory_assurance::metrics::MetricId = metric;
    let budget: Option<factory_core::budget::PolicyInput> = None;
    let _: Option<factory_assurance::budget::PolicyInput> = budget;
    let old: factory_core::policy::ControlStatus = factory_assurance::checks::EvaluationResult {
        control: factory_kernel::ControlRef::new("cra", "a"),
        title: "a".into(),
        kind: factory_core::policy::Kind::Regulation,
        refs: vec![],
        status: factory_assurance::checks::Status::Open { reasons: vec![] },
    };
    assert_eq!(
        serde_json::to_value(old).unwrap(),
        json!({"control":"cra/a", "title":"a", "kind":"regulation", "status":"open", "reasons":[]})
    );
}

#[test]
fn legacy_quality_synthetic_control_api_only_adapts_the_l5_subjects() {
    let catalogue: factory_core::quality::QualityCatalogue = factory_assurance::quality::QualityCatalogue {
        profiles: std::collections::BTreeMap::from([("p".into(), serde_yaml_ng::from_str(
            "attributes:\n  - id: reliability\n    importance: H\n    difficulty: M\n    scenarios:\n      - id: gate\n        measure: {check: task, task: nightly, max_age: 1d}\n"
        ).unwrap())]), findings: vec![],
    };
    let (tree, _) = factory_core::quality::applicable(
        &catalogue,
        "demo",
        &[factory_core::quality::QualityLayer {
            scope: "demo".into(),
            profiles: vec!["p".into()],
        }],
    );
    let subjects = factory_assurance::quality::check_subjects(&tree);
    let old = factory_core::quality::applied_checks(&tree);
    assert_eq!(old.len(), subjects.len());
    assert_eq!(old[0].kind, factory_core::policy::Kind::Standard);
    assert!(old[0].remediation.is_none() && old[0].requires.is_empty());
    assert_eq!(old[0].control, subjects[0].control);
    assert_eq!(old[0].evidence, subjects[0].evidence);
    assert_eq!(old[0].max_age, subjects[0].max_age);
    assert_eq!(
        factory_core::policy::evaluate(&old, &Default::default(), chrono::Utc::now())[0]
            .status
            .kind(),
        factory_assurance::checks::StatusKind::Open
    );
}
