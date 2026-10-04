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

use crate::access::Caller;
use crate::engine::Engine;
use chrono::{NaiveDate, Utc};
use factory_core::config::Factory;
use factory_core::error::Result;
use factory_core::openshell::{self as os, CredentialSource, ProviderCredential, ProviderDecl};
use factory_core::protocol::{SecretChange, SecretRow, SecretUse, UndeclaredCredential};
use factory_core::secrets::{expiry_state, Expiry, SecretMetadata};
use factory_core::task::TaskEntry;
use std::collections::BTreeMap;

/// The journal a metadata change is recorded in. Not a task: the journal is
/// keyed by a task id and has no foreign key, so the catalogue keeps its own
/// line of entries under this fixed id.
pub(crate) const SECRETS_JOURNAL: &str = "factory:secrets";
/// The entry kind of one metadata change.
pub(crate) const SECRET_CHANGED: &str = "secret_changed";
/// How many changes the tab shows.
const CHANGES_SHOWN: u32 = 20;

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

/// Every managed provider in every scope, with what its credential says:
/// `(scope, agent, provider on the gateway, provider, credential)`.
fn managed_providers(factory: &Factory) -> Vec<(String, String, String, os::ManagedProvider)> {
    let suffix = os::instance_suffix(&factory.config.instance.id);
    let mut out = Vec::new();
    for name in factory.scope_names() {
        let Ok(scope) = factory.scope(&name) else { continue };
        for agent in scope.declared_agents() {
            let Some(block) = &agent.openshell else { continue };
            for provider in &block.providers {
                if let ProviderDecl::Managed(managed) = provider {
                    out.push((scope.name.clone(), agent.name(), provider.gateway_name(&suffix), managed.clone()));
                }
            }
        }
    }
    out
}

/// Who names each secret: secret name -> its users.
pub(crate) fn users(factory: &Factory) -> BTreeMap<String, Vec<SecretUse>> {
    let mut out: BTreeMap<String, Vec<SecretUse>> = BTreeMap::new();
    for (scope, agent, provider, managed) in managed_providers(factory) {
        if let ProviderCredential::Secret(name) = &managed.credential {
            out.entry(name.clone()).or_default().push(SecretUse { scope, agent, provider });
        }
    }
    out
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

/// One Important dates observation per declared secret (`#244`): the
/// renewals ledger (`#236`) reads the catalogue rather than keeping a copy,
/// and its milestones -- the 30-day lead, 7 days, 1 day, the day itself --
/// are the secret's Inbox items, one per secret, naming every scope, agent
/// and provider that uses it. Built from the live snapshot on every read, so
/// an edited date counts at once.
pub(crate) fn ledger_observations(
    factory: &Factory,
    now: chrono::DateTime<Utc>,
) -> Vec<factory_core::renewals::ExpiryObservation> {
    use factory_core::renewals::{DateBasis, DateDependency, DateKind, DateSource};
    let mut used = users(factory);
    factory
        .config
        .secrets
        .iter()
        .map(|secret| {
            let mut item = crate::renewals::probes::observation(
                format!("secret:{}", secret.name),
                format!("Secret {}", secret.name),
                DateKind::Credential,
                DateSource::Secret,
                now,
            );
            match secret.expires {
                Some(Expiry::On(date)) => {
                    item.expires_at = date.and_hms_opt(0, 0, 0).map(|midnight| midnight.and_utc());
                    item.basis = DateBasis::Declared;
                    item.detail = "declared in the instance root's secrets:".into();
                }
                Some(Expiry::Never) => {
                    item.no_expiry = true;
                    item.basis = DateBasis::Declared;
                    item.detail = "declared never to expire in the instance root's secrets:".into();
                }
                None => item.detail = "the instance root's secrets: gives no expires: for it".into(),
            }
            if let Some(renew) = &secret.renew {
                item.renew = renew.clone();
            }
            item.affects = used
                .remove(&secret.name)
                .unwrap_or_default()
                .into_iter()
                .map(|u| DateDependency {
                    label: format!("{} / {} / {}", u.scope, u.agent, u.provider),
                    scope: Some(u.scope),
                    agent: Some(u.agent),
                    environment: None,
                    provider: Some(u.provider),
                })
                .collect();
            item
        })
        .collect()
}

/// `"2027-10-04"`, `"never"`, or `"unset"`.
fn shown(expires: Option<Expiry>) -> String {
    expires.map(|e| e.to_string()).unwrap_or_else(|| "unset".into())
}

/// The journal line for a change: what moved, field by field. Metadata
/// only -- there is nothing else in a `SecretMetadata`.
fn change_words(before: &SecretMetadata, after: &SecretMetadata) -> String {
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

impl Engine {
    /// `Request::Environment`'s catalogue half.
    pub(crate) async fn secrets_view(&self) -> (Vec<SecretRow>, Vec<UndeclaredCredential>, Vec<SecretChange>) {
        let factory = self.factory_snapshot();
        let (secrets, undeclared) = rows(&factory, &self.provision.source_checks(), Utc::now().date_naive());
        (secrets, undeclared, self.secret_changes().await)
    }

    /// The newest metadata changes, oldest first.
    async fn secret_changes(&self) -> Vec<SecretChange> {
        let entries = self.store.entries(SECRETS_JOURNAL, CHANGES_SHOWN).await.unwrap_or_default();
        entries
            .into_iter()
            .filter(|e| e.kind == SECRET_CHANGED)
            .map(|e| {
                let data = e.data.clone().unwrap_or_default();
                SecretChange {
                    at: e.at,
                    secret: data.get("secret").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                    by: data.get("by").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                    message: e.message,
                }
            })
            .collect()
    }

    /// `Request::SecretSet`: write the entry's metadata, journal who changed
    /// what, and answer the row as it now stands. An unchanged write writes
    /// and journals nothing.
    pub(crate) async fn set_secret(&self, caller: &Caller, name: &str, metadata: SecretMetadata) -> Result<SecretRow> {
        let metadata = metadata.normalized();
        metadata.validate()?;
        let (before, changed) = self.write_secret_metadata(name, &metadata)?;
        if changed {
            let asked = crate::operations::Asked::new(caller, None);
            let message = format!("secret {name}: {} {}", change_words(&before, &metadata), asked.words());
            let entry: TaskEntry = asked.entry(
                SECRET_CHANGED,
                message,
                serde_json::json!({ "secret": name, "before": before, "after": metadata }),
            );
            if let Err(e) = self.store.append_entry(SECRETS_JOURNAL, &entry).await {
                tracing::warn!(secret = %name, "the secret's metadata was written but not journaled: {e}");
            }
            tracing::info!(secret = %name, "secret metadata changed: {}", change_words(&before, &metadata));
            // The ledger, its Inbox items and its push hook read the new date now.
            self.bus.publish(factory_core::event::Event::ImportantDatesUpdated { at: Utc::now() });
        }
        let (rows, _, _) = self.secrets_view().await;
        rows.into_iter()
            .find(|r| r.name == name)
            .ok_or_else(|| factory_core::error::FactoryError::BadRequest(format!("no secret {name:?} is declared")))
    }
}

#[cfg(test)]
mod tests;
