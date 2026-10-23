//! `#338` against a local fake Bot API, with real `curl`: the one
//! `getUpdates` consumer per token, the allowlist, files in, and the gateway
//! end to end -- a message acknowledged and answered when its run reports, a
//! stranger journaled and never answered, `/jobs`, `/run`, a confirmed
//! reschedule journaled as the owner's, and intake. Runs go to a quiet fake
//! runtime: nothing here reaches herdr or Telegram.

use super::TelegramInterface;
use crate::engine::Engine;
use factory_core::adapter::interface::{Interface, InterfaceContext};
use factory_core::adapter::{AgentRuntime, StartRequest};
use factory_core::protocol::{Envelope, Payload, Request, Response};
use factory_core::run::RunStatus;
use factory_core::task::{NewTask, Schedule, SessionRef, Task, TaskReport};
use factory_environment::channels as ch;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const CHAT: i64 = 4242424242;
const OWNER: i64 = 7_000_001;
const STRANGER: i64 = 9_000_009;

/// One request the fake Bot API saw.
#[derive(Clone, Debug)]
pub(crate) struct Call {
    path: String,
    body: Vec<u8>,
}

impl Call {
    fn method(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or_default()
    }

    /// A multipart form field's value, as curl sent it.
    fn field(&self, name: &str) -> Option<String> {
        let body = String::from_utf8_lossy(&self.body);
        let marker = format!("name=\"{name}\"\r\n\r\n");
        let start = body.find(&marker)? + marker.len();
        let end = body[start..].find("\r\n--").map(|at| start + at).unwrap_or(body.len());
        Some(body[start..end].to_string())
    }

    fn text(&self) -> String {
        self.field("text").unwrap_or_default()
    }

    fn replies_to(&self) -> Option<i64> {
        let parameters: serde_json::Value = serde_json::from_str(&self.field("reply_parameters")?).ok()?;
        parameters["message_id"].as_i64()
    }

    /// The `callback_data` of the button labelled `label`.
    fn button(&self, label: &str) -> Option<String> {
        let markup: serde_json::Value = serde_json::from_str(&self.field("reply_markup")?).ok()?;
        markup["inline_keyboard"][0]
            .as_array()?
            .iter()
            .find(|b| b["text"] == label)
            .and_then(|b| b["callback_data"].as_str().map(str::to_string))
    }
}

#[derive(Default)]
struct Bot {
    calls: Mutex<Vec<Call>>,
    updates: Mutex<Vec<serde_json::Value>>,
    files: Mutex<HashMap<String, (String, Vec<u8>)>>,
    conflict: AtomicBool,
    next: AtomicI64,
}

/// A local stand-in for `api.telegram.org`, one thread per connection so a
/// long-held `getUpdates` never blocks a send. `getUpdates` answers what is
/// queued at or after its offset -- dropping what the offset confirms -- or
/// waits for it, up to its timeout; in conflict mode it answers `409`.
pub(crate) struct FakeBotApi {
    port: u16,
    bot: Arc<Bot>,
}

impl FakeBotApi {
    pub(crate) fn start() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let bot = Arc::new(Bot { next: AtomicI64::new(1000), ..Default::default() });
        let shared = bot.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let bot = shared.clone();
                std::thread::spawn(move || {
                    let _ = answer(stream, &bot);
                });
            }
        });
        Self { port, bot }
    }

    pub(crate) fn api(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub(crate) fn calls(&self) -> Vec<Call> {
        self.bot.calls.lock().unwrap().clone()
    }

    fn sent(&self) -> Vec<Call> {
        self.calls().into_iter().filter(|c| c.method() == "sendMessage").collect()
    }

    pub(crate) fn push(&self, update: serde_json::Value) {
        self.bot.updates.lock().unwrap().push(update);
    }

    fn file(&self, id: &str, path: &str, bytes: &[u8]) {
        self.bot.files.lock().unwrap().insert(id.into(), (path.into(), bytes.to_vec()));
    }
}

fn respond(mut stream: std::net::TcpStream, status: &str, kind: &str, body: &[u8]) -> std::io::Result<()> {
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())?;
    stream.write_all(body)?;
    stream.flush()
}

fn answer(stream: std::net::TcpStream, bot: &Bot) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut first = String::new();
    reader.read_line(&mut first)?;
    let path = first.split_whitespace().nth(1).unwrap_or_default().to_string();
    let (mut length, mut expect) = (0usize, false);
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            length = v.trim().parse().unwrap_or(0);
        }
        expect |= lower.starts_with("expect:") && lower.contains("100-continue");
    }
    let mut writer = stream;
    if expect {
        writer.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let call = Call { path: path.clone(), body };
    bot.calls.lock().unwrap().push(call.clone());
    let json = |value: serde_json::Value| value.to_string().into_bytes();
    if let Some(rest) = path.strip_prefix("/file/bot") {
        let wanted = rest.split_once('/').map(|(_, p)| p).unwrap_or_default();
        let found = bot.files.lock().unwrap().values().find(|(p, _)| p == wanted).map(|(_, b)| b.clone());
        return match found {
            Some(bytes) => respond(writer, "200 OK", "application/octet-stream", &bytes),
            None => respond(writer, "404 Not Found", "text/plain", b"no such file"),
        };
    }
    match call.method() {
        "getUpdates" => {
            if bot.conflict.load(Ordering::SeqCst) {
                let refusal = serde_json::json!({ "ok": false, "error_code": 409,
                    "description": "Conflict: terminated by other getUpdates request; make sure that only one bot instance is running" });
                return respond(writer, "409 Conflict", "application/json", &json(refusal));
            }
            let offset: i64 = call.field("offset").and_then(|o| o.parse().ok()).unwrap_or(0);
            let wait: u64 = call.field("timeout").and_then(|t| t.parse().ok()).unwrap_or(0);
            let deadline = std::time::Instant::now() + Duration::from_secs(wait.min(3));
            loop {
                let ready: Vec<serde_json::Value> = {
                    let mut queued = bot.updates.lock().unwrap();
                    queued.retain(|u| u["update_id"].as_i64().unwrap_or(0) >= offset);
                    queued.clone()
                };
                if !ready.is_empty() || std::time::Instant::now() >= deadline {
                    return respond(writer, "200 OK", "application/json", &json(serde_json::json!({ "ok": true, "result": ready })));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        "getFile" => {
            let id = call.field("file_id").unwrap_or_default();
            match bot.files.lock().unwrap().get(&id) {
                Some((path, bytes)) => respond(
                    writer,
                    "200 OK",
                    "application/json",
                    &json(serde_json::json!({ "ok": true, "result": { "file_id": id, "file_path": path, "file_size": bytes.len() } })),
                ),
                None => respond(
                    writer,
                    "400 Bad Request",
                    "application/json",
                    &json(serde_json::json!({ "ok": false, "error_code": 400, "description": "Bad Request: invalid file_id" })),
                ),
            }
        }
        "answerCallbackQuery" => respond(writer, "200 OK", "application/json", &json(serde_json::json!({ "ok": true, "result": true }))),
        _ => {
            let id = bot.next.fetch_add(1, Ordering::SeqCst);
            respond(writer, "200 OK", "application/json", &json(serde_json::json!({ "ok": true, "result": { "message_id": id } })))
        }
    }
}

/// A run that starts and then says nothing: the test reports for it.
struct QuietRuntime;

#[async_trait::async_trait]
impl AgentRuntime for QuietRuntime {
    fn name(&self) -> &str {
        "quiet"
    }
    async fn start(&self, req: &StartRequest) -> factory_core::error::Result<SessionRef> {
        Ok(SessionRef { runtime: "quiet".into(), handle: req.id.clone(), meta: Default::default() })
    }
    async fn submit(&self, _: &SessionRef, _: &str) -> factory_core::error::Result<()> {
        Ok(())
    }
    async fn status(&self, _: &SessionRef) -> factory_core::error::Result<factory_core::adapter::RuntimeStatus> {
        Ok(factory_core::adapter::RuntimeStatus::Working)
    }
    async fn send_text(&self, _: &SessionRef, _: &str) -> factory_core::error::Result<()> {
        Ok(())
    }
    async fn send_keys(&self, _: &SessionRef, _: &[String]) -> factory_core::error::Result<()> {
        Ok(())
    }
    async fn read(&self, _: &SessionRef, _: u32) -> factory_core::error::Result<String> {
        Ok(String::new())
    }
    async fn stop(&self, _: &SessionRef) -> factory_core::error::Result<()> {
        Ok(())
    }
}

pub(crate) struct Rig {
    pub(crate) engine: Arc<Engine>,
    pub(crate) root: PathBuf,
    pub(crate) bot: FakeBotApi,
    token: String,
    shutdown: Option<tokio::sync::watch::Sender<bool>>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        if let Some(stop) = self.shutdown.take() {
            let _ = stop.send(true);
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn private_file(path: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

/// An engine with the scope `demo` (on the quiet runtime) and the channel
/// `owner-telegram` taking messages in from `OWNER` as tasks for `shell`.
/// `token` is the bot token: a second rig with the same one is a second
/// consumer of the same bot.
pub(crate) fn rig_with(token: &str, bot: FakeBotApi, inbound_extra: &str) -> Rig {
    rig_on(token, bot, inbound_extra, false)
}

/// [`rig`], with blocked runs' questions sent to the channel
/// (`daemon.escalation.channel: owner-telegram`).
fn rig_escalating() -> Rig {
    rig_on(&fresh_token(), FakeBotApi::start(), "", true)
}

fn rig_on(token: &str, bot: FakeBotApi, inbound_extra: &str, escalate: bool) -> Rig {
    use factory_core::config::{Config, DaemonConfig, Factory, Instance};
    use factory_plugins::{Registry, SqliteStore};
    let root = std::env::temp_dir().join(format!("factory-telegram-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join(".factory")).unwrap();
    std::fs::create_dir_all(root.join("projects/demo/out")).unwrap();
    let token_file = root.join(".factory/bot-token");
    private_file(&token_file, &format!("{token}\n"));
    let yaml = format!(
        "secrets:\n\
         - {{ name: telegram-bot-token, kind: token, source: {{ from: file, path: {} }}, expires: never }}\n\
         channels:\n\
         - {{ name: owner-telegram, kind: telegram, token: {{ secret: telegram-bot-token }}, chat: {{ from: command, run: 'printf {CHAT}' }}, \
              api: '{}', inbound: {{ users: [{OWNER}], scope: demo, agent: shell{inbound_extra} }} }}\n",
        token_file.display(),
        bot.api(),
    );
    #[derive(serde::Deserialize)]
    struct Blocks {
        secrets: Vec<factory_core::secrets::SecretDecl>,
        channels: Vec<factory_core::channels::ChannelDecl>,
    }
    let blocks: Blocks = serde_yaml_ng::from_str(&yaml).unwrap();
    let demo: factory_core::config::Scope =
        serde_yaml_ng::from_str("id: demo-id\nname: demo\npath: projects/demo\nruntime: quiet\n").unwrap();
    let config = Config {
        instance: Instance { id: "test".into(), name: "test".into() },
        daemon: DaemonConfig {
            power_assertion: false,
            escalation: factory_core::config::EscalationConfig { channel: escalate.then(|| "owner-telegram".to_string()) },
            ..DaemonConfig::default()
        },
        scopes: vec![demo],
        secrets: blocks.secrets,
        channels: blocks.channels,
        ..Default::default()
    };
    config.validate().unwrap();
    let store: Arc<dyn factory_core::adapter::TaskStore> = Arc::new(SqliteStore::open(&root.join(".factory/factory.sqlite")).unwrap());
    let mut registry = Registry::with_builtins();
    registry.add_runtime(Arc::new(QuietRuntime), "test");
    let factory = Factory { root: root.clone(), config };
    let engine = Arc::new(Engine::new(factory, registry, store, PathBuf::from("/bin/factory"), Vec::new()));
    Rig { engine, root, bot, token: token.into(), shutdown: None }
}

fn fresh_token() -> String {
    format!("{}:TEST-{}", 100000 + (uuid::Uuid::new_v4().as_u128() % 800000), uuid::Uuid::new_v4().simple())
}

pub(crate) fn rig() -> Rig {
    rig_with(&fresh_token(), FakeBotApi::start(), "")
}

fn message(update: i64, id: i64, from: i64, text: &str) -> serde_json::Value {
    serde_json::json!({ "update_id": update, "message": {
        "message_id": id, "date": 1_790_000_000, "from": { "id": from, "is_bot": false }, "chat": { "id": CHAT, "type": "private" }, "text": text } })
}

/// The owner's reply, with Telegram's reply, to the channel's message `to`.
fn reply(update: i64, id: i64, to: i64, text: &str) -> serde_json::Value {
    let mut update = message(update, id, OWNER, text);
    update["message"]["reply_to_message"] = serde_json::json!({ "message_id": to, "date": 0, "chat": { "id": CHAT } });
    update
}

fn press(update: i64, from: i64, data: &str, under: i64) -> serde_json::Value {
    serde_json::json!({ "update_id": update, "callback_query": {
        "id": format!("Q{update}"), "from": { "id": from }, "data": data, "message": { "message_id": under, "chat": { "id": CHAT } } } })
}

async fn wait_for<T>(what: &str, mut found: impl FnMut() -> Option<T>) -> T {
    for _ in 0..300 {
        if let Some(value) = found() {
            return value;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

impl Rig {
    async fn ask(&self, request: Request, token: Option<&str>) -> Response {
        self.engine.handle(Envelope { request, token: token.map(str::to_string) }).await
    }

    async fn poll(&self, consumer: &str, offset: Option<i64>) -> Result<ch::InboundBatch, (String, String)> {
        let request = Request::ChannelPoll { channel: "owner-telegram".into(), consumer: consumer.into(), offset, wait: Some(0) };
        match self.ask(request, None).await {
            Response::Ok { data: Payload::ChannelUpdates { batch } } => Ok(batch),
            Response::Error { code, message } => Err((code, message)),
            other => panic!("{other:?}"),
        }
    }

    async fn journal(&self, id: &str) -> Vec<factory_core::task::TaskEntry> {
        self.engine.l4.store.entries(id, 200).await.unwrap()
    }

    async fn tasks(&self) -> Vec<Task> {
        self.engine.l4.store.list(&Default::default()).await.unwrap()
    }

    /// The first entry of `kind` on `id`'s journal, once there is one.
    async fn entry(&self, id: &str, kind: &str) -> factory_core::task::TaskEntry {
        for _ in 0..300 {
            if let Some(entry) = self.journal(id).await.into_iter().find(|e| e.kind == kind) {
                return entry;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("no {kind} entry on {id}");
    }

    /// Start the gateway, as the daemon mounts it.
    fn serve(&mut self) {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let ctx = InterfaceContext {
            config: factory_core::config::InterfaceConfig { kind: "telegram".into(), settings: Default::default() },
            root: self.root.clone(),
            factory_dir: self.root.join(".factory"),
        };
        let engine = self.engine.clone();
        tokio::spawn(async move { Arc::new(TelegramInterface::new("owner-telegram")).serve(engine, ctx, rx).await });
        self.shutdown = Some(tx);
    }

    /// The task a message became, once it has a run.
    async fn message_task(&self, message: i64) -> (Task, String) {
        for _ in 0..300 {
            let task = self.tasks().await.into_iter().find(|t| t.labels.get(ch::LABEL_MESSAGE) == Some(&message.to_string()));
            if let Some(task) = task {
                if let Some(run) = self.engine.l4.store.active_run(&task.id).await.unwrap() {
                    if let Some(token) = run.token.clone() {
                        return (task, token);
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("message {message} never became a running task");
    }

    async fn report_done(&self, task: &Task, run_token: &str, result: &str) {
        self.report(task, run_token, RunStatus::Done, None, Some(result)).await
    }

    async fn report(&self, task: &Task, run_token: &str, status: RunStatus, message: Option<&str>, result: Option<&str>) {
        let report = TaskReport {
            artifacts: Vec::new(),
            status: Some(status),
            message: message.map(str::to_string),
            result: result.map(str::to_string),
            send_to: None,
            error: None,
            token: Some(run_token.into()),
        };
        match self.ask(Request::TaskReport { id: task.id.clone(), report }, None).await {
            Response::Ok { .. } => {}
            other => panic!("report: {other:?}"),
        }
    }
}

// ------------------------------------------------------------------ poll

#[tokio::test]
async fn a_poll_takes_an_allowed_message_and_its_file_and_keeps_nothing_of_a_strangers() {
    let rig = rig();
    rig.bot.file("DOC-1", "documents/file_7.pdf", b"%PDF-1.4 the owner's report");
    rig.bot.push(message(1, 10, OWNER, "hallo"));
    rig.bot.push(message(2, 11, STRANGER, "STRANGER-SECRET"));
    rig.bot.push(serde_json::json!({ "update_id": 3, "message": {
        "message_id": 12, "date": 1_790_000_000, "from": { "id": OWNER }, "chat": { "id": CHAT }, "caption": "lies das",
        "document": { "file_id": "DOC-1", "file_name": "Bericht 2026.pdf", "mime_type": "application/pdf", "file_size": 28 } } }));
    rig.bot.push(serde_json::json!({ "update_id": 4, "message": {
        "message_id": 13, "date": 0, "from": { "id": STRANGER }, "chat": { "id": CHAT },
        "document": { "file_id": "DOC-1", "file_name": "x.pdf", "file_size": 28 } } }));
    rig.bot.push(press(5, OWNER, "ok:abc", 99));

    let batch = rig.poll("gateway-a", None).await.unwrap();
    assert_eq!(batch.next_offset, Some(6));
    assert_eq!(batch.items.len(), 5);
    assert!(matches!(&batch.items[0], ch::Inbound::Message { text, from: OWNER, message: 10, .. } if text == "hallo"));
    assert!(matches!(&batch.items[1], ch::Inbound::Ignored { from: Some(STRANGER), .. }));
    let ch::Inbound::Message { files, text, .. } = &batch.items[2] else { panic!("{:?}", batch.items[2]) };
    assert_eq!(text, "lies das");
    let saved = Path::new(&files[0].path);
    assert!(saved.starts_with(rig.root.join(".factory/channels/owner-telegram/inbox")), "{saved:?}");
    assert!(saved.file_name().unwrap().to_string_lossy().ends_with("3-0-Bericht_2026.pdf"));
    assert_eq!(std::fs::read(saved).unwrap(), b"%PDF-1.4 the owner's report");
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(saved).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert!(matches!(&batch.items[3], ch::Inbound::Ignored { from: Some(STRANGER), .. }));
    assert!(matches!(&batch.items[4], ch::Inbound::Button { data, .. } if data == "ok:abc"));
    assert!(!serde_json::to_string(&batch).unwrap().contains("STRANGER-SECRET"));

    let calls = rig.bot.calls();
    let methods: Vec<&str> = calls.iter().map(Call::method).collect();
    assert_eq!(methods.iter().filter(|m| **m == "getFile").count(), 1, "a stranger's file is never fetched: {methods:?}");
    assert!(methods.contains(&"answerCallbackQuery"), "the press is answered: {methods:?}");
    let poll = calls.iter().find(|c| c.method() == "getUpdates").unwrap();
    assert_eq!(poll.field("allowed_updates").as_deref(), Some(ch::ALLOWED_UPDATES));
    assert!(poll.path.contains(&rig.token), "the token is in the path, as Telegram wants it");
}

#[tokio::test]
async fn a_file_over_the_limit_is_refused_by_name_and_never_downloaded() {
    let rig = rig_with(&fresh_token(), FakeBotApi::start(), ", max_file_bytes: 10");
    rig.bot.file("BIG", "documents/big.bin", &[b'x'; 64]);
    rig.bot.push(serde_json::json!({ "update_id": 1, "message": {
        "message_id": 10, "date": 0, "from": { "id": OWNER }, "chat": { "id": CHAT },
        "document": { "file_id": "BIG", "file_name": "big.bin", "file_size": 64 } } }));
    // Telegram not saying the size up front: refused once getFile does.
    rig.bot.push(serde_json::json!({ "update_id": 2, "message": {
        "message_id": 11, "date": 0, "from": { "id": OWNER }, "chat": { "id": CHAT }, "document": { "file_id": "BIG" } } }));
    let batch = rig.poll("gateway", None).await.unwrap();
    for item in &batch.items {
        let ch::Inbound::Message { files, refused, .. } = item else { panic!("{item:?}") };
        assert!(files.is_empty() && refused[0].contains("over the 10-byte limit"), "{refused:?}");
    }
    assert!(!rig.bot.calls().iter().any(|c| c.path.starts_with("/file/")), "nothing was downloaded");
}

#[tokio::test]
async fn a_second_consumer_of_the_token_is_refused_in_this_daemon_and_on_the_host() {
    let token = fresh_token();
    let first = rig_with(&token, FakeBotApi::start(), "");
    first.poll("gateway-a", None).await.unwrap();
    let (code, why) = first.poll("someone-else", None).await.unwrap_err();
    assert_eq!(code, "denied");
    assert!(why.contains("already has its one getUpdates consumer") && why.contains("owner-telegram's inbound gateway"), "{why}");
    first.poll("gateway-a", None).await.expect("the holder keeps polling");

    // Another instance on this host, with the same bot token.
    let second = rig_with(&token, FakeBotApi::start(), "");
    let (code, why) = second.poll("gateway-b", None).await.unwrap_err();
    assert_eq!(code, "denied");
    assert!(why.contains("on this host") && why.contains(&first.root.display().to_string()), "{why}");
    assert!(!why.contains(&token), "{why}");
    assert!(second.bot.calls().is_empty(), "the second consumer never reached Telegram");
}

#[tokio::test]
async fn telegram_saying_another_consumer_polls_stops_the_poll_until_a_restart() {
    let rig = rig();
    rig.bot.bot.conflict.store(true, Ordering::SeqCst);
    let (code, why) = rig.poll("gateway", None).await.unwrap_err();
    assert_eq!(code, "denied");
    assert!(why.contains("409") && why.contains("another consumer") && why.contains("restart"), "{why}");
    assert!(!why.contains(&rig.token), "{why}");
    rig.bot.bot.conflict.store(false, Ordering::SeqCst);
    let (_, again) = rig.poll("gateway", None).await.unwrap_err();
    assert!(again.contains("409"), "{again}");
    let polls = rig.bot.calls().iter().filter(|c| c.method() == "getUpdates").count();
    assert_eq!(polls, 1, "it does not fight the other consumer");
}

#[tokio::test]
async fn the_handled_offset_is_kept_so_a_restart_does_not_take_a_batch_in_twice() {
    let rig = rig();
    rig.bot.push(message(41, 10, OWNER, "eins"));
    rig.bot.push(message(42, 11, OWNER, "zwei"));
    let batch = rig.poll("gateway", None).await.unwrap();
    assert_eq!((batch.items.len(), batch.next_offset), (2, Some(43)));
    rig.poll("gateway", batch.next_offset).await.unwrap();
    // The next consumer -- after a restart -- knows no offset of its own.
    let after = rig.poll("gateway", None).await.unwrap();
    assert!(after.items.is_empty(), "{after:?}");
    let offsets: Vec<Option<String>> =
        rig.bot.calls().iter().filter(|c| c.method() == "getUpdates").map(|c| c.field("offset")).collect();
    assert_eq!(offsets, vec![None, Some("43".into()), Some("43".into())]);
}

#[tokio::test]
async fn only_the_owner_answers_a_message_or_puts_buttons_under_one_and_an_answer_is_never_a_repeat_of_another() {
    let rig = rig();
    let notify = |reply_to: Option<i64>, buttons: Vec<ch::Button>| Request::Notify {
        channel: "owner-telegram".into(),
        text: "ok".into(),
        files: Vec::new(),
        reply_to,
        buttons,
    };
    for to in [10, 11] {
        match rig.ask(notify(Some(to), Vec::new()), None).await {
            Response::Ok { data: Payload::Notified { notified } } => assert_eq!(notified.outcome, factory_core::protocol::NotifyOutcome::Sent),
            other => panic!("{other:?}"),
        }
    }
    let sent = rig.bot.sent();
    assert_eq!(sent.iter().map(Call::replies_to).collect::<Vec<_>>(), vec![Some(10), Some(11)]);
    let buttons = vec![ch::Button { text: "Ja".into(), data: "ok:1".into() }];
    rig.ask(notify(None, buttons.clone()), None).await;
    assert_eq!(rig.bot.sent().last().unwrap().button("Ja").as_deref(), Some("ok:1"));

    // A run, even one approved for this channel, sends its result -- not
    // answers, and not buttons.
    let task = match rig.ask(Request::TaskCreate(NewTask {
        title: "approved".into(),
        scope: Some("demo".into()),
        agent: Some("shell".into()),
        worktree: Some(false),
        deliver: Some("owner-telegram".into()),
        ..Default::default()
    }), None).await {
        Response::Ok { data: Payload::Task { task } } => task,
        other => panic!("{other:?}"),
    };
    rig.engine
        .l4
        .store
        .create_run(&factory_core::run::NewRun {
            task_id: task.id.clone(),
            trigger: factory_core::run::Trigger::Manual,
            agent: "shell".into(),
            adapter: "shell".into(),
            runtime: "quiet".into(),
            token: "tok-run".into(),
            queued_at: None,
            scheduled_for: None,
        })
        .await
        .unwrap();
    for request in [notify(Some(12), Vec::new()), notify(None, buttons)] {
        match rig.ask(request, Some("tok-run")).await {
            Response::Error { message, .. } => assert!(message.contains("owner's inbound gateway"), "{message}"),
            other => panic!("{other:?}"),
        }
    }
}

#[tokio::test]
async fn a_schedule_change_is_journaled_as_whoever_asked_and_restating_it_is_not() {
    let rig = rig();
    let task = match rig.ask(Request::TaskCreate(NewTask {
        title: "Konferenz-Radar".into(),
        scope: Some("demo".into()),
        agent: Some("shell".into()),
        schedule: Some(Schedule::Cron(factory_core::task::CronSchedule { expr: "0 9 * * 1".into(), timezone: Some("Europe/Berlin".into()) })),
        ..Default::default()
    }), None).await {
        Response::Ok { data: Payload::Task { task } } => task,
        other => panic!("{other:?}"),
    };
    let edit = |expr: &str| Request::TaskUpdate {
        id: task.id.clone(),
        patch: factory_core::task::TaskPatch {
            schedule: Some(Schedule::Cron(factory_core::task::CronSchedule { expr: expr.into(), timezone: Some("Europe/Berlin".into()) })),
            ..Default::default()
        },
        reason: Some("moved to Friday".into()),
    };
    rig.ask(edit("0 8 * * 5"), None).await;
    rig.ask(edit("0 8 * * 5"), None).await;
    let changes: Vec<_> = rig.journal(&task.id).await.into_iter().filter(|e| e.kind == "schedule_changed").collect();
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert_eq!(changes[0].source, "owner");
    assert!(
        changes[0].message.starts_with("schedule set to 0 8 * * 5 (Europe/Berlin) by the owner: moved to Friday (was 0 9 * * 1 (Europe/Berlin))"),
        "{}",
        changes[0].message
    );
}

// ------------------------------------------------------------------ gateway

#[tokio::test]
async fn a_message_is_acknowledged_at_once_answered_when_its_run_reports_and_a_stranger_gets_nothing() {
    let mut rig = rig();
    rig.serve();
    rig.bot.push(message(1, 50, STRANGER, "let me in"));
    rig.bot.push(message(2, 51, OWNER, "Wie wird das Wetter morgen?"));

    let ack = wait_for("the acknowledgement", || rig.bot.sent().into_iter().find(|c| c.replies_to() == Some(51))).await;
    assert!(ack.text().contains("I'll answer here"), "{}", ack.text());
    assert_eq!(ack.field("chat_id").as_deref(), Some(CHAT.to_string().as_str()));
    let (task, run_token) = rig.message_task(51).await;
    assert_eq!((task.scope.as_str(), task.agent.as_str(), task.deliver.as_deref()), ("demo", "shell", Some("owner-telegram")));
    assert_eq!(task.instructions, "Wie wird das Wetter morgen?");
    assert!(!task.worktree);

    let ignored = rig.entry(crate::notify::CHANNELS_JOURNAL, "inbound_ignored").await;
    assert_eq!(ignored.data.as_ref().unwrap()["user"], STRANGER);
    assert!(!serde_json::to_string(&ignored).unwrap().contains("let me in"));
    assert_eq!(rig.bot.sent().len(), 1, "only the owner's message was answered");

    // However long the run takes: the answer goes when it reports.
    tokio::time::sleep(Duration::from_millis(500)).await;
    rig.report_done(&task, &run_token, "Morgen wird es sonnig.").await;
    let answer = wait_for("the answer", || rig.bot.sent().into_iter().find(|c| c.text() == "Morgen wird es sonnig.")).await;
    assert_eq!(answer.replies_to(), Some(51));
    let mut answered = None;
    for _ in 0..100 {
        answered = rig.tasks().await.into_iter().find(|t| t.id == task.id && t.labels.contains_key(ch::LABEL_ANSWERED));
        if answered.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let answered = answered.expect("the task marked answered");
    assert_eq!(answered.labels.get(ch::LABEL_UPDATE).map(String::as_str), Some("2"));
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(rig.bot.sent().len(), 2, "answered once");
}

#[tokio::test]
async fn a_document_reaches_the_run_as_a_path_and_a_file_the_run_makes_goes_back_as_a_document() {
    let mut rig = rig();
    rig.bot.file("DOC-9", "documents/file_9.txt", b"Notizen vom Montag");
    rig.serve();
    rig.bot.push(serde_json::json!({ "update_id": 1, "message": {
        "message_id": 60, "date": 1_790_000_000, "from": { "id": OWNER }, "chat": { "id": CHAT }, "caption": "Fass das zusammen",
        "document": { "file_id": "DOC-9", "file_name": "notizen.txt", "mime_type": "text/plain", "file_size": 18 } } }));
    let (task, run_token) = rig.message_task(60).await;
    let path = task
        .instructions
        .lines()
        .find_map(|l| l.strip_prefix("#   ").and_then(|l| l.split(" (").next()))
        .unwrap_or_else(|| panic!("{}", task.instructions))
        .to_string();
    assert!(task.instructions.starts_with("Fass das zusammen\n\n# Files sent with this message"), "{}", task.instructions);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Notizen vom Montag");

    // The run writes a file and sends it through #337's path, under the
    // task's standing approval.
    let out = rig.root.join("projects/demo/out/summary.md");
    std::fs::write(&out, "# Zusammenfassung").unwrap();
    let request = Request::Notify {
        channel: "owner-telegram".into(),
        text: "Die Zusammenfassung".into(),
        files: vec![out.display().to_string()],
        reply_to: None,
        buttons: Vec::new(),
    };
    let identity = rig.engine.l4.store.active_run(&task.id).await.unwrap().unwrap().token.unwrap();
    match rig.ask(request, Some(&identity)).await {
        Response::Ok { data: Payload::Notified { notified } } => assert_eq!(notified.documents, 1),
        other => panic!("{other:?}"),
    }
    let document = rig.bot.calls().into_iter().find(|c| c.method() == "sendDocument").unwrap();
    assert!(String::from_utf8_lossy(&document.body).contains("# Zusammenfassung"));
    rig.report_done(&task, &run_token, "Erledigt, siehe Datei.").await;
    wait_for("the answer", || rig.bot.sent().into_iter().find(|c| c.text() == "Erledigt, siehe Datei.")).await;
}

#[tokio::test]
async fn jobs_run_now_and_a_confirmed_reschedule_go_through_the_one_handler_as_the_owner() {
    let mut rig = rig();
    let radar = match rig.ask(Request::TaskCreate(NewTask {
        title: "Konferenz-Radar".into(),
        scope: Some("demo".into()),
        agent: Some("shell".into()),
        worktree: Some(false),
        schedule: Some(Schedule::Cron(factory_core::task::CronSchedule { expr: "0 9 * * 1".into(), timezone: Some("Europe/Berlin".into()) })),
        ..Default::default()
    }), None).await {
        Response::Ok { data: Payload::Task { task } } => task,
        other => panic!("{other:?}"),
    };
    rig.serve();

    rig.bot.push(message(1, 70, OWNER, "/jobs"));
    let jobs = wait_for("the job list", || rig.bot.sent().into_iter().find(|c| c.replies_to() == Some(70))).await;
    assert!(jobs.text().contains("Scheduled:") && jobs.text().contains("Konferenz-Radar – 0 9 * * 1 (Europe/Berlin), next Mon"), "{}", jobs.text());

    // A reschedule is asked first; the timezone left out is the task's own.
    rig.bot.push(message(2, 71, OWNER, "/schedule Konferenz-Radar 0 8 * * 5"));
    let question = wait_for("the question", || rig.bot.sent().into_iter().find(|c| c.replies_to() == Some(71))).await;
    assert!(question.text().contains("Reschedule “Konferenz-Radar”") && question.text().contains("0 8 * * 5 (Europe/Berlin)"), "{}", question.text());
    let yes = question.button("Yes").expect("a Yes button");
    assert_eq!(rig.engine.l4.store.get(&radar.id).await.unwrap().unwrap().schedule, radar.schedule, "nothing changed yet");
    rig.bot.push(press(3, OWNER, &yes, 1001));
    let done = wait_for("the reschedule", || rig.bot.sent().into_iter().find(|c| c.text().starts_with("Rescheduled:"))).await;
    assert!(done.text().contains("0 8 * * 5 (Europe/Berlin)") && done.text().contains("Fri"), "{}", done.text());
    let task = rig.engine.l4.store.get(&radar.id).await.unwrap().unwrap();
    assert_eq!(
        task.schedule,
        Some(Schedule::Cron(factory_core::task::CronSchedule { expr: "0 8 * * 5".into(), timezone: Some("Europe/Berlin".into()) }))
    );
    let changed = rig.journal(&radar.id).await.into_iter().find(|e| e.kind == "schedule_changed").expect("journaled");
    assert!(changed.message.contains("by the owner: confirmed on Telegram"), "{}", changed.message);
    let channel_kinds: Vec<String> = rig.journal(crate::notify::CHANNELS_JOURNAL).await.into_iter().map(|e| e.kind).collect();
    assert!(channel_kinds.contains(&"inbound_proposed".into()) && channel_kinds.contains(&"inbound_confirmed".into()), "{channel_kinds:?}");
    // The same button again: already answered.
    rig.bot.push(press(4, OWNER, &yes, 1001));
    wait_for("the expiry", || rig.bot.sent().into_iter().find(|c| c.text().contains("expired or was already answered"))).await;

    // A no, in words, leaves it.
    rig.bot.push(message(5, 72, OWNER, "/schedule Konferenz-Radar 0 7 * * 1"));
    wait_for("the second question", || rig.bot.sent().into_iter().find(|c| c.replies_to() == Some(72))).await;
    rig.bot.push(message(6, 73, OWNER, "Nein"));
    wait_for("the decline", || rig.bot.sent().into_iter().find(|c| c.text() == "Left as it is.")).await;
    assert_eq!(rig.engine.l4.store.get(&radar.id).await.unwrap().unwrap().schedule, task.schedule);

    // Run now: the owner said so, so it runs.
    rig.bot.push(message(7, 74, OWNER, "/run konferenz"));
    wait_for("the start", || rig.bot.sent().into_iter().find(|c| c.text().starts_with("Started: Konferenz-Radar"))).await;
    let requested = rig.journal(&radar.id).await.into_iter().find(|e| e.kind == "run_requested").expect("journaled");
    assert_eq!(requested.source, "owner");
    assert!(requested.message.contains("by the owner: asked on Telegram"), "{}", requested.message);
}

#[tokio::test]
async fn a_task_the_assistant_proposes_is_offered_as_a_button_and_runs_only_on_a_yes() {
    let mut rig = rig();
    let radar = match rig.ask(Request::TaskCreate(NewTask {
        title: "Konferenz-Radar".into(),
        scope: Some("demo".into()),
        agent: Some("shell".into()),
        worktree: Some(false),
        ..Default::default()
    }), None).await {
        Response::Ok { data: Payload::Task { task } } => task,
        other => panic!("{other:?}"),
    };
    rig.serve();
    rig.bot.push(message(1, 80, OWNER, "Lass den Konferenz-Radar jetzt laufen"));
    let (task, run_token) = rig.message_task(80).await;
    rig.report_done(&task, &run_token, "Mache ich, sobald du bestätigst.\n/run Konferenz-Radar").await;
    wait_for("the answer, without the command", || rig.bot.sent().into_iter().find(|c| c.text() == "Mache ich, sobald du bestätigst.")).await;
    let question = wait_for("the question", || rig.bot.sent().into_iter().find(|c| c.text().starts_with("Run “Konferenz-Radar”"))).await;
    assert!(!rig.journal(&radar.id).await.iter().any(|e| e.kind == "run_requested"), "nothing runs without a yes");
    rig.bot.push(message(2, 81, OWNER, "ja"));
    wait_for("the start", || rig.bot.sent().into_iter().find(|c| c.text().starts_with("Started: Konferenz-Radar"))).await;
    let requested = rig.journal(&radar.id).await.into_iter().find(|e| e.kind == "run_requested").expect("journaled");
    assert!(requested.message.contains("by the owner: confirmed on Telegram"), "{}", requested.message);
    assert!(question.button("Yes").is_some());
}

#[tokio::test]
async fn intake_from_the_chat_is_a_relayed_chat_item_from_telegram_keyed_on_the_message() {
    let mut rig = rig();
    rig.serve();
    rig.bot.push(message(1, 90, OWNER, "/intake Factory soll den Radar auf Mastodon posten"));
    let said = wait_for("the confirmation", || rig.bot.sent().into_iter().find(|c| c.replies_to() == Some(90))).await;
    assert!(said.text().starts_with("Handed to Factory's intake"), "{}", said.text());
    let item = rig.tasks().await.into_iter().find(|t| t.intake.is_some()).expect("an intake item");
    let record = item.intake.unwrap();
    assert_eq!(record.source.kind, factory_core::intake::SourceKind::Chat);
    assert_eq!(record.source.provider.as_deref(), Some("telegram"));
    assert_eq!(record.source.reference.as_deref(), Some("owner-telegram/90"));
    assert_eq!(record.source.relayed_by.as_deref(), Some("the owner"));
    assert_eq!(record.requester, format!("telegram user {OWNER}"));
}

#[tokio::test]
async fn a_gateway_that_finds_its_token_held_says_so_once_and_starts_no_task() {
    let token = fresh_token();
    let holder = rig_with(&token, FakeBotApi::start(), "");
    holder.poll("someone-else", None).await.unwrap();
    let mut second = rig_with(&token, FakeBotApi::start(), "");
    second.serve();
    let refused = second.entry(crate::notify::CHANNELS_JOURNAL, "inbound_refused").await;
    assert!(refused.message.contains("already has its one getUpdates consumer on this host"), "{}", refused.message);
    assert!(second.bot.calls().is_empty());
}

#[tokio::test]
async fn a_run_that_blocks_is_passed_on_once_and_answered_when_it_ends() {
    let mut rig = rig();
    rig.serve();
    rig.bot.push(message(1, 95, OWNER, "Buch mir einen Tisch"));
    let (task, run_token) = rig.message_task(95).await;
    wait_for("the acknowledgement", || rig.bot.sent().into_iter().find(|c| c.replies_to() == Some(95))).await;
    rig.report(&task, &run_token, RunStatus::Blocked, Some("Welches Restaurant?"), None).await;
    let asked = wait_for("the block passed on", || rig.bot.sent().into_iter().find(|c| c.text().starts_with("Waiting for you: Welches Restaurant?"))).await;
    assert_eq!(asked.replies_to(), Some(95));
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(rig.bot.sent().iter().filter(|c| c.text().starts_with("Waiting for you")).count(), 1, "said once");
    rig.report(&task, &run_token, RunStatus::Running, Some("weiter"), None).await;
    rig.report_done(&task, &run_token, "Tisch für 19 Uhr reserviert.").await;
    wait_for("the answer", || rig.bot.sent().into_iter().find(|c| c.text() == "Tisch für 19 Uhr reserviert.")).await;
}

// ------------------------------------------------------------- escalation

impl Rig {
    /// A task of the rig's own in `demo` -- not one a message became --
    /// running, and its run's token.
    async fn running_task(&self, title: &str) -> (Task, String) {
        let task = self
            .engine
            .l4_service()
            .create(NewTask {
                title: title.into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                worktree: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        let run = Request::TaskRun { id: task.id.clone(), reason: None, continue_run: false, override_wait: false };
        assert!(matches!(self.ask(run, None).await, Response::Ok { .. }));
        for _ in 0..300 {
            if let Some(run) = self.engine.l4.store.active_run(&task.id).await.unwrap() {
                if let Some(token) = run.token.clone() {
                    return (task, token);
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("{title} never started");
    }

    /// The message ids the question of `task` went out as.
    async fn question_ids(&self, task: &Task) -> Vec<i64> {
        let sent = self.entry(&task.id, crate::escalation::ESCALATION_SENT_KIND).await;
        sent.data.unwrap()["message_ids"].as_array().unwrap().iter().map(|id| id.as_i64().unwrap()).collect()
    }
}

#[tokio::test]
async fn a_blocked_runs_question_goes_to_the_owner_and_a_reply_answers_it_once() {
    let mut rig = rig_escalating();
    rig.serve();
    let (task, run_token) = rig.running_task("Deploy").await;
    rig.report(&task, &run_token, RunStatus::Blocked, Some("Which region?"), None).await;

    let asked = wait_for("the question", || rig.bot.sent().into_iter().find(|c| c.text().contains("Which region?"))).await;
    assert!(asked.text().contains("Deploy (demo) is waiting for your answer"), "{}", asked.text());
    assert!(asked.text().contains("Reply to this message"), "{}", asked.text());
    let ids = rig.question_ids(&task).await;

    rig.bot.push(reply(1, 500, ids[0], "eu-central"));
    let done = wait_for("the confirmation", || rig.bot.sent().into_iter().find(|c| c.replies_to() == Some(500))).await;
    assert!(done.text().contains("typed into attempt 1"), "{}", done.text());
    let answered = rig.entry(&task.id, "answer").await;
    assert_eq!(answered.data.as_ref().unwrap()["reason"], "replied on owner-telegram");
    assert!(!answered.message.contains("eu-central"), "the owner's text is not journaled");

    rig.bot.push(reply(2, 501, ids[0], "us-east"));
    let again = wait_for("the second reply's answer", || rig.bot.sent().into_iter().find(|c| c.replies_to() == Some(501))).await;
    assert!(again.text().contains("answered already"), "{}", again.text());
    assert!(rig.tasks().await.iter().all(|t| !t.labels.contains_key(ch::LABEL_MESSAGE)), "neither reply became a task");
}

#[tokio::test]
async fn a_questions_button_answers_it_with_that_option() {
    let mut rig = rig_escalating();
    rig.serve();
    let (task, run_token) = rig.running_task("PR 2097").await;
    rig.report(&task, &run_token, RunStatus::Blocked, Some("merge as-is or suppress?"), None).await;
    let first = wait_for("the question", || rig.bot.sent().into_iter().find(|c| c.text().contains("merge as-is or suppress?"))).await;
    assert!(first.button("Merge as-is").is_none(), "an agent's own question has no options");

    // With options -- what an escalation from the orchestrator carries.
    let run = rig.engine.l4.store.active_run(&task.id).await.unwrap().unwrap();
    let options = vec!["Merge as-is".to_string(), "Suppress, then merge".to_string()];
    rig.engine
        .l4_service()
        .escalate_to_owner(&run, "merge while CodeScene is red?", Some("Merging outside the task's rule is yours to decide."), &options)
        .await
        .unwrap();
    let asked = wait_for("the options", || rig.bot.sent().into_iter().find(|c| c.button("Suppress, then merge").is_some())).await;
    let data = asked.button("Merge as-is").unwrap();
    assert_eq!(data, format!("ans:{}:0", run.id));

    rig.bot.push(press(3, OWNER, &data, 900));
    let done = wait_for("the confirmation", || rig.bot.sent().into_iter().find(|c| c.text().starts_with("✓"))).await;
    assert!(done.text().contains("typed into attempt 1"), "{}", done.text());
    assert_eq!(rig.entry(&task.id, "answer").await.data.unwrap()["reason"], "chose on owner-telegram");
}

#[tokio::test]
async fn a_chat_tasks_question_goes_into_its_own_thread_once() {
    let mut rig = rig_escalating();
    rig.serve();
    rig.bot.push(message(1, 95, OWNER, "Buch mir einen Tisch"));
    let (task, run_token) = rig.message_task(95).await;
    wait_for("the acknowledgement", || rig.bot.sent().into_iter().find(|c| c.replies_to() == Some(95))).await;
    rig.report(&task, &run_token, RunStatus::Blocked, Some("Welches Restaurant?"), None).await;
    let asked = wait_for("the question", || rig.bot.sent().into_iter().find(|c| c.text().contains("Welches Restaurant?"))).await;
    assert_eq!(asked.replies_to(), Some(95), "in the conversation it came from");
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(rig.bot.sent().iter().filter(|c| c.text().contains("Welches Restaurant?")).count(), 1, "said once, by the daemon");
}

#[test]
fn escalating_to_a_channel_that_takes_nothing_in_is_refused_at_load() {
    use factory_core::config::{Config, DaemonConfig, EscalationConfig};
    let config = Config {
        daemon: DaemonConfig { escalation: EscalationConfig { channel: Some("nowhere".into()) }, ..DaemonConfig::default() },
        ..Default::default()
    };
    let err = config.validate().unwrap_err().to_string();
    assert!(err.contains("daemon.escalation.channel"), "{err}");
}
