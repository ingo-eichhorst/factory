//! Shared fact schema: data and representation helpers only.
//! Providers, authorisation, deduplication and evaluators stay in their levels.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Shared representation of RunStatus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Dispatching,
    Running,
    Blocked,
    Verifying,
    Done,
    Failed,
    Cancelled,
}

impl RunStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dispatching => "dispatching",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Verifying => "verifying",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl std::str::FromStr for RunStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "dispatching" => Self::Dispatching,
            "running" => Self::Running,
            "blocked" => Self::Blocked,
            "verifying" => Self::Verifying,
            "done" => Self::Done,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            other => return Err(format!("unknown run status: {other}")),
        })
    }
}

/// Shared representation of FailKind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailKind {
    AckTimeout,
    RunTimeout,
    BlockedTimeout,
    SessionGone,
    DispatchFailed,
    AgentFailed,
    TurnEnded,
    StopFailure,
    CancelledByPerson,
    CancelledByAgent,
    CancelledWithParent,
}

impl FailKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AckTimeout => "ack_timeout",
            Self::RunTimeout => "run_timeout",
            Self::BlockedTimeout => "blocked_timeout",
            Self::SessionGone => "session_gone",
            Self::DispatchFailed => "dispatch_failed",
            Self::AgentFailed => "agent_failed",
            Self::TurnEnded => "turn_ended",
            Self::StopFailure => "stop_failure",
            Self::CancelledByPerson => "cancelled_by_person",
            Self::CancelledByAgent => "cancelled_by_agent",
            Self::CancelledWithParent => "cancelled_with_parent",
        }
    }
    pub fn is_infrastructure(self) -> bool {
        matches!(
            self,
            Self::AckTimeout | Self::RunTimeout | Self::SessionGone | Self::DispatchFailed
        )
    }
}

/// Shared representation of WorkflowRunStatus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRunStatus {
    Running,
    Done,
    Failed,
    Cancelled,
}

impl WorkflowRunStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }
}

/// Shared representation of Verdict.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BenchVerdict {
    Pass,
    Fail,
    Unverified,
    Skipped,
    Cancelled,
    Error,
}

impl BenchVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Unverified => "unverified",
            Self::Skipped => "skipped",
            Self::Cancelled => "cancelled",
            Self::Error => "error",
        }
    }
}

/// Shared representation of Grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Grant {
    #[serde(rename = "task.create")]
    TaskCreate,
    #[serde(rename = "task.edit")]
    TaskEdit,
    #[serde(rename = "task.delete")]
    TaskDelete,
    #[serde(rename = "task.run")]
    TaskRun,
    #[serde(rename = "task.cancel")]
    TaskCancel,
    #[serde(rename = "task.close")]
    TaskClose,
    #[serde(rename = "task.report")]
    TaskReport,
    #[serde(rename = "task.attach")]
    TaskAttach,
    #[serde(rename = "agent.start")]
    AgentStart,
    #[serde(rename = "agent.configure")]
    AgentConfigure,
    #[serde(rename = "agent.stop")]
    AgentStop,
    #[serde(rename = "agent.input")]
    AgentInput,
    #[serde(rename = "run.input")]
    RunInput,
    #[serde(rename = "run.approve")]
    RunApprove,
    #[serde(rename = "workflow.create")]
    WorkflowCreate,
    #[serde(rename = "workflow.edit")]
    WorkflowEdit,
    #[serde(rename = "workflow.delete")]
    WorkflowDelete,
    #[serde(rename = "workflow.run")]
    WorkflowRun,
    #[serde(rename = "workflow.cancel")]
    WorkflowCancel,
    #[serde(rename = "knowledge.write")]
    KnowledgeWrite,
    #[serde(rename = "dataset.edit")]
    DatasetEdit,
    #[serde(rename = "bench.run")]
    BenchRun,
    #[serde(rename = "policy.attest")]
    PolicyAttest,
    #[serde(rename = "goals.checkin")]
    GoalsCheckIn,
    #[serde(rename = "backup.run")]
    BackupRun,
    #[serde(rename = "intake.add")]
    IntakeAdd,
    #[serde(rename = "intake.info")]
    IntakeInfo,
    #[serde(rename = "intake.triage")]
    IntakeTriage,
    #[serde(rename = "intake.assess")]
    IntakeAssess,
    #[serde(rename = "intake.decide")]
    IntakeDecide,
    #[serde(rename = "intake.publish")]
    IntakePublish,
    #[serde(rename = "dashboard.edit")]
    DashboardEdit,
}

impl Grant {
    pub const ALL: [Grant; 32] = [
        Grant::TaskCreate,
        Grant::TaskEdit,
        Grant::TaskDelete,
        Grant::TaskRun,
        Grant::TaskCancel,
        Grant::TaskClose,
        Grant::TaskReport,
        Grant::TaskAttach,
        Grant::AgentStart,
        Grant::AgentConfigure,
        Grant::AgentStop,
        Grant::AgentInput,
        Grant::RunInput,
        Grant::RunApprove,
        Grant::WorkflowCreate,
        Grant::WorkflowEdit,
        Grant::WorkflowDelete,
        Grant::WorkflowRun,
        Grant::WorkflowCancel,
        Grant::KnowledgeWrite,
        Grant::DatasetEdit,
        Grant::BenchRun,
        Grant::PolicyAttest,
        Grant::GoalsCheckIn,
        Grant::BackupRun,
        Grant::IntakeAdd,
        Grant::IntakeInfo,
        Grant::IntakeTriage,
        Grant::IntakeAssess,
        Grant::IntakeDecide,
        Grant::IntakePublish,
        Grant::DashboardEdit,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::TaskCreate => "task.create",
            Self::TaskEdit => "task.edit",
            Self::TaskDelete => "task.delete",
            Self::TaskRun => "task.run",
            Self::TaskCancel => "task.cancel",
            Self::TaskClose => "task.close",
            Self::TaskReport => "task.report",
            Self::TaskAttach => "task.attach",
            Self::AgentStart => "agent.start",
            Self::AgentConfigure => "agent.configure",
            Self::AgentStop => "agent.stop",
            Self::AgentInput => "agent.input",
            Self::RunInput => "run.input",
            Self::RunApprove => "run.approve",
            Self::WorkflowCreate => "workflow.create",
            Self::WorkflowEdit => "workflow.edit",
            Self::WorkflowDelete => "workflow.delete",
            Self::WorkflowRun => "workflow.run",
            Self::WorkflowCancel => "workflow.cancel",
            Self::KnowledgeWrite => "knowledge.write",
            Self::DatasetEdit => "dataset.edit",
            Self::BenchRun => "bench.run",
            Self::PolicyAttest => "policy.attest",
            Self::GoalsCheckIn => "goals.checkin",
            Self::BackupRun => "backup.run",
            Self::IntakeAdd => "intake.add",
            Self::IntakeInfo => "intake.info",
            Self::IntakeTriage => "intake.triage",
            Self::IntakeAssess => "intake.assess",
            Self::IntakeDecide => "intake.decide",
            Self::IntakePublish => "intake.publish",
            Self::DashboardEdit => "dashboard.edit",
        }
    }
    pub fn describe(self) -> &'static str {
        match self {
            Self::TaskCreate => "create tasks",
            Self::TaskEdit => "change tasks",
            Self::TaskDelete => "delete tasks",
            Self::TaskRun => "start runs",
            Self::TaskCancel => "cancel runs",
            Self::TaskClose => "close and reopen tasks",
            Self::TaskReport => "report on tasks",
            Self::TaskAttach => "attach dependency scan documents",
            Self::AgentStart => "start agents",
            Self::AgentConfigure => "configure agents",
            Self::AgentStop => "stop agents",
            Self::AgentInput => "type into an agent's session",
            Self::RunInput => "type into a run's session",
            Self::RunApprove => "approve, reject, or accept rework for runs",
            Self::WorkflowCreate => "create workflows",
            Self::WorkflowEdit => "change workflows",
            Self::WorkflowDelete => "delete workflows",
            Self::WorkflowRun => "start workflows",
            Self::WorkflowCancel => "cancel workflows",
            Self::KnowledgeWrite => "add files to the knowledge base",
            Self::DatasetEdit => "create, edit, and delete datasets and their cases",
            Self::BenchRun => "start, cancel, and clean bench runs",
            Self::PolicyAttest => "record and withdraw policy attestations",
            Self::GoalsCheckIn => "record check-ins against manual key results",
            Self::BackupRun => "take or verify a backup of the instance",
            Self::IntakeAdd => "hand something in through the intake gate",
            Self::IntakeInfo => "answer a needs-info on an intake item",
            Self::IntakeTriage => "start a triage run on an intake item",
            Self::IntakeAssess => "record an assessment on an intake item",
            Self::IntakeDecide => "release, send back, split or close an intake item",
            Self::IntakePublish => {
                "publish a decided GitHub item's triage comment and labels to its issue"
            }
            Self::DashboardEdit => "save or reset a scope's dashboard layout",
        }
    }
    pub fn group(self) -> &'static str {
        match self {
            Self::TaskCreate
            | Self::TaskEdit
            | Self::TaskDelete
            | Self::TaskRun
            | Self::TaskCancel
            | Self::TaskClose
            | Self::TaskReport
            | Self::TaskAttach => "Tasks",
            Self::AgentStart | Self::AgentConfigure | Self::AgentStop | Self::AgentInput => {
                "Agents"
            }
            Self::RunInput | Self::RunApprove => "Runs",
            Self::WorkflowCreate
            | Self::WorkflowEdit
            | Self::WorkflowDelete
            | Self::WorkflowRun
            | Self::WorkflowCancel => "Workflows",
            Self::KnowledgeWrite => "Knowledge",
            Self::DatasetEdit => "Datasets",
            Self::BenchRun => "Bench",
            Self::PolicyAttest => "Policy",
            Self::GoalsCheckIn => "Goals",
            Self::BackupRun => "Backup",
            Self::IntakeAdd
            | Self::IntakeInfo
            | Self::IntakeTriage
            | Self::IntakeAssess
            | Self::IntakeDecide
            | Self::IntakePublish => "Intake",
            Self::DashboardEdit => "Dashboard",
        }
    }
}

/// Shared representation of AttachmentKind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    Sbom,
    Vulnerabilities,
}

impl AttachmentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sbom => "sbom",
            Self::Vulnerabilities => "vulnerabilities",
        }
    }
}

impl std::fmt::Display for AttachmentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for AttachmentKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "sbom" => Ok(Self::Sbom),
            "vulnerabilities" => Ok(Self::Vulnerabilities),
            _ => Err(format!("{s:?} is not sbom or vulnerabilities")),
        }
    }
}

/// Shared representation of LifecycleState.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    Declared,
    Built,
    Running,
}

impl LifecycleState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Declared => "declared",
            Self::Built => "built",
            Self::Running => "running",
        }
    }

    pub fn from_phase(phase: &str) -> Option<Self> {
        match phase {
            "pre-build" => Some(Self::Declared),
            "build" => Some(Self::Built),
            "operations" => Some(Self::Running),
            _ => None,
        }
    }
}

impl std::fmt::Display for LifecycleState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Shared representation of Severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
    Unknown,
}

impl Severity {
    pub fn from_cyclonedx(value: Option<&str>) -> Self {
        match value.unwrap_or_default().to_ascii_lowercase().as_str() {
            "critical" => Self::Critical,
            "high" => Self::High,
            "medium" | "moderate" => Self::Medium,
            "low" | "negligible" => Self::Low,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::Unknown => "unknown",
        }
    }
}

/// Shared representation of Attachment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    pub id: String,
    pub kind: AttachmentKind,
    pub scope: String,
    pub run_id: String,
    pub task_id: String,
    pub attempt: u32,
    pub attached_at: DateTime<Utc>,
    pub filename: String,
    pub spec_version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<LifecycleState>,
}

/// Shared representation of AffectedComponent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffectedComponent {
    pub bom_ref: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<String>,
}

/// Shared representation of SourceKind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Cli,
    Ui,
    Agent,
    Github,
    Email,
    Chat,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Ui => "ui",
            Self::Agent => "agent",
            Self::Github => "github",
            Self::Email => "email",
            Self::Chat => "chat",
        }
    }
}

/// Shared representation of IntakeSource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntakeSource {
    pub kind: SourceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relayed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
}

/// Shared representation of StepKind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Gate,
    Review,
    Approval,
}

impl StepKind {
    pub fn of(step: &str) -> Self {
        match step {
            "review" => Self::Review,
            "approval" => Self::Approval,
            _ => Self::Gate,
        }
    }
    pub fn enforced(self) -> bool {
        true
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gate => "gate",
            Self::Review => "review",
            Self::Approval => "approval",
        }
    }
}

/// Shared representation of RequiredStep.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequiredStep {
    pub step: String,
    pub kind: StepKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
}

/// Shared representation of StepAttestation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepAttestation {
    pub id: String,
    pub run_id: String,
    pub task_id: String,
    pub scope: String,
    pub category: String,
    pub step: String,
    pub kind: StepKind,
    pub actor: String,
    pub verdict: AttestationVerdict,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub findings: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub round: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    pub dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirty: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    pub at: DateTime<Utc>,
}

/// Shared representation of AttestationVerdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttestationVerdict {
    Pass,
    Fail,
}

impl AttestationVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
        }
    }
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}
