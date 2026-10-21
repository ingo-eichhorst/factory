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
//! Outbound only. Nothing here calls `getUpdates`: one bot token can have
//! one poller, and that belongs to the inbound gateway (`#338`).
//!
//! Everything in this module is pure: the declaration and its checks, how a
//! text is cut into Telegram messages, the `curl` config a send is made
//! with, and how the Bot API's answer is read. The send itself -- reading the
//! token, running `curl`, the repeat ledger -- is the daemon's L2 code.

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
}

impl ChannelDecl {
    /// The Bot API base, without a trailing slash.
    pub fn api_base(&self) -> &str {
        self.api.as_deref().unwrap_or(TELEGRAM_API).trim_end_matches('/')
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
}

impl std::fmt::Debug for Outgoing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Outgoing")
            .field("channel", &self.channel)
            .field("text", &self.text.len())
            .field("files", &self.files)
            .field("run", &self.run)
            .finish()
    }
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
