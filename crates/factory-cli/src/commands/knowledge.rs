//! `factory knowledge write|list|show` (design §7, backlog §12; ADR 0022).
//!
//! `scope_id` is unused by every operation this module calls — a note lives
//! in the company root's shared `.factory/knowledge/`, never under a scope
//! directory — so every call below passes [`Uuid::nil`], the same filler
//! `factory schedule list`/`factory status` already pass for an operation
//! that ignores it.

use std::path::Path;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::exit;
use crate::rpc;

/// `factory knowledge write`. The body is read from standard input
/// ([`super::read_stdin_body`]) — there is deliberately no `--body` flag
/// (ADR 0022 decision 4).
#[allow(clippy::too_many_arguments)]
pub fn write(
    root: &Path,
    name: &str,
    title: &str,
    status: &str,
    source: &[String],
    update: bool,
    task_id: Option<Uuid>,
) -> i32 {
    let body = match super::read_stdin_body() {
        Ok(body) => body,
        Err(code) => return code,
    };

    let mut payload = json!({
        "name": name,
        "title": title,
        "status": status,
        "sources": source,
        "body": body,
        "update": update,
    });
    if let Some(task_id) = task_id {
        payload["task_id"] = json!(task_id.to_string());
    }

    let outcome = rpc::command(root, Uuid::nil(), "knowledge.write", payload);
    rpc::report(root, outcome, |result| {
        let written_name = rpc::field_str(&result, "name", name).to_string();
        rpc::print_result(&format!("factory: note {written_name} written"), &result);
        exit::OK
    })
}

pub fn list(root: &Path) -> i32 {
    let outcome = rpc::query(root, Uuid::nil(), "knowledge.list", json!({}));
    rpc::report(root, outcome, |result| {
        let count = result
            .get("notes")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        rpc::print_result(&format!("factory: {count} note(s)"), &result);
        exit::OK
    })
}

/// The note exactly as it is on disk (ADR 0022's own consequences): this
/// prints the daemon's `text` field verbatim, never a re-rendering of it —
/// `knowledge show` resolves no `[[link]]` and renders nothing.
pub fn show(root: &Path, name: &str) -> i32 {
    let outcome = rpc::query(root, Uuid::nil(), "knowledge.show", json!({ "name": name }));
    rpc::report(root, outcome, |result| {
        match result.get("text").and_then(Value::as_str) {
            Some(text) => print!("{text}"),
            None => rpc::print_result(&format!("factory: note {name}"), &result),
        }
        exit::OK
    })
}
