//! L1 Infrastructure's service (#193 phase 6, slice S5a): running environments.
//!
//! It owns access to `L1State`'s environment store and deployment lock, and its
//! backup store and lock. It serves deployment recording (`deploy_start`/
//! `deploy_finish`), post-deploy verification, the health-check loop's sample intake,
//! release records and their Git evidence, the sample page, and the backup job (report,
//! run, verify, restore, and the L1 backup facts provider). The bodies live in
//! `environments/` and `backup/`.
//!
//! It reaches the rest of the daemon only through `Wiring<'a, L1>`: the configuration
//! snapshot and the bus. L1 is the lowest level, so no provider can be built there.
//!
//! Page or service (D5), module by module (S5a):
//! - `environments/mod.rs`, `releases.rs` (`enrich_release`), `checks.rs`: service.
//! - `environments/report.rs` (the Operations page, release detail, and the caller
//!   -> actor lookup for `deploy_start`): **page**. It composes L1 history with L4's
//!   pending operations and the producers' build/SBOM facts.
//! - `environments/promotion.rs`, `recovery.rs`: **composition at the router** (D2). A
//!   promotion or recovery reads L1 history, validates a deploy agent (L3's roster and
//!   roles) and creates and starts an L4 workflow; L1 never starts L4 work.
//!   `settle_run_deployments`/`reconcile_run_deployments` list L4 runs and call L1.
//! - `github_deployments.rs`: an **L4** outbound mirror. It reads L1's
//!   `DeploymentPublicationFact` as a reader (`Facts<L4>`) and records its receipts in
//!   L4's workflow store; its busy lock moved to `L4State` with it.
//! - `doctor.rs`: **page** (L1's Doctor tab composes L2's dependency report and gateway
//!   rows), filed with the router.
//!
//! S5b:
//! - `backup/`: service (`archive.rs`, `repos.rs` are plain helpers).
//! - `host_power.rs`: L1 (the `HostPower` runner, lock and reading, no `Engine`).
//!   `host_power/page.rs`: **page**. The Mac tab pairs L1's reading with the journal of
//!   changes, which lives in L4's task store, so it is `impl Engine` beside the router.
//! - `power.rs`: L1's sleep assertion. It has no `Engine` coupling; dispatch acquires and
//!   releases it from L4 code in `engine.rs`, which becomes the L2 -> L1 host command in S8c.
//! - `renewals/mod.rs`: **page**. Important Dates composes L1's and L2's expiry caches,
//!   L6's policy attestations and clock, and the notify hook. The observation job writes
//!   both L1's and L2's caches. `renewals/probes.rs` (pure probes) stays L1.
use crate::engine::Engine;
use crate::facts::Wiring;
use crate::state::L1State;
use factory_kernel::L1;

pub(crate) struct L1Service<'a> {
    pub(crate) state: &'a L1State,
    pub(crate) wiring: Wiring<'a, L1>,
}

impl Engine {
    pub(crate) fn l1_service(&self) -> L1Service<'_> {
        L1Service {
            state: &self.l1,
            wiring: Wiring::new(self),
        }
    }
}

impl<'s> L1Service<'s> {
    /// L1's own backup facts, over its own store and lock: a same-level read is a
    /// plain call here, never a `Facts` lookup. `facts::infrastructure_backup`
    /// hands the same provider to upper readers.
    pub(crate) fn backup_provider(&self) -> factory_infrastructure::backup_facts::Provider<'s> {
        let snapshot = self.wiring.snapshot();
        factory_infrastructure::backup_facts::Provider {
            store: &self.state.backups,
            busy: &self.state.backup_busy,
            root: snapshot.root.clone(),
            instance: snapshot.config.instance.name.clone(),
            config: snapshot.config.infrastructure.backup.clone(),
            booted_at: self.wiring.booted_at(),
        }
    }
}

/// Test-only forwarders so the existing tests keep calling `engine.<method>`.
#[cfg(test)]
mod test_forwarders {
    use crate::engine::Engine;
    use factory_core::environments::{
        DeployFinish, Deployment, DeployVerification, ReleaseAdd, ReleaseFacts, SamplePage, SampleQuery,
    };
    use factory_core::backup::{BackupReport, BackupTrigger, Restoration, Snapshot, Verification};
    use factory_core::error::Result;
    use std::path::PathBuf;

    impl Engine {
        pub(crate) async fn backup_report(&self) -> Result<BackupReport> {
            self.l1_service().backup_report().await
        }
        pub(crate) async fn backup_run(&self, trigger: BackupTrigger, by: String) -> Result<Snapshot> {
            self.l1_service().backup_run(trigger, by).await
        }
        pub(crate) async fn backup_verify(
            &self,
            snapshot: Option<String>,
            identity: Option<PathBuf>,
            by: String,
        ) -> Result<Verification> {
            self.l1_service().backup_verify(snapshot, identity, by).await
        }
        pub(crate) async fn backup_restore(
            &self,
            snapshot: String,
            into: PathBuf,
            identity: Option<PathBuf>,
        ) -> Result<Restoration> {
            self.l1_service().backup_restore(snapshot, into, identity).await
        }
        pub(crate) async fn deploy_finish(&self, req: DeployFinish) -> Result<Deployment> {
            self.l1_service().deploy_finish(req).await
        }
        pub(crate) async fn release_add(&self, req: ReleaseAdd) -> Result<(String, ReleaseFacts)> {
            self.l1_service().release_add(req).await
        }
        pub(crate) async fn check_environment(&self, environment: &str) -> Result<DeployVerification> {
            self.l1_service().check_environment(environment).await
        }
        pub(crate) async fn environment_samples(&self, query: SampleQuery) -> Result<SamplePage> {
            self.l1_service().environment_samples(query).await
        }
    }
}
