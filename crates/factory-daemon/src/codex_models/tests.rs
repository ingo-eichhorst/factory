//! Against a fake models endpoint: a local listener that answers like the
//! Codex backend and keeps every request it was sent. Nothing here reaches
//! chatgpt.com or reads the host's own codex login.
use super::*;
use crate::engine::Engine;
use factory_core::adapter::{AgentRuntime, StartRequest};
use factory_core::config::{Config, Scope};
use factory_core::error::Result;
use factory_core::renewals::{DateState, RenewalMilestone};
use factory_core::run::Trigger;
use factory_core::task::{NewTask, SessionRef, TaskStatus};
use factory_plugins::{Registry, SqliteStore};
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const SECRET: &str = "eyJ.fake-access-token.never-printed";

const LIST: &str = r#"{"models":[
    {"slug":"gpt-6-sol","visibility":"list","priority":4,"base_instructions":"long text"},
    {"slug":"gpt-reserve","visibility":"hide","priority":4},
    {"slug":"gpt-6-astra","visibility":"list","priority":2}]}"#;

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn scratch(name: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!("factory-codex-models-{name}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    Scratch(dir)
}

/// Answers every request with `status` and `body`, and keeps each request's
/// head (request line and headers).
async fn endpoint(status: u16, body: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/backend-api/codex/models", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kept = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && socket.read(&mut byte).await.unwrap_or(0) == 1 {
                head.push(byte[0]);
            }
            kept.lock().unwrap().push(String::from_utf8_lossy(&head).into_owned());
            let reply = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(reply.as_bytes()).await;
        }
    });
    (url, seen)
}

/// A cask-shaped codex install whose package says `version`, and whose
/// binary leaves a mark if anything runs it.
fn cask(dir: &Path, version: &str) -> PathBuf {
    let bin = dir.join("Caskroom/codex/pinned/bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(
        dir.join("Caskroom/codex/pinned/codex-package.json"),
        format!(r#"{{"layoutVersion":1,"version":"{version}","variant":"codex","entrypoint":"bin/codex"}}"#),
    )
    .unwrap();
    let codex = bin.join("codex");
    std::fs::write(&codex, format!("#!/bin/sh\ntouch '{}'\necho 'codex-cli 9.9.9'\n", dir.join("ran").display())).unwrap();
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
    codex
}

/// codex's own files: the config's default model and a ChatGPT login.
fn codex_home(dir: &Path, model: &str) -> PathBuf {
    let home = dir.join(".codex");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("config.toml"), format!("check_for_update_on_startup = false\nmodel = \"{model}\"\n")).unwrap();
    std::fs::write(
        home.join("auth.json"),
        format!(r#"{{"auth_mode":"chatgpt","tokens":{{"id_token":"i","access_token":"{SECRET}","refresh_token":"r","account_id":"acct-test"}},"last_refresh":"2026-10-20T00:00:00Z"}}"#),
    )
    .unwrap();
    home
}

fn tools(dir: &Path, codex: &Path, home: &Path) -> Tools {
    let unavailable = dir.join("unavailable").display().to_string();
    Tools {
        openssl: unavailable.clone(),
        tailscale: unavailable.clone(),
        github: unavailable,
        openshell: None,
        metadata_home: dir.to_path_buf(),
        config_home: dir.join(".config"),
        enabled: true,
        codex: codex.display().to_string(),
        codex_home: home.to_path_buf(),
        curl: "curl".into(),
        codex_memo: Default::default(),
    }
}

fn factory(dir: &Path, endpoint: &str) -> Factory {
    let mut config: Config = serde_yaml_ng::from_str(&format!(
        "instance: {{id: a1b2c3d4-0000-4000-8000-000000000000, name: test}}\n\
         daemon: {{power_assertion: false, default_agent: shell, harness_health: {{enabled: false}}}}\n\
         infrastructure: {{codex_models: {{endpoint: \"{endpoint}\"}}}}\n"
    ))
    .unwrap();
    let mut scope: Scope = serde_yaml_ng::from_str(
        "id: demo-id\nname: demo\nruntime: quiet\nagents:\n\
         \x20 - {name: codex, harness: codex, args: [--yolo]}\n\
         \x20 - {name: scout, harness: codex, args: [--yolo, --search, --model, gpt-6-sol]}\n\
         \x20 - {name: builder, harness: codex, lifetime: permanent, args: [--yolo]}\n\
         \x20 - {name: helper, harness: shell}\n",
    )
    .unwrap();
    scope.path = dir.join("scope");
    std::fs::create_dir_all(&scope.path).unwrap();
    config.scopes = vec![scope];
    Factory { root: dir.join("root"), config }
}

struct QuietRuntime;
#[async_trait::async_trait]
impl AgentRuntime for QuietRuntime {
    fn name(&self) -> &str {
        "quiet"
    }
    async fn start(&self, req: &StartRequest) -> Result<SessionRef> {
        Ok(SessionRef { runtime: "quiet".into(), handle: req.id.clone(), meta: Default::default() })
    }
    async fn submit(&self, _: &SessionRef, _: &str) -> Result<()> {
        Ok(())
    }
    async fn status(&self, _: &SessionRef) -> Result<factory_core::adapter::RuntimeStatus> {
        Ok(factory_core::adapter::RuntimeStatus::Working)
    }
    async fn send_text(&self, _: &SessionRef, _: &str) -> Result<()> {
        Ok(())
    }
    async fn send_keys(&self, _: &SessionRef, _: &[String]) -> Result<()> {
        Ok(())
    }
    async fn read(&self, _: &SessionRef, _: u32) -> Result<String> {
        Ok(String::new())
    }
    async fn stop(&self, _: &SessionRef) -> Result<()> {
        Ok(())
    }
}

fn engine(factory: Factory) -> Arc<Engine> {
    let mut registry = Registry::with_builtins();
    registry.add_runtime(Arc::new(QuietRuntime), "test");
    Arc::new(Engine::new(
        factory,
        registry,
        Arc::new(SqliteStore::in_memory().unwrap()),
        PathBuf::from("/bin/factory"),
        Vec::new(),
    ))
}

/// Observe against `url` and record it, as the important-dates job does.
async fn observe_into(engine: &Engine, dir: &Path, codex: &Path, home: &Path) {
    let snapshot = engine.factory_snapshot();
    let probe = probe(&snapshot, &tools(dir, codex, home), Utc::now()).await.expect("codex agents are declared");
    engine.l1.record_observations(Vec::new(), true, Some(probe)).await.unwrap();
}

#[tokio::test]
async fn the_list_is_asked_for_the_installed_client_with_the_login_as_headers_and_no_secret_survives() {
    let s = scratch("observe");
    let (url, seen) = endpoint(200, LIST).await;
    let codex = cask(&s.0, "0.157.0");
    let home = codex_home(&s.0, "gpt-6.1-sol");
    let engine = engine(factory(&s.0, &url));

    let snapshot = engine.factory_snapshot();
    let job = tools(&s.0, &codex, &home);
    let probe = probe(&snapshot, &job, Utc::now()).await.unwrap();
    assert_eq!(probe.observation.client_version.as_deref(), Some("0.157.0"));
    assert!(!s.0.join("ran").exists(), "the version came from the package, codex never ran");
    assert_eq!(probe.agents.len(), 3, "the shell agent is not a codex agent");
    let slugs: Vec<_> = probe.observation.served.iter().map(|m| (m.slug.as_str(), m.hidden)).collect();
    assert_eq!(slugs, [("gpt-6-astra", false), ("gpt-6-sol", false), ("gpt-reserve", true)]);

    let requests = seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    let request = requests[0].to_ascii_lowercase();
    assert!(request.starts_with("get /backend-api/codex/models?client_version=0.157.0 "), "{request}");
    assert!(request.contains(&format!("authorization: bearer {}", SECRET.to_ascii_lowercase())), "{request}");
    assert!(request.contains("chatgpt-account-id: acct-test"), "{request}");
    assert!(!format!("{:?}", probe.observation).contains(SECRET));
    // The job's next run, five minutes on, uses the list it has.
    let again = super::probe(&snapshot, &job, Utc::now() + chrono::Duration::minutes(5)).await.unwrap();
    assert_eq!((again.observation.observed_at, again.observation.served.len()), (probe.observation.observed_at, 3));
    assert_eq!(seen.lock().unwrap().len(), 1, "asked at most hourly");
    let later = super::probe(&snapshot, &job, Utc::now() + chrono::Duration::minutes(61)).await.unwrap();
    assert!(later.observation.observed_at > probe.observation.observed_at);
    assert_eq!(seen.lock().unwrap().len(), 2);

    engine.l1.record_observations(Vec::new(), true, Some(probe)).await.unwrap();
    let fact = engine.l1_service().codex_models().await.expect("codex agents are declared");
    assert_eq!((fact.default_model.as_deref(), fact.default_served), (Some("gpt-6.1-sol"), Some(false)));
    let rows: Vec<_> = fact.agents.iter().map(|a| (a.agent.as_str(), a.model.as_deref(), a.served)).collect();
    assert_eq!(
        rows,
        [
            ("codex", Some("gpt-6.1-sol"), Some(false)),
            ("scout", Some("gpt-6-sol"), Some(true)),
            ("builder", Some("gpt-6.1-sol"), Some(false)),
        ]
    );

    // Like an expired credential: one dated entry per model in use.
    let report = engine.important_dates(None).await.unwrap();
    let entry = |id: &str| report.entries.iter().find(|e| e.observation.id == id).unwrap_or_else(|| panic!("{id}: {report:?}"));
    let unserved = entry("codex-model:gpt-6.1-sol");
    assert_eq!((unserved.state, unserved.milestone), (DateState::Overdue, Some(RenewalMilestone::Expired)));
    assert_eq!(unserved.observation.source, DateSource::CodexModel);
    assert!(unserved.observation.detail.contains("gpt-6-astra, gpt-6-sol"), "{}", unserved.observation.detail);
    let labels: Vec<_> = unserved.observation.affects.iter().map(|d| d.label.as_str()).collect();
    assert_eq!(labels, ["demo/codex", "demo/builder"]);
    let served = entry("codex-model:gpt-6-sol");
    assert_eq!(served.state, DateState::Ok);
    assert!(served.observation.no_expiry);
    let first = unserved.observation.expires_at;

    for shown in [serde_json::to_string(&fact).unwrap(), serde_json::to_string(&report).unwrap()] {
        assert!(!shown.contains(SECRET) && !shown.contains("acct-test"), "{shown}");
    }

    // Still unserved five minutes later: the same item, not a new one.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    observe_into(&engine, &s.0, &codex, &home).await;
    let again = engine.important_dates(None).await.unwrap();
    let again = again.entries.iter().find(|e| e.observation.id == "codex-model:gpt-6.1-sol").unwrap();
    assert_eq!(again.observation.expires_at, first, "first seen unserved, kept");

    // The owner pins a served model in config.toml: served from the next read.
    std::fs::write(home.join("config.toml"), "model = \"gpt-6-sol\"\n").unwrap();
    let fact = engine.l1_service().codex_models().await.unwrap();
    assert!(fact.agents.iter().all(|a| a.served == Some(true)), "{fact:?}");
}

#[tokio::test]
async fn a_refused_login_a_silent_endpoint_or_no_login_is_an_issue_and_decides_nothing() {
    let s = scratch("issues");
    let codex = cask(&s.0, "0.157.0");
    let home = codex_home(&s.0, "gpt-6.1-sol");

    let (refusing, _) = endpoint(401, r#"{"detail":{"code":"token_expired"}}"#).await;
    let engine = engine(factory(&s.0, &refusing));
    let snapshot = engine.factory_snapshot();
    let refused = probe(&snapshot, &tools(&s.0, &codex, &home), Utc::now()).await.unwrap();
    let issue = refused.observation.issue.clone().unwrap();
    assert!(issue.contains("401") && !issue.contains(SECRET) && !issue.contains("token_expired"), "{issue}");
    engine.l1.record_observations(Vec::new(), true, Some(refused)).await.unwrap();
    let fact = engine.l1_service().codex_models().await.unwrap();
    assert!(fact.agents.iter().all(|a| a.served.is_none()) && fact.default_served.is_none(), "{fact:?}");
    let report = engine.important_dates(None).await.unwrap();
    let entry = report.entries.iter().find(|e| e.observation.id == "codex-model:gpt-6.1-sol").unwrap();
    assert_eq!(entry.state, DateState::Unknown);
    assert_eq!(entry.milestone, None, "no Inbox item from an unknown");

    // Nothing listening any more.
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}/m", listener.local_addr().unwrap())
    };
    let silent = probe(&factory(&s.0, &closed), &tools(&s.0, &codex, &home), Utc::now()).await.unwrap();
    assert_eq!(silent.observation.issue.as_deref(), Some("the models endpoint did not answer"));

    std::fs::remove_file(home.join("auth.json")).unwrap();
    let (url, seen) = endpoint(200, LIST).await;
    let no_login = probe(&factory(&s.0, &url), &tools(&s.0, &codex, &home), Utc::now()).await.unwrap();
    assert!(no_login.observation.issue.unwrap().contains("codex login"));
    assert!(seen.lock().unwrap().is_empty(), "no login, no request");

    let missing = probe(&factory(&s.0, &url), &tools(&s.0, &s.0.join("no-codex"), &home), Utc::now()).await.unwrap();
    assert!(missing.observation.issue.unwrap().contains("not found"));

    codex_home(&s.0, "gpt-6.1-sol");
    let elsewhere = probe(&factory(&s.0, "http://example.invalid/m"), &tools(&s.0, &codex, &home), Utc::now()).await.unwrap();
    assert!(elsewhere.observation.issue.unwrap().contains("neither https nor"));
}

#[tokio::test]
async fn nothing_is_asked_without_a_codex_agent_or_with_the_check_off() {
    let s = scratch("off");
    let (url, seen) = endpoint(200, LIST).await;
    let codex = cask(&s.0, "0.157.0");
    let home = codex_home(&s.0, "gpt-6.1-sol");
    let mut off = factory(&s.0, &url);
    off.config.infrastructure.codex_models.enabled = false;
    assert!(probe(&off, &tools(&s.0, &codex, &home), Utc::now()).await.is_none());
    let mut none = factory(&s.0, &url);
    none.config.scopes[0].agents.retain(|a| a.harness != "codex");
    assert!(probe(&none, &tools(&s.0, &codex, &home), Utc::now()).await.is_none());
    assert!(seen.lock().unwrap().is_empty());
}

async fn task(engine: &Arc<Engine>, agent: &str) -> factory_core::task::Task {
    engine
        .l4_service()
        .create(NewTask {
            title: format!("work for {agent}"),
            instructions: "true".into(),
            scope: Some("demo".into()),
            agent: Some(agent.into()),
            worktree: Some(false),
            ..Default::default()
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn an_agent_whose_model_is_not_served_is_refused_before_a_run_and_a_served_one_dispatches() {
    let s = scratch("dispatch");
    let (url, _) = endpoint(200, LIST).await;
    let codex = cask(&s.0, "0.157.0");
    let home = codex_home(&s.0, "gpt-6.1-sol");
    let engine = engine(factory(&s.0, &url));

    // Before anything was observed, nothing is known and nothing refused.
    let early = task(&engine, "codex").await;
    engine.l4_service().start_run(&early.id, Trigger::Manual).await;
    assert_eq!(engine.l4.store.runs(&early.id, 10).await.unwrap().len(), 1, "unknown refuses nothing");

    observe_into(&engine, &s.0, &codex, &home).await;
    let refused = task(&engine, "codex").await;
    engine.l4_service().start_run(&refused.id, Trigger::Manual).await;
    let now = engine.l4.store.get(&refused.id).await.unwrap().unwrap();
    assert_eq!(now.status, TaskStatus::Blocked);
    assert!(engine.l4.store.runs(&refused.id, 10).await.unwrap().is_empty(), "no run, no session, no TUI");
    let why = now.error.unwrap_or_default();
    assert!(why.contains("model gpt-6.1-sol") && why.contains("config.toml") && why.contains("codex 0.157.0"), "{why}");
    assert!(why.contains("served: gpt-6-astra, gpt-6-sol."), "{why}");

    let served = task(&engine, "scout").await;
    engine.l4_service().start_run(&served.id, Trigger::Manual).await;
    let runs = engine.l4.store.runs(&served.id, 10).await.unwrap();
    assert_eq!(runs.len(), 1, "a served model dispatches");
    assert!(runs[0].error.is_none(), "{:?}", runs[0].error);

    // A standing codex agent with the same default fails to start, saying why.
    let started = engine.l3_service().start_agent("demo", "builder").await;
    let error = started.err().expect("refused").to_string();
    assert!(error.contains("gpt-6.1-sol") && error.contains("not served"), "{error}");
}
