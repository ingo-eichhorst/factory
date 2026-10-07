//! L1 Infrastructure's service (#193 phase 6, slice S5a): running environments.
//!
//! It owns access to `L1State`'s environment store and deployment lock and serves
//! deployment recording (`deploy_start`/`deploy_finish`), post-deploy verification,
//! the health-check loop's sample intake, release records and their Git evidence,
//! and the sample page. The bodies live in `environments/`.
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

/// Test-only forwarders so the existing tests keep calling `engine.<method>`.
#[cfg(test)]
mod test_forwarders {
    use crate::engine::Engine;
    use factory_core::environments::{
        DeployFinish, Deployment, DeployVerification, ReleaseAdd, ReleaseFacts, SamplePage, SampleQuery,
    };
    use factory_core::error::Result;

    impl Engine {
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
