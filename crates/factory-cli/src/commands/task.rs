//! `factory task send|cancel|done|fail|block|resume|list|show` (design §2.4,
//! §5, §7), station 11 gap 2's `assign|progress|decision|verify`, and
//! station 11 gap 3's `rework` (§12.2).

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use uuid::Uuid;

use crate::exit;
use crate::rpc::{self, RpcOutcome, field_str};

/// `task.wait`'s own default and maximum (see `factory_daemon`'s
/// `ops::task` module) — mirrored here as this command's own default so a
/// caller who never asks for `--wait` at all sees the same number if they
/// ever do.
const DEFAULT_WAIT_TIMEOUT_SECS: u64 = 30;
/// The daemon caps any single `task.wait` call at 300s (`MAX_WAIT_TIMEOUT_MS`
/// there). A `--timeout` above that is honoured by chunking, never silently
/// truncated — silently meaning less than what the operator asked for is
/// exactly the silent default this codebase's own error corpus forbids.
const SERVER_MAX_WAIT_MS: u64 = 300_000;

#[allow(clippy::too_many_arguments)]
pub fn send(
    root: &Path,
    scope: Uuid,
    prompt: &str,
    agent_name: Option<&str>,
    target_session: Option<Uuid>,
    workspace: Option<&Path>,
    sender_session: Option<Uuid>,
    wait: bool,
    timeout_secs: Option<u64>,
) -> i32 {
    let task_id = crate::ids::new_id();
    let mut payload = json!({
        "task_id": task_id.to_string(),
        "prompt": prompt,
    });
    if let Some(agent_name) = agent_name {
        payload["agent_name"] = json!(agent_name);
    }
    if let Some(target_session) = target_session {
        payload["target_session_id"] = json!(target_session.to_string());
    }
    if let Some(workspace) = workspace {
        payload["target_workspace_path"] = json!(workspace.display().to_string());
    }
    if let Some(sender_session) = sender_session {
        payload["sender_session_id"] = json!(sender_session.to_string());
    }

    let outcome = rpc::command(root, scope, "task.send", payload);
    let sent_ok = match &outcome {
        RpcOutcome::Ok(result) => {
            let status = field_str(result, "status", "?").to_string();
            rpc::print_result(&format!("factory: task {task_id} {status}"), result);
            true
        }
        _ => false,
    };
    if !sent_ok {
        return rpc::report(root, outcome, |_| exit::OK);
    }

    if !wait {
        return exit::OK;
    }
    wait_for_task(root, task_id, timeout_secs)
}

/// Poll `task.wait` in chunks no larger than the daemon's own maximum until
/// either the task is terminal/`blocked`, or this command's own deadline
/// (independent of and possibly longer than any one chunk) passes. Each
/// chunk is itself a read on the daemon side — see `factory_daemon`'s own
/// `ops::task::wait` docs — so nothing here ever mutates the task; a timeout
/// is reported, never turned into a cancel or a resend.
fn wait_for_task(root: &Path, task_id: Uuid, timeout_secs: Option<u64>) -> i32 {
    let deadline =
        Instant::now() + Duration::from_secs(timeout_secs.unwrap_or(DEFAULT_WAIT_TIMEOUT_SECS));

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            eprintln!(
                "factory: task {task_id} did not finish in time\n  help: check it later with \
                 `factory task show --task-id {task_id}`"
            );
            return exit::TASK_WAIT_TIMEOUT;
        }
        let chunk_ms = u64::try_from(remaining.as_millis())
            .unwrap_or(SERVER_MAX_WAIT_MS)
            .clamp(1, SERVER_MAX_WAIT_MS);

        let outcome = rpc::query(
            root,
            Uuid::nil(),
            "task.wait",
            json!({ "task_id": task_id.to_string(), "timeout_ms": chunk_ms }),
        );
        match outcome {
            RpcOutcome::Ok(result) => {
                let timed_out = result
                    .get("timed_out")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !timed_out {
                    let status = field_str(&result, "status", "?").to_string();
                    print_final(root, task_id, &status);
                    return exit::OK;
                }
                // Still not terminal; loop and re-check our own deadline
                // against a fresh chunk.
            }
            other => return rpc::report(root, other, |_| exit::OK),
        }
    }
}

fn print_final(root: &Path, task_id: Uuid, status: &str) {
    match rpc::query(
        root,
        Uuid::nil(),
        "task.show",
        json!({ "task_id": task_id.to_string() }),
    ) {
        RpcOutcome::Ok(result) => {
            rpc::print_result(&format!("factory: task {task_id} {status}"), &result)
        }
        _ => println!("factory: task {task_id} {status}"),
    }
}

pub fn cancel(root: &Path, task_id: Uuid) -> i32 {
    let outcome = rpc::command(
        root,
        Uuid::nil(),
        "task.cancel",
        json!({ "task_id": task_id.to_string() }),
    );
    rpc::report(root, outcome, |result| {
        let status = field_str(&result, "status", "?").to_string();
        rpc::print_result(&format!("factory: task {task_id} {status}"), &result);
        exit::OK
    })
}

pub fn done(root: &Path, task_id: Uuid, summary: Option<&str>, artifacts: &[String]) -> i32 {
    let mut payload = json!({ "task_id": task_id.to_string() });
    if let Some(summary) = summary {
        payload["result_summary"] = json!(summary);
    }
    if !artifacts.is_empty() {
        payload["result_artifact_paths"] = json!(artifacts);
    }
    let outcome = rpc::command(root, Uuid::nil(), "task.done", payload);
    rpc::report(root, outcome, |result| {
        rpc::print_result(&format!("factory: task {task_id} done"), &result);
        exit::OK
    })
}

pub fn fail(root: &Path, task_id: Uuid, summary: Option<&str>, artifacts: &[String]) -> i32 {
    let mut payload = json!({ "task_id": task_id.to_string() });
    if let Some(summary) = summary {
        payload["result_summary"] = json!(summary);
    }
    if !artifacts.is_empty() {
        payload["result_artifact_paths"] = json!(artifacts);
    }
    let outcome = rpc::command(root, Uuid::nil(), "task.fail", payload);
    rpc::report(root, outcome, |result| {
        rpc::print_result(&format!("factory: task {task_id} failed"), &result);
        exit::OK
    })
}

pub fn block(root: &Path, task_id: Uuid, reason: &str) -> i32 {
    let outcome = rpc::command(
        root,
        Uuid::nil(),
        "task.block",
        json!({ "task_id": task_id.to_string(), "reason": reason }),
    );
    rpc::report(root, outcome, |result| {
        rpc::print_result(&format!("factory: task {task_id} blocked"), &result);
        exit::OK
    })
}

pub fn resume(root: &Path, task_id: Uuid) -> i32 {
    let outcome = rpc::command(
        root,
        Uuid::nil(),
        "task.resume",
        json!({ "task_id": task_id.to_string() }),
    );
    rpc::report(root, outcome, |result| {
        rpc::print_result(&format!("factory: task {task_id} queued"), &result);
        exit::OK
    })
}

pub fn assign(root: &Path, task_id: Uuid, agent_name: &str) -> i32 {
    let outcome = rpc::command(
        root,
        Uuid::nil(),
        "task.assign",
        json!({ "task_id": task_id.to_string(), "agent_name": agent_name }),
    );
    rpc::report(root, outcome, |result| {
        rpc::print_result(
            &format!("factory: task {task_id} assignment recorded"),
            &result,
        );
        exit::OK
    })
}

pub fn progress(root: &Path, task_id: Uuid, note: &str) -> i32 {
    let outcome = rpc::command(
        root,
        Uuid::nil(),
        "task.progress",
        json!({ "task_id": task_id.to_string(), "note": note }),
    );
    rpc::report(root, outcome, |result| {
        rpc::print_result(
            &format!("factory: task {task_id} progress recorded"),
            &result,
        );
        exit::OK
    })
}

#[allow(clippy::too_many_arguments)]
pub fn decision(
    root: &Path,
    task_id: Uuid,
    decision: &str,
    rationale: &str,
    alternatives: Option<&str>,
    consequences: Option<&str>,
) -> i32 {
    let decision_id = crate::ids::new_id();
    let mut payload = json!({
        "decision_id": decision_id.to_string(),
        "task_id": task_id.to_string(),
        "decision": decision,
        "rationale": rationale,
    });
    if let Some(alternatives) = alternatives {
        payload["alternatives"] = json!(alternatives);
    }
    if let Some(consequences) = consequences {
        payload["consequences"] = json!(consequences);
    }

    let outcome = rpc::command(root, Uuid::nil(), "task.decision", payload);
    rpc::report(root, outcome, |result| {
        rpc::print_result(
            &format!("factory: task {task_id} decision recorded"),
            &result,
        );
        exit::OK
    })
}

pub fn verify(root: &Path, task_id: Uuid, verdict: &str, note: Option<&str>) -> i32 {
    let mut payload = json!({ "task_id": task_id.to_string(), "verdict": verdict });
    if let Some(note) = note {
        payload["note"] = json!(note);
    }

    let outcome = rpc::command(root, Uuid::nil(), "task.verify", payload);
    rpc::report(root, outcome, |result| {
        rpc::print_result(
            &format!("factory: task {task_id} verdict recorded"),
            &result,
        );
        exit::OK
    })
}

/// Station 11 gap 3: create a new run that reworks an already-finished one.
/// `task_id` (the new run's own id) is minted here, exactly like `send`'s own
/// `task_id` and `decision`'s own `decision_id` — this crate never lets the
/// daemon mint an id for a caller.
#[allow(clippy::too_many_arguments)]
pub fn rework(
    root: &Path,
    scope: Uuid,
    prompt: &str,
    reworks_task_id: Uuid,
    rework_finding: &str,
    target_session: Option<Uuid>,
    workspace: Option<&Path>,
) -> i32 {
    let task_id = crate::ids::new_id();
    let mut payload = json!({
        "task_id": task_id.to_string(),
        "prompt": prompt,
        "reworks_task_id": reworks_task_id.to_string(),
        "rework_finding": rework_finding,
    });
    if let Some(target_session) = target_session {
        payload["target_session_id"] = json!(target_session.to_string());
    }
    if let Some(workspace) = workspace {
        payload["target_workspace_path"] = json!(workspace.display().to_string());
    }

    let outcome = rpc::command(root, scope, "task.rework", payload);
    rpc::report(root, outcome, |result| {
        let status = field_str(&result, "status", "?").to_string();
        rpc::print_result(&format!("factory: task {task_id} {status}"), &result);
        exit::OK
    })
}

pub fn list(root: &Path) -> i32 {
    let outcome = rpc::query(root, Uuid::nil(), "task.list", json!({}));
    rpc::report(root, outcome, |result| {
        let count = result
            .get("tasks")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        rpc::print_result(&format!("factory: {count} task(s)"), &result);
        exit::OK
    })
}

pub fn show(root: &Path, task_id: Uuid) -> i32 {
    let outcome = rpc::query(
        root,
        Uuid::nil(),
        "task.show",
        json!({ "task_id": task_id.to_string() }),
    );
    rpc::report(root, outcome, |result| {
        let status = field_str(&result, "status", "?").to_string();
        rpc::print_result(&format!("factory: task {task_id} {status}"), &result);
        exit::OK
    })
}
