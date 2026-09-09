//! Integration tests against the real `factory` binary (`env!("CARGO_BIN_EXE_factory")`):
//! genuine process exit codes and genuine stdout/stderr, not just the
//! parser. Every root is a fresh `tempfile::TempDir` — never
//! `/Users/factory/business-factory`.
//!
//! A handful of tests need a daemon to talk to without depending on
//! `factory-adapter` (this crate cannot — see `lib.rs`'s decision 6) or
//! driving a real Herdr pane (forbidden outright). [`spawn_fake_daemon`] is a
//! minimal stand-in: a `UnixListener` bound at `<root>/.factory/factory.sock`
//! that reads one `factory.command/v1` or `factory.query/v1` line per
//! connection (matching `factory_daemon::client::call`'s own one-shot
//! connection pattern) and writes back a canned response, recording every
//! request it saw so a test can assert on the exact sequence of operations
//! the CLI issued.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tempfile::TempDir;

use factory_cli::exit;

fn factory_cmd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_factory"))
}

const NIL_SCOPE: &str = "00000000-0000-0000-0000-000000000000";

// --- `factory init` --------------------------------------------------------

#[test]
fn init_twice_on_the_same_root_is_idempotent() {
    let dir = TempDir::new().unwrap();

    let first = factory_cmd()
        .args(["--root"])
        .arg(dir.path())
        .arg("init")
        .output()
        .unwrap();
    assert!(first.status.success(), "first init failed: {first:?}");

    let config_path = dir.path().join(".factory").join("config.yaml");
    let db_path = dir.path().join(".factory").join("factory.sqlite");
    assert!(config_path.exists(), "config.yaml must exist after init");
    assert!(db_path.exists(), "factory.sqlite must exist after init");

    let instance_id_after_first = factory_config::load(&config_path).unwrap().instance.id;

    let second = factory_cmd()
        .args(["--root"])
        .arg(dir.path())
        .arg("init")
        .output()
        .unwrap();
    assert!(
        second.status.success(),
        "second init on the same root must also succeed: {second:?}"
    );

    let instance_id_after_second = factory_config::load(&config_path).unwrap().instance.id;
    assert_eq!(
        instance_id_after_first, instance_id_after_second,
        "a second init must not regenerate the instance id"
    );
}

#[test]
fn init_creates_a_root_that_does_not_exist_yet() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("nested").join("company");
    assert!(!root.exists());

    let output = factory_cmd()
        .args(["--root"])
        .arg(&root)
        .arg("init")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(root.join(".factory").join("config.yaml").exists());
}

#[test]
fn init_honours_an_explicit_name() {
    let dir = TempDir::new().unwrap();
    factory_cmd()
        .args(["--root"])
        .arg(dir.path())
        .args(["init", "--name", "acme"])
        .output()
        .unwrap();

    let config = factory_config::load(dir.path().join(".factory").join("config.yaml")).unwrap();
    assert_eq!(config.instance.name, "acme");
}

// --- `factory doctor` -------------------------------------------------------

#[test]
fn doctor_cannot_diagnose_a_root_that_was_never_initialized() {
    let dir = TempDir::new().unwrap();
    let output = factory_cmd()
        .args(["--root"])
        .arg(dir.path())
        .arg("doctor")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(exit::DOCTOR_CANT_DIAGNOSE));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("factory init"),
        "should suggest init: {stderr}"
    );
}

#[test]
fn doctor_reports_a_finding_and_exits_non_zero_on_invalid_config() {
    let dir = TempDir::new().unwrap();
    factory_cmd()
        .args(["--root"])
        .arg(dir.path())
        .arg("init")
        .output()
        .unwrap();

    // Corrupt the config after init: `factory_config::parse` rejects an
    // empty file, which is a deterministic, host-independent way to force
    // exactly one finding (`Finding::ConfigInvalid`) regardless of whether
    // `herdr`/`launchctl` are present on this machine.
    std::fs::write(dir.path().join(".factory").join("config.yaml"), "").unwrap();

    let output = factory_cmd()
        .args(["--root"])
        .arg(dir.path())
        .arg("doctor")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(exit::DOCTOR_FINDINGS));
    let stdout = String::from_utf8_lossy(&output.stdout);
    // An invalid config also stops the registry check from running (it
    // needs a valid config), which is itself a reported `CheckSkipped`
    // finding — so this asserts "at least the one we forced", not an exact
    // count.
    assert!(stdout.contains("config invalid"), "stdout: {stdout}");
}

#[test]
fn doctor_exit_code_always_matches_whether_it_printed_any_findings() {
    // Deliberately does not assume a clean host: some checks here
    // (scheduler, herdr) read real system state this test does not control.
    // What must hold regardless is the mapping this crate owns: `exit == 0`
    // if and only if the report said `findings: none`.
    let dir = TempDir::new().unwrap();
    factory_cmd()
        .args(["--root"])
        .arg(dir.path())
        .arg("init")
        .output()
        .unwrap();

    let output = factory_cmd()
        .args(["--root"])
        .arg(dir.path())
        .arg("doctor")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.contains("findings: none") {
        assert_eq!(output.status.code(), Some(exit::OK), "stdout: {stdout}");
    } else {
        assert_ne!(output.status.code(), Some(exit::OK), "stdout: {stdout}");
    }
}

// --- absence of out-of-scope commands ---------------------------------------

#[test]
fn out_of_scope_commands_do_not_exist() {
    for name in ["secret", "schedule", "knowledge", "memory"] {
        let output = factory_cmd().arg(name).output().unwrap();
        assert!(!output.status.success());
        assert_eq!(output.status.code(), Some(2), "{name}: {output:?}");
    }
}

#[test]
fn agent_list_does_not_exist() {
    let output = factory_cmd().args(["agent", "list"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
}

// --- no daemon running -------------------------------------------------------

#[test]
fn command_with_no_daemon_running_gives_an_actionable_message_not_a_panic() {
    let dir = TempDir::new().unwrap();
    let output = factory_cmd()
        .args(["--root"])
        .arg(dir.path())
        .arg("status")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(exit::DAEMON_NOT_RUNNING));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not running"), "stderr: {stderr}");
    assert!(
        stderr.contains("factory start --root"),
        "message must name how to start the daemon: {stderr}"
    );
}

#[test]
fn stop_on_an_already_stopped_daemon_is_success() {
    let dir = TempDir::new().unwrap();
    let output = factory_cmd()
        .args(["--root"])
        .arg(dir.path())
        .arg("stop")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(exit::OK));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("not running"), "stdout: {stdout}");
}

// --- `factory daemon run` reports its missing dependency, honestly ----------

#[test]
fn daemon_run_refuses_a_harness_that_has_no_adapter() {
    let dir = TempDir::new().unwrap();
    factory_cmd()
        .args(["--root"])
        .arg(dir.path())
        .arg("init")
        .output()
        .unwrap();

    // `opencode` is a harness `factory-config` accepts (backlog §1 widened the
    // list beyond design §2.2 because `model-lab` really runs it), but version
    // 1 ships adapters only for `pi` and `claude-code`. A daemon told to serve
    // it must say so and stop, not start and then fail on the first session.
    let output = factory_cmd()
        .args(["--root"])
        .arg(dir.path())
        .args(["daemon", "run", "--harness", "opencode"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(exit::GENERIC_ERROR));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no adapter"),
        "the message must say the harness has no adapter: {stderr}"
    );
}

#[test]
fn daemon_run_refuses_an_uninitialized_root() {
    let dir = TempDir::new().unwrap();
    let output = factory_cmd()
        .args(["--root"])
        .arg(dir.path())
        .args(["daemon", "run"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(exit::GENERIC_ERROR));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("factory init"), "stderr: {stderr}");
}

// --- root discovery walks up from a subdirectory ----------------------------

#[test]
fn root_discovery_walks_up_from_a_workspace_subdirectory() {
    // Exactly the shape `factory task done` runs in: an agent's cwd is a
    // descendant of the instance root, not the root itself.
    let dir = TempDir::new().unwrap();
    factory_cmd()
        .args(["--root"])
        .arg(dir.path())
        .arg("init")
        .output()
        .unwrap();

    let workspace = dir.path().join("scope-a").join("nested");
    std::fs::create_dir_all(&workspace).unwrap();

    // No `--root` at all: discovery must walk up from `workspace` and find
    // `dir.path()/.factory`.
    let output = factory_cmd()
        .current_dir(&workspace)
        .arg("status")
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(exit::DAEMON_NOT_RUNNING),
        "must have found the root (a different failure means discovery did not walk up): {output:?}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(dir.path().to_str().unwrap()),
        "suggested `factory start` must name the discovered root, not the cwd: {stderr}"
    );
}

// --- fake-daemon-backed tests -----------------------------------------------

/// A minimal stand-in daemon: one `UnixListener` at `<root>/.factory/factory.sock`,
/// one line in, one canned line out, every request line recorded. See the
/// module docs for why this exists instead of a real `factory_daemon::Daemon`.
struct FakeDaemon {
    requests: Arc<Mutex<Vec<Value>>>,
}

impl FakeDaemon {
    fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }
}

/// What one canned response should be: a success result, or a full
/// `factory.error/v1` error body.
enum FakeResponse {
    Ok(Value),
    Err {
        code: &'static str,
        message: &'static str,
    },
}

fn spawn_fake_daemon(
    root: &Path,
    mut respond: impl FnMut(&Value) -> FakeResponse + Send + 'static,
) -> FakeDaemon {
    let factory_dir = root.join(".factory");
    std::fs::create_dir_all(&factory_dir).unwrap();
    let socket_path = factory_dir.join("factory.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();

    let requests = Arc::new(Mutex::new(Vec::new()));
    let requests_for_thread = Arc::clone(&requests);

    // The test's own `TempDir` (held by the caller for the test's duration)
    // keeps the socket path valid; this thread is simply abandoned when the
    // test process exits, exactly like `factory_daemon::client`'s own
    // `fake_server_once` test helper leaks its listener's directory.
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                let read = reader.read_line(&mut line).unwrap_or(0);
                if read == 0 {
                    break;
                }
                let Ok(request): Result<Value, _> = serde_json::from_str(&line) else {
                    break;
                };
                requests_for_thread.lock().unwrap().push(request.clone());

                let request_id = request.get("request_id").cloned().unwrap_or(Value::Null);
                let response = match respond(&request) {
                    FakeResponse::Ok(result) => json!({
                        "api": "factory.response/v1",
                        "request_id": request_id,
                        "event_cursor": 1,
                        "result": result,
                    }),
                    FakeResponse::Err { code, message } => json!({
                        "api": "factory.error/v1",
                        "request_id": request_id,
                        "error": { "code": code, "message": message, "retryable": false, "details": {} },
                    }),
                };
                let line = serde_json::to_string(&response).unwrap();
                if writeln!(stream, "{line}").is_err() {
                    break;
                }
            }
        }
    });

    FakeDaemon { requests }
}

/// A fake daemon whose every response is a `factory.error/v1` envelope.
fn spawn_fake_error_daemon(root: &Path, code: &'static str, message: &'static str) -> FakeDaemon {
    spawn_fake_daemon(root, move |_request| FakeResponse::Err { code, message })
}

fn op_name(request: &Value) -> &str {
    request
        .get("command")
        .or_else(|| request.get("query"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

#[test]
fn wait_timing_out_leaves_the_task_untouched_and_names_it() {
    let dir = TempDir::new().unwrap();

    let daemon = spawn_fake_daemon(dir.path(), |request| {
        FakeResponse::Ok(match op_name(request) {
            "task.send" => {
                let task_id = request["payload"]["task_id"].clone();
                json!({
                    "task_id": task_id,
                    "status": "queued",
                    "assignment": {"kind": "deferred", "reason": {"kind": "no_idle_session", "agent_name": "demo"}},
                    "delivery": Value::Null,
                })
            }
            "task.wait" => {
                let task_id = request["payload"]["task_id"].clone();
                // Never terminal, never blocked: the CLI must eventually give
                // up on its own deadline rather than wait forever.
                json!({ "task_id": task_id, "status": "queued", "timed_out": true })
            }
            _ => json!({}),
        })
    });

    let output = factory_cmd()
        .args(["--root"])
        .arg(dir.path())
        .args([
            "task",
            "send",
            "--scope",
            NIL_SCOPE,
            "--prompt",
            "hello",
            "--agent-name",
            "demo",
            "--wait",
            "--timeout",
            "1",
        ])
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(exit::TASK_WAIT_TIMEOUT),
        "{output:?}"
    );

    let requests = daemon.requests();
    assert!(!requests.is_empty());
    assert_eq!(op_name(&requests[0]), "task.send");
    let task_id = requests[0]["payload"]["task_id"]
        .as_str()
        .unwrap()
        .to_string();

    // "Waiting is a read": every request after the initial send must be
    // `task.wait`, never a cancel, block, or a second send.
    for request in &requests[1..] {
        assert_eq!(
            op_name(request),
            "task.wait",
            "no mutation may follow a timed-out wait: {requests:?}"
        );
        assert_eq!(request["payload"]["task_id"].as_str().unwrap(), task_id);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&task_id),
        "stderr must name the task id: {stderr}"
    );
}

#[test]
fn task_send_envelope_matches_decision_9s_payload_table() {
    let dir = TempDir::new().unwrap();
    let daemon = spawn_fake_daemon(dir.path(), |request| {
        let task_id = request["payload"]["task_id"].clone();
        FakeResponse::Ok(json!({
            "task_id": task_id,
            "status": "queued",
            "assignment": {"kind": "deferred", "reason": {"kind": "no_idle_session", "agent_name": "demo"}},
            "delivery": Value::Null,
        }))
    });

    let output = factory_cmd()
        .args(["--root"])
        .arg(dir.path())
        .args([
            "task",
            "send",
            "--scope",
            NIL_SCOPE,
            "--prompt",
            "hello",
            "--agent-name",
            "demo",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");

    let requests = daemon.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request["api"], "factory.command/v1");
    assert_eq!(request["command"], "task.send");
    assert_eq!(request["scope_id"], NIL_SCOPE);
    assert!(request["payload"]["task_id"].is_string());
    assert_eq!(request["payload"]["prompt"], "hello");
    assert_eq!(request["payload"]["agent_name"], "demo");
    // No stray fields this station's ADR 0003 §2 amendment forbids.
    assert!(request.get("idempotency_key").is_none());
}

#[test]
fn agent_start_mints_a_session_id_and_reports_the_returned_state() {
    let dir = TempDir::new().unwrap();
    let daemon = spawn_fake_daemon(dir.path(), |request| {
        let session_id = request["payload"]["session_id"].clone();
        FakeResponse::Ok(json!({
            "session_id": session_id,
            "state": "starting",
            "pane": Value::Null,
            "confidence": Value::Null,
            "harness_session_id": Value::Null,
        }))
    });

    let output = factory_cmd()
        .args(["--root"])
        .arg(dir.path())
        .args([
            "agent",
            "start",
            "--scope",
            NIL_SCOPE,
            "--agent-name",
            "demo",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");

    let requests = daemon.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["command"], "agent.start");
    let session_id = requests[0]["payload"]["session_id"].as_str().unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(session_id),
        "stdout must name the minted session id: {stdout}"
    );
    assert!(stdout.contains("starting"), "stdout: {stdout}");
}

#[test]
fn remote_error_is_rendered_and_exits_distinctly() {
    let dir = TempDir::new().unwrap();
    let _daemon = spawn_fake_error_daemon(dir.path(), "not_found.task", "no such task");

    let output = factory_cmd()
        .args(["--root"])
        .arg(dir.path())
        .args(["task", "show", "--task-id", &uuid::Uuid::nil().to_string()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(exit::REMOTE_ERROR), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not_found.task"), "stderr: {stderr}");
}
