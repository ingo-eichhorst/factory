//! Scope pages (#343) end to end: a real daemon subprocess, the `shell` agent
//! under `herdr` publishing through the real `factory` callback binary, and
//! -- when Google Chrome is installed -- a real browser showing the page in
//! the real UI, so the isolation claims are tested in a browser rather than
//! asserted about one.
//!
//! Skips (prints why, does not fail) without `herdr` on PATH or the `factory`
//! binary built next to `factory-daemon`, like `workflow_shell_e2e.rs`; the
//! browser half skips on its own without Chrome. Run it on a headless herdr
//! session of its own, never the live one:
//!
//!     env -u HERDR_SOCKET_PATH HERDR_SESSION=qa-343 herdr server &
//!     env -u HERDR_SOCKET_PATH -u FACTORY_SOCKET -u FACTORY_TOKEN HERDR_SESSION=qa-343 \
//!         cargo test -p factory-daemon --test pages_e2e -- --nocapture
//!
//! The rig is a copy of `workflow_shell_e2e.rs`'s, cut down to what this file
//! uses, rather than a shared module both would have to agree on.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const REFERENCE_PAGE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/pages/conference-radar.html");
const REFERENCE_DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/pages/conference-radar.json");
const PROBE: &str = include_str!("fixtures/pages/probe.html");
const CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

// ------------------------------------------------------------- test rig

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
            .env("FACTORY_RENEWALS_DISCOVERY", "0")
            .env_remove("FACTORY_SOCKET")
            .env_remove("FACTORY_TOKEN")
            .env_remove("FACTORY_TASK_TOKEN")
            .env_remove("FACTORY_URL")
            .stdout(Stdio::from(log.try_clone().expect("dup log fd")))
            .stderr(Stdio::from(log))
            .spawn()
            .expect("spawn factory-daemon");
        self.child = Some(child);
        let url = format!("{}/api/status", self.base_url());
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some((200, _, _)) = raw_request("GET", &url, None) {
                return;
            }
            if Instant::now() > deadline {
                let log = std::fs::read_to_string(self.root.join("daemon.log")).unwrap_or_default();
                panic!("daemon never answered /api/status; log:\n{log}");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// A restart, the way `launchctl kickstart -k` does one: SIGTERM, then a
    /// fresh process on the same root.
    fn restart(&mut self) {
        let mut child = self.child.take().expect("daemon is running");
        let pid = child.id().to_string();
        let status = Command::new("kill").args(["-TERM", &pid]).status().expect("run kill(1)");
        assert!(status.success(), "kill -TERM {pid} failed");
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.try_wait().expect("poll child").is_none() {
            assert!(Instant::now() < deadline, "daemon did not exit after SIGTERM");
            std::thread::sleep(Duration::from_millis(50));
        }
        self.spawn();
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

fn find_factory_bin() -> Option<PathBuf> {
    let daemon = PathBuf::from(env!("CARGO_BIN_EXE_factory-daemon"));
    let sibling = daemon.parent()?.join("factory");
    sibling.is_file().then_some(sibling)
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn missing_prerequisites() -> bool {
    if find_on_path("herdr").is_none() {
        eprintln!("skipping: herdr is not on PATH");
        return true;
    }
    if find_factory_bin().is_none() {
        eprintln!("skipping: the `factory` binary is not built next to factory-daemon (cargo build --workspace first)");
        return true;
    }
    false
}

/// One HTTP/1.1 request with `Connection: close`: status, lowercased head,
/// body. `None` for any connection failure.
fn raw_request(method: &str, url: &str, body: Option<&Value>) -> Option<(u16, String, String)> {
    let rest = url.strip_prefix("http://")?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let payload = body.map(|value| serde_json::to_vec(value).unwrap());
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n");
    if let Some(payload) = &payload {
        request.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", payload.len()));
    }
    request.push_str("\r\n");
    let mut stream = std::net::TcpStream::connect(authority).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(30))).ok()?;
    stream.write_all(request.as_bytes()).ok()?;
    if let Some(payload) = &payload {
        stream.write_all(payload).ok()?;
    }
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).ok()?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status: u16 = head.lines().next()?.split_whitespace().nth(1)?.parse().ok()?;
    Some((status, head.to_ascii_lowercase(), body.to_string()))
}

fn call(method: &str, url: &str, body: Option<&Value>) -> Value {
    let (status, _, text) = raw_request(method, url, body).unwrap_or_else(|| panic!("{method} {url}: no response"));
    let value: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{method} {url} json ({e}): {text}"));
    assert_eq!(status, 200, "{method} {url} -> HTTP {status}: {text}");
    assert_eq!(value["status"], "ok", "{method} {url}: {value}");
    value["data"].clone()
}

fn wait_for<F: FnMut() -> Option<Value>>(what: &str, timeout: Duration, mut poll: F) -> Value {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(value) = poll() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out after {timeout:?} waiting for {what}");
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// A fresh instance whose root scope, `demo`, runs the `shell` harness and
/// declares two pages, with the reference page on its dashboard.
fn provision() -> Daemon {
    let herdr_bin = find_on_path("herdr").unwrap();
    let factory_bin = find_factory_bin().unwrap();
    let root = std::env::temp_dir().join(format!("f343-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
    std::fs::create_dir_all(&root).unwrap();
    let init = Command::new(env!("CARGO_BIN_EXE_factory-daemon"))
        .arg("--root")
        .arg(&root)
        .args(["init", "--name", "demo"])
        .output()
        .unwrap();
    assert!(init.status.success(), "init: {}", String::from_utf8_lossy(&init.stderr));
    assert!(Command::new("git").arg("init").arg("--quiet").arg(&root).status().unwrap().success());

    let port = free_port();
    let config_path = root.join(".factory/config.yaml");
    let mut doc: serde_yaml_ng::Value = serde_yaml_ng::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    let interfaces = doc["daemon"]["interfaces"].as_sequence_mut().unwrap();
    interfaces.retain(|i| i["kind"].as_str() != Some("http"));
    interfaces.push(serde_yaml_ng::to_value(json!({"kind": "http", "bind": format!("127.0.0.1:{port}")})).unwrap());
    doc["daemon"]["power_assertion"] = false.into();
    doc["scope"]["agent"] = serde_yaml_ng::to_value(json!({"harness": "shell", "lifetime": "task"})).unwrap();
    doc["scope"]["pages"] = serde_yaml_ng::to_value(json!([
        {"name": "conference-radar", "title": "Konferenz-Radar", "max_bytes": "5MB", "keep": 3},
        {"name": "probe"}
    ]))
    .unwrap();
    doc["dashboard"] = serde_yaml_ng::to_value(json!({"tiles": [
        {"page": "conference-radar", "size": "xl"},
        {"view": "kpis", "size": "xl"}
    ]}))
    .unwrap();
    std::fs::write(&config_path, serde_yaml_ng::to_string(&doc).unwrap()).unwrap();

    let mut daemon = Daemon { child: None, root, port, factory_bin, herdr_bin };
    daemon.spawn();
    daemon
}

/// Run `instructions` as a `shell` task in `demo` and wait for its run to end.
fn shell_task(base: &str, title: &str, instructions: &str) -> Value {
    let created = call(
        "POST",
        &format!("{base}/api/tasks"),
        Some(&json!({"title": title, "instructions": instructions, "scope": "demo", "agent": "shell", "worktree": false})),
    );
    let id = created["task"]["id"].as_str().unwrap().to_string();
    call("POST", &format!("{base}/api/tasks/{id}/run"), Some(&json!({})));
    wait_for(title, Duration::from_secs(60), || {
        let task = call("GET", &format!("{base}/api/tasks/{id}"), None)["task"].clone();
        matches!(task["status"].as_str(), Some("done" | "failed" | "blocked")).then_some(task)
    })
}

fn pages(base: &str) -> Vec<Value> {
    call("GET", &format!("{base}/api/pages?scope=demo"), None)["pages"].as_array().cloned().unwrap()
}

fn page<'a>(pages: &'a [Value], name: &str) -> &'a Value {
    pages.iter().find(|p| p["name"] == name).unwrap_or_else(|| panic!("no page {name}: {pages:?}"))
}

// -------------------------------------------------------------- the tests

/// A `shell` run publishes the reference page through `factory page
/// publish`, journaled on that run; an undeclared page is refused; a
/// rollback survives a restart; and the page is served under its own CSP.
#[test]
fn a_shell_run_publishes_the_reference_page_and_a_rollback_survives_a_restart() {
    if missing_prerequisites() {
        return;
    }
    let mut daemon = provision();
    let base = daemon.base_url();
    assert!(page(&pages(&base), "conference-radar")["current"].is_null(), "declared, not published yet");

    let publish = format!(
        "\"$FACTORY_BIN\" page publish conference-radar --file '{REFERENCE_PAGE}' --data '{REFERENCE_DATA}' && \
         \"$FACTORY_BIN\" page publish conference-radar --file '{REFERENCE_PAGE}' --title 'Without data'"
    );
    let task = shell_task(&base, "publish the radar", &publish);
    assert_eq!(task["status"], "done", "{task}");
    let id = task["id"].as_str().unwrap();

    let entries = call("GET", &format!("{base}/api/tasks/{id}/entries"), None)["entries"].as_array().cloned().unwrap();
    let published: Vec<&Value> = entries.iter().filter(|e| e["kind"] == "page_published").collect();
    assert_eq!(published.len(), 2, "every publish is journaled on the run: {entries:?}");
    assert!(published.iter().all(|e| e["run_id"].is_string() && e["data"]["sha256"].is_string() && e["data"]["bytes"].is_u64()));

    let radar = page(&pages(&base), "conference-radar").clone();
    let versions = radar["versions"].as_array().unwrap();
    assert_eq!(versions.len(), 2);
    assert_eq!(radar["current"]["title"], "Without data");
    let with_data = versions.iter().find(|v| v["data_bytes"].is_u64()).expect("the first publish carried data");
    assert_eq!(with_data["run_id"], published[0]["run_id"]);
    let with_data_sha = with_data["sha256"].as_str().unwrap().to_string();

    // Refused: a page the scope does not declare. The run's own output says why.
    let refused = shell_task(
        &base,
        "publish an undeclared page",
        &format!("\"$FACTORY_BIN\" page publish not-declared --file '{REFERENCE_PAGE}' 2>&1; echo \"exit=$?\""),
    );
    let said = refused["result"].as_str().unwrap_or_default();
    assert!(said.contains("declares no page \"not-declared\"") && said.contains("exit=1"), "{refused}");

    // Roll back to the version with data, then restart the daemon.
    let rolled = call(
        "POST",
        &format!("{base}/api/pages/rollback"),
        Some(&json!({"scope": "demo", "name": "conference-radar", "version": &with_data_sha[..10]})),
    );
    assert_eq!(rolled["page"]["current"]["sha256"], with_data_sha.as_str());
    daemon.restart();
    let radar = page(&pages(&base), "conference-radar").clone();
    assert_eq!(radar["current"]["sha256"], with_data_sha.as_str(), "the rollback survives a restart");

    let (status, head, html) = raw_request("GET", &format!("{base}/pages/demo/conference-radar"), None).unwrap();
    assert_eq!(status, 200);
    assert_eq!(html, std::fs::read_to_string(REFERENCE_PAGE).unwrap());
    assert!(head.contains("content-security-policy: default-src 'none'; script-src 'unsafe-inline'"), "{head}");
    assert!(head.contains("connect-src 'none'") && head.contains("sandbox allow-scripts"), "{head}");

    let dashboard = call("GET", &format!("{base}/api/dashboard"), None);
    assert_eq!(dashboard["page_scopes"]["conference-radar"], "demo", "{dashboard}");

    // In a browser: the dashboard tile renders the reference page and hands
    // it the version's data with postMessage -- and the UI's own baseline CSP
    // refuses nothing the UI itself does, on the dashboard or on the views
    // that load scripts, draw on a canvas or open sockets of their own.
    if let Some(chrome) = chrome() {
        let log = chrome_console(&chrome, &format!("{base}/#all/dash/dashboard"), &daemon.root);
        assert!(
            log.contains("factory-page conference-radar rendered 12 conferences from factory data"),
            "the reference page never rendered the published data:\n{log}"
        );
        assert_ui_csp_quiet(&log, &base);
        for route in ["demo/dash/pages/demo/conference-radar", "all/dash/site", "all/imp/knowledge", "all/dir/policy", "all/harn/occupancy"] {
            assert_ui_csp_quiet(&chrome_console(&chrome, &format!("{base}/#{route}"), &daemon.root), &base);
        }
    }
}

/// No console line reports the UI's own CSP refusing something -- only a
/// page's, from under `/pages/`, may.
fn assert_ui_csp_quiet(log: &str, base: &str) {
    let page_source = format!("source: {base}/pages/");
    let refused: Vec<&str> = log
        .lines()
        .filter(|l| l.contains("Content Security Policy") && !l.contains(&page_source))
        .collect();
    assert!(refused.is_empty(), "the UI's baseline CSP refused the UI itself:\n{}", refused.join("\n"));
}

/// The isolation claims, in a real browser: the probe page, shown in the UI's
/// own Pages view, reaches neither the UI's storage nor `/api/*` nor the
/// network. A control run of the same probe outside the sandbox and CSP
/// proves the probe would have noticed.
#[test]
fn a_page_in_the_ui_cannot_reach_the_api_the_uis_storage_or_the_network() {
    if missing_prerequisites() {
        return;
    }
    let Some(chrome) = chrome() else {
        eprintln!("skipping: Google Chrome is not installed at {CHROME}");
        return;
    };
    let daemon = provision();
    let base = daemon.base_url();

    let canary = Canary::start();
    let probe = PROBE.replace("__CANARY__", &canary.url()).replace("__DAEMON__", &base);
    let published = call(
        "POST",
        &format!("{base}/api/pages/publish"),
        Some(&json!({"scope": "demo", "name": "probe", "html": probe})),
    );
    assert!(published["version"]["sha256"].is_string(), "{published}");

    let log = chrome_console(&chrome, &format!("{base}/#demo/dash/pages/demo/probe"), &daemon.root);
    let results = probe_results(&log);
    assert_eq!(results["origin"], "null", "an opaque origin: {results}");
    for key in ["storage", "parent_storage", "parent_document", "cookie"] {
        let got = results[key].as_str().unwrap_or_default();
        assert!(got.starts_with("blocked:"), "{key} reached the UI: {got} ({results})");
    }
    for key in ["api_status", "api_status_absolute", "api_create_task", "api_create_task_simple", "xhr", "fetch_canary"] {
        let got = results[key].as_str().unwrap_or_default();
        assert!(got.starts_with("blocked:"), "{key} was not refused: {got} ({results})");
    }
    assert_eq!(results["popup_canary"], "opened:false", "{results}");
    assert!(results["top_navigation"].as_str().unwrap_or_default().starts_with("blocked:"), "{results}");
    // Let anything late arrive before counting. No HTTP request reached the
    // canary -- not even the frame navigating itself away, which the UI's own
    // `frame-src 'self'` refuses. Chrome may still open and drop a bare TCP
    // connection for that refused navigation before it checks the policy
    // (no request line, nothing sent); that is counted apart and documented
    // in the README rather than mistaken for a request.
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(canary.hits(), Vec::<String>::new(), "the page reached the network: {results}");
    eprintln!("bare connections the browser opened and dropped: {}", canary.bare());
    let tasks = call("GET", &format!("{base}/api/tasks"), None)["tasks"].as_array().cloned().unwrap();
    assert!(!tasks.iter().any(|t| t["title"] == "probe-pwned"), "a page created a task through /api/*");

    // The control: the same probe, served plainly -- top level, no CSP, no
    // sandbox. Here it must reach storage and the canary, or the assertions
    // above proved nothing.
    let control_canary = Canary::start();
    let plain = Canary::serving(PROBE.replace("__CANARY__", &control_canary.url()).replace("__DAEMON__", &base));
    let log = chrome_console(&chrome, &format!("{}/probe", plain.url()), &daemon.root);
    let results = probe_results(&log);
    assert!(results["storage"].as_str().unwrap_or_default().starts_with("read:"), "control: {results}");
    std::thread::sleep(Duration::from_millis(500));
    let hits = control_canary.hits();
    assert!(hits.iter().any(|h| h.contains("/fetch")), "control canary saw {hits:?}");
    assert!(hits.iter().any(|h| h.contains("/image")), "control canary saw {hits:?}");
}

// --------------------------------------------------------------- browser

fn chrome() -> Option<PathBuf> {
    let path = PathBuf::from(CHROME);
    path.is_file().then_some(path)
}

/// Load `url` in headless Chrome with a throwaway profile and return what
/// every frame logged to the console. Chrome prints each as
/// `INFO:CONSOLE... "message", source: ...` on stderr.
fn chrome_console(chrome: &Path, url: &str, scratch: &Path) -> String {
    let profile = scratch.join(format!("chrome-{}", uuid::Uuid::new_v4().simple()));
    let output = Command::new(chrome)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-sync",
            "--enable-logging=stderr",
            "--v=0",
            "--virtual-time-budget=60000",
            "--dump-dom",
        ])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(url)
        .output()
        .expect("run Chrome");
    let _ = std::fs::remove_dir_all(&profile);
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .filter(|l| l.contains(":CONSOLE"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn probe_results(log: &str) -> Value {
    let line = log
        .lines()
        .find(|l| l.contains("\"FACTORY-PROBE "))
        .unwrap_or_else(|| panic!("the probe never reported; console:\n{log}"));
    let start = line.find("FACTORY-PROBE ").unwrap() + "FACTORY-PROBE ".len();
    let end = line.rfind("\", source:").unwrap();
    serde_json::from_str(&line[start..end]).unwrap_or_else(|e| panic!("probe output ({e}): {line}"))
}

/// A loopback listener that records every request it is sent and answers
/// each with `body` (or an empty 204) -- the canary a page must never reach,
/// and the plain server the control run is served from.
struct Canary {
    port: u16,
    /// The request line of every HTTP request it was sent.
    hits: Arc<Mutex<Vec<String>>>,
    /// Connections that sent nothing before closing.
    bare: Arc<AtomicUsize>,
}

impl Canary {
    fn start() -> Self {
        Self::listen(None)
    }

    fn serving(body: String) -> Self {
        Self::listen(Some(body))
    }

    fn listen(body: Option<String>) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(Mutex::new(Vec::new()));
        let bare = Arc::new(AtomicUsize::new(0));
        let (seen, counted) = (hits.clone(), bare.clone());
        std::thread::spawn(move || {
            let bare = counted;
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let first = String::from_utf8_lossy(&buf[..n]).lines().next().unwrap_or("").to_string();
                if first.is_empty() {
                    bare.fetch_add(1, Ordering::SeqCst);
                    continue;
                }
                seen.lock().unwrap().push(first);
                let response = match &body {
                    Some(html) => format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                        html.len()
                    ),
                    None => "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".to_string(),
                };
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self { port, hits, bare }
    }

    fn bare(&self) -> usize {
        self.bare.load(Ordering::SeqCst)
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn hits(&self) -> Vec<String> {
        self.hits.lock().unwrap().clone()
    }
}
