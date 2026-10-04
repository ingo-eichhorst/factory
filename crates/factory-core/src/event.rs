use crate::adapter::RuntimeStatus;
use crate::agent::AgentSession;
use crate::goals::KrRef;
use crate::policy::ControlRef;
use crate::run::Run;
use crate::task::{Task, TaskEntry};
use crate::workflow::{WorkflowDefinition, WorkflowRun};
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
    /// Expiry metadata was refreshed; carries no credential or raw output.
    ImportantDatesUpdated { at: DateTime<Utc> },
    TaskCreated {
        task: Task,
    },
    TaskUpdated {
        task: Task,
    },
    TaskDeleted {
        id: String,
    },
    WorkflowCreated {
        workflow: WorkflowDefinition,
    },
    WorkflowUpdated {
        workflow: WorkflowDefinition,
    },
    WorkflowDeleted {
        id: String,
    },
    WorkflowRunUpdated {
        run: WorkflowRun,
    },
    /// A bench run's attempts changed -- one was dispatched, judged, or the
    /// run itself was cancelled. Wired exactly like `WorkflowRunUpdated`.
    BenchRunUpdated {
        run: crate::bench::BenchRun,
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
    AgentUpdated {
        agent: AgentSession,
    },
    AgentRemoved {
        id: String,
    },
    /// A declaration was added to one scope's local configuration.
    AgentConfigured {
        scope: String,
        name: String,
    },
    /// A declaration was removed from one scope's local configuration.
    AgentDeleted {
        scope: String,
        name: String,
    },
    /// A role definition was written into, or removed from, one scope's own
    /// configuration. Every scope below it may now resolve that name
    /// differently.
    RolesChanged {
        scope: String,
        name: String,
    },
    /// One scope's own dashboard layout was saved or reset (`#160`). Every
    /// scope below it may now resolve a different one, the same reach
    /// `RolesChanged` carries for a role layer.
    DashboardChanged {
        scope: String,
    },
    /// An attestation was recorded or withdrawn for one control at one
    /// scope. Published on both `Request::PolicyAttest` and
    /// `Request::PolicyWithdraw`, like `RolesChanged` on a role write.
    PolicyChanged {
        scope: String,
        control: ControlRef,
    },
    /// A check-in was recorded against a manual key result. Published on
    /// `Request::GoalsCheckIn`, the same way `PolicyChanged` follows an
    /// attestation.
    GoalsChanged {
        kr: KrRef,
    },
    /// What `.factory/quality/` or a `quality:`/`scope.quality` declaration
    /// says changed since the last `Request::Quality` read it. Quality has
    /// no write of its own to publish on the way `GoalsChanged` follows a
    /// check-in -- profiles are authored by hand and nothing in Factory
    /// watches files -- so this is the issue's "on the next read": each
    /// report fingerprints what it loaded, and publishes this when the
    /// fingerprint moved. `profiles` is every profile id now on disk.
    /// Evidence moving (a fitness-function task finishing) is not this
    /// event; it already arrives as `RunUpdated`.
    QualityChanged {
        profiles: Vec<String>,
    },
    /// A backup finished and retention ran after it (`#116`). Published for
    /// the schedule's backups and a person's alike.
    BackupCompleted {
        snapshot: crate::backup::Snapshot,
    },
    /// A backup could not be taken. The daemon carries on; this, the
    /// daemon's log and the page's warning are where the reason goes.
    BackupFailed {
        at: DateTime<Utc>,
        trigger: crate::backup::BackupTrigger,
        reason: String,
    },
    /// A snapshot was verified, whether or not it passed.
    BackupVerified {
        verification: crate::backup::VerifySummary,
    },
    /// A deployment began or ended (`#185`).
    DeploymentUpdated {
        deployment: Box<crate::environments::Deployment>,
    },
    /// An environment's status changed: its checks' latest answers add up
    /// to something else now. Only a change is published, never a sample.
    EnvironmentStatusChanged {
        environment: String,
        status: crate::environments::EnvStatus,
        at: DateTime<Utc>,
    },
    /// A push from a runtime, mapped onto whichever standing agent or run's
    /// session it was about. This never moves a task or a run -- only the
    /// agent's own `factory task report` may do that -- so `task_id()` is
    /// `None` here on purpose: this is activity with no task behind it, the
    /// exact gap the activity view has when nothing is running.
    AgentActivity {
        /// A standing agent's id (`scope/name`), or `run:<id>` for a task's
        /// session.
        subject: String,
        scope: String,
        agent: String,
        status: RuntimeStatus,
        at: DateTime<Utc>,
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
            Event::AgentUpdated { .. }
            | Event::AgentRemoved { .. }
            | Event::AgentConfigured { .. }
            | Event::AgentDeleted { .. }
            | Event::RolesChanged { .. }
            | Event::DashboardChanged { .. }
            | Event::PolicyChanged { .. }
            | Event::GoalsChanged { .. }
            | Event::QualityChanged { .. }
            | Event::BackupCompleted { .. }
            | Event::BackupFailed { .. }
            | Event::BackupVerified { .. }
            | Event::DeploymentUpdated { .. }
            | Event::EnvironmentStatusChanged { .. } => None,
            Event::WorkflowCreated { .. }
            | Event::WorkflowUpdated { .. }
            | Event::WorkflowDeleted { .. }
            | Event::WorkflowRunUpdated { .. }
            | Event::BenchRunUpdated { .. } => None,
            Event::AgentActivity { .. } => None,
            Event::DaemonStarted { .. } | Event::ImportantDatesUpdated { .. } => None,
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
