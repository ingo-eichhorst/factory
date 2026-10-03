//! L4's artifact evidence vocabulary. No filesystem reads or evaluation in L0.
use crate::{RequiredStep, StepAttestation};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactSource {
    pub commit: String,
    pub worktree_digest: String,
    pub dirty: bool,
}

/// Bytes captured at the producing agent's done report. Storage is immutable
/// and instance-owned; the source path is retained only for final validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactSnapshot {
    pub id: String,
    pub name: String,
    pub scope: String,
    pub category: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub storage_path: String,
    pub source_path: String,
    pub source: ArtifactSource,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactProvenance {
    pub id: String,
    pub run_id: String,
    pub task_id: String,
    pub scope: String,
    pub artifact: ArtifactSnapshot,
    pub statement: ProvenanceStatement,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvenanceStatement {
    #[serde(rename = "_type")]
    pub statement_type: String,
    pub subject: Vec<ProvenanceSubject>,
    pub predicate_type: String,
    pub predicate: ProvenancePredicate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceSubject {
    pub name: String,
    pub digest: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvenancePredicate {
    pub build_definition: ProvenanceBuildDefinition,
    pub run_details: ProvenanceRunDetails,
    /// URI-named extension, as allowed by SLSA v1. Requirement references
    /// remain opaque; L4 never interprets a control or framework name.
    #[serde(rename = "https://github.com/ingo-eichhorst/factory/run-evidence/v1")]
    pub evidence: ProvenanceRunEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvenanceBuildDefinition {
    pub build_type: String,
    pub external_parameters: ProvenanceParameters,
    pub resolved_dependencies: Vec<ProvenanceDependency>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvenanceParameters {
    pub task_id: String,
    pub scope: String,
    pub category: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceDependency {
    pub name: String,
    pub digest: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvenanceRunDetails {
    pub builder: ProvenanceBuilder,
    pub metadata: ProvenanceMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceBuilder {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvenanceMetadata {
    pub invocation_id: String,
    pub started_on: DateTime<Utc>,
    pub finished_on: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProvenanceRunEvidence {
    pub agent: String,
    pub adapter: String,
    pub runtime: String,
    pub source: ArtifactSource,
    pub required_steps: Vec<RequiredStep>,
    pub attestations: Vec<StepAttestation>,
}
