//! `factory context show` (design §2.5, §7).

use std::path::Path;

use serde_json::json;
use uuid::Uuid;

use crate::exit;
use crate::rpc;

pub fn show(root: &Path, scope: Uuid, agent_name: &str, task_prompt: Option<&str>) -> i32 {
    let mut payload = json!({ "agent_name": agent_name });
    if let Some(task_prompt) = task_prompt {
        payload["task_prompt"] = json!(task_prompt);
    }

    let outcome = rpc::query(root, scope, "context.show", payload);
    rpc::report(root, outcome, |result| {
        if let Some(text) = result.get("text").and_then(serde_json::Value::as_str) {
            println!("{text}");
        }
        if let Some(sources) = result.get("sources") {
            if let Ok(pretty) = serde_json::to_string_pretty(sources) {
                eprintln!("sources:\n{pretty}");
            }
        }
        exit::OK
    })
}
