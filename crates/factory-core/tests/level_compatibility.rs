//! Public core paths remain the identical owning-crate types, not copies.
use factory_core::{adapter::agent, config, error, task};
use factory_kernel::{LaunchKind, LaunchSpec, Schedule, Span};
use serde_json::json;

#[test]
fn metric_registry_wire_view_and_counts_are_identical_l5_owner_types() {
    let definition = factory_assurance::metrics::resolve(
        &factory_assurance::metrics::MetricId::new("fail_rate").unwrap(),
    )
    .unwrap();
    let old: factory_core::protocol::MetricDefView = definition.into();
    let canonical: factory_assurance::metrics::MetricDefView = old;
    let wire = serde_json::to_value(&canonical).unwrap();
    assert_eq!(wire["id"], "fail_rate");
    assert_eq!(wire["unit"], "ratio");
    assert_eq!(wire["better"], "lower");
    assert!(wire["description"].as_str().unwrap().contains("failed"));
    let old: factory_core::policy::StatusCounts =
        factory_assurance::evaluation_rollup::StatusCounts {
            satisfied: 2,
            attested: 1,
            stale: 3,
            open: 4,
            not_applicable: 5,
        };
    let canonical: factory_assurance::evaluation_rollup::StatusCounts = old;
    assert_eq!(
        serde_json::to_value(canonical).unwrap(),
        json!({
            "satisfied":2,"attested":1,"stale":3,"open":4,"not_applicable":5,
        })
    );
}

#[test]
fn roster_declarations_and_sandbox_are_canonical_owners_with_unchanged_wire_forms() {
    let legacy: config::AgentRef = serde_yaml_ng::from_str(
        "harness: pi\nname: worker\nargs: [--fixture]\nmax_sessions: 2"
    ).unwrap();
    let canonical: factory_agents::roster::AgentRef = legacy;
    assert_eq!(serde_json::to_value(&canonical).unwrap(),
        json!({"harness":"pi","name":"worker","args":["--fixture"],"max_sessions":2}));
    let legacy: config::ScopeAgent = serde_yaml_ng::from_str("harness: shell").unwrap();
    let canonical: factory_agents::roster::ScopeAgent = legacy;
    assert_eq!(serde_json::to_value(canonical).unwrap(),
        json!({"harness":"shell","lifetime":"task","role":"worker"}));
    let legacy: config::ForemanConfig = config::ForemanConfig::default();
    let canonical: factory_agents::roster::ForemanConfig = legacy;
    assert_eq!(serde_json::to_value(canonical).unwrap(),
        json!({"enabled":false,"name":"foreman","exclude":["root"]}));
    for name in ["none", "docker", "srt", "openshell"] {
        let legacy: config::Sandbox = serde_json::from_value(json!(name)).unwrap();
        let canonical: factory_environment::sandbox::Sandbox = legacy;
        assert_eq!(serde_json::to_value(canonical).unwrap(), json!(name));
        assert_eq!(canonical.is_enforced(), name == "openshell");
    }
}

#[test]
fn credential_metadata_is_canonical_l2_data_and_preserves_legacy_json() {
    let row: factory_core::protocol::CredentialRow =
        factory_environment::credentials::CredentialRow {
            label: "demo .env".into(),
            path: "/scratch/demo/.env".into(),
            integration: "scope env".into(),
            present: false,
            scope: Some("demo".into()),
        };
    assert_eq!(
        serde_json::to_value(&row).unwrap(),
        json!({
            "label":"demo .env","path":"/scratch/demo/.env","integration":"scope env",
            "present":false,"scope":"demo"
        })
    );
    let _: factory_environment::credentials::CredentialRow = row;
    let usage: factory_core::protocol::SecretUse =
        factory_environment::credential_expiry::SecretUse {
            scope: "demo".into(),
            agent: "curator".into(),
            provider: "provider-id".into(),
        };
    assert_eq!(
        serde_json::to_value(usage).unwrap(),
        json!({"scope":"demo","agent":"curator","provider":"provider-id"})
    );
}

#[test]
fn native_host_and_configured_interfaces_are_canonical_l1_data_with_legacy_json() {
    let config: factory_core::config::InterfaceConfig =
        serde_yaml_ng::from_str("kind: http").unwrap();
    let canonical: factory_infrastructure::interfaces::InterfaceConfig = config;
    assert_eq!(
        canonical.http_bind(),
        factory_core::config::DEFAULT_HTTP_BIND
    );
    let facts: Vec<factory_core::protocol::InterfaceFacts> =
        factory_infrastructure::interfaces::interface_facts(&[
            canonical,
            serde_yaml_ng::from_str("kind: cli").unwrap(),
        ]);
    assert_eq!(
        serde_json::to_value(facts).unwrap(),
        json!([{"kind":"http","bind":"127.0.0.1:8787"},{"kind":"cli"}])
    );
    let old: Option<factory_core::protocol::HostFacts> = None;
    let _: Option<factory_infrastructure::host::HostFacts> = old;
    let old: Option<factory_core::protocol::DiskFacts> = None;
    let _: Option<factory_infrastructure::host::DiskFacts> = old;
}

#[test]
fn interface_seam_context_and_observer_stream_are_canonical_outside_stack_types() {
    let old: Option<&dyn factory_core::adapter::Interface<()>> = None;
    let canonical: Option<&dyn factory_interfaces::Interface<()>> = old;
    assert!(canonical.is_none());
    let context: Option<factory_core::adapter::InterfaceContext> = None;
    let _: Option<factory_interfaces::InterfaceContext> = context;
    let bus: factory_core::EventBus = factory_interfaces::EventBus::new(8);
    let _: factory_interfaces::EventBus = bus;
    let event: factory_core::Event = factory_interfaces::Event::TaskDeleted { id: "t".into() };
    assert_eq!(
        serde_json::to_value(event).unwrap(),
        json!({"type":"task_deleted", "id":"t"})
    );
    let envelope: factory_core::protocol::Envelope =
        factory_interfaces::protocol::Request::Status.into();
    let _: factory_interfaces::protocol::Envelope = envelope;
    let response: factory_core::protocol::Response =
        factory_interfaces::protocol::Response::error("denied", "No grant");
    let _: factory_interfaces::protocol::Response = response;
}

#[test]
fn config_and_page_projection_paths_are_canonical_outside_stack_types() {
    let config: Option<factory_core::Config> = None;
    let _: Option<factory_composition::config::Config> = config;
    let factory: Option<factory_core::Factory> = None;
    let _: Option<factory_composition::config::Factory> = factory;
    let scope: Option<factory_core::Scope> = None;
    let _: Option<factory_composition::config::Scope> = scope;
    let dashboard: Option<factory_core::dashboard::DashboardConfig> = None;
    let _: Option<factory_composition::dashboard::DashboardConfig> = dashboard;
    let activity: Option<factory_core::building::Activity> = None;
    let _: Option<factory_composition::building::Activity> = activity;
    let operations: Option<factory_core::operations::OperationsReport> = None;
    let _: Option<factory_composition::operations::OperationsReport> = operations;
}

#[test]
fn direction_domains_and_goals_store_keep_their_canonical_core_paths() {
    let policy: Option<factory_core::policy::Catalogue> = None;
    let _: Option<factory_direction::policy::Catalogue> = policy;
    let cycle: Option<factory_core::goals::Cycle> = None;
    let _: Option<factory_direction::goals::Cycle> = cycle;
    let scenario: Option<factory_core::scenario::Scenario> = None;
    let _: Option<factory_direction::scenario::Scenario> = scenario;
    let budget: Option<factory_core::budget::Catalogue> = None;
    let _: Option<factory_direction::budget::Catalogue> = budget;
    let clock: Option<factory_core::reporting_clock::ReportingClock> = None;
    let _: Option<factory_direction::reporting_clock::ReportingClock> = clock;
    let export: Option<factory_core::policy_export::PolicyExport> = None;
    let _: Option<factory_direction::policy_export::PolicyExport> = export;
    let store: Option<factory_core::goals_store::GoalsStore> = None;
    let _: Option<factory_direction::goals_store::GoalsStore> = store;
}

#[test]
fn operations_run_arithmetic_is_canonical_process_behavior_and_preserves_json() {
    let figure: factory_core::operations::Figure =
        factory_process::operations::Figure::of(None, 0, "no evidence");
    let owned: factory_process::operations::Figure = figure;
    assert_eq!(
        serde_json::to_value(owned).unwrap(),
        json!({"value":null,"samples":0,"reason":"no evidence"})
    );
    let old: Option<factory_core::operations::AgePercentiles> = None;
    let _: Option<factory_process::operations::AgePercentiles> = old;
    let old: Option<factory_core::operations::Pace> = None;
    let _: Option<factory_process::operations::Pace> = old;
    let window: factory_core::operations::Window =
        factory_process::window::Window::trailing("2026-09-25T12:00:00Z".parse().unwrap(), 28);
    let _: factory_process::operations::Window = window;
    assert_eq!(
        factory_core::operations::registry_metric as *const (),
        factory_process::operations::registry_metric as *const ()
    );
    assert_eq!(
        factory_composition::operations::registry_metric_as_of as *const (),
        factory_process::operations::registry_metric_as_of as *const ()
    );
}

#[test]
fn wire_policy_report_is_the_l6_report_with_identical_legacy_json() {
    let wire = json!({
        "scope":"demo", "rows":[{
            "scope":"demo", "statuses":[{
                "control":"cra/a", "title":"A", "kind":"regulation",
                "status":"open", "reasons":["Missing evidence"]
            }], "rollup":[]
        }], "rollup":[], "not_applicable":[], "findings":[], "catalogues":[]
    });
    let old: factory_core::protocol::PolicyReport = serde_json::from_value(wire.clone()).unwrap();
    let canonical: factory_direction::policy_report::PolicyReport = old;
    assert_eq!(serde_json::to_value(&canonical).unwrap(), wire);
    let response = factory_core::protocol::Payload::Policy { report: canonical };
    let round_trip: factory_core::protocol::Payload =
        serde_json::from_value(serde_json::to_value(response).unwrap()).unwrap();
    assert!(matches!(
        round_trip,
        factory_core::protocol::Payload::Policy { .. }
    ));
}

#[test]
fn compatibility_values_are_the_canonical_kernel_types() {
    let launch: agent::LaunchSpec = LaunchSpec {
        kind: LaunchKind::Named("shell".into()),
        args: vec![],
        env: Default::default(),
        agent_kind: None,
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
        ..Default::default()
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

#[test]
fn benchmark_knowledge_and_provider_paths_are_the_canonical_l5_owners() {
    let dataset: Option<factory_core::dataset::Dataset> = None;
    let _: Option<factory_assurance::dataset::Dataset> = dataset;
    let origin: Option<factory_core::bench::BenchOrigin> = None;
    let _: Option<factory_assurance::bench::BenchOrigin> = origin;
    let config: Option<factory_core::benchmark::Configuration> = None;
    let _: Option<factory_assurance::benchmark::Configuration> = config;
    let index: Option<factory_core::knowledge::Index> = None;
    let _: Option<factory_assurance::knowledge::Index> = index;
    let provider: Option<&dyn factory_core::adapter::KnowledgeProvider> = None;
    let canonical: Option<&dyn factory_assurance::knowledge_provider::KnowledgeProvider> = provider;
    assert!(canonical.is_none());
    let query: factory_core::adapter::KnowledgeQuery =
        factory_assurance::knowledge_provider::KnowledgeQuery {
            text: "question".into(),
            tags: vec![],
            scope: None,
            limit: 10,
        };
    assert_eq!(
        serde_json::to_value(query).unwrap(),
        json!({"text":"question", "limit":10})
    );
    let store: Option<factory_core::bench_store::BenchStore> = None;
    let _: Option<factory_assurance::bench_store::BenchStore> = store;
}

#[test]
fn spend_query_is_canonical_plain_kernel_vocabulary_with_unchanged_legacy_json() {
    let query = factory_kernel::SpendQuery {
        scope: Some("work".into()),
        group_by: factory_kernel::CostGroupBy::Scope,
        ..Default::default()
    };
    let old: factory_core::usage::SpendQuery = query.clone();
    let process: factory_process::usage::SpendQuery = old.clone();
    assert_eq!(process, query);
    assert_eq!(
        serde_json::to_value(old).unwrap(),
        json!({"basis":"started", "scope":"work", "group_by":"scope"})
    );
    let empty: factory_kernel::SpendQuery = serde_json::from_value(json!({})).unwrap();
    assert_eq!(empty.basis, factory_kernel::SpendBasis::Started);
    assert_eq!(empty.group_by, factory_kernel::CostGroupBy::Task);
    assert!(empty.scope.is_none() && empty.from.is_none() && empty.to.is_none());
}

#[test]
fn metric_identity_values_and_goals_views_are_canonical_with_the_original_wire_shape() {
    let id = factory_kernel::MetricId::new("compliance.cra").unwrap();
    let legacy: factory_core::metrics::MetricId = id.clone();
    let owned: factory_assurance::metrics::MetricId = legacy;
    assert_eq!(owned, id);
    let value = factory_kernel::MetricValue {
        id,
        value: None,
        as_of: "2026-10-16T12:00:00Z".parse().unwrap(),
        reason: Some("no catalogue".into()),
    };
    let legacy: factory_core::metrics::MetricValue = value.clone();
    assert_eq!(
        serde_json::to_value(legacy).unwrap(),
        json!({"id": "compliance.cra", "value": null, "as_of": "2026-10-16T12:00:00Z", "reason": "no catalogue"})
    );
    let report: Option<factory_direction::goals_view::GoalsReport> = None;
    let report: Option<factory_core::protocol::GoalsReport> = report;
    let _: Option<factory_interfaces::protocol::GoalsReport> = report;
    assert!(factory_kernel::MetricId::new("invalid..id").is_err());
}

#[test]
fn policy_declaration_is_canonical_l6_data_with_the_original_config_json() {
    let owned: factory_direction::policy_intent::PolicyDeclaration =
        serde_json::from_value(json!({
            "frameworks": ["cra"], "tighten": {"cra/a": {"max_age": "7d"}},
            "not_applicable": [{"control": "cra/b", "rationale": "not used"}]
        }))
        .unwrap();
    let legacy: factory_core::config::PolicyDeclaration = owned.clone();
    let outside: factory_composition::config::PolicyDeclaration = legacy;
    assert_eq!(outside, owned);
    assert_eq!(
        serde_json::to_value(outside).unwrap(),
        json!({
            "frameworks": ["cra"], "tighten": {"cra/a": {"max_age": "1w"}},
            "not_applicable": [{"control": "cra/b", "rationale": "not used"}]
        })
    );
    assert!(
        serde_json::from_value::<factory_core::config::PolicyDeclaration>(
            json!({"framework": ["cra"]})
        )
        .is_err()
    );
}

#[test]
fn check_results_are_canonical_kernel_data_with_unchanged_policy_wire_fields() {
    let wire = json!({"control": "cra/a", "title": "A", "kind": "regulation",
        "status": "stale", "reasons": ["expired"], "refs": [{"kind": "attestation", "id": "receipt"}]});
    let owned: factory_kernel::EvaluationResult<factory_direction::policy::Kind> =
        serde_json::from_value(wire.clone()).unwrap();
    let assurance: factory_assurance::checks::EvaluationResult<factory_direction::policy::Kind> =
        owned;
    let legacy: factory_core::policy::ControlStatus = assurance;
    assert_eq!(serde_json::to_value(&legacy).unwrap(), wire);
    let status: factory_kernel::Status = legacy.status;
    let _: factory_core::policy::Status = status;
    let reference: factory_kernel::EvidenceRef = legacy.refs[0].clone();
    let _: factory_assurance::checks::EvidenceRef = reference;
    let finding: factory_assurance::checks::EvidenceFinding = factory_kernel::EvidenceFinding {
        subject: "demo".into(),
        detail: "ambiguous".into(),
    };
    assert_eq!(finding.detail, "ambiguous");
}
