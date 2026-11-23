//! Outbound channels (`#337`): the instance root's `channels:` block, and
//! what one send through a channel is. A channel is somewhere Factory can
//! hand the owner a message -- today a Telegram bot writing to one chat.
//!
//! ```yaml
//! channels:
//!   - name: owner-telegram
//!     kind: telegram
//!     token: { secret: telegram-bot-token }
//!     chat: { from: env, name: FACTORY_OWNER_TELEGRAM_CHAT }
//! ```
//!
//! The bot token is always an entry of the secrets catalogue (`#244`), by
//! reference: it never appears in config, a task, the journal or a log. The
//! chat is fixed here too -- a catalogue entry or an inline source -- and
//! never comes from whoever sends, so there is no way to name another chat.
//!
//! A channel may also take messages in (`#338`): its `inbound:` block names
//! who may write to it -- Telegram user ids, from the config or a source --
//! and which scope and agent a message becomes a task for. One bot token has
//! exactly one `getUpdates` consumer, so the block is the only poller there
//! is: two channels taking messages in through the same token are refused at
//! load, and the daemon refuses a second consumer while one holds the token.
//!
//! ```yaml
//!     inbound:
//!       users: { secret: telegram-owner-ids }   # or [123456789]
//!       scope: assistant
//!       agent: assistant
//! ```
//!
//! Everything in this module is pure: the declaration and its checks, how a
//! text is cut into Telegram messages, the `curl` config a call is made
//! with, and how the Bot API's answers -- a send's, `getUpdates`',
//! `getFile`'s -- are read. Reading the token, running `curl`, the ledgers
//! and the poll itself are the daemon's L2 code.

use crate::openshell::ProviderCredential;
use crate::secrets::SecretDecl;
use chrono::{DateTime, Utc};
use factory_kernel::{CommandPort, FactoryError, Result, L2};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// Telegram's own Bot API.
pub const TELEGRAM_API: &str = "https://api.telegram.org";
/// The most one `sendMessage` carries, counted the way Telegram counts it
/// (UTF-16 code units, which is at most the number of characters).
pub const TEXT_LIMIT: usize = 4096;
/// A longer text is cut into at most this many messages; past that it is
/// refused, with the advice to send it as a file instead.
pub const MAX_PARTS: usize = 10;
/// The most one `sendDocument` uploads to Telegram's Bot API.
pub const DOCUMENT_LIMIT: u64 = 50 * 1024 * 1024;
/// How many files one send may carry.
pub const MAX_FILES: usize = 5;
/// An identical message to the same channel within this many hours is not
/// sent again, as `scripts/notify-ingo` has it: a scheduled task that keeps
/// failing the same way messages once, not every run.
pub const REPEAT_WINDOW_HOURS: i64 = 12;
/// The most one file a message brings in may be, unless `inbound:` says
/// less: what Telegram's own Bot API lets `getFile` download.
pub const INBOUND_FILE_LIMIT: u64 = 20 * 1024 * 1024;
/// How long one `getUpdates` waits for something to arrive, by default and
/// at most. Telegram holds the request open that long when nothing comes.
pub const POLL_WAIT_SECONDS: u64 = 25;
pub const POLL_WAIT_MAX_SECONDS: u64 = 50;
/// The labels on a task a message became (`#338`): the channel it came in
/// on, the message and the update it was, and -- once its answer went --
/// the run that answered it.
pub const LABEL_CHANNEL: &str = "inbound.channel";
pub const LABEL_MESSAGE: &str = "inbound.message";
pub const LABEL_UPDATE: &str = "inbound.update";
pub const LABEL_ANSWERED: &str = "inbound.answered";
/// What a poll asks Telegram for: messages and button presses, nothing else.
pub const ALLOWED_UPDATES: &str = r#"["message","callback_query"]"#;

/// What kind of channel an entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChannelKind {
    Telegram,
}

impl ChannelKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Telegram => "telegram",
        }
    }
}

/// One `channels:` entry. Strict: a misspelt key is refused by name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelDecl {
    /// Unique among channels. What `factory notify <name>` and a task's
    /// `--deliver <name>` name.
    pub name: String,
    pub kind: ChannelKind,
    /// The bot token: `{ secret: <name> }`, an entry of `secrets:`. Never
    /// written inline.
    pub token: ProviderCredential,
    /// The one chat this channel writes to: `{ secret: <name> }` or a
    /// source written inline (`{ from: env, name: ... }`).
    pub chat: ProviderCredential,
    /// The Bot API's base URL. Absent is Telegram's own; a self-hosted
    /// `telegram-bot-api` server (or a test's fake one) otherwise. `https`,
    /// or `http` to a loopback address only: the token travels in the path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Taking messages in (`#338`). Absent, the channel is send-only and
    /// nothing ever polls its token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbound: Option<InboundDecl>,
}

impl ChannelDecl {
    /// The Bot API base, without a trailing slash.
    pub fn api_base(&self) -> &str {
        self.api.as_deref().unwrap_or(TELEGRAM_API).trim_end_matches('/')
    }
}

/// Who may write to a channel: Telegram user ids, written here
/// (`users: [123456789]`) or read from a source like any credential
/// (`users: { secret: telegram-owner-ids }`), whose value is the ids
/// separated by commas or spaces. Never from a message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Allowlist {
    Ids(Vec<i64>),
    From(ProviderCredential),
}

/// The ids an allowlist source holds: `123, 456 789`. Anything that is not
/// a positive number is refused, so a mistyped file allows nobody rather
/// than somebody.
pub fn parse_user_ids(value: &str) -> std::result::Result<Vec<i64>, String> {
    let ids = value
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .map(|part| part.parse::<i64>().ok().filter(|id| *id > 0).ok_or_else(|| format!("{part:?} is not a Telegram user id")))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if ids.is_empty() {
        return Err("it names no Telegram user id".into());
    }
    Ok(ids)
}

/// The language the gateway's own lines -- the acknowledgement, the job
/// list, the confirmation question -- are written in. The assistant answers
/// in whatever language it is asked in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Language {
    #[default]
    En,
    De,
}

/// A channel's `inbound:` block (`#338`). Strict, like the channel itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboundDecl {
    /// The Telegram users whose messages are taken. Anyone else's are
    /// ignored and journaled, never answered.
    pub users: Allowlist,
    /// The scope and agent a message becomes a task for.
    pub scope: String,
    pub agent: String,
    /// Where files that come with a message are kept: an absolute path or
    /// one under `~/`. Absent, `.factory/channels/<name>/inbox` in the
    /// instance root -- never inside a scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<String>,
    /// The largest file taken in, in bytes. At most `DOCUMENT_LIMIT`;
    /// absent, `INBOUND_FILE_LIMIT` (what Telegram's own Bot API lets a bot
    /// download).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_file_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<Language>,
}

impl InboundDecl {
    pub fn file_limit(&self) -> u64 {
        self.max_file_bytes.unwrap_or(INBOUND_FILE_LIMIT)
    }
}

fn is_channel_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name.len() <= 63
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Why `api` cannot be used, if it cannot: `https://host[:port][/path]`, or
/// `http://` to `127.0.0.1`, `localhost` or `[::1]`. No credentials, query
/// or fragment, so the only secret in the URL is the one Factory puts there.
fn api_problem(api: &str) -> Option<String> {
    let (scheme, rest) = api.split_once("://")?;
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() {
        return Some(format!("{api:?} names no host"));
    }
    if rest.contains(['?', '#', '@', ' ']) || api.chars().any(char::is_control) {
        return Some(format!("{api:?} carries credentials, a query or a fragment; give only scheme, host, port and path"));
    }
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => authority.split(':').next().unwrap_or_default(),
    };
    match scheme {
        "https" => None,
        "http" if matches!(host, "127.0.0.1" | "localhost" | "::1") => None,
        "http" => Some(format!(
            "{api:?} is plain http to a host that is not this machine; the bot token travels in the URL, so use https"
        )),
        _ => Some(format!("{api:?} is not an http(s) URL")),
    }
}

/// The whole block, checked at load against the secrets catalogue: unique
/// names, a token that is a declared secret, a chat that can be read, and
/// an API base that does not leak the token.
pub fn validate(channels: &[ChannelDecl], secrets: &[SecretDecl]) -> Result<()> {
    let mut seen = BTreeSet::new();
    let declared = || {
        let names: Vec<&str> = secrets.iter().map(|s| s.name.as_str()).collect();
        if names.is_empty() {
            "nothing".to_string()
        } else {
            names.join(", ")
        }
    };
    for channel in channels {
        let refuse = |what: String| Err(FactoryError::BadRequest(format!("channels: {what}")));
        if !is_channel_name(&channel.name) {
            return refuse(format!(
                "{:?} is not a channel name (letters, digits, '-', '_' and '.', at most 63)",
                channel.name
            ));
        }
        if !seen.insert(channel.name.as_str()) {
            return refuse(format!("declares {:?} twice", channel.name));
        }
        match &channel.token {
            ProviderCredential::Secret(name) if crate::secrets::find(secrets, name).is_some() => {}
            ProviderCredential::Secret(name) => {
                return refuse(format!(
                    "{:?} names the token {{ secret: {name} }}, which the instance root's secrets: does not declare (it declares: {})",
                    channel.name,
                    declared()
                ))
            }
            ProviderCredential::Source(_) => {
                return refuse(format!(
                    "{:?} writes its token inline; a bot token is {{ secret: <name> }}, an entry of the instance root's secrets:",
                    channel.name
                ))
            }
        }
        match &channel.chat {
            ProviderCredential::Secret(name) if crate::secrets::find(secrets, name).is_none() => {
                return refuse(format!(
                    "{:?} names the chat {{ secret: {name} }}, which the instance root's secrets: does not declare (it declares: {})",
                    channel.name,
                    declared()
                ))
            }
            ProviderCredential::Secret(_) => {}
            ProviderCredential::Source(source) => {
                if let Some(problem) = source.problems(&format!("the channel {:?}'s chat", channel.name)).into_iter().next() {
                    return refuse(problem);
                }
            }
        }
        if let Some(api) = &channel.api {
            if let Some(problem) = api_problem(api).or_else(|| (!api.contains("://")).then(|| format!("{api:?} is not a URL"))) {
                return refuse(format!("{:?}'s api: {problem}", channel.name));
            }
        }
        if channel.note.as_deref().is_some_and(|n| n.contains(['\n', '\r'])) {
            return refuse(format!("{:?}'s note is one line", channel.name));
        }
        if let Some(inbound) = &channel.inbound {
            validate_inbound(channel, inbound, secrets).map_err(|what| FactoryError::BadRequest(format!("channels: {what}")))?;
        }
    }
    // One bot token, one `getUpdates` consumer: Telegram cuts off whichever
    // poll came first when a second one starts, so two inbound channels on
    // one token would each lose messages to the other.
    let mut polled: Vec<(&str, &str)> = Vec::new();
    for channel in channels.iter().filter(|c| c.inbound.is_some()) {
        let Some(token) = channel.token.secret() else { continue };
        if let Some((other, _)) = polled.iter().find(|(_, t)| *t == token) {
            return Err(FactoryError::BadRequest(format!(
                "channels: {other:?} and {:?} both take messages in through the bot token {{ secret: {token} }}; \
                 a bot token has exactly one getUpdates consumer, so only one channel on it may have inbound:",
                channel.name
            )));
        }
        polled.push((channel.name.as_str(), token));
    }
    Ok(())
}

fn validate_inbound(channel: &ChannelDecl, inbound: &InboundDecl, secrets: &[SecretDecl]) -> std::result::Result<(), String> {
    let name = &channel.name;
    match &inbound.users {
        Allowlist::Ids(ids) if ids.is_empty() => {
            return Err(format!("{name:?}'s inbound users: is empty, so nobody could write to it; name at least one Telegram user id"))
        }
        Allowlist::Ids(ids) => {
            if let Some(bad) = ids.iter().find(|id| **id <= 0) {
                return Err(format!("{name:?}'s inbound users: {bad} is not a Telegram user id"));
            }
        }
        Allowlist::From(ProviderCredential::Secret(secret)) if crate::secrets::find(secrets, secret).is_none() => {
            return Err(format!(
                "{name:?}'s inbound users: names {{ secret: {secret} }}, which the instance root's secrets: does not declare"
            ))
        }
        Allowlist::From(ProviderCredential::Secret(_)) => {}
        Allowlist::From(ProviderCredential::Source(source)) => {
            if let Some(problem) = source.problems(&format!("the channel {name:?}'s inbound users")).into_iter().next() {
                return Err(problem);
            }
        }
    }
    for (field, value) in [("scope", &inbound.scope), ("agent", &inbound.agent)] {
        if value.trim().is_empty() {
            return Err(format!("{name:?}'s inbound {field}: is empty; it names the {field} a message becomes a task for"));
        }
    }
    if let Some(dir) = &inbound.files {
        if !(dir.starts_with('/') || dir.starts_with("~/")) || dir.chars().any(char::is_control) {
            return Err(format!("{name:?}'s inbound files: {dir:?} is not an absolute path or one under ~/"));
        }
    }
    match inbound.max_file_bytes {
        Some(0) => return Err(format!("{name:?}'s inbound max_file_bytes: 0 would refuse every file")),
        Some(bytes) if bytes > DOCUMENT_LIMIT => {
            return Err(format!("{name:?}'s inbound max_file_bytes: {bytes} is over the Bot API's 50 MB"))
        }
        _ => {}
    }
    Ok(())
}

pub fn find<'a>(channels: &'a [ChannelDecl], name: &str) -> Option<&'a ChannelDecl> {
    channels.iter().find(|c| c.name == name)
}

/// The names a refusal lists: `owner-telegram, ops`, or `nothing`.
pub fn names(channels: &[ChannelDecl]) -> String {
    if channels.is_empty() {
        return "nothing".into();
    }
    channels.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")
}

fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// The longest prefix of `text` that fits in `limit` UTF-16 code units, as
/// a byte index on a character boundary.
fn fitting_prefix(text: &str, limit: usize) -> usize {
    let mut units = 0;
    for (at, c) in text.char_indices() {
        units += c.len_utf16();
        if units > limit {
            return at;
        }
    }
    text.len()
}

/// `text` cut into Telegram messages of at most `TEXT_LIMIT` each. A text
/// that fits is one message, unchanged. A longer one is cut at the last line
/// break in each message's second half, else the last space there, else
/// wherever the limit falls; nothing is dropped except parts that would be
/// only whitespace, which Telegram refuses. More than `MAX_PARTS` messages,
/// or nothing to send at all, is refused.
pub fn split_text(text: &str) -> std::result::Result<Vec<String>, String> {
    if text.trim().is_empty() {
        return Err("there is no text to send".into());
    }
    let mut parts = Vec::new();
    let mut rest = text;
    while utf16_len(rest) > TEXT_LIMIT {
        let fits = fitting_prefix(rest, TEXT_LIMIT);
        let window = &rest[..fits];
        let half = fits / 2;
        let cut = window
            .rfind('\n')
            .filter(|at| *at >= half)
            .or_else(|| window.rfind(' ').filter(|at| *at >= half))
            .map(|at| at + 1)
            .unwrap_or(fits);
        parts.push(rest[..cut].to_string());
        rest = &rest[cut..];
    }
    parts.push(rest.to_string());
    parts.retain(|part| !part.trim().is_empty());
    if parts.len() > MAX_PARTS {
        return Err(format!(
            "the text is {} characters, more than {MAX_PARTS} Telegram messages of {TEXT_LIMIT}; send it as a file with --file instead",
            text.chars().count()
        ));
    }
    Ok(parts)
}

/// The SHA-256 of some bytes, as lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// What makes two sends the same message: the channel, the text and every
/// file's content, in order. Never stored with the text it was made from.
pub fn message_digest(channel: &str, text: &str, file_hashes: &[String]) -> String {
    let mut hash = Sha256::new();
    for part in [channel, text].into_iter().chain(file_hashes.iter().map(String::as_str)) {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// A file name Telegram is given for an upload: the name's own letters,
/// digits, `.`, `-` and `_`, anything else as `_`, at most 64 characters.
/// Never a path, and never anything `curl`'s form syntax would read.
pub fn upload_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .take(64)
        .collect();
    if clean.trim_matches(['.', '_']).is_empty() {
        "file".into()
    } else {
        clean
    }
}

/// One `--config` value, double-quoted the way `curl` reads it. A line
/// break cannot be in one at all: refused rather than escaped.
fn quoted(value: &str) -> std::result::Result<String, String> {
    if value.chars().any(|c| c.is_control()) {
        return Err("it contains a control character".into());
    }
    Ok(format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\"")))
}

/// A form field of one Bot API call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field {
    /// Taken literally (`--form-string`): a chat id like `@channel` is not
    /// a file to upload.
    Text(&'static str, String),
    /// The value is the content of this file (`--form name=<path`).
    FromFile(&'static str, String),
    /// Upload this file under this name (`--form name=@path;filename=..`).
    Upload(&'static str, String, String),
}

/// The `curl --config -` text for one Bot API call: the URL with the token
/// in it, and the form. It is written to `curl`'s stdin, so neither the
/// token nor the chat is ever on a command line. Paths are the daemon's own
/// staging files; one with a character `curl`'s form syntax would read is
/// refused, as is a token or a chat with a line break in it.
pub fn curl_config(api: &str, token: &str, method: &str, fields: &[Field]) -> std::result::Result<String, String> {
    let plain = |path: &str| path.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_'));
    let mut out = String::new();
    let url = quoted(&format!("{api}/bot{token}/{method}")).map_err(|e| format!("the bot token cannot be used: {e}"))?;
    out.push_str(&format!("url = {url}\n"));
    for field in fields {
        let line = match field {
            Field::Text(name, value) => {
                let value = quoted(&format!("{name}={value}")).map_err(|e| format!("the {name} cannot be used: {e}"))?;
                format!("form-string = {value}\n")
            }
            Field::FromFile(name, path) if plain(path) => format!("form = \"{name}=<{path}\"\n"),
            Field::Upload(name, path, filename) if plain(path) && plain(filename) => {
                format!("form = \"{name}=@{path};filename={filename}\"\n")
            }
            _ => return Err("a staging path or file name has characters curl's form syntax reads".into()),
        };
        out.push_str(&line);
    }
    Ok(out)
}

/// The Bot API's answer to one call: the id of the message it made, or why
/// not, in Telegram's own words (its `description` never carries the token).
pub fn read_answer(http_code: u16, body: &str) -> std::result::Result<i64, String> {
    #[derive(Deserialize)]
    struct Answer {
        ok: bool,
        #[serde(default)]
        result: Option<serde_json::Value>,
        #[serde(default)]
        error_code: Option<i64>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        parameters: Option<serde_json::Value>,
    }
    let Ok(answer) = serde_json::from_str::<Answer>(body) else {
        return Err(format!("the Bot API answered HTTP {http_code} with no answer it understands"));
    };
    if answer.ok {
        return answer
            .result
            .as_ref()
            .and_then(|r| r.get("message_id"))
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| "the Bot API said ok but named no message".to_string());
    }
    let mut said = format!(
        "Telegram refused it ({}): {}",
        answer.error_code.unwrap_or(http_code as i64),
        answer.description.unwrap_or_else(|| "no reason given".into())
    );
    if let Some(after) = answer.parameters.as_ref().and_then(|p| p.get("retry_after")).and_then(serde_json::Value::as_i64) {
        said.push_str(&format!("; try again in {after}s"));
    }
    Err(said)
}

/// `text` with every occurrence of each value replaced: what an error is
/// passed through before it leaves the send, whatever `curl` or Telegram
/// said.
pub fn redact(text: &str, values: &[&str]) -> String {
    let mut out = text.to_string();
    for value in values.iter().filter(|v| v.len() >= 4) {
        out = out.replace(value, "<redacted>");
    }
    out
}

/// One file to send, already read by whoever had the right to read it.
#[derive(Clone)]
pub struct OutgoingFile {
    /// The name Telegram shows (`upload_name` is applied on the way out).
    pub name: String,
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for OutgoingFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutgoingFile").field("bytes", &self.bytes.len()).finish()
    }
}

/// A button under a message (`#338`): what it says, and what its press
/// hands back through the inbound poll. Telegram allows 64 bytes of data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Button {
    pub text: String,
    pub data: String,
}

/// One send through a channel: the command L4 gives, through L3, to L2.
/// The recipient is not in it: the channel's declaration fixes it.
#[derive(Clone)]
pub struct Outgoing {
    pub channel: String,
    pub text: String,
    pub files: Vec<OutgoingFile>,
    /// The run it is sent from, for the repeat ledger; `None` is the owner.
    pub run: Option<String>,
    pub task: Option<String>,
    /// The inbound message this answers (`#338`): the first message goes
    /// as a reply to it, and two answers to different messages are never
    /// one another's repeats, however alike they read.
    pub reply_to: Option<i64>,
    /// Buttons under the last message of the text (`#338`).
    pub buttons: Vec<Button>,
}

impl std::fmt::Debug for Outgoing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Outgoing")
            .field("channel", &self.channel)
            .field("text", &self.text.len())
            .field("files", &self.files)
            .field("run", &self.run)
            .field("reply_to", &self.reply_to)
            .field("buttons", &self.buttons.len())
            .finish()
    }
}

impl Outgoing {
    /// `message_digest`, with what this send answers and the buttons it
    /// carries folded in: a reply is the same message as another only when
    /// it answers the same message the same way.
    pub fn digest(&self, file_hashes: &[String]) -> String {
        let mut parts = file_hashes.to_vec();
        if let Some(id) = self.reply_to {
            parts.push(format!("reply:{id}"));
        }
        parts.extend(self.buttons.iter().map(|b| format!("button:{}\u{0}{}", b.text, b.data)));
        message_digest(&self.channel, &self.text, &parts)
    }
}

/// `reply_parameters` for a message that answers `id`: shown as a reply,
/// and still sent if that message is gone.
pub fn reply_parameters(id: i64) -> String {
    serde_json::json!({ "message_id": id, "allow_sending_without_reply": true }).to_string()
}

/// `reply_markup` for one row of buttons, or why it cannot be sent.
pub fn inline_keyboard(buttons: &[Button]) -> std::result::Result<String, String> {
    if buttons.len() > 8 {
        return Err("at most 8 buttons go under one message".into());
    }
    for button in buttons {
        if button.text.trim().is_empty() || button.data.is_empty() || button.data.len() > 64 {
            return Err("a button needs a label and 1 to 64 bytes of data".into());
        }
    }
    let row: Vec<_> = buttons.iter().map(|b| serde_json::json!({ "text": b.text, "callback_data": b.data })).collect();
    Ok(serde_json::json!({ "inline_keyboard": [row] }).to_string())
}

// ------------------------------------------------------------------ inbound

/// One file a message brought in, saved on the host (`#338`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceivedFile {
    /// Absolute, on the daemon's host.
    pub path: String,
    /// `document`, `photo`, `voice`, `audio`, `video` or `video_note`.
    pub kind: String,
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
}

/// What one Telegram update became, after the allowlist (`#338`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Inbound {
    /// A message from an allowed user in the channel's chat: its text (or
    /// a file's caption), and the files it brought, already saved.
    Message {
        update: i64,
        message: i64,
        from: i64,
        at: DateTime<Utc>,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files: Vec<ReceivedFile>,
        /// Files that were not taken, and why: too big, or the download
        /// failed. Never their content.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        refused: Vec<String>,
        /// The channel's message this one replies to, when the owner used
        /// Telegram's reply -- how an answer finds the question it answers.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply_to: Option<i64>,
    },
    /// An allowed user pressed a button under one of the channel's
    /// messages. The press has already been answered, so the button stops
    /// spinning.
    Button {
        update: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<i64>,
        from: i64,
        data: String,
    },
    /// Anything else: someone not on the allowlist, another chat, a bot,
    /// a kind of update the gateway does not take. Nothing of its content
    /// is kept -- only who and where, so it can be journaled.
    Ignored {
        update: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chat: Option<i64>,
        what: String,
    },
}

impl Inbound {
    pub fn update(&self) -> i64 {
        match self {
            Self::Message { update, .. } | Self::Button { update, .. } | Self::Ignored { update, .. } => *update,
        }
    }
}

/// One poll's answer: what came in, and the offset the next poll passes,
/// which tells Telegram everything before it was handled.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboundBatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<i64>,
    #[serde(default)]
    pub items: Vec<Inbound>,
}

/// A file a message names, before it is downloaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRef {
    pub file_id: String,
    pub kind: &'static str,
    /// What the saved file is called: the sender's name for it reduced to
    /// `upload_name`, or the kind's own.
    pub name: String,
    pub size: Option<u64>,
    pub mime: Option<String>,
}

/// One update as the gateway sorts it, before any file is fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sorted {
    Message { update: i64, message: i64, from: i64, at: DateTime<Utc>, text: String, files: Vec<FileRef>, reply_to: Option<i64> },
    Button { update: i64, query: String, message: Option<i64>, from: i64, data: String },
    Ignored { update: i64, from: Option<i64>, chat: Option<i64>, what: String },
}

#[derive(Deserialize)]
struct TgUser {
    id: i64,
    #[serde(default)]
    is_bot: bool,
}

#[derive(Deserialize)]
struct TgChat {
    id: i64,
}

#[derive(Deserialize)]
struct TgFile {
    file_id: String,
    #[serde(default)]
    file_name: Option<String>,
    #[serde(default)]
    mime_type: Option<String>,
    #[serde(default)]
    file_size: Option<u64>,
}

#[derive(Deserialize)]
struct TgMessage {
    message_id: i64,
    #[serde(default)]
    from: Option<TgUser>,
    chat: TgChat,
    #[serde(default)]
    date: i64,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    caption: Option<String>,
    #[serde(default)]
    document: Option<TgFile>,
    #[serde(default)]
    photo: Option<Vec<TgFile>>,
    #[serde(default)]
    voice: Option<TgFile>,
    #[serde(default)]
    audio: Option<TgFile>,
    #[serde(default)]
    video: Option<TgFile>,
    #[serde(default)]
    video_note: Option<TgFile>,
    /// The message this one replies to; only its id is read.
    #[serde(default)]
    reply_to_message: Option<TgReplied>,
}

#[derive(Deserialize)]
struct TgReplied {
    message_id: i64,
}

#[derive(Deserialize)]
struct TgCallback {
    id: String,
    from: TgUser,
    #[serde(default)]
    message: Option<TgMessage>,
    #[serde(default)]
    data: Option<String>,
}

#[derive(Deserialize)]
struct TgUpdate {
    #[serde(default)]
    message: Option<TgMessage>,
    #[serde(default)]
    callback_query: Option<TgCallback>,
}

/// Why the Bot API would not do what it was asked: Telegram's own code and
/// words. `409` is another consumer polling the same token, or a webhook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: i64,
    pub said: String,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Telegram refused it ({}): {}", self.code, self.said)
    }
}

/// The `result` of any Bot API answer, or Telegram's refusal.
pub fn answer_result(http_code: u16, body: &str) -> std::result::Result<serde_json::Value, Refusal> {
    let Ok(answer) = serde_json::from_str::<serde_json::Value>(body) else {
        return Err(Refusal { code: http_code as i64, said: format!("HTTP {http_code} with no answer it understands") });
    };
    if answer.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(answer.get("result").cloned().unwrap_or(serde_json::Value::Null));
    }
    Err(Refusal {
        code: answer.get("error_code").and_then(serde_json::Value::as_i64).unwrap_or(http_code as i64),
        said: answer.get("description").and_then(serde_json::Value::as_str).unwrap_or("no reason given").to_string(),
    })
}

fn file_ref(kind: &'static str, file: TgFile, fallback: &str) -> FileRef {
    let name = match &file.file_name {
        Some(name) if !name.trim().is_empty() => upload_name(name),
        _ => fallback.to_string(),
    };
    FileRef { file_id: file.file_id, kind, name, size: file.file_size, mime: file.mime_type }
}

/// The files a message carries. Of a photo's sizes, the largest that fits
/// `limit` -- or, when none does, the largest, to be refused by name.
fn message_files(message: &mut TgMessage, limit: u64) -> Vec<FileRef> {
    let mut files = Vec::new();
    if let Some(file) = message.document.take() {
        files.push(file_ref("document", file, "document"));
    }
    if let Some(mut sizes) = message.photo.take() {
        sizes.sort_by_key(|s| s.file_size.unwrap_or(0));
        let pick = sizes.iter().rposition(|s| s.file_size.is_none_or(|n| n <= limit)).unwrap_or(sizes.len().saturating_sub(1));
        if !sizes.is_empty() {
            files.push(file_ref("photo", sizes.swap_remove(pick), "photo.jpg"));
        }
    }
    for (kind, file, fallback) in [
        ("voice", message.voice.take(), "voice.ogg"),
        ("audio", message.audio.take(), "audio"),
        ("video", message.video.take(), "video.mp4"),
        ("video_note", message.video_note.take(), "video-note.mp4"),
    ] {
        if let Some(file) = file {
            files.push(file_ref(kind, file, fallback));
        }
    }
    files
}

/// `getUpdates`' answer, sorted against the channel's one chat and its
/// allowlist. Only a message from an allowed user (not a bot) in that chat,
/// or an allowed user's button press under a message in it, is taken;
/// everything else is `Ignored`, carrying no content.
pub fn sort_updates(result: &serde_json::Value, chat: i64, users: &[i64], limit: u64) -> std::result::Result<Vec<Sorted>, String> {
    let raw = result.as_array().ok_or("getUpdates did not answer with a list")?;
    let mut sorted = Vec::new();
    for value in raw {
        let Some(update_id) = value.get("update_id").and_then(serde_json::Value::as_i64) else {
            return Err("getUpdates answered an update with no update_id".into());
        };
        let ignored = |from: Option<i64>, in_chat: Option<i64>, what: &str| Sorted::Ignored {
            update: update_id,
            from,
            chat: in_chat,
            what: what.to_string(),
        };
        let Ok(update) = serde_json::from_value::<TgUpdate>(value.clone()) else {
            sorted.push(ignored(None, None, "an update the gateway could not read"));
            continue;
        };
        if let Some(mut message) = update.message {
            let from = message.from.as_ref().map(|u| u.id);
            let allowed = message.from.as_ref().is_some_and(|u| !u.is_bot && users.contains(&u.id));
            sorted.push(if message.chat.id != chat {
                ignored(from, Some(message.chat.id), "a message in another chat")
            } else if !allowed {
                ignored(from, Some(message.chat.id), "a message from a user who is not on the allowlist")
            } else {
                let text = message.text.take().or_else(|| message.caption.take()).unwrap_or_default();
                let files = message_files(&mut message, limit);
                Sorted::Message {
                    update: update_id,
                    message: message.message_id,
                    from: from.unwrap_or_default(),
                    at: DateTime::from_timestamp(message.date, 0).unwrap_or_else(Utc::now),
                    text,
                    files,
                    reply_to: message.reply_to_message.as_ref().map(|replied| replied.message_id),
                }
            });
        } else if let Some(query) = update.callback_query {
            let in_chat = query.message.as_ref().map(|m| m.chat.id);
            let allowed = !query.from.is_bot && users.contains(&query.from.id) && in_chat == Some(chat);
            sorted.push(if allowed {
                Sorted::Button {
                    update: update_id,
                    query: query.id,
                    message: query.message.as_ref().map(|m| m.message_id),
                    from: query.from.id,
                    data: query.data.unwrap_or_default(),
                }
            } else {
                ignored(Some(query.from.id), in_chat, "a button press from a user who is not on the allowlist, or under another chat")
            });
        } else {
            sorted.push(ignored(None, None, "a kind of update the gateway does not take"));
        }
    }
    Ok(sorted)
}

/// `getFile`'s answer: the path to download the file from, and its size
/// when Telegram says. The path goes into a URL, so it must be Telegram's
/// own plain shape -- letters, digits, `/`, `.`, `-`, `_`, never `..`.
pub fn file_location(result: &serde_json::Value) -> std::result::Result<(String, Option<u64>), String> {
    let path = result.get("file_path").and_then(serde_json::Value::as_str).ok_or("getFile named no file_path")?;
    let plain = !path.is_empty()
        && !path.starts_with('/')
        && !path.split('/').any(|part| part.is_empty() || part == "..")
        && path.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_'));
    if !plain {
        return Err("getFile named a file_path that is not a plain relative path".into());
    }
    Ok((path.to_string(), result.get("file_size").and_then(serde_json::Value::as_u64)))
}

/// The `curl --config -` text that downloads one file into `output`, and
/// stops past `limit` bytes. The token is in the URL, as Telegram has it.
pub fn download_config(api: &str, token: &str, file_path: &str, output: &str, limit: u64) -> std::result::Result<String, String> {
    let url = quoted(&format!("{api}/file/bot{token}/{file_path}")).map_err(|e| format!("the bot token cannot be used: {e}"))?;
    let output = quoted(output).map_err(|e| format!("the file's path cannot be used: {e}"))?;
    Ok(format!("url = {url}\noutput = {output}\nmax-filesize = {limit}\n"))
}

/// The acknowledgement of a send: the ids it made, or the earlier send it
/// repeats. Never the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Receipt {
    Sent {
        /// The ledger's id for this send.
        id: String,
        /// Telegram's message ids: the text's messages, then each file's.
        messages: Vec<i64>,
        documents: Vec<i64>,
    },
    /// Skipped: the same message went to this channel at `sent_at`, within
    /// `REPEAT_WINDOW_HOURS`.
    Repeat { id: String, sent_at: DateTime<Utc> },
}

/// L2's channel command (`Commands<L3, L2Port>`): send one message through a
/// declared channel to its fixed recipient.
#[async_trait::async_trait]
pub trait ChannelSends: CommandPort<Level = L2> + Send + Sync {
    async fn deliver(&self, outgoing: Outgoing) -> Result<Receipt>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openshell::CredentialSource;

    const SECRETS: &str = "\
- name: telegram-bot-token
  kind: token
  source: { from: keychain, service: factory-telegram-bot }
  expires: never
- name: owner-chat
  kind: other
  source: { from: file, path: ~/.config/factory/secrets/owner-chat }
";

    fn secrets() -> Vec<SecretDecl> {
        serde_yaml_ng::from_str(SECRETS).unwrap()
    }

    fn parse(yaml: &str) -> Vec<ChannelDecl> {
        serde_yaml_ng::from_str(yaml).unwrap()
    }

    #[test]
    fn the_issues_block_parses_validates_and_round_trips() {
        let channels = parse(
            "- name: owner-telegram\n  kind: telegram\n  token: { secret: telegram-bot-token }\n  chat: { from: env, name: FACTORY_OWNER_TELEGRAM_CHAT }\n",
        );
        validate(&channels, &secrets()).unwrap();
        assert_eq!(channels[0].kind, ChannelKind::Telegram);
        assert_eq!(channels[0].token, ProviderCredential::Secret("telegram-bot-token".into()));
        assert_eq!(
            channels[0].chat,
            ProviderCredential::Source(CredentialSource::Env { name: "FACTORY_OWNER_TELEGRAM_CHAT".into() })
        );
        assert_eq!(channels[0].api_base(), TELEGRAM_API);
        let again: Vec<ChannelDecl> = serde_yaml_ng::from_str(&serde_yaml_ng::to_string(&channels).unwrap()).unwrap();
        assert_eq!(again, channels);
    }

    #[test]
    fn a_block_that_cannot_work_or_would_leak_is_refused_by_name() {
        let ok = "token: { secret: telegram-bot-token }, chat: { secret: owner-chat }";
        for (yaml, expected) in [
            (format!("[{{ name: a, kind: telegram, {ok} }}, {{ name: a, kind: telegram, {ok} }}]"), "twice".to_string()),
            (format!("[{{ name: 'a b', kind: telegram, {ok} }}]"), "not a channel name".into()),
            ("[{ name: a, kind: telegram, token: { from: env, name: BOT }, chat: { secret: owner-chat } }]".into(), "inline".into()),
            ("[{ name: a, kind: telegram, token: { secret: nope }, chat: { secret: owner-chat } }]".into(), "does not declare".into()),
            ("[{ name: a, kind: telegram, token: { secret: telegram-bot-token }, chat: { secret: nope } }]".into(), "does not declare".into()),
            ("[{ name: a, kind: telegram, token: { secret: telegram-bot-token }, chat: { from: file, path: rel } }]".into(), "absolute".into()),
            (format!("[{{ name: a, kind: telegram, {ok}, api: 'http://example.com' }}]"), "use https".into()),
            (format!("[{{ name: a, kind: telegram, {ok}, api: 'https://u:p@example.com' }}]"), "credentials".into()),
            (format!("[{{ name: a, kind: telegram, {ok}, api: 'ftp://example.com' }}]"), "not an http".into()),
            (format!("[{{ name: a, kind: telegram, {ok}, api: 'example.com' }}]"), "not a URL".into()),
        ] {
            let e = validate(&parse(&yaml), &secrets()).unwrap_err().to_string();
            assert!(e.contains(&expected), "{yaml}: {e}");
        }
        for (bad, expected) in [
            ("- { name: a, kind: email, token: { secret: t }, chat: { secret: c } }", "email"),
            ("- { name: a, kind: telegram, token: { secret: t }, chat: { secret: c }, to: 42 }", "to"),
            ("- { name: a, kind: telegram, token: { secret: t, from: env }, chat: { secret: c } }", "not both"),
        ] {
            let e = serde_yaml_ng::from_str::<Vec<ChannelDecl>>(bad).unwrap_err().to_string();
            assert!(e.contains(expected), "{bad}: {e}");
        }
        for api in ["https://api.telegram.org/", "http://127.0.0.1:8081", "http://localhost:9/x", "http://[::1]:9"] {
            let yaml = format!("[{{ name: a, kind: telegram, {ok}, api: '{api}' }}]");
            validate(&parse(&yaml), &secrets()).unwrap_or_else(|e| panic!("{api}: {e}"));
        }
        assert_eq!(parse(&format!("[{{ name: a, kind: telegram, {ok}, api: 'https://x.test/' }}]"))[0].api_base(), "https://x.test");
    }

    const OK: &str = "token: { secret: telegram-bot-token }, chat: { secret: owner-chat }";

    #[test]
    fn an_inbound_block_parses_with_ids_or_a_source_and_round_trips() {
        let channels = parse(&format!(
            "[{{ name: owner-telegram, kind: telegram, {OK}, inbound: {{ users: [123456789], scope: assistant, agent: assistant, language: de }} }}]"
        ));
        validate(&channels, &secrets()).unwrap();
        let inbound = channels[0].inbound.as_ref().unwrap();
        assert_eq!(inbound.users, Allowlist::Ids(vec![123456789]));
        assert_eq!((inbound.file_limit(), inbound.language), (INBOUND_FILE_LIMIT, Some(Language::De)));
        let again: Vec<ChannelDecl> = serde_yaml_ng::from_str(&serde_yaml_ng::to_string(&channels).unwrap()).unwrap();
        assert_eq!(again, channels);

        let from_secret = parse(&format!(
            "[{{ name: a, kind: telegram, {OK}, inbound: {{ users: {{ secret: owner-chat }}, scope: s, agent: a, files: ~/inbox, max_file_bytes: 1024 }} }}]"
        ));
        validate(&from_secret, &secrets()).unwrap();
        assert_eq!(from_secret[0].inbound.as_ref().unwrap().users, Allowlist::From(ProviderCredential::Secret("owner-chat".into())));
        assert_eq!(parse_user_ids("123, 456\n789").unwrap(), vec![123, 456, 789]);
        assert!(parse_user_ids("123, @someone").unwrap_err().contains("@someone"));
        assert!(parse_user_ids(" ").unwrap_err().contains("no Telegram user id"));
    }

    #[test]
    fn an_inbound_block_that_could_not_work_is_refused_and_one_token_has_one_poller() {
        for (inbound, expected) in [
            ("{ users: [], scope: s, agent: a }", "is empty"),
            ("{ users: [-5], scope: s, agent: a }", "not a Telegram user id"),
            ("{ users: { secret: nope }, scope: s, agent: a }", "does not declare"),
            ("{ users: { from: file, path: rel }, scope: s, agent: a }", "absolute"),
            ("{ users: [1], scope: '', agent: a }", "scope: is empty"),
            ("{ users: [1], scope: s, agent: a, files: inbox }", "not an absolute path"),
            ("{ users: [1], scope: s, agent: a, max_file_bytes: 0 }", "refuse every file"),
            ("{ users: [1], scope: s, agent: a, max_file_bytes: 60000000 }", "50 MB"),
        ] {
            let yaml = format!("[{{ name: a, kind: telegram, {OK}, inbound: {inbound} }}]");
            let e = validate(&parse(&yaml), &secrets()).unwrap_err().to_string();
            assert!(e.contains(expected), "{inbound}: {e}");
        }
        let e = serde_yaml_ng::from_str::<Vec<ChannelDecl>>(
            "- { name: a, kind: telegram, token: { secret: t }, chat: { secret: c }, inbound: { users: [1], scope: s, agent: a, chat: 5 } }",
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("chat"), "{e}");

        let inbound = "inbound: { users: [1], scope: s, agent: a }";
        let two = format!("[{{ name: a, kind: telegram, {OK}, {inbound} }}, {{ name: b, kind: telegram, {OK}, {inbound} }}]");
        let e = validate(&parse(&two), &secrets()).unwrap_err().to_string();
        assert!(e.contains("exactly one getUpdates consumer") && e.contains("\"a\" and \"b\""), "{e}");
        // Sending through the same token from a second, send-only channel is fine.
        let one = format!("[{{ name: a, kind: telegram, {OK}, {inbound} }}, {{ name: b, kind: telegram, {OK} }}]");
        validate(&parse(&one), &secrets()).unwrap();
    }

    fn update(id: i64, body: serde_json::Value) -> serde_json::Value {
        let mut value = body;
        value["update_id"] = id.into();
        value
    }

    #[test]
    fn updates_are_sorted_against_the_chat_and_the_allowlist_and_a_stranger_keeps_nothing() {
        let chat = 4242;
        let result = serde_json::json!([
            update(1, serde_json::json!({ "message": { "message_id": 10, "date": 1_790_000_000, "from": { "id": 7, "is_bot": false }, "chat": { "id": chat }, "text": "hallo" } })),
            update(2, serde_json::json!({ "message": { "message_id": 11, "date": 0, "from": { "id": 99 }, "chat": { "id": chat }, "text": "SECRET-STRANGER" } })),
            update(3, serde_json::json!({ "message": { "message_id": 12, "date": 0, "from": { "id": 7 }, "chat": { "id": 1 }, "text": "elsewhere" } })),
            update(4, serde_json::json!({ "message": { "message_id": 13, "date": 0, "from": { "id": 7 }, "chat": { "id": chat }, "caption": "the report",
                "document": { "file_id": "D1", "file_name": "../Report 1.pdf", "mime_type": "application/pdf", "file_size": 2048 } } })),
            update(5, serde_json::json!({ "message": { "message_id": 14, "date": 0, "from": { "id": 7 }, "chat": { "id": chat },
                "photo": [ { "file_id": "P-small", "file_size": 100 }, { "file_id": "P-huge", "file_size": 99_000_000 }, { "file_id": "P-mid", "file_size": 5000 } ],
                "voice": { "file_id": "V1", "mime_type": "audio/ogg", "file_size": 300 } } })),
            update(6, serde_json::json!({ "callback_query": { "id": "Q1", "from": { "id": 7 }, "data": "ok:abc", "message": { "message_id": 20, "chat": { "id": chat } } } })),
            update(7, serde_json::json!({ "callback_query": { "id": "Q2", "from": { "id": 99 }, "data": "ok:abc", "message": { "message_id": 20, "chat": { "id": chat } } } })),
            update(8, serde_json::json!({ "edited_message": { "message_id": 10 } })),
            update(9, serde_json::json!({ "message": { "message_id": 15, "date": 0, "from": { "id": 7, "is_bot": true }, "chat": { "id": chat }, "text": "bot" } })),
        ]);
        let sorted = sort_updates(&result, chat, &[7], 10_000).unwrap();
        assert_eq!(sorted.len(), 9);
        assert!(matches!(&sorted[0], Sorted::Message { message: 10, from: 7, text, files, .. } if text == "hallo" && files.is_empty()));
        match &sorted[1] {
            Sorted::Ignored { from: Some(99), chat: Some(4242), what, .. } => assert!(what.contains("allowlist")),
            other => panic!("{other:?}"),
        }
        assert!(!format!("{sorted:?}").contains("SECRET-STRANGER"), "nothing of a stranger's message is kept");
        assert!(matches!(&sorted[2], Sorted::Ignored { what, .. } if what.contains("another chat")));
        match &sorted[3] {
            Sorted::Message { text, files, .. } => {
                assert_eq!(text, "the report");
                assert_eq!((files[0].file_id.as_str(), files[0].name.as_str(), files[0].size), ("D1", ".._Report_1.pdf", Some(2048)));
            }
            other => panic!("{other:?}"),
        }
        match &sorted[4] {
            Sorted::Message { files, .. } => {
                let ids: Vec<_> = files.iter().map(|f| (f.kind, f.file_id.as_str())).collect();
                assert_eq!(ids, vec![("photo", "P-mid"), ("voice", "V1")], "the largest photo that fits");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(&sorted[5], Sorted::Button { query, data, message: Some(20), from: 7, .. } if query == "Q1" && data == "ok:abc"));
        assert!(matches!(&sorted[6], Sorted::Ignored { from: Some(99), .. }));
        assert!(matches!(&sorted[7], Sorted::Ignored { what, .. } if what.contains("does not take")));
        assert!(matches!(&sorted[8], Sorted::Ignored { from: Some(7), .. }), "a bot is never taken");
        assert!(sort_updates(&serde_json::json!({}), chat, &[7], 1).is_err());
    }

    #[test]
    fn a_reply_names_the_message_it_answers() {
        let chat = 4242;
        let result = serde_json::json!([
            update(1, serde_json::json!({ "message": { "message_id": 30, "date": 0, "from": { "id": 7 }, "chat": { "id": chat }, "text": "eu-central",
                "reply_to_message": { "message_id": 12, "date": 0, "chat": { "id": chat }, "text": "Which region?" } } })),
            update(2, serde_json::json!({ "message": { "message_id": 31, "date": 0, "from": { "id": 7 }, "chat": { "id": chat }, "text": "hallo" } })),
        ]);
        let sorted = sort_updates(&result, chat, &[7], 10_000).unwrap();
        assert!(matches!(&sorted[0], Sorted::Message { reply_to: Some(12), text, .. } if text == "eu-central"));
        assert!(matches!(&sorted[1], Sorted::Message { reply_to: None, .. }));
    }

    #[test]
    fn the_bot_apis_answers_and_paths_are_read_strictly() {
        assert_eq!(answer_result(200, r#"{"ok":true,"result":[]}"#), Ok(serde_json::json!([])));
        let conflict = answer_result(409, r#"{"ok":false,"error_code":409,"description":"Conflict: terminated by other getUpdates request"}"#).unwrap_err();
        assert_eq!(conflict.code, 409);
        assert!(conflict.to_string().contains("terminated by other getUpdates"));
        assert_eq!(answer_result(502, "<html>").unwrap_err().code, 502);

        let ok = serde_json::json!({ "file_path": "documents/file_1.pdf", "file_size": 12 });
        assert_eq!(file_location(&ok).unwrap(), ("documents/file_1.pdf".into(), Some(12)));
        for bad in ["../etc/passwd", "/abs", "a//b", "a b", "docs/../x", ""] {
            assert!(file_location(&serde_json::json!({ "file_path": bad })).is_err(), "{bad}");
        }
        let config = download_config("http://127.0.0.1:9", "1:T", "documents/file_1.pdf", "/Users/x/Application Support/in/1-0-a.pdf", 99).unwrap();
        assert_eq!(
            config,
            "url = \"http://127.0.0.1:9/file/bot1:T/documents/file_1.pdf\"\noutput = \"/Users/x/Application Support/in/1-0-a.pdf\"\nmax-filesize = 99\n"
        );
        assert!(download_config("x", "1:T", "a", "/tmp/a\nb", 1).is_err());
    }

    #[test]
    fn a_reply_and_its_buttons_make_it_a_different_message() {
        let base = Outgoing {
            channel: "c".into(),
            text: "ok".into(),
            files: Vec::new(),
            run: None,
            task: None,
            reply_to: None,
            buttons: Vec::new(),
        };
        let plain = base.digest(&[]);
        assert_eq!(plain, message_digest("c", "ok", &[]), "without a reply it is the #337 digest");
        let to_one = Outgoing { reply_to: Some(1), ..base.clone() };
        let to_two = Outgoing { reply_to: Some(2), ..base.clone() };
        assert_ne!(to_one.digest(&[]), plain);
        assert_ne!(to_one.digest(&[]), to_two.digest(&[]));
        let asked = Outgoing { buttons: vec![Button { text: "Ja".into(), data: "ok:1".into() }], ..base.clone() };
        assert_ne!(asked.digest(&[]), plain);

        // Compared as JSON, not as text: the daemon builds serde_json with
        // `preserve_order`, so a workspace build keeps the keys as written and
        // a build of this crate alone sorts them. Telegram reads either.
        let json = |text: &str| serde_json::from_str::<serde_json::Value>(text).unwrap();
        assert_eq!(json(&reply_parameters(42)), serde_json::json!({ "message_id": 42, "allow_sending_without_reply": true }));
        let keyboard = inline_keyboard(&[Button { text: "Ja".into(), data: "ok:1".into() }, Button { text: "Nein".into(), data: "no:1".into() }]).unwrap();
        assert_eq!(
            json(&keyboard),
            serde_json::json!({ "inline_keyboard": [[{ "text": "Ja", "callback_data": "ok:1" }, { "text": "Nein", "callback_data": "no:1" }]] })
        );
        assert!(inline_keyboard(&[Button { text: "x".into(), data: "y".repeat(65) }]).is_err());
    }

    #[test]
    fn a_text_that_fits_is_one_message_and_a_long_one_is_cut_at_line_breaks() {
        assert_eq!(split_text("hello").unwrap(), vec!["hello"]);
        assert!(split_text("  \n ").unwrap_err().contains("no text"));
        let exact = "x".repeat(TEXT_LIMIT);
        assert_eq!(split_text(&exact).unwrap(), vec![exact.clone()]);

        let line = format!("{}\n", "y".repeat(99));
        let text = line.repeat(50); // 5000 characters, 100 per line
        let parts = split_text(&text).unwrap();
        assert_eq!(parts.len(), 2);
        assert!(parts.iter().all(|p| utf16_len(p) <= TEXT_LIMIT));
        assert!(parts[0].ends_with('\n'), "cut after a line break");
        assert_eq!(parts.concat(), text, "nothing lost");

        // No line breaks: cut at a space; none at all: at the limit.
        let words = "word ".repeat(1000);
        let parts = split_text(&words).unwrap();
        assert!(parts[0].ends_with(' ') && parts.concat() == words);
        let solid = "z".repeat(TEXT_LIMIT + 10);
        let parts = split_text(&solid).unwrap();
        assert_eq!(parts.iter().map(|p| p.len()).collect::<Vec<_>>(), vec![TEXT_LIMIT, 10]);
    }

    #[test]
    fn the_limit_counts_utf16_units_never_splits_a_character_and_caps_the_parts() {
        // An emoji is two UTF-16 units: 3000 of them are 6000 units.
        let emoji = "\u{1F680}".repeat(3000);
        let parts = split_text(&emoji).unwrap();
        assert_eq!(parts.len(), 2);
        assert!(parts.iter().all(|p| utf16_len(p) <= TEXT_LIMIT));
        assert_eq!(parts.concat(), emoji);

        let too_long = "a".repeat(TEXT_LIMIT * MAX_PARTS + 1);
        assert!(split_text(&too_long).unwrap_err().contains("--file"));
        let just = "a".repeat(TEXT_LIMIT * MAX_PARTS);
        assert_eq!(split_text(&just).unwrap().len(), MAX_PARTS);
    }

    #[test]
    fn digests_tell_messages_apart_by_channel_text_and_files() {
        let a = message_digest("c", "hi", &[]);
        assert_eq!(a, message_digest("c", "hi", &[]));
        assert_ne!(a, message_digest("d", "hi", &[]));
        assert_ne!(a, message_digest("c", "hi", &[sha256_hex(b"x")]));
        // Length-prefixed: moving a boundary is a different message.
        assert_ne!(message_digest("ab", "c", &[]), message_digest("a", "bc", &[]));
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn the_curl_config_keeps_the_token_off_the_command_line_and_quotes_what_it_writes() {
        let config = curl_config(
            "https://api.telegram.org",
            "123:ABC",
            "sendDocument",
            &[
                Field::Text("chat_id", "@my\"chan".into()),
                Field::FromFile("caption", "/tmp/f/text".into()),
                Field::Upload("document", "/tmp/f/file-0".into(), upload_name("Konferenz Radar;26.10.html")),
            ],
        )
        .unwrap();
        assert_eq!(
            config,
            "url = \"https://api.telegram.org/bot123:ABC/sendDocument\"\n\
             form-string = \"chat_id=@my\\\"chan\"\n\
             form = \"caption=</tmp/f/text\"\n\
             form = \"document=@/tmp/f/file-0;filename=Konferenz_Radar_26.10.html\"\n"
        );
        assert!(curl_config("https://x", "12\n3", "sendMessage", &[]).unwrap_err().contains("token"));
        assert!(curl_config("https://x", "1", "sendMessage", &[Field::Text("chat_id", "4\r2".into())]).is_err());
        assert!(curl_config("https://x", "1", "m", &[Field::FromFile("text", "/tmp/a b".into())]).is_err());
        assert_eq!(upload_name("../.."), "file");
        assert_eq!(upload_name("report.html"), "report.html");
    }

    #[test]
    fn the_bot_apis_answer_is_a_message_id_or_telegrams_own_reason() {
        assert_eq!(read_answer(200, r#"{"ok":true,"result":{"message_id":42,"chat":{"id":1}}}"#), Ok(42));
        assert_eq!(
            read_answer(400, r#"{"ok":false,"error_code":400,"description":"Bad Request: chat not found"}"#),
            Err("Telegram refused it (400): Bad Request: chat not found".into())
        );
        assert!(read_answer(429, r#"{"ok":false,"error_code":429,"description":"Too Many Requests","parameters":{"retry_after":7}}"#)
            .unwrap_err()
            .ends_with("try again in 7s"));
        assert!(read_answer(502, "<html>bad gateway</html>").unwrap_err().contains("HTTP 502"));
        assert!(read_answer(200, r#"{"ok":true,"result":true}"#).is_err());
        assert_eq!(redact("url /bot123:SECRET/x failed for 4242", &["123:SECRET", "4242"]), "url /bot<redacted>/x failed for <redacted>");
    }
}
