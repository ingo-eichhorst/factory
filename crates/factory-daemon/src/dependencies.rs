//! Canonical L2 dependency inventory. Attaching a document to a run (`l2_pages.rs`) is a page: it composes L2's
//! attachment files with L4's run, token and journal.
use crate::engine::Engine;
use factory_core::dependencies::{Attachment, DependenciesReport};
use serde_json::Value;
use factory_core::error::Result;
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
}
