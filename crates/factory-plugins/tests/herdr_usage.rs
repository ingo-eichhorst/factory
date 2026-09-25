//! `HerdrRuntime::usage` against a real herdr (#117).
//!
//! Two tests, each skipped -- with a line saying why -- when what it needs is
//! not there:
//!
//! * `a_fixture_plugin_answers_through_an_isolated_herdr` stands up a herdr
//!   server of its own under a throwaway `HOME` (never the company's
//!   session: a separate `HOME` means a separate config, plugin list and
//!   socket), links `tests/fixtures/herdr-usage-plugin`, opens a pane and
//!   reads its usage. Skipped when there is no `herdr` on `PATH`.
//! * `an_installed_usage_plugin_answers_in_the_contract` asks the herdr
//!   session this test process is pointed at (`HERDR_SESSION`) for the
//!   usage of one pane -- `FACTORY_USAGE_TEST_PANE`, or its first -- through
//!   whichever installed plugin offers `usage` (Irrlicht's). Skipped when
//!   that session is not running or no plugin offers the action; with
//!   `HERDR_SESSION=qa-none`, as Factory's tests are run, it always is.

use factory_core::adapter::AgentRuntime;
use factory_core::task::SessionRef;
use factory_plugins::HerdrRuntime;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn herdr_on_path() -> bool {
    Command::new("herdr")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn session(pane: &str) -> SessionRef {
    SessionRef {
        runtime: "herdr".into(),
        handle: pane.into(),
        meta: Default::default(),
    }
}

/// A herdr server nobody else can see: its own `HOME`, so its own config,
/// plugins and socket. Stopped and removed on drop.
struct IsolatedHerdr {
    home: PathBuf,
    server: Child,
}

impl IsolatedHerdr {
    const SESSION: &'static str = "qa-none";

    fn start() -> Self {
        // Short on purpose: a socket path past `sun_path`'s ~104 bytes
        // cannot be bound at all.
        let home = PathBuf::from(format!("/tmp/fu-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
        std::fs::create_dir_all(&home).unwrap();
        let server = Self::command(&home)
            .arg("server")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start herdr server");
        let herdr = Self { home, server };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !herdr.socket().exists() {
            assert!(Instant::now() < deadline, "herdr server did not come up");
            std::thread::sleep(Duration::from_millis(50));
        }
        herdr
    }

    fn command(home: &Path) -> Command {
        let mut c = Command::new("herdr");
        c.env("HOME", home).env("HERDR_SESSION", Self::SESSION);
        for inherited in ["HERDR_SOCKET_PATH", "HERDR_CLIENT_SOCKET_PATH", "HERDR_ENV", "HERDR_PANE_ID"] {
            c.env_remove(inherited);
        }
        c
    }

    fn socket(&self) -> PathBuf {
        self.home.join(".config/herdr/sessions").join(Self::SESSION).join("herdr.sock")
    }

    fn json(&self, args: &[&str]) -> Value {
        let out = Self::command(&self.home).args(args).output().unwrap();
        assert!(out.status.success(), "herdr {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap()
    }
}

impl Drop for IsolatedHerdr {
    fn drop(&mut self) {
        let _ = Self::command(&self.home).args(["server", "stop"]).output();
        let _ = self.server.kill();
        let _ = self.server.wait();
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

#[tokio::test]
async fn a_fixture_plugin_answers_through_an_isolated_herdr() {
    if !herdr_on_path() {
        eprintln!("skipped: no herdr on PATH");
        return;
    }
    let herdr = IsolatedHerdr::start();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/herdr-usage-plugin");
    herdr.json(&["plugin", "link", fixture.to_str().unwrap()]);
    // Two panes, the second not focused: the plugin must be asked about the
    // pane the session is in, not whichever one herdr has in front.
    herdr.json(&["workspace", "create", "--label", "front"]);
    let created = herdr.json(&["workspace", "create", "--label", "back", "--no-focus"]);
    let pane = created
        .pointer("/result/root_pane/pane_id")
        .and_then(Value::as_str)
        .expect("a root pane")
        .to_string();

    let runtime = HerdrRuntime::with_bin("herdr").with_api_socket(herdr.socket());
    let usage = runtime.usage(&session(&pane)).await.unwrap().expect("the fixture plugin answers");
    assert_eq!(usage.handle.as_deref(), Some(pane.as_str()), "asked about the session's own pane");
    let s = &usage.sessions[0];
    assert_eq!(s.tokens.input, Some(1200));
    assert_eq!(s.tokens.cache_read, None);
    assert_eq!(s.unavailable.get("tokens.cache_read").map(String::as_str), Some("the fixture does not say"));
    assert_eq!(s.cost.pricing_source.as_deref(), Some("fixture"));

    herdr.json(&["plugin", "unlink", "factory-usage-fixture"]);
    assert_eq!(
        runtime.usage(&session(&pane)).await.unwrap(),
        None,
        "with no plugin offering usage the runtime has no source, which is not an error"
    );
}

#[tokio::test]
async fn an_installed_usage_plugin_answers_in_the_contract() {
    if !herdr_on_path() {
        eprintln!("skipped: no herdr on PATH");
        return;
    }
    let listed = Command::new("herdr").args(["plugin", "action", "list"]).output().unwrap();
    let offers_usage = listed.status.success()
        && serde_json::from_slice::<Value>(&listed.stdout)
            .ok()
            .and_then(|v| v.pointer("/result/actions").and_then(Value::as_array).cloned())
            .is_some_and(|a| a.iter().any(|a| a.get("action_id").and_then(Value::as_str) == Some("usage")));
    if !offers_usage {
        eprintln!(
            "skipped: no plugin offers `usage` on herdr session {:?}",
            std::env::var("HERDR_SESSION").unwrap_or_default()
        );
        return;
    }
    let pane = match std::env::var("FACTORY_USAGE_TEST_PANE") {
        Ok(p) => p,
        Err(_) => {
            let panes = Command::new("herdr").args(["pane", "list"]).output().unwrap();
            let v: Value = serde_json::from_slice(&panes.stdout).unwrap();
            v.pointer("/result/panes/0/pane_id").and_then(Value::as_str).expect("a pane").to_string()
        }
    };
    let usage = HerdrRuntime::new()
        .usage(&session(&pane))
        .await
        .expect("the installed plugin answers")
        .expect("a plugin offers usage");
    assert_eq!(usage.schema, 1);
    if let Some(handle) = &usage.handle {
        assert_eq!(handle, &pane);
    }
    for s in &usage.sessions {
        assert!(!s.session_id.is_empty());
    }
}
