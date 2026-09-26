//! CycloneDX dependency inventory vocabulary shared by the daemon, CLI and UI.
//!
//! Scanners stay ordinary workflow tasks. This module is only their wire
//! contract and the pure validation/shaping vocabulary around the two files a
//! scan attaches; it is deliberately not a sixth adapter seam.

use crate::config::DependencyService;
use crate::policy::Duration;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    Sbom,
    Vulnerabilities,
}

impl AttachmentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sbom => "sbom",
            Self::Vulnerabilities => "vulnerabilities",
        }
    }
}

impl std::fmt::Display for AttachmentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for AttachmentKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "sbom" => Ok(Self::Sbom),
            "vulnerabilities" => Ok(Self::Vulnerabilities),
            _ => Err(format!("{s:?} is not sbom or vulnerabilities")),
        }
    }
}

/// The product lifecycle states ADR 0005 maps from CycloneDX phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    Declared,
    Built,
    Running,
}

impl LifecycleState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Declared => "declared",
            Self::Built => "built",
            Self::Running => "running",
        }
    }

    pub fn from_phase(phase: &str) -> Option<Self> {
        match phase {
            "pre-build" => Some(Self::Declared),
            "build" => Some(Self::Built),
            "operations" => Some(Self::Running),
            _ => None,
        }
    }
}

impl std::fmt::Display for LifecycleState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct ValidatedDocument {
    pub json: Value,
    pub spec_version: String,
    pub states: Vec<LifecycleState>,
}

/// Validate the deliberately small part of CycloneDX that is Factory's
/// contract. Full schema validation remains a scanner concern; Factory checks
/// the discriminating facts before accepting a document into its audit trail.
pub fn validate_document(bytes: &[u8], kind: AttachmentKind) -> Result<ValidatedDocument, String> {
    let json: Value =
        serde_json::from_slice(bytes).map_err(|e| format!("attachment is not JSON: {e}"))?;
    let object = json
        .as_object()
        .ok_or_else(|| "CycloneDX document must be a JSON object".to_string())?;
    if object.get("bomFormat").and_then(Value::as_str) != Some("CycloneDX") {
        return Err("attachment is not CycloneDX JSON (`bomFormat` must be `CycloneDX`)".into());
    }
    let spec_version = object
        .get("specVersion")
        .and_then(Value::as_str)
        .ok_or_else(|| "CycloneDX document has no string `specVersion`".to_string())?;
    let spec_version = spec_version.to_string();
    let mut parts = spec_version.split('.');
    let major = parts.next().and_then(|p| p.parse::<u32>().ok());
    let minor = parts.next().and_then(|p| p.parse::<u32>().ok());
    if major.zip(minor).is_none_or(|v| v < (1, 6)) {
        return Err(format!(
            "CycloneDX specVersion {spec_version:?} is too old; Factory requires >= 1.6"
        ));
    }

    let states = object
        .get("metadata")
        .and_then(Value::as_object)
        .and_then(|m| m.get("lifecycles"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("phase").and_then(Value::as_str))
        .filter_map(LifecycleState::from_phase)
        .collect::<Vec<_>>();

    match kind {
        AttachmentKind::Sbom => {
            // Presence is refused even when the array is empty: BSI
            // TR-03183-2's separation is structural, and accepting the field
            // would let a producer silently start populating it later.
            if object.contains_key("vulnerabilities") {
                return Err(
                    "an SBOM must not carry `vulnerabilities`; attach them as --kind vulnerabilities"
                        .into(),
                );
            }
            if states.is_empty() {
                return Err(
                    "an SBOM must declare metadata.lifecycles phase pre-build, build, or operations"
                        .into(),
                );
            }
        }
        AttachmentKind::Vulnerabilities => {
            let vulnerabilities = object
                .get("vulnerabilities")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    "a vulnerabilities attachment must carry a CycloneDX `vulnerabilities` array"
                        .to_string()
                })?;
            for (index, vulnerability) in vulnerabilities.iter().enumerate() {
                if vulnerability.get("id").and_then(Value::as_str).is_none() {
                    return Err(format!("vulnerabilities[{index}] has no string `id`"));
                }
                let affects = vulnerability
                    .get("affects")
                    .and_then(Value::as_array)
                    .filter(|affects| !affects.is_empty())
                    .ok_or_else(|| format!("vulnerabilities[{index}] has no `affects` entries"))?;
                if affects
                    .iter()
                    .any(|affect| affect.get("ref").and_then(Value::as_str).is_none())
                {
                    return Err(format!(
                        "vulnerabilities[{index}] has an `affects` entry without a string `ref`"
                    ));
                }
            }
        }
    }

    Ok(ValidatedDocument {
        json,
        spec_version,
        states,
    })
}

/// The append-only sidecar stored beside one raw CycloneDX document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    pub id: String,
    pub kind: AttachmentKind,
    pub scope: String,
    pub run_id: String,
    pub task_id: String,
    pub attempt: u32,
    pub attached_at: DateTime<Utc>,
    pub filename: String,
    pub spec_version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<LifecycleState>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentSummary {
    pub attachment: Attachment,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_time: Option<DateTime<Utc>>,
    /// The released product this document describes. Old documents and
    /// third-party scope scans need not carry it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<ProductIdentity>,
}

/// Factory's release identity. Version alone is not enough while releases
/// still share the workspace's `0.1.0`; the source commit is part of the key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductIdentity {
    pub version: String,
    pub git_sha: String,
}

/// Read a release identity from CycloneDX's root component. Producers put the
/// commit beside the component as a namespaced property, leaving the SBOM
/// standard and useful to tools that know nothing about Factory.
pub fn product_identity(document: &Value) -> Option<ProductIdentity> {
    let component = document.get("metadata")?.get("component")?;
    let version = component.get("version")?.as_str()?.trim();
    let git_sha = component
        .get("properties")?
        .as_array()?
        .iter()
        .find(|property| property.get("name").and_then(Value::as_str) == Some("factory:git-sha"))?
        .get("value")?
        .as_str()?
        .trim();
    if version.is_empty() || git_sha.is_empty() || git_sha == "unknown" {
        return None;
    }
    Some(ProductIdentity {
        version: version.to_string(),
        git_sha: git_sha.to_string(),
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LifecycleDocuments {
    pub state: LifecycleState,
    pub sbom: DocumentSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vulnerabilities: Option<DocumentSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
    Unknown,
}

impl Severity {
    pub fn from_cyclonedx(value: Option<&str>) -> Self {
        match value.unwrap_or_default().to_ascii_lowercase().as_str() {
            "critical" => Self::Critical,
            "high" => Self::High,
            "medium" | "moderate" => Self::Medium,
            "low" | "negligible" => Self::Low,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rating {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub severity: Severity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vector: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffectedComponent {
    pub bom_ref: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingStatus {
    Open,
    Assessed,
    Resolved,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DependencyFinding {
    pub id: String,
    pub state: LifecycleState,
    pub status: FindingStatus,
    pub severity: Severity,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ratings: Vec<Rating>,
    pub affected: AffectedComponent,
    pub scan: DocumentSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vex_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vex_justification: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vex_response: Vec<String>,
    #[serde(default)]
    pub kev: bool,
    #[serde(default)]
    pub euvd: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epss: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DependencyServiceView {
    #[serde(flatten)]
    pub service: DependencyService,
    /// `None` means the credential location is not one the Secrets inventory
    /// currently knows how to check. It is never guessed absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_present: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DependenciesReport {
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_workflow: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age: Option<Duration>,
    pub documents: Vec<LifecycleDocuments>,
    pub findings: Vec<DependencyFinding>,
    pub services: Vec<DependencyServiceView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoctorStatus {
    Current,
    Behind,
    Missing,
}

/// `GET /api/doctor`: Factory as installed on this instance, compared with
/// the newest Factory build the dependency evidence knows about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub now: DateTime<Utc>,
    pub status: DoctorStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built: Option<LifecycleDocuments>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running: Option<LifecycleDocuments>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<DependencyFinding>,
}

/// The policy evaluator's compact view of a Dependencies report.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependenciesFact {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_sbom_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built_sbom_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub open: BTreeMap<Severity, u32>,
    #[serde(default)]
    pub exploited_open: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SBOM: &[u8] = include_bytes!("../tests/fixtures/dependencies/declared-sbom.cdx.json");
    const VULNS: &[u8] = include_bytes!("../tests/fixtures/dependencies/vulnerabilities.cdx.json");

    #[test]
    fn accepts_the_two_cyclonedx_16_documents() {
        let sbom = validate_document(SBOM, AttachmentKind::Sbom).unwrap();
        assert_eq!(sbom.states, vec![LifecycleState::Declared]);
        let vulnerabilities = validate_document(VULNS, AttachmentKind::Vulnerabilities).unwrap();
        assert!(vulnerabilities.states.is_empty());
    }

    #[test]
    fn refuses_vulnerability_data_inside_an_sbom() {
        let error = validate_document(VULNS, AttachmentKind::Sbom).unwrap_err();
        assert!(
            error.contains("must not carry `vulnerabilities`"),
            "{error}"
        );
    }

    #[test]
    fn refuses_old_or_unclassified_documents() {
        let old = br#"{"bomFormat":"CycloneDX","specVersion":"1.5","metadata":{"lifecycles":[{"phase":"pre-build"}]}}"#;
        assert!(validate_document(old, AttachmentKind::Sbom)
            .unwrap_err()
            .contains(">= 1.6"));
        let no_state = br#"{"bomFormat":"CycloneDX","specVersion":"1.6","components":[]}"#;
        assert!(validate_document(no_state, AttachmentKind::Sbom)
            .unwrap_err()
            .contains("lifecycles"));
    }

    #[test]
    fn reads_product_identity_only_when_version_and_commit_are_present() {
        let built: Value = serde_json::from_slice(include_bytes!(
            "../tests/fixtures/dependencies/build-sbom.cdx.json"
        ))
        .unwrap();
        assert_eq!(
            product_identity(&built),
            Some(ProductIdentity {
                version: "0.1.0".into(),
                git_sha: "0123456789abcdef0123456789abcdef01234567".into(),
            })
        );

        let declared: Value = serde_json::from_slice(SBOM).unwrap();
        assert_eq!(product_identity(&declared), None);
    }
}
