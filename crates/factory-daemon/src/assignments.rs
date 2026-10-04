//! L3 assignment preparation asks the L2 workspace owner to provision cwd.
//! Conversation reset and workspace lifetime are deliberately independent.
use crate::worktree::{OwnedWorkspace, Owner};
use factory_kernel::WorkspaceSpec;
use std::path::{Path, PathBuf};

pub struct AssignmentWorkspace {
    pub spec: WorkspaceSpec,
    pub candidate: PathBuf,
    pub branch: String,
    pub base: Option<String>,
    pub previous: Option<(PathBuf, String)>,
}

impl AssignmentWorkspace {
    pub async fn provision(
        self,
        owner: &Owner,
        scope: &Path,
    ) -> Result<(OwnedWorkspace, bool), String> {
        if let Some((path, branch)) = self.previous {
            return owner
                .adopt(self.spec, scope, path, branch)
                .await
                .map(|record| (record, true));
        }
        owner
            .provision(
                self.spec,
                scope,
                self.candidate,
                self.branch,
                self.base.as_deref(),
            )
            .await
    }
}

pub async fn release(
    owner: &Owner,
    paths: &[PathBuf],
) -> Result<Vec<(OwnedWorkspace, Result<(), String>)>, String> {
    owner.release(paths).await
}
