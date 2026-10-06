//! Outside wiring for the live L6 reporting-clock service. Lower providers
//! are constructed here; subtree selection, receipt reads and arithmetic
//! are owned by Direction, not by the transport router.
use crate::l6_service::L6Service;
use factory_core::error::Result;
use factory_core::reporting_clock::ReportingClock;

impl L6Service<'_> {
    pub(crate) async fn policy_clock(&self, scope: Option<&str>) -> Result<ReportingClock> {
        let snapshot = self.wiring.snapshot();
        let findings = self.wiring.provider::<factory_kernel::ExploitedFinding>();
        let reports = self.wiring.provider::<factory_kernel::ConfirmedSecurityReport>();
        self.policy_service(&snapshot)
            .clock(scope, &findings, &reports)
            .await
    }
}
