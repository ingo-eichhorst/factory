//! Drill 6: ADR 0014's own open item — "a daemon that has never been killed
//! and observed to come back is not known to be restartable" — proven against
//! a **real `factory-daemon` process**, not a temp-directory unit test with a
//! fake adapter standing in for the whole thing.
//!
//! This file is two things at once, selected by whether
//! `FACTORY_E2E_DAEMON_ROOT` is set in the environment:
//!
//! - Every normal `cargo test` run executes only
//!   [`a_killed_daemon_process_restarts_and_re_adopts_rather_than_cold_starting`].
//!   It is a plain `#[test]`, not `#[ignore]`d, and it is hermetic: no live
//!   Herdr, no network, nothing outside its own `tempfile::TempDir` — it
//!   belongs in `check.sh`.
//! - [`daemon_worker_process`] is `#[ignore]`d *and* gated on the same
//!   environment variable being present, so a plain `cargo test -- --ignored`
//!   run — which every live drill in this crate is invoked with — never
//!   blocks on it. The parent test above finds its own already-compiled test
//!   binary with `std::env::current_exe()` and re-executes *that binary*,
//!   filtered to just this one test, as its daemon subprocess. This needs no
//!   new dependency and no `[[bin]]` target: a `[[bin]]` target in this crate
//!   cannot see this crate's dev-dependencies at all (`factory_daemon`
//!   included) once it is built as a runnable artifact rather than a unit-test
//!   harness — measured directly against a throwaway workspace before writing
//!   this file, not assumed. Re-invoking the already-linked test binary sidesteps
//!   the whole problem.
//!
//! `factory-cli`'s `factory daemon run` does not exist yet (station 10's own
//! binary is still `eprintln!("... under construction")`), so per this
//! crate's brief this drill is written directly against `factory_daemon`
//! rather than blocked on another agent's crate.

mod common;

use std::io::Read;
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use factory_adapter::{
    Adapter, AdapterError, Confidence, Observation, PaneId, StartRequest, StartedSession,
    TaskSignal,
};
use factory_daemon::envelope::{CommandRequest, QueryRequest, Request};

const ENV_ROOT: &str = "FACTORY_E2E_DAEMON_ROOT";
const ENV_PIDFILE: &str = "FACTORY_E2E_HARNESS_PIDFILE";

// --- A deterministic stand-in for a harness adapter ------------------------

/// [`Adapter::start`] spawns a real, long-lived `sleep` process — standing in
/// for "a harness process in a Herdr pane" — and records its pid in
/// `FACTORY_E2E_HARNESS_PIDFILE`. That pid is what lets the *parent* test
/// (which never shares memory with this subprocess) check, from the outside,
/// whether the stand-in "harness" is still alive after the daemon holding it
/// has been `SIGKILL`ed — ADR 0014's second open item: "they are harness
/// processes in Herdr panes and do not die with it."
///
/// Every other method is a trivial success: this drill's subject is the
/// daemon process's own lifecycle, not adapter behaviour, which is covered
/// exhaustively elsewhere (`factory-adapter`'s own contract suite,
/// `factory-recovery`'s tests).
struct SpawnsRealProcessAdapter;

impl Adapter for SpawnsRealProcessAdapter {
    fn start(&self, req: &StartRequest) -> Result<StartedSession, AdapterError> {
        let child = Command::new("sleep")
            .arg("300")
            .spawn()
            .expect("spawn stand-in harness process");
        if let Ok(pidfile) = std::env::var(ENV_PIDFILE) {
            std::fs::write(&pidfile, child.id().to_string()).expect("record harness pid");
        }
        // `child` is intentionally never `.wait()`-ed or killed here: dropping
        // a `std::process::Child` does not kill the process it spawned (only
        // an explicit `.kill()` does), so it keeps running as an ordinary
        // orphan once this daemon stand-in itself is `SIGKILL`ed — which is
        // exactly the claim this drill exists to prove.
        drop(child);

        Ok(StartedSession {
            pane: PaneId(format!("pane-{}", req.session_id)),
            harness_session_id: Some(format!("harness-{}", req.session_id)),
            confidence: Confidence::Authoritative,
        })
    }

    fn send(
        &self,
        _pane: &PaneId,
        _task_id: uuid::Uuid,
        _prompt: &str,
    ) -> Result<(), AdapterError> {
        Ok(())
    }

    fn observe(&self, pane: &PaneId) -> Result<Observation, AdapterError> {
        Ok(Observation {
            pane: pane.clone(),
            harness_state: "idle".to_string(),
            confidence: Confidence::Unavailable,
            session_alive: true,
            task_signal: TaskSignal::NoChange,
            transcript_path: None,
            harness_session_id: None,
        })
    }

    fn interrupt(&self, _pane: &PaneId) -> Result<(), AdapterError> {
        Ok(())
    }

    fn stop(&self, _pane: &PaneId) -> Result<(), AdapterError> {
        Ok(())
    }

    fn attach_command(&self, pane: &PaneId) -> Result<Vec<String>, AdapterError> {
        Ok(vec!["echo".to_string(), pane.0.clone()])
    }

    fn runtime_version(&self) -> Result<String, AdapterError> {
        Ok("stand-in-1.0".to_string())
    }

    /// This double reports no cost data. Stated rather than omitted: the
    /// trait has no default, so an implementor cannot answer `None` by
    /// forgetting the method. ADR 0021 decision 6 makes `None` a valid
    /// answer, and it is this fake's real one.
    fn cost_sample(
        &self,
        _pane: &PaneId,
    ) -> Result<Option<factory_adapter::CostSample>, AdapterError> {
        Ok(None)
    }
}

// --- The daemon subprocess, from the *inside* ------------------------------

/// Runs only when re-invoked by
/// [`a_killed_daemon_process_restarts_and_re_adopts_rather_than_cold_starting`]
/// with [`ENV_ROOT`] set. A bare `cargo test -- --ignored` run (how every
/// other live drill in this crate is invoked) leaves the environment variable
/// unset, so this returns immediately rather than blocking forever — being
/// `#[ignore]`d alone would not be enough to guarantee that.
#[test]
#[ignore = "only meaningful when re-exec'd by this file's own parent test; see module docs"]
fn daemon_worker_process() {
    let Ok(root) = std::env::var(ENV_ROOT) else {
        return;
    };
    let root = PathBuf::from(root);

    // Crate docs decision 4: the lock is taken before the socket is ever
    // bound. A failure here (another daemon already holds it) is exactly
    // what the "a live daemon's socket is never stolen" half of this drill
    // exercises from the parent process — surfaced as a loud, readable
    // failure on this subprocess's own stderr rather than a silent hang.
    let daemon = factory_daemon::Daemon::start(&root)
        .unwrap_or_else(|e| panic!("daemon_worker_process: Daemon::start failed: {e}"));

    // ADR 0014's own open item, closed by `startup::build_handler`: a
    // restarted daemon reconciles the database *before* it ever answers a
    // request. Mutation test 1 (see this file's own report) deletes this
    // call's effect and confirms the parent test goes red.
    let handler = factory_daemon::startup::build_handler(&root, SpawnsRealProcessAdapter)
        .unwrap_or_else(|e| panic!("daemon_worker_process: build_handler failed: {e:?}"));

    eprintln!(
        "daemon_worker_process: serving on {}",
        daemon.socket_path().display()
    );
    let _ = daemon.serve(std::sync::Arc::new(handler));
}

// --- The parent test: a real process, killed, observed, restarted ---------

/// A daemon subprocess, from the parent's side: spawned, waited on for
/// readiness, and guaranteed not to be leaked (holding the installation lock
/// on a `TempDir` this test is about to delete) even if an assertion panics
/// partway through.
struct DaemonProcess {
    child: Option<Child>,
    pid: u32,
    socket_path: PathBuf,
    lock_path: PathBuf,
}

impl DaemonProcess {
    /// Spawn this very test binary, filtered to just `daemon_worker_process`,
    /// with [`ENV_ROOT`] (and, when given, [`ENV_PIDFILE`]) set — and block
    /// until `daemon.status` answers or `deadline` passes.
    fn spawn(root: &Path, harness_pidfile: Option<&Path>) -> Self {
        let exe = std::env::current_exe().expect("current_exe (this test binary)");
        let mut cmd = Command::new(&exe);
        cmd.args([
            "daemon_worker_process",
            "--exact",
            "--ignored",
            "--nocapture",
        ])
        .env(ENV_ROOT, root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        if let Some(pidfile) = harness_pidfile {
            cmd.env(ENV_PIDFILE, pidfile);
        }
        let mut child = cmd.spawn().expect("spawn daemon subprocess");
        let pid = child.id();

        let socket_path = factory_daemon::socket_path(root);
        let lock_path = factory_daemon::lock_path(root);

        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = child.try_wait().expect("try_wait") {
                let (out, err) = drain(&mut child);
                panic!(
                    "daemon subprocess exited before it ever became ready: {status:?}\n\
                     --- stdout ---\n{out}\n--- stderr ---\n{err}"
                );
            }
            if ping(&socket_path) {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let (out, err) = drain(&mut child);
                panic!(
                    "daemon subprocess never answered daemon.status within {deadline:?}\n\
                     --- stdout ---\n{out}\n--- stderr ---\n{err}",
                    deadline = Duration::from_secs(15)
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        Self {
            child: Some(child),
            pid,
            socket_path,
            lock_path,
        }
    }

    fn call(&self, request: &Request) -> factory_daemon::SuccessResponse {
        factory_daemon::client::call(&self.socket_path, request)
            .unwrap_or_else(|e| panic!("call to daemon pid {}: {e}", self.pid))
    }

    /// SIGKILL — the case ADR 0014's brief singles out as "leaves everything
    /// behind": no chance to unlink the socket, release the lock cleanly, or
    /// run any drop glue. Returns the reaped exit status, so the caller can
    /// prove death rather than assume it.
    fn sigkill_and_reap(mut self) -> std::process::ExitStatus {
        let mut child = self.child.take().expect("child already reaped");
        child.kill().expect("SIGKILL");
        child.wait().expect("wait")
    }
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        // Best-effort hygiene: a panic earlier in the test must not leave a
        // daemon process holding the installation lock on a `TempDir` this
        // test's `common::build()` is about to delete.
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn drain(child: &mut Child) -> (String, String) {
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

fn ping(socket_path: &Path) -> bool {
    let request = Request::Query(QueryRequest {
        request_id: next_request_id(),
        scope_id: uuid::Uuid::nil(),
        query: "daemon.status".to_string(),
        payload: serde_json::json!({}),
    });
    factory_daemon::client::call(socket_path, &request).is_ok()
}

/// A fresh, distinct request id for each call. This workspace pins `uuid`
/// without the `v4` feature (`common::uid`'s own doc comment on the same
/// constraint), and a request id here needs only to be distinct per call —
/// `client::call` itself is what checks a response actually answers the
/// request it was sent for — so a monotonic counter through `common::uid` is
/// simpler than reinventing a random id.
fn next_request_id() -> uuid::Uuid {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(9_000);
    common::uid(COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

fn command(scope_id: uuid::Uuid, command: &str, payload: serde_json::Value) -> Request {
    Request::Command(CommandRequest {
        request_id: next_request_id(),
        scope_id,
        command: command.to_string(),
        payload,
        expected_revision: None,
    })
}

fn query(scope_id: uuid::Uuid, query: &str, payload: serde_json::Value) -> Request {
    Request::Query(QueryRequest {
        request_id: next_request_id(),
        scope_id,
        query: query.to_string(),
        payload,
    })
}

/// The lock file's holder pid, per `lock::record_holder_pid`'s own format:
/// the plain decimal pid, nothing else.
fn holder_pid(lock_path: &Path) -> Option<u32> {
    std::fs::read_to_string(lock_path).ok()?.trim().parse().ok()
}

#[test]
fn a_killed_daemon_process_restarts_and_re_adopts_rather_than_cold_starting() {
    let inst = common::build();
    let root = inst.dir.path().to_path_buf();
    let alpha = inst.alpha;
    let gamma = inst.gamma;

    let harness_pidfile = root.join("harness.pid");

    // --- generation 1: a real daemon process, real work through its socket
    let daemon1 = DaemonProcess::spawn(&root, Some(&harness_pidfile));

    // `agent.start`: a real session, brought up through the socket exactly as
    // `factory agent start` would submit it. The stand-in adapter reports
    // `Confidence::Authoritative`, so this session is `running`, not merely
    // `starting`, by the time the call returns.
    let start_resp = daemon1.call(&command(
        alpha,
        "agent.start",
        serde_json::json!({
            "session_id": common::uid(500).to_string(),
            "agent_name": "alpha-agent",
        }),
    ));
    let session_id = common::uid(500);
    assert_eq!(
        start_resp.result["state"], "running",
        "{:?}",
        start_resp.result
    );

    // `task.send`: queued, assigned to the session just started, and
    // delivered — real work, not a seeded row.
    let task_id = common::uid(600);
    let send_resp = daemon1.call(&command(
        alpha,
        "task.send",
        serde_json::json!({
            "task_id": task_id.to_string(),
            "prompt": "do the thing",
            "agent_name": "alpha-agent",
        }),
    ));
    assert_eq!(
        send_resp.result["status"], "running",
        "{:?}",
        send_resp.result
    );
    assert_eq!(
        send_resp.result["delivery"]["sent"], true,
        "{:?}",
        send_resp.result
    );

    // A second task, targeted at a scope with no session at all: stays
    // `queued`, nothing ever delivered — ADR 0019's "no delivery attempt"
    // row, produced through the socket rather than seeded.
    let never_sent_task_id = common::uid(601);
    let deferred_resp = daemon1.call(&command(
        gamma,
        "task.send",
        serde_json::json!({
            "task_id": never_sent_task_id.to_string(),
            "prompt": "nobody home yet",
            "agent_name": "gamma-agent",
        }),
    ));
    assert_eq!(
        deferred_resp.result["status"], "queued",
        "{:?}",
        deferred_resp.result
    );
    assert_eq!(
        deferred_resp.result["assignment"]["kind"], "deferred",
        "{:?}",
        deferred_resp.result
    );

    // The stand-in harness process the adapter spawned for `session_id`.
    let deadline = Instant::now() + Duration::from_secs(5);
    let harness_pid: u32 = loop {
        if let Ok(s) = std::fs::read_to_string(&harness_pidfile) {
            if let Ok(pid) = s.trim().parse() {
                break pid;
            }
        }
        if Instant::now() >= deadline {
            panic!("stand-in harness process never recorded its pid");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(
        process_is_alive(harness_pid),
        "the stand-in harness process must be alive before the daemon is ever killed"
    );

    // --- a live daemon's socket and lock are never stolen -------------------
    let pid_before_kill = holder_pid(&daemon1.lock_path).unwrap_or_else(|| {
        panic!(
            "lock file at {:?} must record a holder pid",
            daemon1.lock_path
        )
    });
    assert_eq!(
        pid_before_kill, daemon1.pid,
        "the lock file must record the live daemon's own pid"
    );

    match factory_daemon::Daemon::start(&root) {
        Err(factory_daemon::DaemonError::Lock(factory_daemon::LockError::AlreadyLocked {
            holder_pid,
            ..
        })) => {
            assert_eq!(
                holder_pid,
                Some(daemon1.pid),
                "a second daemon must be refused and told exactly which pid holds the lock"
            );
        }
        other => panic!(
            "starting a second daemon while the first is alive must fail with AlreadyLocked, got {other:?}"
        ),
    }
    // Refusing a competitor is only half the claim; the live daemon must
    // still be answering normally — its socket was never touched by the
    // refused attempt above.
    let still_alive = daemon1.call(&query(alpha, "daemon.status", serde_json::json!({})));
    assert_eq!(
        still_alive.result["socket_path"],
        daemon1.socket_path.to_string_lossy().into_owned()
    );

    // --- the kill: SIGKILL, the case that leaves everything behind ---------
    assert!(
        UnixStream::connect(&daemon1.socket_path).is_ok(),
        "sanity: the socket must be connectable immediately before the kill"
    );
    let socket_path = daemon1.socket_path.clone();
    let lock_path = daemon1.lock_path.clone();
    let pid1 = daemon1.pid;
    let status = daemon1.sigkill_and_reap();

    // Prove death, not assume it: the exact signal, and that the socket no
    // longer answers. A drill that restarted a daemon still quietly running
    // would prove the opposite of what it claims.
    assert_eq!(
        status.signal(),
        Some(9),
        "the first daemon must actually have been SIGKILL'd, got {status:?}"
    );
    assert!(
        UnixStream::connect(&socket_path).is_err(),
        "no process should be listening on the socket right after a SIGKILL"
    );
    // The socket *file* itself, though, is exactly what a `kill -9` leaves
    // behind (`UnixListener` does not unlink on drop, and the process never
    // got the chance to either).
    assert!(
        socket_path.exists(),
        "a killed daemon leaves its socket file behind — that is the case this drill exists for"
    );
    assert!(
        process_is_alive(harness_pid),
        "the stand-in harness process is not a child of the socket/accept loop and must outlive \
         the daemon that started it — ADR 0014's second open item"
    );

    // --- generation 2: restart, over the same stale socket file -------------
    let daemon2 = DaemonProcess::spawn(&root, None);
    assert_ne!(
        daemon2.pid, pid1,
        "sanity: this really is a second process, not the reaped first one"
    );
    // Not just "a daemon came up" — it bound *the same path* the stale file
    // from generation 1 occupied. Without this, a bug in `socket::bind` that
    // stopped removing the stale file could still leave this drill green by
    // binding some other path, which would prove nothing about the stale
    // file at all.
    assert_eq!(
        daemon2.socket_path, socket_path,
        "the restarted daemon must bind the exact socket path the killed daemon left behind"
    );
    let pid_after_restart = holder_pid(&lock_path)
        .unwrap_or_else(|| panic!("lock file at {lock_path:?} must record the new holder"));
    assert_eq!(
        pid_after_restart, daemon2.pid,
        "the installation lock must be released by death and re-acquired by the new daemon"
    );

    // Reconciliation ran on the way up (`startup::build_handler`), not a cold
    // start: the session that was `running` is now `disconnected` — read
    // exactly like ADR 0019 says a restart reads it — and still visible, not
    // silently forgotten.
    let status_resp = daemon2.call(&query(
        alpha,
        "agent.status",
        serde_json::json!({ "session_id": session_id.to_string() }),
    ));
    assert_eq!(
        status_resp.result["session"]["state"], "disconnected",
        "a restart must never cold-start: the session must be reconciled, not lost or left \
         `running` while nothing can see it. {:?}",
        status_resp.result
    );
    let leases = status_resp.result["leases"]
        .as_array()
        .expect("leases array");
    assert!(
        leases.iter().any(|l| l["released_at"].is_null()),
        "a disconnected session must keep its lease — an unknown occupant must not have its \
         workspace handed to a second harness. {leases:?}"
    );

    // The task that was `running` comes back `blocked: interrupted`, never a
    // silent lie that it is still running.
    let task_resp = daemon2.call(&query(
        alpha,
        "task.show",
        serde_json::json!({ "task_id": task_id.to_string() }),
    ));
    assert_eq!(
        task_resp.result["status"], "blocked",
        "{:?}",
        task_resp.result
    );
    assert_eq!(
        task_resp.result["blocked_reason"], "interrupted",
        "{:?}",
        task_resp.result
    );

    // The task nothing was ever sent for stays queued, its stale assignment
    // cleared.
    let never_sent_resp = daemon2.call(&query(
        gamma,
        "task.show",
        serde_json::json!({ "task_id": never_sent_task_id.to_string() }),
    ));
    assert_eq!(
        never_sent_resp.result["status"], "queued",
        "{:?}",
        never_sent_resp.result
    );
    assert!(
        never_sent_resp.result["assigned_session_id"].is_null(),
        "{:?}",
        never_sent_resp.result
    );

    // ADR 0014's second open item, proven at the OS level: the harness
    // process is not a child of the daemon's own accept loop, and really did
    // outlive the daemon that started it, all the way through a restart.
    assert!(
        process_is_alive(harness_pid),
        "the stand-in harness process must still be alive after the daemon that started it was \
         killed and a new one took its place"
    );

    // --- bonus: a second restart changes nothing further (process-level
    // idempotence, mirroring `restore::reconcile`'s own function-level test)
    let pid2_before_kill = holder_pid(&lock_path).expect("lock file holder");
    assert_eq!(pid2_before_kill, daemon2.pid);
    let status2 = daemon2.sigkill_and_reap();
    assert_eq!(status2.signal(), Some(9));

    let daemon3 = DaemonProcess::spawn(&root, None);
    let task_resp_again = daemon3.call(&query(
        alpha,
        "task.show",
        serde_json::json!({ "task_id": task_id.to_string() }),
    ));
    assert_eq!(
        task_resp_again.result["status"], "blocked",
        "a second restart must reach the same conservative state, not re-derive a different one"
    );
    assert_eq!(task_resp_again.result["blocked_reason"], "interrupted");

    // --- cleanup: our own spawned processes only (never a pattern-kill) ----
    let _ = Command::new("kill")
        .args(["-9", &harness_pid.to_string()])
        .status();
    drop(daemon3); // Drop kills+reaps if somehow still alive.
}

fn process_is_alive(pid: u32) -> bool {
    // `kill -0`: signal 0 sends nothing and only checks whether the process
    // (and our permission to signal it) exists. This is our own drill's
    // stand-in harness process, spawned above — never a pattern-kill, and
    // never anyone else's process.
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
