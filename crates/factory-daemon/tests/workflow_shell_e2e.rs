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
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
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
    github_path: Option<PathBuf>,
}

impl Daemon {
    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn spawn(&mut self) {
        assert!(self.child.is_none(), "a previous daemon was never stopped");
        let log = std::fs::File::create(self.root.join("daemon.log")).expect("daemon.log");
        let mut command = Command::new(env!("CARGO_BIN_EXE_factory-daemon"));
        command.arg("--root")
            .arg(&self.root)
            .arg("run")
            .env("FACTORY_BIN", &self.factory_bin)
            .env("FACTORY_HERDR_BIN", &self.herdr_bin)
            .stdout(Stdio::from(log.try_clone().expect("dup log fd")))
            .stderr(Stdio::from(log));
        if let Some(directory) = &self.github_path {
            command.env("PATH", format!("{}:{}", directory.display(), std::env::var("PATH").unwrap_or_default()));
        }
        let child = command.spawn().expect("spawn factory-daemon");
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
        let url = format!("{}/api/status", self.base_url());
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some((200, _)) = raw_request("GET", &url, None) {
                return;
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
//
// A hand-rolled HTTP/1.1 client rather than a crate: this test's only need
// for one was GET/POST with a small JSON body, and pulling in a real client
// for that outweighs the dozen lines below. `Connection: close` is the
// whole trick -- the daemon closes its end once the response is fully
// written, so reading the socket to EOF *is* reading the whole response,
// with no need to parse `Content-Length` or chunked framing at all.

/// One request, or `None` for any connection/IO failure -- used by
/// `wait_for_http`, which needs to retry a daemon that is not listening
/// yet, not panic the first time it is not.
fn raw_request(method: &str, url: &str, body: Option<&Value>) -> Option<(u16, String)> {
    let rest = url.strip_prefix("http://")?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let payload = body.map(|value| serde_json::to_vec(value).expect("serialize request body"));

    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n");
    if let Some(payload) = &payload {
        request.push_str("Content-Type: application/json\r\n");
        request.push_str(&format!("Content-Length: {}\r\n", payload.len()));
    }
    request.push_str("\r\n");

    let mut stream = std::net::TcpStream::connect(authority).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(30))).ok()?;
    stream.set_write_timeout(Some(Duration::from_secs(10))).ok()?;
    stream.write_all(request.as_bytes()).ok()?;
    if let Some(payload) = &payload {
        stream.write_all(payload).ok()?;
    }

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).ok()?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, response_body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status: u16 = head.lines().next()?.split_whitespace().nth(1)?.parse().ok()?;
    Some((status, response_body.to_string()))
}

fn get(url: &str) -> Value {
    let (status, body) = raw_request("GET", url, None).unwrap_or_else(|| panic!("GET {url}: no response"));
    if status != 200 {
        panic!("GET {url} -> HTTP {status}: {body}");
    }
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("GET {url} json ({e}): {body}"))
}

fn expect_ok(url: &str, value: &Value) -> Value {
    if value["status"] != "ok" {
        panic!("{url} refused: {value}");
    }
    value["data"].clone()
}

fn post(url: &str, body: &Value) -> Value {
    let (status, text) =
        raw_request("POST", url, Some(body)).unwrap_or_else(|| panic!("POST {url}: no response"));
    if status != 200 {
        panic!("POST {url} -> HTTP {status}: {text}");
    }
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

    // The process id alone would collide: the default test harness runs every
    // `#[test]` in this binary as a thread of one process, and more than one
    // of them now calls `provision()`. The uuid is what keeps two concurrent
    // instances from fighting over the same root.
    let root = std::env::temp_dir().join(format!("f45e2e-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
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
    let interfaces = doc["daemon"]["interfaces"]
        .as_sequence_mut()
        .expect("daemon.interfaces is a list");
    // `factory-daemon init` already wrote a default `http` entry with no
    // `bind` of its own, which falls back to a fixed default port -- drop it
    // rather than leave it running alongside ours, or two instances
    // provisioned for two tests in the same process (this file now has more
    // than one) collide on that fixed port even though each picked its own
    // free one for the interface it actually talks to.
    interfaces.retain(|i| i["kind"].as_str() != Some("http"));
    interfaces.push(serde_yaml_ng::Value::Mapping(http));

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
        github_path: None,
    };
    daemon.spawn();
    // The browser loads the roster immediately. Exercise the real axum
    // request stack too, not only workflow endpoints on a test thread.
    expect_ok(&format!("{}/api/agents", daemon.base_url()),
        &get(&format!("{}/api/agents", daemon.base_url())));
    daemon
}

// -------------------------------------------------------------- the test

#[test]
fn approved_github_mirror_uses_the_real_cli_api_and_persistent_receipts_without_live_outbound_writes() {
    if missing_prerequisites() { return; }
    use std::os::unix::fs::PermissionsExt;
    let mut daemon = provision(); daemon.sigterm();
    let config_path = daemon.root.join(".factory/config.yaml");
    let mut scope: serde_yaml_ng::Value = serde_yaml_ng::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    scope["scope"]["environments"] = serde_yaml_ng::from_str("- name: mirror-test\n  tier: staging\n  checks: [{ kind: command, command: 'true' }]\n  github_deployments: { repository: fixture-owner/fixture-repo }\n").unwrap();
    std::fs::write(&config_path, serde_yaml_ng::to_string(&scope).unwrap()).unwrap();
    let fixture = daemon.root.join("fake-github"); std::fs::create_dir(&fixture).unwrap();
    let gh = fixture.join("gh");
    std::fs::write(&gh, format!(r#"#!/bin/sh
set -eu
data='{}'
printf '%s\n' "$*" >> "$data/calls"
case "$2" in
  *'/statuses?'*) if test -f "$data/status.json"; then printf '['; cat "$data/status.json"; printf ']'; else printf '[]'; fi ;;
  *'/statuses') cat > "$data/status-input.json"
    if test -f "$data/fail-status"; then printf 'credential-error-sentinel-not-for-publication' >&2; exit 1; fi
    cp "$data/expected-status.json" "$data/status.json"; cat "$data/status.json" ;;
  *'/deployments?'*) if test -f "$data/deployment.json"; then printf '['; cat "$data/deployment.json"; printf ']'; else printf '[]'; fi ;;
  *'/deployments') cat > "$data/deployment-input.json"; cp "$data/expected-deployment.json" "$data/deployment.json"; cat "$data/deployment.json" ;;
  *) exit 3 ;;
esac
"#, fixture.display())).unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    daemon.github_path = Some(fixture.clone()); daemon.spawn();
    let factory_bin = daemon.factory_bin.clone();
    let instance_root = daemon.root.clone();
    let instance_url = daemon.base_url();
    let cli = |args: &[&str]| Command::new(&factory_bin).arg("--root").arg(&instance_root).arg("--url").arg(&instance_url).arg("--json").args(args).env_remove("FACTORY_TOKEN").env_remove("FACTORY_TASK_TOKEN").env_remove("FACTORY_RUN_TOKEN").env_remove("FACTORY_URL").env_remove("FACTORY_SOCKET").output().unwrap();
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let output = cli(&["deploy", "start", "--env", "mirror-test", "--commit", sha, "--via", "private-origin-sentinel"]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let deployment: Value = serde_json::from_slice(&output.stdout).unwrap(); let id = deployment["deployment"]["id"].as_str().unwrap().to_owned();
    let plan_url = format!("{}/api/deployments/{id}/mirror-plan", daemon.base_url());
    let plan = expect_ok(&plan_url, &get(&plan_url))["plan"].clone();
    let publish_url = format!("{}/api/deployments/{id}/publish", daemon.base_url());
    let inspected = cli(&["deploy", "mirror-plan", &id]);
    assert!(inspected.status.success()); assert_eq!(serde_json::from_slice::<Value>(&inspected.stdout).unwrap()["plan"], plan);
    assert!(!fixture.join("calls").exists());
    let (status, _) = raw_request("POST", &publish_url, Some(&json!({"approval": "unapproved"}))).unwrap();
    assert_ne!(status, 200);
    assert!(!fixture.join("calls").exists());
    std::fs::write(fixture.join("expected-deployment.json"), json!({"id": 71, "sha": sha, "task": "factory:mirror", "environment": "mirror-test",
        "production_environment": false, "transient_environment": false,
        "payload": {"factory_deployment": id, "scope": "demo"}}).to_string()).unwrap();
    let set_status = |plan: &Value, number: u64| std::fs::write(fixture.join("expected-status.json"),
        json!({"id": number, "state": plan["state"], "description": format!("Factory mirror {}", plan["approval"].as_str().unwrap())}).to_string()).unwrap();
    set_status(&plan, 81);
    std::fs::write(fixture.join("fail-status"), "1").unwrap();
    let failed = cli(&["deploy", "publish", &id, "--approval", plan["approval"].as_str().unwrap()]);
    assert!(!failed.status.success());
    let failed: Value = serde_json::from_slice(&failed.stdout).unwrap(); assert_eq!(failed["receipt"]["phase"], "failed");
    assert!(!failed.to_string().contains("credential-error-sentinel"));
    daemon.sigterm(); daemon.spawn();
    std::fs::remove_file(fixture.join("fail-status")).unwrap();
    let resumed = cli(&["deploy", "publish", &id, "--approval", plan["approval"].as_str().unwrap()]);
    assert!(resumed.status.success(), "{}", String::from_utf8_lossy(&resumed.stderr));
    let resumed: Value = serde_json::from_slice(&resumed.stdout).unwrap(); assert_eq!(resumed["receipt"]["phase"], "published");
    let finish_url = format!("{}/api/deployments/{id}/finish", daemon.base_url());
    expect_ok(&finish_url, &post(&finish_url, &json!({"status": "succeeded"})));
    let updated = expect_ok(&plan_url, &get(&plan_url))["plan"].clone();
    assert_eq!(updated["verified"], true); assert_ne!(updated["approval"], plan["approval"]);
    let (status, _) = raw_request("POST", &publish_url, Some(&json!({"approval": plan["approval"]}))).unwrap();
    assert_ne!(status, 200);
    set_status(&updated, 82);
    let published = expect_ok(&publish_url, &post(&publish_url, &json!({"approval": updated["approval"]})))["receipt"].clone();
    assert_eq!(published["phase"], "published"); assert_eq!(published["remote_id"], 71); assert_eq!(published["status_id"], 82);
    daemon.sigterm(); daemon.spawn();
    let calls = std::fs::read_to_string(fixture.join("calls")).unwrap();
    assert_eq!(calls.lines().filter(|line| line.contains("/deployments --method POST")).count(), 1);
    assert_eq!(expect_ok(&publish_url, &post(&publish_url, &json!({"approval": updated["approval"]})))["receipt"], published);
    assert_eq!(std::fs::read_to_string(fixture.join("calls")).unwrap(), calls, "persisted successful retries do not issue another write");
    let report_url = format!("{}/api/environments?scope=demo", daemon.base_url());
    let report = expect_ok(&report_url, &get(&report_url))["report"].clone();
    assert_eq!(report["deployment_mirrors"][&id]["receipt"], published);
    assert_eq!(report["deployments"].as_array().unwrap().len(), 1); assert_eq!(report["deployments"][0]["status"], "succeeded");
    assert!(tasks(&daemon.base_url()).is_empty(), "publishing metadata never manufactures a release task or another deployment");
    let posted: Value = serde_json::from_slice(&std::fs::read(fixture.join("deployment-input.json")).unwrap()).unwrap();
    assert_eq!(posted["auto_merge"], false); assert_eq!(posted["required_contexts"], json!([]));
    assert!(!posted.to_string().contains("private-origin-sentinel"));
    let status: Value = serde_json::from_slice(&std::fs::read(fixture.join("status-input.json")).unwrap()).unwrap(); assert_eq!(status["auto_inactive"], false);
}

#[test]
fn offline_recovery_receipts_import_without_fake_runs_and_survive_outbox_removal_and_restart() {
    if missing_prerequisites() { return; }
    let mut daemon = provision();
    let other = daemon.root.join("projects/other/.factory");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("config.yaml"), "version: 1\nscope:\n  id: other-scope\n  name: other\n").unwrap();
    daemon.sigterm();
    let cli = |args: &[&str]| {
        let output = Command::new(&daemon.factory_bin).arg("--root").arg(&daemon.root)
            .env("FACTORY_URL", "http://127.0.0.1:1").args(["recovery-journal"]).args(args).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    let started = |scope: &str, command: &str| cli(&["start", "--scope", scope, "--env", "offline-test",
        "--source", "test-shell", "--actor", "operator", "--reason", "record while daemon is down", "--command", command]);
    let failed = started("demo", "false");
    let actual_exit = Command::new("sh").args(["-c", "false"]).status().unwrap().code().unwrap().to_string();
    assert!(raw_request("GET", &format!("{}/api/status", daemon.base_url()), None).is_none());
    cli(&["finish", &failed, "--exit-code", &actual_exit, "--local-http", "false", "--detail", "actual false command and unavailable HTTP"]);
    let pending = started("demo", "awaiting a result; no runtime outcome inferred");
    let foreign = started("other", "true");
    let actual_exit = Command::new("sh").args(["-c", "true"]).status().unwrap().code().unwrap().to_string();
    cli(&["finish", &foreign, "--exit-code", &actual_exit]);
    daemon.spawn();
    // Prove startup imports without requiring an Operations page request.
    wait_for("offline recovery history is persisted at startup", Duration::from_secs(5), || {
        let db = rusqlite::Connection::open_with_flags(daemon.root.join(".factory/factory.sqlite"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
        let count: i64 = db.query_row("SELECT COUNT(*) FROM recovery_journal", [], |row| row.get(0)).ok()?;
        (count == 5).then_some(json!(true))
    });
    std::fs::rename(daemon.root.join(".factory/recovery-outbox"), daemon.root.join(".factory/recovery-outbox-archived")).unwrap();
    let report_url = format!("{}/api/environments", daemon.base_url());
    let report = expect_ok(&report_url, &get(&report_url))["report"].clone();
    let actions = report["recovery_journal"]["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 3);
    let action = actions.iter().find(|action| action["id"] == failed).unwrap();
    assert_eq!(action["finish"]["exit_code"], 1);
    assert_eq!(action["finish"]["local_http"], false);
    assert!(action["finish"]["network_routes"].is_null(), "an unperformed network probe stays unknown");
    assert!(actions.iter().find(|action| action["id"] == pending).unwrap().get("finish").is_none());
    assert!(report["deployments"].as_array().unwrap().is_empty());
    assert!(tasks(&daemon.base_url()).is_empty(), "offline receipts never create or complete Factory tasks");
    let scoped_url = format!("{report_url}?scope=other");
    let scoped = expect_ok(&scoped_url, &get(&scoped_url))["report"]["recovery_journal"]["actions"].clone();
    assert_eq!(scoped.as_array().unwrap().len(), 1); assert_eq!(scoped[0]["id"], foreign);
    daemon.sigterm(); daemon.spawn();
    assert_eq!(expect_ok(&report_url, &get(&report_url))["report"]["recovery_journal"]["actions"], report["recovery_journal"]["actions"]);
}

#[test]
fn ensure_restarts_a_real_dead_daemon_offline_and_records_a_failed_required_route_without_a_deployment() {
    if missing_prerequisites() { return; }
    use std::os::unix::fs::{symlink, PermissionsExt};
    let mut daemon = provision();
    let assets = daemon.root.join("release-test");
    let env_home = assets.join("envs/qa");
    std::fs::create_dir_all(env_home.join("bin")).unwrap();
    symlink(env!("CARGO_BIN_EXE_factory-daemon"), env_home.join("bin/factory-daemon")).unwrap();
    symlink(&daemon.root, env_home.join("root")).unwrap();
    std::fs::write(env_home.join("RELEASED"), "bind: 127.0.0.1\n").unwrap();
    let conf = assets.join("envs.conf");
    std::fs::write(&conf, format!("qa {} any isolated\n", daemon.port)).unwrap();
    let fake_bin = assets.join("fixtures"); std::fs::create_dir_all(&fake_bin).unwrap();
    let route_log = assets.join("route-attempts");
    let tailscale = fake_bin.join("tailscale");
    std::fs::write(&tailscale, "#!/bin/sh\nprintf 'route attempt\\n' >> \"$RECOVERY_TEST_ROUTE_LOG\"\nexit 42\n").unwrap();
    std::fs::set_permissions(&tailscale, std::fs::Permissions::from_mode(0o755)).unwrap();
    let original = daemon.child.as_ref().unwrap().id();
    std::fs::write(env_home.join("daemon.pid"), original.to_string()).unwrap();
    // Clean up only the new process launched by this isolated script fixture.
    struct Installed { pid_file: PathBuf, original: u32 }
    impl Drop for Installed {
        fn drop(&mut self) {
            if let Ok(pid) = std::fs::read_to_string(&self.pid_file).unwrap_or_default().trim().parse::<u32>() {
                if pid != self.original {
                    let _ = Command::new("kill").args(["-TERM", &pid.to_string()]).status();
                    std::thread::sleep(Duration::from_millis(300));
                }
            }
        }
    }
    let _installed = Installed { pid_file: env_home.join("daemon.pid"), original };
    daemon.sigterm();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().join(".claude/skills/release/scripts/ensure.sh");
    let run = |record_cli: &Path| Command::new("bash").arg(&script).arg("qa")
        .env("FACTORY_ENVS", assets.join("envs")).env("FACTORY_ENVS_CONF", &conf)
        .env("FACTORY_RELEASE_ROOT", &daemon.root).env("FACTORY_RELEASE_SCOPE", "demo")
        .env("FACTORY_RECORD_CLI", record_cli).env("FACTORY_BIN", &daemon.factory_bin).env("FACTORY_HERDR_BIN", &daemon.herdr_bin)
        .env("RECOVERY_TEST_ROUTE_LOG", &route_log)
        .env("PATH", format!("{}:{}", fake_bin.display(), std::env::var("PATH").unwrap_or_default()))
        .output().unwrap();
    let refused = run(&assets.join("missing-record-cli"));
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("no restart was attempted"));
    assert!(!route_log.exists());
    assert!(raw_request("GET", &format!("{}/api/status", daemon.base_url()), None).is_none());
    let output = run(&daemon.factory_bin);
    assert_eq!(output.status.code(), Some(1), "the intentionally failed route provider must not be called success: {}", String::from_utf8_lossy(&output.stderr));
    daemon.wait_for_http();
    let report_url = format!("{}/api/environments", daemon.base_url());
    let report = expect_ok(&report_url, &get(&report_url))["report"].clone();
    let action = &report["recovery_journal"]["actions"][0];
    assert_eq!(action["source"], "ensure.sh"); assert_eq!(action["reason"], "installed daemon was not running");
    assert_eq!(action["finish"]["exit_code"], 1); assert_eq!(action["finish"]["local_http"], true);
    assert_eq!(action["finish"]["network_routes"], false);
    assert!(report["deployments"].as_array().unwrap().is_empty()); assert!(tasks(&daemon.base_url()).is_empty());
    let pid = std::fs::read_to_string(env_home.join("daemon.pid")).unwrap();
    let output = run(&daemon.factory_bin);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(std::fs::read_to_string(env_home.join("daemon.pid")).unwrap(), pid, "a running daemon is not restarted for route repair");
    let report = expect_ok(&report_url, &get(&report_url))["report"].clone();
    assert_eq!(report["recovery_journal"]["actions"].as_array().unwrap().len(), 2);
    assert_eq!(report["recovery_journal"]["actions"][0]["reason"], "verify and repair routes for a running daemon");
    let before = std::fs::read_to_string(&route_log).unwrap();
    let refused = run(&assets.join("missing-record-cli"));
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("no route repair was attempted"));
    assert_eq!(std::fs::read_to_string(&route_log).unwrap(), before, "no unjournaled route mutation on a receipt-write failure");
}

#[test]
fn release_evidence_matches_a_real_build_and_sbom_and_survives_git_changes_and_restart() {
    if missing_prerequisites() { return; }
    let mut daemon = provision();
    let git = |args: &[&str]| {
        let output = Command::new("git").current_dir(&daemon.root).args(args).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    git(&["config", "user.email", "release-evidence@example.invalid"]);
    git(&["config", "user.name", "Release evidence QA"]);
    std::fs::write(daemon.root.join(".gitignore"), "*\n").unwrap();
    git(&["add", "-f", ".gitignore"]);
    git(&["commit", "-q", "-m", "baseline"]);
    git(&["remote", "add", "origin", "https://github.com/owner/repo.git"]);
    let baseline = git(&["rev-parse", "HEAD"]);
    let base = daemon.base_url();
    let releases_url = format!("{base}/api/releases");
    expect_ok(&releases_url, &post(&releases_url, &json!({ "scope": "demo", "commit": baseline })));
    git(&["commit", "-q", "--allow-empty", "-m", "Merge pull request #90 from release", "-m", "Fixes #91\nRefs #92"]);
    let commit = git(&["rev-parse", "HEAD"]);
    let sbom = |sha: &str, phase: &str| json!({ "bomFormat": "CycloneDX", "specVersion": "1.6", "version": 1,
        "metadata": { "lifecycles": [{ "phase": phase }], "component": { "type": "application", "name": "product", "version": "v1",
            "properties": [{ "name": "factory:git-sha", "value": sha }] } }, "components": [] });
    let instructions = format!(
        "set -eu\nprintf 'built release bytes' > product.bin\nprintf '%s' '{}' > build.cdx.json\nprintf '%s' '{}' > wrong.cdx.json\nprintf '%s' '{}' > running.cdx.json\n\
         \"$FACTORY_BIN\" task attach --kind sbom build.cdx.json\n\"$FACTORY_BIN\" task attach --kind sbom wrong.cdx.json\n\"$FACTORY_BIN\" task attach --kind sbom running.cdx.json\n\
         \"$FACTORY_BIN\" task report \"$FACTORY_TASK_ID\" --status done --artifact product.bin --result 'immutable product build'\n",
        sbom(&commit, "build"), sbom(&"a".repeat(40), "build"), sbom(&commit, "operations")
    );
    let task_url = format!("{base}/api/tasks");
    let task = expect_ok(&task_url, &post(&task_url, &json!({ "title": "Build product with provenance", "instructions": instructions,
        "scope": "demo", "agent": "shell", "worktree": false, "category": "release" })))["task"].clone();
    let task_id = task["id"].as_str().unwrap();
    let run_url = format!("{task_url}/{task_id}/run");
    expect_ok(&run_url, &post(&run_url, &json!({})));
    wait_for("shell-agent build reports immutable artifacts", Duration::from_secs(30), || {
        tasks(&base).into_iter().find(|task| task["id"] == task_id && task["status"] == "done")
    });
    let history_url = format!("{task_url}/{task_id}/runs");
    let producing_run = expect_ok(&history_url, &get(&history_url))["runs"][0].clone();
    let run_id = producing_run["id"].as_str().unwrap();
    expect_ok(&releases_url, &post(&releases_url, &json!({ "scope": "demo", "commit": commit, "version": "v1", "build_run": run_id })));
    let detail_url = format!("{releases_url}/detail?scope=demo&commit={commit}");
    let detail = wait_for("completed source-matching build evidence is readable", Duration::from_secs(5), || {
        let detail = expect_ok(&detail_url, &get(&detail_url))["detail"].clone();
        detail["build"].is_object().then_some(detail)
    });
    assert_eq!(detail["changes"]["base"], baseline);
    assert_eq!(detail["changes"]["commits"].as_array().unwrap().len(), 1);
    assert_eq!(detail["changes"]["commits"][0]["pull_requests"], json!([90]));
    assert_eq!(detail["changes"]["commits"][0]["issues"], json!([91]));
    assert_eq!(detail["build"]["run"]["id"], run_id);
    assert_eq!(detail["build"]["artifacts"][0]["artifact"]["source"]["commit"], commit);
    assert_eq!(detail["build"]["artifacts"][0]["artifact"]["source"]["dirty"], false);
    // Router reads are people-side composition, not L4 reading its own fact.
    let provenance_url = format!("{base}/api/runs/{run_id}/provenance");
    assert_eq!(expect_ok(&provenance_url, &get(&provenance_url))["records"], detail["build"]["artifacts"]);
    let costs_url = format!("{base}/api/costs?scope=demo&group_by=task");
    let costs = expect_ok(&costs_url, &get(&costs_url))["report"].clone();
    assert_eq!(costs["total"]["runs"], 1);
    assert_eq!(costs["rows"][0]["key"], task_id);
    assert_eq!(detail["sboms"].as_array().unwrap().len(), 1, "wrong commit and operations lifecycle are not release build SBOMs");
    let document_url = format!("{base}/api/dependencies/documents/{}?scope=demo", detail["sboms"][0]["attachment"]["id"].as_str().unwrap());
    let document = expect_ok(&document_url, &get(&document_url));
    assert_eq!(document["document"]["metadata"]["component"]["properties"][0]["value"], commit);
    // A deployment actor is distinct from the producing run and does not
    // replace the build's provenance when the same release is deployed.
    let deployments_url = format!("{base}/api/deployments");
    let deployment = expect_ok(&deployments_url, &post(&deployments_url, &json!({ "environment": "review-evidence", "scope": "demo", "commit": commit,
        "version": "v1", "build_run": run_id, "compare_to": baseline })))["deployment"].clone();
    let finish = format!("{deployments_url}/{}/finish", deployment["id"].as_str().unwrap());
    expect_ok(&finish, &post(&finish, &json!({ "status": "succeeded" })));
    let selected_url = format!("{detail_url}&deployment={}", deployment["id"].as_str().unwrap());
    let selected = expect_ok(&selected_url, &get(&selected_url))["detail"].clone();
    assert_eq!(selected["build"]["run"]["id"], run_id);
    assert_eq!(deployment["actor"]["kind"], "person");
    let report_url = format!("{base}/api/environments");
    let report = expect_ok(&report_url, &get(&report_url))["report"].clone();
    assert_eq!(report["environments"][0]["effectiveness"].as_array().unwrap().len(), 4);
    assert_eq!(report["releases"][0]["effectiveness"]["finished"], 1);
    assert_eq!(report["releases"][0]["effectiveness"]["observing"], 1, "fresh success is still inside the observation horizon");
    let (_, wrong_selection) = raw_request("GET", &format!("{detail_url}&deployment=unknown"), None).unwrap();
    assert!(wrong_selection.contains("does not belong"));
    expect_ok(&releases_url, &post(&releases_url, &json!({ "scope": "demo", "commit": "b".repeat(40), "build_run": run_id })));
    let mismatch_url = format!("{releases_url}/detail?scope=demo&commit={}", "b".repeat(40));
    let mismatch = expect_ok(&mismatch_url, &get(&mismatch_url))["detail"].clone();
    assert!(mismatch.get("build").is_none(), "a run cannot prove a different source commit");
    assert!(mismatch["sboms"].as_array().unwrap().is_empty());
    let other_version = expect_ok(&deployments_url, &post(&deployments_url, &json!({ "environment": "review-version", "scope": "demo",
        "commit": commit, "version": "v2", "build_run": run_id })))["deployment"].clone();
    let version_url = format!("{detail_url}&deployment={}", other_version["id"].as_str().unwrap());
    let version_detail = expect_ok(&version_url, &get(&version_url))["detail"].clone();
    assert_eq!(version_detail["release"]["version"], "v2", "the header identifies the selected deployment, not the aggregate catalogue");
    assert!(version_detail["sboms"].as_array().unwrap().is_empty(), "an exact commit does not override a mismatched product version");
    let other_scope = daemon.root.join("projects/other/.factory");
    std::fs::create_dir_all(&other_scope).unwrap();
    std::fs::write(other_scope.join("config.yaml"), "version: 1\nscope:\n  id: other-scope\n  name: other\n").unwrap();
    std::fs::rename(daemon.root.join(".git"), daemon.root.join("git-is-unavailable")).unwrap();
    daemon.sigterm();
    daemon.spawn();
    let recovered = expect_ok(&selected_url, &get(&selected_url))["detail"].clone();
    assert_eq!(recovered["changes"], selected["changes"]);
    assert_eq!(recovered["build"], selected["build"]);
    assert_eq!(recovered["sboms"], selected["sboms"]);
    assert_eq!(expect_ok(&document_url, &get(&document_url))["document"], document["document"]);
    let (_, wrong_scope) = raw_request("GET", &document_url.replace("scope=demo", "scope=other"), None).unwrap();
    assert!(wrong_scope.contains("no such attachment"), "{wrong_scope}");
    expect_ok(&releases_url, &post(&releases_url, &json!({ "scope": "other", "commit": commit, "build_run": run_id })));
    let foreign_url = format!("{releases_url}/detail?scope=other&commit={commit}");
    let foreign = expect_ok(&foreign_url, &get(&foreign_url))["detail"].clone();
    assert!(foreign.get("build").is_none(), "a completed run from another scope cannot prove this release");
    assert!(foreign["sboms"].as_array().unwrap().is_empty());
}

/// `true` (and prints why) when this binary and the environment cannot run
/// any test in this file -- shared so a second test does not have to repeat
/// (or drift from) the same two checks.
fn missing_prerequisites() -> bool {
    if find_on_path("herdr").is_none() {
        eprintln!("skipping: herdr is not on PATH");
        return true;
    }
    if find_factory_bin().is_none() {
        eprintln!(
            "skipping: the `factory` binary is not built next to factory-daemon \
             (run `cargo build --workspace` or `cargo build --bin factory` first) -- \
             the shell agent's own prompt calls back into it to report status, so \
             without it no task here could ever reach `done`"
        );
        return true;
    }
    false
}

fn provision_recovery(command: &str) -> (Daemon, serde_yaml_ng::Value) {
    let mut daemon = provision();
    daemon.sigterm();
    let path = daemon.root.join(".factory/config.yaml");
    let mut config: serde_yaml_ng::Value = serde_yaml_ng::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["scope"]["agents"] = serde_yaml_ng::from_str("[{ name: operator, harness: shell, lifetime: task, role: foreman }]").unwrap();
    config["scope"]["environments"] = serde_yaml_ng::from_str(
        "- name: production\n  checks: [{ name: api, kind: command, command: 'test -f healthy', every: 5s, timeout: 2s }]\n  recover: { agent: operator, command: 'touch healthy', timeout: 30s }\n"
    ).unwrap();
    config["scope"]["environments"][0]["recover"]["command"] = command.into();
    std::fs::write(&path, serde_yaml_ng::to_string(&config).unwrap()).unwrap();
    (daemon, config)
}

#[test]
fn recovery_is_approved_journalled_verified_and_not_a_deployment() {
    if missing_prerequisites() { return; }
    let (mut daemon, mut config) = provision_recovery("touch healthy");
    let path = daemon.root.join(".factory/config.yaml");
    std::fs::write(daemon.root.join("healthy"), "installed release was healthy").unwrap();
    daemon.spawn();
    let base = daemon.base_url();
    let deployments_url = format!("{base}/api/deployments");
    let installed = "a".repeat(40);
    let deployed = expect_ok(&deployments_url, &post(&deployments_url, &json!({ "environment": "production", "commit": installed })))["deployment"].clone();
    let finish = format!("{deployments_url}/{}/finish", deployed["id"].as_str().unwrap());
    expect_ok(&finish, &post(&finish, &json!({ "status": "succeeded" })));
    std::fs::remove_file(daemon.root.join("healthy")).unwrap();
    let report_url = format!("{base}/api/environments");
    let down = wait_for("continuous health sees the installed system fail", Duration::from_secs(15), || {
        let report = expect_ok(&report_url, &get(&report_url))["report"].clone();
        (report["environments"][0]["status"] == "down").then_some(report)
    });
    assert_eq!(down["environments"][0]["recovery_ready"], true);
    let recover = format!("{base}/api/environments/recover");
    let selection = json!({ "environment": "production", "reason": "restart installed system after failed health checks" });
    let started = expect_ok(&recover, &post(&recover, &selection))["run"].clone();
    let workflow_run_id = started["id"].as_str().unwrap();
    let held = wait_for("recovery is held for the owner", Duration::from_secs(15), || {
        tasks(&base).into_iter().find(|t| t["title"] == "Recover production" && t["status"] == "blocked")
    });
    assert_eq!(held["category"], "recovery");
    assert!(!daemon.root.join("healthy").exists());
    let (_, duplicate) = raw_request("POST", &recover, Some(&selection)).unwrap();
    assert!(duplicate.contains("already pending"), "{duplicate}");
    config["scope"]["environments"][0]["recover"]["command"] = "echo newer recipe must not run; exit 9".into();
    std::fs::write(&path, serde_yaml_ng::to_string(&config).unwrap()).unwrap();
    daemon.sigterm();
    daemon.spawn();
    assert!(!daemon.root.join("healthy").exists(), "restart is not approval");
    let pending = expect_ok(&report_url, &get(&report_url))["report"].clone();
    let action = &pending["recoveries"][0];
    assert_eq!(action["reason"], selection["reason"]);
    assert_eq!(action["requested_by"], "owner");
    assert_eq!(action["expected_commit"], installed);
    assert_eq!(action["run"]["status"], "blocked");
    let approve = format!("{base}/api/runs/{}/approve", action["run"]["id"].as_str().unwrap());
    expect_ok(&approve, &post(&approve, &json!({ "reason": "approved restart of the installed release" })));
    wait_for("repaired system passes mandatory checks", Duration::from_secs(30), || {
        let run = run_status(&base, workflow_run_id);
        (run["status"] == "done").then_some(run)
    });
    let recovered = expect_ok(&report_url, &get(&report_url))["report"].clone();
    assert_eq!(recovered["environments"][0]["status"], "up");
    assert_eq!(recovered["environments"][0]["current"]["id"], deployed["id"]);
    assert_eq!(recovered["deployments"].as_array().unwrap().len(), 1, "restart cannot inflate DORA deployments");
    assert_eq!(recovered["recoveries"][0]["status"], "done");
    assert_eq!(recovered["recoveries"][0]["run"]["status"], "done");
    let journal = format!("{base}/api/tasks/{}/entries", held["id"].as_str().unwrap());
    let entries = expect_ok(&journal, &get(&journal));
    assert!(entries["entries"].as_array().unwrap().iter().any(|e| e["kind"] == "approved"));
    let samples = format!("{base}/api/environments/samples?environment=production&check=api&scope=demo&limit=1");
    let page = expect_ok(&samples, &get(&samples))["page"].clone();
    assert_eq!(page["samples"].as_array().unwrap().len(), 1);
    assert_eq!(page["samples"][0]["ok"], true);
    let older = format!("{samples}&from={}&to={}&before={}", page["from"].as_str().unwrap(), page["to"].as_str().unwrap(), page["next_before"].as_i64().unwrap());
    let earlier = expect_ok(&older, &get(&older))["page"].clone();
    assert_ne!(earlier["samples"][0]["id"], page["samples"][0]["id"]);
    daemon.sigterm();
    daemon.spawn();
    let persisted = expect_ok(&report_url, &get(&report_url))["report"].clone();
    assert_eq!(persisted["recoveries"][0]["status"], "done");
    assert_eq!(persisted["deployments"].as_array().unwrap().len(), 1);
    assert_eq!(expect_ok(&older, &get(&older))["page"], earlier, "fixed sample page survives restart");
}

#[test]
fn failed_recovery_commands_and_failed_or_paused_checks_never_report_success() {
    if missing_prerequisites() { return; }
    for (command, pause_before_approval) in [("exit 7", false), ("true", false), ("touch healthy", true)] {
        let (mut daemon, mut config) = provision_recovery(command);
        daemon.spawn();
        let base = daemon.base_url();
        let recover = format!("{base}/api/environments/recover");
        let started = expect_ok(&recover, &post(&recover, &json!({ "environment": "production", "reason": "exercise failed recovery" })))["run"].clone();
        let task = wait_for("failed-path recovery waits for approval", Duration::from_secs(15), || {
            tasks(&base).into_iter().find(|t| t["title"] == "Recover production" && t["status"] == "blocked")
        });
        if pause_before_approval {
            daemon.sigterm();
            config["scope"]["environments"][0]["paused"] = true.into();
            std::fs::write(daemon.root.join(".factory/config.yaml"), serde_yaml_ng::to_string(&config).unwrap()).unwrap();
            daemon.spawn();
        }
        let report_url = format!("{base}/api/environments");
        let held = expect_ok(&report_url, &get(&report_url))["report"]["recoveries"][0].clone();
        let approve = format!("{base}/api/runs/{}/approve", held["run"]["id"].as_str().unwrap());
        expect_ok(&approve, &post(&approve, &json!({ "reason": "approve negative-path QA" })));
        wait_for("command or mandatory health verification fails the reported task", Duration::from_secs(30), || {
            let action = expect_ok(&report_url, &get(&report_url))["report"]["recoveries"][0].clone();
            (action["run"]["status"] == "failed").then_some(action)
        });
        let failed_task = wait_for("failed attempt is mirrored to its standing task", Duration::from_secs(5), || {
            tasks(&base).into_iter().find(|t| t["id"] == task["id"] && t["status"] == "blocked" && t["error"].is_string())
        });
        assert_eq!(failed_task["status"], "blocked", "failed attempts leave the standing task open");
        assert!(failed_task["error"].is_string());
        for restart in [false, true] {
            if restart { daemon.sigterm(); daemon.spawn(); }
            let report = expect_ok(&report_url, &get(&report_url))["report"].clone();
            assert_eq!(report["recoveries"][0]["workflow_run_id"], started["id"]);
            assert_eq!(report["recoveries"][0]["run"]["status"], "failed");
            assert!(report["deployments"].as_array().unwrap().is_empty(), "recovery must not fabricate a deployment");
            assert_ne!(report["recoveries"][0]["status"], "done");
        }
    }
}

#[test]
fn promotion_is_pinned_policy_gated_owner_approved_and_verified_after_restart() {
    if missing_prerequisites() { return; }
    let mut daemon = provision();
    daemon.sigterm();
    let git = |args: &[&str]| {
        let out = Command::new("git").current_dir(&daemon.root).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    git(&["config", "user.email", "promotion@example.invalid"]);
    git(&["config", "user.name", "Promotion QA"]);
    std::fs::write(daemon.root.join(".gitignore"), ".factory/\ndaemon.log\n").unwrap();
    std::fs::write(daemon.root.join("release-source"), "v1").unwrap();
    git(&["add", ".gitignore", "release-source"]);
    git(&["commit", "-q", "-m", "verified source"]);
    let selected = git(&["rev-parse", "HEAD"]);
    let marker = daemon.root.join(".factory/promoted");
    let permit = daemon.root.join(".factory/gate-allowed");
    let config_path = daemon.root.join(".factory/config.yaml");
    let mut config: serde_yaml_ng::Value = serde_yaml_ng::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    config["scope"]["agents"] = serde_yaml_ng::from_str("[{ name: releaser, harness: shell, lifetime: task, role: foreman, max_sessions: 1 }]").unwrap();
    config["policies"] = serde_yaml_ng::from_str("{ frameworks: [house] }").unwrap();
    config["scope"]["environments"] = serde_yaml_ng::to_value(json!([
        { "name": "staging", "promotes_to": "production", "checks": [{ "name": "source", "kind": "command", "command": "true", "every": "5s", "timeout": "2s" }] },
        { "name": "production", "checks": [{ "name": "installed", "kind": "command", "command": format!("test -f '{}'", marker.display()), "every": "5s", "timeout": "2s" }],
          "deploy": { "agent": "releaser", "prepare": "test \"$(cat release-source)\" = v1", "command": format!("printf '%s' \"$FACTORY_RELEASE_COMMIT\" > '{}'", marker.display()) } }
    ])).unwrap();
    std::fs::write(&config_path, serde_yaml_ng::to_string(&config).unwrap()).unwrap();
    std::fs::create_dir_all(daemon.root.join(".factory/policies")).unwrap();
    let policy = json!({ "framework": "house", "title": "Release policy", "kind": "best-practice", "controls": [
        { "id": "tested", "title": "Selected release checked", "requires": [{ "applies_to": ["release"], "step": "tests",
          "gate": format!("test \"$(cat release-source)\" = v1 && test -f '{}'", permit.display()) }] }
    ] });
    std::fs::write(daemon.root.join(".factory/policies/house.yaml"), serde_yaml_ng::to_string(&policy).unwrap()).unwrap();
    daemon.spawn();
    let base = daemon.base_url();
    let start = format!("{base}/api/deployments");
    let receipt = expect_ok(&start, &post(&start, &json!({ "environment": "staging", "commit": selected })))["deployment"].clone();
    let finish = format!("{base}/api/deployments/{}/finish", receipt["id"].as_str().unwrap());
    let verified = expect_ok(&finish, &post(&finish, &json!({ "status": "succeeded" })));
    assert_eq!(verified["deployment"]["verification"]["ok"], true);
    // Moving the scope's HEAD after verification must not select another release.
    std::fs::write(daemon.root.join("release-source"), "v2").unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git").current_dir(&daemon.root).args(args).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    git(&["add", "release-source"]);
    git(&["commit", "-q", "-m", "next release, not selected"]);
    assert_ne!(git(&["rev-parse", "HEAD"]), selected);
    let promote = format!("{base}/api/environments/promote");
    let selection = json!({ "environment": "staging", "deployment": receipt["id"] });
    let started = expect_ok(&promote, &post(&promote, &selection))["run"].clone();
    let workflow_run_id = started["id"].as_str().unwrap();
    assert_eq!(started["definition"]["workspace_ref"], selected);
    let initial = tasks(&base);
    assert_eq!(initial.len(), 2, "gates are not extra tasks");
    let preflight_id = initial.iter().find(|t| t["title"] == "Release preflight").unwrap()["id"].as_str().unwrap().to_owned();
    let deploy_id = initial.iter().find(|t| t["title"].as_str().unwrap().starts_with("Deploy ")).unwrap()["id"].as_str().unwrap().to_owned();
    wait_for("preflight fails its release policy gate", Duration::from_secs(30), || {
        tasks(&base).into_iter().find(|t| t["id"] == preflight_id && t["status"] == "blocked")
    });
    assert_eq!(tasks(&base).into_iter().find(|t| t["id"] == deploy_id).unwrap()["runs"], 0,
        "the approval node must not bypass the failed upstream gate");
    assert!(!marker.exists());
    let (_, duplicate) = raw_request("POST", &promote, Some(&selection)).unwrap();
    assert!(duplicate.contains("already pending"), "{duplicate}");
    let preflight_runs = format!("{base}/api/tasks/{preflight_id}/runs");
    let first = expect_ok(&preflight_runs, &get(&preflight_runs))["runs"][0].clone();
    std::fs::write(&permit, "allowed").unwrap();
    let rework = format!("{base}/api/runs/{}/rework", first["id"].as_str().unwrap());
    expect_ok(&rework, &post(&rework, &json!({})));
    wait_for("deploy run waits for owner approval", Duration::from_secs(30), || {
        tasks(&base).into_iter().find(|t| t["id"] == deploy_id && t["status"] == "blocked" && t["runs"] == 1)
    });
    assert!(!marker.exists(), "passing preflight is not owner approval");
    daemon.sigterm();
    daemon.spawn();
    assert_eq!(run_status(&base, workflow_run_id)["definition"]["workspace_ref"], selected);
    assert!(!marker.exists(), "restart must not approve a deployment");
    let deploy_runs = format!("{base}/api/tasks/{deploy_id}/runs");
    let held = expect_ok(&deploy_runs, &get(&deploy_runs))["runs"][0].clone();
    assert_eq!(held["blocked_source"], "verification");
    assert!(held.get("session").is_none(), "approval must hold the run before launching its shell");
    assert!(held["required_steps"].as_array().unwrap().iter().any(|step| step["step"] == "environment-promotion" && step["kind"] == "approval"));
    let approve = format!("{base}/api/runs/{}/approve", held["id"].as_str().unwrap());
    expect_ok(&approve, &post(&approve, &json!({ "reason": "reviewed selected release and preflight evidence" })));
    wait_for("verified promotion workflow completes", Duration::from_secs(30), || {
        let run = run_status(&base, workflow_run_id);
        (run["status"] == "done").then_some(run)
    });
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), selected);
    let report_url = format!("{base}/api/environments");
    let report = expect_ok(&report_url, &get(&report_url))["report"].clone();
    let current = &report["environments"].as_array().unwrap().iter().find(|c| c["name"] == "production").unwrap()["current"];
    assert_eq!(current["release"]["commit"], selected);
    assert_eq!(current["verification"]["ok"], true);
    assert_eq!(current["strict_verification"], true);
    assert_eq!(current["manual"], false);
    assert_eq!(current["actor"]["kind"], "run");
    assert_eq!(current["actor"]["task_id"], deploy_id);
    assert_eq!(current["via"], "promotion");
    let (_, redundant) = raw_request("POST", &promote, Some(&selection)).unwrap();
    assert!(redundant.contains("already runs this release"), "{redundant}");
}

#[test]
fn promotion_check_failure_is_not_a_successful_release_or_a_running_orphan() {
    if missing_prerequisites() { return; }
    let mut daemon = provision();
    daemon.sigterm();
    for args in [vec!["config", "user.email", "promotion@example.invalid"], vec!["config", "user.name", "Promotion QA"],
        vec!["commit", "-q", "--allow-empty", "-m", "selected release"]] {
        let output = Command::new("git").current_dir(&daemon.root).args(args).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }
    let commit = Command::new("git").current_dir(&daemon.root).args(["rev-parse", "HEAD"]).output().unwrap();
    let selected = String::from_utf8(commit.stdout).unwrap().trim().to_owned();
    let path = daemon.root.join(".factory/config.yaml");
    let mut config: serde_yaml_ng::Value = serde_yaml_ng::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["scope"]["agents"] = serde_yaml_ng::from_str("[{ name: releaser, harness: shell, lifetime: task, role: foreman }]").unwrap();
    config["scope"]["environments"] = serde_yaml_ng::from_str(
        "- name: staging\n  promotes_to: production\n  checks: [{ kind: command, name: source, command: 'true' }]\n- name: production\n  checks: [{ kind: command, name: installed, command: 'echo wrong-release >&2; exit 1' }]\n  deploy: { agent: releaser, command: 'printf deployed' }\n"
    ).unwrap();
    std::fs::write(&path, serde_yaml_ng::to_string(&config).unwrap()).unwrap();
    daemon.spawn();
    let base = daemon.base_url();
    let url = format!("{base}/api/deployments");
    let source = expect_ok(&url, &post(&url, &json!({ "environment": "staging", "commit": selected })))["deployment"].clone();
    let finish = format!("{url}/{}/finish", source["id"].as_str().unwrap());
    expect_ok(&finish, &post(&finish, &json!({ "status": "succeeded" })));
    let promote = format!("{base}/api/environments/promote");
    expect_ok(&promote, &post(&promote, &json!({ "environment": "staging", "deployment": source["id"] })));
    let held_task = wait_for("promotion owner hold", Duration::from_secs(30), || {
        tasks(&base).into_iter().find(|t| t["title"].as_str().unwrap().starts_with("Deploy ") && t["status"] == "blocked")
    });
    let runs = format!("{base}/api/tasks/{}/runs", held_task["id"].as_str().unwrap());
    let held = expect_ok(&runs, &get(&runs))["runs"][0].clone();
    let approve = format!("{base}/api/runs/{}/approve", held["id"].as_str().unwrap());
    expect_ok(&approve, &post(&approve, &json!({ "reason": "reviewed release" })));
    let report_url = format!("{base}/api/environments");
    let failed = wait_for("mandatory post-deploy check fails", Duration::from_secs(30), || {
        let report = expect_ok(&report_url, &get(&report_url))["report"].clone();
        report["deployments"].as_array().unwrap().iter()
            .find(|d| d["environment"] == "production" && d["status"] == "failed").cloned()
    });
    assert_eq!(failed["strict_verification"], true);
    assert_eq!(failed["verification"]["ok"], false);
    assert!(failed["reason"].as_str().unwrap().contains("wrong-release"));
    let failed_task = wait_for("shell agent reports deploy command failure", Duration::from_secs(15), || {
        tasks(&base).into_iter().find(|t| t["id"] == held_task["id"] && t["status"] == "blocked" && t["error"].is_string())
    });
    assert_eq!(failed_task["runs"], 1);
    daemon.sigterm();
    daemon.spawn();
    let report = expect_ok(&report_url, &get(&report_url))["report"].clone();
    let target = report["environments"].as_array().unwrap().iter().find(|c| c["name"] == "production").unwrap();
    assert!(target.get("current").is_none());
    assert!(target.get("running").is_none());
    assert_eq!(report["deployments"][0]["id"], failed["id"]);
    assert_eq!(report["deployments"][0]["status"], "failed");
}

#[test]
fn waiting_tasks_exist_before_release_and_survive_a_real_restart() {
    if missing_prerequisites() { return; }
    let mut daemon = provision();
    let base = daemon.base_url();
    let release = daemon.root.join("release-parent");
    let instruction = format!("while ! test -f '{}'; do sleep 0.1; done; printf 'bound at dispatch\\n'", release.display());
    let draft = json!({ "name": "waiting", "scope": "demo",
        "nodes": [task_node("parent", &instruction), task_node("child", "cat \"$FACTORY_UPSTREAM_FILE\"")],
        "edges": [edge("parent-child", "parent", "child")] });
    let created = expect_ok(&format!("{base}/api/workflows"), &post(&format!("{base}/api/workflows"), &draft));
    let id = created["workflow"]["id"].as_str().unwrap();
    let started = expect_ok(&format!("{base}/api/workflows/{id}/run"), &post(&format!("{base}/api/workflows/{id}/run"), &json!({})));
    let run_id = started["run"]["id"].as_str().unwrap();
    let initial = tasks(&base);
    assert_eq!(initial.len(), 2, "all task nodes exist at start");
    let parent = initial.iter().find(|task| task["title"] == "parent").unwrap();
    let child = initial.iter().find(|task| task["title"] == "child").unwrap();
    let child_id = child["id"].as_str().unwrap();
    assert_eq!(child["after"], json!([parent["id"]]));
    assert_eq!(child["status"], "pending");
    assert_eq!(child["runs"], 0);
    let (_, refusal) = raw_request("POST", &format!("{base}/api/tasks/{child_id}/run"), Some(&json!({}))).unwrap();
    assert!(refusal.contains("override-wait"), "{refusal}");
    wait_for("parent launch", Duration::from_secs(15), || {
        tasks(&base).into_iter().find(|task| task["id"] == parent["id"] && task["status"] == "running")
    });
    daemon.sigterm();
    daemon.spawn();
    let recovered = tasks(&base);
    assert_eq!(recovered.len(), 2);
    let recovered_child = recovered.iter().find(|task| task["id"] == child["id"]).unwrap();
    assert_eq!(recovered_child["after"], child["after"]);
    assert_eq!(recovered_child["runs"], 0);
    std::fs::write(&release, b"released").unwrap();
    let finished = wait_for("upstream-triggered completion", Duration::from_secs(30), || {
        let run = run_status(&base, run_id);
        (run["status"] == "done").then_some(run)
    });
    assert_eq!(finished["status"], "done");
    let all = tasks(&base);
    let completed = all.iter().find(|task| task["id"] == child["id"]).unwrap();
    assert_eq!(completed["runs"], 1);
    assert!(completed.get("after").is_none());
    assert!(completed["result"].as_str().unwrap().contains("bound at dispatch"));
}

#[test]
fn slow_health_recovers_after_a_shell_task_and_history_survives_restart() {
    if missing_prerequisites() { return; }
    let mut daemon = provision();
    daemon.sigterm();
    let path = daemon.root.join(".factory/config.yaml");
    let mut config: serde_yaml_ng::Value = serde_yaml_ng::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["scope"]["environments"] = serde_yaml_ng::from_str(
        "- name: staging\n  slo: { availability: 99% }\n  checks:\n    - { name: api, kind: command, command: 'if ! test -f healthy; then sleep 0.6; fi', every: 5s, timeout: 2s, slow_after_ms: 250 }\n",
    ).unwrap();
    std::fs::write(&path, serde_yaml_ng::to_string(&config).unwrap()).unwrap();
    daemon.spawn();
    let base = daemon.base_url();
    let report = || {
        let url = format!("{base}/api/environments");
        expect_ok(&url, &get(&url))["report"].clone()
    };
    let slow = wait_for("slow check degrades staging", Duration::from_secs(15), || {
        let r = report();
        (r["environments"][0]["status"] == "degraded").then_some(r)
    });
    let card = &slow["environments"][0];
    assert_eq!(card["uptime_window"], 1.0);
    assert_eq!(card["error_budget"], 1.0);
    assert_eq!(card["incidents"], json!([]));
    assert_eq!(card["checks"][0]["last"]["slow"], true);
    assert!(card["checks"][0]["last"]["detail"].as_str().unwrap().contains("exceeds 250ms"));

    let draft = json!({ "name": "restore-health", "scope": "demo",
        "nodes": [task_node("restore", "touch healthy")], "edges": [] });
    let created = expect_ok(&format!("{base}/api/workflows"), &post(&format!("{base}/api/workflows"), &draft));
    let id = created["workflow"]["id"].as_str().unwrap();
    let started = expect_ok(&format!("{base}/api/workflows/{id}/run"), &post(&format!("{base}/api/workflows/{id}/run"), &json!({})));
    let run_id = started["run"]["id"].as_str().unwrap();
    wait_for("shell agent reports restore done", Duration::from_secs(30), || {
        let run = run_status(&base, run_id);
        (run["status"] == "done").then_some(run)
    });
    let recovered = wait_for("fast check restores staging", Duration::from_secs(15), || {
        let r = report();
        (r["environments"][0]["status"] == "up").then_some(r)
    });
    assert_eq!(recovered["environments"][0]["uptime_window"], 1.0);
    daemon.sigterm();
    // Raising the threshold does not rewrite samples already classified slow.
    config["scope"]["environments"][0]["checks"][0]["slow_after_ms"] = 1000.into();
    std::fs::write(&path, serde_yaml_ng::to_string(&config).unwrap()).unwrap();
    daemon.spawn();
    let persisted = report();
    let card = &persisted["environments"][0];
    assert_eq!(card["status"], "up");
    assert_eq!(card["checks"][0]["slow_after_ms"], 1000);
    assert!(card["checks"][0]["strip"].as_array().unwrap().iter().any(|b| b["slow"].as_u64().unwrap() > 0));
    assert_eq!(card["incidents"], json!([]));
}

#[test]
fn workspace_files_survive_fresh_shell_retry_restart_and_are_released_only_after_close_is_safe() {
    if missing_prerequisites() { return; }
    let mut daemon = provision();
    for args in [vec!["config", "user.email", "workspace@example.invalid"], vec!["config", "user.name", "Workspace QA"]] {
        assert!(Command::new("git").current_dir(&daemon.root).args(args).status().unwrap().success());
    }
    std::fs::write(daemon.root.join(".gitignore"), ".factory/\n").unwrap();
    for args in [vec!["add", ".gitignore"], vec!["commit", "-q", "-m", "base"]] {
        assert!(Command::new("git").current_dir(&daemon.root).args(args).status().unwrap().success());
    }
    let base = daemon.base_url();
    let created = expect_ok(&format!("{base}/api/tasks"), &post(&format!("{base}/api/tasks"), &json!({
        "title": "Workspace continuity", "instructions": "printf 'unfinished work' > unfinished; exit 7",
        "agent": "shell", "worktree": true,
    })));
    let id = created["task"]["id"].as_str().unwrap().to_string();
    expect_ok(&format!("{base}/api/tasks/{id}/run"), &post(&format!("{base}/api/tasks/{id}/run"), &json!({})));
    wait_for("first shell attempt fails", Duration::from_secs(25), || {
        tasks(&base).into_iter().find(|task| task["id"] == id && task["status"] == "blocked")
    });
    let first = expect_ok(&format!("{base}/api/tasks/{id}/runs"), &get(&format!("{base}/api/tasks/{id}/runs")))["runs"][0].clone();
    let path = PathBuf::from(first["worktree_path"].as_str().unwrap());
    assert_eq!(std::fs::read_to_string(path.join("unfinished")).unwrap(), "unfinished work");
    daemon.sigterm();
    daemon.spawn();
    assert!(path.exists());
    let (_, patched) = raw_request("PATCH", &format!("{base}/api/tasks/{id}"), Some(&json!({
        "instructions": "test \"$(cat unfinished)\" = 'unfinished work' && printf 'continued with existing files'"
    }))).unwrap();
    expect_ok("patch instructions", &serde_json::from_str::<Value>(&patched).unwrap());
    expect_ok(&format!("{base}/api/tasks/{id}/run"), &post(&format!("{base}/api/tasks/{id}/run"), &json!({})));
    wait_for("fresh shell retry completes", Duration::from_secs(25), || {
        tasks(&base).into_iter().find(|task| task["id"] == id && task["status"] == "done")
    });
    let second = expect_ok(&format!("{base}/api/tasks/{id}/runs"), &get(&format!("{base}/api/tasks/{id}/runs")))["runs"][0].clone();
    assert_eq!(second["worktree_path"], first["worktree_path"]);
    assert_eq!(second["worktree_branch"], first["worktree_branch"]);
    assert!(second["resumed_session"].is_null(), "shell starts a fresh conversation, not a fake resume");
    assert!(path.exists(), "closed task with untracked work must be retained");
    // The done mirror is published before the asynchronous finish path's
    // workspace sweep. Wait for its retention decision before moving the
    // file, or the sweep may correctly find it already clean and never leave
    // the retention receipt this test asserts below.
    wait_for("unsafe closed workspace is retained and journalled", Duration::from_secs(10), || {
        let url = format!("{base}/api/tasks/{id}/entries");
        let entries = expect_ok(&url, &get(&url));
        entries["entries"].as_array().unwrap().iter()
            .any(|entry| entry["kind"] == "workspace_retained").then_some(entries)
    });
    let saved = daemon.root.join("saved-work");
    std::fs::rename(path.join("unfinished"), &saved).unwrap();
    daemon.sigterm();
    daemon.spawn();
    wait_for("startup sweep releases safe closed workspace", Duration::from_secs(15), || {
        (!path.exists()).then(|| json!({"released": true}))
    });
    assert_eq!(std::fs::read_to_string(saved).unwrap(), "unfinished work");
    let entries = wait_for("release receipt is journalled", Duration::from_secs(10), || {
        let entries = expect_ok(&format!("{base}/api/tasks/{id}/entries"), &get(&format!("{base}/api/tasks/{id}/entries")));
        entries["entries"].as_array().unwrap().iter().any(|entry| entry["kind"] == "workspace_released").then_some(entries)
    });
    let entries = entries["entries"].as_array().unwrap();
    assert!(entries.iter().any(|entry| entry["kind"] == "workspace_retained"));
    assert!(entries.iter().any(|entry| entry["kind"] == "workspace_released"));
}

#[test]
fn diamond_dag_and_restart_recovery_with_the_shell_agent() {
    if missing_prerequisites() {
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

/// Issue #57: a node's task receives the outputs of its direct parents. Node
/// A prints to stdout; node B reads `$FACTORY_UPSTREAM_FILE` back out with
/// `cat`, which is the acceptance bar the design settled on -- a downstream
/// shell command must be able to get at what its parent said with nothing
/// more than that one environment variable.
#[test]
fn a_downstream_shell_node_reads_its_parents_stdout_from_the_upstream_file() {
    if missing_prerequisites() {
        return;
    }

    let daemon = provision();
    let base = daemon.base_url();

    let draft = json!({
        "name": "upstream-outputs",
        "scope": "demo",
        "nodes": [
            task_node("a", "printf 'hello from A\\n'"),
            task_node("b", "cat \"$FACTORY_UPSTREAM_FILE\""),
        ],
        "edges": [edge("ab", "a", "b")],
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

    let finished = wait_for("the upstream-outputs run to finish", Duration::from_secs(30), || {
        let run = run_status(&base, &run_id);
        matches!(run["status"].as_str(), Some("done") | Some("failed") | Some("cancelled"))
            .then_some(run)
    });
    assert_eq!(
        finished["status"], "done",
        "the upstream-outputs run did not succeed: {finished}"
    );

    let all_tasks = tasks(&base);
    let mut by_node = std::collections::HashMap::new();
    for task in &all_tasks {
        let Some(origin) = task["workflow_origin"].as_object() else { continue };
        if origin["workflow_run_id"].as_str() != Some(run_id.as_str()) {
            continue;
        }
        let node_id = origin["node_id"].as_str().unwrap().to_string();
        by_node.insert(node_id, task.clone());
    }
    assert_eq!(by_node.len(), 2, "expected exactly 2 tasks, one per node: {all_tasks:#?}");

    let a_result = by_node["a"]["result"].as_str().unwrap_or_default();
    assert!(a_result.contains("hello from A"), "a's own stdout should be in its result: {a_result:?}");
    assert!(a_result.contains("command exited 0"), "and its exit code: {a_result:?}");

    let b_result = by_node["b"]["result"].as_str().unwrap_or_default();
    assert!(
        b_result.contains("hello from A"),
        "b read a's parent output back out of $FACTORY_UPSTREAM_FILE: {b_result:?}"
    );
}

#[test]
fn feedback_is_two_runs_per_task_through_real_shell_reports_and_restart() {
    if missing_prerequisites() {
        return;
    }
    let mut daemon = provision();
    let base = daemon.base_url();
    let mut review = task_node("review", "touch reviewed; printf 'fix the parser\\n'");
    review["exits"] = json!([{ "to": "implement", "check": "test ! -f fixed", "max_rounds": 2 }]);
    let draft = json!({
        "name": "feedback-rounds", "scope": "demo",
        "nodes": [task_node("implement", "if test -f reviewed; then touch fixed; cat \"$FACTORY_UPSTREAM_FILE\"; else printf 'initial work\\n'; fi"), review],
        "edges": [edge("review-work", "implement", "review")]
    });
    let created = expect_ok(&format!("{base}/api/workflows"), &post(&format!("{base}/api/workflows"), &draft));
    let workflow = created["workflow"]["id"].as_str().unwrap();
    let started = expect_ok(&format!("{base}/api/workflows/{workflow}/run"),
        &post(&format!("{base}/api/workflows/{workflow}/run"), &json!({})));
    let workflow_run = started["run"]["id"].as_str().unwrap();
    let finished = wait_for("feedback rounds to settle", Duration::from_secs(45), || {
        let run = run_status(&base, workflow_run);
        matches!(run["status"].as_str(), Some("done" | "failed" | "cancelled")).then_some(run)
    });
    assert_eq!(finished["status"], "done", "{finished}");
    let original_tasks = tasks(&base);
    assert_eq!(original_tasks.len(), 2, "feedback must not clone tasks: {original_tasks:?}");
    for task in &original_tasks {
        assert_eq!(task["runs"], 2);
        assert_eq!(task["status"], "done");
        assert!(!task["title"].as_str().unwrap().contains("rework"));
        let task_id = task["id"].as_str().unwrap();
        let history = expect_ok(&format!("{base}/api/tasks/{task_id}/runs"),
            &get(&format!("{base}/api/tasks/{task_id}/runs")));
        let runs = history["runs"].as_array().unwrap();
        assert_eq!(runs.len(), 2);
        let second = runs.iter().find(|run| run["attempt"] == 2).unwrap();
        assert_eq!(second["workflow_round"], 1);
        assert_eq!(second["task_id"], task["id"]);
        if task["title"] == "implement" {
            assert!(second["feedback"]["feedback"].as_str().unwrap().contains("fix the parser"));
            assert!(second["result"].as_str().unwrap().contains("fix the parser"));
            assert!(second["result"].as_str().unwrap().contains("review sent this work back"));
        }
        let node = finished["nodes"].as_array().unwrap().iter().find(|node| node["task_id"] == task["id"]).unwrap();
        assert_eq!(node["attempts"].as_array().unwrap().len(), 2);
        assert!(node.get("superseded_task_ids").is_none());
    }
    daemon.sigterm();
    daemon.spawn();
    assert_eq!(run_status(&base, workflow_run)["status"], "done");
    let recovered_tasks = tasks(&base);
    for task in original_tasks {
        assert!(recovered_tasks.iter().any(|recovered| recovered["id"] == task["id"] && recovered["runs"] == 2));
    }
}

/// The real proof against the pty: a command over 1100 bytes -- comfortably
/// past the 1024-byte canonical-mode limit (`MAX_CANON`) that a typed line
/// this long would have been silently cut off by -- still reaches `done`,
/// because `ShellAgent::prompt` only ever types a short `. '<script path>'`
/// regardless of how long the instructions are.
#[test]
fn a_shell_instruction_over_eleven_hundred_bytes_still_reaches_done() {
    if missing_prerequisites() {
        return;
    }

    let daemon = provision();
    let base = daemon.base_url();

    let payload = "y".repeat(1150);
    let instructions = format!("printf '%s\\n' '{payload}'");
    assert!(instructions.len() > 1100, "the instruction really is the long case: {}", instructions.len());

    let draft = json!({
        "name": "long-instruction",
        "scope": "demo",
        "nodes": [task_node("solo", &instructions)],
        "edges": [],
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

    let finished = wait_for("the long-instruction run to finish", Duration::from_secs(30), || {
        let run = run_status(&base, &run_id);
        matches!(run["status"].as_str(), Some("done") | Some("failed") | Some("cancelled"))
            .then_some(run)
    });
    assert_eq!(
        finished["status"], "done",
        "an instruction over 1100 bytes must still reach done via a real pty: {finished}"
    );

    let all_tasks = tasks(&base);
    let task = all_tasks
        .iter()
        .find(|t| t["workflow_origin"]["workflow_run_id"].as_str() == Some(run_id.as_str()))
        .unwrap();
    let result = task["result"].as_str().unwrap_or_default();
    assert!(
        result.contains(&payload),
        "the long stdout should have made it through whole (result is {} bytes)",
        result.len()
    );
}
