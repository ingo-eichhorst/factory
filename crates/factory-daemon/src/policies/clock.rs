//! Outside wiring for the live L6 reporting-clock service. Lower providers
//! are constructed here; subtree selection, receipt reads and arithmetic
//! are owned by Direction, not by the transport router.
use crate::{engine::Engine, facts::Port};
use factory_core::error::Result;
use factory_core::reporting_clock::ReportingClock;

impl Engine {
    pub(crate) async fn policy_clock(&self, scope: Option<&str>) -> Result<ReportingClock> {
        let snapshot = self.factory_snapshot();
        let findings = factory_kernel::ExploitedFinding::provider(self);
        let reports = factory_kernel::ConfirmedSecurityReport::provider(self);
        self.policy_service(&snapshot)
            .clock(scope, &findings, &reports)
            .await
    }
}
