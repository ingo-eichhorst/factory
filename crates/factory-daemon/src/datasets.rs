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

/// Checked at every daemon entry point that takes a dataset `name` off the
/// wire, before it ever reaches `factory_core::dataset` (which refuses it
/// again itself -- defense in depth, since `name` becomes a path component
/// and `../elsewhere` or an absolute path is otherwise a legal string).
pub(crate) fn refuse_bad_name(name: &str) -> Result<()> {
    if factory_core::dataset::is_slug(name) {
        Ok(())
    } else {
        Err(FactoryError::BadRequest(format!(
            "dataset name {name:?} must match [a-z0-9][a-z0-9-]*"
        )))
    }
}

/// The same, for a case `id` taken off the wire on its own (`delete_case`) --
/// every other entry point either generates the id itself or validates it as
/// part of the whole dataset via `Dataset::validate`.
fn refuse_bad_case_id(id: &str) -> Result<()> {
    if factory_core::dataset::is_slug(id) {
        Ok(())
    } else {
        Err(FactoryError::BadRequest(format!(
            "case id {id:?} must match [a-z0-9][a-z0-9-]*"
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
        refuse_bad_name(name)?;
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
        let mut dataset = Dataset::new(name, description);
        // `Dataset::new` starts at 0 -- the value of a dataset that has never
        // been written. This call is itself the first write Factory makes,
        // so it bumps to 1, the same as every other write here does.
        dataset.revision += 1;
        dataset.validate()?;
        dataset.write_atomic(&dir)?;
        Ok(dataset)
    }

    pub(crate) async fn dataset_add_cases(&self, name: &str, cases: Vec<Case>) -> Result<Dataset> {
        refuse_bad_name(name)?;
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
        refuse_bad_name(name)?;
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
        refuse_bad_name(name)?;
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
        refuse_bad_name(name)?;
        refuse_bad_case_id(id)?;
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
        refuse_bad_name(name)?;
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

#[cfg(test)]
mod tests {
    //! `refuse_bad_name` at every entry point, and `refuse_bad_case_id` in
    //! `dataset_delete_case` -- reported by QA, reproduced live:
    //! `DELETE /api/datasets/..%2Foutside` deleted a file outside the
    //! datasets directory entirely, and `GET /api/datasets/..%2Fconfig` read
    //! `.factory/config.yaml` back as a dataset parse error. Every one of
    //! these is checked here directly against the engine method, not just
    //! through HTTP, since `factory_core::dataset::load`/`write_atomic`
    //! refusing the same name too (see that module's own tests) is defense
    //! in depth behind this, not a replacement for it.

    use super::*;
    use factory_core::adapter::TaskStore;
    use factory_core::config::{Config, DaemonConfig, Factory, Instance};
    use factory_plugins::{Registry, SqliteStore};
    use std::path::PathBuf;

    fn test_engine() -> Arc<Engine> {
        let root = std::env::temp_dir().join(format!("factory-datasets-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".factory")).unwrap();
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig::default(),
            roles: Default::default(),
            dashboard: None,
            policies: Default::default(),
            quality: Default::default(),
            scope: None,
            scopes: Vec::new(),
            infrastructure: Default::default(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let factory = Factory { root, config };
        let registry = Registry::with_builtins();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        Arc::new(Engine::new(factory, registry, store, PathBuf::from("factory"), Vec::new()))
    }

    fn escaping_name(engine: &Engine) -> String {
        // A dataset-shaped file one level above the datasets directory --
        // exactly where `../outside` would resolve to.
        let dir = engine.factory_snapshot().factory_dir();
        std::fs::write(dir.join("outside.yaml"), "name: outside\nversion: 1\ncases: []\n").unwrap();
        "../outside".to_string()
    }

    #[tokio::test]
    async fn every_read_and_write_entry_point_refuses_a_name_that_would_escape_the_datasets_directory() {
        let engine = test_engine();
        let bad = escaping_name(&engine);

        let e = engine.dataset_view(&bad).unwrap_err().to_string();
        assert!(e.contains("must match"), "{e}");

        let e = engine.dataset_add_cases(&bad, Vec::new()).await.unwrap_err().to_string();
        assert!(e.contains("must match"), "{e}");

        let e = engine
            .dataset_import(&bad, "jsonl", "", false)
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("must match"), "{e}");

        let e = engine.dataset_from_tasks(&bad, Vec::new()).await.unwrap_err().to_string();
        assert!(e.contains("must match"), "{e}");

        let e = engine.dataset_delete_case(&bad, "some-id").await.unwrap_err().to_string();
        assert!(e.contains("must match"), "{e}");

        let e = engine.dataset_delete(&bad).await.unwrap_err().to_string();
        assert!(e.contains("must match"), "{e}");

        // The file this test planted must still be exactly where it was --
        // never read, never deleted, by any of the calls above.
        let dir = engine.factory_snapshot().factory_dir();
        assert!(dir.join("outside.yaml").exists(), "the file outside the datasets directory must survive untouched");
    }

    #[tokio::test]
    async fn dataset_delete_case_also_refuses_a_bad_case_id() {
        let engine = test_engine();
        engine.dataset_create("demo", None).await.unwrap();
        let e = engine
            .dataset_delete_case("demo", "../elsewhere")
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("must match"), "{e}");
    }

    /// Reported by QA: `Dataset::new` starts at revision 0, and
    /// `dataset_create` never bumped it -- so a freshly created dataset
    /// stayed at `revision: 0` even though the create itself is a write.
    /// Every other write here (`dataset_add_cases`, `dataset_import`,
    /// `dataset_from_tasks`) already bumps to 1 on a dataset's first write,
    /// via the same `Dataset::new` plus `+= 1`; this is the one that did not.
    #[tokio::test]
    async fn dataset_create_leaves_a_fresh_dataset_at_revision_one() {
        let engine = test_engine();
        let ds = engine.dataset_create("probe", None).await.unwrap();
        assert_eq!(ds.revision, 1, "{ds:?}");
        // And the same read back off disk, not only the in-memory answer.
        let (loaded, _findings) = engine.dataset_view("probe").unwrap();
        assert_eq!(loaded.revision, 1, "{loaded:?}");
    }
}
