use crate::agent::AgentSession;
use crate::error::Result;
use crate::occupancy::StatusChange;
use crate::run::{NewRun, Run, RunPatch};
use crate::task::{NewTask, Task, TaskEntry, TaskFilter, TaskPatch};

/// Adapter seam 3: where tasks live. The CRUD contract, the runs each task has
/// had, and the journal. Backing this with an issue tracker instead of the
/// built-in sqlite is the point of it being an adapter.
#[async_trait::async_trait]
pub trait TaskStore: Send + Sync {
    fn name(&self) -> &str;

    fn description(&self) -> String {
        format!("{} task store", self.name())
    }

    async fn create(&self, task: &Task) -> Result<Task>;
    async fn get(&self, id: &str) -> Result<Option<Task>>;
    async fn list(&self, filter: &TaskFilter) -> Result<Vec<Task>>;
    async fn update(&self, id: &str, patch: &TaskPatch) -> Result<Task>;
    async fn delete(&self, id: &str) -> Result<bool>;

    // -- runs ---------------------------------------------------------------

    /// Start a run. The store assigns the id and the attempt number; computing
    /// the attempt here rather than in the caller is what keeps a manual
    /// dispatch and a scheduled one from claiming the same number.
    async fn create_run(&self, new: &NewRun) -> Result<Run>;
    async fn get_run(&self, id: &str) -> Result<Option<Run>>;
    async fn update_run(&self, id: &str, patch: &RunPatch) -> Result<Run>;
    /// Newest first.
    async fn runs(&self, task_id: &str, limit: u32) -> Result<Vec<Run>>;
    /// The one run of this task that has not finished, if there is one.
    async fn active_run(&self, task_id: &str) -> Result<Option<Run>>;
    /// Every run anywhere that has not finished. What the watchdog walks.
    async fn active_runs(&self) -> Result<Vec<Run>>;

    // -- standing agents ----------------------------------------------------

    /// Written whole: there are few of them, they change rarely, and a partial
    /// update has no meaning for something whose whole state is "is it up".
    async fn put_agent(&self, agent: &AgentSession) -> Result<()>;
    async fn get_agent(&self, id: &str) -> Result<Option<AgentSession>>;
    async fn agents(&self) -> Result<Vec<AgentSession>>;
    async fn delete_agent(&self, id: &str) -> Result<bool>;

    // -- journal ------------------------------------------------------------

    async fn append_entry(&self, task_id: &str, entry: &TaskEntry) -> Result<()>;
    /// Every entry for a task, across all its runs.
    async fn entries(&self, task_id: &str, limit: u32) -> Result<Vec<TaskEntry>>;
    /// Only the entries from one run.
    async fn run_entries(&self, run_id: &str, limit: u32) -> Result<Vec<TaskEntry>>;

    /// The newest `limit` entries that belong to the task itself and to no
    /// run -- a schedule paused, a slot skipped, a run asked for -- oldest
    /// first. The task modal shows them beside a run's own lines; read
    /// through `entries`, a chatty run's transcript would crowd them out.
    /// The default reads the newest thousand entries and filters them, so
    /// a store without an index for it still answers, if less far back.
    async fn task_own_entries(&self, task_id: &str, limit: u32) -> Result<Vec<TaskEntry>> {
        let own: Vec<TaskEntry> = self
            .entries(task_id, 1000)
            .await?
            .into_iter()
            .filter(|e| e.run_id.is_none())
            .collect();
        let skip = own.len().saturating_sub(limit as usize);
        Ok(own.into_iter().skip(skip).collect())
    }

    /// Every journal entry of one of `kinds` written after `since`, across
    /// all tasks, oldest first, each with the id of the task it is in. What
    /// the Operations projection reads its few journal-only facts from --
    /// passed-over slots, answered blocks -- without walking every task's
    /// whole journal, transcripts and all, on every read (`#106`).
    ///
    /// A store that cannot search its journal this way may answer nothing,
    /// like `status_changes`: the report then shows no missed slots and
    /// counts no answers, a floor rather than a wrong number.
    async fn entries_of_kinds(
        &self,
        _kinds: &[&str],
        _since: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<(String, TaskEntry)>> {
        Ok(Vec::new())
    }

    /// Tasks whose `next_run_at` has come.
    async fn due(&self, now: chrono::DateTime<chrono::Utc>) -> Result<Vec<Task>>;

    // -- liveness history ---------------------------------------------------

    /// Remember that a session's liveness changed. The runtime keeps no past
    /// -- herdr will tell you what an agent is doing now and nothing about
    /// what it was doing an hour ago -- so if this is not written down here,
    /// it is gone.
    ///
    /// A store that does not keep history may drop it: the occupancy chart
    /// then draws runs only, which is the authoritative half anyway.
    async fn append_status(&self, _change: &StatusChange) -> Result<()> {
        Ok(())
    }

    /// Every change from `since` onward, oldest first.
    async fn status_changes(
        &self,
        _since: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<StatusChange>> {
        Ok(Vec::new())
    }

    /// The oldest change on record. Marks where the chart may start claiming
    /// that a quiet agent was idle rather than unobserved.
    async fn status_origin(&self) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        Ok(None)
    }

    /// Every run that overlaps the window, whatever task it belongs to.
    async fn runs_between(
        &self,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<Run>>;
}

/// Fill in the fields a store owns rather than the caller.
pub fn task_from_new(new: NewTask, scope: String, agent: String, runtime: String) -> Task {
    let now = chrono::Utc::now();
    Task {
        id: uuid::Uuid::new_v4().to_string(),
        title: new.title,
        instructions: new.instructions,
        scope,
        agent,
        runtime,
        status: crate::task::TaskStatus::Pending,
        schedule: new.schedule,
        estimate_seconds: new.estimate_seconds,
        result: None,
        error: None,
        runs: 0,
        ack_timeout_seconds: new.ack_timeout_seconds,
        timeout_seconds: new.timeout_seconds,
        blocked_timeout_seconds: new.blocked_timeout_seconds,
        labels: new.labels,
        created_at: now,
        updated_at: now,
        last_run_at: None,
        next_run_at: None,
        // Resolved here, once, rather than left for a reader of `Task` to
        // guess at: `new.worktree` absent means on, and writing the resolved
        // bool down now is what keeps that ambiguity from ever reaching the
        // store. See `Task::worktree` for why an absent key means the
        // opposite once a task already exists.
        worktree: new.worktree.unwrap_or(true),
        knowledge_hints: new.knowledge_hints,
        workflow_origin: None,
        bench_origin: None,
        retry: new.retry,
        pending_retry: None,
        schedule_paused: false,
        category: new.category,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::NewTask;

    #[test]
    fn a_new_task_with_an_absent_worktree_reads_as_on() {
        let new = NewTask {
            title: "do the thing".into(),
            ..Default::default()
        };
        assert_eq!(new.worktree, None, "the wire says nothing either way");
        let task = task_from_new(new, "demo".into(), "shell".into(), "herdr".into());
        assert!(task.worktree, "absent on a new task means on");
    }

    #[test]
    fn a_new_task_that_says_no_worktree_is_believed() {
        let new = NewTask {
            title: "do the thing".into(),
            worktree: Some(false),
            ..Default::default()
        };
        let task = task_from_new(new, "demo".into(), "shell".into(), "herdr".into());
        assert!(!task.worktree, "an explicit no is not the same as silence");
    }

    #[test]
    fn a_new_tasks_estimate_reaches_the_stored_task() {
        let task = task_from_new(
            NewTask {
                title: "do the thing".into(),
                estimate_seconds: Some(900),
                ..Default::default()
            },
            "demo".into(),
            "shell".into(),
            "herdr".into(),
        );
        assert_eq!(task.estimate_seconds, Some(900));
    }
}
