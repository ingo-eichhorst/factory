use factory_kernel::SessionRef;
use serde_json::json;

#[test]
fn opaque_session_ids_preserve_legacy_defaults_and_runtime_metadata() {
    let bare = json!({"runtime": "runtime", "handle": "opaque/handle"});
    let parsed: SessionRef = serde_json::from_value(bare.clone()).unwrap();
    assert!(parsed.meta.is_empty());
    assert_eq!(serde_json::to_value(parsed).unwrap(), bare);
    let detailed = json!({
        "runtime": "runtime", "handle": "opaque/handle",
        "meta": {"workspace": "demo", "conversation": "opaque"}
    });
    let parsed: SessionRef = serde_json::from_value(detailed.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), detailed);
}
