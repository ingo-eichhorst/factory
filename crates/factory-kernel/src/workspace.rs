//! Workspace commands carry intent, not a git implementation or task store.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceLifetime {
    Run,
    Task,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSpec {
    pub task_id: String,
    pub workflow_run_id: Option<String>,
    pub lifetime: WorkspaceLifetime,
}
