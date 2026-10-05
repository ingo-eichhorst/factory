//! Test the physical L5 preview without a daemon, Core or an upper owner.
use factory_assurance::{
    plan_service, quality_inputs,
    workflow_preview::{Provider, Read, Subject},
};
use factory_kernel::{
    FactProvider, FactoryError, Facts, FunctionaryRosterFact, Provide, Result, ScopeNode,
    WorkflowBlueprintFact, WorkflowBlueprintQuery, WorkflowPreviewFact, WorkflowTargetsFact, L3,
    L4, L6,
};
use factory_process::{workflow::WorkflowDefinition, workflow_blueprints::project};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
};

struct Roster {
    names: Mutex<Vec<String>>,
    calls: AtomicUsize,
}
impl FactProvider for Roster {
    type Level = L3;
}
#[async_trait::async_trait]
impl Provide<FunctionaryRosterFact> for Roster {
    type Query = String;
    type Value = FunctionaryRosterFact;
    type Error = FactoryError;
    async fn get(&self, scope: &String) -> Result<Self::Value> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(FunctionaryRosterFact {
            scope: scope.clone(),
            default_agent: Some("worker".into()),
            names: self.names.lock().unwrap().clone(),
        })
    }
}
struct Blueprints(WorkflowBlueprintFact);
impl FactProvider for Blueprints {
    type Level = L4;
}
#[async_trait::async_trait]
impl Provide<WorkflowBlueprintFact> for Blueprints {
    type Query = WorkflowBlueprintQuery;
    type Value = Vec<WorkflowBlueprintFact>;
    type Error = FactoryError;
    async fn get(&self, _: &Self::Query) -> Result<Self::Value> {
        Ok(vec![self.0.clone()])
    }
}
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let root = Self(
            std::env::temp_dir().join(format!("factory-preview-owner-{}", uuid::Uuid::new_v4())),
        );
        std::fs::create_dir_all(&root.0).unwrap();
        root
    }
    fn provider(&self, names: &[&str]) -> Provider<Roster> {
        Provider::new(
            plan_service::Provider::new(
                self.0.clone(),
                quality_inputs::Configuration {
                    scopes: vec![quality_inputs::ScopeConfiguration {
                        id: "id".into(),
                        scope: ScopeNode {
                            name: "demo".into(),
                            path: "projects/demo".into(),
                        },
                        layers: vec![],
                    }],
                },
            ),
            Roster {
                names: Mutex::new(names.iter().map(|name| (*name).into()).collect()),
                calls: AtomicUsize::new(0),
            },
            "daemon-default".into(),
        )
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn blueprint() -> WorkflowBlueprintFact {
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "id":"workflow", "name":"Author name", "description":"", "scope":"demo", "category":"feature",
        "nodes":[{"id":"work", "task":{"title":"work", "worktree":true}}], "edges":[],
        "revision":1, "created_at":"2026-10-05T12:00:00Z", "updated_at":"2026-10-05T12:00:00Z"
    })).unwrap();
    project(&definition).unwrap()
}
fn requirements(category: &str) -> plan_service::Read {
    plan_service::Read {
        scope: "demo".into(),
        category: category.into(),
        policy: vec![factory_assurance::control_plan::PlanSource {
            source: "house/checked".into(),
            not_applicable: None,
            requires: serde_json::from_value(json!([
                {"applies_to":["*"], "step":"tests", "gate":"true"},
                {"applies_to":["*"], "step":"review", "by":"independent"},
                {"applies_to":["*"], "step":"approval", "by":"person"}
            ]))
            .unwrap(),
        }],
    }
}

#[tokio::test]
async fn live_l5_preview_compiles_injects_binds_and_reports_without_an_upper_report() {
    let root = Root::new();
    let provider = root.provider(&["worker", "critic", "third"]);
    let facts = Facts::<L6>::new();
    let blueprint = blueprint();
    let targets = facts
        .get::<WorkflowTargetsFact, _>(&provider, &blueprint)
        .await
        .unwrap();
    assert_eq!(targets.categories, ["feature"]);
    let read = Read {
        blueprint,
        requirements: vec![Ok(requirements("feature"))],
    };
    let first = facts
        .get::<WorkflowPreviewFact, _>(&provider, &read)
        .await
        .unwrap();
    assert_eq!(first.enforcement.len(), 3);
    assert_eq!(
        first
            .enforcement
            .iter()
            .find(|row| row.step == "review")
            .unwrap()
            .actor
            .as_deref(),
        Some("critic")
    );
    assert_eq!(
        first
            .enforcement
            .iter()
            .find(|row| row.step == "approval")
            .unwrap()
            .actor
            .as_deref(),
        Some("owner")
    );
    assert!(first
        .enforcement
        .iter()
        .all(|row| row.name == "Author name" && row.required_by == ["house/checked"]));
    assert!(first.findings.is_empty());
    // Next provider read sees the current declaration set, not cached actors.
    let missing = root.provider(&["worker"]);
    let next = facts
        .get::<WorkflowPreviewFact, _>(&missing, &read)
        .await
        .unwrap();
    assert_eq!(
        next.enforcement
            .iter()
            .find(|row| row.step == "review")
            .unwrap()
            .actor,
        None
    );
    assert!(next.findings[0]
        .detail
        .contains("has no independent functionary"));
}

#[tokio::test]
async fn task_category_override_matches_before_implicit_and_bare_scope_uses_canonical_plan_scope() {
    let root = Root::new();
    let provider = root.provider(&["worker", "critic"]);
    let blueprints = Blueprints(blueprint());
    let prepared = provider
        .prepare(
            Subject {
                workflow: None,
                task: Some("task".into()),
                scope: None,
                category: Some("release".into()),
            },
            &blueprints,
        )
        .await
        .unwrap();
    assert_eq!(prepared.categories(), ["release"]);
    let lint = provider
        .finish(prepared, vec![Ok(requirements("release"))])
        .await
        .unwrap();
    let definition = lint.injected.unwrap();
    assert_eq!(definition.category.as_deref(), Some("release"));
    assert_eq!(
        definition
            .nodes
            .iter()
            .find(|node| node.id == "work")
            .unwrap()
            .task
            .category
            .as_deref(),
        Some("release")
    );
    let bare = provider
        .prepare(
            Subject {
                workflow: None,
                task: None,
                scope: Some("projects/demo".into()),
                category: None,
            },
            &blueprints,
        )
        .await
        .unwrap();
    let mut read = requirements("default");
    read.policy.clear();
    let lint = provider.finish(bare, vec![Ok(read)]).await.unwrap();
    assert_eq!(lint.scope, "demo");
    assert!(lint.injected.is_none());
}

#[tokio::test]
async fn blueprint_validation_precedes_deferred_authored_errors_and_quality_precedes_later_categories(
) {
    let root = Root::new();
    let provider = root.provider(&["worker"]);
    let facts = Facts::<L6>::new();
    let mut invalid = blueprint();
    invalid.blueprint.nodes[0].id = String::new();
    let read = Read {
        blueprint: invalid,
        requirements: vec![Err(FactoryError::BadRequest("late authored error".into()))],
    };
    let error = facts
        .get::<WorkflowPreviewFact, _>(&provider, &read)
        .await
        .unwrap_err()
        .to_string();
    assert!(!error.contains("late authored error"), "{error}");
    let mut first = requirements("feature");
    first.scope = "missing".into();
    let read = Read {
        blueprint: blueprint(),
        requirements: vec![
            Ok(first),
            Err(FactoryError::BadRequest("later category".into())),
        ],
    };
    assert_eq!(
        facts
            .get::<WorkflowPreviewFact, _>(&provider, &read)
            .await
            .unwrap_err()
            .code(),
        "no_such_scope"
    );
    let read = Read {
        blueprint: blueprint(),
        requirements: vec![Err(FactoryError::BadRequest("authored failure".into()))],
    };
    assert_eq!(
        facts
            .get::<WorkflowPreviewFact, _>(&provider, &read)
            .await
            .unwrap_err()
            .to_string(),
        FactoryError::BadRequest("authored failure".into()).to_string()
    );
}

#[tokio::test]
async fn part_targets_and_preview_use_copied_nodes_and_original_authored_names() {
    let root = Root::new();
    let provider = root.provider(&["worker", "critic"]);
    let facts = Facts::<L6>::new();
    let mut blueprint = blueprint();
    blueprint.blueprint.part = Some(Default::default());
    let targets = facts
        .get::<WorkflowTargetsFact, _>(&provider, &blueprint)
        .await
        .unwrap();
    assert_eq!(targets.categories, ["feature"]);
    let read = Read {
        blueprint,
        requirements: vec![Ok(requirements("feature"))],
    };
    let preview = facts
        .get::<WorkflowPreviewFact, _>(&provider, &read)
        .await
        .unwrap();
    assert_eq!(preview.enforcement.len(), 6);
    assert!(preview
        .enforcement
        .iter()
        .all(|row| ["a-work", "b-work"].contains(&row.node.as_str()) && row.name == "Author name"));
}
