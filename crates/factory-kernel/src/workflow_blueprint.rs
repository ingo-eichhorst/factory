//! Plain authored workflow snapshots from L4; no execution, validation or
//! preview judgement. Producing owners normalize legacy rows before projection.
use crate::Schedule;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlueprintTimeEstimate {
    pub low: u64,
    pub expected: u64,
    pub high: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BlueprintCostEstimate {
    pub low: f64,
    pub expected: f64,
    pub high: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlueprintEstimate {
    pub time: BlueprintTimeEstimate,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<BlueprintCostEstimate>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BlueprintTaskSpec {
    pub title: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decomposition_part: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<Schedule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate: Option<BlueprintEstimate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ack_timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    /// Give this task its own git worktree, made fresh before each run.
    /// Absent means on, so every way of making a task -- the form, `factory
    /// task create`, the HTTP API, another agent -- gets the same default
    /// without having to say so. `Some(false)` is how a caller means it, not
    /// merely fails to mention it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<bool>,
    /// Hand each run the knowledge pages that match this task -- see
    /// `Task::knowledge_hints`. Absent means off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub knowledge_hints: bool,
    /// This task's own retry policy, overriding the daemon's default. Absent
    /// means "use the default" -- see `BlueprintRetryPolicy`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<BlueprintRetryPolicy>,
    /// See `Task::category`. Absent is the default category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlueprintRetryPolicy {
    /// A failed run displaces nothing: the task sits `pending`, showing that
    /// run's error, until its next regular firing.
    None,
    Backoff {
        /// How many retries this failure streak may queue, on top of the
        /// attempt that just failed.
        max_attempts: u32,
        /// How long to wait before each retry.
        backoff_seconds: u64,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct BlueprintPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlueprintNode {
    pub id: String,
    /// Feedback resumes by default. Independent work may explicitly opt out.
    #[serde(default, skip_serializing_if = "is_resume")]
    pub session: BlueprintSessionPolicy,
    #[serde(default)]
    pub position: BlueprintPoint,
    #[serde(default)]
    pub kind: BlueprintNodeKind,
    /// For a `Task` node, the task it spawns. For a `Gate` node only its
    /// `title` (the card's label) and `scope` mean anything -- a gate never
    /// spawns a task.
    pub task: BlueprintTaskSpec,
    /// What a `Gate` node checks. `None` on every `Task` node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<BlueprintGate>,
    /// Ordered conditional exits, checked after this task reports `done`.
    /// The first condition that holds selects its target exclusively; when
    /// none holds the node's ordinary outgoing edges remain the default.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exits: Vec<BlueprintExit>,
    /// Fan-out/join policy for an `Expand` node.  Absent everywhere else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expand: Option<BlueprintExpandSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlueprintExit {
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rounds: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlueprintInput {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlueprintExpandJoin {
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub tolerate: u8,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlueprintExpandSpec {
    #[serde(default)]
    pub join: BlueprintExpandJoin,
    #[serde(default)]
    pub cancel: BlueprintCancelPolicy,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<String>,
    #[serde(default = "default_rework_rounds")]
    pub max_rework_rounds: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlueprintGate {
    /// The step it is -- `tests`, `sbom`, `security_scan` -- and so which
    /// required step of the same name it satisfies.
    pub step: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    /// The task node whose run this gate judges. Injected gates always name
    /// it; an authored one may leave it to `WorkflowBlueprint::gate_subject`,
    /// which follows its parents back to the one task node above it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// `<framework>/<control>` or `quality/<attribute>` for each control
    /// whose plan this gate carries out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_by: Vec<String>,
    /// Injected by the control plan at run start.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
    /// `independent` for review, `person` for approval. Absent on v1 gates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// Concrete functionary frozen into a run snapshot. A missing review
    /// actor is a visible lint/execution gap, never permission to self-review.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlueprintEdge {
    pub id: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlueprintPart {
    /// The task node whose worktree branch is integrated, and which merge
    /// conflicts and failed combined checks are sent back to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deliverable: Option<String>,
    /// The node whose `done` releases the merge: the one node nothing
    /// follows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowBlueprint {
    pub id: String,
    pub name: String,
    pub description: String,
    pub scope: String,
    /// Authored revision on the definition; frozen commit on a run snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_ref: Option<String>,
    /// See `WorkflowDraft::category`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// See `WorkflowDraft::inputs`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<BlueprintInput>,
    /// See `WorkflowDraft::part`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<BlueprintPart>,
    pub nodes: Vec<BlueprintNode>,
    pub edges: Vec<BlueprintEdge>,
    pub revision: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlueprintSessionPolicy {
    #[default]
    Resume,
    Fresh,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlueprintNodeKind {
    #[default]
    Task,
    /// A required step (`#118`): a shell command the daemon runs in the
    /// worktree of the task node it verifies, exit 0 = pass, leaving an
    /// attestation behind. Never spawns a task and never runs an agent --
    /// its status mirrors the attestations its subject's run collected.
    Gate,
    /// An independent agent judges the subject's result after deterministic
    /// gates. The verifier spawns its task; the node mirrors that evidence.
    Review,
    /// A person decides before the subject is dispatched.
    Approval,
    /// A daemon-owned fan-out boundary.  It never spawns an agent itself;
    /// its children are task nodes materialised from an approved intake
    /// decomposition in this run's snapshot.
    Expand,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlueprintCancelPolicy {
    #[default]
    Terminate,
    Abandon,
}

fn is_zero_u8(value: &u8) -> bool {
    *value == 0
}
fn default_rework_rounds() -> u32 {
    3
}
fn is_resume(value: &BlueprintSessionPolicy) -> bool {
    *value == BlueprintSessionPolicy::Resume
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowBlueprintFact {
    pub blueprint: WorkflowBlueprint,
    pub categories: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkflowBlueprintQuery {
    All,
    Workflow(String),
    Task(String),
}
