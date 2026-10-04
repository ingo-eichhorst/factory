//! Tests use only throwaway files and L1 capabilities; no upper-level fixture.
use crate::{
    backup, backup_facts, backup_store::BackupStore, environment_facts,
    environment_store::EnvironmentStore, environments as env, interfaces, renewal_declarations,
    settings_facts::SettingsProvider,
};
use chrono::{Duration, Utc};
use factory_kernel::{Facts, People, Provide, ScopeNode, ScopeTree};
use std::path::PathBuf;

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("factory-l1-provider-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn settings(bind: Option<&str>) -> SettingsProvider {
    SettingsProvider {
        foreman_enabled: false,
        power_assertion: true,
        tick_seconds: 5,
        scopes: vec![("root".into(), None), ("child".into(), Some(3))],
        interfaces: bind
            .into_iter()
            .map(|bind| serde_yaml_ng::from_str(&format!("kind: http\nbind: '{bind}'\n")).unwrap())
            .collect(),
    }
}
#[tokio::test]
async fn configured_listener_and_facts_share_one_bind_and_unknown_never_resolves() {
    let facts = Facts::<People>::new();
    for (bind, expected) in [
        (None, Some(true)),
        (Some("127.0.0.1:8787"), Some(true)),
        (Some("[::1]:8787"), Some(true)),
        (Some("0.0.0.0:8787"), Some(false)),
        (Some("localhost:8787"), None),
        (Some("invalid"), None),
    ] {
        let provider = settings(bind);
        let value = facts
            .get::<factory_kernel::DaemonConfigFact, _>(&provider, &())
            .await
            .unwrap();
        assert_eq!(value.http_loopback_only, expected);
        assert!(!value.foreman_enabled && value.power_assertion);
        if let Some(interface) = provider.interfaces.first() {
            assert_eq!(
                interfaces::interface_facts(&provider.interfaces)[0].bind,
                Some(interface.http_bind())
            );
        }
    }
    let capacity = facts
        .get::<factory_kernel::ScopeCapacityFact, _>(&settings(None), &())
        .await
        .unwrap();
    assert_eq!(
        capacity.max_sessions,
        std::collections::BTreeMap::from([("child".into(), 3)])
    );
    assert_eq!(capacity.tick_seconds, 5);
    let interface: interfaces::InterfaceConfig = serde_yaml_ng::from_str("kind: http").unwrap();
    assert_eq!(interface.http_bind(), interfaces::DEFAULT_HTTP_BIND);
}

#[tokio::test]
async fn live_backup_reads_archives_busy_and_requested_instant_without_upper_capabilities() {
    let scratch = Scratch::new();
    let store = BackupStore::in_memory().unwrap();
    let busy = tokio::sync::Mutex::new(());
    let now = Utc::now();
    let mut provider = backup_facts::Provider {
        store: &store,
        busy: &busy,
        root: scratch.0.clone(),
        instance: "test".into(),
        config: None,
        booted_at: now - Duration::days(1),
    };
    let absent = provider.get(&now).await.unwrap();
    assert!(!absent.configured && absent.newest.is_none());
    assert_eq!(absent.at, now);
    let dest = scratch.0.join("destination");
    std::fs::create_dir_all(&dest).unwrap();
    provider.config = Some(
        serde_yaml_ng::from_str(&format!(
            "destination: {}\nkeep: {{daily: 7}}",
            dest.display()
        ))
        .unwrap(),
    );
    let captured = provider.capture(now).await.unwrap();
    assert!(captured.found.is_empty() && !captured.running);
    assert_eq!(captured.destination.unwrap().same_device, Some(true));
    let at = now - Duration::hours(1);
    let archive = backup::archive_name("test", at, false);
    std::fs::write(dest.join(&archive), b"archive").unwrap();
    std::fs::write(
        dest.join(backup::archive_name("other", now, false)),
        b"other",
    )
    .unwrap();
    let guard = busy.lock().await;
    let captured = provider.capture(now).await.unwrap();
    assert!(captured.running);
    assert_eq!(captured.found.len(), 1);
    assert_eq!(captured.found[0].name, archive);
    let live = provider.get(&now).await.unwrap();
    assert_eq!(live.at, now);
    // Archive names have second precision, not invented nanosecond evidence.
    assert_eq!(live.newest.unwrap().timestamp(), at.timestamp());
    drop(guard);
    std::fs::remove_file(dest.join(&archive)).unwrap();
    assert!(provider.get(&now).await.unwrap().newest.is_none());
}

#[tokio::test]
async fn backup_and_environment_own_store_failures_are_not_empty_success() {
    let scratch = Scratch::new();
    let db = scratch.0.join("own.sqlite");
    let backup = BackupStore::open(&db).unwrap();
    let environments = EnvironmentStore::open(&db).unwrap();
    let busy = tokio::sync::Mutex::new(());
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch("DROP TABLE backup_events; DROP TABLE health_samples;")
        .unwrap();
    let provider = backup_facts::Provider {
        store: &backup,
        busy: &busy,
        root: scratch.0.clone(),
        instance: "test".into(),
        config: None,
        booted_at: Utc::now(),
    };
    assert!(provider.get(&Utc::now()).await.is_err());
    let provider = environment_facts::Provider {
        store: &environments,
        scopes: ScopeTree::default(),
        declarations: vec![],
    };
    assert!(
        Provide::<factory_kernel::EnvironmentMetricFact>::get(&provider, &None)
            .await
            .is_err()
    );
}

fn environment_provider(store: &EnvironmentStore) -> environment_facts::Provider<'_> {
    environment_facts::Provider {
        store,
        scopes: ScopeTree {
            scopes: vec![
                ScopeNode {
                    name: "root".into(),
                    path: ".".into(),
                },
                ScopeNode {
                    name: "child".into(),
                    path: "projects/child".into(),
                },
                ScopeNode {
                    name: "sibling".into(),
                    path: "projects/sibling".into(),
                },
            ],
        },
        declarations: vec![(
            "child".into(),
            serde_yaml_ng::from_str("name: prod\ntier: production").unwrap(),
        )],
    }
}
#[tokio::test]
async fn environment_metrics_share_history_and_resolve_live_scope_membership() {
    let store = EnvironmentStore::in_memory().unwrap();
    let mut provider = environment_provider(&store);
    let empty =
        Provide::<factory_kernel::EnvironmentMetricFact>::get(&provider, &Some("child".into()))
            .await
            .unwrap();
    assert!(empty["prod"].availability.is_none());
    store
        .append_sample(env::Sample {
            environment: "prod".into(),
            check: "alive".into(),
            at: Utc::now(),
            ok: true,
            latency_ms: 1,
            slow: false,
            detail: None,
        })
        .await
        .unwrap();
    let cards = provider.cards(Some("projects/child")).await.unwrap();
    let metric =
        Provide::<factory_kernel::EnvironmentMetricFact>::get(&provider, &Some("root".into()))
            .await
            .unwrap();
    assert_eq!(metric["prod"].availability, cards["prod"].uptime_window);
    assert_eq!(metric["prod"].availability, Some(1.0));
    assert!(Provide::<factory_kernel::EnvironmentMetricFact>::get(
        &provider,
        &Some("sibling".into())
    )
    .await
    .unwrap()
    .is_empty());
    assert!(Provide::<factory_kernel::EnvironmentMetricFact>::get(
        &provider,
        &Some("missing".into())
    )
    .await
    .is_err());
    provider.declarations.clear();
    assert!(
        Provide::<factory_kernel::EnvironmentMetricFact>::get(&provider, &None)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn publication_uses_own_deployment_and_current_declaration_without_inventing_success() {
    let store = EnvironmentStore::in_memory().unwrap();
    let mut provider = environment_provider(&store);
    assert!(Provide::<factory_kernel::DeploymentPublicationFact>::get(
        &provider,
        &"missing".into()
    )
    .await
    .unwrap()
    .is_none());
    let deployment = env::Deployment {
        id: "d".into(),
        scope: "child".into(),
        environment: "prod".into(),
        release: env::ReleaseFacts {
            commit: "abc".into(),
            dirty: true,
            ..Default::default()
        },
        actor: env::Actor {
            kind: env::ActorKind::Person,
            name: "owner".into(),
            run_id: None,
            task_id: None,
        },
        via: None,
        manual: true,
        strict_verification: false,
        started_at: Utc::now(),
        finished_at: None,
        status: env::DeployStatus::Running,
        reason: None,
        previous_commit: None,
        verification: None,
    };
    store.started(&deployment).await.unwrap();
    let value = Provide::<factory_kernel::DeploymentPublicationFact>::get(&provider, &"d".into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(value.state, "in_progress");
    assert_eq!(value.verified, None);
    assert!(value.production && !value.transient && value.repository.is_none());
    provider.declarations.clear();
    let value = Provide::<factory_kernel::DeploymentPublicationFact>::get(&provider, &"d".into())
        .await
        .unwrap()
        .unwrap();
    assert!(!value.production && !value.transient);
    assert_eq!(value.commit, "abc");
}

#[tokio::test]
async fn renewal_files_are_live_and_root_scope_cache_keys_never_collide() {
    let scratch = Scratch::new();
    std::fs::create_dir_all(scratch.0.join(".factory")).unwrap();
    let path = scratch.0.join(".factory/config.yaml");
    let cache = renewal_declarations::DeclarationCache::default();
    let provider = renewal_declarations::Provider {
        cache: &cache,
        instance_path: path.clone(),
        declarations: vec![],
        scopes: vec![
            renewal_declarations::ScopeRenewals {
                name: "root".into(),
                path: scratch.0.clone(),
                declarations: vec![],
            },
            renewal_declarations::ScopeRenewals {
                name: "root".into(),
                path: scratch.0.clone(),
                declarations: vec![],
            },
        ],
    };
    std::fs::write(&path, "renewals: [{name: global, kind: licence, expires: 2020-01-01}]\nscope:\n renewals: [{name: scoped, kind: licence, expires: 2021-01-01}]\n").unwrap();
    let before = provider.get(&()).await.unwrap();
    assert_eq!(before.declarations.len(), 2); // duplicate scope input stays deduplicated
    assert_eq!(before.declarations[0].declaration.name, "global");
    assert_eq!(before.declarations[1].scope.as_deref(), Some("root"));
    assert_eq!(before.declarations[1].declaration.name, "scoped");
    for invalid in ["[invalid", &"x".repeat(1024 * 1024 + 1)] {
        std::fs::write(&path, invalid).unwrap();
        let after = provider.get(&()).await.unwrap();
        assert_eq!(after.declarations, before.declarations);
        assert_eq!(after.findings.len(), 2);
    }
    std::fs::remove_file(&path).unwrap();
    let missing = provider.get(&()).await.unwrap();
    assert_eq!(missing.declarations, before.declarations);
    assert_eq!(missing.findings.len(), 2);
    std::fs::write(&path, "renewals: []\nscope: {renewals: []}").unwrap();
    let cleared = provider.get(&()).await.unwrap();
    assert!(cleared.declarations.is_empty() && cleared.findings.is_empty());
}
