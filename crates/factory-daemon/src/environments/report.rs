//! The Operations page and the release detail: composition over L1's deployment
//! history and records with L4's pending operations and the producers' build and
//! SBOM facts (`Facts<People>`). It is a page, not L1 state, so it lives beside the
//! entry point; the L1 pieces it uses are `L1Service`.
use super::*;
use crate::facts::{Facts, ReleaseBuildQuery, ReleaseSbomQuery};
use factory_core::environments::{ReleaseDetail, ReleaseQuery};

impl Engine {
    /// The Operations tab's report, narrowed to `scope`'s subtree when one
    /// is named.
    pub(crate) async fn environments_report(&self, scope: Option<String>) -> Result<EnvironmentsReport> {
        self.environment_report(scope, true).await
    }

    /// Metric production reads L1 history only, not pending L4 workflows.
    async fn environment_report(&self, scope: Option<String>, actions: bool) -> Result<EnvironmentsReport> {
        let factory_infrastructure::environment_facts::History {
            mut report, declarations: declared, members, deployments: history,
        } = crate::facts::infrastructure_environments(self).history(scope.as_deref()).await?;
        if actions {
            let pending = self.l4.workflows.active_runs().await?;
            for deployment in &report.deployments {
                if let Ok(plan) = self.deployment_mirror_plan(&deployment.id).await {
                    let receipt = crate::facts::Facts::<factory_kernel::People>::new(self).get::<factory_kernel::DeploymentMirrorFact>(&deployment.id).await?.into_iter().next();
                    report.deployment_mirrors.insert(deployment.id.clone(), env::DeploymentMirrorOffer { plan, receipt });
                }
            }
            report.recovery_journal = Some(crate::facts::Facts::<factory_kernel::People>::new(self).get::<factory_kernel::RecoveryJournalFact>(
                &crate::facts::RecoveryQuery { scopes: members.clone(), limit: 200 }
            ).await?);
            report.recoveries = crate::facts::Facts::<factory_kernel::People>::new(self).get::<factory_kernel::EnvironmentRecoveryFact>(
                &crate::facts::RecoveryQuery { scopes: members.clone(), limit: 200 }
            ).await?;
            for card in &mut report.environments {
                if let Some((scope, environment)) = declared.iter().find(|(_, environment)| environment.name == card.name) {
                    let mut reason = self.recovery_recipe(scope, environment).await.err().map(|error| error.to_string());
                    if pending.iter().any(|run| operation_targets(run, &card.name)) || card.running.is_some() {
                        reason = Some("an environment operation is already pending or running".into());
                    }
                    card.recovery_ready = reason.is_none();
                    card.recovery_reason = reason;
                }
                if let Some((_, source)) = declared.iter().find(|(_, environment)| {
                    environment.name == card.name && environment.promotes_to.is_some()
                }) {
                    let reason = match self.promotion_target(source, card.current.as_ref(), &history).await {
                        Err(error) => Some(error.to_string()),
                        Ok((_, target, _)) if pending.iter().any(|run| operation_targets(run, &target.name)) => {
                            Some("a promotion to this target is already pending".into())
                        }
                        Ok(_) => None,
                    };
                    card.promotion_ready = reason.is_none();
                    card.promotion_reason = reason;
                }
            }
        }
        Ok(report)
    }

    async fn actor(&self, caller: &Caller) -> Actor {
        match caller {
            Caller::Owner => Actor { kind: ActorKind::Person, name: "owner".into(), run_id: None, task_id: None },
            Caller::Agent { name, run_id: Some(run_id), .. } => {
                let task_id = self.l4.store.get_run(run_id).await.ok().flatten().map(|r| r.task_id);
                Actor { kind: ActorKind::Run, name: name.clone(), run_id: Some(run_id.clone()), task_id }
            }
            Caller::Agent { name, run_id: None, .. } => {
                Actor { kind: ActorKind::Agent, name: name.clone(), run_id: None, task_id: None }
            }
        }
    }

    pub(crate) async fn release_detail(&self, query: ReleaseQuery) -> Result<ReleaseDetail> {
        let snapshot = self.factory_snapshot();
        snapshot.scope(&query.scope)?;
        let report = self.environment_report(Some(query.scope.clone()), false).await?;
        let mut release = report
            .releases
            .into_iter()
            .find(|release| release.scope == query.scope && release.facts.commit == query.commit)
            .ok_or_else(|| FactoryError::BadRequest("release is not recorded in the selected scope".into()))?;
        let facts = if let Some(id) = &query.deployment {
            let deployment = self
                .l1.environments
                .deployment(id)
                .await?
                .filter(|deployment| deployment.scope == query.scope && deployment.release.commit == query.commit)
                .ok_or_else(|| {
                    FactoryError::BadRequest("deployment does not belong to the selected release and scope".into())
                })?;
            deployment.release
        } else {
            release.facts.clone()
        };
        // A selected attempt's version/build identity must agree with its evidence,
        // even when the catalogue aggregates other attempts at the same commit.
        release.facts = facts.clone();
        let reader = Facts::<factory_kernel::People>::new(self);
        let mut build = None;
        let build_reason = if let Some(run_id) = &facts.build_run {
            let asked = ReleaseBuildQuery {
                scope: facts.build_scope.clone().unwrap_or_else(|| query.scope.clone()),
                commit: query.commit.clone(),
                run_id: run_id.clone(),
            };
            match reader.get::<factory_kernel::ReleaseBuildFact>(&asked).await {
                Ok(evidence) => {
                    build = evidence;
                    build
                        .is_none()
                        .then(|| "selected run has no completed, clean, source-matching artifact provenance".into())
                }
                Err(error) => Some(format!("build evidence could not be read: {error}")),
            }
        } else {
            Some("no producing build run was recorded; a deployment actor is not a build".into())
        };
        let asked = ReleaseSbomQuery {
            scope: facts.build_scope.clone().unwrap_or_else(|| query.scope.clone()),
            commit: query.commit,
            version: facts.version.clone(),
        };
        let (sboms, sbom_reason) = match reader.get::<factory_kernel::ReleaseSbomFact>(&asked).await {
            Ok(sboms) => {
                let reason = sboms
                    .is_empty()
                    .then(|| "no build SBOM attachment matches this exact product commit/version".into());
                (sboms, reason)
            }
            Err(error) => (vec![], Some(format!("SBOM evidence could not be read: {error}"))),
        };
        Ok(ReleaseDetail { changes: facts.changes, release, build, sboms, build_reason, sbom_reason })
    }

    /// Record a deployment starting: resolve who is asking (a run's task id is
    /// L4's), then L1 records it.
    pub(crate) async fn deploy_start(&self, caller: &Caller, req: DeployStart) -> Result<Deployment> {
        let actor = self.actor(caller).await;
        self.l1_service().deploy_start(caller, actor, req).await
    }
}
