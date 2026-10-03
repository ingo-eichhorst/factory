//! `Request::PolicyClock`: the CRA Art. 14 reporting clock (`#157`, phase
//! 1). This module owns no state of its own -- the one piece of new state
//! the clock reads, `policy_attestations`, already lives in `PolicyStore`
//! (`store.rs`) -- it only folds the L2 exploited-findings read
//! (`Engine::exploited_findings`, `dependencies.rs`) and the L4
//! confirmed-security-report fact (`Engine::confirmed_security_reports`,
//! `intake.rs`) into `factory_core::reporting_clock::compute`, itself pure
//! and tested on its own.

use std::collections::BTreeSet;

use chrono::Utc;
use factory_core::error::Result;
use factory_core::reporting_clock::{self, ReportingClock};

use crate::engine::Engine;

use super::subtree_scopes;

impl Engine {
    /// `Request::PolicyClock`: every exploited finding and confirmed
    /// security report over `scope`'s subtree (the whole instance when
    /// `scope` is `None`), with 24h/72h and evidenced 14-day deadlines computed fresh.
    /// The same subtree resolution `Request::Policy` and
    /// `confirmed_security_reports` themselves use -- `exploited_findings`
    /// itself reads one scope at a time, so this asks it once per scope in
    /// the subtree and folds the answers together.
    pub(crate) async fn policy_clock(&self, scope: Option<&str>) -> Result<ReportingClock> {
        let snapshot = self.factory_snapshot();
        let (asked, target_scopes) = subtree_scopes(&snapshot, scope)?;
        let mut names: BTreeSet<String> = target_scopes.into_iter().map(|s| s.name).collect();
        if let Some(asked) = &asked {
            names.insert(asked.name.clone());
        }

        let mut findings = Vec::new();
        for name in &names {
            findings.extend(crate::facts::Facts::<factory_kernel::L6>::new(self)
                .get::<factory_kernel::ExploitedFinding>(name).await?);
        }
        let reports = crate::facts::Facts::<factory_kernel::L6>::new(self)
            .get::<factory_kernel::ConfirmedSecurityReport>(&scope.map(str::to_string)).await?;
        let attestations = self.policies.all().await?;
        Ok(reporting_clock::compute(&findings, &reports, &attestations, Utc::now()))
    }
}
