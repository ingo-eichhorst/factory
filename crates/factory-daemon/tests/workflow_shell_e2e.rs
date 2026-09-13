//! End-to-end coverage against a real daemon subprocess, a real HTTP client,
//! and the `shell` agent under `herdr` -- the acceptance criterion this
//! repository's other tests cannot stand in for, because they all talk to an
//! in-process `Engine` with a fake runtime.
//!
//! Skips (prints why, does not fail) rather than asserting anything when
//! `herdr` is not on PATH or the `factory` binary is not built next to
//! `factory-daemon`. The second check is not incidental: `ShellAgent::prompt`
//! (see `crates/factory-plugins/src/builtin/agents.rs`) writes the literal
//! shell command `{factory_bin} task report {id} --status done ...` into the
//! session -- a task's terminal status comes from that binary actually
//! running, not from herdr's own idea of whether the pane looks busy. Without
//! it, no task here could ever reach `done` and the test would prove nothing
//! by passing.
//!
//! Never shells out to `cargo build`: that would contend on the cargo lock
//! with whatever invoked this test. If the `factory` binary has not been
//! built, the fix is `cargo build --workspace` (or `--bin factory`) first.

use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

// ------------------------------------------------------------- test rig

/// Kills the daemon and removes the temp root even when an assertion panics
/// partway through -- `Drop` runs during unwinding, plain cleanup code after
/// the assertions would not.
struct Daemon {
    child: Option<Child>,
    root: PathBuf,
    port: u16,
    factory_bin: PathBuf,
    herdr_bin: PathBuf,
}

impl Daemon {
    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn spawn(&mut self) {
        assert!(self.child.is_none(), "a previous daemon was never stopped");
        let log = std::fs::File::create(self.root.join("daemon.log")).expect("daemon.log");
        let child = Command::new(env!("CARGO_BIN_EXE_factory-daemon"))
            .arg("--root")
            .arg(&self.root)
            .arg("run")
            .env("FACTORY_BIN", &self.factory_bin)
            .env("FACTORY_HERDR_BIN", &self.herdr_bin)
            .stdout(Stdio::from(log.try_clone().expect("dup log fd")))
            .stderr(Stdio::from(log))
            .spawn()
            .expect("spawn factory-daemon");
        self.child = Some(child);
        self.wait_for_http();
    }

    /// Send SIGTERM without a `libc`/`nix` dependency -- `kill(1)` already
    /// does exactly this, and it is on every machine that can run this test.
    fn sigterm(&mut self) {
        let Some(child) = &mut self.child else {
            panic!("daemon is not running");
        };
        let pid = child.id().to_string();
        let status = Command::new("kill")
            .args(["-TERM", &pid])
            .status()
            .expect("run kill(1)");
        assert!(status.success(), "kill -TERM {pid} failed: {status}");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match child.try_wait().expect("poll child") {
                Some(_) => break,
                None if Instant::now() > deadline => panic!("daemon did not exit after SIGTERM"),
                None => std::thread::sleep(Duration::from_millis(50)),
            }
        }
        self.child = None;
    }

    fn wait_for_http(&self) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Ok(response) = ureq::get(format!("{}/api/status", self.base_url())).call() {
                if response.status().as_u16() == 200 {
                    return;
                }
            }
            if Instant::now() > deadline {
                let log = std::fs::read_to_string(self.root.join("daemon.log")).unwrap_or_default();
                panic!("daemon never answered /api/status; log:\n{log}");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(name);
        candidate.is_file().then_some(candidate)
    })
}

/// Where the daemon looks for its callback binary (`factory_bin()` in
/// `main.rs`): a sibling of its own executable, which is where cargo puts
/// every workspace binary. Never built here -- only ever found.
fn find_factory_bin() -> Option<PathBuf> {
    let daemon = PathBuf::from(env!("CARGO_BIN_EXE_factory-daemon"));
    let sibling = daemon.parent()?.join("factory");
    sibling.is_file().then_some(sibling)
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind an ephemeral port")
        .local_addr()
        .expect("read local_addr")
        .port()
}

// ------------------------------------------------------------- http helpers

fn get(url: &str) -> Value {
    let mut response = ureq::get(url).call().unwrap_or_else(|e| panic!("GET {url}: {e}"));
    let body = response
        .body_mut()
        .read_to_string()
        .unwrap_or_else(|e| panic!("GET {url} body: {e}"));
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("GET {url} json ({e}): {body}"))
}

fn expect_ok(url: &str, value: &Value) -> Value {
    if value["status"] != "ok" {
        panic!("{url} refused: {value}");
    }
    value["data"].clone()
}

fn post(url: &str, body: &Value) -> Value {
    let mut response = ureq::post(url)
        .send_json(body.clone())
        .unwrap_or_else(|e| panic!("POST {url}: {e}"));
    let text = response
        .body_mut()
        .read_to_string()
        .unwrap_or_else(|e| panic!("POST {url} body: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("POST {url} json ({e}): {text}"))
}

fn wait_for<F>(what: &str, timeout: Duration, mut poll: F) -> Value
where
    F: FnMut() -> Option<Value>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(value) = poll() {
            return value;
        }
        if Instant::now() > deadline {
            panic!("timed out after {timeout:?} waiting for {what}");
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

fn run_status(base: &str, run_id: &str) -> Value {
    expect_ok(
        &format!("{base}/api/workflow-runs/{run_id}"),
        &get(&format!("{base}/api/workflow-runs/{run_id}")),
    )["run"]
        .clone()
}

fn tasks(base: &str) -> Vec<Value> {
    expect_ok(&format!("{base}/api/tasks"), &get(&format!("{base}/api/tasks")))["tasks"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

// ----------------------------------------------------------------- fixture

fn task_node(id: &str, instructions: &str) -> Value {
    json!({
        "id": id,
        "task": {
            "title": id,
            "instructions": instructions,
            "agent": "shell",
            "worktree": false,
        },
    })
}

fn edge(id: &str, from: &str, to: &str) -> Value {
    json!({ "id": id, "from": from, "to": to })
}

/// A fresh instance: `init`, an HTTP bind on a free port, and a git-backed
/// `demo` scope whose default agent is the `shell` harness -- everything
/// `factory task report` and `git` might want, so the daemon under test
/// behaves exactly as a real one would.
fn provision() -> Daemon {
    let herdr_bin = find_on_path("herdr").expect("checked by the caller");
    let factory_bin = find_factory_bin().expect("checked by the caller");

    let root = std::env::temp_dir().join(format!("f45e2e-{}", std::process::id()));
    if root.exists() {
        std::fs::remove_dir_all(&root).expect("clear a leftover root from a previous run");
    }
    std::fs::create_dir_all(&root).expect("create temp root");

    let init = Command::new(env!("CARGO_BIN_EXE_factory-daemon"))
        .arg("--root")
        .arg(&root)
        .arg("init")
        .arg("--name")
        .arg("demo")
        .output()
        .expect("run factory-daemon init");
    assert!(
        init.status.success(),
        "factory-daemon init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let git_init = Command::new("git")
        .arg("init")
        .arg("--quiet")
        .arg(&root)
        .status()
        .expect("run git init");
    assert!(git_init.success(), "git init failed in {}", root.display());

    let port = free_port();
    let config_path = root.join(".factory/config.yaml");
    let text = std::fs::read_to_string(&config_path).expect("read generated config.yaml");
    let mut doc: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&text).expect("parse generated config.yaml");

    let mut http = serde_yaml_ng::Mapping::new();
    http.insert("kind".into(), "http".into());
    http.insert("bind".into(), format!("127.0.0.1:{port}").into());
    doc["daemon"]["interfaces"]
        .as_sequence_mut()
        .expect("daemon.interfaces is a list")
        .push(serde_yaml_ng::Value::Mapping(http));

    let mut agent = serde_yaml_ng::Mapping::new();
    agent.insert("harness".into(), "shell".into());
    agent.insert("lifetime".into(), "task".into());
    doc["scope"]["agent"] = serde_yaml_ng::Value::Mapping(agent);

    std::fs::write(&config_path, serde_yaml_ng::to_string(&doc).expect("serialize config.yaml"))
        .expect("write config.yaml");

    let mut daemon = Daemon {
        child: None,
        root,
        port,
        factory_bin,
        herdr_bin,
    };
    daemon.spawn();
    daemon
}

// -------------------------------------------------------------- the test

#[test]
fn diamond_dag_and_restart_recovery_with_the_shell_agent() {
    if find_on_path("herdr").is_none() {
        eprintln!("skipping: herdr is not on PATH");
        return;
    }
    if find_factory_bin().is_none() {
        eprintln!(
            "skipping: the `factory` binary is not built next to factory-daemon \
             (run `cargo build --workspace` or `cargo build --bin factory` first) -- \
             the shell agent's own prompt calls back into it to report status, so \
             without it no task here could ever reach `done`"
        );
        return;
    }

    let mut daemon = provision();
    let base = daemon.base_url();

    // -- a diamond: a -> b, a -> c, b -> d, c -> d -----------------------
    let draft = json!({
        "name": "diamond",
        "scope": "demo",
        "nodes": [
            task_node("a", "true"),
            task_node("b", "true"),
            task_node("c", "true"),
            task_node("d", "true"),
        ],
        "edges": [
            edge("ab", "a", "b"),
            edge("ac", "a", "c"),
            edge("bd", "b", "d"),
            edge("cd", "c", "d"),
        ],
    });
    let created = expect_ok(
        &format!("{base}/api/workflows"),
        &post(&format!("{base}/api/workflows"), &draft),
    );
    let workflow_id = created["workflow"]["id"].as_str().unwrap().to_string();

    let started = expect_ok(
        &format!("{base}/api/workflows/{workflow_id}/run"),
        &post(&format!("{base}/api/workflows/{workflow_id}/run"), &json!({})),
    );
    let run_id = started["run"]["id"].as_str().unwrap().to_string();

    let finished = wait_for("the diamond run to finish", Duration::from_secs(30), || {
        let run = run_status(&base, &run_id);
        matches!(run["status"].as_str(), Some("done") | Some("failed") | Some("cancelled"))
            .then_some(run)
    });
    assert_eq!(
        finished["status"], "done",
        "the diamond run did not succeed: {finished}"
    );

    // Exactly one task per node, each carrying that node's provenance.
    let all_tasks = tasks(&base);
    let mut by_node = std::collections::HashMap::new();
    for task in &all_tasks {
        let Some(origin) = task["workflow_origin"].as_object() else { continue };
        if origin["workflow_run_id"].as_str() != Some(run_id.as_str()) {
            continue;
        }
        let node_id = origin["node_id"].as_str().unwrap().to_string();
        assert!(
            by_node.insert(node_id.clone(), task.clone()).is_none(),
            "node {node_id:?} spawned more than one task"
        );
    }
    assert_eq!(
        by_node.len(),
        4,
        "expected exactly 4 tasks, one per node: {all_tasks:#?}"
    );
    for node_id in ["a", "b", "c", "d"] {
        let task = &by_node[node_id];
        assert_eq!(task["status"], "done", "node {node_id} did not finish done: {task}");
        assert_eq!(
            task["result"], "command exited 0",
            "node {node_id}'s task never actually ran `factory task report`: {task}"
        );
    }

    // d must not have been created before both its parents were done --
    // RFC3339 UTC timestamps of the same precision sort lexicographically in
    // chronological order, so a plain string comparison is exact.
    let created_at = |id: &str| by_node[id]["created_at"].as_str().unwrap().to_string();
    let (b_at, c_at, d_at) = (created_at("b"), created_at("c"), created_at("d"));
    assert!(d_at >= b_at, "d ({d_at}) was created before b ({b_at})");
    assert!(d_at >= c_at, "d ({d_at}) was created before c ({c_at})");

    // -- restart mid-run: a root that is still running when SIGTERM lands --
    let restart_draft = json!({
        "name": "restart-probe",
        "scope": "demo",
        "nodes": [task_node("root", "sleep 5")],
        "edges": [],
    });
    let restart_wf = expect_ok(
        &format!("{base}/api/workflows"),
        &post(&format!("{base}/api/workflows"), &restart_draft),
    );
    let restart_wf_id = restart_wf["workflow"]["id"].as_str().unwrap().to_string();
    let restart_started = expect_ok(
        &format!("{base}/api/workflows/{restart_wf_id}/run"),
        &post(&format!("{base}/api/workflows/{restart_wf_id}/run"), &json!({})),
    );
    let restart_run_id = restart_started["run"]["id"].as_str().unwrap().to_string();

    // Wait for the shell prompt's own first report -- "running" -- so
    // SIGTERM lands while `sleep 5` is actually in flight, not before the
    // session even exists.
    wait_for("the restart-probe root to start running", Duration::from_secs(15), || {
        let run = run_status(&base, &restart_run_id);
        run["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["node_id"] == "root")
            .filter(|n| n["status"] == "running")
            .cloned()
    });

    daemon.sigterm();
    daemon.spawn(); // same --root: recovery runs at startup, see main.rs

    let restart_finished = wait_for(
        "the restart-probe run to finish after recovery",
        Duration::from_secs(30),
        || {
            let run = run_status(&base, &restart_run_id);
            matches!(run["status"].as_str(), Some("done") | Some("failed") | Some("cancelled"))
                .then_some(run)
        },
    );
    assert_eq!(
        restart_finished["status"], "done",
        "the restart-probe run did not survive the restart: {restart_finished}"
    );

    let after_restart = tasks(&base);
    let root_tasks: Vec<&Value> = after_restart
        .iter()
        .filter(|task| {
            task["workflow_origin"]["workflow_run_id"].as_str() == Some(restart_run_id.as_str())
        })
        .collect();
    assert_eq!(
        root_tasks.len(),
        1,
        "restart recovery duplicated the root task: {root_tasks:#?}"
    );
    assert_eq!(root_tasks[0]["status"], "done");
}
