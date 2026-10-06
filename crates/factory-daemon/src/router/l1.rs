//! Requests served by L1 Infrastructure. The arms are moved verbatim from the single
//! `dispatch_request` match; `Request::level` decides which file a request lands in.
use crate::engine::*;
use super::misrouted;

impl Engine {
    pub(super) async fn route_l1(
        self: &Arc<Self>,
        caller: &crate::access::Caller,
        req: Request,
    ) -> Result<Payload> {
        match req {
            Request::Doctor => Ok(Payload::Doctor {
                report: self.doctor_report().await?,
            }),
            // Keep the provider/snapshot projection out of this already-large
            // request future's stack frame. Every request variant shares that
            // frame even when Infrastructure was not the one selected.
            Request::Infrastructure => Ok(Box::pin(self.infrastructure()).await),
            // Boxed like `Infrastructure`: a handful of awaited commands.
            Request::HostPowerMode => Ok(Payload::HostPowerMode {
                report: Box::pin(self.host_power_report()).await,
            }),
            Request::HostPowerModeSet { mode } => Ok(Payload::HostPowerMode {
                report: Box::pin(self.set_host_power_mode(caller, mode)).await?,
            }),
            Request::ImportantDates { scope } => Ok(Payload::ImportantDates { report: Box::new(self.important_dates(scope.as_deref()).await?) }),
            Request::Backup => Ok(Payload::Backup {
                report: Box::new(self.backup_report().await?),
            }),
            // `backup_completed`/`backup_failed`/`backup_verified` are
            // published inside, where the job's own backups publish them too.
            Request::BackupRun => Ok(Payload::BackupRun {
                snapshot: self
                    .backup_run(
                        factory_core::backup::BackupTrigger::Manual,
                        crate::policies::caller_name(caller),
                    )
                    .await?,
            }),
            Request::Environments { scope } => Ok(Payload::Environments {
                report: Box::new(self.environments_report(scope).await?),
            }),
            Request::EnvironmentPromote(req) => Ok(Payload::WorkflowRun {
                run: self.promote_environment(caller, req).await?,
            }),
            Request::EnvironmentRecover(req) => Ok(Payload::WorkflowRun { run: self.recover_environment(caller, req).await? }),
            Request::EnvironmentCheck { environment } => Ok(Payload::EnvironmentVerification { verification: self.l1_service().check_environment(&environment).await? }),
            Request::EnvironmentSamples(query) => Ok(Payload::EnvironmentSamples { page: self.l1_service().environment_samples(query).await? }),
            Request::ReleaseDetail(query) => Ok(Payload::ReleaseDetail { detail: Box::new(self.release_detail(query).await?) }),
            // `deployment_updated` is published inside.
            Request::DeployStart(req) => Ok(Payload::Deployment {
                deployment: Box::new(self.deploy_start(caller, req).await?),
            }),
            Request::DeployFinish(req) => Ok(Payload::Deployment {
                deployment: Box::new(self.l1_service().deploy_finish(req).await?),
            }),
            Request::DeployMirrorPlan { id } => Ok(Payload::DeploymentMirrorPlan { plan: self.deployment_mirror_plan(&id).await? }),
            Request::DeployPublish { id, approval } => Ok(Payload::DeploymentMirror { receipt: self.publish_deployment(caller, &id, &approval).await? }),
            Request::ReleaseAdd(req) => {
                let (scope, release) = self.l1_service().release_add(req).await?;
                Ok(Payload::ReleaseAdded { scope, release })
            }
            Request::BackupVerify { snapshot, identity } => Ok(Payload::BackupVerify {
                verification: self
                    .backup_verify(snapshot, identity, crate::policies::caller_name(caller))
                    .await?,
            }),
            Request::BackupRestore { snapshot, into, identity } => Ok(Payload::BackupRestore {
                restoration: self.backup_restore(snapshot, into, identity).await?,
            }),
            other => Err(misrouted(other.level())),
        }
    }
}
