//! Does a piece of text hold a secret? (`#339`)
//!
//! Scope memory and learned skills are text an agent writes and every later
//! run is handed, so a credential pasted into one would travel to every
//! prompt from then on. Writes are checked here first, against the shapes of
//! every kind the secrets catalogue (`#244`) knows -- a token, an API key, a
//! certificate or SSH private key, a password -- whatever the catalogue
//! happens to declare today, and against each declared entry's own name and
//! environment variable written with a value beside it.
//!
//! Nothing here resolves a catalogue source: a value is never read to be
//! compared against (`secrets.rs`'s rule). And a finding never carries the
//! text it matched -- only which kind of secret it looks like and why --
//! so a refusal can be shown, journaled and logged without repeating it.
use crate::openshell::CredentialSource;
use crate::secrets::{SecretDecl, SecretKind};

/// Something in a text that looks like a secret. Never the text itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretFinding {
    pub kind: SecretKind,
    /// What it looks like, in words: `an Anthropic API key`.
    pub looks_like: String,
}

impl SecretFinding {
    fn new(kind: SecretKind, looks_like: impl Into<String>) -> Self {
        Self { kind, looks_like: looks_like.into() }
    }
}

impl std::fmt::Display for SecretFinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = serde_json::to_value(self.kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        write!(f, "{} (kind {kind})", self.looks_like)
    }
}

/// A well-known prefix and what has to follow it for the whole to be a
/// credential rather than a word that happens to start the same way.
struct Prefix {
    prefix: &'static str,
    /// The characters the rest is made of.
    body: fn(char) -> bool,
    /// At least this many of them after the prefix.
    min: usize,
    kind: SecretKind,
    looks_like: &'static str,
}

fn alnum(c: char) -> bool {
    c.is_ascii_alphanumeric()
}
fn token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}
fn upper_digit(c: char) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit()
}

/// Longest prefix first wherever two share a start (`sk-ant-` before `sk-`).
const PREFIXES: &[Prefix] = &[
    Prefix { prefix: "sk-ant-", body: token_char, min: 20, kind: SecretKind::ApiKey, looks_like: "an Anthropic API key" },
    Prefix { prefix: "sk-proj-", body: token_char, min: 20, kind: SecretKind::ApiKey, looks_like: "an OpenAI API key" },
    Prefix { prefix: "sk-", body: token_char, min: 32, kind: SecretKind::ApiKey, looks_like: "an API secret key" },
    Prefix { prefix: "github_pat_", body: token_char, min: 30, kind: SecretKind::Token, looks_like: "a GitHub token" },
    Prefix { prefix: "ghp_", body: alnum, min: 30, kind: SecretKind::Token, looks_like: "a GitHub token" },
    Prefix { prefix: "gho_", body: alnum, min: 30, kind: SecretKind::Token, looks_like: "a GitHub token" },
    Prefix { prefix: "ghu_", body: alnum, min: 30, kind: SecretKind::Token, looks_like: "a GitHub token" },
    Prefix { prefix: "ghs_", body: alnum, min: 30, kind: SecretKind::Token, looks_like: "a GitHub token" },
    Prefix { prefix: "ghr_", body: alnum, min: 30, kind: SecretKind::Token, looks_like: "a GitHub token" },
    Prefix { prefix: "glpat-", body: token_char, min: 20, kind: SecretKind::Token, looks_like: "a GitLab token" },
    Prefix { prefix: "xoxb-", body: token_char, min: 10, kind: SecretKind::Token, looks_like: "a Slack token" },
    Prefix { prefix: "xoxp-", body: token_char, min: 10, kind: SecretKind::Token, looks_like: "a Slack token" },
    Prefix { prefix: "xoxa-", body: token_char, min: 10, kind: SecretKind::Token, looks_like: "a Slack token" },
    Prefix { prefix: "xoxe-", body: token_char, min: 10, kind: SecretKind::Token, looks_like: "a Slack token" },
    Prefix { prefix: "xoxr-", body: token_char, min: 10, kind: SecretKind::Token, looks_like: "a Slack token" },
    Prefix { prefix: "xoxs-", body: token_char, min: 10, kind: SecretKind::Token, looks_like: "a Slack token" },
    Prefix { prefix: "ya29.", body: token_char, min: 20, kind: SecretKind::Token, looks_like: "a Google OAuth token" },
    Prefix { prefix: "AIza", body: token_char, min: 30, kind: SecretKind::ApiKey, looks_like: "a Google API key" },
    Prefix { prefix: "AKIA", body: upper_digit, min: 16, kind: SecretKind::ApiKey, looks_like: "an AWS access key" },
    Prefix { prefix: "ASIA", body: upper_digit, min: 16, kind: SecretKind::ApiKey, looks_like: "an AWS access key" },
    Prefix { prefix: "sk_live_", body: alnum, min: 16, kind: SecretKind::ApiKey, looks_like: "a Stripe secret key" },
    Prefix { prefix: "rk_live_", body: alnum, min: 16, kind: SecretKind::ApiKey, looks_like: "a Stripe secret key" },
    Prefix { prefix: "sk_test_", body: alnum, min: 16, kind: SecretKind::ApiKey, looks_like: "a Stripe secret key" },
    Prefix { prefix: "npm_", body: alnum, min: 36, kind: SecretKind::Token, looks_like: "an npm token" },
    Prefix { prefix: "pypi-AgE", body: token_char, min: 40, kind: SecretKind::Token, looks_like: "a PyPI token" },
    Prefix { prefix: "hf_", body: alnum, min: 30, kind: SecretKind::Token, looks_like: "a Hugging Face token" },
];

/// Words that, written with a value after `=` or `:`, name a secret.
const ASSIGNED: &[(&str, SecretKind)] = &[
    ("password", SecretKind::Password),
    ("passwd", SecretKind::Password),
    ("passphrase", SecretKind::Password),
    ("pwd", SecretKind::Password),
    ("client_secret", SecretKind::Password),
    ("secret_key", SecretKind::ApiKey),
    ("api_key", SecretKind::ApiKey),
    ("apikey", SecretKind::ApiKey),
    ("api-key", SecretKind::ApiKey),
    ("access_token", SecretKind::Token),
    ("auth_token", SecretKind::Token),
    ("refresh_token", SecretKind::Token),
    ("bot_token", SecretKind::Token),
    ("private_key", SecretKind::Certificate),
];

/// The first thing in `text` that looks like a secret, or `None`.
/// `catalogue` is the instance's `secrets:` block, by reference only.
pub fn find(text: &str, catalogue: &[SecretDecl]) -> Option<SecretFinding> {
    private_key_block(text)
        .or_else(|| prefixed(text))
        .or_else(|| json_web_token(text))
        .or_else(|| telegram_bot_token(text))
        .or_else(|| bearer(text))
        .or_else(|| url_password(text))
        .or_else(|| {
            ASSIGNED.iter().find_map(|(word, kind)| {
                assigned(text, word, 8, true).then(|| SecretFinding::new(*kind, format!("a {word} written with its value")))
            })
        })
        .or_else(|| declared(text, catalogue))
}

/// The byte offsets where `needle` starts in `text` with no word character
/// right before it, so `mask_ghp_...` is not `ghp_...`.
fn starts<'a>(text: &'a str, needle: &'a str, case_insensitive: bool) -> impl Iterator<Item = usize> + 'a {
    let haystack = if case_insensitive { text.to_ascii_lowercase() } else { text.to_string() };
    let needle = if case_insensitive { needle.to_ascii_lowercase() } else { needle.to_string() };
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = haystack[from..].find(&needle) {
        let at = from + at;
        let boundary = text[..at].chars().next_back().is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        if boundary {
            found.push(at);
        }
        from = at + needle.len().max(1);
    }
    found.into_iter()
}

/// How many characters from `at` on satisfy `body`.
fn run_of(text: &str, at: usize, body: fn(char) -> bool) -> usize {
    text[at..].chars().take_while(|c| body(*c)).count()
}

fn private_key_block(text: &str) -> Option<SecretFinding> {
    for at in starts(text, "-----BEGIN ", false) {
        let line = text[at..].lines().next().unwrap_or_default();
        if line.contains("PRIVATE KEY-----") {
            let kind = if line.contains("OPENSSH") { SecretKind::SshKey } else { SecretKind::Certificate };
            return Some(SecretFinding::new(kind, "a private key block"));
        }
    }
    None
}

fn prefixed(text: &str) -> Option<SecretFinding> {
    for rule in PREFIXES {
        for at in starts(text, rule.prefix, false) {
            if run_of(text, at + rule.prefix.len(), rule.body) >= rule.min {
                return Some(SecretFinding::new(rule.kind, rule.looks_like));
            }
        }
    }
    None
}

/// `eyJ<header>.eyJ<payload>.<signature>`: a signed JWT, which is a bearer
/// credential for as long as it has not expired.
fn json_web_token(text: &str) -> Option<SecretFinding> {
    for at in starts(text, "eyJ", false) {
        let rest = &text[at..];
        let mut parts = rest.splitn(3, '.');
        let (Some(header), Some(payload), Some(signature)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let signature_len = signature.chars().take_while(|c| token_char(*c)).count();
        if header.len() >= 10
            && header.chars().all(token_char)
            && payload.starts_with("eyJ")
            && payload.len() >= 10
            && payload.chars().all(token_char)
            && signature_len >= 10
        {
            return Some(SecretFinding::new(SecretKind::Token, "a JSON Web Token"));
        }
    }
    None
}

/// `<8-10 digits>:<35 characters>` -- the shape `@BotFather` hands out.
fn telegram_bot_token(text: &str) -> Option<SecretFinding> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (i, (at, c)) in chars.iter().enumerate() {
        if !c.is_ascii_digit() || text[..*at].chars().next_back().is_some_and(|p| p.is_alphanumeric()) {
            continue;
        }
        let digits = chars[i..].iter().take_while(|(_, c)| c.is_ascii_digit()).count();
        if !(8..=10).contains(&digits) {
            continue;
        }
        let Some((colon, ':')) = chars.get(i + digits).copied() else { continue };
        if run_of(text, colon + 1, token_char) >= 35 {
            return Some(SecretFinding::new(SecretKind::Token, "a Telegram bot token"));
        }
    }
    None
}

fn bearer(text: &str) -> Option<SecretFinding> {
    for at in starts(text, "bearer ", true) {
        let value = at + "bearer ".len();
        let long = text[value..]
            .chars()
            .take_while(|c| token_char(*c) || "./+=~".contains(*c))
            .count();
        if long >= 20 {
            return Some(SecretFinding::new(SecretKind::Token, "a bearer token"));
        }
    }
    None
}

/// `scheme://user:password@host`. A masked password (`***`, `xxx`) or a
/// placeholder (`<password>`, `${VAR}`) is not one.
fn url_password(text: &str) -> Option<SecretFinding> {
    let mut from = 0;
    while let Some(at) = text[from..].find("://") {
        let start = from + at + 3;
        from = start;
        let authority: String = text[start..]
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '/' && *c != '?' && *c != '#')
            .collect();
        let Some((userinfo, _host)) = authority.rsplit_once('@') else { continue };
        let Some((user, password)) = userinfo.split_once(':') else { continue };
        let masked = password.chars().all(|c| matches!(c, '*' | 'x' | 'X' | '.'));
        let placeholder = password.starts_with('<') || password.starts_with("${") || password.starts_with('$');
        if !user.is_empty() && password.chars().count() >= 3 && !masked && !placeholder {
            return Some(SecretFinding::new(SecretKind::Password, "a password in a URL"));
        }
    }
    None
}

/// `word = value` or `word: value`, the word on its own (any case), the
/// value at least `min` characters with no space in it. `mixed` asks for a
/// letter and a digit in the value too, so `password: ask the owner` and
/// `api_key: rotated monthly` stay what they are -- notes about a secret.
fn assigned(text: &str, word: &str, min: usize, mixed: bool) -> bool {
    for at in starts(text, word, true) {
        let after = &text[at + word.len()..];
        // The word itself must end here: `password_hint` is another word.
        if after.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-') {
            continue;
        }
        let after = after.trim_start_matches(['"', '\'']).trim_start_matches([' ', '\t']);
        let Some(after) = after.strip_prefix('=').or_else(|| after.strip_prefix(':')) else { continue };
        let value: String = after
            .trim_start_matches([' ', '\t', '"', '\''])
            .chars()
            .take_while(|c| !c.is_whitespace() && !matches!(c, '"' | '\'' | ',' | ';' | ')' | ']' | '}'))
            .collect();
        let placeholder = value.starts_with('<') || value.starts_with("${") || value.starts_with('$')
            || value.chars().all(|c| matches!(c, '*' | 'x' | 'X' | '.'));
        let letters = value.chars().any(|c| c.is_alphabetic());
        let digits = value.chars().any(|c| c.is_ascii_digit());
        if value.chars().count() >= min && !placeholder && (!mixed || (letters && digits)) {
            return true;
        }
    }
    false
}

/// The catalogue's own entries: an entry's name, or an `env` source's
/// variable, written with a value -- `claude-oauth-token: ...`,
/// `ANTHROPIC_API_KEY=...`. Only the declaration is read, never the source.
fn declared(text: &str, catalogue: &[SecretDecl]) -> Option<SecretFinding> {
    for secret in catalogue {
        if assigned(text, &secret.name, 8, false) {
            return Some(SecretFinding::new(secret.kind, format!("a value for the declared secret {}", secret.name)));
        }
        if let CredentialSource::Env { name } = &secret.source {
            if assigned(text, name, 4, false) {
                return Some(SecretFinding::new(
                    secret.kind,
                    format!("a value for {name}, the declared secret {}", secret.name),
                ));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalogue() -> Vec<SecretDecl> {
        serde_yaml_ng::from_str(
            "- name: claude-oauth-token\n  kind: token\n  source: { from: file, path: ~/.config/factory/secrets/claude }\n\
             - name: openai\n  kind: api-key\n  source: { from: env, name: OPENAI_API_KEY }\n",
        )
        .unwrap()
    }

    fn looks(text: &str) -> Option<String> {
        find(text, &catalogue()).map(|f| f.looks_like)
    }

    #[test]
    fn every_well_known_shape_is_found_and_named_without_its_text() {
        let cases = [
            ("key sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWxYz012345", "an Anthropic API key"),
            ("export OPENAI=sk-proj-AbCdEfGhIjKlMnOpQrStUvWx", "an OpenAI API key"),
            ("token ghp_0123456789abcdefghijABCDEFGHIJklmnop", "a GitHub token"),
            ("github_pat_11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyz", "a GitHub token"),
            ("aws AKIAIOSFODNN7EXAMPLE", "an AWS access key"),
            ("slack xoxb-123456789012-abcdefghij", "a Slack token"),
            ("AIzaSyD-0123456789abcdefghijklmnopqrstu", "a Google API key"),
            ("-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk=\n", "a private key block"),
            ("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N", "a JSON Web Token"),
            ("bot 123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsawQ", "a Telegram bot token"),
            ("curl -H 'Authorization: Bearer abcdefghijklmnopqrstuvwxyz012345'", "a bearer token"),
            ("clone https://ingo:hunter2pass@git.example.com/repo", "a password in a URL"),
            ("db password=s3cretValue9", "a password written with its value"),
            ("claude-oauth-token: abcdefgh12345678", "a value for the declared secret claude-oauth-token"),
            ("OPENAI_API_KEY=whatever-it-is", "a value for OPENAI_API_KEY, the declared secret openai"),
        ];
        for (text, expected) in cases {
            let finding = find(text, &catalogue()).unwrap_or_else(|| panic!("nothing found in {text:?}"));
            assert_eq!(finding.looks_like, expected, "{text:?}");
            assert!(!finding.to_string().contains(&text[text.len() - 6..]), "a finding never repeats the text");
        }
        assert_eq!(find("-----BEGIN RSA PRIVATE KEY-----", &[]).unwrap().kind, SecretKind::Certificate);
        assert_eq!(find("-----BEGIN OPENSSH PRIVATE KEY-----", &[]).unwrap().kind, SecretKind::SshKey);
    }

    #[test]
    fn notes_about_secrets_and_ordinary_text_are_not_secrets() {
        for text in [
            "The GitHub token lives in the secrets catalogue as github-gh-login; renew with gh auth login.",
            "password: ask the owner",
            "api_key: rotated monthly",
            "The password_hint field is optional",
            "commit 3eaec55f1c8a1b0d4e2f6a7b8c9d0e1f2a3b4c5d merged",
            "the deadline is 2026-11-30 and the CFP closes at 23:59",
            "https://user@example.com/path and https://example.com:8443/x",
            "mask_ghp_ is a prefix, not a token",
            "sk-short",
            "password=${DB_PASSWORD}",
            "postgres://app:***@db/prod",
            "Telegram chat 123456789 replied at 10:30",
            "claude-oauth-token expires 2027-10-04",
        ] {
            assert_eq!(looks(text), None, "{text:?}");
        }
    }
}
