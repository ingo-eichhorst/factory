//! L6 Direction's service (#193 phase 6, slice S11).
//!
//! It owns access to `L6State` (the policy receipts and the goals check-ins) and
//! serves what L6 serves: policy, goals, scenarios and budget. The request
//! arms in `router/l6.rs` call it; the method bodies live beside their catalogues
//! (`policies/`, `goals/`, `scenarios/`, `budgets.rs`).
//!
//! What it cannot do is name another level's state: the rest of the daemon
//! reaches it only through `Wiring` (configuration, typed fact providers, the
//! command chain below, the event bus), whose `Engine` is private. When the
//! providers are constructed from their owning services (S12), `Wiring` goes
//! and the service holds its own `Facts<L6>` and `Commands<L6, ...>`.
//!
//! Page or service (D5), module by module:
//! - `policies/`, `goals/`, `scenarios/`, `budgets.rs`: service (authored intent,
//!   receipts, and the L6 views over lower facts).
//! - Dashboard layout (`Request::Dashboard*`, `configuration.rs`): a page. It edits
//!   scope config files, owns no L6 state, and stays with the entry point.
//! - `policies/mod.rs`'s `policy_export*` and `scenarios`' task payload hydration are
//!   page composition on top of the service and are kept in the same files.
use crate::engine::Engine;
use crate::facts::Wiring;
use crate::state::L6State;

pub(crate) struct L6Service<'a> {
    pub(crate) state: &'a L6State,
    pub(crate) wiring: Wiring<'a>,
}

impl Engine {
    pub(crate) fn l6_service(&self) -> L6Service<'_> {
        L6Service {
            state: &self.l6,
            wiring: Wiring::new(self),
        }
    }
}

/// Test-only forwarders so the existing tests keep calling `engine.<method>`.
/// Production code reaches L6 through `engine.l6_service()`.
#[cfg(test)]
mod test_forwarders {
    use crate::engine::Engine;
    use crate::access::Caller;
    use chrono::{DateTime, Utc};
    use factory_core::checks::CheckSource;
    use factory_core::config::Scope;
    use factory_core::error::Result;
    use factory_core::policy::{Attestation, ControlRef};
    use factory_core::protocol::{
        GoalsReport, PolicyControlDetail, PolicyReport, ScenarioPromoteResult,
        ScenarioWhatIfResult, ScenariosReport,
    };
    use factory_core::reporting_clock::{self, ClockMark, ReportingClock};
    use factory_core::task::Task;
    use std::collections::BTreeMap;

    impl Engine {
        pub(crate) async fn policy_report(&self, scope: Option<&str>) -> Result<PolicyReport> {
            self.l6_service().policy_report(scope).await
        }
        pub(crate) async fn policy_control(&self, control: ControlRef, scope: &str) -> Result<PolicyControlDetail> {
            self.l6_service().policy_control(control, scope).await
        }
        #[allow(clippy::too_many_arguments)]
        pub(crate) async fn policy_attest(
            &self,
            caller: &Caller,
            control: ControlRef,
            scope: String,
            evidence: String,
            note: Option<String>,
            expires_at: DateTime<Utc>,
            clock: Option<ClockMark>,
            corrective: Option<reporting_clock::CorrectiveMeasureMark>,
        ) -> Result<Attestation> {
            self.l6_service()
                .policy_attest(caller, control, scope, evidence, note, expires_at, clock, corrective)
                .await
        }
        pub(crate) async fn policy_withdraw(&self, caller: &Caller, id: String, reason: Option<String>) -> Result<Attestation> {
            self.l6_service().policy_withdraw(caller, id, reason).await
        }
        pub(crate) async fn policy_remediate(&self, control: ControlRef, scope: String, agent: Option<String>) -> Result<Task> {
            self.l6_service().policy_remediate(control, scope, agent).await
        }
        pub(crate) async fn policy_export(&self, scope: Option<&str>) -> Result<factory_core::policy_export::PolicyExport> {
            self.l6_service().policy_export(scope).await
        }
        pub(crate) async fn policy_export_render(&self, scope: Option<&str>, format: &str) -> Result<(String, String)> {
            self.l6_service().policy_export_render(scope, format).await
        }
        pub(crate) async fn policy_clock(&self, scope: Option<&str>) -> Result<ReportingClock> {
            self.l6_service().policy_clock(scope).await
        }
        pub(crate) async fn goals_report(&self, scope: Option<&str>, cycle_id: Option<&str>) -> Result<GoalsReport> {
            self.l6_service().goals_report(scope, cycle_id).await
        }
        pub(crate) async fn goals_checkin(
            &self,
            caller: &Caller,
            kr: factory_core::goals::KrRef,
            value: f64,
            confidence: u8,
            note: Option<String>,
        ) -> Result<factory_core::goals::CheckIn> {
            self.l6_service().goals_checkin(caller, kr, value, confidence, note).await
        }
        pub(crate) async fn goal_context(
            &self,
            root: std::path::PathBuf,
            label: Option<String>,
        ) -> Option<factory_core::adapter::agent::GoalContext> {
            self.l6_service().goal_context(root, label).await
        }
        pub(crate) async fn scenarios_report(&self, scope: Option<&str>) -> Result<ScenariosReport> {
            self.l6_service().scenarios_report(scope).await
        }
        pub(crate) async fn scenario_whatif(
            &self,
            scenario: Option<String>,
            drivers: BTreeMap<String, String>,
            scope: Option<&str>,
        ) -> Result<ScenarioWhatIfResult> {
            self.l6_service().scenario_whatif(scenario, drivers, scope).await
        }
        pub(crate) async fn scenario_promote(
            &self,
            scenario: String,
            scope: String,
            agent: Option<String>,
        ) -> Result<ScenarioPromoteResult> {
            self.l6_service().scenario_promote(scenario, scope, agent).await
        }
        pub(crate) async fn budget_report(
            &self,
            scope: Option<&str>,
            group_by: factory_kernel::CostGroupBy,
            now: DateTime<Utc>,
        ) -> Result<factory_core::budget::Report> {
            self.l6_service().budget_report(scope, group_by, now).await
        }
        pub(crate) async fn subtree_daily(
            &self,
            asked: Option<&str>,
            now: DateTime<Utc>,
        ) -> Result<Vec<factory_core::protocol::ProductionBucket>> {
            self.l6_service().subtree_daily(asked, now).await
        }
        pub(crate) async fn open_policy_task(
            &self,
            control: &ControlRef,
            scope: &str,
        ) -> Result<Option<factory_kernel::TaskInventoryFact>> {
            self.l6_service().open_policy_task(control, scope).await
        }
        pub(crate) async fn metric_policy_inputs(
            &self,
            snapshot: &factory_core::config::Factory,
            scope: Option<&str>,
        ) -> Result<factory_assurance::metrics_service::PolicyInputs> {
            self.l6_service().metric_policy_inputs(snapshot, scope).await
        }
        pub(crate) async fn load_catalogues_and_tags(
            &self,
        ) -> Result<(
            Vec<factory_core::policy::Catalogue>,
            Vec<factory_core::policy::Finding>,
            std::collections::BTreeSet<String>,
        )> {
            self.l6_service().load_catalogues_and_tags().await
        }
        pub(crate) async fn dataset_level_facts<S: CheckSource>(
            &self,
            per_scope_applied: &[(&Scope, Vec<S>)],
        ) -> Result<(
            BTreeMap<String, factory_core::policy::GateFact>,
            Option<factory_core::policy::DaemonFact>,
            BTreeMap<String, factory_kernel::SecretsPresence>,
            Option<factory_core::backup::BackupFact>,
            Option<factory_core::budget::PolicyConfig>,
        )> {
            self.l6_service().dataset_level_facts(per_scope_applied).await
        }
    }
}
