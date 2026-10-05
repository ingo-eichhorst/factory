use super::*;
use crate::role::{Grant, RoleOrigin};
use factory_environment::sandbox::Sandbox;
use factory_kernel::{Facts, People, Provide, L6};
use std::collections::BTreeSet;

fn scope(name: &str, path: &str) -> RosterScope {
    RosterScope {
        name: name.into(),
        path: path.into(),
        agent: None,
        agents: vec![
            serde_yaml_ng::from_str("name: critic\nharness: shell\nrole: reviewer").unwrap(),
        ],
        roles: BTreeMap::new(),
    }
}
fn spec(grants: &[&str]) -> RoleSpec {
    RoleSpec {
        grants: grants.iter().map(|g| (*g).into()).collect(),
        ..Default::default()
    }
}
fn provider(scopes: Vec<RosterScope>) -> Provider {
    Provider {
        scopes,
        root_roles: BTreeMap::from([("reviewer".into(), spec(&["task.report"]))]),
        foreman: ForemanConfig::default(),
    }
}

#[tokio::test]
async fn functionary_fact_keeps_raw_default_and_declaration_order_without_arguments_or_grants() {
    use factory_kernel::{FunctionaryRosterFact, L5};
    let mut declared = scope("team/demo", "projects/demo");
    declared.agent = Some(serde_yaml_ng::from_str(
        "harness: pi\nname: worker\nargs: [private-fixture-argument]\nrole: reviewer"
    ).unwrap());
    declared.agents.push(serde_yaml_ng::from_str("name: third\nharness: shell").unwrap());
    let mut provider = provider(vec![declared]);
    provider.foreman.enabled = true;
    let facts = Facts::<L5>::new();
    let first = facts.get::<FunctionaryRosterFact, _>(&provider, &"demo".into()).await.unwrap();
    assert_eq!(first.scope, "team/demo");
    assert_eq!(first.default_agent.as_deref(), Some("pi"));
    assert_eq!(first.names, ["worker", "critic", "third", "foreman"]);
    let value = serde_json::to_value(&first).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 3);
    assert!(!value.to_string().contains("private-fixture-argument"));
    provider.scopes[0].agents.remove(0);
    provider.foreman.exclude = vec!["demo".into()];
    let next = facts.get::<FunctionaryRosterFact, _>(&provider, &"projects/demo".into()).await.unwrap();
    assert_eq!(next.names, ["worker", "third"]);
    assert_eq!(facts.get::<FunctionaryRosterFact, _>(&provider, &"missing".into()).await.unwrap_err().code(), "no_such_scope");
}

#[test]
fn declaration_wire_defaults_strictness_and_roster_order_are_preserved() {
    let primary: AgentRef = serde_yaml_ng::from_str(
        "harness: pi\nname: critic\nargs: [first]\nrole: reviewer\nmax_sessions: 3",
    )
    .unwrap();
    let extra: ScopeAgent =
        serde_yaml_ng::from_str("name: critic\nharness: shell\nargs: [ignored]").unwrap();
    let second: ScopeAgent =
        serde_yaml_ng::from_str("harness: shell\nlifetime: permanent").unwrap();
    let roster = declared_agents(Some(&primary), &[extra, second]);
    assert_eq!(
        roster.iter().map(ScopeAgent::name).collect::<Vec<_>>(),
        ["critic", "shell"]
    );
    assert_eq!(roster[0].harness, "pi");
    assert_eq!(roster[0].args, ["first"]);
    assert_eq!(roster[0].max_sessions, Some(3));
    assert!(!roster[0].autostart());
    assert!(roster[1].autostart());
    assert_eq!(roster[0].sandbox, Sandbox::None);
    let legacy = serde_json::to_value(&primary).unwrap();
    assert_eq!(
        legacy,
        serde_json::json!({"harness":"pi","name":"critic","role":"reviewer","args":["first"],"max_sessions":3})
    );
    assert!(serde_yaml_ng::from_str::<AgentRef>("harness: pi\nmisspelled: yes").is_err());
    assert!(serde_yaml_ng::from_str::<ScopeAgent>("harness: pi\nmisspelled: yes").is_err());
    assert!(serde_yaml_ng::from_str::<AgentRef>("[pi]").is_err());
    let named: AgentRef = serde_yaml_ng::from_str("pi").unwrap();
    assert!(declared_agents(Some(&named), &[]).is_empty());
    assert_eq!(named.adapter(), "pi");
}

#[test]
fn synthesis_keeps_exclusions_harness_precedence_and_declared_foremen() {
    let primary = AgentRef::Name("pi".into());
    let mut foreman = ForemanConfig {
        enabled: true,
        ..Default::default()
    };
    assert!(agents_with("team/root", Some(&primary), &[], &foreman).is_empty());
    let added = agents_with("team/demo", Some(&primary), &[], &foreman);
    assert_eq!(added[0].harness, "pi");
    assert_eq!(added[0].role, Role::foreman());
    assert_eq!(added[0].lifetime, Lifetime::Permanent);
    assert!(added[0].autostart());
    assert_eq!(added[0].sandbox, Sandbox::None);
    assert_eq!(
        agents_with("demo", None, &[], &foreman)[0].harness,
        "claude-code"
    );
    foreman.harness = Some("shell".into());
    assert_eq!(
        agents_with("demo", Some(&primary), &[], &foreman)[0].harness,
        "shell"
    );
    foreman.exclude = vec!["team/demo".into()];
    assert!(agents_with("team/demo", None, &[], &foreman).is_empty());
    assert_eq!(agents_with("else/demo", None, &[], &foreman).len(), 1);
    foreman.exclude = vec!["demo".into()];
    assert!(agents_with("else/demo", None, &[], &foreman).is_empty());
    foreman.exclude.clear();
    let declared: ScopeAgent =
        serde_yaml_ng::from_str("name: chef\nharness: pi\nrole: foreman\nsandbox: docker").unwrap();
    let kept = agents_with("demo", None, &[declared], &foreman);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].name(), "chef");
    assert_eq!(kept[0].sandbox, Sandbox::Docker);
    foreman.enabled = false;
    assert!(agents_with("demo", None, &[], &foreman).is_empty());
}

#[test]
fn shared_role_chain_follows_paths_and_replaces_whole_definitions() {
    let mut parent = scope("unrelated-name", "projects");
    parent
        .roles
        .insert("reviewer".into(), spec(&["task.edit", "task.report"]));
    let mut child = scope("renamed", "projects/demo");
    child
        .roles
        .insert("reviewer".into(), spec(&["task.create"]));
    let sibling = scope("unrelated-name/fake-child", "elsewhere");
    let lookalike = scope("lookalike", "projects-x");
    let scopes = vec![parent, child, sibling, lookalike];
    let root = BTreeMap::from([("reviewer".into(), spec(&["task.run"]))]);
    let child_roles = role_chain::roles_for_scope(&root, &scopes, &scopes[1]).unwrap();
    let entry = child_roles.entry(&Role::new("reviewer")).unwrap();
    assert_eq!(entry.def.grants, BTreeSet::from([Grant::TaskCreate]));
    assert_eq!(
        entry.origin,
        RoleOrigin::Scope {
            scope: "renamed".into()
        }
    );
    assert_eq!(
        entry.overrides,
        Some(RoleOrigin::Scope {
            scope: "unrelated-name".into()
        })
    );
    assert_eq!(entry.def.reach, crate::role::Reach::Own);
    for s in &scopes[2..] {
        assert_eq!(
            role_chain::roles_for_scope(&root, &scopes, s)
                .unwrap()
                .get(&Role::new("reviewer"))
                .unwrap()
                .grants,
            BTreeSet::from([Grant::TaskRun])
        );
    }
    let mut invalid = scopes.clone();
    invalid[0]
        .roles
        .insert("worker".into(), spec(&["task.run"]));
    let error = role_chain::roles_for_scope(&root, &invalid, &invalid[1])
        .unwrap_err()
        .to_string();
    assert!(error.contains("worker") && error.contains("unrelated-name"));
}

#[tokio::test]
async fn actual_l3_port_reads_live_aliases_grants_sandboxes_and_foremen() {
    let mut owner = provider(vec![scope("team/demo", "projects/demo")]);
    let people = Facts::<People>::new();
    let upper = Facts::<L6>::new();
    let before = people
        .get::<AgentFact, _>(&owner, &"demo".into())
        .await
        .unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].grants, Some(BTreeSet::from([Grant::TaskReport])));
    assert!(!before[0].has_sandbox && !before[0].sandbox_enforced);
    owner.scopes[0].agents[0].sandbox = Sandbox::Docker;
    owner.root_roles.insert("reviewer".into(), spec(&[]));
    owner.foreman.enabled = true;
    let edited = upper
        .get::<AgentFact, _>(&owner, &"projects/demo".into())
        .await
        .unwrap();
    assert_eq!(edited.len(), 2);
    assert_eq!(edited[0].grants, Some(BTreeSet::new()));
    assert!(edited[0].has_sandbox && !edited[0].sandbox_enforced);
    assert_eq!(edited[1].name, "foreman");
    assert!(!edited[1].has_sandbox && !edited[1].sandbox_enforced);
    owner.scopes[0].agents[0].sandbox = Sandbox::Openshell;
    owner.root_roles.clear();
    let unknown = Provide::<AgentFact>::get(&owner, &"team/demo".into()).await.unwrap();
    assert!(unknown[0].has_sandbox && unknown[0].sandbox_enforced);
    assert_eq!(unknown[0].grants, None);
    // Invalid programmatic config follows the original built-in-role fallback.
    owner
        .root_roles
        .insert("worker".into(), spec(&["task.run"]));
    owner.scopes[0].agents[0].role = Role::worker();
    let fallback = Provide::<AgentFact>::get(&owner, &"demo".into()).await.unwrap();
    assert_eq!(
        fallback[0].grants,
        Some(
            Roles::presets()
                .get(&Role::worker())
                .unwrap()
                .grants
                .clone()
        )
    );
    owner.scopes.push(scope("ops/demo", "projects/ops/demo"));
    assert!(matches!(
        Provide::<AgentFact>::get(&owner, &"demo".into()).await,
        Err(FactoryError::BadRequest(_))
    ));
    assert!(matches!(
        Provide::<AgentFact>::get(&owner, &"gone".into()).await,
        Err(FactoryError::NoSuchScope(_))
    ));
    assert_eq!(Provide::<AgentFact>::get(&owner, &"projects/demo".into()).await.unwrap().len(), 2);
}
