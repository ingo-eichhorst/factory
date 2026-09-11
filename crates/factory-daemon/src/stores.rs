//! `ScopedStores`: a `TaskStore` that is really several, chosen by scope.
//!
//! One `Arc<dyn TaskStore>` serves the whole instance today. Rather than teach
//! every one of the ~60 call sites that say `self.store.…` to pick a store for
//! itself, the *value* behind that `Arc` becomes a router: `Engine.store`
//! keeps its type, and one project's tasks can live in an issue tracker while
//! another's stay in the built-in sqlite.
//!
//! Not everything routes. A task can live wherever its scope says, but a run
//! -- an attempt, a token, a session, a started/ended pair -- has no home in
//! an issue tracker, and neither does a journal entry or an `AgentSession`.
//! Those stay on one store for the whole instance, always: that is what keeps
//! `active_runs()` complete, which is what lets the watchdog and the
//! scheduler go on reading `self.store` without ever learning this router
//! exists.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use factory_core::adapter::TaskStore;
use factory_core::agent::AgentSession;
use factory_core::error::Result;
use factory_core::occupancy::StatusChange;
use factory_core::run::{NewRun, Run, RunPatch, RunStatus};
use factory_core::task::{Task, TaskEntry, TaskFilter, TaskPatch};

pub struct ScopedStores {
    /// Where runs, the journal, standing agents and liveness live, whatever
    /// store holds the task itself.
    ledger: Arc<dyn TaskStore>,
    /// scope name -> the store that holds its tasks.
    by_scope: HashMap<String, Arc<dyn TaskStore>>,
    /// Every distinct store, ledger first. What a question with no scope in
    /// it has to be asked of.
    all: Vec<Arc<dyn TaskStore>>,
}

impl ScopedStores {
    /// `ledger` is the instance default; `by_scope` names only the scopes that
    /// chose something else.
    pub fn new(ledger: Arc<dyn TaskStore>, by_scope: HashMap<String, Arc<dyn TaskStore>>) -> Self {
        let mut all = vec![ledger.clone()];
        // Sorted so two daemons built from the same config end up with the
        // same `all`, and a `description()` written from it reads the same
        // way on every run rather than shuffling with HashMap iteration order.
        let mut scopes: Vec<&String> = by_scope.keys().collect();
        scopes.sort();
        for scope in scopes {
            let store = &by_scope[scope];
            if all.iter().any(|s| s.name() == store.name()) {
                continue;
            }
            all.push(store.clone());
        }
        Self {
            ledger,
            by_scope,
            all,
        }
    }

    fn store_for(&self, scope: &str) -> &Arc<dyn TaskStore> {
        self.by_scope.get(scope).unwrap_or(&self.ledger)
    }

    /// Who holds `id`, fanning out over `all` in the order it was built and
    /// stopping at the first hit. `None` when nobody does.
    ///
    /// A single configured store has nowhere else the task could be, so it is
    /// asked directly rather than walked as a one-element list -- the early
    /// return every other method's "one store" behaviour rides on.
    async fn locate(&self, id: &str) -> Result<Option<(Arc<dyn TaskStore>, Task)>> {
        if self.all.len() == 1 {
            return Ok(self
                .ledger
                .get(id)
                .await?
                .map(|task| (self.ledger.clone(), task)));
        }
        for store in &self.all {
            if let Some(task) = store.get(id).await? {
                return Ok(Some((store.clone(), task)));
            }
        }
        Ok(None)
    }
}

#[async_trait::async_trait]
impl TaskStore for ScopedStores {
    fn name(&self) -> &str {
        "scoped"
    }

    fn description(&self) -> String {
        let engines: Vec<&str> = self.all.iter().map(|s| s.name()).collect();
        let mut scopes: Vec<String> = self
            .by_scope
            .iter()
            .map(|(scope, store)| format!("{scope} on {}", store.name()))
            .collect();
        scopes.sort();
        if scopes.is_empty() {
            return format!("tasks in {}, for every scope", engines.join(", "));
        }
        format!(
            "tasks in {}; runs and the journal in {}; {}",
            engines.join(" and "),
            self.ledger.name(),
            scopes.join(", ")
        )
    }

    async fn create(&self, task: &Task) -> Result<Task> {
        self.store_for(&task.scope).create(task).await
    }

    async fn get(&self, id: &str) -> Result<Option<Task>> {
        Ok(self.locate(id).await?.map(|(_, task)| task))
    }

    async fn list(&self, filter: &TaskFilter) -> Result<Vec<Task>> {
        if let Some(scope) = &filter.scope {
            return self.store_for(scope).list(filter).await;
        }
        let mut tasks = Vec::new();
        for store in &self.all {
            tasks.extend(store.list(filter).await?);
        }
        // Each store already applied `limit` to its own share, so a merge of
        // several already-truncated lists is not itself truncated: sort first
        // and cut after, or a limit of 1 could bury the newest task behind an
        // older one that merely came from a different store.
        tasks.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        if let Some(limit) = filter.limit {
            tasks.truncate(limit as usize);
        }
        Ok(tasks)
    }

    async fn update(&self, id: &str, patch: &TaskPatch) -> Result<Task> {
        match self.locate(id).await? {
            Some((store, _)) => store.update(id, patch).await,
            // Nobody claims it. Ask the ledger anyway, so the caller gets the
            // ordinary TaskNotFound rather than a shape only this router
            // invents.
            None => self.ledger.update(id, patch).await,
        }
    }

    async fn delete(&self, id: &str) -> Result<bool> {
        let Some((store, _)) = self.locate(id).await? else {
            return Ok(false);
        };
        let gone = store.delete(id).await?;
        // Deleting a task takes its runs and its journal with it -- that is
        // what a store's `delete` means, and the sqlite one does it in the
        // same breath. When the task lived somewhere else, the ledger still
        // holds both, so it has to be told too, or a deleted task leaves runs
        // behind that nothing can name and the chart draws forever as
        // "(deleted task)".
        if store.name() != self.ledger.name() {
            let _ = self.ledger.delete(id).await;
        }
        Ok(gone)
    }

    async fn create_run(&self, new: &NewRun) -> Result<Run> {
        let run = self.ledger.create_run(new).await?;
        // The ledger's own transaction already mirrored the task if the
        // ledger is who holds it -- the common case, and done here. Only a
        // task owned by some other store needs a second write, since the
        // ledger cannot reach into a table it does not have.
        if let Some((owner, _)) = self.locate(&new.task_id).await? {
            if owner.name() != self.ledger.name() {
                let patch = TaskPatch {
                    runs: Some(run.attempt),
                    status: Some(RunStatus::Dispatching.as_task_status()),
                    last_run_at: Some(run.started_at),
                    ..Default::default()
                };
                // Best effort: the run itself is already durable on the
                // ledger, which is what active_runs() and the watchdog read.
                // A mirror that fails leaves a list stale, not a run lost.
                let _ = owner.update(&new.task_id, &patch).await;
            }
        }
        Ok(run)
    }

    async fn due(&self, now: DateTime<Utc>) -> Result<Vec<Task>> {
        let mut tasks = Vec::new();
        for store in &self.all {
            tasks.extend(store.due(now).await?);
        }
        Ok(tasks)
    }

    // -- delegated to the ledger, unconditionally ---------------------------
    //
    // A run, a journal entry and an AgentSession have no home in an issue
    // tracker or anywhere else a task's own store might be -- a task can live
    // in one, but an attempt at it, a line in its journal, and a standing
    // agent's session cannot. Keeping all of it on one store, always, is what
    // lets active_runs() stay complete without this router having to fan
    // anything out, which is the whole reason the watchdog and the scheduler
    // do not need to know it exists.

    async fn get_run(&self, id: &str) -> Result<Option<Run>> {
        self.ledger.get_run(id).await
    }

    async fn update_run(&self, id: &str, patch: &RunPatch) -> Result<Run> {
        self.ledger.update_run(id, patch).await
    }

    async fn runs(&self, task_id: &str, limit: u32) -> Result<Vec<Run>> {
        self.ledger.runs(task_id, limit).await
    }

    async fn active_run(&self, task_id: &str) -> Result<Option<Run>> {
        self.ledger.active_run(task_id).await
    }

    async fn active_runs(&self) -> Result<Vec<Run>> {
        self.ledger.active_runs().await
    }

    async fn put_agent(&self, agent: &AgentSession) -> Result<()> {
        self.ledger.put_agent(agent).await
    }

    async fn get_agent(&self, id: &str) -> Result<Option<AgentSession>> {
        self.ledger.get_agent(id).await
    }

    async fn agents(&self) -> Result<Vec<AgentSession>> {
        self.ledger.agents().await
    }

    async fn delete_agent(&self, id: &str) -> Result<bool> {
        self.ledger.delete_agent(id).await
    }

    async fn append_entry(&self, task_id: &str, entry: &TaskEntry) -> Result<()> {
        self.ledger.append_entry(task_id, entry).await
    }

    async fn entries(&self, task_id: &str, limit: u32) -> Result<Vec<TaskEntry>> {
        self.ledger.entries(task_id, limit).await
    }

    async fn run_entries(&self, run_id: &str, limit: u32) -> Result<Vec<TaskEntry>> {
        self.ledger.run_entries(run_id, limit).await
    }

    async fn append_status(&self, change: &StatusChange) -> Result<()> {
        self.ledger.append_status(change).await
    }

    async fn status_changes(&self, since: DateTime<Utc>) -> Result<Vec<StatusChange>> {
        self.ledger.status_changes(since).await
    }

    async fn status_origin(&self) -> Result<Option<DateTime<Utc>>> {
        self.ledger.status_origin().await
    }

    async fn runs_between(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Vec<Run>> {
        self.ledger.runs_between(from, to).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_core::run::Trigger;
    use factory_core::task::TaskStatus;
    use factory_plugins::SqliteStore;

    // A store of its own per scope, rather than the delegating fake the
    // packet for this file expected: `store_sqlite.rs` grew
    // `in_memory_named` while this file was being written (see its own
    // `two_in_memory_named_stores_...` test), which gives two genuine,
    // distinguishable sqlite stores for free. That is a better fake than a
    // hand-written delegate -- it is not a fake at all -- so there is no
    // `Fake` type here.
    fn store(name: &str) -> Arc<dyn TaskStore> {
        Arc::new(SqliteStore::in_memory_named(name).unwrap())
    }

    fn task(id: &str, scope: &str, created_secs: i64) -> Task {
        let at = DateTime::from_timestamp(1_700_000_000 + created_secs, 0).unwrap();
        Task {
            id: id.to_string(),
            title: id.to_string(),
            instructions: "do it".into(),
            scope: scope.to_string(),
            agent: "assistant".into(),
            runtime: "shell".into(),
            status: TaskStatus::Pending,
            schedule: None,
            result: None,
            error: None,
            runs: 0,
            ack_timeout_seconds: None,
            timeout_seconds: None,
            labels: Default::default(),
            created_at: at,
            updated_at: at,
            last_run_at: None,
            next_run_at: None,
        }
    }

    fn new_run(task_id: &str) -> NewRun {
        NewRun {
            task_id: task_id.to_string(),
            trigger: Trigger::Manual,
            agent: "assistant".into(),
            adapter: "shell".into(),
            runtime: "shell".into(),
            token: "tok".into(),
        }
    }

    /// The default and scope "b" backed by two distinct sqlite databases;
    /// "b" is the only scope not on the default.
    fn two_stores() -> ScopedStores {
        let ledger = store("ledger");
        let b = store("b");
        let mut by_scope = HashMap::new();
        by_scope.insert("b".to_string(), b);
        ScopedStores::new(ledger, by_scope)
    }

    #[tokio::test]
    async fn a_task_created_in_a_scope_lands_only_in_that_scopes_store() {
        let ledger = store("ledger");
        let b = store("b");
        let mut by_scope = HashMap::new();
        by_scope.insert("b".to_string(), b.clone());
        let scoped = ScopedStores::new(ledger.clone(), by_scope);

        scoped.create(&task("t1", "b", 0)).await.unwrap();

        assert!(b.get("t1").await.unwrap().is_some());
        assert!(ledger.get("t1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn get_finds_a_task_in_a_non_default_store() {
        let scoped = two_stores();
        scoped.create(&task("t1", "b", 0)).await.unwrap();

        let found = scoped.get("t1").await.unwrap().unwrap();
        assert_eq!(found.id, "t1");
    }

    #[tokio::test]
    async fn list_with_no_filter_returns_tasks_from_both_newest_first() {
        let scoped = two_stores();
        scoped.create(&task("old", "default", 0)).await.unwrap();
        scoped.create(&task("new", "b", 10)).await.unwrap();

        let listed = scoped.list(&TaskFilter::default()).await.unwrap();
        let ids: Vec<&str> = listed.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["new", "old"]);
    }

    #[tokio::test]
    async fn list_with_a_scope_filter_asks_only_that_scopes_store() {
        let scoped = two_stores();
        scoped.create(&task("in-default", "default", 0)).await.unwrap();
        scoped.create(&task("in-b", "b", 10)).await.unwrap();

        let filter = TaskFilter {
            scope: Some("b".into()),
            ..Default::default()
        };
        let listed = scoped.list(&filter).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "in-b");
    }

    #[tokio::test]
    async fn list_with_a_limit_truncates_after_the_merge_not_per_store() {
        let scoped = two_stores();
        // One store alone would satisfy a limit of 1 from its own tasks; the
        // truncation has to run again after the merge or the newest overall
        // task can be dropped in favour of an older one from another store.
        scoped.create(&task("default-old", "default", 0)).await.unwrap();
        scoped.create(&task("b-newest", "b", 20)).await.unwrap();
        scoped.create(&task("default-newer", "default", 10)).await.unwrap();

        let filter = TaskFilter {
            limit: Some(1),
            ..Default::default()
        };
        let listed = scoped.list(&filter).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "b-newest");
    }

    #[tokio::test]
    async fn due_returns_tasks_from_both_stores() {
        let scoped = two_stores();
        let past = DateTime::from_timestamp(1_700_000_000 - 60, 0).unwrap();
        let mut in_default = task("due-default", "default", 0);
        in_default.next_run_at = Some(past);
        let mut in_b = task("due-b", "b", 0);
        in_b.next_run_at = Some(past);
        scoped.create(&in_default).await.unwrap();
        scoped.create(&in_b).await.unwrap();

        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let due = scoped.due(now).await.unwrap();
        let ids: std::collections::HashSet<&str> = due.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains("due-default"));
        assert!(ids.contains("due-b"));
    }

    #[tokio::test]
    async fn update_reaches_a_task_in_a_non_default_store() {
        let scoped = two_stores();
        scoped.create(&task("t1", "b", 0)).await.unwrap();

        let patch = TaskPatch {
            title: Some("changed".into()),
            ..Default::default()
        };
        let updated = scoped.update("t1", &patch).await.unwrap();
        assert_eq!(updated.title, "changed");
    }

    #[tokio::test]
    async fn delete_reaches_a_task_in_a_non_default_store() {
        let scoped = two_stores();
        scoped.create(&task("t1", "b", 0)).await.unwrap();

        assert!(scoped.delete("t1").await.unwrap());
        assert!(scoped.get("t1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn deleting_a_task_in_another_store_takes_its_runs_out_of_the_ledger() {
        let ledger = store("ledger");
        let b = store("b");
        let mut by_scope = HashMap::new();
        by_scope.insert("b".to_string(), b.clone());
        let scoped = ScopedStores::new(ledger.clone(), by_scope);

        scoped.create(&task("t1", "b", 0)).await.unwrap();
        scoped.create_run(&new_run("t1")).await.unwrap();
        assert_eq!(ledger.runs("t1", 10).await.unwrap().len(), 1);

        assert!(scoped.delete("t1").await.unwrap());
        // Otherwise the run outlives the task that named it, and the
        // occupancy chart draws it forever as "(deleted task)".
        assert!(ledger.runs("t1", 10).await.unwrap().is_empty());
        assert!(scoped.active_runs().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn update_on_an_id_nobody_has_still_errors() {
        let scoped = two_stores();
        let patch = TaskPatch::default();
        let err = scoped.update("nope", &patch).await.unwrap_err();
        assert_eq!(err.code(), "task_not_found");
    }

    #[tokio::test]
    async fn delete_on_an_id_nobody_has_returns_false() {
        let scoped = two_stores();
        assert!(!scoped.delete("nope").await.unwrap());
    }

    #[tokio::test]
    async fn a_run_for_a_task_in_a_non_default_store_is_written_to_the_ledger_and_bumps_that_tasks_mirror(
    ) {
        let ledger = store("ledger");
        let b = store("b");
        let mut by_scope = HashMap::new();
        by_scope.insert("b".to_string(), b.clone());
        let scoped = ScopedStores::new(ledger.clone(), by_scope);
        scoped.create(&task("t1", "b", 0)).await.unwrap();

        let run = scoped.create_run(&new_run("t1")).await.unwrap();
        assert_eq!(run.attempt, 1);

        // The run lives on the ledger, which does not hold the task itself.
        assert!(ledger.get_run(&run.id).await.unwrap().is_some());
        assert!(ledger.get("t1").await.unwrap().is_none());

        // The task's own store carries the mirror the ledger could not apply.
        let mirrored = b.get("t1").await.unwrap().unwrap();
        assert_eq!(mirrored.runs, 1);
        assert_eq!(mirrored.status, TaskStatus::Dispatching);
        assert_eq!(mirrored.last_run_at, Some(run.started_at));
    }

    #[tokio::test]
    async fn with_one_store_configured_everything_behaves_exactly_as_that_store_alone_would() {
        let only = store("only");
        let scoped = ScopedStores::new(only.clone(), HashMap::new());

        scoped.create(&task("t1", "anywhere", 0)).await.unwrap();
        assert!(only.get("t1").await.unwrap().is_some());

        let run = scoped.create_run(&new_run("t1")).await.unwrap();
        assert_eq!(run.attempt, 1);
        let mirrored = scoped.get("t1").await.unwrap().unwrap();
        assert_eq!(mirrored.runs, 1);
        assert_eq!(mirrored.status, TaskStatus::Dispatching);

        assert!(scoped.delete("t1").await.unwrap());
        assert!(only.get("t1").await.unwrap().is_none());
    }
}
