//! `factory notify` (`#337`): a run hands the owner its result through a
//! declared channel. The one handler every path calls -- the CLI over the
//! socket, `POST /api/rpc`, anything else that sends `Request::Notify`.
//!
//! The gate is here, at the run's own level: a run may send only through
//! the channel its task's `deliver` names -- the owner's standing approval,
//! which only the owner can give (`access.rs`) -- and only from the run its
//! token names. Anything else is refused, and the refusal is journaled on
//! the run. The owner, with no token, may send through any declared channel.
//!
//! What to send is handed down as a command (`Commands<L4, L3Port>`, which L3
//! passes to L2, the channel's owner); the receipt is ids only. Every send,
//! skip, refusal and failure is journaled with the channel, the sizes and the
//! SHA-256 hashes -- never the text, never a file, never the recipient.

use crate::access::Caller;
use crate::l4_service::L4Service;
use factory_agents::dispatch::Channels;
use factory_core::error::{FactoryError, Result};
use factory_core::protocol::{Notified, NotifyOutcome};
use factory_core::run::Run;
use crate::operations::Asked;
use factory_core::task::{Task, TaskEntry, TaskPatch};
use factory_environment::channels::{self as ch, Outgoing, OutgoingFile, Receipt};
use std::io::Read;
use std::path::{Path, PathBuf};

/// The journal an owner's own sends are recorded in: they belong to no run,
/// so the channels keep their own line of entries under this fixed id, the
/// way the secrets catalogue does (`secrets::SECRETS_JOURNAL`).
pub(crate) const CHANNELS_JOURNAL: &str = "factory:channels";

fn refused(message: impl Into<String>) -> FactoryError {
    FactoryError::BadRequest(message.into())
}

/// Who a send is on behalf of, and where it is journaled.
enum Sender {
    Owner,
    Run { run: Run, task: Task },
}

impl Sender {
    fn journal(&self) -> &str {
        match self {
            Self::Owner => CHANNELS_JOURNAL,
            Self::Run { task, .. } => &task.id,
        }
    }

    fn entry(&self, kind: &str, message: String, data: serde_json::Value) -> TaskEntry {
        match self {
            Self::Owner => TaskEntry::new("owner", kind, message).with_data(data),
            Self::Run { run, .. } => TaskEntry::new("agent", kind, message).in_run(&run.id).with_data(data),
        }
    }
}

/// A file read for sending: what goes, and what the journal says of it.
struct ReadFile {
    file: OutgoingFile,
    sha256: String,
}

/// Open `path` as a send may: a regular file, not through a symlink, no
/// larger than Telegram takes, read whole.
fn read_file(path: &Path) -> Result<ReadFile> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A FIFO must not hang the daemon waiting for a writer.
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let shown = path.display();
    let file = options.open(path).map_err(|e| refused(format!("reading {shown}: {e}")))?;
    let meta = file.metadata().map_err(|e| refused(format!("reading {shown}: {e}")))?;
    if !meta.is_file() {
        return Err(refused(format!("{shown} is not a regular file")));
    }
    if meta.len() > ch::DOCUMENT_LIMIT {
        return Err(refused(format!(
            "{shown} is {} bytes; Telegram's Bot API takes a document of at most 50 MB",
            meta.len()
        )));
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(ch::DOCUMENT_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| refused(format!("reading {shown}: {e}")))?;
    if bytes.len() as u64 > ch::DOCUMENT_LIMIT {
        return Err(refused(format!("{shown} grew past 50 MB while it was read")));
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(ReadFile { sha256: ch::sha256_hex(&bytes), file: OutgoingFile { name, bytes } })
}

/// Where a run's files may come from: its working directory, and nothing
/// under `.git` or `.factory` inside it. The owner's may come from anywhere.
fn file_path(dir: Option<&Path>, asked: &str) -> Result<PathBuf> {
    let given = Path::new(asked);
    let Some(dir) = dir else {
        if !given.is_absolute() {
            return Err(refused(format!("{asked:?} is not an absolute path")));
        }
        return std::fs::canonicalize(given).map_err(|e| refused(format!("{asked:?}: {e}")));
    };
    let path = std::fs::canonicalize(dir.join(given)).map_err(|e| refused(format!("{asked:?}: {e}")))?;
    let inside = path
        .strip_prefix(dir)
        .map_err(|_| refused(format!("{asked:?} is not inside this run's working directory; a run sends only its own files")))?;
    if inside.components().any(|c| c.as_os_str() == ".git" || c.as_os_str() == ".factory") {
        return Err(refused("Git internals and Factory state are not sent"));
    }
    Ok(path)
}

/// A change to a task's standing approval: what it was, and what it will be.
pub(crate) type DeliveryChange = (Option<String>, Option<String>);

impl L4Service<'_> {
    /// What a `task.update` does to the standing approval (`Task::deliver`),
    /// decided before the edit is written: the channel it names must be
    /// declared, and saying what is already so changes nothing and journals
    /// nothing. Who may send this at all is `access.rs`'s -- the owner alone.
    pub(crate) async fn delivery_change(&self, id: &str, patch: &mut TaskPatch) -> Result<Option<DeliveryChange>> {
        if patch.deliver.is_none() && !patch.clear_deliver {
            return Ok(None);
        }
        let current = self.require(id).await?;
        if let Some(channel) = &patch.deliver {
            let declared: Vec<String> = self.wiring.snapshot().config.channels.iter().map(|c| c.name.clone()).collect();
            factory_process::creation::check_channel(&declared, channel)?;
            // A new approval replaces the old one; it is not also withdrawn.
            patch.clear_deliver = false;
        }
        if patch.deliver.is_some() && patch.deliver == current.deliver {
            patch.deliver = None;
        }
        if patch.clear_deliver && current.deliver.is_none() {
            patch.clear_deliver = false;
        }
        Ok((patch.deliver.is_some() || patch.clear_deliver).then(|| (current.deliver, patch.deliver.clone())))
    }

    /// Journal a change `delivery_change` let through, once the edit landed
    /// -- what the store kept, as for a pause: an out-of-process store that
    /// predates `deliver` would drop it, and a journal saying "approved"
    /// over a task that cannot send would be a lie.
    pub(crate) async fn record_delivery_change(&self, task: &Task, (before, after): DeliveryChange, asked: &Asked) -> Result<()> {
        if task.deliver != after {
            return Err(FactoryError::adapter(
                "task store",
                "the task store did not keep the delivery approval; it may predate channels, so the approval is unchanged",
            ));
        }
        let by = asked.words();
        let (kind, message) = match (&before, &after) {
            (_, Some(channel)) => (
                "delivery_approved",
                format!("delivering this task's result to the channel {channel} approved {by}; no other external effect"),
            ),
            (Some(channel), None) => (
                "delivery_withdrawn",
                format!("the approval to deliver this task's result to the channel {channel} withdrawn {by}"),
            ),
            (None, None) => return Ok(()),
        };
        let data = serde_json::json!({ "channel": after.as_ref().or(before.as_ref()), "before": before });
        self.entry(&task.id, asked.entry(kind, message, data)).await;
        Ok(())
    }

    /// `Request::Notify`: check the approval, read the files, hand the send
    /// down, and journal what became of it.
    pub(crate) async fn notify(
        &self,
        caller: &Caller,
        channel: &str,
        text: String,
        files: Vec<String>,
    ) -> Result<Notified> {
        let snapshot = self.wiring.snapshot();
        let sender = match caller {
            Caller::Owner => Sender::Owner,
            Caller::Agent { run_id: Some(run_id), .. } => {
                let run = self.require_run(run_id).await?;
                let task = self.require(&run.task_id).await?;
                Sender::Run { run, task }
            }
            Caller::Agent { .. } => {
                let why = "only a run whose task has a standing delivery approval, or the owner, sends through a channel";
                self.entry(
                    CHANNELS_JOURNAL,
                    TaskEntry::new("agent", "notify_refused", format!("{} was refused a send to {channel}: {why}", caller.describe()))
                        .with_data(serde_json::json!({ "channel": channel, "by": caller.describe() })),
                )
                .await;
                return Err(FactoryError::Denied(format!("{} may not send to {channel}: {why}", caller.describe())));
            }
        };
        let approval = match &sender {
            Sender::Owner => "owner",
            Sender::Run { run, task } if task.deliver.as_deref() == Some(channel) && !run.status.is_terminal() => "task",
            Sender::Run { task, .. } => {
                let why = match &task.deliver {
                    Some(other) => format!("its standing approval is for the channel {other}, not {channel}"),
                    None => "it has no standing approval to deliver its result through a channel".to_string(),
                };
                let message = format!("refused a send to {channel}: {why}");
                let data = serde_json::json!({ "channel": channel, "approved": task.deliver });
                self.entry(sender.journal(), sender.entry("notify_refused", message, data)).await;
                return Err(FactoryError::Denied(format!(
                    "task {} may not send to {channel}: {why}. The owner gives one with `factory task edit {} --deliver {channel}`",
                    task.id, task.id
                )));
            }
        };

        if ch::find(&snapshot.config.channels, channel).is_none() {
            let why = format!(
                "no channel {channel:?} is declared in the instance root's channels: (it declares: {})",
                ch::names(&snapshot.config.channels)
            );
            let data = serde_json::json!({ "channel": channel });
            self.entry(sender.journal(), sender.entry("notify_refused", format!("refused a send to {channel}: {why}"), data)).await;
            return Err(refused(why));
        }
        ch::split_text(&text).map_err(refused)?;

        let dir = match &sender {
            Sender::Owner => None,
            Sender::Run { run, task } => Some(
                std::fs::canonicalize(match &run.worktree_path {
                    Some(path) => PathBuf::from(path),
                    None => snapshot.scope_path(&task.scope)?,
                })
                .map_err(|e| refused(format!("this run's working directory: {e}")))?,
            ),
        };
        if files.len() > ch::MAX_FILES {
            return Err(refused(format!("at most {} files go in one send", ch::MAX_FILES)));
        }
        let mut read = Vec::new();
        for asked in &files {
            read.push(read_file(&file_path(dir.as_deref(), asked)?)?);
        }

        let data = serde_json::json!({
            "channel": channel,
            "approval": approval,
            "text": { "chars": text.chars().count(), "bytes": text.len(), "sha256": ch::sha256_hex(text.as_bytes()) },
            "files": read.iter().map(|r| serde_json::json!({ "bytes": r.file.bytes.len(), "sha256": r.sha256 })).collect::<Vec<_>>(),
        });
        let bytes: usize = text.len() + read.iter().map(|r| r.file.bytes.len()).sum::<usize>();
        let (run, task) = match &sender {
            Sender::Owner => (None, None),
            Sender::Run { run, task } => (Some(run.id.clone()), Some(task.id.clone())),
        };
        let outgoing = Outgoing {
            channel: channel.to_string(),
            text,
            files: read.into_iter().map(|r| r.file).collect(),
            run,
            task,
        };

        let with = |mut data: serde_json::Value, extra: serde_json::Value| {
            if let (Some(map), Some(extra)) = (data.as_object_mut(), extra.as_object()) {
                map.extend(extra.clone());
            }
            data
        };
        match self.wiring.l3().port().deliver(outgoing).await {
            Ok(Receipt::Sent { id, messages, documents }) => {
                let message = format!(
                    "sent to {channel}: {} message(s), {} file(s), {bytes} bytes",
                    messages.len(),
                    documents.len()
                );
                let extra = serde_json::json!({ "delivery": id, "messages": messages.len(), "documents": documents.len() });
                self.entry(sender.journal(), sender.entry("notify_sent", message, with(data, extra))).await;
                Ok(Notified {
                    channel: channel.to_string(),
                    outcome: NotifyOutcome::Sent,
                    delivery: id,
                    messages: messages.len(),
                    documents: documents.len(),
                    sent_at: None,
                })
            }
            Ok(Receipt::Repeat { id, sent_at }) => {
                let message = format!(
                    "not sent to {channel}: the same message went at {}, within {}h",
                    sent_at.to_rfc3339(),
                    ch::REPEAT_WINDOW_HOURS
                );
                let extra = serde_json::json!({ "repeats": id, "sent_at": sent_at });
                self.entry(sender.journal(), sender.entry("notify_skipped", message, with(data, extra))).await;
                Ok(Notified {
                    channel: channel.to_string(),
                    outcome: NotifyOutcome::Skipped,
                    delivery: id,
                    messages: 0,
                    documents: 0,
                    sent_at: Some(sent_at),
                })
            }
            Err(e) => {
                let message = format!("the send to {channel} failed: {e}");
                self.entry(sender.journal(), sender.entry("notify_failed", message, data)).await;
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use factory_core::protocol::{Envelope, NotifyOutcome, Payload, Request, Response};
    use factory_core::run::{NewRun, Run, Trigger};
    use factory_core::task::{NewTask, Task, TaskPatch};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    const TOKEN: &str = "123456:TEST-not-a-real-token";
    const CHAT: &str = "4242424242";

    /// One request the fake Bot API saw.
    #[derive(Clone, Debug)]
    struct Call {
        path: String,
        body: Vec<u8>,
    }

    impl Call {
        fn method(&self) -> &str {
            self.path.rsplit('/').next().unwrap_or_default()
        }
        fn has(&self, needle: &[u8]) -> bool {
            self.body.windows(needle.len()).any(|w| w == needle)
        }
    }

    /// A local stand-in for `api.telegram.org`: records every call, and
    /// answers each with a fresh message id -- or, for the method named in
    /// `refuse`, with Telegram's own shape of a refusal.
    struct FakeBotApi {
        port: u16,
        calls: Arc<Mutex<Vec<Call>>>,
        refuse: Arc<Mutex<Option<&'static str>>>,
    }

    impl FakeBotApi {
        fn start() -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let calls = Arc::new(Mutex::new(Vec::new()));
            let refuse = Arc::new(Mutex::new(None));
            let (seen, refusing) = (calls.clone(), refuse.clone());
            std::thread::spawn(move || {
                for (n, stream) in listener.incoming().enumerate() {
                    let Ok(stream) = stream else { continue };
                    let _ = answer(stream, n as i64 + 1, &seen, &refusing);
                }
            });
            Self { port, calls, refuse }
        }

        fn api(&self) -> String {
            format!("http://127.0.0.1:{}", self.port)
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }
    }

    fn answer(
        stream: std::net::TcpStream,
        id: i64,
        seen: &Mutex<Vec<Call>>,
        refusing: &Mutex<Option<&'static str>>,
    ) -> std::io::Result<()> {
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
        let call = Call { path, body };
        let refused = refusing.lock().unwrap().is_some_and(|m| m == call.method());
        seen.lock().unwrap().push(call);
        let (status, json) = if refused {
            ("400 Bad Request", r#"{"ok":false,"error_code":400,"description":"Bad Request: chat not found"}"#.to_string())
        } else {
            ("200 OK", format!(r#"{{"ok":true,"result":{{"message_id":{id},"chat":{{"id":1}}}}}}"#))
        };
        write!(
            writer,
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
            json.len()
        )?;
        writer.flush()
    }

    struct Rig {
        engine: Arc<Engine>,
        root: PathBuf,
        bot: FakeBotApi,
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    impl Rig {
        fn scope_dir(&self) -> PathBuf {
            self.root.join("projects/demo")
        }

        async fn send(&self, token: Option<&str>, channel: &str, text: &str, files: Vec<String>) -> Response {
            self.engine
                .handle(Envelope {
                    request: Request::Notify { channel: channel.into(), text: text.into(), files },
                    token: token.map(str::to_string),
                })
                .await
        }

        async fn task(&self, deliver: Option<&str>) -> Task {
            let new = NewTask {
                title: "Konferenz-Radar".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                worktree: Some(false),
                deliver: deliver.map(str::to_string),
                ..Default::default()
            };
            match self.engine.handle(Envelope { request: Request::TaskCreate(new), token: None }).await {
                Response::Ok { data: Payload::Task { task }, .. } => task,
                other => panic!("task.create: {other:?}"),
            }
        }

        async fn run(&self, task: &Task, token: &str) -> Run {
            self.engine
                .l4
                .store
                .create_run(&NewRun {
                    task_id: task.id.clone(),
                    trigger: Trigger::Manual,
                    agent: "shell".into(),
                    adapter: "shell".into(),
                    runtime: "herdr".into(),
                    token: token.into(),
                    queued_at: None,
                    scheduled_for: None,
                })
                .await
                .unwrap()
        }

        async fn journal(&self, id: &str) -> Vec<factory_core::task::TaskEntry> {
            self.engine.l4.store.entries(id, 100).await.unwrap()
        }
    }

    fn private_file(path: &Path, text: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, text).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn rig() -> Rig {
        use factory_core::config::{Config, DaemonConfig, Instance};
        use factory_plugins::{Registry, SqliteStore};
        let bot = FakeBotApi::start();
        let root = std::env::temp_dir().join(format!("factory-notify-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".factory")).unwrap();
        std::fs::create_dir_all(root.join("projects/demo/out")).unwrap();
        let token_file = root.join(".factory/bot-token");
        private_file(&token_file, &format!("{TOKEN}\n"));
        let yaml = format!(
            "secrets:\n\
             - {{ name: telegram-bot-token, kind: token, source: {{ from: file, path: {} }}, expires: never }}\n\
             channels:\n\
             - {{ name: owner-telegram, kind: telegram, token: {{ secret: telegram-bot-token }}, chat: {{ from: command, run: 'printf {CHAT}' }}, api: '{}' }}\n\
             - {{ name: elsewhere, kind: telegram, token: {{ secret: telegram-bot-token }}, chat: {{ from: command, run: 'printf 1' }}, api: '{}' }}\n",
            token_file.display(),
            bot.api(),
            bot.api(),
        );
        #[derive(serde::Deserialize)]
        struct Blocks {
            secrets: Vec<factory_core::secrets::SecretDecl>,
            channels: Vec<factory_core::channels::ChannelDecl>,
        }
        let blocks: Blocks = serde_yaml_ng::from_str(&yaml).unwrap();
        let demo: factory_core::config::Scope = serde_yaml_ng::from_str("id: demo-id\nname: demo\npath: projects/demo\n").unwrap();
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig { power_assertion: false, ..DaemonConfig::default() },
            scope: None,
            scopes: vec![demo],
            roles: Default::default(),
            dashboard: None,
            policies: Default::default(),
            quality: Default::default(),
            infrastructure: Default::default(),
            secrets: blocks.secrets,
            channels: blocks.channels,
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        config.validate().unwrap();
        let store: Arc<dyn factory_core::adapter::TaskStore> =
            Arc::new(SqliteStore::open(&root.join(".factory/factory.sqlite")).unwrap());
        let factory = factory_core::config::Factory { root: root.clone(), config };
        let engine = Arc::new(Engine::new(factory, Registry::with_builtins(), store, PathBuf::from("factory"), Vec::new()));
        Rig { engine, root, bot }
    }

    fn notified(response: Response) -> factory_core::protocol::Notified {
        match response {
            Response::Ok { data: Payload::Notified { notified }, .. } => notified,
            other => panic!("expected a notified payload, got {other:?}"),
        }
    }

    fn refusal(response: Response) -> String {
        match response {
            Response::Error { message, .. } => message,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_run_with_the_standing_approval_sends_its_text_and_file_and_the_journal_keeps_only_sizes_and_hashes() {
        let rig = rig();
        let task = rig.task(Some("owner-telegram")).await;
        assert!(rig.journal(&task.id).await.iter().any(|e| e.kind == "delivery_approved"), "approval journaled at creation");
        let run = rig.run(&task, "tok-run").await;

        // A self-contained HTML report the size of the radar's.
        let report = rig.scope_dir().join("out/ingo-conference-radar.html");
        let mut html = b"<!doctype html><title>Radar</title><p>MARKER-START".to_vec();
        html.extend(std::iter::repeat_n(b'x', 1_700_000));
        html.extend(b"MARKER-END</p>");
        std::fs::write(&report, &html).unwrap();
        let text = "Konferenz-Radar 26.10.: 2 neue CFPs, Top 5 \u{1F680}";

        let sent = notified(rig.send(Some("tok-run"), "owner-telegram", text, vec![report.display().to_string()]).await);
        assert_eq!((sent.outcome, sent.messages, sent.documents), (NotifyOutcome::Sent, 1, 1));

        let calls = rig.bot.calls();
        assert_eq!(calls.iter().map(Call::method).collect::<Vec<_>>(), vec!["sendMessage", "sendDocument"]);
        assert!(calls.iter().all(|c| c.path.starts_with(&format!("/bot{TOKEN}/"))), "the token is in the path, as Telegram wants it");
        assert!(calls.iter().all(|c| c.has(CHAT.as_bytes())), "the configured chat, on every call");
        assert!(calls[0].has(text.as_bytes()));
        assert!(calls[1].has(b"filename=\"ingo-conference-radar.html\""));
        assert!(calls[1].has(&html), "the file arrives byte for byte");

        let entries = rig.journal(&task.id).await;
        let entry = entries.iter().find(|e| e.kind == "notify_sent").expect("the send is journaled");
        assert_eq!(entry.run_id.as_deref(), Some(run.id.as_str()));
        let data = entry.data.as_ref().unwrap();
        assert_eq!(data["channel"], "owner-telegram");
        assert_eq!(data["approval"], "task");
        assert_eq!(data["text"]["sha256"], factory_core::channels::sha256_hex(text.as_bytes()));
        assert_eq!(data["files"][0]["bytes"], html.len());
        assert_eq!(data["files"][0]["sha256"], factory_core::channels::sha256_hex(&html));
        let everything = serde_json::to_string(&entries).unwrap();
        for secret in [text, TOKEN, CHAT, "MARKER-START", "ingo-conference-radar"] {
            assert!(!everything.contains(secret), "the journal must not hold {secret:?}: {everything}");
        }

        // The same message again, within 12 hours: not sent, and said so.
        let again = notified(rig.send(Some("tok-run"), "owner-telegram", text, vec![report.display().to_string()]).await);
        assert_eq!((again.outcome, again.delivery), (NotifyOutcome::Skipped, sent.delivery.clone()));
        assert_eq!(rig.bot.calls().len(), 2, "nothing more reached the Bot API");
        assert!(rig.journal(&task.id).await.iter().any(|e| e.kind == "notify_skipped"));

        // A different message is not a repeat.
        let other = notified(rig.send(Some("tok-run"), "owner-telegram", "something else", Vec::new()).await);
        assert_eq!((other.outcome, other.messages, other.documents), (NotifyOutcome::Sent, 1, 0));
    }

    #[tokio::test]
    async fn a_run_without_the_approval_for_that_channel_is_refused_and_the_refusal_journaled() {
        let rig = rig();
        let unapproved = rig.task(None).await;
        let run = rig.run(&unapproved, "tok-a").await;
        let message = refusal(rig.send(Some("tok-a"), "owner-telegram", "hello", Vec::new()).await);
        assert!(message.contains("no standing approval"), "{message}");
        let entries = rig.journal(&unapproved.id).await;
        let refused = entries.iter().find(|e| e.kind == "notify_refused").expect("refusal journaled");
        assert_eq!(refused.run_id.as_deref(), Some(run.id.as_str()));

        // Approved for one channel is not approved for another.
        let approved = rig.task(Some("owner-telegram")).await;
        rig.run(&approved, "tok-b").await;
        let message = refusal(rig.send(Some("tok-b"), "elsewhere", "hello", Vec::new()).await);
        assert!(message.contains("not elsewhere"), "{message}");
        assert!(rig.bot.calls().is_empty(), "nothing reached the Bot API");

        // A token nobody knows is no owner.
        let message = refusal(rig.send(Some("tok-unknown"), "owner-telegram", "hello", Vec::new()).await);
        assert!(message.contains("no agent"), "{message}");
    }

    #[tokio::test]
    async fn only_the_owner_gives_moves_or_withdraws_the_standing_approval() {
        let rig = rig();
        let task = rig.task(None).await;
        rig.run(&task, "tok-w").await;
        let edit = |deliver: Option<&str>, clear: bool| Request::TaskUpdate {
            id: task.id.clone(),
            patch: TaskPatch { deliver: deliver.map(str::to_string), clear_deliver: clear, ..Default::default() },
            reason: Some("weekly radar".into()),
        };

        // The run's own agent, a worker with task.edit on its own task.
        let as_run = |request| rig.engine.handle(Envelope { request, token: Some("tok-w".into()) });
        let message = refusal(as_run(edit(Some("owner-telegram"), false)).await);
        assert!(message.contains("owner"), "{message}");
        let new = NewTask { title: "self-approved".into(), scope: Some("demo".into()), deliver: Some("owner-telegram".into()), ..Default::default() };
        let message = refusal(as_run(Request::TaskCreate(new)).await);
        assert!(message.contains("owner"), "{message}");

        let as_owner = |request| rig.engine.handle(Envelope { request, token: None });
        let message = refusal(as_owner(edit(Some("nowhere"), false)).await);
        assert!(message.contains("does not declare") && message.contains("owner-telegram"), "{message}");
        let Response::Ok { data: Payload::Task { task: approved }, .. } = as_owner(edit(Some("owner-telegram"), false)).await else {
            panic!("the owner may approve")
        };
        assert_eq!(approved.deliver.as_deref(), Some("owner-telegram"));
        let Response::Ok { data: Payload::Task { task: withdrawn }, .. } = as_owner(edit(None, true)).await else {
            panic!("the owner may withdraw")
        };
        assert_eq!(withdrawn.deliver, None);
        let kinds: Vec<_> = rig.journal(&task.id).await.into_iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&"delivery_approved".to_string()) && kinds.contains(&"delivery_withdrawn".to_string()), "{kinds:?}");
    }

    #[tokio::test]
    async fn a_long_text_goes_as_several_messages_and_files_stay_inside_the_runs_working_directory() {
        let rig = rig();
        let task = rig.task(Some("owner-telegram")).await;
        rig.run(&task, "tok-l").await;
        let long = format!("{}\n", "z".repeat(99)).repeat(50);
        let sent = notified(rig.send(Some("tok-l"), "owner-telegram", &long, Vec::new()).await);
        assert_eq!(sent.messages, 2);

        let outside = rig.root.join("outside.txt");
        std::fs::write(&outside, "not the run's").unwrap();
        let message = refusal(rig.send(Some("tok-l"), "owner-telegram", "with a file", vec![outside.display().to_string()]).await);
        assert!(message.contains("not inside this run's working directory"), "{message}");

        // Over Telegram's 50 MB: refused before anything is read or sent.
        let big = rig.scope_dir().join("out/big.bin");
        std::fs::File::create(&big).unwrap().set_len(factory_core::channels::DOCUMENT_LIMIT + 1).unwrap();
        let message = refusal(rig.send(Some("tok-l"), "owner-telegram", "big", vec![big.display().to_string()]).await);
        assert!(message.contains("at most 50 MB"), "{message}");
        assert_eq!(rig.bot.calls().len(), 2, "only the long text went");
    }

    #[tokio::test]
    async fn a_refusal_from_telegram_is_reported_without_the_token_and_the_next_attempt_is_not_a_repeat() {
        let rig = rig();
        *rig.bot.refuse.lock().unwrap() = Some("sendMessage");
        let task = rig.task(Some("owner-telegram")).await;
        rig.run(&task, "tok-f").await;
        let message = refusal(rig.send(Some("tok-f"), "owner-telegram", "hello", Vec::new()).await);
        assert!(message.contains("chat not found") && message.contains("nothing was sent"), "{message}");
        assert!(!message.contains(TOKEN) && !message.contains(CHAT), "{message}");
        assert!(rig.journal(&task.id).await.iter().any(|e| e.kind == "notify_failed"));

        *rig.bot.refuse.lock().unwrap() = None;
        let sent = notified(rig.send(Some("tok-f"), "owner-telegram", "hello", Vec::new()).await);
        assert_eq!(sent.outcome, NotifyOutcome::Sent, "a failed send is not recorded as sent");
    }

    #[tokio::test]
    async fn the_owner_sends_through_any_channel_and_it_is_journaled_on_the_channels_own_line() {
        let rig = rig();
        let note = rig.root.join("note.txt");
        std::fs::write(&note, "from the owner").unwrap();
        let sent = notified(rig.send(None, "elsewhere", "a test message", vec![note.display().to_string()]).await);
        assert_eq!((sent.outcome, sent.documents), (NotifyOutcome::Sent, 1));
        let entries = rig.journal(super::CHANNELS_JOURNAL).await;
        let entry = entries.iter().find(|e| e.kind == "notify_sent").expect("journaled");
        assert_eq!(entry.data.as_ref().unwrap()["approval"], "owner");
        let message = refusal(rig.send(None, "nowhere", "x", Vec::new()).await);
        assert!(message.contains("owner-telegram, elsewhere"), "{message}");
        let message = refusal(rig.send(None, "elsewhere", "x", vec!["relative.txt".into()]).await);
        assert!(message.contains("not an absolute path"), "{message}");
    }
}
