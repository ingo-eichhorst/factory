//! The physical transport owner, without the compatibility facade or daemon.
use factory_composition::config::InterfaceConfig;
use factory_interfaces::{
    protocol::{Envelope, Request, Response},
    Event, EventBus, Interface, InterfaceContext,
};
use factory_kernel::Result;
use serde_json::json;
use std::sync::Arc;

#[test]
fn legacy_envelopes_and_error_answers_keep_their_flattening_and_token_semantics() {
    let absent: Envelope = serde_json::from_value(json!({"op":"status"})).unwrap();
    assert!(absent.token.is_none());
    assert!(matches!(absent.request, Request::Status));
    assert_eq!(
        serde_json::to_value(absent).unwrap(),
        json!({"op":"status"})
    );
    let present: Envelope = serde_json::from_value(json!({
        "op":"task.run", "params":{"id":"t"}, "token":"synthetic-token"
    }))
    .unwrap();
    assert_eq!(present.token.as_deref(), Some("synthetic-token"));
    let answer = Response::error("denied", "No grant");
    assert_eq!(
        serde_json::to_value(answer).unwrap(),
        json!({
            "status":"error", "code":"denied", "message":"No grant"
        })
    );
}

#[tokio::test]
async fn the_observer_bus_redacts_run_credentials_and_digests_without_mutating_the_run() {
    let run: factory_process::run::Run = serde_json::from_value(json!({
        "id":"r", "task_id":"t", "attempt":1, "status":"running", "trigger":"manual",
        "agent":"shell", "adapter":"shell", "runtime":"runtime",
        "started_at":"2026-10-04T12:00:00Z", "token":"synthetic-current-token",
        "spent_token_sha256":"synthetic-spent-digest",
        "superseded_token_sha256s":["synthetic-older-digest"]
    }))
    .unwrap();
    let bus = EventBus::new(2);
    let mut receiver = bus.subscribe();
    assert_eq!(bus.subscriber_count(), 1);
    bus.publish(Event::RunUpdated { run: run.clone() });
    let observed = receiver.recv().await.unwrap();
    assert_eq!(observed.task_id(), Some("t"));
    let json = serde_json::to_value(observed).unwrap();
    for key in ["token", "spent_token_sha256", "superseded_token_sha256s"] {
        assert!(json["run"].get(key).is_none(), "{key}");
    }
    assert_eq!(run.token.as_deref(), Some("synthetic-current-token"));
    assert_eq!(run.superseded_token_sha256s, vec!["synthetic-older-digest"]);
}

#[tokio::test]
async fn a_slow_observer_loses_old_events_and_cannot_turn_the_bus_into_a_fact_log() {
    let bus = EventBus::new(1);
    let mut receiver = bus.subscribe();
    bus.publish(Event::TaskDeleted { id: "older".into() });
    bus.publish(Event::TaskDeleted { id: "newer".into() });
    assert!(matches!(
        receiver.recv().await,
        Err(tokio::sync::broadcast::error::RecvError::Lagged(1))
    ));
    assert_eq!(receiver.recv().await.unwrap().task_id(), Some("newer"));
    drop(receiver);
    assert_eq!(bus.subscriber_count(), 0);
    // No subscribers is a normal headless daemon, not an outage.
    bus.publish(Event::TaskDeleted {
        id: "headless".into(),
    });
}

struct Loopback;
#[async_trait::async_trait]
impl Interface<tokio::sync::Notify> for Loopback {
    fn name(&self) -> &str {
        "loopback"
    }
    async fn serve(
        self: Arc<Self>,
        api: Arc<tokio::sync::Notify>,
        ctx: InterfaceContext,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<()> {
        assert_eq!(ctx.resolve("relative.sock"), ctx.root.join("relative.sock"));
        assert_eq!(
            ctx.resolve("/tmp/absolute.sock"),
            std::path::PathBuf::from("/tmp/absolute.sock")
        );
        api.notify_one();
        loop {
            if *shutdown.borrow() || shutdown.changed().await.is_err() {
                break;
            }
        }
        Ok(())
    }
}

#[tokio::test]
async fn the_unchanged_generic_interface_seam_mounts_and_honors_shutdown() {
    let interface = Arc::new(Loopback);
    assert_eq!(interface.description(), "loopback interface");
    let api = Arc::new(tokio::sync::Notify::new());
    let ctx = InterfaceContext {
        config: InterfaceConfig {
            kind: "loopback".into(),
            settings: Default::default(),
        },
        root: "/tmp/factory-interface-owner-no-io".into(),
        factory_dir: "/tmp/factory-interface-owner-no-io/.factory".into(),
    };
    let (tx, rx) = tokio::sync::watch::channel(false);
    let mount = tokio::spawn(interface.serve(api.clone(), ctx, rx));
    tokio::time::timeout(std::time::Duration::from_secs(1), api.notified())
        .await
        .unwrap();
    assert!(!mount.is_finished());
    tx.send(true).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), mount)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
