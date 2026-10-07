//! Requests served by L2 Environment. The arms are moved verbatim from the single
//! `dispatch_request` match; `Request::level` decides which file a request lands in.
use crate::engine::*;
use super::misrouted;

impl Engine {
    pub(super) async fn route_l2(
        &self,
        caller: &crate::access::Caller,
        req: Request,
    ) -> Result<Payload> {
        match req {
            // Boxed, like intake's: inline, these size every request's
            // future past a worker thread's stack in a debug build.
            Request::Environment => {
                let (sandboxes, credentials) = Box::pin(self.environment()).await?;
                let (secrets, undeclared, secret_changes) = Box::pin(self.secrets_view()).await;
                Ok(Payload::Environment {
                    sandboxes,
                    credentials,
                    secrets,
                    undeclared,
                    secret_changes,
                })
            }
            Request::SecretSet { name, metadata } => Ok(Payload::Secret {
                secret: Box::pin(self.set_secret(caller, &name, metadata)).await?,
            }),
            Request::Dependencies { scope } => {
                let mut report = self.dependencies_report(&scope).await?;
                report.service_evidence = Some(crate::facts::Facts::<factory_kernel::People>::new(self)
                    .get::<factory_kernel::SandboxServiceEvidenceFact>(&report.scope).await?);
                Ok(Payload::Dependencies { report })
            }
            Request::DependenciesVex { scope } => Ok(Payload::Text {
                text: self.dependencies_vex(&scope).await?,
            }),
            Request::DependencyDocument { scope, id } => {
                let (attachment, document) = self.dependency_document(&scope, &id).await?;
                Ok(Payload::DependencyDocument { attachment, document })
            }
            Request::TaskAttach { id, token, kind, filename: _, bytes } => {
                let attachment = self.attach_dependency(&id, kind, bytes, Some(&token)).await?;
                Ok(Payload::Attachment { attachment })
            }
            other => Err(misrouted(other.level())),
        }
    }
}
