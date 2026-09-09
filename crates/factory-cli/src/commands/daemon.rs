//! `factory daemon run` (ADR 0002 decision 1) and the three commands that
//! manage and inspect it: `start`, `stop`, `status`.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::exit;
use crate::rpc::{self, Liveness};

const START_POLL_TIMEOUT: Duration = Duration::from_secs(10);
const STOP_POLL_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How often the daemon's observe loop reads its sessions back from the
/// harness. Slow enough that a Herdr call per session per tick is cheap;
/// fast enough that `factory agent status` is not reporting minutes-old
/// state. Nothing depends on the exact figure — no rule is timed off it.
const OBSERVE_INTERVAL: Duration = Duration::from_secs(5);

/// `factory daemon run`: bind the socket, hold the installation lock, serve
/// requests until this process is signalled.
///
/// The adapter is chosen here and nowhere else. `--harness` decides which
/// one, because a daemon serves one instance and an instance's sessions run
/// on one harness — design §2.2's `harness:` is per scope, and a daemon that
/// tried to infer it per request would need a second copy of that rule.
///
/// Startup order is not arbitrary: `build_handler` reconciles the restored
/// database *before* the socket ever answers (ADR 0014's open item — a
/// restarted daemon re-adopts rather than cold-starts), and the observe loop
/// starts only once that reconciliation has finished, so it can never read a
/// session in the state a crash left behind.
pub fn daemon_run(root: &Path, harness: &str) -> i32 {
    let Some(parsed) = factory_config::Harness::parse(harness) else {
        eprintln!("factory: unknown --harness `{harness}`; expected `pi` or `claude-code`");
        return exit::GENERIC_ERROR;
    };

    let config_path = factory_daemon::config_path(root);
    if !config_path.exists() {
        eprintln!(
            "factory: {} does not exist\n  help: run `factory init --root {}` first",
            config_path.display(),
            root.display()
        );
        return exit::GENERIC_ERROR;
    }

    let herdr = factory_adapter::HerdrCli::new("herdr");
    match parsed {
        factory_config::Harness::Pi => serve_with(root, factory_adapter::PiAdapter::new(herdr)),
        factory_config::Harness::ClaudeCode => serve_with(
            root,
            factory_adapter::ClaudeAdapter::new(herdr, factory_adapter::IrrlichtHttp::new()),
        ),
        other => {
            eprintln!(
                "factory: harness `{other:?}` has no adapter\n  help: version 1 drives `pi` and \
                 `claude-code`; a scope may be configured for another harness, but no daemon can \
                 run its sessions yet"
            );
            exit::GENERIC_ERROR
        }
    }
}

/// The half of `daemon_run` that is the same whichever adapter was chosen.
fn serve_with<A: factory_adapter::Adapter + Send + Sync + 'static>(root: &Path, adapter: A) -> i32 {
    let daemon = match factory_daemon::Daemon::start(root) {
        Ok(daemon) => daemon,
        Err(source) => {
            eprintln!("factory: {source}");
            return exit::GENERIC_ERROR;
        }
    };

    let handler = match factory_daemon::startup::build_handler(root, adapter) {
        Ok(handler) => handler,
        Err(body) => {
            eprintln!("factory: {}: {}", body.code, body.message);
            return exit::GENERIC_ERROR;
        }
    };
    let handler = std::sync::Arc::new(handler);

    let (_observe_join, _stop) =
        factory_daemon::observe::spawn(std::sync::Arc::clone(&handler), OBSERVE_INTERVAL);

    println!(
        "factory: daemon listening at {}",
        daemon.socket_path().display()
    );
    if let Err(source) = daemon.serve(handler) {
        eprintln!("factory: the daemon stopped accepting connections: {source}");
        return exit::GENERIC_ERROR;
    }
    exit::OK
}

/// `factory start`: run `factory daemon run` in the background, and wait for
/// it to either start listening or exit immediately with a reason.
pub fn start(root: &Path, harness: &str) -> i32 {
    match rpc::ping(root) {
        Liveness::Running => {
            println!(
                "factory: daemon already running at {}",
                factory_daemon::socket_path(root).display()
            );
            return exit::OK;
        }
        Liveness::Unclear(message) => {
            eprintln!("factory: {message}");
            return exit::GENERIC_ERROR;
        }
        Liveness::NotRunning => {}
    }

    let config_path = factory_daemon::config_path(root);
    if !config_path.exists() {
        eprintln!(
            "factory: {} does not exist\n  help: run `factory init --root {}` first",
            config_path.display(),
            root.display()
        );
        return exit::GENERIC_ERROR;
    }

    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(source) => {
            eprintln!("factory: could not find my own executable: {source}");
            return exit::GENERIC_ERROR;
        }
    };

    let factory_dir = root.join(".factory");
    if let Err(source) = std::fs::create_dir_all(&factory_dir) {
        eprintln!(
            "factory: could not create {}: {source}",
            factory_dir.display()
        );
        return exit::GENERIC_ERROR;
    }
    let log_path = factory_dir.join("daemon.log");
    let (stdout, stderr) = match open_log_pair(&log_path) {
        Ok(pair) => pair,
        Err(source) => {
            eprintln!("factory: could not open {}: {source}", log_path.display());
            return exit::GENERIC_ERROR;
        }
    };

    let mut command = std::process::Command::new(&exe);
    command
        .arg("--root")
        .arg(root)
        .arg("daemon")
        .arg("run")
        .arg("--harness")
        .arg(harness)
        .stdin(std::process::Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so a signal sent to this shell's group
        // (e.g. an interactive Ctrl-C) does not also reach the daemon —
        // the closest thing to detaching available without `setsid`, which
        // needs `unsafe` `libc` and this workspace forbids `unsafe_code`.
        command.process_group(0);
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(source) => {
            eprintln!("factory: could not start the daemon: {source}");
            return exit::GENERIC_ERROR;
        }
    };

    let deadline = Instant::now() + START_POLL_TIMEOUT;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            eprintln!(
                "factory: the daemon exited immediately ({status})\n  help: see {}",
                log_path.display()
            );
            return exit::GENERIC_ERROR;
        }

        match rpc::ping(root) {
            Liveness::Running => {
                println!(
                    "factory: daemon started (pid {}), listening at {}",
                    child.id(),
                    factory_daemon::socket_path(root).display()
                );
                return exit::OK;
            }
            Liveness::Unclear(message) => {
                eprintln!("factory: {message}");
                return exit::GENERIC_ERROR;
            }
            Liveness::NotRunning => {}
        }

        if Instant::now() >= deadline {
            eprintln!(
                "factory: the daemon did not start listening within {}s\n  help: see {}",
                START_POLL_TIMEOUT.as_secs(),
                log_path.display()
            );
            return exit::GENERIC_ERROR;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn open_log_pair(log_path: &Path) -> std::io::Result<(std::fs::File, std::fs::File)> {
    let stdout = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    let stderr = stdout.try_clone()?;
    Ok((stdout, stderr))
}

/// `factory stop`: idempotent (stopping an already-stopped daemon is
/// success, not an error), and reads the pid the daemon already recorded in
/// `.factory/factory.lock` only to name a signal target — never to probe
/// whether the lock can be acquired (decision 7).
pub fn stop(root: &Path) -> i32 {
    match rpc::ping(root) {
        Liveness::NotRunning => {
            println!("factory: daemon is not running");
            return exit::OK;
        }
        Liveness::Unclear(message) => {
            eprintln!("factory: {message}");
            return exit::GENERIC_ERROR;
        }
        Liveness::Running => {}
    }

    let lock_path = factory_daemon::lock_path(root);
    let pid = match read_pid(&lock_path) {
        Some(pid) => pid,
        None => {
            eprintln!(
                "factory: the daemon appears to be running but its pid could not be read from {}\n  \
                 help: find it independently (e.g. `lsof {}`) and stop it by hand",
                lock_path.display(),
                factory_daemon::socket_path(root).display()
            );
            return exit::GENERIC_ERROR;
        }
    };

    if let Err(source) = std::process::Command::new("/bin/kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
    {
        eprintln!("factory: could not signal pid {pid}: {source}");
        return exit::GENERIC_ERROR;
    }

    let deadline = Instant::now() + STOP_POLL_TIMEOUT;
    loop {
        match rpc::ping(root) {
            Liveness::NotRunning => {
                println!("factory: daemon (pid {pid}) stopped");
                return exit::OK;
            }
            Liveness::Unclear(message) => {
                eprintln!("factory: {message}");
                return exit::GENERIC_ERROR;
            }
            Liveness::Running => {}
        }

        if Instant::now() >= deadline {
            eprintln!(
                "factory: sent SIGTERM to pid {pid} but it is still answering after {}s\n  help: \
                 check `ps -p {pid}` and consider `kill -9 {pid}`",
                STOP_POLL_TIMEOUT.as_secs()
            );
            return exit::STOP_TIMED_OUT;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn read_pid(lock_path: &Path) -> Option<u32> {
    std::fs::read_to_string(lock_path).ok()?.trim().parse().ok()
}

/// `factory status`.
pub fn status(root: &Path) -> i32 {
    let outcome = rpc::query(
        root,
        uuid::Uuid::nil(),
        "daemon.status",
        serde_json::json!({}),
    );
    rpc::report(root, outcome, |result| {
        rpc::print_result("factory: daemon running", &result);
        exit::OK
    })
}
