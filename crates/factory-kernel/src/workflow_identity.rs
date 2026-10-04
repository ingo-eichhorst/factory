//! Workflow/node identities and a workspace ref carried on dispatch commands.
//! No task lifecycle, workflow scheduling or workspace provisioning lives here.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowOrigin {
    pub workflow_id: String,
    pub workflow_run_id: String,
    pub node_id: String,
    /// Workspace constraints owned by the workflow.  A decomposition uses
    /// this to branch every worker from the integration branch as it stood
    /// when that worker became runnable; ordinary workflows leave it empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkflowWorkspace>,
}

/// The part of a workflow workspace a task runner needs to know.  The
/// workflow engine owns and advances the ref; the regular run/worktree path
/// merely provisions the task's branch from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowWorkspace {
    pub base_ref: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_origin_omits_absent_workspace_and_keeps_explicit_base_ref() {
        let wire = serde_json::json!({"workflow_id":"w", "workflow_run_id":"wr", "node_id":"n"});
        let mut origin: WorkflowOrigin = serde_json::from_value(wire.clone()).unwrap();
        assert!(origin.workspace.is_none());
        assert_eq!(serde_json::to_value(&origin).unwrap(), wire);
        origin.workspace = Some(WorkflowWorkspace {
            base_ref: "refs/heads/integration".into(),
        });
        let restored: WorkflowOrigin =
            serde_json::from_value(serde_json::to_value(&origin).unwrap()).unwrap();
        assert_eq!(restored, origin);
    }
}
