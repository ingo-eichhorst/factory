//! `factory scope add|reconcile|list` (design §7; ADR 0016).

use std::path::Path;

use serde_json::json;
use uuid::Uuid;

use crate::exit;
use crate::rpc;

/// Always refused by the daemon (decision 9) — round-tripped anyway so the
/// operator sees the daemon's own remedy text (edit `config.yaml`, then
/// `scope reconcile --apply`), not a canned local one.
pub fn add(root: &Path, path: &Path, name: &str) -> i32 {
    let payload = json!({ "path": path.display().to_string(), "name": name });
    let outcome = rpc::command(root, Uuid::nil(), "scope.add", payload);
    rpc::report(root, outcome, |result| {
        rpc::print_result("factory: scope added", &result);
        exit::OK
    })
}

pub fn reconcile(root: &Path, apply: bool) -> i32 {
    let outcome = rpc::command(
        root,
        Uuid::nil(),
        "scope.reconcile",
        json!({ "apply": apply }),
    );
    rpc::report(root, outcome, |result| {
        let clean = result
            .get("clean")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let headline = if clean {
            "factory: scope reconcile: clean"
        } else {
            "factory: scope reconcile: drift found"
        };
        rpc::print_result(headline, &result);
        exit::OK
    })
}

pub fn list(root: &Path) -> i32 {
    let outcome = rpc::query(root, Uuid::nil(), "scope.list", json!({}));
    rpc::report(root, outcome, |result| {
        let count = result
            .get("scopes")
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len);
        rpc::print_result(&format!("factory: {count} registered scope(s)"), &result);
        exit::OK
    })
}
