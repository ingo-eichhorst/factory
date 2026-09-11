//! `factory memory add|list` (design §7, backlog §12; ADR 0022).

use std::path::Path;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::exit;
use crate::rpc;

/// `factory memory add --scope <scope>`. The entry is read from standard
/// input ([`super::read_stdin_body`]), the same reason `knowledge write`'s
/// body is. `scope` is the resolved target scope, sent as the envelope's own
/// `scope_id` — the same stance `agent start` already takes toward it.
pub fn add(root: &Path, scope: Uuid) -> i32 {
    let entry = match super::read_stdin_body() {
        Ok(entry) => entry,
        Err(code) => return code,
    };

    let entry_id = crate::ids::new_id();
    let payload = json!({
        "entry_id": entry_id.to_string(),
        "entry": entry,
    });

    let outcome = rpc::command(root, scope, "memory.add", payload);
    rpc::report(root, outcome, |result| {
        rpc::print_result(&format!("factory: memory entry {entry_id} added"), &result);
        exit::OK
    })
}

pub fn list(root: &Path, scope: Uuid) -> i32 {
    let outcome = rpc::query(root, scope, "memory.list", json!({}));
    rpc::report(root, outcome, |result| {
        let count = result
            .get("entries")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        rpc::print_result(&format!("factory: {count} memory entries"), &result);
        exit::OK
    })
}
