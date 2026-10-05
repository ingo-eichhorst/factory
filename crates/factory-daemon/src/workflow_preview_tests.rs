//! Outside-stack acceptance using the actual physical L3/L4/L5/L6 owners.
use factory_kernel::{Facts, WorkflowBlueprintFact, WorkflowBlueprintQuery, L6};
use factory_plugins::SqliteStore;
use factory_process::{
    store::{task_from_new, TaskStore},
    task::NewTask,
    workflow::{WorkflowDefinition, WorkflowDraft},
    workflow_blueprints::{self, Provider as Blueprints},
    workflow_store::WorkflowStore,
};
use serde_json::json;

struct Fixture {
    root: std::path::PathBuf,
    tasks: SqliteStore,
    workflows: WorkflowStore,
    receipts: factory_direction::policy_store::PolicyStore,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("factory-workflow-preview-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let db = root.join("instance.sqlite");
        Self {
            tasks: SqliteStore::open(&db).unwrap(),
            workflows: WorkflowStore::open(&db).unwrap(),
            receipts: factory_direction::policy_store::PolicyStore::open(&db).unwrap(),
            root,
        }
    }
    fn blueprints(&self) -> Blueprints<'_> {
        Blueprints {
            tasks: &self.tasks,
            workflows: &self.workflows,
        }
    }
    fn direction(&self) -> factory_direction::policy_service::Service<'_> {
        use factory_direction::{
            policy_intent::{Configuration, Scope, Service},
            policy_service,
        };
        policy_service::Service::new(Service::new(
            self.root.clone(),
            Configuration {
                scopes: vec![Scope {
                    id: "demo-id".into(),
                    name: "demo".into(),
                    path: "projects/demo".into(),
                    policies: serde_yaml_ng::from_str("frameworks: [house]").unwrap(),
                }],
                root_policies: Default::default(),
                root_name: None,
                instance_name: "instance".into(),
            },
            &self.receipts,
        ))
    }
    fn preview(
        &self,
    ) -> factory_assurance::workflow_preview::Provider<factory_agents::roster::Provider> {
        use factory_agents::roster::{Provider, RosterScope};
        use factory_assurance::{plan_service, quality_inputs, workflow_preview};
        workflow_preview::Provider::new(
            plan_service::Provider::new(
                self.root.clone(),
                quality_inputs::Configuration {
                    scopes: vec![quality_inputs::ScopeConfiguration {
                        id: "demo-id".into(),
                        scope: factory_kernel::ScopeNode {
                            name: "demo".into(),
                            path: "projects/demo".into(),
                        },
                        layers: vec![],
                    }],
                },
            ),
            Provider {
                scopes: vec![RosterScope {
                    name: "demo".into(),
                    path: "projects/demo".into(),
                    agent: Some(serde_yaml_ng::from_str("shell").unwrap()),
                    agents: vec![serde_yaml_ng::from_str(
                        "name: critic\nharness: shell\nargs: [private-fixture]",
                    )
                    .unwrap()],
                    roles: Default::default(),
                }],
                root_roles: Default::default(),
                foreman: Default::default(),
            },
            "shell".into(),
        )
    }
    fn catalogue(&self, step: &str) {
        let directory = factory_direction::policy::policies_dir(&self.root);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("house.yaml"), format!(
            "framework: house\ntitle: House\nkind: best-practice\ncontrols:\n  - id: tested\n    title: Tested\n    requires:\n      - {{applies_to: [feature], step: {step}, gate: 'true'}}\n      - {{applies_to: [feature], step: review, by: independent}}\n")).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}
fn definition(name: &str, scope: &str) -> WorkflowDefinition {
    WorkflowDefinition::from_draft(WorkflowDraft {
        name: name.into(),
        scope: scope.into(),
        category: Some("feature".into()),
        nodes: vec![serde_json::from_value(
            json!({"id":"work", "task":{"title":"Work", "worktree":true}}),
        )
        .unwrap()],
        ..Default::default()
    })
}

#[tokio::test]
async fn physical_l4_blueprints_are_live_normalized_and_keep_legacy_store_selection_errors() {
    let fixture = Fixture::new();
    let provider = fixture.blueprints();
    let facts = Facts::<L6>::new();
    let mut authored = definition("Before", "demo");
    fixture.workflows.put_definition(&authored).await.unwrap();
    let before = facts
        .get::<WorkflowBlueprintFact, _>(&provider, &WorkflowBlueprintQuery::All)
        .await
        .unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].blueprint.name, "Before");
    assert_eq!(before[0].categories, ["feature"]);
    authored.name = "After".into();
    authored.category = Some("release".into());
    fixture.workflows.put_definition(&authored).await.unwrap();
    let next = facts
        .get::<WorkflowBlueprintFact, _>(
            &provider,
            &WorkflowBlueprintQuery::Workflow(authored.id.clone()),
        )
        .await
        .unwrap();
    assert_eq!(next[0].blueprint.name, "After");
    assert_eq!(next[0].categories, ["release"]);
    let task = task_from_new(
        NewTask {
            title: "Implicit".into(),
            category: Some("bugfix".into()),
            ..Default::default()
        },
        "demo".into(),
        "shell".into(),
        "herdr".into(),
    );
    fixture.tasks.create(&task).await.unwrap();
    let implicit = facts
        .get::<WorkflowBlueprintFact, _>(&provider, &WorkflowBlueprintQuery::Task(task.id.clone()))
        .await
        .unwrap();
    assert_eq!(implicit[0].blueprint.id, format!("task:{}", task.id));
    assert_eq!(implicit[0].categories, ["bugfix"]);
    let task_wire = serde_json::to_value(&implicit[0]).unwrap();
    assert!(task_wire["blueprint"]["nodes"][0]["task"]
        .get("status")
        .is_none());
    assert_eq!(
        facts
            .get::<WorkflowBlueprintFact, _>(
                &provider,
                &WorkflowBlueprintQuery::Task("missing".into())
            )
            .await
            .unwrap_err()
            .code(),
        "task_not_found"
    );
    fixture
        .workflows
        .delete_definition(&authored.id)
        .await
        .unwrap();
    let missing = facts
        .get::<WorkflowBlueprintFact, _>(&provider, &WorkflowBlueprintQuery::Workflow(authored.id))
        .await
        .unwrap_err();
    assert!(missing.to_string().contains("no such workflow:"));
}

#[tokio::test]
async fn physical_l6_selects_and_folds_actual_l5_previews_rereading_live_authored_files() {
    let fixture = Fixture::new();
    fixture.catalogue("tests");
    let valid = definition("Valid", "demo");
    let sibling = definition("Sibling", "elsewhere");
    let mut invalid = definition("Invalid", "demo");
    invalid.nodes[0].id.clear();
    for workflow in [&valid, &invalid, &sibling] {
        fixture.workflows.put_definition(workflow).await.unwrap();
    }
    let service = fixture.direction();
    let blueprints = fixture.blueprints();
    let preview = fixture.preview();
    let (first, failures) = service
        .workflow_enforcement(Some("projects/demo"), &blueprints, &preview)
        .await
        .unwrap();
    assert_eq!(first.len(), 2);
    assert!(first
        .iter()
        .all(|row| row.workflow == valid.id && row.scope == "demo"));
    assert_eq!(
        first
            .iter()
            .find(|row| row.step == "review")
            .unwrap()
            .actor
            .as_deref(),
        Some("critic")
    );
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].workflow, invalid.id);
    assert_eq!(failures[0].name, "Invalid");
    fixture.catalogue("scan");
    let (next, _) = service
        .workflow_enforcement(Some("demo"), &blueprints, &preview)
        .await
        .unwrap();
    assert!(next.iter().any(|row| row.step == "scan"));
    assert!(!next.iter().any(|row| row.step == "tests"));
    // A broken workflow-store read is fatal, not an apparently empty board.
    rusqlite::Connection::open(fixture.root.join("instance.sqlite"))
        .unwrap()
        .execute("DROP TABLE workflow_definitions", [])
        .unwrap();
    assert!(service
        .workflow_enforcement(Some("demo"), &blueprints, &preview)
        .await
        .is_err());
}

#[test]
fn blueprint_projection_preserves_every_authored_field_and_normalizes_old_rework_rows() {
    let raw = json!({"id":"w", "name":"Full", "description":"Description", "scope":"demo",
        "workspace_ref":"abc", "category":"release", "inputs":[{"name":"branch", "description":"Branch"}],
        "part":{"entry":"work", "deliverable":"work", "terminal":"work"},
        "nodes":[{"id":"work", "session":"fresh", "position":{"x":1.5,"y":2.5}, "kind":"task",
            "task":{"title":"Title", "instructions":"Instructions", "scope":"demo", "agent":"worker", "runtime":"herdr",
                "parent_task_id":"parent", "decomposition_part":"part", "depends_on":["dependency"],
                "schedule":{"cron":{"expr":"0 9 * * 1", "timezone":"Europe/Berlin"}}, "after":["other"], "after_condition":"done",
                "estimate_seconds":4, "estimate":{"time":{"low":1,"expected":2,"high":3},"cost":{"low":0.1,"expected":0.2,"high":0.3}},
                "ack_timeout_seconds":5, "timeout_seconds":6, "blocked_timeout_seconds":7, "labels":{"k":"v"},
                "worktree":false, "knowledge_hints":true, "retry":{"backoff":{"max_attempts":2,"backoff_seconds":8}}, "category":"feature"},
            "gate":{"step":"review", "command":"true", "timeout_seconds":2,"subject":"work", "required_by":["house/a"],"locked":true,"by":"independent","actor":"critic"},
            "rework":{"to":"prior","max_rounds":4},
            "expand":{"join":{"tolerate":1},"cancel":"abandon","children":["child"],"max_rework_rounds":7}}],
        "edges":[{"id":"edge","from":"prior","to":"work"}], "revision":7,
        "created_at":"2026-10-05T00:00:00Z", "updated_at":"2026-10-05T01:00:00Z"});
    let domain: WorkflowDefinition = serde_json::from_value(raw).unwrap();
    let expected = serde_json::to_value(&domain).unwrap();
    let projected = workflow_blueprints::project(&domain).unwrap();
    assert_eq!(
        serde_json::to_value(&projected.blueprint).unwrap(),
        expected
    );
    assert_eq!(
        serde_json::to_value(workflow_blueprints::definition(&projected).unwrap()).unwrap(),
        expected
    );
    assert_eq!(expected["nodes"][0]["task"].as_object().unwrap().len(), 21);
    assert!(expected["nodes"][0].get("rework").is_none());
    assert_eq!(expected["nodes"][0]["exits"][0]["to"], "prior");
    // Guard additions too: a new authored field must enter the typed snapshot,
    // not silently disappear in serde's otherwise permissive projection.
    fn fields(source: &str, name: &str) -> Vec<String> {
        let body = source
            .split(&format!("pub struct {name} {{"))
            .nth(1)
            .unwrap()
            .split("\n}")
            .next()
            .unwrap();
        body.lines()
            .filter_map(|line| {
                line.trim()
                    .strip_prefix("pub ")
                    .and_then(|field| field.split_once(':'))
                    .map(|(field, _)| field.to_string())
            })
            .collect()
    }
    let snapshots = include_str!("../../factory-kernel/src/workflow_blueprint.rs");
    let workflow = include_str!("../../factory-process/src/workflow.rs");
    for (domain, snapshot) in [
        ("WorkflowDefinition", "WorkflowBlueprint"),
        ("WorkflowNode", "BlueprintNode"),
        ("WorkflowInput", "BlueprintInput"),
        ("WorkflowExit", "BlueprintExit"),
        ("GateSpec", "BlueprintGate"),
        ("CanvasPoint", "BlueprintPoint"),
        ("WorkflowEdge", "BlueprintEdge"),
        ("PartSpec", "BlueprintPart"),
        ("ExpandSpec", "BlueprintExpandSpec"),
        ("ExpandJoin", "BlueprintExpandJoin"),
    ] {
        assert_eq!(
            fields(workflow, domain),
            fields(snapshots, snapshot),
            "{domain}"
        );
    }
    let tasks = include_str!("../../factory-process/src/task.rs");
    for (domain, snapshot) in [
        ("NewTask", "BlueprintTaskSpec"),
        ("Estimate", "BlueprintEstimate"),
        ("TimeEstimateRange", "BlueprintTimeEstimate"),
        ("CostEstimateRange", "BlueprintCostEstimate"),
    ] {
        assert_eq!(
            fields(tasks, domain),
            fields(snapshots, snapshot),
            "{domain}"
        );
    }
}

#[test]
fn live_preview_and_plan_owners_do_not_reenter_engine_or_an_upper_report() {
    let preview: String = include_str!("../../factory-assurance/src/workflow_preview.rs")
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    assert!(preview.contains("Facts::<L5>::new().get::<FunctionaryRosterFact"));
    assert!(preview.contains("Facts::<L5>::new().get::<WorkflowBlueprintFact"));
    assert!(!preview.contains("factory_core"));
    assert!(!preview.contains("factory_direction"));
    let direction = include_str!("../../factory-direction/src/policy_service.rs");
    assert!(direction.contains("get::<WorkflowTargetsFact"));
    assert!(direction.contains("get::<WorkflowPreviewFact"));
    let wiring = include_str!("verification.rs");
    let lint = wiring
        .split("pub(crate) async fn workflow_lint(")
        .nth(1)
        .unwrap()
        .split("\n}\n\n#[cfg(test)]")
        .next()
        .unwrap();
    for escaped in [
        ".validate()",
        ".inject(",
        ".ordering_violations(",
        ".workflows.",
        "self.require(",
    ] {
        assert!(
            !lint.contains(escaped),
            "preview judgement leaked outside: {escaped}"
        );
    }
}
