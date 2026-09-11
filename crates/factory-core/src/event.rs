use crate::run::Run;
use crate::task::{Task, TaskEntry};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Everything an observer can see. One stream feeds the WebSocket, the CLI's
/// `watch`, and anything else that subscribes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    DaemonStarted {
        at: DateTime<Utc>,
        instance: String,
    },
    TaskCreated {
        task: Task,
    },
    TaskUpdated {
        task: Task,
    },
    TaskDeleted {
        id: String,
    },
    /// A journal line was appended. Carries the task id so a UI can route it,
    /// and the entry carries the run it belongs to.
    TaskEntry {
        id: String,
        entry: TaskEntry,
    },
    RunStarted {
        run: Run,
    },
    RunUpdated {
        run: Run,
    },
}

impl Event {
    /// Events carry whole runs, and a run carries the callback token that is
    /// the only thing stopping one agent from closing another's. Strip it
    /// before the event leaves the daemon.
    pub fn redacted(self) -> Event {
        match self {
            Event::RunStarted { run } => Event::RunStarted {
                run: run.redacted(),
            },
            Event::RunUpdated { run } => Event::RunUpdated {
                run: run.redacted(),
            },
            other => other,
        }
    }

    pub fn task_id(&self) -> Option<&str> {
        match self {
            Event::TaskCreated { task } | Event::TaskUpdated { task } => Some(&task.id),
            Event::TaskDeleted { id } | Event::TaskEntry { id, .. } => Some(id),
            Event::RunStarted { run } | Event::RunUpdated { run } => Some(&run.task_id),
            Event::DaemonStarted { .. } => None,
        }
    }
}

/// The daemon's fan-out. Subscribers that fall behind lose the oldest events
/// rather than stalling the daemon; a UI recovers by re-listing tasks.
#[derive(Clone)]
pub struct EventBus {
    tx: tokio::sync::broadcast::Sender<Event>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = tokio::sync::broadcast::channel(capacity);
        Self { tx }
    }

    pub fn publish(&self, event: Event) {
        // No subscribers is the normal case for a headless daemon, not an error.
        let _ = self.tx.send(event.redacted());
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(1024)
    }
}
