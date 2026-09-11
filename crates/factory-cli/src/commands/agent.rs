//! `factory agent start|stop|status|attach|list` (design §2.2, §2.3, §7;
//! `list` is ADR 0022).

use std::path::Path;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::exit;
use crate::rpc::{self, field_str};

pub fn start(root: &Path, scope: Uuid, agent_name: &str, workspace: Option<&Path>) -> i32 {
    let session_id = crate::ids::new_id();
    let mut payload = json!({
        "session_id": session_id.to_string(),
        "agent_name": agent_name,
    });
    if let Some(workspace) = workspace {
        payload["workspace_path"] = json!(workspace.display().to_string());
    }

    let outcome = rpc::command(root, scope, "agent.start", payload);
    rpc::report(root, outcome, |result| {
        let state = field_str(&result, "state", "?").to_string();
        rpc::print_result(&format!("factory: session {session_id} {state}"), &result);
        exit::OK
    })
}

pub fn stop(root: &Path, session: Uuid, reason: Option<&str>) -> i32 {
    let mut payload = json!({ "session_id": session.to_string() });
    if let Some(reason) = reason {
        payload["reason"] = json!(reason);
    }

    let outcome = rpc::command(root, Uuid::nil(), "agent.stop", payload);
    rpc::report(root, outcome, |result| {
        let state = field_str(&result, "state", "?").to_string();
        rpc::print_result(&format!("factory: session {session} {state}"), &result);
        exit::OK
    })
}

pub fn status(root: &Path, scope: Uuid, session: Option<Uuid>) -> i32 {
    let mut payload = json!({});
    if let Some(session) = session {
        payload["session_id"] = json!(session.to_string());
    }

    let outcome = rpc::query(root, scope, "agent.status", payload);
    rpc::report(root, outcome, |result| {
        rpc::print_result("factory: agent status", &result);
        exit::OK
    })
}

/// The daemon says what to run; this process execs it, because the daemon
/// cannot hand an operator's terminal to a harness from inside a worker
/// thread (decision 9).
pub fn attach(root: &Path, session: Uuid) -> i32 {
    let outcome = rpc::query(
        root,
        Uuid::nil(),
        "agent.attach_command",
        json!({ "session_id": session.to_string() }),
    );
    match outcome {
        rpc::RpcOutcome::Ok(result) => exec_argv(&result, session),
        other => rpc::report(root, other, |_| exit::OK),
    }
}

/// `factory agent list`. Exactly one of `session`/`scope` must already be
/// `Some` by the time this is called — `dispatch` resolves `--scope` to an
/// id first, and the daemon (`ops::agent::resolve_caller`) is the one place
/// that actually refuses `(None, None)` or `(Some, Some)`, so this function
/// simply forwards whichever the operator gave.
pub fn list(root: &Path, session: Option<Uuid>, scope: Option<Uuid>) -> i32 {
    let mut payload = json!({});
    if let Some(session) = session {
        payload["session_id"] = json!(session.to_string());
    }
    if let Some(scope) = scope {
        payload["scope_id"] = json!(scope.to_string());
    }

    // The envelope's own `scope_id` is unused by `agent.list` (`lib.rs`'s
    // own call-out on the decision-9 table) — the caller is named entirely
    // by the payload above, so `Uuid::nil()` here is filler, not a claim.
    let outcome = rpc::query(root, Uuid::nil(), "agent.list", payload);
    rpc::report(root, outcome, |result| {
        let count = result
            .get("scopes")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        rpc::print_result(&format!("factory: {count} scope(s)"), &result);
        exit::OK
    })
}

fn exec_argv(result: &serde_json::Value, session: Uuid) -> i32 {
    let Some(argv) = result.get("argv").and_then(serde_json::Value::as_array) else {
        eprintln!("factory: daemon did not return an argv to attach with");
        return exit::GENERIC_ERROR;
    };
    let argv: Vec<&str> = argv.iter().filter_map(serde_json::Value::as_str).collect();
    let Some((program, args)) = argv.split_first() else {
        eprintln!("factory: daemon returned an empty argv for session {session}");
        return exit::GENERIC_ERROR;
    };

    use std::os::unix::process::CommandExt;
    // `exec` replaces this process image; it only returns on failure.
    let error = std::process::Command::new(program).args(args).exec();
    eprintln!("factory: could not exec `{program}`: {error}");
    exit::GENERIC_ERROR
}
