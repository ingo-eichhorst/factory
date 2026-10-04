//! Important-date evidence only. Observing, choosing dates and alerting stay in their owners.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateKind {
    Credential,
    Certificate,
    Domain,
    Licence,
    Subscription,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateBasis {
    Observed,
    Derived,
    Declared,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateSource {
    Declaration,
    Tls,
    Openshell,
    ClaudeMetadata,
    GithubAuth,
    PolicyAttestation,
    CraDeadline,
    /// An entry of the instance root's declared secrets catalogue (`#244`).
    Secret,
}

/// Authored metadata vocabulary. Parsing and validation belong to L1/core,
/// not to L0; this type contains no credential source or value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenewalDecl {
    pub name: String,
    pub kind: DateKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<crate::Duration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observe: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub affects: Vec<String>,
    #[serde(default)]
    pub renew: String,
    #[serde(default = "default_owner")]
    pub owner: String,
}
fn default_owner() -> String {
    "owner".into()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopedRenewalDeclaration {
    pub scope: Option<String>,
    pub declaration: RenewalDecl,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenewalDeclarationsFact {
    pub declarations: Vec<ScopedRenewalDeclaration>,
    pub findings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DateDependency {
    pub scope: Option<String>,
    pub agent: Option<String>,
    pub environment: Option<String>,
    pub provider: Option<String>,
    pub label: String,
}

/// No credential contents, auth material, raw subprocess output or private certificate key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpiryObservation {
    pub id: String,
    pub name: String,
    pub kind: DateKind,
    pub scope: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub no_expiry: bool,
    pub basis: DateBasis,
    pub source: DateSource,
    pub detail: String,
    pub observed_at: Option<DateTime<Utc>>,
    pub attempted_at: DateTime<Utc>,
    pub issue: Option<String>,
    pub affects: Vec<DateDependency>,
    pub lead_seconds: u64,
    pub renew: String,
    pub owner: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InfrastructureExpiryFact {
    pub observations: Vec<ExpiryObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialExpiryFact {
    pub observations: Vec<ExpiryObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduledRunDate {
    pub task: String,
    pub title: String,
    pub scope: String,
    pub agent: String,
    pub next_run_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduledRunDatesFact {
    pub runs: Vec<ScheduledRunDate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateState {
    Ok,
    DueSoon,
    Overdue,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenewalMilestone {
    Lead,
    SevenDays,
    OneDay,
    Expired,
    ScheduledRun,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportantDateEntry {
    pub observation: ExpiryObservation,
    /// Retained beside conflicting observed evidence, never silently discarded.
    pub declared_expires_at: Option<DateTime<Utc>>,
    pub declared_no_expiry: bool,
    pub conflict: bool,
    pub state: DateState,
    pub milestone: Option<RenewalMilestone>,
    pub scheduled_risks: Vec<ScheduledRunDate>,
    pub href: String,
    /// Existing clocks keep their own fulfillment logic, not the renewals ledger's.
    pub resolved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportantDatesReport {
    pub at: DateTime<Utc>,
    pub entries: Vec<ImportantDateEntry>,
    pub next_expiry: Option<DateTime<Utc>>,
    pub due_soon: usize,
    pub overdue: usize,
    pub unknown: usize,
    pub observation_issues: Vec<String>,
}
