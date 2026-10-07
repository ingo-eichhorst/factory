//! The declared secrets catalogue (`#244`), as the L2 Secrets tab and the
//! Inbox read it, and the one write into it.
//!
//! What is read: the instance root's `secrets:` (metadata and *where* each
//! secret lives), whether a `file` source is there and owner-only (its
//! metadata, never its content), and whether each source resolved at the
//! provisioner's last pass -- a yes/no and a value-free reason. A page load
//! never runs a credential command.
//!
//! What is written: an entry's `expires`, `renew` and `note`, into the
//! instance root's config (`Engine::write_secret_metadata`), journaled with
//! who changed what. Nothing else, and never a value.
//!
//! Each entry is also an Important dates observation (`ledger_observations`),
//! which is how a declared expiry reaches the Inbox (`#236`).

use chrono::NaiveDate;
#[cfg(test)]
use chrono::Utc;
use factory_core::config::Factory;
use factory_core::openshell::{self as os, CredentialSource, ProviderCredential};
#[cfg(test)]
use crate::engine::Engine;
#[cfg(test)]
use factory_core::protocol::SecretChange;
use factory_core::protocol::{SecretRow, SecretUse, UndeclaredCredential};
use factory_core::secrets::{expiry_state, Expiry, SecretMetadata};
use std::collections::BTreeMap;

/// The journal a metadata change is recorded in. Not a task: the journal is
/// keyed by a task id and has no foreign key, so the catalogue keeps its own
/// line of entries under this fixed id.
pub(crate) const SECRETS_JOURNAL: &str = "factory:secrets";
/// The entry kind of one metadata change.
pub(crate) const SECRET_CHANGED: &str = "secret_changed";
/// How many changes the tab shows.
pub(crate) const CHANGES_SHOWN: u32 = 20;

/// `file`, `command`, `env` or `keychain`.
pub(crate) fn source_kind(source: &CredentialSource) -> &'static str {
    match source {
        CredentialSource::Env { .. } => "env",
        CredentialSource::File { .. } => "file",
        CredentialSource::Command { .. } => "command",
        CredentialSource::Keychain { .. } => "keychain",
    }
}

/// For a file source: whether it is there, and whether it is a regular file
/// of the daemon's user that nobody else can read or write. Its metadata
/// only; the file is never opened here.
fn file_presence(source: &CredentialSource) -> (Option<bool>, Option<bool>) {
    use std::os::unix::fs::MetadataExt;
    let CredentialSource::File { path } = source else { return (None, None) };
    match std::fs::metadata(crate::provision::expand_home(path)) {
        Err(_) => (Some(false), None),
        Ok(meta) => {
            let owner_only =
                meta.is_file() && meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o077 == 0;
            (Some(true), Some(owner_only))
        }
    }
}

/// Every managed provider, canonically derived inside L2 from declarations.
fn managed_providers(factory: &Factory) -> Vec<(String, String, String, os::ManagedProvider)> {
    factory_environment::credential_expiry::managed_providers(
        &factory.config.instance.id, &crate::facts::environment_provider_declarations(factory),
    )
}
/// Who names each secret: secret name -> its users.
pub(crate) fn users(factory: &Factory) -> BTreeMap<String, Vec<SecretUse>> {
    factory_environment::credential_expiry::users(
        &factory.config.instance.id, &crate::facts::environment_provider_declarations(factory),
    )
}

/// The tab's rows: the catalogue, and every inline credential still
/// written in a scope.
pub(crate) fn rows(
    factory: &Factory,
    checks: &BTreeMap<String, crate::provision::SourceCheck>,
    today: NaiveDate,
) -> (Vec<SecretRow>, Vec<UndeclaredCredential>) {
    let mut used = users(factory);
    let secrets = factory
        .config
        .secrets
        .iter()
        .map(|secret| {
            let (present, owner_only) = file_presence(&secret.source);
            let check = checks.get(&secret.name);
            let (state, days_left) = secret.expiry(today);
            SecretRow {
                name: secret.name.clone(),
                kind: secret.kind,
                from: source_kind(&secret.source).to_string(),
                source: secret.source.describe(),
                present,
                owner_only,
                resolves: check.map(|c| c.resolves),
                reason: check.and_then(|c| c.reason.clone()),
                checked_at: check.map(|c| c.checked_at),
                expires: secret.expires,
                days_left,
                state,
                renew: secret.renew.clone(),
                note: secret.note.clone(),
                used_by: used.remove(&secret.name).unwrap_or_default(),
            }
        })
        .collect();
    let undeclared = managed_providers(factory)
        .into_iter()
        .filter_map(|(scope, agent, provider, managed)| {
            let ProviderCredential::Source(source) = &managed.credential else { return None };
            let (state, days_left) = expiry_state(managed.expires.map(Expiry::On), today);
            Some(UndeclaredCredential {
                scope,
                agent,
                provider,
                from: source_kind(source).to_string(),
                source: source.describe(),
                expires: managed.expires,
                days_left,
                state,
            })
        })
        .collect();
    (secrets, undeclared)
}

/// Live authored secret metadata, canonically derived in L2.
#[cfg(test)]
pub(crate) fn ledger_observations(
    factory: &Factory,
    now: chrono::DateTime<Utc>,
) -> Vec<factory_core::renewals::ExpiryObservation> {
    factory_environment::credential_expiry::ledger_observations(
        &factory.config.instance.id, &factory.config.secrets,
        &crate::facts::environment_provider_declarations(factory), now,
    )
}

/// `"2027-10-04"`, `"never"`, or `"unset"`.
fn shown(expires: Option<Expiry>) -> String {
    expires.map(|e| e.to_string()).unwrap_or_else(|| "unset".into())
}

/// The journal line for a change: what moved, field by field. Metadata
/// only -- there is nothing else in a `SecretMetadata`.
pub(crate) fn change_words(before: &SecretMetadata, after: &SecretMetadata) -> String {
    let mut parts = Vec::new();
    if before.expires != after.expires {
        parts.push(format!("expires {} -> {}", shown(before.expires), shown(after.expires)));
    }
    for (field, b, a) in [("renew", &before.renew, &after.renew), ("note", &before.note, &after.note)] {
        match (b, a) {
            (x, y) if x == y => {}
            (_, None) => parts.push(format!("{field} removed")),
            (_, Some(text)) => parts.push(format!("{field} set to {text:?}")),
        }
    }
    parts.join("; ")
}

#[cfg(test)]
mod tests;
