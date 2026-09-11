use crate::error::Result;
use crate::task::{NewTask, Task, TaskEntry, TaskFilter, TaskPatch};

/// Adapter seam 3: where tasks live. The CRUD contract, plus the per-task
/// journal. Backing this with an issue tracker instead of the built-in sqlite
/// is the point of it being an adapter.
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

    async fn append_entry(&self, id: &str, entry: &TaskEntry) -> Result<()>;
    async fn entries(&self, id: &str, limit: u32) -> Result<Vec<TaskEntry>>;

    /// Tasks whose `next_run_at` has come. Kept on the trait so a store that
    /// can answer it with a query does not have to be paged through.
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
        session: None,
        result: None,
        error: None,
        token: None,
        labels: new.labels,
        created_at: now,
        updated_at: now,
        last_run_at: None,
        next_run_at: None,
    }
}
