//! Throwaway own-store/filesystem tests. No credentials or company state.
use crate::{
    credential_expiry, credentials, dependencies as dep, dependency_inventory as inventory,
    expiry_store,
};
use chrono::Utc;
use factory_kernel::{Facts, People, Provide, ScopeNode, ScopeTree};
use std::{collections::BTreeSet, path::PathBuf};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("factory-l2-provider-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn tree() -> ScopeTree {
    ScopeTree {
        scopes: vec![
            ScopeNode {
                name: "demo".into(),
                path: "projects/demo".into(),
            },
            ScopeNode {
                name: "sibling".into(),
                path: "projects/sibling".into(),
            },
        ],
    }
}
fn credentials(root: &Scratch) -> credentials::Provider {
    credentials::Provider {
        root: root.0.clone(),
        scopes: tree(),
        home: Some(root.0.join("home")),
    }
}
#[tokio::test]
async fn credential_presence_is_live_metadata_only_and_empty_queries_need_no_probe() {
    let root = Scratch::new();
    let mut provider = credentials(&root);
    let empty = provider.get(&BTreeSet::new()).await.unwrap();
    assert!(empty.is_empty());
    let asked = BTreeSet::from(["projects/demo".into()]);
    let before = provider.get(&asked).await.unwrap();
    assert!(!before["demo"].0["scope_env"]);
    let env = root.0.join("projects/demo/.env");
    std::fs::create_dir_all(env.parent().unwrap()).unwrap();
    std::fs::write(&env, "DO_NOT_READ=PRIVATE_SENTINEL").unwrap();
    let ssh = root.0.join("home/.ssh");
    std::fs::create_dir_all(&ssh).unwrap();
    std::fs::write(ssh.join("id_fixture.pub"), "public").unwrap();
    assert!(!provider.get(&asked).await.unwrap()["demo"].0["ssh"]);
    std::fs::write(ssh.join("id_fixture"), "PRIVATE_SENTINEL").unwrap();
    let live = provider.get(&asked).await.unwrap();
    assert!(live["demo"].0["scope_env"] && live["demo"].0["ssh"]);
    assert!(!serde_json::to_string(&live)
        .unwrap()
        .contains("PRIVATE_SENTINEL"));
    assert!(
        !provider
            .get(&BTreeSet::from(["sibling".into()]))
            .await
            .unwrap()["sibling"]
            .0["scope_env"]
    );
    assert!(provider
        .get(&BTreeSet::from(["missing".into()]))
        .await
        .is_err());
    std::fs::remove_file(&env).unwrap();
    assert!(!provider.get(&asked).await.unwrap()["demo"].0["scope_env"]);
    provider.home = None;
    assert!(!provider.get(&asked).await.unwrap()["demo"]
        .0
        .contains_key("ssh"));
    provider.scopes.scopes = vec![
        ScopeNode {
            name: "team/demo".into(),
            path: "projects/team/demo".into(),
        },
        ScopeNode {
            name: "ops/demo".into(),
            path: "projects/ops/demo".into(),
        },
    ];
    assert!(provider
        .get(&BTreeSet::from(["demo".into()]))
        .await
        .is_err());
    assert!(provider
        .get(&BTreeSet::from(["projects/team/demo".into()]))
        .await
        .unwrap()
        .contains_key("team/demo"));
}

fn inventory(root: &Scratch) -> inventory::Provider {
    inventory::Provider {
        root: root.0.clone(),
        scopes: tree(),
        declarations: vec![
            inventory::Scope {
                name: "demo".into(),
                dependencies: Default::default(),
            },
            inventory::Scope {
                name: "sibling".into(),
                dependencies: Default::default(),
            },
        ],
        credentials: credentials(root),
    }
}
fn attach(root: &Scratch, kind: dep::AttachmentKind, bytes: &[u8]) -> dep::Attachment {
    let validated = dep::validate_document(bytes, kind).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let attachment = dep::Attachment {
        id: id.clone(),
        kind,
        scope: "demo".into(),
        run_id: "run".into(),
        task_id: "task".into(),
        attempt: 1,
        attached_at: Utc::now(),
        filename: format!("{id}.cdx.json"),
        spec_version: validated.spec_version,
        states: validated.states,
    };
    inventory::write_attachment(&root.0, &attachment, bytes).unwrap();
    attachment
}
#[tokio::test]
async fn dependency_ports_reread_attachments_and_keep_exact_release_selection() {
    let root = Scratch::new();
    let provider = inventory(&root);
    let facts = Facts::<People>::new();
    let before = facts
        .get::<factory_kernel::DependenciesFact, _>(&provider, &"demo".into())
        .await
        .unwrap();
    assert!(before.built_sbom_at.is_none());
    let sbom = attach(
        &root,
        dep::AttachmentKind::Sbom,
        include_bytes!("../tests/fixtures/dependencies/build-sbom.cdx.json"),
    );
    attach(
        &root,
        dep::AttachmentKind::Vulnerabilities,
        include_bytes!("../tests/fixtures/dependencies/vulnerabilities.cdx.json"),
    );
    let live = facts
        .get::<factory_kernel::DependenciesFact, _>(&provider, &"projects/demo".into())
        .await
        .unwrap();
    assert_eq!(live.built_sbom_at, Some(sbom.attached_at));
    assert!(!live.open.is_empty());
    let report = provider.dependencies_report("demo").await.unwrap();
    assert_eq!(live, inventory::fact(&report));
    let exploited = facts
        .get::<factory_kernel::ExploitedFinding, _>(&provider, &"demo".into())
        .await
        .unwrap();
    assert!(!exploited.is_empty());
    assert!(exploited.iter().all(|f| f.scope == "demo"));
    let query = inventory::ReleaseSbomQuery {
        scope: "projects/demo".into(),
        commit: "0123456789abcdef0123456789abcdef01234567".into(),
        version: Some("0.1.0".into()),
    };
    let release = facts
        .get::<factory_kernel::ReleaseSbomFact, _>(&provider, &query)
        .await
        .unwrap();
    assert_eq!(release.len(), 1);
    assert_eq!(release[0].attachment, sbom);
    let wrong = inventory::ReleaseSbomQuery {
        version: Some("other".into()),
        ..query
    };
    assert!(facts
        .get::<factory_kernel::ReleaseSbomFact, _>(&provider, &wrong)
        .await
        .unwrap()
        .is_empty());
    assert!(facts
        .get::<factory_kernel::DependenciesFact, _>(&provider, &"sibling".into())
        .await
        .unwrap()
        .built_sbom_at
        .is_none());
    assert!(provider.dependencies_report("missing").await.is_err());
    // Malformed immutable evidence is an error, never a fresh empty inventory.
    std::fs::write(
        root.0
            .join(".factory/dependencies/demo/run")
            .join(&sbom.filename),
        "{bad",
    )
    .unwrap();
    assert!(facts
        .get::<factory_kernel::DependenciesFact, _>(&provider, &"demo".into())
        .await
        .is_err());
    assert!(facts
        .get::<factory_kernel::ReleaseSbomFact, _>(&provider, &wrong)
        .await
        .is_err());
}

#[tokio::test]
async fn authored_expiry_is_live_and_never_resolves_or_runs_a_credential_source() {
    let root = Scratch::new();
    let store = expiry_store::ObservationStore::in_memory().unwrap();
    let mut provider = credential_expiry::Provider {
        store: &store, instance: "a1b2c3d4".into(),
        declarations: serde_yaml_ng::from_str(&format!(
            "- name: secret\n  kind: token\n  source: {{from: command, run: 'touch {}'}}\n  expires: 2020-01-01\n",
            root.0.join("must-not-exist").display())).unwrap(),
        providers: vec![credential_expiry::ScopedProviders {
            name: "demo".into(), agents: vec![credential_expiry::AgentProviders {
                name: "curator".into(),
                config: serde_yaml_ng::from_str("providers:\n - name: factory-github\n   type: github-publish\n   credential: {secret: secret}\npolicy: {}").unwrap(),
            }],
        }],
    };
    let before = provider.get(&()).await.unwrap();
    assert_eq!(before.observations.len(), 1);
    let item = &before.observations[0];
    assert_eq!(item.basis, factory_kernel::DateBasis::Declared);
    assert_eq!(
        item.affects[0].label,
        "demo / curator / factory-github-a1b2c3d4"
    );
    assert!(item.observed_at.is_none());
    assert!(!root.0.join("must-not-exist").exists());
    let mut cached = item.clone();
    cached.id = "cached-probe".into();
    store.replace(vec![cached], true).await.unwrap();
    provider.declarations[0].expires = Some(crate::secrets::Expiry::Never);
    let live = provider.get(&()).await.unwrap();
    assert_eq!(live.observations.len(), 2);
    assert!(
        live.observations
            .iter()
            .find(|o| o.id == "secret:secret")
            .unwrap()
            .no_expiry
    );
    provider.declarations[0].expires = None;
    let unknown = provider.get(&()).await.unwrap();
    assert_eq!(
        unknown
            .observations
            .iter()
            .find(|o| o.id == "secret:secret")
            .unwrap()
            .basis,
        factory_kernel::DateBasis::Unknown
    );
    assert!(!root.0.join("must-not-exist").exists());
}
#[tokio::test]
async fn expiry_provider_propagates_own_store_errors_even_with_authored_metadata() {
    let root = Scratch::new();
    let db = root.0.join("own.sqlite");
    let store = expiry_store::ObservationStore::open(&db).unwrap();
    let provider = credential_expiry::Provider {
        store: &store,
        instance: "test".into(),
        declarations: serde_yaml_ng::from_str(
            "- name: authored\n  kind: token\n  source: {from: command, run: no-such-source}\n  expires: never"
        ).unwrap(),
        providers: vec![],
    };
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch("DROP TABLE credential_expiries")
        .unwrap();
    assert!(provider.get(&()).await.is_err());
}
