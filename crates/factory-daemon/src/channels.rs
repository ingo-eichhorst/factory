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
//!
//! Inbound (`#338`): `poll` is the one `getUpdates` there is for a bot
//! token. Its first caller holds the token -- in this daemon, and on the
//! host, by an `flock` on a file named by the token's digest -- and any other
//! consumer is refused with who holds it. When Telegram itself says another
//! consumer polls the token (`409`), the poll stops asking until the daemon
//! restarts, rather than fight it. The allowlist is applied here: a message
//! from anyone else is handed up with no content, and its files are never
//! fetched. Files from an allowed user are downloaded with a size cap into
//! the inbound directory, owner-only.

use crate::l2_service::L2Service;
use chrono::{DateTime, Utc};
use factory_core::error::{FactoryError, Result};
use factory_core::openshell::ProviderCredential;
use factory_environment::channel_ledger::{ChannelLedger, SentRecord};
use factory_environment::channels::{self as ch, Allowlist, ChannelDecl, Field, Inbound, InboundBatch, Outgoing, Receipt, Sorted};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

/// How long one Bot API call may take: a 50 MB upload on a slow line.
const CALL_TIMEOUT: Duration = Duration::from_secs(300);
/// How long a consumer's hold on a bot token outlives its last poll before
/// another may take it over: longer than any one poll waits.
const HOLD_STALE: Duration = Duration::from_secs(120);

/// L2's channel state: the ledger, one send at a time, and who polls which
/// bot token.
pub(crate) struct ChannelState {
    pub(crate) ledger: ChannelLedger,
    /// The repeat check, the send and its record are one step: two identical
    /// messages at once go once.
    pub(crate) sending: tokio::sync::Mutex<()>,
    /// The one consumer of each bot token (`#338`), by the token's digest.
    consumers: std::sync::Mutex<HashMap<String, Consumer>>,
}

impl ChannelState {
    pub(crate) fn new(ledger: ChannelLedger) -> Self {
        Self { ledger, sending: tokio::sync::Mutex::new(()), consumers: std::sync::Mutex::new(HashMap::new()) }
    }
}

/// Who holds a bot token's `getUpdates`, and since when.
struct Consumer {
    id: String,
    channel: String,
    since: DateTime<Utc>,
    last: Instant,
    /// A poll of it is waiting on Telegram now.
    polling: Arc<AtomicBool>,
    /// What Telegram said when it refused a poll with `409`: another
    /// consumer, or a webhook. Nothing more is asked of the token until the
    /// daemon restarts.
    conflict: Option<String>,
    /// The host-wide hold, released when the daemon exits.
    _lock: std::fs::File,
}

/// Clears `Consumer::polling` however the poll ends.
struct Polling(Arc<AtomicBool>);

impl Drop for Polling {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
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
        let keyboard = match outgoing.buttons.as_slice() {
            [] => None,
            buttons => Some(ch::inline_keyboard(buttons).map_err(refused)?),
        };
        let file_hashes: Vec<String> = outgoing.files.iter().map(|f| ch::sha256_hex(&f.bytes)).collect();
        let digest = outgoing.digest(&file_hashes);

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
            let mut fields = vec![Field::Text("chat_id", chat.clone()), Field::FromFile("text", text)];
            if let (0, Some(id)) = (i, outgoing.reply_to) {
                fields.push(Field::Text("reply_parameters", ch::reply_parameters(id)));
            }
            if let (true, Some(keyboard)) = (i + 1 == parts.len(), &keyboard) {
                fields.push(Field::Text("reply_markup", keyboard.clone()));
            }
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

    /// `Request::ChannelPoll` (`#338`): one `getUpdates` for `channel`'s bot
    /// token on behalf of `consumer`, which must be the token's one
    /// consumer. `offset` is the first update the caller has not handled;
    /// everything before it is confirmed to Telegram and remembered, so a
    /// restart does not take it in again. What comes back is sorted against
    /// the allowlist and the channel's chat, with an allowed message's files
    /// already saved.
    pub(crate) async fn poll(&self, name: &str, consumer: &str, offset: Option<i64>, wait: Option<u64>) -> Result<InboundBatch> {
        let factory = self.wiring.snapshot();
        let channel = ch::find(&factory.config.channels, name)
            .ok_or_else(|| {
                refused(format!(
                    "no channel {name:?} is declared in the instance root's channels: (it declares: {})",
                    ch::names(&factory.config.channels)
                ))
            })?
            .clone();
        let inbound = channel
            .inbound
            .clone()
            .ok_or_else(|| refused(format!("the channel {} takes no messages in: it has no inbound: block", channel.name)))?;
        if consumer.trim().is_empty() {
            return Err(refused("a poll names its consumer"));
        }
        let wait = wait.unwrap_or(ch::POLL_WAIT_SECONDS).min(ch::POLL_WAIT_MAX_SECONDS);
        let catalogue = &factory.config.secrets;
        let token = read(&channel, &channel.token, "token", catalogue).await?;
        let chat_value = read(&channel, &channel.chat, "chat", catalogue).await?;
        let chat: i64 = chat_value.trim().parse().map_err(|_| {
            refused(format!(
                "the channel {}'s chat is not a numeric chat id; taking messages in needs the chat's own id, not a @name",
                channel.name
            ))
        })?;
        let users = match &inbound.users {
            Allowlist::Ids(ids) => ids.clone(),
            Allowlist::From(credential) => {
                let value = read(&channel, credential, "inbound users", catalogue).await?;
                ch::parse_user_ids(&value).map_err(|why| {
                    FactoryError::adapter(format!("channel {}", channel.name), format!("its inbound users could not be read: {why}"))
                })?
            }
        };
        let secrets = [token.as_str(), chat_value.trim()];
        let digest = ch::sha256_hex(token.as_bytes());
        let _polling = self.hold(&digest, &channel.name, consumer, &factory.root)?;

        let ledger = &self.state.channels.ledger;
        let offset = match (offset, ledger.offset(&channel.name).await?) {
            (Some(given), Some(kept)) => Some(given.max(kept)),
            (given, kept) => given.or(kept),
        };
        if let Some(offset) = offset {
            ledger.handled_before(&channel.name, offset).await?;
        }
        let mut fields = vec![
            Field::Text("timeout", wait.to_string()),
            Field::Text("allowed_updates", ch::ALLOWED_UPDATES.to_string()),
        ];
        if let Some(offset) = offset {
            fields.push(Field::Text("offset", offset.to_string()));
        }
        let api = channel.api_base().to_string();
        let failed = |why: String| FactoryError::adapter(format!("channel {}", channel.name), ch::redact(&why, &secrets));
        let result = match call_value(&api, &token, "getUpdates", &fields, Duration::from_secs(wait + 15)).await {
            Ok(result) => result,
            Err(Failed::Refused(refusal)) if refusal.code == 409 => {
                let said = ch::redact(&refusal.said, &secrets);
                if let Some(held) = self.state.channels.consumers.lock().unwrap().get_mut(&digest) {
                    held.conflict = Some(said.clone());
                }
                return Err(FactoryError::Denied(conflict_message(&channel.name, &said)));
            }
            Err(Failed::Refused(refusal)) => return Err(failed(refusal.to_string())),
            Err(Failed::Unreachable(why)) => return Err(failed(why)),
        };
        let sorted = ch::sort_updates(&result, chat, &users, inbound.file_limit()).map_err(failed)?;
        let next_offset = sorted
            .iter()
            .map(|s| match s {
                Sorted::Message { update, .. } | Sorted::Button { update, .. } | Sorted::Ignored { update, .. } => *update,
            })
            .max()
            .map(|last| last + 1)
            .or(offset);

        let dir = match &inbound.files {
            Some(dir) => crate::provision::expand_home(dir),
            None => factory.factory_dir().join("channels").join(&channel.name).join("inbox"),
        };
        let mut items = Vec::new();
        for item in sorted {
            items.push(match item {
                Sorted::Message { update, message, from, at, text, files } => {
                    let (mut kept, mut refusals) = (Vec::new(), Vec::new());
                    for (i, file) in files.into_iter().enumerate() {
                        match fetch(&api, &token, &file, &dir, update, i, inbound.file_limit()).await {
                            Ok(saved) => kept.push(saved),
                            Err(why) => refusals.push(format!("{} ({}): {}", file.name, file.kind, ch::redact(&why, &secrets))),
                        }
                    }
                    Inbound::Message { update, message, from, at, text, files: kept, refused: refusals }
                }
                Sorted::Button { update, query, message, from, data } => {
                    // The press is answered at once, so the button stops
                    // spinning; what it asked for is the caller's to do.
                    let fields = [Field::Text("callback_query_id", query)];
                    if let Err(Failed::Refused(ch::Refusal { said, .. }) | Failed::Unreachable(said)) =
                        call_value(&api, &token, "answerCallbackQuery", &fields, Duration::from_secs(30)).await
                    {
                        tracing::warn!(channel = %channel.name, "could not answer a button press: {}", ch::redact(&said, &secrets));
                    }
                    Inbound::Button { update, message, from, data }
                }
                Sorted::Ignored { update, from, chat, what } => Inbound::Ignored { update, from, chat, what },
            });
        }
        Ok(InboundBatch { next_offset, items })
    }

    /// Make `consumer` the one consumer of the bot token whose digest is
    /// `digest`, or say who already is. A hold another consumer has not used
    /// for `HOLD_STALE` is taken over: whatever held it has stopped.
    fn hold(&self, digest: &str, channel: &str, consumer: &str, root: &Path) -> Result<Polling> {
        let mut consumers = self.state.channels.consumers.lock().unwrap();
        if let Some(held) = consumers.get_mut(digest) {
            if let Some(said) = &held.conflict {
                return Err(FactoryError::Denied(conflict_message(channel, said)));
            }
            if held.id != consumer && held.last.elapsed() < HOLD_STALE {
                return Err(FactoryError::Denied(format!(
                    "the bot token of channel {channel} already has its one getUpdates consumer: {}'s inbound gateway, polling \
                     since {}; Telegram allows one consumer per bot token, so this one is refused",
                    held.channel,
                    held.since.to_rfc3339()
                )));
            }
            if held.polling.swap(true, Ordering::SeqCst) {
                return Err(FactoryError::Denied(format!(
                    "a poll of channel {channel}'s bot token is already waiting on Telegram; one consumer polls one at a time"
                )));
            }
            if held.id != consumer {
                held.id = consumer.to_string();
                held.channel = channel.to_string();
                held.since = Utc::now();
            }
            held.last = Instant::now();
            return Ok(Polling(held.polling.clone()));
        }
        let lock = host_hold(digest, channel, root).map_err(FactoryError::Denied)?;
        let polling = Arc::new(AtomicBool::new(true));
        consumers.insert(
            digest.to_string(),
            Consumer {
                id: consumer.to_string(),
                channel: channel.to_string(),
                since: Utc::now(),
                last: Instant::now(),
                polling: polling.clone(),
                conflict: None,
                _lock: lock,
            },
        );
        Ok(Polling(polling))
    }
}

fn conflict_message(channel: &str, said: &str) -> String {
    format!(
        "Telegram refused channel {channel}'s getUpdates (409: {said}): another consumer is polling this bot token, or a \
         webhook is set. A bot token has exactly one consumer, so this one stopped rather than fight it, and asks Telegram \
         nothing more until the daemon restarts. Stop the other consumer (Factory never removes a webhook), then restart."
    )
}

/// Where this user's host-wide holds live: one private directory in `/tmp`,
/// the same for every Factory instance this user runs, whatever its
/// environment says `TMPDIR` is.
fn holds_dir() -> std::result::Result<PathBuf, String> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    let dir = PathBuf::from(format!("/tmp/factory-telegram-{uid}"));
    match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("could not make {}: {e}", dir.display())),
    }
    let meta = std::fs::symlink_metadata(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
        return Err(format!("{} is not a private directory of this user, so no hold on a bot token is taken there", dir.display()));
    }
    Ok(dir)
}

/// The host-wide hold on a bot token: an exclusive `flock` on a file named
/// by the token's digest (never the token), saying who holds it. Another
/// daemon on this host -- another instance with the same token -- is
/// refused with that line. Released when this process exits.
fn host_hold(digest: &str, channel: &str, root: &Path) -> std::result::Result<std::fs::File, String> {
    use std::io::{Read, Seek, Write};
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;
    let path = holds_dir()?.join(format!("{}.lock", &digest[..32]));
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|e| format!("could not open {}: {e}", path.display()))?;
    // SAFETY: the descriptor is open for as long as `file` lives.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let mut holder = String::new();
        let _ = file.read_to_string(&mut holder);
        let holder = holder.trim();
        return Err(format!(
            "the bot token of channel {channel} already has its one getUpdates consumer on this host ({}); Telegram allows \
             one consumer per bot token, so this one is refused",
            if holder.is_empty() { "another process" } else { holder }
        ));
    }
    let line = format!(
        "daemon pid {}, instance {}, channel {channel}, since {}\n",
        std::process::id(),
        root.display(),
        Utc::now().to_rfc3339()
    );
    let _ = file.set_len(0).and_then(|()| file.rewind()).and_then(|()| file.write_all(line.as_bytes()));
    Ok(file)
}

/// Download one file a message brought into `dir/<date>/`, owner-only,
/// refusing it past `limit` bytes -- by what Telegram says it is, by what
/// `curl` is allowed to write, and by what was written.
async fn fetch(
    api: &str,
    token: &str,
    file: &ch::FileRef,
    dir: &Path,
    update: i64,
    index: usize,
    limit: u64,
) -> std::result::Result<ch::ReceivedFile, String> {
    use std::os::unix::fs::DirBuilderExt;
    let too_big = |bytes: u64| format!("{bytes} bytes is over the {limit}-byte limit for a file taken in");
    if let Some(size) = file.size.filter(|size| *size > limit) {
        return Err(too_big(size));
    }
    let fields = [Field::Text("file_id", file.file_id.clone())];
    let result = call_value(api, token, "getFile", &fields, Duration::from_secs(30)).await.map_err(|e| match e {
        Failed::Refused(refusal) => refusal.to_string(),
        Failed::Unreachable(why) => why,
    })?;
    let (location, size) = ch::file_location(&result)?;
    if let Some(size) = size.filter(|size| *size > limit) {
        return Err(too_big(size));
    }
    let day = dir.join(Utc::now().format("%Y-%m-%d").to_string());
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&day)
        .map_err(|e| format!("could not make {}: {e}", day.display()))?;
    let path = day.join(format!("{update}-{index}-{}", file.name));
    let config = ch::download_config(api, token, &location, &path.display().to_string(), limit)?;
    let outcome = run_curl(&config, CALL_TIMEOUT).await;
    let written = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let refuse = |why: String| {
        let _ = std::fs::remove_file(&path);
        Err(why)
    };
    match outcome {
        Err(why) => return refuse(format!("the download failed: {why}")),
        Ok((code, _)) if code != 200 => return refuse(format!("the download answered HTTP {code}")),
        Ok(_) if written > limit => return refuse(too_big(written)),
        Ok(_) => {}
    }
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(ch::ReceivedFile { path: path.display().to_string(), kind: file.kind.to_string(), bytes: written, mime: file.mime.clone() })
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
    let (code, body) = run_curl(&config, CALL_TIMEOUT).await?;
    ch::read_answer(code, &body)
}

/// Why a call that wants an answer's `result` did not get one.
enum Failed {
    /// Telegram answered, and said no.
    Refused(ch::Refusal),
    /// Nothing usable came back.
    Unreachable(String),
}

/// One Bot API call whose answer is its `result`, whatever shape that is.
async fn call_value(api: &str, token: &str, method: &str, fields: &[Field], timeout: Duration) -> std::result::Result<serde_json::Value, Failed> {
    let config = ch::curl_config(api, token, method, fields).map_err(Failed::Unreachable)?;
    let (code, body) = run_curl(&config, timeout).await.map_err(Failed::Unreachable)?;
    ch::answer_result(code, &body).map_err(Failed::Refused)
}

/// `curl` with `config` on its stdin; the HTTP status and the body.
async fn run_curl(config: &str, timeout: Duration) -> std::result::Result<(u16, String), String> {
    let curl = curl().ok_or("curl is not on the daemon's PATH")?;
    let mut child = tokio::process::Command::new(&curl)
        .args([
            "--silent",
            "--show-error",
            "--proto",
            "=https,http",
            "--max-time",
            &timeout.as_secs().to_string(),
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
    let output = match tokio::time::timeout(timeout + Duration::from_secs(10), child.wait_with_output()).await {
        Err(_) => return Err(format!("the Bot API did not answer within {}s", timeout.as_secs())),
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
    Ok((code, body.to_string()))
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
