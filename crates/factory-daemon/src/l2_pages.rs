//! Pages that compose L2's catalogues with L4's runs and journals (#193 phase 6, S12 part 6).
//!
//! **Page (D5).** Both started life as L2 code reaching up into L4's store, which a level may not do (facts flow up;
//! an L2 file has nothing below it to read an L4 record through). They are compositions, so they live here, owned by
//! the entry point and counted by the page ratchet:
//! - `attach_dependency`: an agent attaches an SBOM or VEX to its run. L4's run and token decide whether it may; L2's
//!   inventory writes the document; the task's journal records it.
//! - `secrets_view` / `secret_changes` / `set_secret`: the secrets catalogue (L2) with its change history, which is
//!   kept as entries in L4's journal under a reserved id, and the bus event that tells the Important Dates ledger.
use crate::access::Caller;
use crate::engine::Engine;
use crate::secrets::{change_words, rows, CHANGES_SHOWN, SECRETS_JOURNAL, SECRET_CHANGED};
use chrono::Utc;
use factory_core::dependencies::{validate_document, Attachment, AttachmentKind};
use factory_core::error::{FactoryError, Result};
use factory_core::protocol::{SecretChange, SecretRow, UndeclaredCredential};
use factory_core::secrets::SecretMetadata;
use factory_core::task::TaskEntry;
use factory_environment::dependency_inventory::write_attachment;

impl Engine {
    pub(crate) async fn attach_dependency(
        &self,
        task_id: &str,
        kind: AttachmentKind,
        bytes: Vec<u8>,
        token: Option<&str>,
    ) -> Result<Attachment> {
        let run = self.l4.store.active_run(task_id).await?.ok_or_else(|| {
            FactoryError::BadRequest(format!(
                "task {task_id} has no run in progress; attachments are no longer accepted"
            ))
        })?;
        self.check_run_token(&run, token, task_id)?;
        let task = self
            .l4.store
            .get(task_id)
            .await?
            .ok_or_else(|| FactoryError::BadRequest(format!("no such task: {task_id}")))?;
        let validated = validate_document(&bytes, kind).map_err(FactoryError::BadRequest)?;
        let id = uuid::Uuid::new_v4().to_string();
        let attached_at = Utc::now();
        let filename = format!(
            "{}-{}-{id}.cdx.json",
            attached_at.format("%Y%m%dT%H%M%S%.6fZ"),
            kind.as_str()
        );
        let attachment = Attachment {
            id,
            kind,
            scope: task.scope.clone(),
            run_id: run.id.clone(),
            task_id: task.id.clone(),
            attempt: run.attempt,
            attached_at,
            filename,
            spec_version: validated.spec_version,
            states: validated.states,
        };
        let root = self.factory_snapshot().root;
        let to_write = attachment.clone();
        tokio::task::spawn_blocking(move || write_attachment(&root, &to_write, &bytes))
            .await
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("attachment writer: {e}")))?
            .map_err(FactoryError::BadRequest)?;
        self.entry(
            task_id,
            TaskEntry::new(
                "agent",
                "attachment",
                format!("attached {} as {}", attachment.filename, attachment.kind),
            )
            .in_run(&run.id),
        )
        .await;
        Ok(attachment)
    }

    /// `Request::Environment`'s catalogue half.
    pub(crate) async fn secrets_view(&self) -> (Vec<SecretRow>, Vec<UndeclaredCredential>, Vec<SecretChange>) {
        let factory = self.factory_snapshot();
        let (secrets, undeclared) = rows(&factory, &self.l2.provision.source_checks(), Utc::now().date_naive());
        (secrets, undeclared, self.secret_changes().await)
    }

    /// The newest metadata changes, oldest first.
    async fn secret_changes(&self) -> Vec<SecretChange> {
        let entries = self.l4.store.entries(SECRETS_JOURNAL, CHANGES_SHOWN).await.unwrap_or_default();
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
            if let Err(e) = self.l4.store.append_entry(SECRETS_JOURNAL, &entry).await {
                tracing::warn!(secret = %name, "the secret's metadata was written but not journaled: {e}");
            }
            tracing::info!(secret = %name, "secret metadata changed: {}", change_words(&before, &metadata));
            // The ledger, its Inbox items and its push hook read the new date now.
            self.shared.bus.publish(factory_core::event::Event::ImportantDatesUpdated { at: Utc::now() });
        }
        let (rows, _, _) = self.secrets_view().await;
        rows.into_iter()
            .find(|r| r.name == name)
            .ok_or_else(|| factory_core::error::FactoryError::BadRequest(format!("no secret {name:?} is declared")))
    }
}
