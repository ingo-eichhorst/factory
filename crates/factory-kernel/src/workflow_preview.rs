//! Plain live workflow preview observations. Validation, compilation,
//! injection, binding and selection belong to the producing services.
use crate::StepKind;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowTargetsFact {
    pub scope: String,
    pub categories: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowEnforcement {
    pub workflow: String,
    pub name: String,
    pub scope: String,
    pub node: String,
    pub step: String,
    pub kind: StepKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowEnforcementFinding {
    pub workflow: String,
    pub name: String,
    pub scope: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowPreviewFact {
    pub enforcement: Vec<WorkflowEnforcement>,
    pub findings: Vec<WorkflowEnforcementFinding>,
}
