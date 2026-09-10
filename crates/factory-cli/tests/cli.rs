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
//!
//! The tests near the bottom of this file, in the "the dispatcher is
//! actually wired into `daemon run`" section, need the opposite of a fake
//! daemon: a genuine `factory daemon run` child process, because what they
//! prove — `factory_daemon::dispatch::spawn` is really called from
//! `commands/daemon.rs::serve_with`, and its own `log_failed_fires` really
//! reaches stderr — cannot be shown against a stand-in that never runs that
//! code at all.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

use factory_cli::exit;
use factory_daemon::envelope::{QueryRequest, Request};

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

// --- the dispatcher is actually wired into `daemon run` ---------------------
//
// `factory_daemon::dispatch::spawn` exists and has its own thorough unit
// suite in `factory-daemon/tests/dispatch.rs`, but nothing there proves
// `commands/daemon.rs::serve_with` actually calls it, or that a failed fire
// is actually visible to an operator once it does. A fake daemon (this
// file's usual tool, above) cannot prove either — it never runs
// `serve_with` at all. The only honest proof is a real `factory daemon run`
// process: `a_real_daemon_process_writes_dispatcher_state` below reads
// `dispatcher_state` back out of the real `factory.sqlite` file the daemon
// writes (deleting the `dispatch::spawn(...)` call from `serve_with` must
// turn it red), and `a_schedule_with_an_unreadable_timezone_is_named_on_stderr`
// reads the daemon's own stderr for the line `log_failed_fires` is supposed
// to write (deleting that call from `dispatch::spawn`'s loop must turn it
// red). See this task's report for both confirmed mutation outputs.

/// A real `factory daemon run` child process — `Child` held directly, never
/// `factory start`'s detached process group, because tearing this down
/// (including on a failed assertion) needs the child in hand. Mirrors
/// `factory-e2e`'s `daemon_process_drill.rs::DaemonProcess`, minus the
/// generations-of-restart machinery this test does not need.
struct RealDaemon {
    child: Option<Child>,
}

impl RealDaemon {
    /// Spawn `factory --root <root> daemon run --harness pi` and block until
    /// it answers `daemon.status`, or panic with its captured output if it
    /// exits first or never answers within 10s.
    ///
    /// `--harness pi` builds a `PiAdapter` over a real `HerdrCli::new("herdr")`
    /// (`commands/daemon.rs::daemon_run`). That is safe here without a live
    /// `herdr` on this machine only because a fresh root has no sessions to
    /// observe: checked directly against the source before writing this
    /// test, not assumed — `factory_daemon::startup::build_handler` makes no
    /// adapter call at all, and `observe::reconcile_once` only ever visits a
    /// `starting`/`running` session, of which `factory_session::list` returns
    /// none on a database `factory init` just created.
    fn spawn(root: &Path) -> Self {
        // `stdout`/`stderr` are piped but not read here — the readiness loop
        // below only ever needs `try_wait`. That is safe only because every
        // test using `RealDaemon` today either produces near-zero output
        // (this file's happy-path daemon) or reads its own stderr
        // incrementally as it goes (`wait_for_stderr_line`, below, for the
        // noisy-schedule test). A pipe nobody reads holds only 64 KiB before
        // a write to it blocks — a daemon stuck mid-`eprintln!` looks nothing
        // like a hung write, so a future test with a chatty daemon must drain
        // as it runs, never rely on `drain_child`'s blocking `read_to_string`
        // while the process is still alive.
        let mut child = factory_cmd()
            .args(["--root"])
            .arg(root)
            .args(["daemon", "run", "--harness", "pi"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn `factory daemon run`");

        let socket_path = root.join(".factory").join("factory.sock");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                let (out, err) = drain_child(&mut child);
                panic!(
                    "`factory daemon run` exited before it ever became ready: {status}\n\
                     --- stdout ---\n{out}\n--- stderr ---\n{err}"
                );
            }
            if daemon_answers(&socket_path) {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let (out, err) = drain_child(&mut child);
                panic!(
                    "`factory daemon run` never answered daemon.status within 10s\n\
                     --- stdout ---\n{out}\n--- stderr ---\n{err}"
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        Self { child: Some(child) }
    }

    /// Kill and reap the daemon. Consumes `self` so the same process can
    /// never be stopped twice from the normal path — [`Drop`] below is only
    /// the safety net for a test that panicked before reaching this call.
    fn stop(mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Read the daemon's stderr, one line at a time, until a line contains
    /// `needle` or `timeout` passes.
    ///
    /// The read happens on a background thread that forwards each line over
    /// a channel; this function's own deadline lives entirely in
    /// `recv_timeout` on that channel, never in the read itself. That
    /// distinction matters here specifically: `drain_child` reads to EOF,
    /// and a live daemon's stderr has no EOF until the process exits, which
    /// it never does on its own — a `read_to_string` against it would hang
    /// this test forever instead of failing it. Reading incrementally is
    /// what lets a deadline apply at all while the daemon keeps running.
    ///
    /// Takes the child's stderr handle, so call this at most once per
    /// `RealDaemon`.
    fn wait_for_stderr_line(&mut self, needle: &str, timeout: Duration) -> Option<String> {
        let stderr = self
            .child
            .as_mut()
            .expect("daemon already stopped")
            .stderr
            .take()
            .expect("stderr already taken from this daemon");

        let (line_tx, line_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if line_tx.send(line).is_err() {
                    break;
                }
            }
        });

        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match line_rx.recv_timeout(remaining) {
                Ok(line) if line.contains(needle) => return Some(line),
                Ok(_) => {}
                Err(_) => return None,
            }
        }
    }
}

impl Drop for RealDaemon {
    fn drop(&mut self) {
        // Best-effort net for the failure path: an assertion panicking
        // earlier in the test must not leave this daemon process running
        // past the test that started it.
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn drain_child(child: &mut Child) -> (String, String) {
    let mut out = String::new();
    let mut err = String::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_string(&mut out);
    }
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_string(&mut err);
    }
    (out, err)
}

/// A `daemon.status` round trip: `Ok` means something is listening and
/// speaking `factory.query/v1` at `socket_path`, `Err` covers both "nothing
/// is there yet" and any other failure — this helper only ever needs the
/// yes/no answer, never the reason.
fn daemon_answers(socket_path: &Path) -> bool {
    let request = Request::Query(QueryRequest {
        request_id: uuid::Uuid::nil(),
        scope_id: uuid::Uuid::nil(),
        query: "daemon.status".to_string(),
        payload: json!({}),
    });
    factory_daemon::client::call(socket_path, &request).is_ok()
}

/// `dispatcher_state`'s row count, through a fresh read-only connection —
/// never `factory_store::Store::open`, which would migrate a database file
/// the real daemon process already owns and is writing to concurrently.
/// `Store::open_read_only` (ADR 0018 decision 1) is documented to read a
/// WAL-mode database correctly while a writer still holds it open, and a
/// fresh connection per call (rather than one held across the whole poll
/// loop) is what guarantees each read sees the daemon's latest commit.
fn dispatcher_state_row_count(db_path: &Path) -> Result<i64, String> {
    let store = factory_store::Store::open_read_only(db_path).map_err(|e| e.to_string())?;
    store
        .connection()
        .query_row("SELECT COUNT(*) FROM dispatcher_state", [], |row| {
            row.get(0)
        })
        .map_err(|e| e.to_string())
}

/// Poll [`dispatcher_state_row_count`] until it reports at least one row or
/// `timeout` passes, returning whatever the last read was either way — a
/// deadline that expires on `Ok(0)` or on a read error are both legitimate,
/// distinct ways for the caller's assertion to fail and explain why.
fn wait_for_dispatcher_state(db_path: &Path, timeout: Duration) -> Result<i64, String> {
    let deadline = Instant::now() + timeout;
    loop {
        let outcome = dispatcher_state_row_count(db_path);
        let row_arrived = matches!(&outcome, Ok(n) if *n >= 1);
        if row_arrived || Instant::now() >= deadline {
            return outcome;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The acceptance test for station 11's defect: `dispatch::spawn` exists and
/// is unit-tested, but until `serve_with` calls it, no production daemon ever
/// runs a schedule. This starts a real daemon, over a real socket, against a
/// real `factory.sqlite`, and proves the dispatcher thread inside it actually
/// ticks.
///
/// `dispatch::spawn`'s loop runs its first tick before its first wait
/// (`dispatch.rs`'s own doc comment on `spawn`), so a real fire lands within
/// milliseconds of the daemon becoming ready — the 10s deadline here is
/// generous, not tight.
#[test]
fn a_real_daemon_process_writes_dispatcher_state() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    let init = factory_cmd()
        .args(["--root"])
        .arg(root)
        .arg("init")
        .output()
        .unwrap();
    assert!(init.status.success(), "init failed: {init:?}");

    let db_path = root.join(".factory").join("factory.sqlite");
    assert_eq!(
        dispatcher_state_row_count(&db_path),
        Ok(0),
        "sanity: no daemon has run against this fresh database yet"
    );

    let daemon = RealDaemon::spawn(root);

    let result = wait_for_dispatcher_state(&db_path, Duration::from_secs(10));
    daemon.stop();

    assert_eq!(
        result,
        Ok(1),
        "dispatcher_state must gain exactly one row once a real daemon has been running for a \
         while — Ok(0) or an Err here means the dispatcher thread was never started; see \
         `serve_with` in `commands/daemon.rs`"
    );
}

/// A fixed-format UUID from a small seed, not `uuid::Uuid::new_v4()` — this
/// workspace pins `uuid` without the `v4`/`v7` feature everywhere, and
/// `factory-daemon/tests/dispatch.rs`'s own `uid` helper mints test ids the
/// identical way.
fn uid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

/// Insert one scope, one template naming that scope, and one schedule whose
/// `timezone` no IANA database can resolve — all with plain SQL, never
/// `factory_task`, which this crate does not depend on (`lib.rs`'s own
/// decision 6). `"Not/AZone"` is not a real zone, and `* * * * *` matches
/// every minute, so the very first tick after the daemon starts must try,
/// and fail, to fire this schedule.
///
/// Confirmed by reading the source before relying on it, not assumed:
/// `factory_task::schedule::due`'s SQL only filters `enabled = 1` — it never
/// validates `timezone` — so this row survives that query and reaches
/// `schedule::matches`, whose `Tz::from_str` call is what actually rejects
/// it, landing the row in `Due::unreadable`. `dispatch::tick` then folds
/// every `Due::unreadable` entry into a `ScheduleReport` carrying
/// `FireOutcome::Failed`, which is exactly the outcome `log_failed_fires`
/// prints.
///
/// `factory init` writes `scopes: []` and never touches the `scopes` table
/// (`commands/init.rs`), so a fresh root has no scope to reuse — this
/// function writes its own, the same way `dispatch.rs`'s own
/// `create_template_with_no_scope` writes a template by hand for a state no
/// typed API will construct.
fn seed_a_schedule_with_an_unreadable_timezone(db_path: &Path) -> uuid::Uuid {
    let scope_id = uid(900);
    let template_id = uid(901);
    let schedule_id = uid(902);

    let mut store = factory_store::Store::open_at(db_path).expect("open store read-write");
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO scopes (id, name, declared_path) VALUES (?1, 'seeded', '.')",
        [scope_id.to_string()],
    )
    .expect("insert scope");
    tx.execute(
        "INSERT INTO task_templates (id, name, target_scope_id, prompt) \
         VALUES (?1, 'seeded-template', ?2, 'do it')",
        (template_id.to_string(), scope_id.to_string()),
    )
    .expect("insert template");
    tx.execute(
        "INSERT INTO schedules (id, template_id, cron, timezone, enabled) \
         VALUES (?1, ?2, '* * * * *', 'Not/AZone', 1)",
        (schedule_id.to_string(), template_id.to_string()),
    )
    .expect("insert a schedule whose timezone cannot be read");
    tx.commit().expect("commit");

    schedule_id
}

/// backlog §11's own words for the defect this closes: "today nothing would
/// reveal that a cron schedule has not fired for days." `log_failed_fires`
/// is the fix, and this is its proof — through the real binary, not a direct
/// call into `factory_daemon::dispatch`. Deleting the
/// `log_failed_fires(&reports)` call from `dispatch::spawn`'s loop must turn
/// this test red; see this task's report for the confirmed mutation output.
///
/// Asserts the line names *this* schedule's id, not merely that some text
/// was written — an operator reading `daemon.log` needs to know which row is
/// broken, not just that something is.
#[test]
fn a_schedule_with_an_unreadable_timezone_is_named_on_stderr() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    let init = factory_cmd()
        .args(["--root"])
        .arg(root)
        .arg("init")
        .output()
        .unwrap();
    assert!(init.status.success(), "init failed: {init:?}");

    let db_path = root.join(".factory").join("factory.sqlite");
    let schedule_id = seed_a_schedule_with_an_unreadable_timezone(&db_path);

    let mut daemon = RealDaemon::spawn(root);
    let line = daemon.wait_for_stderr_line(&schedule_id.to_string(), Duration::from_secs(10));
    daemon.stop();

    let line = line.unwrap_or_else(|| {
        panic!(
            "the daemon's stderr never named schedule {schedule_id} as failing to fire within \
             10s of starting — see `log_failed_fires` in `factory-daemon/src/dispatch.rs`"
        )
    });
    assert!(
        line.contains("failed to fire"),
        "the line must say the schedule failed to fire, not merely mention its id: {line:?}"
    );
}
