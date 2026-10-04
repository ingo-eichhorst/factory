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
