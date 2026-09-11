//! One place that builds a request envelope, calls the daemon
//! (`factory_daemon::client::call`), and turns whatever comes back into
//! either a JSON result or an actionable outcome every command renders the
//! same way.
//!
//! Binding decision 7 (crate root docs): detecting whether the daemon is
//! running is always this module sending `daemon.status` over the socket
//! ([`ping`]), never a probe of the installation lock.

use std::path::{Path, PathBuf};

use serde_json::Value;
use uuid::Uuid;

use crate::exit;
use crate::ids;

/// What came back from one request to the daemon.
pub enum RpcOutcome {
    Ok(Value),
    /// Nothing is listening at the socket — a routine condition, not a
    /// crash. Carries the socket path so a caller can render `factory
    /// start`'s suggestion against it.
    NotRunning(PathBuf),
    /// The daemon rejected the request, or the transport itself failed after
    /// connecting (a malformed response, an I/O error mid-read). Already
    /// rendered to a message.
    Error(String),
}

fn send(root: &Path, request: factory_daemon::Request) -> RpcOutcome {
    let socket = factory_daemon::socket_path(root);
    match factory_daemon::call(&socket, &request) {
        Ok(success) => RpcOutcome::Ok(success.result),
        Err(factory_daemon::ClientError::NotRunning(path)) => RpcOutcome::NotRunning(path),
        // Accepted, then closed unanswered: a daemon on its way down. Read as
        // "not running" rather than as an error, so a successful `factory
        // stop` does not report a failure on its way out.
        Err(factory_daemon::ClientError::ClosedWithoutResponse) => RpcOutcome::NotRunning(socket),
        // `remote.details` is not echoed here. It stays on the wire — a
        // caller reading `factory_daemon::client::call`'s own return value
        // directly still gets it, in `RemoteError::details` — but a refusal
        // that already explains itself in `message` (station 12's own
        // `conflict.ambiguous_scope_name`, among others) does not need that
        // structured data restated as a raw JSON blob on the human-facing
        // line: it only repeats what the sentence already said and shows a
        // person internals they cannot use.
        Err(factory_daemon::ClientError::Remote(remote)) => {
            RpcOutcome::Error(format!("{}: {}", remote.code, remote.message))
        }
        Err(other) => RpcOutcome::Error(other.to_string()),
    }
}

/// Issue a command (may change state). `scope_id` is the scope the
/// operation concerns; pass [`Uuid::nil`] when the operation ignores it
/// (decision 9's payload table names which ones read it).
pub fn command(root: &Path, scope_id: Uuid, name: &str, payload: Value) -> RpcOutcome {
    send(
        root,
        factory_daemon::Request::Command(factory_daemon::CommandRequest {
            request_id: ids::new_id(),
            scope_id,
            command: name.to_string(),
            payload,
            expected_revision: None,
        }),
    )
}

/// Issue a query (read only).
pub fn query(root: &Path, scope_id: Uuid, name: &str, payload: Value) -> RpcOutcome {
    send(
        root,
        factory_daemon::Request::Query(factory_daemon::QueryRequest {
            request_id: ids::new_id(),
            scope_id,
            query: name.to_string(),
            payload,
        }),
    )
}

/// Whether the daemon at `root` is listening right now — always a socket
/// call ([`send`] via [`query`]'s `daemon.status`), never a lock probe.
pub enum Liveness {
    Running,
    NotRunning,
    /// Something answered, or something failed, in a way that is neither of
    /// the above — worth surfacing rather than folding into "not running".
    Unclear(String),
}

pub fn ping(root: &Path) -> Liveness {
    match query(root, Uuid::nil(), "daemon.status", serde_json::json!({})) {
        RpcOutcome::Ok(_) => Liveness::Running,
        RpcOutcome::NotRunning(_) => Liveness::NotRunning,
        RpcOutcome::Error(message) => Liveness::Unclear(message),
    }
}

/// The standard rendering for "the daemon is not running": named once so
/// every command suggests the same fix.
pub fn not_running_message(root: &Path, socket: &Path) -> String {
    format!(
        "factory: the daemon is not running (no listener at {})\n  help: start it with `factory start --root {}`",
        socket.display(),
        root.display()
    )
}

/// The standard handling for an [`RpcOutcome`]: render `Ok` through
/// `on_ok`, or print an actionable message and return the matching exit
/// code.
pub fn report(root: &Path, outcome: RpcOutcome, on_ok: impl FnOnce(Value) -> i32) -> i32 {
    match outcome {
        RpcOutcome::Ok(value) => on_ok(value),
        RpcOutcome::NotRunning(socket) => {
            eprintln!("{}", not_running_message(root, &socket));
            exit::DAEMON_NOT_RUNNING
        }
        RpcOutcome::Error(message) => {
            eprintln!("factory: {message}");
            exit::REMOTE_ERROR
        }
    }
}

/// Print a one-line headline (stable ids, state) followed by the full JSON
/// result, so a human reads the headline and a script reads the JSON.
pub fn print_result(headline: &str, result: &Value) {
    println!("{headline}");
    if let Ok(pretty) = serde_json::to_string_pretty(result) {
        println!("{pretty}");
    }
}

/// `result.field` as a string, or `default` when the field is missing or not
/// a string — every response field this crate reads for a headline is
/// optional to the *renderer*, even though the wire contract makes it
/// required, so a future daemon change degrades to a duller headline rather
/// than a panic.
pub fn field_str<'a>(result: &'a Value, field: &str, default: &'a str) -> &'a str {
    result.get(field).and_then(Value::as_str).unwrap_or(default)
}
