//! Canonical L2 dependency inventory; authorization/journaling of attachment
//! commands stays at the router until adjacent command ports are isolated.
use crate::engine::Engine;
use chrono::Utc;
use factory_core::dependencies::{
    validate_document, Attachment, AttachmentKind, DependenciesReport,
};
use factory_core::error::{FactoryError, Result};
use factory_core::task::TaskEntry;
use factory_environment::dependency_inventory::write_attachment;
use serde_json::Value;
impl Engine {
    pub(crate) async fn dependency_document(
        &self,
        scope: &str,
        id: &str,
    ) -> Result<(Attachment, Value)> {
        crate::facts::environment_dependencies(self)
            .dependency_document(scope, id)
            .await
    }
    pub(crate) async fn dependencies_report(&self, scope: &str) -> Result<DependenciesReport> {
        crate::facts::environment_dependencies(self)
            .dependencies_report(scope)
            .await
    }
    pub(crate) async fn dependencies_vex(&self, scope: &str) -> Result<String> {
        crate::facts::environment_dependencies(self)
            .dependencies_vex(scope)
            .await
    }
    pub(crate) async fn attach_dependency(
        &self,
        task_id: &str,
        kind: AttachmentKind,
        bytes: Vec<u8>,
        token: Option<&str>,
    ) -> Result<Attachment> {
        let run = self.store.active_run(task_id).await?.ok_or_else(|| {
            FactoryError::BadRequest(format!(
                "task {task_id} has no run in progress; attachments are no longer accepted"
            ))
        })?;
        self.check_run_token(&run, token, task_id)?;
        let task = self
            .store
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
}
