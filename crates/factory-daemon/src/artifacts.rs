//! L4's immutable release artifacts and their append-only provenance (#158).
//! Artifact selection is explicit in a done report; statuses are never guessed.
use crate::l4_service::L4Service;
use factory_core::{
    control_plan,
    error::{FactoryError, Result},
    run::Run,
    task::Task,
};
use factory_kernel::{ArtifactSnapshot, ArtifactSource};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAX_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ARTIFACTS: usize = 16;

fn refused(message: impl Into<String>) -> FactoryError {
    FactoryError::BadRequest(message.into())
}

fn open_artifact(path: &Path) -> Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A file replaced with a FIFO must not hang the daemon while open
        // waits for another endpoint. Canonical targets need no symlink.
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|e| refused(format!("reading artifact {}: {e}", path.display())))?;
    let metadata = file.metadata().map_err(|e| refused(e.to_string()))?;
    if !metadata.is_file() || metadata.len() > MAX_ARTIFACT_BYTES {
        return Err(refused(
            "an artifact must be a regular file no larger than 256 MiB",
        ));
    }
    Ok(file)
}

fn hash_file(path: &Path) -> Result<(String, u64)> {
    let metadata = std::fs::metadata(path).map_err(|e| refused(e.to_string()))?;
    if !metadata.is_file() || metadata.len() > MAX_ARTIFACT_BYTES {
        return Err(refused(
            "an artifact must be a regular file no larger than 256 MiB",
        ));
    }
    let mut file = open_artifact(path)?;
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut bytes = [0u8; 65536];
    loop {
        let n = file.read(&mut bytes).map_err(|e| refused(e.to_string()))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > MAX_ARTIFACT_BYTES {
            return Err(refused("artifact exceeded 256 MiB while reading"));
        }
        hash.update(&bytes[..n]);
    }
    Ok((format!("{:x}", hash.finalize()), total))
}

async fn source(dir: &Path) -> Result<ArtifactSource> {
    // Do not invoke the verifier's non-Git directory fallback here: a
    // release needs a real commit, and arbitrary trees may contain FIFOs.
    let git = tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .await;
    if !matches!(git, Ok(out) if out.status.success() && out.stdout == b"true\n") {
        return Err(refused(
            "artifact provenance requires a readable Git worktree and source digest",
        ));
    }
    let state = crate::verification::git_state(dir).await;
    match (state.commit, state.digest, state.dirty) {
        (Some(commit), Some(worktree_digest), Some(dirty)) => Ok(ArtifactSource {
            commit,
            worktree_digest,
            dirty,
        }),
        _ => Err(refused(
            "artifact provenance requires a readable Git worktree and source digest",
        )),
    }
}

fn artifact_path(dir: &Path, asked: &str) -> Result<PathBuf> {
    let path = std::fs::canonicalize(dir.join(asked))
        .map_err(|e| refused(format!("artifact {asked:?}: {e}")))?;
    let relative = path
        .strip_prefix(dir)
        .map_err(|_| refused("an artifact must be inside this run's working directory"))?;
    if relative
        .components()
        .any(|c| c.as_os_str() == ".git" || c.as_os_str() == ".factory")
    {
        return Err(refused(
            "Git internals and Factory state are not release artifacts",
        ));
    }
    Ok(path)
}

impl L4Service<'_> {
    fn artifact_workdir(&self, run: &Run, scope: &str) -> Result<PathBuf> {
        let dir = match &run.worktree_path {
            Some(path) => PathBuf::from(path),
            None => self.wiring.snapshot().scope_path(scope)?,
        };
        std::fs::canonicalize(dir).map_err(|e| refused(format!("artifact worktree: {e}")))
    }

    pub(crate) async fn capture_artifacts(
        &self,
        run: &Run,
        task: &Task,
        paths: &[String],
    ) -> Result<Vec<ArtifactSnapshot>> {
        if paths.len() > MAX_ARTIFACTS {
            return Err(refused("at most 16 artifacts may be reported at once"));
        }
        let dir = self.artifact_workdir(run, &task.scope)?;
        // Reject every target before reading source bytes or writing any
        // snapshots; in particular never open a directory, device or FIFO.
        let mut names = std::collections::BTreeSet::new();
        for asked in paths {
            let path = artifact_path(&dir, asked)?;
            let metadata = std::fs::metadata(&path).map_err(|e| refused(e.to_string()))?;
            if !metadata.is_file() || metadata.len() > MAX_ARTIFACT_BYTES {
                return Err(refused(
                    "an artifact must be a regular file no larger than 256 MiB",
                ));
            }
            if !names.insert(path.file_name().map(|n| n.to_owned())) {
                return Err(refused("artifact filenames in a report must be unique"));
            }
        }
        let before = source(&dir).await?;
        let root = self.wiring.snapshot().root;
        let run_id = run.id.clone();
        let scope = task.scope.clone();
        let category = control_plan::effective_category(task.category.as_deref()).to_string();
        let paths = paths.to_vec();
        let source_snapshot = before.clone();
        let copy_dir = dir.clone();
        let records = tokio::task::spawn_blocking(move || {
            let mut records = Vec::new();
            let mut names = std::collections::BTreeSet::new();
            for asked in paths {
                let path = artifact_path(&copy_dir, &asked)?;
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| refused("artifact filename must be UTF-8"))?
                    .to_string();
                if !names.insert(name.clone()) {
                    return Err(refused("artifact filenames in a report must be unique"));
                }
                let (sha256, size_bytes) = hash_file(&path)?;
                let id = uuid::Uuid::new_v4().to_string();
                let storage_path = format!(".factory/artifacts/{run_id}/{id}/{name}");
                let stored = root.join(&storage_path);
                std::fs::create_dir_all(stored.parent().unwrap())
                    .map_err(|e| refused(e.to_string()))?;
                let mut output = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&stored)
                    .map_err(|e| refused(e.to_string()))?;
                let input = open_artifact(&path)?;
                std::io::copy(&mut input.take(MAX_ARTIFACT_BYTES + 1), &mut output)
                    .map_err(|e| refused(e.to_string()))?;
                output.flush().map_err(|e| refused(e.to_string()))?;
                output.sync_all().map_err(|e| refused(e.to_string()))?;
                let copied = hash_file(&stored)?;
                if copied != (sha256.clone(), size_bytes) {
                    return Err(refused(
                        "artifact changed while being captured; report it again",
                    ));
                }
                records.push(ArtifactSnapshot {
                    id,
                    name,
                    scope: scope.clone(),
                    category: category.clone(),
                    sha256,
                    size_bytes,
                    storage_path,
                    source_path: path.display().to_string(),
                    source: source_snapshot.clone(),
                });
            }
            Ok::<_, FactoryError>(records)
        })
        .await
        .map_err(|e| refused(format!("artifact capture: {e}")))??;
        if source(&dir).await? != before {
            return Err(refused(
                "source worktree changed while capturing artifacts; report them again",
            ));
        }
        Ok(records)
    }

    /// Validate before publishing. The source, original output and immutable
    /// copy must all still match what the agent explicitly handed in.
    pub(crate) async fn validate_artifacts(
        &self,
        run: &Run,
    ) -> Result<Vec<factory_core::control_plan::StepAttestation>> {
        let Some(first) = run.artifacts.first() else {
            return Ok(Vec::new());
        };
        let dir = self.artifact_workdir(run, &first.scope)?;
        let now = source(&dir).await?;
        if run.artifacts.iter().any(|a| a.source != now) {
            return Err(refused(
                "artifact source worktree changed; rebuild and report artifacts again",
            ));
        }
        let attestations = self.run_attestations(&run.id).await?;
        // Approval authorizes dispatch, not the later build output. Preserve
        // its own pre-dispatch source binding; gates/reviews must match the
        // captured output's source, just as the verifier requires.
        let matching: Vec<_> = attestations
            .iter()
            .filter(|a| {
                a.kind == factory_core::control_plan::StepKind::Approval
                    || a.worktree_digest.as_deref() == Some(now.worktree_digest.as_str())
            })
            .cloned()
            .collect();
        let verdict = control_plan::judge(
            &run.required_steps,
            &matching,
            &run.agent,
            chrono::DateTime::<chrono::Utc>::MIN_UTC,
        );
        if !verdict.passed {
            return Err(refused(format!(
                "artifact provenance requires matching control-plan evidence: {}",
                verdict.reason()
            )));
        }
        let records = run.artifacts.clone();
        let root = self.wiring.snapshot().root;
        tokio::task::spawn_blocking(move || {
            for a in records {
                let original = artifact_path(&dir, &a.source_path)?;
                let expected = (a.sha256, a.size_bytes);
                if hash_file(&original)? != expected
                    || hash_file(&root.join(a.storage_path))? != expected
                {
                    return Err(refused(
                        "artifact bytes changed; rebuild and report artifacts again",
                    ));
                }
            }
            Ok::<_, FactoryError>(())
        })
        .await
        .map_err(|e| refused(format!("artifact validation: {e}")))??;
        Ok(matching)
    }

    pub(crate) async fn publish_artifacts(
        &self,
        run: &Run,
        finished_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<()> {
        if run.artifacts.is_empty() {
            return Ok(());
        }
        let attestations = self.validate_artifacts(run).await?;
        let instance = self.wiring.snapshot().config.instance.id;
        for artifact in &run.artifacts {
            let record = factory_core::provenance::statement(
                run,
                artifact,
                &attestations,
                &instance,
                finished_at,
            );
            self.state.run_evidence.append_provenance(&record).await?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn run_provenance(
        &self,
        id: &str,
    ) -> Result<Vec<factory_kernel::ArtifactProvenance>> {
        factory_kernel::Provide::<factory_kernel::ArtifactProvenance>::get(
            &factory_process::facts::ProvenanceProvider::new(self.state.store.as_ref(), &self.state.run_evidence),
            &id.to_owned(),
        ).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provenance_file_limits_and_canonical_paths_are_checked_without_reading() {
        let root = std::env::temp_dir().join(format!(
            "factory-artifact-path-test-{}",
            uuid::Uuid::new_v4()
        ));
        let dir = root.join("work");
        std::fs::create_dir_all(dir.join(".factory")).unwrap();
        let dir = std::fs::canonicalize(dir).unwrap();
        std::fs::write(root.join("outside"), "private").unwrap();
        std::fs::write(dir.join("release.bin"), "release").unwrap();
        std::fs::write(dir.join(".factory/private"), "private").unwrap();
        assert!(artifact_path(&dir, "../outside").is_err());
        assert!(artifact_path(&dir, ".factory/private").is_err());
        assert!(hash_file(&dir).is_err());
        let large = dir.join("large.bin");
        std::fs::File::create(&large)
            .unwrap()
            .set_len(MAX_ARTIFACT_BYTES + 1)
            .unwrap();
        assert!(hash_file(&large).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("outside"), dir.join("escape")).unwrap();
            assert!(artifact_path(&dir, "escape").is_err());
            let fifo = dir.join("pipe");
            assert!(std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success());
            assert!(hash_file(&fifo).is_err());
            assert!(open_artifact(&fifo).is_err());
        }
    }
}
