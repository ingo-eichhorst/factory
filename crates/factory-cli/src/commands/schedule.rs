//! `factory schedule create|list|enable|disable` (design §7, §11; ADR 0021
//! decision 11).
//!
//! Every flag this module collects is sent straight through, whichever
//! combination the operator gave — the daemon owns deciding which
//! combination is legal (ADR 0014: the CLI is a client). See
//! `factory_daemon::ops::schedule`'s own module docs for the refusal this
//! module deliberately does not duplicate.

use std::path::Path;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::exit;
use crate::rpc::{self, field_str};

#[allow(clippy::too_many_arguments)]
pub fn create(
    root: &Path,
    scope: Uuid,
    template: Option<&str>,
    task: Option<&str>,
    name: Option<&str>,
    cron: &str,
    tz: &str,
    agent: Option<&str>,
    acceptance: Option<&str>,
) -> i32 {
    let schedule_id = crate::ids::new_id();
    let mut payload = json!({
        "schedule_id": schedule_id.to_string(),
        "cron": cron,
        "timezone": tz,
    });
    if let Some(template) = template {
        payload["template_name"] = json!(template);
    }
    // Minted here, not by the daemon: `--task`/`--name`'s new template gets
    // its own id up front, mirroring `task send`'s own `task_id` — a caller
    // can log or chain on it immediately, without a second read.
    if task.is_some() || name.is_some() {
        payload["template_id"] = json!(crate::ids::new_id().to_string());
    }
    if let Some(task) = task {
        payload["task"] = json!(task);
    }
    if let Some(name) = name {
        payload["name"] = json!(name);
    }
    if let Some(agent) = agent {
        payload["agent_name"] = json!(agent);
    }
    if let Some(acceptance) = acceptance {
        payload["acceptance_criteria"] = json!(acceptance);
    }

    let outcome = rpc::command(root, scope, "schedule.create", payload);
    rpc::report(root, outcome, |result| {
        let id = field_str(&result, "id", &schedule_id.to_string()).to_string();
        rpc::print_result(&format!("factory: schedule {id} created"), &result);
        exit::OK
    })
}

pub fn list(root: &Path) -> i32 {
    let outcome = rpc::query(root, Uuid::nil(), "schedule.list", json!({}));
    rpc::report(root, outcome, |result| {
        let count = result
            .get("schedules")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        rpc::print_result(&format!("factory: {count} schedule(s)"), &result);
        exit::OK
    })
}

pub fn enable(root: &Path, schedule_id: Uuid) -> i32 {
    let outcome = rpc::command(
        root,
        Uuid::nil(),
        "schedule.enable",
        json!({ "schedule_id": schedule_id.to_string() }),
    );
    rpc::report(root, outcome, |result| {
        rpc::print_result(&format!("factory: schedule {schedule_id} enabled"), &result);
        exit::OK
    })
}

pub fn disable(root: &Path, schedule_id: Uuid) -> i32 {
    let outcome = rpc::command(
        root,
        Uuid::nil(),
        "schedule.disable",
        json!({ "schedule_id": schedule_id.to_string() }),
    );
    rpc::report(root, outcome, |result| {
        rpc::print_result(
            &format!("factory: schedule {schedule_id} disabled"),
            &result,
        );
        exit::OK
    })
}
