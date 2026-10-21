//! L2's outbound channels (`#337`): one send through a declared channel, to
//! its one fixed recipient. What L4 hands down (`Commands<L3, L2Port>`) is
//! already approved -- the run's task names this channel, or the owner
//! asked -- so this decides only whether the same message already went, and
//! then sends it.
//!
//! The bot token and the chat are read from their sources here, the moment
//! a send needs them, and handed to `curl` on its stdin (`--config -`): never
//! on a command line, never in a file, a log or an answer. Whatever `curl`
//! or Telegram says on failure is passed through `channels::redact` first.
//! The text and the files are written to a private staging directory for
//! `curl` to read, and removed when the send is over.
//!
//! The ledger (`ChannelLedger`) is written only once everything went: a send
//! that failed halfway is not a repeat, and the next attempt sends it again.

use crate::l2_service::L2Service;
use chrono::Utc;
use factory_core::error::{FactoryError, Result};
use factory_core::openshell::ProviderCredential;
use factory_environment::channel_ledger::{ChannelLedger, SentRecord};
use factory_environment::channels::{self as ch, ChannelDecl, Field, Outgoing, Receipt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// How long one Bot API call may take: a 50 MB upload on a slow line.
const CALL_TIMEOUT: Duration = Duration::from_secs(300);

/// L2's channel state: the ledger, and one send at a time.
pub(crate) struct ChannelState {
    pub(crate) ledger: ChannelLedger,
    /// The repeat check, the send and its record are one step: two identical
    /// messages at once go once.
    pub(crate) sending: tokio::sync::Mutex<()>,
}

impl ChannelState {
    pub(crate) fn new(ledger: ChannelLedger) -> Self {
        Self { ledger, sending: tokio::sync::Mutex::new(()) }
    }
}

impl crate::engine::Engine {
    /// Install the instance database's channel ledger in place of the
    /// in-memory one `Engine::new` starts with.
    pub fn with_channel_ledger(mut self, ledger: ChannelLedger) -> Self {
        self.l2.channels = ChannelState::new(ledger);
        self
    }
}

fn refused(message: impl Into<String>) -> FactoryError {
    FactoryError::BadRequest(message.into())
}

impl L2Service<'_> {
    /// Send `outgoing` through its channel, or say it already went.
    pub(crate) async fn deliver(&self, outgoing: Outgoing) -> Result<Receipt> {
        let factory = self.wiring.snapshot();
        let channel = ch::find(&factory.config.channels, &outgoing.channel)
            .ok_or_else(|| {
                refused(format!(
                    "no channel {:?} is declared in the instance root's channels: (it declares: {})",
                    outgoing.channel,
                    ch::names(&factory.config.channels)
                ))
            })?
            .clone();
        let parts = ch::split_text(&outgoing.text).map_err(refused)?;
        if outgoing.files.len() > ch::MAX_FILES {
            return Err(refused(format!("at most {} files go in one send", ch::MAX_FILES)));
        }
        if let Some(big) = outgoing.files.iter().find(|f| f.bytes.len() as u64 > ch::DOCUMENT_LIMIT) {
            return Err(refused(format!(
                "{} is {} bytes; Telegram's Bot API takes a document of at most 50 MB",
                ch::upload_name(&big.name),
                big.bytes.len()
            )));
        }
        let file_hashes: Vec<String> = outgoing.files.iter().map(|f| ch::sha256_hex(&f.bytes)).collect();
        let digest = ch::message_digest(&channel.name, &outgoing.text, &file_hashes);

        let _one = self.state.channels.sending.lock().await;
        let since = Utc::now() - chrono::Duration::hours(ch::REPEAT_WINDOW_HOURS);
        if let Some(previous) = self.state.channels.ledger.last_since(&channel.name, &digest, since).await? {
            return Ok(Receipt::Repeat { id: previous.id, sent_at: previous.sent_at });
        }

        let token = read(&channel, &channel.token, "token", &factory.config.secrets).await?;
        let chat = read(&channel, &channel.chat, "chat", &factory.config.secrets).await?;
        let secrets = [token.as_str(), chat.as_str()];
        let failed = |sent: &str, why: String| {
            FactoryError::adapter(
                format!("channel {}", channel.name),
                format!("{}{sent}", ch::redact(&why, &secrets)),
            )
        };
        let staging = Staging::new().map_err(|e| failed("", e))?;
        let api = channel.api_base();
        let (mut messages, mut documents) = (Vec::new(), Vec::new());
        let so_far = |messages: usize, documents: usize| match (messages, documents) {
            (0, 0) => "; nothing was sent".to_string(),
            (m, d) => format!(
                "; {m} message(s) and {d} file(s) went before it, and nothing was recorded, so sending it again repeats them"
            ),
        };
        for (i, part) in parts.iter().enumerate() {
            let text = staging.write(&format!("text-{i}"), part.as_bytes()).map_err(|e| failed("", e))?;
            let fields = [Field::Text("chat_id", chat.clone()), Field::FromFile("text", text)];
            let id = call(api, &token, "sendMessage", &fields)
                .await
                .map_err(|e| failed(&so_far(messages.len(), documents.len()), e))?;
            messages.push(id);
        }
        for (i, file) in outgoing.files.iter().enumerate() {
            let path = staging.write(&format!("file-{i}"), &file.bytes).map_err(|e| failed("", e))?;
            let fields = [
                Field::Text("chat_id", chat.clone()),
                Field::Upload("document", path, ch::upload_name(&file.name)),
            ];
            let id = call(api, &token, "sendDocument", &fields)
                .await
                .map_err(|e| failed(&so_far(messages.len(), documents.len()), e))?;
            documents.push(id);
        }
        drop(staging);

        let record = SentRecord {
            id: uuid::Uuid::new_v4().to_string(),
            channel: channel.name.clone(),
            digest,
            run: outgoing.run.clone(),
            task: outgoing.task.clone(),
            sent_at: Utc::now(),
            messages: messages.len(),
            documents: documents.len(),
            bytes: outgoing.text.len() as u64 + outgoing.files.iter().map(|f| f.bytes.len() as u64).sum::<u64>(),
        };
        // It went. A ledger that will not write costs the repeat check for
        // this one message, never the answer that it was sent.
        if let Err(e) = self.state.channels.ledger.record(&record).await {
            tracing::warn!(channel = %record.channel, "could not record a channel send: {e}");
        }
        Ok(Receipt::Sent { id: record.id, messages, documents })
    }
}

/// A channel's token or chat, from its source. Value-free on failure.
async fn read(
    channel: &ChannelDecl,
    credential: &ProviderCredential,
    what: &str,
    catalogue: &[factory_core::secrets::SecretDecl],
) -> Result<String> {
    let source = credential.source(catalogue).ok_or_else(|| {
        refused(format!(
            "the channel {}'s {what} names a secret the instance root's secrets: does not declare",
            channel.name
        ))
    })?;
    crate::provision::resolve(source)
        .await
        .map(|secret| secret.expose().to_string())
        .map_err(|why| {
            FactoryError::adapter(
                format!("channel {}", channel.name),
                format!("its {what} could not be read from {}: {why}", source.describe()),
            )
        })
}

/// `curl`, on the daemon's PATH or where launchd's PATH would not look.
fn curl() -> Option<PathBuf> {
    std::env::split_paths(&crate::provision::augmented_path())
        .map(|dir| dir.join("curl"))
        .find(|path| path.is_file())
}

/// One Bot API call: the config on stdin, the answer read off stdout.
async fn call(api: &str, token: &str, method: &str, fields: &[Field]) -> std::result::Result<i64, String> {
    let config = ch::curl_config(api, token, method, fields)?;
    let curl = curl().ok_or("curl is not on the daemon's PATH")?;
    let mut child = tokio::process::Command::new(&curl)
        .args([
            "--silent",
            "--show-error",
            "--proto",
            "=https,http",
            "--max-time",
            &CALL_TIMEOUT.as_secs().to_string(),
            "--write-out",
            "\n%{http_code}",
            "--config",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("could not start curl: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("curl has no stdin")?;
    stdin.write_all(config.as_bytes()).await.map_err(|e| format!("could not hand curl its request: {e}"))?;
    drop(stdin);
    let output = match tokio::time::timeout(CALL_TIMEOUT + Duration::from_secs(10), child.wait_with_output()).await {
        Err(_) => return Err(format!("the Bot API did not answer within {}s", CALL_TIMEOUT.as_secs())),
        Ok(Err(e)) => return Err(format!("curl failed: {e}")),
        Ok(Ok(output)) => output,
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let (body, code) = stdout.rsplit_once('\n').unwrap_or(("", stdout.as_ref()));
    let code: u16 = code.trim().parse().unwrap_or(0);
    if !output.status.success() || code == 0 {
        let said = String::from_utf8_lossy(&output.stderr);
        let said = said.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("no answer");
        return Err(format!("the Bot API could not be reached: {said}"));
    }
    ch::read_answer(code, body)
}

/// A private directory for one send's text and files, gone when dropped.
struct Staging(PathBuf);

impl Staging {
    fn new() -> std::result::Result<Self, String> {
        use std::os::unix::fs::DirBuilderExt;
        // `curl`'s form syntax reads some characters in a path; a temp
        // directory that has one is passed over for /tmp.
        let plain = |p: &Path| p.to_str().is_some_and(|s| s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_')));
        let base = Some(std::env::temp_dir()).filter(|p| plain(p)).unwrap_or_else(|| PathBuf::from("/tmp"));
        let dir = base.join(format!("factory-notify-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(|e| format!("could not make a staging directory: {e}"))?;
        Ok(Self(dir))
    }

    /// Write `bytes` owner-only and return the path, as text.
    fn write(&self, name: &str, bytes: &[u8]) -> std::result::Result<String, String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let path = self.0.join(name);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| format!("could not stage {name}: {e}"))?;
        file.write_all(bytes).map_err(|e| format!("could not stage {name}: {e}"))?;
        Ok(path.display().to_string())
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
