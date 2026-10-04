//! The declared secrets catalogue (`#244`): the instance root's `secrets:`
//! block. Each entry says *where* a secret lives and *what it is for* --
//! never what it is. Values never enter Factory's config, journal, logs,
//! API answers or pages; a source is read only by the provisioner, only to
//! hand the value to the one child that needs it, and only to learn whether
//! it resolves.
//!
//! ```yaml
//! secrets:
//!   - name: claude-oauth-token
//!     kind: token
//!     source: { from: file, path: ~/.config/factory/secrets/claude-oauth-token }
//!     expires: 2027-10-04
//!     renew: "claude setup-token, then (umask 077; cat > ~/.config/factory/secrets/claude-oauth-token)"
//!   - name: github-gh-login
//!     kind: token
//!     source: { from: command, run: "gh auth token" }
//!     expires: never
//!     renew: "gh auth login"
//! ```
//!
//! An OpenShell provider names an entry with `credential: { secret: <name> }`
//! (`openshell::ProviderCredential`). The only thing Factory ever writes
//! here is an entry's metadata -- `expires`, `renew` and `note` -- from the
//! L2 Secrets tab, journaled.

use crate::error::{FactoryError, Result};
use crate::openshell::CredentialSource;
use chrono::NaiveDate;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeSet;

/// How far ahead a declared expiry is `due soon` and raised in the Inbox.
pub const DUE_SOON_DAYS: i64 = crate::openshell::EXPIRY_WARNING_DAYS;
/// The second, louder warning.
pub const URGENT_DAYS: i64 = 7;

/// What sort of secret an entry is. Shown, never acted on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SecretKind {
    Token,
    ApiKey,
    Certificate,
    SshKey,
    Password,
    Other,
}

/// When a secret stops working: a date, or `never`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Expiry {
    Never,
    On(NaiveDate),
}

impl Expiry {
    pub fn date(self) -> Option<NaiveDate> {
        match self {
            Self::Never => None,
            Self::On(date) => Some(date),
        }
    }
}

impl std::fmt::Display for Expiry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Never => f.write_str("never"),
            Self::On(date) => write!(f, "{}", date.format("%Y-%m-%d")),
        }
    }
}

impl std::str::FromStr for Expiry {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, String> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("never") {
            return Ok(Self::Never);
        }
        NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map(Self::On)
            .map_err(|_| format!("expires is a date (YYYY-MM-DD) or never, not {s:?}"))
    }
}

impl Serialize for Expiry {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Expiry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// One `secrets:` entry. Strict: a misspelt key is refused by name, never
/// read as nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretDecl {
    /// Unique in the catalogue. What a provider's `{ secret: <name> }` names.
    pub name: String,
    pub kind: SecretKind,
    /// Where the value is read from: the same kinds an OpenShell provider's
    /// inline `credential:` takes (`#234`). By reference, never by value.
    pub source: CredentialSource,
    /// Absent is `unknown`: nobody has said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<Expiry>,
    /// How to renew it: one line, often a command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renew: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The three fields the Secrets tab may change, and the only thing Factory
/// ever writes into a `secrets:` entry. A field left `None` is removed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretMetadata {
    #[serde(default)]
    pub expires: Option<Expiry>,
    #[serde(default)]
    pub renew: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

impl SecretMetadata {
    /// Blank text is no text: an emptied field is removed, not written as `""`.
    pub fn normalized(self) -> Self {
        let clean = |s: Option<String>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        Self { expires: self.expires, renew: clean(self.renew), note: clean(self.note) }
    }

    /// Refuse what cannot be written as one plain line.
    pub fn validate(&self) -> Result<()> {
        for (field, value) in [("renew", &self.renew), ("note", &self.note)] {
            if let Some(value) = value {
                if value.contains(['\n', '\r']) {
                    return Err(FactoryError::BadRequest(format!("a secret's {field} is one line")));
                }
                if value.chars().count() > 500 {
                    return Err(FactoryError::BadRequest(format!("a secret's {field} is at most 500 characters")));
                }
            }
        }
        Ok(())
    }
}

impl SecretDecl {
    pub fn metadata(&self) -> SecretMetadata {
        SecretMetadata { expires: self.expires, renew: self.renew.clone(), note: self.note.clone() }
    }

    pub fn expiry(&self, today: NaiveDate) -> (ExpiryState, Option<i64>) {
        expiry_state(self.expires, today)
    }
}

/// Where a declared expiry stands today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpiryState {
    Ok,
    DueSoon,
    Expired,
    Never,
    Unknown,
}

impl ExpiryState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::DueSoon => "due soon",
            Self::Expired => "expired",
            Self::Never => "never",
            Self::Unknown => "unknown",
        }
    }
}

/// The state, and the days left for a date (negative once it has passed).
/// The expiry day itself is `due soon` with 0 days left; the day after is
/// `expired`.
pub fn expiry_state(expires: Option<Expiry>, today: NaiveDate) -> (ExpiryState, Option<i64>) {
    match expires {
        None => (ExpiryState::Unknown, None),
        Some(Expiry::Never) => (ExpiryState::Never, None),
        Some(Expiry::On(date)) => {
            let left = (date - today).num_days();
            let state = if left < 0 {
                ExpiryState::Expired
            } else if left <= DUE_SOON_DAYS {
                ExpiryState::DueSoon
            } else {
                ExpiryState::Ok
            };
            (state, Some(left))
        }
    }
}

/// The Inbox stage a date is in: the day it opened (30 days ahead, 7 days
/// ahead, the expiry day itself) -- so a dismissed item comes back, as a
/// new one, at the next stage. `None` outside the warning window.
pub fn warning_stage(expires: NaiveDate, today: NaiveDate) -> Option<NaiveDate> {
    let left = (expires - today).num_days();
    if left > DUE_SOON_DAYS {
        None
    } else if left > URGENT_DAYS {
        Some(expires - chrono::Duration::days(DUE_SOON_DAYS))
    } else if left > 0 {
        Some(expires - chrono::Duration::days(URGENT_DAYS))
    } else {
        Some(expires)
    }
}

fn is_secret_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name.len() <= 63
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The whole catalogue, checked at load: unique names, sources that can
/// work, one-line metadata.
pub fn validate(secrets: &[SecretDecl]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for secret in secrets {
        let refuse = |what: String| Err(FactoryError::BadRequest(format!("secrets: {what}")));
        if !is_secret_name(&secret.name) {
            return refuse(format!(
                "{:?} is not a secret name (letters, digits, '-', '_' and '.', at most 63)",
                secret.name
            ));
        }
        if !seen.insert(secret.name.as_str()) {
            return refuse(format!("declares {:?} twice", secret.name));
        }
        if let Some(problem) = secret.source.problems(&format!("the secret {:?}", secret.name)).into_iter().next() {
            return refuse(problem);
        }
        secret
            .metadata()
            .validate()
            .map_err(|e| FactoryError::BadRequest(format!("secrets: {:?}: {e}", secret.name)))?;
    }
    Ok(())
}

pub fn find<'a>(secrets: &'a [SecretDecl], name: &str) -> Option<&'a SecretDecl> {
    secrets.iter().find(|s| s.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CATALOGUE: &str = "\
- name: claude-oauth-token
  kind: token
  source: { from: file, path: ~/.config/factory/secrets/claude-oauth-token }
  expires: 2027-10-04
  renew: \"claude setup-token, then (umask 077; cat > ~/.config/factory/secrets/claude-oauth-token)\"
- name: github-gh-login
  kind: token
  source: { from: command, run: \"gh auth token\" }
  expires: never
  renew: \"gh auth login\"
";

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn the_issues_catalogue_parses_and_round_trips() {
        let c: Vec<SecretDecl> = serde_yaml_ng::from_str(CATALOGUE).unwrap();
        validate(&c).unwrap();
        assert_eq!(c[0].expires, Some(Expiry::On(day(2027, 10, 4))));
        assert_eq!(c[0].source, CredentialSource::File { path: "~/.config/factory/secrets/claude-oauth-token".into() });
        assert_eq!(c[1].expires, Some(Expiry::Never));
        assert_eq!(c[1].kind, SecretKind::Token);
        let again: Vec<SecretDecl> = serde_yaml_ng::from_str(&serde_yaml_ng::to_string(&c).unwrap()).unwrap();
        assert_eq!(again, c);
        assert_eq!(serde_json::to_value(c[0].expires).unwrap(), "2027-10-04");
    }

    #[test]
    fn unknown_keys_kinds_and_bad_dates_are_refused_by_name() {
        for (bad, expected) in [
            ("- { name: a, kind: token, source: { from: env, name: A }, expire: never }", "expire"),
            ("- { name: a, kind: tokn, source: { from: env, name: A } }", "tokn"),
            ("- { name: a, kind: token, source: { from: env, name: A }, expires: next-year }", "next-year"),
            ("- { name: a, kind: token, source: { from: env, name: A }, value: hunter2 }", "value"),
            ("- { name: a, kind: token, source: { from: env, name: A, value: x } }", "value"),
        ] {
            let e = serde_yaml_ng::from_str::<Vec<SecretDecl>>(bad).unwrap_err().to_string();
            assert!(e.contains(expected), "{bad}: {e}");
        }
    }

    #[test]
    fn a_catalogue_that_cannot_work_is_refused() {
        let parse = |y: &str| serde_yaml_ng::from_str::<Vec<SecretDecl>>(y).unwrap();
        for (yaml, expected) in [
            ("[{ name: a, kind: token, source: { from: env, name: A } }, { name: a, kind: token, source: { from: env, name: B } }]", "twice"),
            ("[{ name: 'a b', kind: token, source: { from: env, name: A } }]", "not a secret name"),
            ("[{ name: a, kind: token, source: { from: file, path: rel/x } }]", "not an absolute path"),
            ("[{ name: a, kind: token, source: { from: env, name: A }, renew: \"one\\ntwo\" }]", "one line"),
        ] {
            let e = validate(&parse(yaml)).unwrap_err().to_string();
            assert!(e.contains(expected), "{yaml}: {e}");
        }
    }

    #[test]
    fn expiry_states_and_inbox_stages() {
        let today = day(2026, 10, 4);
        assert_eq!(expiry_state(Some(Expiry::On(day(2027, 10, 4))), today), (ExpiryState::Ok, Some(365)));
        assert_eq!(expiry_state(Some(Expiry::On(day(2026, 11, 3))), today), (ExpiryState::DueSoon, Some(30)));
        assert_eq!(expiry_state(Some(Expiry::On(today)), today), (ExpiryState::DueSoon, Some(0)));
        assert_eq!(expiry_state(Some(Expiry::On(day(2026, 10, 3))), today), (ExpiryState::Expired, Some(-1)));
        assert_eq!(expiry_state(Some(Expiry::Never), today), (ExpiryState::Never, None));
        assert_eq!(expiry_state(None, today), (ExpiryState::Unknown, None));

        let expires = day(2026, 12, 1);
        assert_eq!(warning_stage(expires, day(2026, 10, 31)), None);
        assert_eq!(warning_stage(expires, day(2026, 11, 1)), Some(day(2026, 11, 1)));
        assert_eq!(warning_stage(expires, day(2026, 11, 23)), Some(day(2026, 11, 1)));
        assert_eq!(warning_stage(expires, day(2026, 11, 24)), Some(day(2026, 11, 24)), "seven days left");
        assert_eq!(warning_stage(expires, day(2026, 11, 30)), Some(day(2026, 11, 24)));
        assert_eq!(warning_stage(expires, expires), Some(expires));
        assert_eq!(warning_stage(expires, day(2027, 1, 1)), Some(expires));
    }

    #[test]
    fn metadata_is_normalized_and_one_line() {
        let m = SecretMetadata { expires: None, renew: Some("  ".into()), note: Some(" hi ".into()) }.normalized();
        assert_eq!(m.renew, None);
        assert_eq!(m.note.as_deref(), Some("hi"));
        assert!(SecretMetadata { note: Some("a\rb".into()), ..Default::default() }.validate().is_err());
        assert_eq!("NEVER".parse::<Expiry>(), Ok(Expiry::Never));
        assert!("2027-13-01".parse::<Expiry>().is_err());
    }
}
