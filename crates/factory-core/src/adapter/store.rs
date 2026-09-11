use crate::error::Result;
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

    // -- journal ------------------------------------------------------------

    async fn append_entry(&self, task_id: &str, entry: &TaskEntry) -> Result<()>;
    /// Every entry for a task, across all its runs.
    async fn entries(&self, task_id: &str, limit: u32) -> Result<Vec<TaskEntry>>;
    /// Only the entries from one run.
    async fn run_entries(&self, run_id: &str, limit: u32) -> Result<Vec<TaskEntry>>;

    /// Tasks whose `next_run_at` has come.
    async fn due(&self, now: chrono::DateTime<chrono::Utc>) -> Result<Vec<Task>>;
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
        result: None,
        error: None,
        runs: 0,
        labels: new.labels,
        created_at: now,
        updated_at: now,
        last_run_at: None,
        next_run_at: None,
    }
}
