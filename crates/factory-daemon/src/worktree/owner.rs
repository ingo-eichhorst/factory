//! L2 owns workspace provisioning, its durable receipts, and safe release.
//! Unknown directories are never discovered and deleted by a filesystem walk.
use super::{create, git_error, git_output, is_registered};
use factory_kernel::{WorkspaceLifetime, WorkspaceSpec};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Back-pressure, never eviction of live or unsaved work. Reuse needs no slot.
pub const SCOPE_WORKSPACE_CAP: usize = 128;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnedWorkspace {
    pub spec: WorkspaceSpec,
    pub scope_path: PathBuf,
    pub path: PathBuf,
    pub branch: String,
    #[serde(default)]
    pub created: bool,
    #[serde(default)]
    pub last_refusal: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn git(dir: &Path, args: &[&str]) {
        let output = git_output(dir, args).await.unwrap();
        assert!(
            output.status.success(),
            "{}",
            git_error("test git", &output)
        );
    }

    async fn fixture() -> (PathBuf, PathBuf, Owner) {
        let root =
            std::env::temp_dir().join(format!("factory-workspace-owner-{}", uuid::Uuid::new_v4()));
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]).await;
        git(&repo, &["config", "user.name", "Workspace test"]).await;
        git(
            &repo,
            &["config", "user.email", "workspace@example.invalid"],
        )
        .await;
        std::fs::write(repo.join(".gitignore"), "ignored/\n").unwrap();
        git(&repo, &["add", ".gitignore"]).await;
        git(&repo, &["commit", "-q", "-m", "base"]).await;
        let owner = Owner::new(root.join("worktrees"));
        (root, repo, owner)
    }

    fn spec(task: &str, lifetime: WorkspaceLifetime) -> WorkspaceSpec {
        WorkspaceSpec {
            task_id: task.into(),
            workflow_run_id: None,
            lifetime,
        }
    }

    async fn provision(
        owner: &Owner,
        repo: &Path,
        task: &str,
        lifetime: WorkspaceLifetime,
    ) -> OwnedWorkspace {
        owner
            .provision(
                spec(task, lifetime),
                repo,
                owner.root.join(task),
                format!("factory/{task}"),
                None,
            )
            .await
            .unwrap()
            .0
    }

    #[tokio::test]
    async fn workspace_task_reuse_survives_restart_but_run_lifetime_is_fresh() {
        let (root, repo, owner) = fixture().await;
        let first = provision(&owner, &repo, "task", WorkspaceLifetime::Task).await;
        std::fs::write(first.path.join("unfinished"), "keep this").unwrap();
        let restarted = Owner::new(owner.root.clone());
        let (again, reused) = restarted
            .provision(
                spec("task", WorkspaceLifetime::Task),
                &repo,
                owner.root.join("unused"),
                "factory/unused".into(),
                None,
            )
            .await
            .unwrap();
        assert!(reused);
        assert_eq!(again.path, first.path);
        assert_eq!(
            std::fs::read_to_string(again.path.join("unfinished")).unwrap(),
            "keep this"
        );
        let one = provision(&owner, &repo, "bench-one", WorkspaceLifetime::Run).await;
        let (two, reused) = owner
            .provision(
                one.spec.clone(),
                &repo,
                owner.root.join("bench-two"),
                "factory/bench-two".into(),
                None,
            )
            .await
            .unwrap();
        assert!(!reused);
        assert_ne!(one.path, two.path);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn workspace_release_refuses_dirty_ignored_and_unpushed_then_reclaims_after_push() {
        let (root, repo, owner) = fixture().await;
        let record = provision(&owner, &repo, "task", WorkspaceLifetime::Task).await;
        std::fs::write(record.path.join("work"), "valuable work").unwrap();
        let paths = [record.path.clone()];
        let result = owner.release(&paths).await.unwrap();
        assert!(result[0].1.as_ref().unwrap_err().contains("uncommitted"));
        assert!(record.path.exists());
        assert!(
            owner.release(&paths).await.unwrap().is_empty(),
            "unchanged refusal is not journalled every tick"
        );
        git(&record.path, &["add", "work"]).await;
        git(&record.path, &["commit", "-q", "-m", "valuable work"]).await;
        let result = owner.release(&paths).await.unwrap();
        assert!(result[0].1.as_ref().unwrap_err().contains("unpushed"));
        let remote = root.join("backup.git");
        std::fs::create_dir(&remote).unwrap();
        git(&remote, &["init", "--bare", "-q"]).await;
        git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        )
        .await;
        git(
            &record.path,
            &["push", "-q", "-u", "origin", &record.branch],
        )
        .await;
        let ignored = record.path.join("ignored");
        std::fs::create_dir(&ignored).unwrap();
        std::fs::write(ignored.join("secret"), "private handwritten data").unwrap();
        assert!(owner.release(&paths).await.unwrap()[0].1.is_err());
        assert!(ignored.join("secret").exists());
        std::fs::remove_file(ignored.join("secret")).unwrap();
        std::fs::remove_dir(ignored).unwrap();
        let restarted = Owner::new(owner.root.clone());
        assert!(restarted.release(&paths).await.unwrap()[0].1.is_ok());
        assert!(!record.path.exists());
        assert!(restarted.records().await.unwrap().is_empty());
        assert!(!git_output(
            &repo,
            &[
                "rev-parse",
                "--verify",
                &format!("refs/heads/{}", record.branch)
            ]
        )
        .await
        .unwrap()
        .status
        .success());
        assert!(git_output(
            &remote,
            &[
                "rev-parse",
                "--verify",
                &format!("refs/heads/{}", record.branch)
            ]
        )
        .await
        .unwrap()
        .status
        .success());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn workspace_owner_never_adopts_primary_outside_symlink_or_changed_branches() {
        let (root, repo, owner) = fixture().await;
        let record = provision(&owner, &repo, "task", WorkspaceLifetime::Task).await;
        let primary_owner = Owner::new(root.clone());
        assert!(primary_owner
            .adopt(record.spec.clone(), &repo, repo.clone(), "main".into())
            .await
            .is_err());
        git(&repo, &["branch", "factory/pre-existing", "HEAD"]).await;
        assert!(owner
            .provision(
                spec("new", WorkspaceLifetime::Task),
                &repo,
                owner.root.join("new"),
                "factory/pre-existing".into(),
                None
            )
            .await
            .is_err());
        assert_eq!(
            owner.records().await.unwrap().len(),
            1,
            "failed creation must not claim an existing branch"
        );
        assert!(owner
            .adopt(
                record.spec.clone(),
                &repo,
                repo.clone(),
                "factory/task".into()
            )
            .await
            .is_err());
        let outside = root.join("manual");
        create(&repo, &outside, "factory/manual", None)
            .await
            .unwrap();
        assert!(owner
            .adopt(
                record.spec.clone(),
                &repo,
                outside.clone(),
                "factory/manual".into()
            )
            .await
            .is_err());
        assert!(owner.release(&[outside.clone()]).await.unwrap().is_empty());
        assert!(outside.exists());
        #[cfg(unix)]
        {
            let link = owner.root.join("linked");
            std::os::unix::fs::symlink(&record.path, &link).unwrap();
            assert!(owner
                .adopt(record.spec.clone(), &repo, link, record.branch.clone())
                .await
                .is_err());
        }
        git(&record.path, &["switch", "-q", "-c", "human-branch"]).await;
        assert!(owner.release(&[record.path.clone()]).await.unwrap()[0]
            .1
            .is_err());
        assert!(record.path.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn workspace_cap_blocks_allocation_not_reuse_or_eviction() {
        let (root, repo, owner) = fixture().await;
        let first = provision(&owner, &repo, "task", WorkspaceLifetime::Task).await;
        let mut records = vec![first.clone()];
        for number in 1..SCOPE_WORKSPACE_CAP {
            let mut record = first.clone();
            record.path = owner.root.join(format!("retained-{number}"));
            record.spec.task_id = format!("retained-{number}");
            records.push(record);
        }
        owner.write(&records).unwrap();
        let error = owner
            .provision(
                spec("new", WorkspaceLifetime::Task),
                &repo,
                owner.root.join("new"),
                "factory/new".into(),
                None,
            )
            .await
            .unwrap_err();
        assert!(error.contains("cap"));
        assert!(!owner.root.join("new").exists());
        let (again, reused) = owner
            .provision(
                first.spec.clone(),
                &repo,
                owner.root.join("again"),
                "factory/again".into(),
                None,
            )
            .await
            .unwrap();
        assert!(reused);
        assert_eq!(again.path, first.path);
        assert_eq!(owner.records().await.unwrap().len(), SCOPE_WORKSPACE_CAP);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn workspace_receipt_before_creation_crash_is_released_idempotently() {
        let (root, repo, owner) = fixture().await;
        let record = OwnedWorkspace {
            spec: spec("task", WorkspaceLifetime::Task),
            scope_path: repo,
            path: owner.root.join("never-created"),
            branch: "factory/never-created".into(),
            created: false,
            last_refusal: None,
        };
        owner.write(&[record.clone()]).unwrap();
        let restarted = Owner::new(owner.root.clone());
        assert!(restarted.release(&[record.path.clone()]).await.unwrap()[0]
            .1
            .is_ok());
        assert!(restarted.release(&[record.path]).await.unwrap().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}

pub struct Owner {
    root: PathBuf,
    lock: tokio::sync::Mutex<()>,
}

impl Owner {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            lock: tokio::sync::Mutex::new(()),
        }
    }

    fn ledger(&self) -> PathBuf {
        self.root.join(".workspace-owner.json")
    }

    fn read(&self) -> Result<Vec<OwnedWorkspace>, String> {
        match std::fs::read(self.ledger()) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|e| format!("workspace ledger: {e}"))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(format!("workspace ledger: {e}")),
        }
    }

    fn write(&self, records: &[OwnedWorkspace]) -> Result<(), String> {
        std::fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        let temporary = self
            .root
            .join(format!(".workspace-owner-{}.tmp", uuid::Uuid::new_v4()));
        let bytes = serde_json::to_vec(records).map_err(|e| e.to_string())?;
        // Sync before atomic replacement: the receipt precedes git creation,
        // and an interrupted release keeps its receipt for the next sweep.
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        std::fs::rename(&temporary, self.ledger()).map_err(|e| e.to_string())?;
        std::fs::File::open(&self.root)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())
    }

    pub async fn records(&self) -> Result<Vec<OwnedWorkspace>, String> {
        let _guard = self.lock.lock().await;
        self.read()
    }

    /// Migrate only a recorded run's exact registered managed workspace.
    /// Never infer ownership from a branch name or a directory scan alone.
    pub async fn adopt(
        &self,
        spec: WorkspaceSpec,
        scope: &Path,
        path: PathBuf,
        branch: String,
    ) -> Result<OwnedWorkspace, String> {
        let _guard = self.lock.lock().await;
        let mut records = self.read()?;
        let record = OwnedWorkspace {
            spec,
            scope_path: scope.canonicalize().map_err(|e| e.to_string())?,
            path,
            branch,
            created: true,
            last_refusal: None,
        };
        self.validate(&record).await?;
        if let Some(previous) = records.iter().find(|previous| previous.path == record.path) {
            if previous.spec != record.spec
                || previous.scope_path != record.scope_path
                || previous.branch != record.branch
            {
                return Err("workspace ownership differs from the assignment".into());
            }
            return Ok(previous.clone());
        }
        // Adoption/reuse allocates no disk, even when a legacy backlog already
        // exceeds the admission cap. Provision alone enforces new allocations.
        records.push(record.clone());
        self.write(&records)?;
        Ok(record)
    }

    fn validate_path(&self, path: &Path) -> Result<(), String> {
        if path.parent() != Some(self.root.as_path()) || path.file_name().is_none() {
            return Err("workspace is outside the owner's direct directory".into());
        }
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            if meta.file_type().is_symlink() {
                return Err("refusing a symlink workspace".into());
            }
            let root = self.root.canonicalize().map_err(|e| e.to_string())?;
            if path.canonicalize().map_err(|e| e.to_string())?.parent() != Some(root.as_path()) {
                return Err("workspace resolves outside its owner".into());
            }
        }
        Ok(())
    }

    pub async fn provision(
        &self,
        spec: WorkspaceSpec,
        scope_path: &Path,
        candidate: PathBuf,
        branch: String,
        base: Option<&str>,
    ) -> Result<(OwnedWorkspace, bool), String> {
        let _guard = self.lock.lock().await;
        let mut records = self.read()?;
        let scope_path = scope_path.canonicalize().map_err(|e| e.to_string())?;
        if spec.lifetime == WorkspaceLifetime::Task {
            for record in records.iter().rev().filter(|record| {
                record.spec == spec && record.scope_path == scope_path && record.path.exists()
            }) {
                if self.validate(record).await.is_ok() {
                    return Ok((record.clone(), true));
                }
            }
        }
        self.validate_path(&candidate)?;
        if records.iter().any(|record| record.path == candidate) || candidate.exists() {
            return Err("workspace candidate already exists; refusing to overwrite it".into());
        }
        if records
            .iter()
            .filter(|record| record.scope_path == scope_path)
            .count()
            >= SCOPE_WORKSPACE_CAP
        {
            return Err(format!("scope workspace cap ({SCOPE_WORKSPACE_CAP}) reached; release clean, backed-up closed workspaces before starting new work"));
        }
        let reference = format!("refs/heads/{branch}");
        if git_output(
            &scope_path,
            &["show-ref", "--verify", "--quiet", &reference],
        )
        .await?
        .status
        .success()
        {
            return Err(
                "workspace branch already exists; refusing to claim another writer's branch".into(),
            );
        }
        let mut record = OwnedWorkspace {
            spec,
            scope_path,
            path: candidate,
            branch,
            created: false,
            last_refusal: None,
        };
        records.push(record.clone());
        self.write(&records)?;
        create(&record.scope_path, &record.path, &record.branch, base).await?;
        record.created = true;
        *records.last_mut().expect("receipt was appended") = record.clone();
        self.write(&records)?;
        Ok((record, false))
    }

    async fn validate(&self, record: &OwnedWorkspace) -> Result<(), String> {
        self.validate_path(&record.path)?;
        if !record.branch.starts_with("factory/")
            || !is_registered(&record.scope_path, &record.path).await
        {
            return Err(
                "workspace is not a registered Factory branch in its recorded repository".into(),
            );
        }
        let listing = git_output(&record.scope_path, &["worktree", "list", "--porcelain"]).await?;
        let primary = String::from_utf8_lossy(&listing.stdout)
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("worktree "))
            .map(PathBuf::from);
        if primary.and_then(|path| path.canonicalize().ok()) == record.path.canonicalize().ok() {
            return Err("the primary worktree is never owned or released".into());
        }
        let actual = git_output(&record.path, &["symbolic-ref", "--short", "HEAD"]).await?;
        if !actual.status.success()
            || String::from_utf8_lossy(&actual.stdout).trim() != record.branch
        {
            return Err("workspace branch changed; preserving it for a person".into());
        }
        Ok(())
    }

    async fn release_one(&self, record: &OwnedWorkspace) -> Result<(), String> {
        self.validate_path(&record.path)?;
        if record.path.exists() {
            self.validate(record).await?;
            // Ignored files may be secrets or handwritten data, not just target/.
            // Non-force git removal alone can discard them, so include them.
            let status = git_output(
                &record.path,
                &[
                    "status",
                    "--porcelain",
                    "--ignored",
                    "--untracked-files=all",
                ],
            )
            .await?;
            if !status.status.success() {
                return Err(git_error("workspace status", &status));
            }
            if !status.stdout.is_empty() {
                return Err(format!(
                    "uncommitted, untracked or ignored files retained:\n{}",
                    String::from_utf8_lossy(&status.stdout)
                        .chars()
                        .take(2048)
                        .collect::<String>()
                ));
            }
        }
        if !record.branch.starts_with("factory/") {
            return Err("not a Factory branch".into());
        }
        let reference = format!("refs/heads/{}", record.branch);
        let exists = git_output(
            &record.scope_path,
            &["show-ref", "--verify", "--quiet", &reference],
        )
        .await?;
        if exists.status.code() == Some(1) && !record.path.exists() {
            return Ok(()); // Receipt-before-create crash, or completed release.
        }
        if !exists.status.success() {
            return Err(git_error("workspace branch lookup", &exists));
        }
        let head = git_output(&record.scope_path, &["rev-parse", "--verify", &reference]).await?;
        if !head.status.success() {
            return Err(git_error("workspace branch", &head));
        }
        // A commit already on the primary HEAD is safe even without a remote.
        // Otherwise every commit must be reachable from a remote-tracking ref.
        let main = git_output(&record.scope_path, &["rev-parse", "HEAD"]).await?;
        if !record.created && !record.path.exists() {
            return Err("creation was not confirmed; preserving the branch for inspection".into());
        }
        if !main.status.success() {
            return Err(git_error("scope HEAD", &main));
        }
        let count = git_output(
            &record.scope_path,
            &[
                "rev-list",
                "--count",
                &reference,
                "--not",
                "--remotes",
                String::from_utf8_lossy(&main.stdout).trim(),
            ],
        )
        .await?;
        if !count.status.success() {
            return Err(git_error("workspace backup check", &count));
        }
        if String::from_utf8_lossy(&count.stdout).trim() != "0" {
            return Err("unpushed commits retained; push the branch or integrate it into the scope before releasing".into());
        }
        let listing = git_output(&record.scope_path, &["worktree", "list", "--porcelain"]).await?;
        if !listing.status.success() {
            return Err(git_error("workspace registrations", &listing));
        }
        let mut registered_path = None;
        for line in String::from_utf8_lossy(&listing.stdout).lines() {
            if let Some(path) = line.strip_prefix("worktree ") {
                registered_path = Some(PathBuf::from(path));
            }
            if line.strip_prefix("branch ") == Some(reference.as_str())
                && registered_path
                    .as_ref()
                    .and_then(|path| path.canonicalize().ok())
                    != record.path.canonicalize().ok()
            {
                return Err("branch is checked out in another worktree; preserving it".into());
            }
        }
        if record.path.exists() || is_registered(&record.scope_path, &record.path).await {
            let removed = git_output(
                &record.scope_path,
                &[
                    "worktree",
                    "remove",
                    record.path.to_str().ok_or("non-UTF8 workspace")?,
                ],
            )
            .await?;
            if !removed.status.success() {
                return Err(git_error("workspace removal (without force)", &removed));
            }
        }
        // Atomic compare-and-delete: a concurrent new commit preserves the ref.
        let removed = git_output(
            &record.scope_path,
            &[
                "update-ref",
                "-d",
                &reference,
                String::from_utf8_lossy(&head.stdout).trim(),
            ],
        )
        .await?;
        if !removed.status.success() {
            return Err(git_error("workspace branch release", &removed));
        }
        Ok(())
    }

    /// L4 sends eligible identities; L2 owns all deletion and refusal receipts.
    /// Returns only changed outcomes, avoiding a journal flood every tick.
    pub async fn release(
        &self,
        paths: &[PathBuf],
    ) -> Result<Vec<(OwnedWorkspace, Result<(), String>)>, String> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let _guard = self.lock.lock().await;
        let mut records = self.read()?;
        let mut outcomes = Vec::new();
        let mut kept = Vec::new();
        for mut record in records.drain(..) {
            if !paths.contains(&record.path) {
                kept.push(record);
                continue;
            }
            let outcome = self.release_one(&record).await;
            if let Err(reason) = &outcome {
                if record.last_refusal.as_ref() != Some(reason) {
                    outcomes.push((record.clone(), outcome.clone()));
                    record.last_refusal = Some(reason.clone());
                }
                kept.push(record);
            } else {
                outcomes.push((record, outcome));
            }
        }
        if !outcomes.is_empty() {
            self.write(&kept)?;
        }
        Ok(outcomes)
    }
}
