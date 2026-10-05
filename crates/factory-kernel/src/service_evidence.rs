//! L2's reported sandbox service access, never inferred from declarations.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedTransport {
    Network,
    Socket,
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessDisposition {
    Allowed,
    Denied,
    Blocked,
}

/// Only the endpoint/path and the control's disposition, not request payloads,
/// URL query strings, credential values or an inferred compliance verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedServiceAccess {
    pub at: DateTime<Utc>,
    pub transport: ObservedTransport,
    pub target: String,
    pub disposition: AccessDisposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<String>,
    /// OpenShell's own `[reason:...]` text, kept verbatim (it can contain
    /// spaces). Absent for a decision the log gave no reason for, and for
    /// every record captured before this field existed -- `#[serde(default)]`
    /// keeps old stored receipts and `capture_identity()` loading unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxServiceCapture {
    pub id: String,
    pub run_id: String,
    pub task_id: String,
    pub agent: String,
    pub sandbox: String,
    pub captured_at: DateTime<Utc>,
    pub source: String,
    /// A bounded log window is always partial, even when it contains no rows.
    pub partial: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accesses: Vec<ObservedServiceAccess>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxServiceEvidenceFact {
    pub scope: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub captures: Vec<SandboxServiceCapture>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<String>,
}
