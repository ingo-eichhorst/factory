//! Where dataset requests are served. `<root>/.factory/datasets/<name>.yaml`
//! is the source of truth -- every read here re-parses it, so a hand edit
//! shows up on the next request without anything having to notice it
//! changed. Everything about *what* a dataset means (parsing, validation,
//! the bulk importers, task -> case conversion) lives in
//! `factory_core::dataset`, pure and tested on its own; this module is only
//! the file and the one lock per dataset that keeps two writers from tearing
//! it in half.

use crate::engine::Engine;
use factory_core::dataset::{Case, Dataset, DatasetFinding, DatasetSummary};
use factory_core::error::{FactoryError, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;

/// One async lock per dataset name, created on first use and never removed
/// -- an instance names very few distinct datasets over its life, so there
/// is nothing worth reclaiming here.
#[derive(Default)]
pub struct DatasetLocks {
    by_name: std::sync::Mutex<BTreeMap<String, Arc<AsyncMutex<()>>>>,
}

impl DatasetLocks {
    fn lock_for(&self, name: &str) -> Arc<AsyncMutex<()>> {
        self.by_name
            .lock()
            .unwrap()
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }
}

fn refuse_bad_name(name: &str) -> Result<()> {
    if factory_core::dataset::is_slug(name) {
        Ok(())
    } else {
        Err(FactoryError::BadRequest(format!(
            "dataset name {name:?} must match [a-z0-9][a-z0-9-]*"
        )))
    }
}

impl Engine {
    pub(crate) fn known_scope_names(&self) -> BTreeSet<String> {
        self.factory_snapshot()
            .config
            .scopes
            .iter()
            .map(|s| s.name.clone())
            .collect()
    }

    /// Every dataset file in `<root>/.factory/datasets/`, by name, sorted.
    fn dataset_names(&self) -> Vec<String> {
        let dir = self.factory_snapshot().datasets_dir();
        let Ok(read) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut names: Vec<String> = read
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                    return None;
                }
                path.file_stem().map(|s| s.to_string_lossy().into_owned())
            })
            .collect();
        names.sort();
        names
    }

    pub(crate) fn dataset_summaries(&self) -> Result<Vec<DatasetSummary>> {
        let known = self.known_scope_names();
        let dir = self.factory_snapshot().datasets_dir();
        let mut out = Vec::new();
        for name in self.dataset_names() {
            if let Some(ds) = Dataset::load(&dir, &name)? {
                out.push(factory_core::dataset::summarize(&ds, &known));
            }
        }
        Ok(out)
    }

    pub(crate) fn dataset_view(&self, name: &str) -> Result<(Dataset, Vec<DatasetFinding>)> {
        let dir = self.factory_snapshot().datasets_dir();
        let dataset = Dataset::load(&dir, name)?
            .ok_or_else(|| FactoryError::BadRequest(format!("no such dataset: {name:?}")))?;
        let findings = factory_core::dataset::findings(&dataset, &self.known_scope_names());
        Ok((dataset, findings))
    }

    pub(crate) async fn dataset_create(&self, name: &str, description: Option<String>) -> Result<Dataset> {
        refuse_bad_name(name)?;
        let lock = self.dataset_locks.lock_for(name);
        let _guard = lock.lock().await;
        let dir = self.factory_snapshot().datasets_dir();
        if Dataset::load(&dir, name)?.is_some() {
            return Err(FactoryError::BadRequest(format!("dataset {name:?} already exists")));
        }
        let dataset = Dataset::new(name, description);
        dataset.validate()?;
        dataset.write_atomic(&dir)?;
        Ok(dataset)
    }

    pub(crate) async fn dataset_add_cases(&self, name: &str, cases: Vec<Case>) -> Result<Dataset> {
        let lock = self.dataset_locks.lock_for(name);
        let _guard = lock.lock().await;
        let dir = self.factory_snapshot().datasets_dir();
        let mut dataset = Dataset::load(&dir, name)?
            .ok_or_else(|| FactoryError::BadRequest(format!("no such dataset: {name:?}")))?;
        dataset.cases.extend(cases);
        dataset.validate()?;
        dataset.revision += 1;
        dataset.write_atomic(&dir)?;
        Ok(dataset)
    }

    pub(crate) async fn dataset_import(
        &self,
        name: &str,
        format: &str,
        content: &str,
        replace: bool,
    ) -> Result<Dataset> {
        let cases = factory_core::dataset::import(format, content).map_err(|problems| {
            FactoryError::BadRequest(
                problems.iter().map(|p| p.to_string()).collect::<Vec<_>>().join("; "),
            )
        })?;

        let lock = self.dataset_locks.lock_for(name);
        let _guard = lock.lock().await;
        let dir = self.factory_snapshot().datasets_dir();
        let mut dataset = match Dataset::load(&dir, name)? {
            Some(existing) => existing,
            None => Dataset::new(name, None),
        };
        if replace {
            dataset.cases = cases;
        } else {
            // A duplicate against what the dataset already has is caught by
            // `validate()` below, the same as a hand-edited file would be.
            dataset.cases.extend(cases);
        }
        dataset.validate()?;
        dataset.revision += 1;
        dataset.write_atomic(&dir)?;
        Ok(dataset)
    }

    pub(crate) async fn dataset_from_tasks(&self, name: &str, task_ids: Vec<String>) -> Result<Dataset> {
        let lock = self.dataset_locks.lock_for(name);
        let _guard = lock.lock().await;
        let dir = self.factory_snapshot().datasets_dir();
        let mut dataset = match Dataset::load(&dir, name)? {
            Some(existing) => existing,
            None => Dataset::new(name, None),
        };

        let mut taken: BTreeSet<String> = dataset.cases.iter().map(|c| c.id.clone()).collect();
        let factory = self.factory_snapshot();
        let now = chrono::Utc::now();

        for task_id in &task_ids {
            let task = self
                .store
                .get(task_id)
                .await?
                .ok_or_else(|| FactoryError::BadRequest(format!("no such task: {task_id:?}")))?;
            let newest_run = self.store.runs(task_id, 1).await?.into_iter().next();
            let outcome = newest_run
                .as_ref()
                .map(|r| r.status.as_str().to_string())
                .unwrap_or_else(|| task.status.as_str().to_string());
            let run_id = newest_run.as_ref().map(|r| r.id.clone()).unwrap_or_default();

            let base = match &newest_run {
                Some(run) => match (&run.worktree_branch, factory.scope_path(&task.scope).ok()) {
                    (Some(branch), Some(scope_path)) => merge_base_if_branch_exists(&scope_path, branch).await,
                    _ => None,
                },
                None => None,
            };

            let id = factory_core::dataset::next_case_id(&task.title, &mut taken);
            dataset.cases.push(factory_core::dataset::case_from_task(
                id,
                &task.title,
                &task.scope,
                &task.instructions,
                task_id,
                &run_id,
                &outcome,
                now,
                base,
            ));
        }

        dataset.validate()?;
        dataset.revision += 1;
        dataset.write_atomic(&dir)?;
        Ok(dataset)
    }

    pub(crate) async fn dataset_delete_case(&self, name: &str, id: &str) -> Result<Dataset> {
        let lock = self.dataset_locks.lock_for(name);
        let _guard = lock.lock().await;
        let dir = self.factory_snapshot().datasets_dir();
        let mut dataset = Dataset::load(&dir, name)?
            .ok_or_else(|| FactoryError::BadRequest(format!("no such dataset: {name:?}")))?;
        let before = dataset.cases.len();
        dataset.cases.retain(|c| c.id != id);
        if dataset.cases.len() == before {
            return Err(FactoryError::BadRequest(format!("dataset {name:?} has no case {id:?}")));
        }
        dataset.revision += 1;
        dataset.write_atomic(&dir)?;
        Ok(dataset)
    }

    pub(crate) async fn dataset_delete(&self, name: &str) -> Result<bool> {
        let lock = self.dataset_locks.lock_for(name);
        let _guard = lock.lock().await;
        let dir = self.factory_snapshot().datasets_dir();
        if Dataset::load(&dir, name)?.is_none() {
            return Ok(false);
        }
        std::fs::remove_file(dir.join(format!("{name}.yaml")))
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("deleting dataset {name:?}: {e}")))?;
        Ok(true)
    }
}

/// `None` when `branch` no longer exists in `scope_path`; otherwise
/// `git merge-base <branch> HEAD`, or `None` again if that call itself
/// fails (an empty repository, a detached and orphaned HEAD, and the like --
/// all read the same way as "no pinned base" rather than as an error, since
/// `base` absent is always a legal case).
async fn merge_base_if_branch_exists(scope_path: &Path, branch: &str) -> Option<String> {
    let exists = tokio::process::Command::new("git")
        .arg("-C")
        .arg(scope_path)
        .args(["rev-parse", "--verify", "-q", &format!("refs/heads/{branch}")])
        .output()
        .await
        .ok()?
        .status
        .success();
    if !exists {
        return None;
    }
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(scope_path)
        .args(["merge-base", branch, "HEAD"])
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
