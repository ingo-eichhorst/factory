//! One request/response envelope, spoken by every interface adapter. The CLI
//! sends it over a unix socket, the HTTP adapter maps REST onto it; adding a
//! third interface means translating to this, not inventing a new API.

use crate::event::Event;
use factory_agents::agent::AgentSession;
use factory_agents::role::{RoleOrigin, RoleSpec};
use factory_agents::runtime::{RuntimeConnectionDiagnostic, Screen};
use factory_assurance::benchmark::Configuration;
use factory_assurance::knowledge::{Document, Finding, Gap, Page, Refusal, Tag};
use factory_composition::building::{Activity, Cues, RepoMetrics, Shape};
use factory_composition::config::ScopeAgent;
use factory_environment::dependencies::{
    Attachment, AttachmentKind, DependenciesReport, DoctorReport,
};
use factory_process::occupancy::Occupancy;
use factory_process::run::Run;
use factory_process::task::{
    CloseReason, NewTask, Task, TaskEntry, TaskFilter, TaskPatch, TaskReport, TurnEnded,
};
use factory_process::workflow::{WorkflowDefinition, WorkflowDraft, WorkflowLint, WorkflowRun};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", content = "params", rename_all = "snake_case")]
pub enum Request {
    #[serde(rename = "status")]
    Status,
    #[serde(rename = "adapters")]
    Adapters,
    /// One connection diagnostic per effective runtime, grouped with every
    /// scope that uses it.
    #[serde(rename = "runtime.connections")]
    RuntimeConnections,
    /// The scopes, the agents each one declares, and what they are doing.
    #[serde(rename = "agents")]
    Agents,
    /// Bring a declared standing agent up.
    #[serde(rename = "agent.start")]
    AgentStart { scope: String, name: String },
    /// Add an agent declaration to one scope's local Factory config.
    #[serde(rename = "agent.configure")]
    AgentConfigure { scope: String, agent: ScopeAgent },
    /// Remove an agent declaration from one scope's local Factory config.
    #[serde(rename = "agent.delete")]
    AgentDelete { scope: String, name: String },
    /// Take one down and leave it down.
    #[serde(rename = "agent.stop")]
    AgentStop { id: String },
    /// Every role in effect in one scope -- where each was defined, what it
    /// may do in the vocabulary the check uses, and who holds it -- plus where
    /// roles are written across the whole tree. With no scope, only the
    /// latter, and the roles that hold everywhere.
    #[serde(rename = "role.list")]
    RoleList {
        #[serde(default)]
        scope: Option<String>,
    },
    /// Write one role into a scope's own config: its `scope.roles`, or the
    /// top-level `roles:` for the instance root. `replace` says the role is
    /// already defined in that file and this replaces it; without it, a name
    /// the file already defines is refused rather than silently overwritten.
    /// The owner's alone: an agent that could rewrite a role definition would
    /// not be bounded by the one it has.
    #[serde(rename = "role.define")]
    RoleDefine {
        scope: String,
        name: String,
        role: RoleSpec,
        #[serde(default)]
        replace: bool,
    },
    /// Remove one role from a scope's own config. Refused while an agent still
    /// depends on that definition. The owner's alone, like `RoleDefine`.
    #[serde(rename = "role.delete")]
    RoleDelete { scope: String, name: String },
    /// Give a standing agent a role, or take the given one away and let the
    /// config decide again. The owner's to do, and nobody else's.
    #[serde(rename = "agent.role")]
    AgentRole {
        id: String,
        /// `None` clears an assignment rather than naming one.
        #[serde(default)]
        role: Option<String>,
    },
    /// Type at a standing agent's session.
    #[serde(rename = "agent.input")]
    AgentInput {
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        keys: Vec<String>,
        id: String,
    },
    #[serde(rename = "agent.output")]
    AgentOutput {
        id: String,
        #[serde(default)]
        lines: Option<u32>,
    },
    /// One frame of a standing agent's screen, for a viewer that wants the
    /// terminal rather than the transcript.
    #[serde(rename = "agent.screen")]
    AgentScreen { id: String },
    /// The same for a task run's session.
    #[serde(rename = "run.screen")]
    RunScreen { id: String },
    /// Type at a task run's session, for the same reason.
    #[serde(rename = "run.input")]
    RunInput {
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        keys: Vec<String>,
        id: String,
    },
    /// `#106`: answer a blocked run -- `run.input` narrowed to the one case
    /// the Line tab offers it for. Only into the run's own session,
    /// only while the run is `Blocked`, and never without a reason, which
    /// is journaled as an intervention together with who gave it. The text
    /// itself is typed, not journaled: a record is no place for whatever a
    /// person had to type to unblock an agent. The run's status is left
    /// alone -- whether the answer unblocked it is the agent's to report.
    #[serde(rename = "run.answer")]
    RunAnswer {
        id: String,
        text: String,
        reason: String,
    },
    #[serde(rename = "task.create")]
    TaskCreate(NewTask),
    #[serde(rename = "task.get")]
    TaskGet { id: String },
    #[serde(rename = "task.list")]
    TaskList(TaskFilter),
    /// `reason` (`#106`) is why, in the asker's words. It is not part of the
    /// patch -- a store applies a patch, and a reason is not a property of
    /// the task -- and it is only written down where the edit already
    /// journals something: pausing or resuming a schedule.
    #[serde(rename = "task.update")]
    TaskUpdate {
        id: String,
        patch: TaskPatch,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    #[serde(rename = "task.delete")]
    TaskDelete { id: String },
    /// Journaled with who asked, and `reason` when there is one (`#106`).
    /// `continue_run` (`continue` on the wire -- a Rust keyword) is
    /// `factory task run --continue` (`#178`): resume the task's newest
    /// run's harness session rather than start a fresh one. Refused outright
    /// unless that newest run is terminal and ended on an infrastructure
    /// failure; from there, `Engine::dispatch` falls back to a fresh session
    /// -- journaled with the exact reason -- for anything that stops the
    /// resume itself from going through.
    #[serde(rename = "task.run")]
    TaskRun {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(
            default,
            rename = "continue",
            skip_serializing_if = "std::ops::Not::not"
        )]
        continue_run: bool,
        /// Deliberately bypass a waiting trigger, with a journaled reason.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        override_wait: bool,
    },
    /// Journaled with who asked, and `reason` when there is one (`#106`).
    #[serde(rename = "task.cancel")]
    TaskCancel {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// The run the caller means to cancel. When given, the cancel is
        /// refused unless it is still the task's active run -- so a person
        /// cancelling the attempt they were shown never ends a retry that
        /// started in the meantime.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run: Option<String>,
    },
    /// `#122`: close a task on purpose, with a reason -- `completed` makes
    /// it `Done`, `not_planned` and `duplicate` make it `Cancelled` -- and
    /// an optional note, journaled with who closed it. Works with no run at
    /// all, and on a task blocked by a failure; refused while a run is
    /// active (cancel it first) and on a task already closed (reopen it
    /// first).
    #[serde(rename = "task.close")]
    TaskClose {
        id: String,
        reason: CloseReason,
        /// The task this one duplicates. Only with `duplicate`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duplicate_of: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// `#122`: put a closed task back to `Pending`, its close record gone.
    /// Journaled with who asked, and `reason` when there is one.
    #[serde(rename = "task.reopen")]
    TaskReopen {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// `#106`: pass over a scheduled task's next slot. `next_run_at` moves
    /// to the slot after it, and the skip is journaled with who asked and
    /// why -- as `slot_skipped`, never `schedule_skipped`, which is the
    /// scheduler's record of slots that passed *without* anyone deciding so.
    /// `slot`, when given, is the firing the caller means to skip (the
    /// `next_run_at` it read); the skip is refused if the schedule has moved
    /// on since, rather than landing on a firing nobody chose.
    #[serde(rename = "task.skip_next")]
    TaskSkipNext {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slot: Option<chrono::DateTime<chrono::Utc>>,
    },
    #[serde(rename = "task.report")]
    TaskReport { id: String, report: TaskReport },
    #[serde(rename = "task.attach")]
    TaskAttach {
        id: String,
        token: String,
        kind: AttachmentKind,
        filename: String,
        bytes: Vec<u8>,
    },
    /// A harness's lifecycle hook saying the agent's turn ended. Its own
    /// request rather than a kind of `TaskReport`, so nothing can mistake the
    /// harness speaking for the agent reporting.
    #[serde(rename = "task.turn_ended")]
    TaskTurnEnded { id: String, turn: TurnEnded },
    #[serde(rename = "task.entries")]
    TaskEntries {
        id: String,
        #[serde(default)]
        limit: Option<u32>,
        /// Only the task's own entries, the ones that belong to no run
        /// (`TaskStore::task_own_entries`).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        task_only: bool,
    },
    /// Terminal output for the task's most recent run.
    #[serde(rename = "task.output")]
    TaskOutput {
        id: String,
        #[serde(default)]
        lines: Option<u32>,
    },
    #[serde(rename = "workflow.create")]
    WorkflowCreate(WorkflowDraft),
    #[serde(rename = "workflow.get")]
    WorkflowGet { id: String },
    #[serde(rename = "workflow.list")]
    WorkflowList {
        #[serde(default)]
        scope: Option<String>,
    },
    #[serde(rename = "workflow.update")]
    WorkflowUpdate { id: String, workflow: WorkflowDraft },
    #[serde(rename = "workflow.delete")]
    WorkflowDelete { id: String },
    #[serde(rename = "workflow.run")]
    WorkflowStart {
        id: String,
        /// The values its declared inputs are started with (`#140`).
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        inputs: std::collections::BTreeMap<String, String>,
    },
    #[serde(rename = "workflow_run.get")]
    WorkflowRunGet { id: String },
    #[serde(rename = "workflow_run.list")]
    WorkflowRunList {
        #[serde(default)]
        workflow_id: Option<String>,
        #[serde(default)]
        scope: Option<String>,
        #[serde(default)]
        limit: Option<u32>,
    },
    #[serde(rename = "workflow_run.cancel")]
    WorkflowRunCancel { id: String },
    /// `#118`'s author-facing preview: the control plan a stored workflow,
    /// a task (as its implicit one-node workflow) or a bare scope and
    /// category is held to, and what a run of it would get injected.
    /// `category` overrides the one the workflow or task names.
    #[serde(rename = "workflow.lint")]
    WorkflowLint {
        #[serde(default)]
        workflow: Option<String>,
        #[serde(default)]
        task: Option<String>,
        #[serde(default)]
        scope: Option<String>,
        #[serde(default)]
        category: Option<String>,
    },
    /// Every attestation a run's required steps left behind (`#118`).
    #[serde(rename = "run.attestations")]
    RunAttestations { id: String },
    /// Append-only artifact provenance for this completed run (#158).
    #[serde(rename = "run.provenance")]
    RunProvenance { id: String },
    /// Person/functionary decisions for enforced approval and rework.
    #[serde(rename = "run.approve")]
    RunApprove { id: String, reason: String },
    #[serde(rename = "run.reject")]
    RunReject { id: String, reason: String },
    #[serde(rename = "run.rework")]
    RunRework { id: String },
    #[serde(rename = "run.list")]
    RunList {
        task_id: String,
        #[serde(default)]
        limit: Option<u32>,
    },
    #[serde(rename = "run.get")]
    RunGet { id: String },
    #[serde(rename = "run.entries")]
    RunEntries {
        id: String,
        #[serde(default)]
        limit: Option<u32>,
    },
    /// Terminal output for one run: live from its session while it is running,
    /// and the transcript kept at the end once it is not.
    #[serde(rename = "run.output")]
    RunOutput {
        id: String,
        #[serde(default)]
        lines: Option<u32>,
    },
    /// What every bay was doing over a window: the runs that held it, what is
    /// scheduled to hold it next, and what the runtime saw in between.
    #[serde(rename = "occupancy")]
    Occupancy {
        /// How far back to look. Defaults to the last twelve hours. The
        /// width of the window when only one of `from` and `to` is given,
        /// and ignored when both are.
        #[serde(default)]
        minutes: Option<u32>,
        /// An explicit window, for a chart that has been panned or zoomed
        /// away from the one that ends a little after now. Either side of
        /// now, or across it; its width is still clamped.
        #[serde(default)]
        from: Option<chrono::DateTime<chrono::Utc>>,
        #[serde(default)]
        to: Option<chrono::DateTime<chrono::Utc>>,
    },
    /// The run history the dashboard's window, sparklines, throughput chart
    /// and production-year grid all read from one request -- not one per
    /// card. A run carries no scope of its own; narrowed by joining through
    /// its task, the way `Occupancy` above already does.
    #[serde(rename = "production")]
    Production {
        /// How far back the throughput window looks, in minutes. Defaults to
        /// fourteen days.
        #[serde(default)]
        minutes: Option<u32>,
        /// Hour, day or week. Defaults to day. Sent by the caller rather than
        /// inferred from `minutes`, so a view can ask for exactly the
        /// granularity it draws instead of being silently regrouped.
        #[serde(default)]
        bin: Option<ProductionBin>,
        #[serde(default)]
        scope: Option<String>,
    },
    /// How big each scope is on disk, for the site plan's hall sizes. Nothing
    /// else needs this, which is why it is its own request rather than a field
    /// every `agents` call would have to pay for.
    #[serde(rename = "site.footprint")]
    SiteFootprint,
    /// The L2 Environment page: where every declared agent's runs execute
    /// (Sandboxes), and the honest inventory of what an agent can already
    /// reach because it runs as the daemon's owner (Secrets). Read-only, like
    /// `RuntimeConnections` -- it reports on what is already true rather than
    /// changing anything.
    #[serde(rename = "environment")]
    Environment,
    #[serde(rename = "dependencies")]
    Dependencies { scope: String },
    #[serde(rename = "dependencies.vex")]
    DependenciesVex { scope: String },
    /// The L1 Doctor dependency view: newest Factory build versus the
    /// installed binaries, derived from immutable scan evidence.
    #[serde(rename = "doctor")]
    Doctor,
    /// The L1 Infrastructure page: what everything runs on -- the host and
    /// the daemon on it, read live on every request, and the AI accounts the
    /// root config declares with the agents each one pays for. Read-only,
    /// like `Environment`, and it never reads a credential: a provider is
    /// only ever what the config says it is.
    #[serde(rename = "infrastructure")]
    Infrastructure,
    /// The L1 Mac tab (`#260`): the host's macOS power mode for AC and for
    /// battery, which modes the host offers, and whether Factory may change
    /// it -- found out without changing anything. Read-only; off macOS the
    /// answer is "not applicable", never an error.
    #[serde(rename = "host.power_mode")]
    HostPowerMode,
    /// Set the host's power mode for every power source (`pmset -a
    /// powermode 0|1|2`), through `sudo -n` and a sudoers rule scoped to
    /// exactly those three commands. `mode` is one of three closed values;
    /// nothing else deserializes, so no free string reaches a command.
    /// Journaled with who changed it, from what, to what. Needs
    /// `host.power` vocabulary, restricted to the owner even with a named grant.
    #[serde(rename = "host.power_mode.set")]
    HostPowerModeSet { mode: PowerMode },
    /// Read-only important-date metadata and native policy/CRA projections.
    #[serde(rename = "important-dates")]
    ImportantDates {
        #[serde(default)]
        scope: Option<String>,
    },
    /// The L1 Backup page (`#116`): the configured destination and schedule,
    /// every snapshot of this instance found there with how it was kept and
    /// verified, and the honest warnings those facts add up to. Read-only,
    /// like `Infrastructure`: listing a destination changes nothing.
    #[serde(rename = "backup")]
    Backup,
    /// Take a snapshot now -- the same one the schedule takes -- then apply
    /// retention. `backup.run`, checked against the root scope like
    /// `policy.attest`: the instance's state is company-wide, not one
    /// project's. Refused while another backup operation is running.
    #[serde(rename = "backup.run")]
    BackupRun,
    /// The Operations tab (`#185`): every environment the systems Factory
    /// builds are deployed to -- declared ones and any a deployment names --
    /// with its status, current release, SLA figures and DORA keys, the
    /// release catalogue and the deployment history. Read-only, computed on
    /// read from recorded samples and deployments. `scope` narrows it to
    /// that scope's subtree.
    #[serde(rename = "environments")]
    Environments {
        #[serde(default)]
        scope: Option<String>,
    },
    /// Owner-only: create and start a revision-frozen, policy-gated promotion
    /// workflow. Deployment still requires an explicit approval of its run.
    #[serde(rename = "environment.promote")]
    EnvironmentPromote(factory_infrastructure::environments::Promote),
    #[serde(rename = "environment.recover")]
    EnvironmentRecover(factory_infrastructure::environments::Recover),
    #[serde(rename = "environment.check")]
    EnvironmentCheck { environment: String },
    #[serde(rename = "environment.samples")]
    EnvironmentSamples(factory_infrastructure::environments::SampleQuery),
    #[serde(rename = "release.detail")]
    ReleaseDetail(factory_infrastructure::environments::ReleaseQuery),
    #[serde(rename = "dependency.document")]
    DependencyDocument { scope: String, id: String },
    /// A deployment has begun (`#185`). `deploy.record`, checked against
    /// the environment's scope. Answers the recorded `Deployment`, whose
    /// `id` the matching `deploy.finish` names.
    #[serde(rename = "deploy.start")]
    DeployStart(factory_infrastructure::environments::DeployStart),
    /// A deployment has ended. A success is only recorded as one once the
    /// environment's own checks pass, unless `verify: false` says to skip
    /// them. The same grant as `DeployStart`.
    #[serde(rename = "deploy.finish")]
    DeployFinish(factory_infrastructure::environments::DeployFinish),
    #[serde(rename = "deploy.mirror_plan")]
    DeployMirrorPlan { id: String },
    /// Explicit approval of the exact metadata/destination, not a background effect.
    #[serde(rename = "deploy.publish")]
    DeployPublish { id: String, approval: String },
    /// Put a release in the catalogue without deploying it. The same grant.
    #[serde(rename = "release.add")]
    ReleaseAdd(factory_infrastructure::environments::ReleaseAdd),
    /// Unpack a snapshot into a temporary directory and prove it would
    /// restore: every checksum in its manifest, `integrity_check` on the
    /// database copy, and every authored-content loader. `snapshot: None`
    /// is the newest. The same grant as `BackupRun` -- unless `identity` is
    /// `Some`, which decrypts an encrypted snapshot and is the owner's alone
    /// (`#152`): a role grant must never become "read this key file".
    /// Nothing in the destination or the instance is changed; the result is
    /// recorded (an encrypted snapshot given no identity is refused before
    /// anything is recorded -- see the daemon's `backup` module).
    #[serde(rename = "backup.verify")]
    BackupVerify {
        #[serde(default)]
        snapshot: Option<String>,
        /// A path to a file holding one native `AGE-SECRET-KEY-1…` identity
        /// (`#152`), read once by the daemon and never stored, logged or
        /// echoed back. Required to verify an encrypted snapshot; refused
        /// for a path inside the instance's own `.factory/`. The HTTP verify
        /// endpoint never accepts one.
        #[serde(default)]
        identity: Option<PathBuf>,
    },
    /// Verify and materialize a snapshot as a new instance root -- plaintext
    /// as v1 did, or encrypted with `identity` supplied (`#152`). The
    /// destination must not exist or must be empty; the daemon stages it
    /// beside that destination and renames only after every required check
    /// passes. CLI-only and owner-only: there is intentionally no grant or
    /// HTTP endpoint for restore.
    #[serde(rename = "backup.restore")]
    BackupRestore {
        snapshot: String,
        into: PathBuf,
        /// See `BackupVerify::identity`.
        #[serde(default)]
        identity: Option<PathBuf>,
    },
    /// The L5 Knowledge tab: an index of `<root>/.factory/knowledge/`,
    /// rebuilt from the files on every request. Read-only, like
    /// `Environment` -- see `knowledge::index`, which does the actual walk.
    #[serde(rename = "knowledge")]
    Knowledge,
    /// Search the vault through the instance's knowledge provider
    /// (`daemon.knowledge_provider`). Read-only and open to every agent,
    /// like `Knowledge`. Hits are page ids with a reason, never page text --
    /// see `adapter::KnowledgeProvider`. `limit` absent means
    /// `DEFAULT_SEARCH_LIMIT`, and the daemon caps it at `MAX_SEARCH_LIMIT`.
    #[serde(rename = "knowledge.search")]
    KnowledgeSearch {
        #[serde(default)]
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tags: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
    },
    /// Bulk-copy a directory tree into the vault, preserving relative paths
    /// so `[[area/page]]` still resolves afterwards. The daemon does the
    /// copy -- the CLI and the daemon share a machine and a user, so the CLI
    /// sends the path rather than the bytes. Needs `Grant::KnowledgeWrite`.
    #[serde(rename = "knowledge.import")]
    KnowledgeImport {
        source: String,
        #[serde(default)]
        into: Option<String>,
        #[serde(default)]
        overwrite: bool,
    },
    /// Add one or more files by path, the same way, for `factory knowledge
    /// add`. Needs `Grant::KnowledgeWrite`.
    #[serde(rename = "knowledge.add")]
    KnowledgeAdd {
        sources: Vec<String>,
        #[serde(default)]
        into: Option<String>,
        #[serde(default)]
        overwrite: bool,
    },
    /// Write one file's bytes directly -- the UI's "Add documents" upload,
    /// which cannot name a path on the daemon's own disk the way a CLI
    /// invocation can. `PUT /api/knowledge/files?path=&overwrite=` is the
    /// only caller in practice. Needs `Grant::KnowledgeWrite`.
    #[serde(rename = "knowledge.write_file")]
    KnowledgeWriteFile {
        path: String,
        #[serde(default)]
        overwrite: bool,
        bytes: Vec<u8>,
    },
    /// The L5 Benchmarks tab: one configuration per distinct harness, full
    /// `args`, and sandbox that a task could be dispatched with today,
    /// including the foreman `daemon.foreman` would synthesize. Nothing is
    /// run and nothing is scored -- see `benchmark::configurations`.
    #[serde(rename = "benchmarks")]
    Benchmarks,
    /// Every dataset, summarized. Read-only.
    #[serde(rename = "datasets")]
    Datasets,
    /// One dataset, in full -- every case.
    #[serde(rename = "dataset")]
    Dataset { name: String },
    /// `factory dataset create`. `dataset.edit`, checked against the root
    /// scope -- datasets are company-wide, not one project's.
    #[serde(rename = "dataset.create")]
    DatasetCreate {
        name: String,
        #[serde(default)]
        description: Option<String>,
    },
    /// `factory dataset case add`.
    #[serde(rename = "dataset.add_cases")]
    DatasetAddCases {
        name: String,
        cases: Vec<factory_assurance::dataset::Case>,
    },
    /// `factory dataset import`. All or nothing: a bad case anywhere refuses
    /// the whole file, naming every line or row and field that is wrong.
    #[serde(rename = "dataset.import")]
    DatasetImport {
        name: String,
        /// `jsonl`, `json`, `yaml`, `yml`, or `csv`.
        format: String,
        content: String,
        #[serde(default)]
        replace: bool,
    },
    /// `factory dataset from-tasks`. Each task becomes one case; a generated
    /// case never carries a `gate`.
    #[serde(rename = "dataset.from_tasks")]
    DatasetFromTasks { name: String, task_ids: Vec<String> },
    #[serde(rename = "dataset.delete_case")]
    DatasetDeleteCase { name: String, id: String },
    #[serde(rename = "dataset.delete")]
    DatasetDelete { name: String },
    /// `factory bench run`. `bench.run`, checked against the root scope, the
    /// same as `dataset.edit`.
    #[serde(rename = "bench.run")]
    BenchRunStart {
        dataset: String,
        /// An agent name from each `--agent [<scope>/]<agent>`; only the
        /// trailing name is kept, since the agent that actually resolves in
        /// each case's own scope is what matters, not the scope a person
        /// happened to find it under in the roster.
        agents: Vec<String>,
        #[serde(default)]
        attempts: Option<u32>,
        #[serde(default)]
        concurrency: Option<u32>,
        /// A subset of the dataset's cases. Absent means every case.
        #[serde(default)]
        cases: Option<Vec<String>>,
    },
    /// Every bench run, most recently updated first. Read-only.
    #[serde(rename = "bench.runs")]
    BenchRuns {
        #[serde(default)]
        dataset: Option<String>,
    },
    /// One bench run, with its attempts and the results aggregated from
    /// them. Read-only.
    #[serde(rename = "bench.run_get")]
    BenchRunGet { id: String },
    /// `factory bench cancel`.
    #[serde(rename = "bench.cancel")]
    BenchRunCancel { id: String },
    /// `factory bench clean`. Owner-only: an explicit removal of exactly
    /// this finished run's worktrees and branches. Refused while the run is
    /// still going.
    #[serde(rename = "bench.clean")]
    BenchRunClean { id: String },
    /// The L6 Policy tab: every applicable control's status for `scope` and
    /// every scope below it, rolled up per framework over that subtree, plus
    /// every `n/a`, finding and catalogue -- so nothing is silent. `scope:
    /// None` means the whole instance. Read-only, like `Knowledge` and
    /// `Datasets` -- evaluated fresh on every request from the catalogues on
    /// disk, the knowledge index, and the attestations store, never a status
    /// table kept in step (ADR 0004).
    #[serde(rename = "policy")]
    Policy {
        #[serde(default)]
        scope: Option<String>,
    },
    /// The CRA Art. 14 reporting clock (`#157`, phase 1): the 24-hour early
    /// warning and 72-hour notification deadlines for every exploited L2
    /// finding and confirmed L4 security report over `scope`'s subtree (the
    /// whole instance when `scope` is `None`) -- the same rollup
    /// `Request::Policy` itself uses. Computed fresh on every read, like
    /// `Policy` (ADR 0004: no status table). Read-only.
    #[serde(rename = "policy.clock")]
    PolicyClock {
        #[serde(default)]
        scope: Option<String>,
    },
    /// One control's full detail at `scope`: its catalogue data, its status
    /// there, and its whole attestation history for that scope and its
    /// ancestors. Read-only.
    #[serde(rename = "policy.control")]
    PolicyControl {
        control: factory_direction::policy::ControlRef,
        scope: String,
    },
    /// Record an attestation -- the one check kind a person satisfies by
    /// saying so (ADR 0004). `policy.attest`, checked against the root
    /// scope like `knowledge.write`: an attestation speaks for the company,
    /// not for one project, even when it is recorded for a nested scope's
    /// control. `expires_at` is absolute; a CLI that wants to accept `30d`
    /// or a bare date converts it before sending this (`#78`).
    #[serde(rename = "policy.attest")]
    PolicyAttest {
        control: factory_direction::policy::ControlRef,
        scope: String,
        /// A pointer to the evidence, not the evidence itself.
        evidence: String,
        #[serde(default)]
        note: Option<String>,
        expires_at: chrono::DateTime<chrono::Utc>,
        /// A submission against the CRA Art. 14 reporting clock (`#157`,
        /// phase 1) -- absent for an ordinary attestation. `policy_attest`
        /// refuses it against any control but `cra/art-14`, an item that
        /// does not exist or belongs to a scope other than the canonical
        /// `scope` above, an excluded item, or a deadline that already has
        /// a live (unwithdrawn) submission. Uses the same grant and
        /// root-scope reach as an ordinary attestation -- not a new door.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clock: Option<factory_direction::reporting_clock::ClockMark>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        corrective: Option<factory_direction::reporting_clock::CorrectiveMeasureMark>,
    },
    /// Withdraw a previously recorded attestation. Append-only like the rest
    /// of the store: this writes a new row that references `id`, and never
    /// touches the one it names. `policy.attest`, the same grant as
    /// recording one.
    #[serde(rename = "policy.withdraw")]
    PolicyWithdraw {
        id: String,
        #[serde(default)]
        reason: Option<String>,
    },
    /// Close a gap: create the task that carries `control`'s own
    /// `remediation:` guidance and its current missing evidence, in `scope`.
    /// `#83`. Needs `task.create` in `scope`, under exactly the same reach
    /// rule `Request::TaskCreate` itself needs (`access.rs` maps both to
    /// `Grant::TaskCreate` and checks `scope` against the caller's own the
    /// same way) -- this is not a second door into task creation, it is the
    /// same one. Refused, naming the existing task, when a non-terminal
    /// task labelled `policy=<framework>/<id>` is already open in `scope`;
    /// refused outright when the control is already `satisfied`, `attested`,
    /// or `n/a` there. Answered with `Payload::Task`, the same as
    /// `Request::TaskCreate` -- it is an ordinary task in every way but how
    /// it was asked for.
    #[serde(rename = "policy.remediate")]
    PolicyRemediate {
        control: factory_direction::policy::ControlRef,
        scope: String,
        #[serde(default)]
        agent: Option<String>,
    },
    /// A snapshot of `Request::Policy { scope }` for an auditor:
    /// `policy_export::PolicyExport`, rendered as `format` (`"md"` or
    /// `"json"`; anything else is refused). `#83`. Read-only, like `Policy`
    /// itself.
    #[serde(rename = "policy.export")]
    PolicyExport {
        #[serde(default)]
        scope: Option<String>,
        format: String,
    },
    /// Every named metric, computed fresh (`factory-daemon`'s `metrics.rs`)
    /// -- lazy like a policy fact: only the ids actually asked are computed.
    /// `ids` empty means every non-parameterised metric plus whatever the
    /// loaded goals catalogue and policy catalogues imply (see
    /// `Engine::metrics`'s own doc comment). Read-only.
    #[serde(rename = "metrics")]
    Metrics {
        #[serde(default)]
        ids: Vec<factory_assurance::metrics::MetricId>,
        /// Only this scope and its descendants. Absent means the whole
        /// instance; definitions marked `instance_wide` ignore it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
        /// Override the run-backed metrics' established default intervals.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<factory_assurance::metrics::MetricsWindow>,
    },
    /// The dashboard's resolved layout for `scope` (`#159`, phase 4 of
    /// `#150`): the nearest `dashboard:` block down `Scope.path`, the
    /// instance root's own top-level `dashboard:`, or `None` for "use the
    /// built-in default" -- see `Config::dashboard_for_scope`. `scope`
    /// absent means the instance root itself. An unknown scope is refused
    /// (`FactoryError::NoSuchScope`), unlike `roles_for`'s tolerant fallback
    /// to the built-in roles for a scope that has since gone: a dashboard
    /// request names a place to show, and a place that resolves to nothing
    /// has no dashboard to show, rather than silently substituting the
    /// root's. Read-only.
    #[serde(rename = "dashboard")]
    Dashboard {
        #[serde(default)]
        scope: Option<String>,
    },
    /// Save `scope`'s own dashboard layout whole (`#160`, phase 5 of `#150`):
    /// the instance root's own configured scope writes its top-level
    /// `dashboard:`, any other scope writes `scope.dashboard` in its own
    /// config -- the same split `Request::RoleDefine` makes for a role
    /// layer, and `scope` is required for the same reason: naming which
    /// scope's own block this is, not a fallback to the caller's. Refused
    /// (`FactoryError::BadRequest`) when `tiles` is empty, names an unknown
    /// metric, or is otherwise invalid -- `Config::validate` checked whole,
    /// the same gate every other write here passes through before a byte is
    /// written. Answered with the resolved `Payload::Dashboard { tiles,
    /// source }` for `scope`, same as `Request::Dashboard` would answer
    /// right after. Needs `dashboard.edit` in `scope`.
    #[serde(rename = "dashboard.set")]
    DashboardSet {
        scope: String,
        tiles: Vec<factory_composition::dashboard::Tile>,
    },
    /// Remove `scope`'s own `dashboard:` block and reveal whatever it was
    /// overriding -- the nearest ancestor's layout, or the built-in default.
    /// Refused when `scope` writes no block of its own to remove. Answered
    /// like `Request::DashboardSet`. Needs `dashboard.edit` in `scope`.
    #[serde(rename = "dashboard.reset")]
    DashboardReset { scope: String },
    /// Set a declared secret's metadata -- `expires`, `renew` and `note`,
    /// replaced whole; a field left out is removed -- in the instance
    /// root's `secrets:` (`#244`). The only write to the catalogue, and it
    /// never carries, writes, reads back or returns a value: the entry's
    /// `source` is never touched. Journaled with who changed what.
    /// Answered with `Payload::Secret`. Needs `secrets.edit`.
    #[serde(rename = "secret.set")]
    SecretSet {
        name: String,
        #[serde(default)]
        metadata: factory_environment::secrets::SecretMetadata,
    },
    /// The L6 Goals tab: vision, mission, the north star and its inputs,
    /// every cycle's own summary, the asked-for (or current) cycle's full
    /// graded report, and the roadmap -- narrowed to `scope` (and its
    /// descendants) when given, the whole instance otherwise. Read-only,
    /// evaluated fresh from `.factory/goals/` on every call, like `Policy`.
    #[serde(rename = "goals")]
    Goals {
        #[serde(default)]
        scope: Option<String>,
        #[serde(default)]
        cycle: Option<String>,
    },
    /// Record a check-in against a manual key result -- the one way a
    /// manual key result's value ever moves, the same append-only pattern
    /// `PolicyAttest` writes attestations with. `goals.checkin`, checked
    /// against the root scope for the same reason `policy.attest` is: a
    /// check-in speaks for the company's own goals. Refused when `kr` names
    /// no key result in any loaded cycle, or one that is not `manual: true`
    /// -- a computed key result cannot be checked in -- or when
    /// `confidence` is outside `0..=10` or `value` is not finite.
    #[serde(rename = "goals.checkin")]
    GoalsCheckIn {
        kr: factory_direction::goals::KrRef,
        value: f64,
        confidence: u8,
        #[serde(default)]
        note: Option<String>,
    },
    /// `#100`: the L6 Scenarios tab. Every scenario at
    /// `<root>/.factory/scenarios/`, played out against the exact policy
    /// evaluator, a seeded Monte Carlo forecast, the driver tree and
    /// signposts -- alongside the baseline they are all compared against.
    /// `scope: None` means the whole instance, the same "roll up the
    /// subtree" rule `Request::Policy`/`Request::Goals` already follow.
    /// Read-only, evaluated fresh on every call: a scenario file never
    /// changes the real config (`ScenariosReport`'s own doc comment).
    #[serde(rename = "scenarios")]
    Scenarios {
        #[serde(default)]
        scope: Option<String>,
    },
    /// Turn a chosen scenario into real work: one task per newly-open
    /// control in `scope`'s own slice of that scenario's policy delta,
    /// through the exact path `Request::TaskCreate`/`Request::PolicyRemediate`
    /// themselves use. `#100`. An explicit owner/agent action -- a scenario
    /// itself never writes anything (design §8, the same guardrail
    /// `Request::PolicyRemediate` already answers to). Needs `task.create`
    /// in `scope`, the same reach rule `Request::TaskCreate` is checked
    /// against (`access.rs` maps both to `Grant::TaskCreate`, no wildcard).
    #[serde(rename = "scenario.promote")]
    ScenarioPromote {
        scenario: String,
        scope: String,
        #[serde(default)]
        agent: Option<String>,
    },
    /// Recompute a scenario's driver outcomes, tornado and forecast with
    /// slider overrides applied server-side -- pure and read-only, `#100`,
    /// for the UI's driver panel to call on every slider change (debounced
    /// client-side). `scenario: None` starts from the built-in baseline
    /// drivers alone, with no policy, goal or signpost context layered on
    /// top -- a bare what-if over the driver tree. `drivers` uses the same
    /// override syntax a scenario file's own `drivers:` map does (`×2`,
    /// `+20%`, `+5`, `=0.9`) and is layered over the named scenario's own
    /// `drivers:` overrides, last write wins -- see
    /// `ScenarioWhatIfResult`'s own doc comment for exactly how.
    #[serde(rename = "scenario.whatif")]
    ScenarioWhatIf {
        #[serde(default)]
        scope: Option<String>,
        #[serde(default)]
        scenario: Option<String>,
        #[serde(default)]
        drivers: std::collections::BTreeMap<factory_direction::scenario::DriverId, String>,
    },
    /// `#107`: the L6 Quality attributes tab. Every scope's merged utility
    /// tree (`.factory/quality/` profiles bound by the root's `quality:` and
    /// each scope's `scope.quality`), every scenario judged against the
    /// same metric registry Goals reads and the same policy evidence the
    /// Policy tab gathers. `scope: None` is the whole instance; a scope
    /// narrows to it and its descendants, the "roll up the subtree" rule
    /// `Request::Policy` follows. Read-only, evaluated fresh on every call.
    #[serde(rename = "quality")]
    Quality {
        #[serde(default)]
        scope: Option<String>,
    },
    /// Create the task that closes one quality scenario's gap in `scope`,
    /// labelled `quality=<scope>/<attribute>/<scenario>` -- the same door
    /// `Request::PolicyRemediate` opens, through `Engine::create`, needing
    /// `task.create` in `scope` under the same reach rule. Refused when the
    /// scenario is already `met`, or a `draft` (with no measure there is no
    /// gap to close, only a measure to write). When a non-terminal task
    /// with that label is already open in `scope`, answers *that* task,
    /// `created: false`, and creates nothing (`#98`).
    #[serde(rename = "quality.remediate")]
    QualityRemediate {
        scope: String,
        /// The attribute id, `characteristic[.sub-characteristic]`.
        attribute: String,
        scenario: String,
        #[serde(default)]
        agent: Option<String>,
    },
    /// `#106`: the L4 Line tab and `factory stats` -- what needs a
    /// human now, where work is stuck, and how the line has been running
    /// over `window`. A read projection over tasks, runs, standing agents
    /// and the journal, computed fresh on every call like
    /// `Request::Production` (`factory_core::operations`). `scope: None` is
    /// every scope; a scope means its whole subtree, resolved by
    /// `Scope.path` like the policy and scenario reports -- unlike
    /// production.rs, which matches a scope by its own name only.
    #[serde(rename = "operations")]
    Operations {
        #[serde(default)]
        scope: Option<String>,
        #[serde(default)]
        window: factory_composition::operations::HealthWindow,
        /// Include the charts' per-step and per-run detail
        /// (`Health::days`, `Health::finished_runs`). The Line tab
        /// asks for it; the Inbox and `factory stats` do not.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        detail: bool,
    },
    /// `#119`: hand work in through the intake gate instead of straight
    /// onto the line. Creates a task in `TaskStatus::Intake` with its
    /// `Intake` record -- no run, and none until it is released. Needs
    /// `intake.add` in the target scope (`#172`; `task.create` before it).
    /// An agent's request is recorded as source `agent`, whatever it says.
    #[serde(rename = "intake.add")]
    IntakeAdd(factory_process::intake::NewIntake),
    /// The Intake view and `factory intake list`: every intake item in the
    /// four columns, computed fresh (`factory_core::intake::board`). A scope
    /// means its whole subtree, as for `Operations`.
    #[serde(rename = "intake.board")]
    IntakeBoard {
        #[serde(default)]
        scope: Option<String>,
    },
    /// Start the triage node on an item: a task in the item's scope whose
    /// instructions are the generalised `ir:triage`, dispatched at once, that
    /// answers with `IntakeAssess`. Needs `intake.triage` and reach over the
    /// item (`#172`; `task.create` before it). Refused if the named `agent`
    /// -- or the scope's default, when none is named -- holds a role that
    /// may not `task.report`: a triage run that could never report its own
    /// result would start and stay stuck.
    #[serde(rename = "intake.triage")]
    IntakeTriage {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
    },
    /// Record an assessment on an item. With `decide`, also apply what the
    /// rules give -- ready or needs-info, never wontfix. Needs `intake.assess`
    /// and reach over the item -- or to be the run of the item's own triage
    /// task (`#172`; `task.edit` before it).
    #[serde(rename = "intake.assess")]
    IntakeAssess {
        id: String,
        assessment: factory_process::intake::Assessment,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        decide: bool,
    },
    /// Decide an item: release it, send it back, or close it. Needs
    /// `intake.decide` and the same reach as `IntakeAssess` (`#172`;
    /// `task.edit` before it). Releasing ready with `run: true` also needs
    /// `task.run`, exactly as a workflow-routed release already does.
    #[serde(rename = "intake.decide")]
    IntakeDecide {
        id: String,
        decision: factory_process::intake::Decision,
    },
    /// Add information to an item that is still in intake -- the answer to a
    /// needs-info, which puts it back in `received`. Needs `intake.info` with
    /// reach over the item, or to be the one who handed it in (`#172`;
    /// `task.create` before it).
    #[serde(rename = "intake.info")]
    IntakeInfo { id: String, text: String },
    /// Flag an item still in intake as a possible security report
    /// (`#170`) -- from anyone who could assess it: flagging only adds
    /// scrutiny. Needs `intake.assess` with the same reach `IntakeAssess`
    /// checks. Refused once the item already carries a flag of any kind.
    #[serde(rename = "intake.flag_security")]
    IntakeFlagSecurity { id: String, reason: String },
    /// A person confirms or dismisses a possible security report -- the one
    /// decision an agent never makes, whatever role it holds (`Needs::Owner`).
    /// Confirming needs no evidence; dismissing does, or it is refused.
    /// Journaled as `intake_security_confirmed`/`intake_security_dismissed`.
    #[serde(rename = "intake.security")]
    IntakeSecurity {
        id: String,
        verdict: factory_process::intake::SecurityVerdict,
        #[serde(default)]
        evidence: String,
    },
    /// Every confirmed security report over `scope`'s subtree (every scope,
    /// absent) -- `Engine::confirmed_security_reports`, read live off the
    /// store, including an item that has since left intake. The L4 fact the
    /// CRA reporting clock (`#157`, phase 2) will read; a plain read, needing
    /// nothing, like `IntakeBoard`.
    #[serde(rename = "intake.security_reports")]
    IntakeSecurityReports {
        #[serde(default)]
        scope: Option<String>,
    },
    /// Approve and post a decided GitHub item's triage comment and labels to
    /// the issue it came from (`#171`). The daemon never does this on its
    /// own -- a decision only records `awaiting_approval`; this request is
    /// the approval. Needs `intake.publish`, named exactly: never a
    /// wildcard, never in `foreman` or `triager`. Refused for anything not
    /// sourced from GitHub, for an item with no decision or no assessment to
    /// publish, and for one carrying a possible or confirmed security
    /// report -- posting triage details publicly would disclose it.
    #[serde(rename = "intake.publish")]
    IntakePublish { id: String },
    /// One task's usage and cost: every run's, and their sum (#117).
    /// Read-only, derived from what the runs already carry.
    #[serde(rename = "task.usage")]
    TaskUsage { id: String },
    /// One run's usage snapshots as taken -- dispatch, each turn end, run
    /// end -- answered or not. The record behind `Run::usage`.
    #[serde(rename = "run.usage")]
    RunUsage { id: String },
    /// Usage and cost summed over the runs that started in `[from, to)`,
    /// grouped (#117). `from` defaults to thirty days before `to`, `to` to
    /// now; `scope` narrows to that scope and its descendants.
    #[serde(rename = "costs")]
    Costs {
        #[serde(default)]
        group_by: factory_process::usage::CostGroupBy,
        #[serde(default)]
        from: Option<chrono::DateTime<chrono::Utc>>,
        #[serde(default)]
        to: Option<chrono::DateTime<chrono::Utc>>,
        #[serde(default)]
        scope: Option<String>,
    },
    /// L6 authored monthly scope limits and the single L4 spend read.
    /// UTC calendar month, with explicit unknown/partial amounts.
    #[serde(rename = "budget")]
    Budget {
        #[serde(default)]
        scope: Option<String>,
        #[serde(default = "factory_direction::budget::default_group_by")]
        group_by: factory_process::usage::CostGroupBy,
    },
    /// Turn this connection into an event stream. Only the socket interface
    /// answers this; HTTP uses its WebSocket instead.
    #[serde(rename = "subscribe")]
    Subscribe,
}

/// Struct variants throughout: an internally tagged enum can only carry a map,
/// and a `Tasks(Vec<Task>)` newtype would fail at serialization time rather
/// than at compile time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Payload {
    RunProvenance {
        records: Vec<factory_kernel::ArtifactProvenance>,
    },
    Ok,
    Status {
        status: StatusInfo,
    },
    Adapters {
        adapters: Vec<AdapterEntry>,
    },
    RuntimeConnections {
        runtimes: Vec<RuntimeConnectionView>,
    },
    Task {
        task: Task,
    },
    Tasks {
        tasks: Vec<Task>,
    },
    Workflow {
        workflow: WorkflowDefinition,
    },
    Workflows {
        workflows: Vec<WorkflowDefinition>,
    },
    WorkflowRun {
        run: WorkflowRun,
    },
    WorkflowRuns {
        runs: Vec<WorkflowRun>,
    },
    WorkflowLint {
        lint: WorkflowLint,
    },
    Attestations {
        attestations: Vec<factory_process::control_plan::StepAttestation>,
    },
    Run {
        run: Run,
    },
    Runs {
        runs: Vec<Run>,
    },
    Agent {
        agent: AgentSession,
    },
    Scopes {
        scopes: Vec<ScopeView>,
        /// Every registered agent adapter, once -- not copied onto each
        /// scope in `scopes`. `ScopeView.available` used to carry this same
        /// list per scope; a handful of scopes made that harmless, but
        /// discovery can make a scope list a few thousand entries long, at
        /// which point the repeated copy is the bulk of the payload for
        /// saying the same thing every time.
        #[serde(default)]
        available: Vec<String>,
        /// The roles that hold in every scope -- built-in and instance --
        /// so a picker can offer them.
        #[serde(default)]
        roles: Vec<RoleView>,
        /// The roles in effect in each scope whose set differs from `roles`,
        /// because it or a scope above it defines roles of its own. Keyed by
        /// scope name, and absent for every scope that simply inherits the
        /// instance's, for the same reason `available` is served once.
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        scope_roles: std::collections::BTreeMap<String, Vec<RoleView>>,
    },
    Roles {
        board: RoleBoard,
    },
    Entries {
        entries: Vec<TaskEntry>,
    },
    Text {
        text: String,
    },
    Deleted {
        deleted: bool,
    },
    Event {
        event: Event,
    },
    Occupancy {
        occupancy: Occupancy,
    },
    Production {
        production: Production,
    },
    Screen {
        screen: Screen,
    },
    SiteFootprint {
        footprint: SiteFootprint,
    },
    /// The L2 Environment page. There is deliberately no "everything here is
    /// reachable by every agent" sentence in this payload: it is a fact about
    /// how the daemon runs its agents, true whether or not this request
    /// succeeded, so the page states it from its own markup and goes on
    /// stating it when the fetch fails.
    Environment {
        sandboxes: Vec<SandboxRow>,
        credentials: Vec<CredentialRow>,
        /// The declared catalogue (`#244`), one row per entry.
        #[serde(default)]
        secrets: Vec<SecretRow>,
        /// Managed OpenShell providers whose credential is still written
        /// inline in a scope's config, not declared in the catalogue.
        #[serde(default)]
        undeclared: Vec<UndeclaredCredential>,
        /// The newest metadata changes, newest last.
        #[serde(default)]
        secret_changes: Vec<SecretChange>,
    },
    /// The answer to `Request::SecretSet`: the entry as it now stands.
    Secret {
        secret: SecretRow,
    },
    Attachment {
        attachment: Attachment,
    },
    Dependencies {
        report: DependenciesReport,
    },
    Doctor {
        report: DoctorReport,
    },
    /// The L1 Infrastructure page, read from the bottom up: the host, the
    /// daemon on it, the declared AI accounts above that with the agents
    /// each one serves, and the model agents no account claims yet. A host
    /// fact that cannot be read is `null`, never a failed request.
    Infrastructure {
        host: HostFacts,
        daemon: DaemonFacts,
        providers: Vec<ProviderRow>,
        unassigned: Vec<UnassignedAgent>,
        /// Whether each harness the config names starts (`#131`), as its
        /// last probe before a dispatch found it. Read from the daemon's
        /// cache: showing the page never probes anything. Left out when
        /// the config names no harness with a probe.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        harnesses: Vec<factory_agents::harness::HarnessRow>,
    },
    /// The L1 Mac tab's power mode (`#260`) -- the answer to both
    /// `Request::HostPowerMode` and `Request::HostPowerModeSet`.
    HostPowerMode {
        report: PowerModeReport,
    },
    /// The L1 Backup page -- see `backup::BackupReport`. Boxed for the same
    /// reason `Operations` is.
    Backup {
        report: Box<factory_infrastructure::backup::BackupReport>,
    },
    /// The answer to `Request::BackupRun`: the snapshot as taken.
    BackupRun {
        snapshot: factory_infrastructure::backup::Snapshot,
    },
    /// The answer to `Request::BackupVerify`: every step and its outcome.
    BackupVerify {
        verification: factory_infrastructure::backup::Verification,
    },
    /// The Operations tab -- see `environments::EnvironmentsReport`. Boxed
    /// for the same reason `Operations` is.
    Environments {
        report: Box<factory_infrastructure::environments::EnvironmentsReport>,
    },
    #[serde(rename = "environment_verification")]
    EnvironmentVerification {
        verification: factory_infrastructure::environments::DeployVerification,
    },
    #[serde(rename = "environment_samples")]
    EnvironmentSamples {
        page: factory_infrastructure::environments::SamplePage,
    },
    #[serde(rename = "release_detail")]
    ReleaseDetail {
        detail: Box<factory_infrastructure::environments::ReleaseDetail>,
    },
    #[serde(rename = "dependency_document")]
    DependencyDocument {
        attachment: factory_kernel::Attachment,
        document: serde_json::Value,
    },
    /// A deployment as recorded: the answer to `deploy.start` and
    /// `deploy.finish`.
    Deployment {
        deployment: Box<factory_infrastructure::environments::Deployment>,
    },
    DeploymentMirrorPlan {
        plan: factory_kernel::DeploymentMirrorPlan,
    },
    DeploymentMirror {
        receipt: factory_kernel::DeploymentMirrorFact,
    },
    /// The answer to `release.add`.
    ReleaseAdded {
        scope: String,
        release: factory_infrastructure::environments::ReleaseFacts,
    },
    /// The answer to `Request::BackupRestore`: the newly materialized root.
    BackupRestore {
        restoration: factory_infrastructure::backup::Restoration,
    },
    /// The L5 Knowledge tab. `present: false` when the vault
    /// (`<root>/.factory/knowledge/`) does not exist -- an empty state, not
    /// an error -- with `root` still naming the path that was looked in, and
    /// `legacy` naming `<root>/knowledge/wiki` when that v1 path exists. No
    /// page body text and no document bytes are ever in here; see
    /// `knowledge::index`.
    Knowledge {
        root: String,
        present: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        legacy: Option<String>,
        pages: Vec<Page>,
        tags: Vec<Tag>,
        documents: Vec<Document>,
        gaps: Vec<Gap>,
        findings: Vec<Finding>,
    },
    /// The answer to `Request::KnowledgeSearch`, best first, with the name of
    /// the provider that ranked it. `vault` is the vault's absolute path, so
    /// a hit's page is `<vault>/<page>.md` -- the one step from an id to the
    /// file an agent then opens for itself.
    KnowledgeHits {
        provider: String,
        vault: String,
        hits: Vec<factory_kernel::KnowledgeHit>,
    },
    /// The result of `Request::KnowledgeImport`/`KnowledgeAdd`/
    /// `KnowledgeWriteFile` -- never a body: `copied` and the rest are
    /// vault-relative paths, never bytes. See `knowledge::WriteResult`.
    KnowledgeWrite {
        copied: Vec<String>,
        skipped_existing: Vec<String>,
        skipped_hidden: Vec<String>,
        refused: Vec<Refusal>,
        truncated: bool,
    },
    /// The L5 Benchmarks tab. Every configuration is `pinned: false` today --
    /// see `benchmark::Configuration`.
    Benchmarks {
        configurations: Vec<Configuration>,
    },
    /// Every dataset, summarized -- see `dataset::DatasetSummary`.
    Datasets {
        root: String,
        datasets: Vec<factory_assurance::dataset::DatasetSummary>,
    },
    /// One dataset in full, with the findings against the live config a
    /// summary already carries too.
    Dataset {
        dataset: factory_assurance::dataset::Dataset,
        findings: Vec<factory_assurance::dataset::DatasetFinding>,
    },
    /// A bench run, with its attempts and the results aggregated from them
    /// -- see `bench::aggregate`.
    BenchRun {
        run: factory_assurance::bench::BenchRun,
        results: Vec<factory_assurance::bench::BenchResult>,
    },
    /// Every bench run, most recently updated first.
    BenchRuns {
        runs: Vec<factory_assurance::bench::BenchRun>,
    },
    /// The L6 Policy tab -- see `PolicyReport`.
    Policy {
        report: PolicyReport,
    },
    /// The answer to `Request::PolicyClock`.
    PolicyClock {
        clock: factory_direction::reporting_clock::ReportingClock,
    },
    ImportantDates {
        report: Box<factory_infrastructure::renewals::ImportantDatesReport>,
    },
    /// The answer to `Request::PolicyControl`.
    PolicyControl {
        detail: PolicyControlDetail,
    },
    /// The answer to `Request::PolicyAttest`/`Request::PolicyWithdraw`: the
    /// attestation as it now stands -- withdrawn, for the latter.
    PolicyAttestation {
        attestation: factory_direction::policy::Attestation,
    },
    /// The answer to `Request::PolicyExport`: the rendered body, its
    /// `format` (echoed back so a caller need not remember what it asked
    /// for), and the filename `policy_export::export_filename` computed for
    /// it -- the `http` interface's `Content-Disposition` and the CLI both
    /// read straight off this rather than re-deriving either.
    PolicyExport {
        format: String,
        filename: String,
        body: String,
    },
    /// The answer to `Request::Metrics`: every requested (or implied)
    /// metric's computed value, a history series for the ones that have
    /// one, and the definition behind each value -- exactly the ids in
    /// `values`, not the whole registry (`factory_assurance::metrics::registry()`,
    /// which a caller wanting the full vocabulary reads directly, off the
    /// wire in no request at all -- it is fixed and compiled in, not
    /// something the daemon computes).
    Metrics {
        values: Vec<factory_assurance::metrics::MetricValue>,
        series: Vec<factory_assurance::metrics::MetricSeries>,
        registry: Vec<MetricDefView>,
    },
    /// The answer to `Request::Dashboard`: the resolved tile list, or `null`
    /// on the wire for "no block anywhere in the chain names one -- use the
    /// built-in default" (the default list itself lives in `dashboard-model.js`,
    /// not here), and `source` naming the scope whose `dashboard:` block
    /// answered -- `null` for the same built-in-default case, never a magic
    /// string like `"root"` or `"default"`: a scope can be named either of
    /// those, and `source` must never be mistaken for one.
    Dashboard {
        tiles: Option<Vec<factory_composition::dashboard::Tile>>,
        source: Option<String>,
    },
    /// The L6 Goals tab -- see `GoalsReport`.
    Goals {
        report: GoalsReport,
    },
    /// The answer to `Request::GoalsCheckIn`: the check-in as recorded.
    GoalsCheckIn {
        checkin: factory_direction::goals::CheckIn,
    },
    /// The L6 Scenarios tab -- see `ScenariosReport`.
    Scenarios {
        report: ScenariosReport,
    },
    /// The answer to `Request::ScenarioPromote` -- see `ScenarioPromoteResult`.
    ScenarioPromote {
        result: ScenarioPromoteResult,
    },
    /// The answer to `Request::ScenarioWhatIf` -- see `ScenarioWhatIfResult`.
    ScenarioWhatIf {
        result: ScenarioWhatIfResult,
    },
    /// The L6 Quality attributes tab -- see `QualityReport`.
    Quality {
        report: QualityReport,
    },
    /// The answer to `Request::QualityRemediate` -- see `QualityRemediation`.
    QualityRemediate {
        result: QualityRemediation,
    },
    /// The L4 Line tab -- see `factory_core::operations::OperationsReport`.
    /// Boxed: the report is several times the size of every other payload,
    /// and would otherwise set the size of every `Response` (serde writes a
    /// box as what it holds).
    Operations {
        report: Box<factory_composition::operations::OperationsReport>,
    },
    /// The Intake view -- see `factory_core::intake::IntakeBoard`.
    IntakeBoard {
        board: factory_process::intake::IntakeBoard,
    },
    /// `Request::IntakeSecurityReports`' answer.
    IntakeSecurityReports {
        reports: Vec<factory_process::intake::ConfirmedSecurityReport>,
    },
    /// `Request::TaskUsage`'s answer.
    TaskUsage {
        usage: factory_process::usage::TaskUsage,
    },
    /// `Request::RunUsage`'s answer.
    UsageSnapshots {
        snapshots: Vec<factory_process::usage::UsageSnapshot>,
    },
    /// `Request::Costs`' answer.
    Costs {
        report: factory_process::usage::CostReport,
    },
    Budget {
        report: factory_direction::budget::Report,
    },
}

/// A request plus who is making it.
///
/// The token is how an agent says which agent it is. Absent means the owner --
/// the person at the socket. That is not a security boundary: every agent runs
/// as the owner and can read the socket, so one that leaves the token out is
/// indistinguishable from a person. It keeps agents inside their role by
/// accident, not against intent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    #[serde(flatten)]
    pub request: Request,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl From<Request> for Envelope {
    fn from(request: Request) -> Self {
        Self {
            request,
            token: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ok { data: Payload },
    Error { code: String, message: String },
}

impl Response {
    pub fn ok(data: Payload) -> Self {
        Self::Ok { data }
    }

    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Error {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// One declared agent's `max_sessions` picture, for `factory status`
/// (`#179`). Only an agent that declares its own cap gets a row -- a
/// scope-wide cap alone already shows in `factory stats`' Flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapacityRow {
    pub scope: String,
    pub agent: String,
    pub in_use: u32,
    pub max: u32,
    pub waiting: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusInfo {
    pub instance: String,
    pub instance_id: String,
    pub root: String,
    pub version: String,
    pub uptime_seconds: u64,
    pub tasks_total: usize,
    pub tasks_active: usize,
    pub subscribers: usize,
    pub interfaces: Vec<String>,
    /// Every agent that declares its own `max_sessions`, across every scope.
    /// Absent on a build old enough to predate this, which reads as no
    /// declared agent caps -- the same as today.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capacity: Vec<CapacityRow>,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterEntry {
    pub kind: String,
    pub name: String,
    pub description: String,
    /// `builtin` or `plugin:<manifest path>`.
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterList {
    pub adapters: Vec<AdapterEntry>,
}

/// One runtime adapter connection and every scope whose effective
/// configuration points at it. The probe result is flattened so the wire
/// reads as one diagnostic card rather than a wrapper around one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConnectionView {
    pub runtime: String,
    pub source: String,
    pub description: String,
    pub scopes: Vec<String>,
    pub checked_at: chrono::DateTime<chrono::Utc>,
    #[serde(flatten)]
    pub diagnostic: RuntimeConnectionDiagnostic,
}

impl From<AdapterList> for Payload {
    fn from(list: AdapterList) -> Self {
        Payload::Adapters {
            adapters: list.adapters,
        }
    }
}

/// One thing an agent is doing right now.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentActivity {
    pub run_id: String,
    pub task_id: String,
    pub task_title: String,
    pub scope: String,
    pub attempt: u32,
    pub status: String,
    pub runtime: String,
    pub trigger: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// The runtime's handle for the session, so a person can find the window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

/// One agent in one scope, as the agents page shows it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentView {
    /// The standing agent's session id, when it is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// What it is called in this scope.
    pub name: String,
    /// The adapter behind it.
    pub adapter: String,
    pub description: String,
    /// `builtin` or `plugin:<manifest path>`.
    pub source: String,
    /// `permanent`, `temporary`, or `task`.
    pub lifetime: String,
    /// The role it is working under -- the config's, unless somebody gave it
    /// another.
    pub role: String,
    /// `none`, `docker`, `srt`, or `openshell` -- what the declaration
    /// says. Only `openshell` is enforced; see `Sandbox`'s doc comment.
    pub sandbox: String,
    /// Set when a person gave it this role, so the roster can say that the
    /// config says something else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_role: Option<String>,
    pub autostart: bool,
    /// For a standing agent: `starting`, `ready`, `gone`, `stopped`.
    /// For a task agent: `task`.
    pub state: String,
    /// True when tasks in this scope use it unless they say otherwise.
    pub is_default: bool,
    /// False once the config stops declaring it.
    pub declared: bool,
    /// True only for a declaration physically owned by this scope's config.
    /// Synthesized foremen and bare adapter defaults cannot be deleted here.
    pub deletable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attach: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Task runs this agent is working on in this scope right now.
    pub active: Vec<AgentActivity>,
    /// For a `sandbox: openshell` agent, whether its sandbox prerequisites
    /// are in place ahead of any run (`#234`). Absent for every other agent
    /// and from an older daemon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readiness: Option<factory_environment::openshell::Readiness>,
}

/// One role, as the roster, the pickers and the Roles view show it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleView {
    pub name: String,
    pub describe: String,
    /// What it may do, written the way the config writes it.
    pub grants: Vec<String>,
    /// `own` or `scope`.
    pub reach: String,
    /// Where this definition was written.
    #[serde(default)]
    pub origin: RoleOrigin,
    /// The inherited definition this one replaces, when it replaces one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overrides: Option<RoleOrigin>,
    /// The agents in the scope asked about that hold this role. Only filled in
    /// when the answer is about one scope.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub held_by: Vec<RoleHolder>,
}

/// An agent holding a role, and how it came to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleHolder {
    pub name: String,
    /// Given with `factory agent role` rather than declared in the config.
    #[serde(default)]
    pub given: bool,
}

/// One grant in the words a person reads, so no view keeps its own copy of
/// the vocabulary `role.rs` exists to keep in one place.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrantView {
    /// As the config writes it: `task.create`.
    pub name: String,
    /// As `Grant::describe` says it: `create tasks`.
    pub describe: String,
    /// `Tasks`, `Agents`, `Runs` or `Workflows`.
    pub group: String,
}

/// One place roles are written: Factory's own, the instance root's, or a
/// scope's `scope.roles`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleLayer {
    pub origin: RoleOrigin,
    /// The scope's path, for a scope layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The nearest scope above this one that also writes roles. `None` sits
    /// the layer directly under the instance's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// The definitions written here, each saying which one it replaces.
    pub roles: Vec<RoleView>,
}

/// The L3 Roles view in one answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleBoard {
    /// Every grant there is, in `Grant::ALL` order.
    pub grants: Vec<GrantView>,
    /// The scope `roles` is about, by its canonical name. `None` for all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// The layer that scope writes when a role is defined there: the instance
    /// root's `roles:` for the root scope, its own `scope.roles` otherwise.
    /// A role whose origin is this one is "defined here".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writes: Option<RoleOrigin>,
    /// Every role in effect in `scope`, with who holds each -- or, with no
    /// scope, the roles that hold everywhere.
    pub roles: Vec<RoleView>,
    /// Every layer that writes roles: Factory's, the instance root's, then
    /// each scope that defines roles of its own, in path order.
    pub layers: Vec<RoleLayer>,
}

// Canonical L6 response data; transport envelopes stay outside the ladder.
pub use factory_direction::policy_report::{
    CatalogueSummary, NotApplicableEntry, PolicyControlDetail, PolicyReport, ScopePolicy,
    WorkflowEnforcement, WorkflowEnforcementFinding,
};

/// `factory_assurance::metrics::MetricDef`, with its two `&'static str` fields turned
/// into owned `String`s so it can cross the wire and come back --
/// `MetricDef` itself stays `Serialize`-only (see its own doc comment: it
/// is a fixed, compiled-in vocabulary, never something a caller builds),
/// so this is the view `Payload::Metrics::registry` actually carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricDefView {
    pub id: String,
    pub title: String,
    pub description: String,
    pub unit: factory_assurance::metrics::Unit,
    pub better: factory_assurance::metrics::Better,
    pub coverage: factory_assurance::metrics::MetricCoverage,
    pub source: String,
    pub available: bool,
    pub unavailable_reason: Option<String>,
}

impl From<factory_assurance::metrics::MetricDef> for MetricDefView {
    fn from(d: factory_assurance::metrics::MetricDef) -> Self {
        Self {
            id: d.id,
            title: d.title,
            description: d.description,
            unit: d.unit,
            better: d.better,
            coverage: d.coverage,
            source: d.source.to_string(),
            available: d.available,
            unavailable_reason: d.unavailable_reason.map(str::to_string),
        }
    }
}

/// One cycle's place in `GoalsReport::cycles`: enough to draw a picker or a
/// timeline without evaluating every cycle's full report, which
/// `Request::Goals` only ever does for the one asked (or current) cycle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CycleSummary {
    pub id: String,
    pub from: chrono::NaiveDate,
    pub to: chrono::NaiveDate,
    pub status: factory_direction::goals::CycleStatus,
    /// The mean of every scope-filtered objective's own score in this
    /// cycle -- `None` when none of them are scored yet, the same
    /// "unscored, not zero" rule `goals::ObjectiveResult::score` follows.
    pub score: Option<f64>,
}

/// The north star metric, as `Request::Goals` shows it: `direction.yaml`'s
/// own `why`, plus its current computed value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NorthStarView {
    pub metric: factory_assurance::metrics::MetricId,
    pub why: String,
    pub value: factory_assurance::metrics::MetricValue,
}

/// One of `direction.yaml`'s `inputs`, with its current computed value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputView {
    pub metric: factory_assurance::metrics::MetricId,
    pub value: factory_assurance::metrics::MetricValue,
}

/// The L6 Goals tab's whole answer: `Request::Goals`'s response. Goals
/// enforce nothing (design §8) -- this is a read of what is authored and
/// what the data says about it, never a status this itself computes and
/// keeps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoalsReport {
    /// `None` when the whole instance was asked about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<factory_direction::goals::Direction>,
    /// Every cycle on disk, oldest first, whatever cycle was asked for.
    pub cycles: Vec<CycleSummary>,
    /// The asked cycle's (or, with none named, the current one's) full
    /// graded report, scope-filtered the same way `cycles`' own scores are.
    /// `None` when no cycle was asked for and none is current right now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<factory_direction::goals::CycleReport>,
    pub findings: Vec<factory_direction::goals::Finding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub north_star: Option<NorthStarView>,
    pub inputs: Vec<InputView>,
    /// Scope-filtered the same way `report`'s own objectives are -- an item
    /// with no `scope` of its own belongs to the root, exactly like an
    /// objective without one.
    pub roadmap: Vec<factory_direction::goals::RoadmapItem>,
    /// Every manual key result's own check-in history, oldest first, for a
    /// sparkline -- across every cycle, not narrowed to `report`'s own one,
    /// since a key result's history outlives the cycle it happens to be
    /// asked about. `goals::KrResult::confidence` already carries the
    /// latest one; this is the series behind it.
    pub checkins: std::collections::BTreeMap<
        factory_direction::goals::KrRef,
        Vec<factory_direction::goals::CheckIn>,
    >,
}

// ============================================================= scenarios

/// One scope's exact policy delta under a scenario -- `factory_core::scenario::PolicyDelta`
/// paired with which scope it is about, since a report carries one per scope
/// in the asked subtree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioScopeDelta {
    pub scope: String,
    pub delta: factory_direction::scenario::PolicyDelta,
}

/// The state every scenario in a `ScenariosReport` is compared against --
/// computed once per request and shared, the same "compute once, project
/// many ways" shape `Request::Policy`'s own dataset/daemon facts already
/// follow, rather than recomputed per card.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioBaseline {
    /// Every metric id any loaded scenario's signposts, goal changes, or the
    /// built-in driver tree itself reference -- computed once
    /// (`Engine::metrics`), not once per scenario.
    pub metrics: Vec<factory_assurance::metrics::MetricValue>,
    /// `factory_core::scenario::driver_defs()`'s own baseline value for
    /// every driver: a registry-backed driver's current metric value (when
    /// it has one to read), `capacity_factor`'s neutral `1.0` (no
    /// adjustment, the same default `evaluate_outcomes` itself falls back to
    /// when a driver is absent), and `rework_rate`'s neutral `0.0` (no
    /// rework) -- see `Engine::driver_baseline`'s own doc comment for why
    /// those two, not `evaluate_outcomes`' formula, get a value here.
    pub drivers: std::collections::BTreeMap<factory_direction::scenario::DriverId, f64>,
    /// The current policy rollup over the asked subtree -- the same
    /// `policy::FrameworkRollup` list `PolicyReport::rollup` carries,
    /// computed from the very statuses every scenario's own delta is
    /// diffed against, not a second, separate call to `Request::Policy`.
    pub policy: Vec<factory_direction::policy::FrameworkRollup>,
    /// What happens with no scenario at all: `forecast_completion` over the
    /// asked subtree's own weekly throughput history, backlog = every
    /// non-terminal task in scope right now, `Horizon::default()`'s 26
    /// weeks -- the same `Forecast` shape every `ScenarioResult::forecast`
    /// carries, so a fan chart can draw baseline and scenario side by side.
    /// See `Engine::scenarios_report`'s own doc comment for why this reading
    /// of "baseline forecast" was chosen over a bare metric trend.
    pub forecast: factory_direction::scenario::Forecast,
}

/// One scenario's own computed answer -- the exact policy delta, the driver
/// outcomes and tornado, the Monte Carlo forecast, goal-scenario
/// probabilities, and signposts, all evaluated against
/// `ScenariosReport::baseline`. See the module doc comment on
/// `factory_core::scenario` for how these three kinds of answer (exact,
/// probabilistic, qualitative) are never mixed into one number.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioResult {
    pub scenario: factory_direction::scenario::Scenario,
    /// This scenario's own findings -- its load-time validation, its own
    /// staleness check, and its own `overlay_chain` findings -- a subset of
    /// `ScenariosReport::findings` repeated here so a card can show just its
    /// own without re-filtering the whole report's list by subject.
    pub findings: Vec<factory_direction::scenario::Finding>,
    /// Empty when the scenario carries no `policy:` overlay at all -- there
    /// is nothing to diff.
    pub policy: Vec<ScenarioScopeDelta>,
    /// `policy`, aggregated over the whole asked subtree the same way
    /// `PolicyReport::rollup` aggregates per-scope statuses --
    /// `scenario::policy_delta` over each side's own `policy::worst_across_scopes`.
    pub policy_subtree: factory_direction::scenario::PolicyDelta,
    pub drivers: ScenarioDrivers,
    /// The backlog `forecast` was run against, and where it came from --
    /// carried alongside the forecast itself since "how big is the backlog"
    /// is exactly what a scenario's policy delta and `goals:` labels decide,
    /// not a fixed number a card can otherwise guess at.
    pub backlog: ScenarioBacklog,
    pub forecast: factory_direction::scenario::Forecast,
    /// One per `scenario.goals` entry, in authored order.
    pub goals: Vec<ScenarioGoalProbability>,
    pub signposts: Vec<factory_direction::scenario::SignpostStatus>,
}

/// A scenario's own driver tree: the shared baseline, this scenario's own
/// overrides applied on top, the outcome before and after, and the tornado
/// ranking the swing by driver -- everything `ScenarioDrivers` needs to draw
/// a driver panel and its drill-down without a second round trip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioDrivers {
    pub overridden: std::collections::BTreeMap<factory_direction::scenario::DriverId, f64>,
    pub outcomes_before: std::collections::BTreeMap<factory_direction::scenario::OutcomeId, f64>,
    pub outcomes_after: std::collections::BTreeMap<factory_direction::scenario::OutcomeId, f64>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub outcome_reasons_before:
        std::collections::BTreeMap<factory_direction::scenario::OutcomeId, String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub outcome_reasons_after:
        std::collections::BTreeMap<factory_direction::scenario::OutcomeId, String>,
    /// Sensitivity for every measured outcome, including USD and tokens.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub tornados: std::collections::BTreeMap<
        factory_direction::scenario::OutcomeId,
        Vec<factory_direction::scenario::TornadoBar>,
    >,
    /// Against `effective_throughput`, retained for existing clients.
    pub tornado: Vec<factory_direction::scenario::TornadoBar>,
}

/// Where a scenario's `forecast` backlog came from -- see
/// `Engine::scenarios_report`'s own doc comment for the exact rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioBacklog {
    pub total: f64,
    /// Newly-open controls, from this scenario's own `policy_subtree` delta
    /// -- one remediation item each.
    pub newly_open_controls: usize,
    /// Non-terminal tasks labelled `goal=<objective>/<kr>` for one of this
    /// scenario's own `goals:` entries, summed across every entry.
    pub open_goal_tasks: usize,
}

/// One `scenario.goals` entry's re-scored probability -- see
/// `factory_core::scenario::goal_probability`. `target`/`by` are the
/// *effective* values (the change's own, or the key result's/cycle's
/// current one when the change leaves it unnamed), not merely echoing the
/// authored `GoalChange`, so a card never has to re-resolve what "unwritten"
/// defaulted to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioGoalProbability {
    pub kr: factory_direction::goals::KrRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<chrono::NaiveDate>,
    pub probability: factory_direction::scenario::GoalProbability,
}

/// A signpost that is `Triggered`, named alongside the scenario it belongs
/// to -- `ScenariosReport::triggered`'s own element. Exists because a
/// triggered signpost is meant to be visible outside the Scenarios tab too
/// (on the dashboard, in the inbox, as an observation with no automatic
/// consequence -- design §8) and a reader building that view should not have
/// to walk every `ScenarioResult::signposts` and filter by state itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggeredSignpost {
    pub scenario: String,
    pub metric: factory_assurance::metrics::MetricId,
    pub reason: String,
}

/// The L6 Scenarios tab's whole answer: `Request::Scenarios`'s response.
/// `#100`. A scenario file never changes the real config -- everything here
/// is computed fresh, in memory, against `.factory/scenarios/`,
/// `.factory/policies/` (and its `drafts/` subdirectory), and whatever
/// `baseline` itself reads, on every call, like `PolicyReport`/`GoalsReport`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenariosReport {
    /// `None` when the whole instance was asked about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    pub baseline: ScenarioBaseline,
    /// Every scenario on disk, sorted by name (`scenario::load`'s own order).
    pub scenarios: Vec<ScenarioResult>,
    /// Load-time validation, staleness, and overlay findings --
    /// `factory_core::scenario::Finding`s only; a catalogue's own load or
    /// applicability findings (real or draft) are `policy_findings` below,
    /// the same split `PolicyReport::findings` already draws between a
    /// catalogue mistake and an applicability one.
    pub findings: Vec<factory_direction::scenario::Finding>,
    /// `policy::load_all`'s and `scenario::load_drafts`'s own catalogue
    /// findings, plus every scope's `policy::applicable`/`evidence_findings`
    /// output across both the baseline and every scenario's own evaluation,
    /// deduplicated.
    pub policy_findings: Vec<factory_direction::policy::Finding>,
    /// Every currently `Triggered` signpost across every scenario -- see
    /// `TriggeredSignpost`'s own doc comment for why this is surfaced
    /// outside `scenarios[].signposts` too.
    pub triggered: Vec<TriggeredSignpost>,
}

/// One control `Request::ScenarioPromote` created a task for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromotedControl {
    pub control: factory_direction::policy::ControlRef,
    pub task: Task,
}

/// One control `Request::ScenarioPromote` left alone, and why -- always
/// because a non-terminal task already carries that control's own
/// `policy=<framework>/<id>` label in the scope, the same rule
/// `Request::PolicyRemediate` refuses a second call under.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkippedControl {
    pub control: factory_direction::policy::ControlRef,
    pub existing_task: String,
}

/// The answer to `Request::ScenarioPromote`: one task created per
/// newly-open control in `scope`'s own slice of the scenario's policy
/// delta, and every control left alone because a remediation task for it is
/// already open.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioPromoteResult {
    pub scenario: String,
    pub scope: String,
    pub created: Vec<PromotedControl>,
    pub skipped: Vec<SkippedControl>,
}

/// The answer to `Request::ScenarioWhatIf`: the driver tree recomputed with
/// the request's own overrides layered over the named scenario's (if any),
/// and the forecast that follows from it -- see `Engine::scenario_whatif`'s
/// own doc comment for exactly how backlog is chosen with no scenario named.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioWhatIfResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario: Option<String>,
    pub drivers: ScenarioDrivers,
    pub forecast: factory_direction::scenario::Forecast,
}

/// One ISO 25010 characteristic as `QualityReport::catalogue` carries it:
/// `quality::CATALOGUE`'s entry, owned so it can be read back off the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CharacteristicView {
    pub id: String,
    pub title: String,
    pub subs: Vec<SubCharacteristicView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubCharacteristicView {
    pub id: String,
    pub title: String,
    pub standard: factory_assurance::quality::Standard,
}

impl From<&factory_assurance::quality::Characteristic> for CharacteristicView {
    fn from(c: &factory_assurance::quality::Characteristic) -> Self {
        CharacteristicView {
            id: c.id.to_string(),
            title: c.title.to_string(),
            subs: c
                .subs
                .iter()
                .map(|s| SubCharacteristicView {
                    id: s.id.to_string(),
                    title: s.title.to_string(),
                    standard: s.standard,
                })
                .collect(),
        }
    }
}

/// One scope's row in `QualityReport`: its evaluated utility tree, plus the
/// remediation task already open for any of its scenarios, so a reader can
/// show that task instead of offering to create another (`#98`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScopeQuality {
    #[serde(flatten)]
    pub report: factory_assurance::quality::ScopeReport,
    /// `<attribute>/<scenario>` to the id of the non-terminal task in this
    /// scope labelled `quality=<scope>/<attribute>/<scenario>`.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub open_tasks: std::collections::BTreeMap<String, String>,
}

/// The L6 Quality attributes tab's whole answer: `Request::Quality`'s
/// response. `#107`. Nothing here is stored -- the profiles are re-read,
/// the chains re-resolved and every scenario re-judged on each call.
/// There is deliberately no score anywhere, per scope or overall: an
/// attribute is the worst of its scenarios, and that is as far as any
/// rollup goes (`quality.rs`'s module doc comment).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QualityReport {
    /// `None` when the whole instance was asked about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Every scope in the asked subtree whose chain binds at least one
    /// profile, in config order. A scope binding none is omitted, not
    /// shown empty: an undeclared attribute is "not a stated concern",
    /// never "failing".
    pub scopes: Vec<ScopeQuality>,
    /// `quality::load`'s findings (subject: the profile file) and every
    /// listed scope's `quality::applicable` findings (subject: the scope),
    /// sorted and deduplicated.
    pub findings: Vec<factory_assurance::quality::Finding>,
    /// The nine ISO 25010 characteristics in the standard's own order, each
    /// with its sub-characteristics -- every column a heatmap draws, whether
    /// or not any scope declares it.
    pub catalogue: Vec<CharacteristicView>,
    /// The history of every metric a listed scenario measures by that has
    /// one (today the three production metrics), for a sparkline beside
    /// the scenario. A metric appears once however many scenarios read it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub series: Vec<factory_assurance::metrics::MetricSeries>,
}

/// The answer to `Request::QualityRemediate`: the task that now carries the
/// scenario's gap, and whether this call created it (`false`: one was
/// already open, and that one is returned instead).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityRemediation {
    pub task: Task,
    pub created: bool,
}

/// A scope and everything that runs in it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeView {
    /// Stable identity from this scope's local Factory config.
    pub id: String,
    pub name: String,
    pub path: String,
    /// The adapter a task here runs on unless it says otherwise.
    pub default_agent: String,
    pub runtime: String,
    pub agents: Vec<AgentView>,
    /// The adapters a task here can be started with, when that genuinely
    /// differs from every adapter the instance has registered. Nothing
    /// produces that today -- every scope can be started with any registered
    /// adapter -- so this is normally absent; `Payload::Scopes::available`
    /// carries the shared list instead. See its doc comment for why the
    /// duplication moved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available: Option<Vec<String>>,
    /// Where this scope's tasks are kept. The instance default unless the
    /// scope named something else.
    pub task_store: String,
    /// Every registered task-store adapter, so the page can say what the
    /// alternatives are.
    pub available_stores: Vec<String>,
    /// Whether this scope's directory can host a git worktree at all: a git
    /// repository with at least one commit to branch a worktree from. When
    /// it cannot, a task's "work in its own worktree" checkbox is disabled
    /// rather than quietly ignored.
    pub worktree_capable: bool,
    /// Why it cannot, in words fit to show next to the disabled checkbox.
    /// `None` exactly when `worktree_capable` is true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_reason: Option<String>,
}

/// Where one declared agent's runs execute, for the Sandboxes tab. Built
/// directly from `ScopeView`/`AgentView` rather than recomputed, so it can
/// never disagree with the roster the same fields already appear on there.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxRow {
    pub scope: String,
    pub scope_path: String,
    pub runtime: String,
    pub agent: String,
    pub harness: String,
    /// `permanent`, `temporary`, or `task`.
    pub lifetime: String,
    /// `none`, `docker`, `srt`, or `openshell`.
    pub sandbox: String,
    /// Whether dispatch actually puts this agent's runs inside `sandbox`
    /// (`Sandbox::is_enforced`) -- `openshell` only, today. Absent from an
    /// older daemon's answer, which enforced nothing.
    #[serde(default)]
    pub enforced: bool,
    pub worktree_capable: bool,
    /// See `AgentView::readiness`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readiness: Option<factory_environment::openshell::Readiness>,
}

/// One place on disk a credential might already sit, checked for existence
/// only -- see `Payload::Environment`. The value itself is never read, held,
/// or returned; `present` is the whole of what this says.
pub use factory_environment::credentials::CredentialRow;

/// One declared secret (`#244`), as the Secrets tab shows it. Everything
/// here is metadata or a yes/no about the source; never a value, and never
/// anything a source printed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretRow {
    pub name: String,
    pub kind: factory_environment::secrets::SecretKind,
    /// `file`, `command`, `env` or `keychain`.
    pub from: String,
    /// Where it lives, in words: the path, the command, the variable's
    /// name, the Keychain item.
    pub source: String,
    /// For a `file`: whether it is there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub present: Option<bool>,
    /// For a `file` that is there: owned by the daemon's user and readable
    /// by nobody else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_only: Option<bool>,
    /// Whether the source gave a value at the provisioner's last pass --
    /// it ran, or was found. `None` until the first pass has checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolves: Option<bool>,
    /// Why it did not. Never a value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<factory_environment::secrets::Expiry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days_left: Option<i64>,
    pub state: factory_environment::secrets::ExpiryState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renew: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Every scope, agent and provider that names it.
    #[serde(default)]
    pub used_by: Vec<SecretUse>,
}

/// One provider naming a secret: `scope / agent / provider`, the provider
/// by its name on the gateway (this instance's suffix included).
pub use factory_environment::credential_expiry::SecretUse;

/// A managed OpenShell provider whose credential is written inline in a
/// scope's config (`#234`) rather than declared in the catalogue: it works,
/// and the Secrets tab names it so it can be moved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UndeclaredCredential {
    pub scope: String,
    pub agent: String,
    pub provider: String,
    pub from: String,
    pub source: String,
    /// The provider's own `expires:`, if it gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<chrono::NaiveDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days_left: Option<i64>,
    pub state: factory_environment::secrets::ExpiryState,
}

/// One journaled metadata change. Metadata only, as the journal holds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretChange {
    pub at: chrono::DateTime<chrono::Utc>,
    pub secret: String,
    pub by: String,
    pub message: String,
}

pub use factory_infrastructure::host::{HostFacts, DiskFacts};

/// The host's macOS power mode (`#260`), as System Settings calls it
/// *Energy Mode*. Closed: these three are the only values the wire accepts,
/// and the daemon maps each to its `pmset powermode` number itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerMode {
    /// `powermode 0`: macOS decides.
    Automatic,
    /// `powermode 2`: High Power.
    HighPerformance,
    /// `powermode 1`: Low Power.
    EnergySaving,
}

impl PowerMode {
    /// In the order the segmented control draws them.
    pub const ALL: [PowerMode; 3] = [Self::Automatic, Self::HighPerformance, Self::EnergySaving];

    /// The wire name: `automatic`, `high_performance`, `energy_saving`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::HighPerformance => "high_performance",
            Self::EnergySaving => "energy_saving",
        }
    }

    /// What a person reads.
    pub fn label(self) -> &'static str {
        match self {
            Self::Automatic => "Automatic",
            Self::HighPerformance => "High performance",
            Self::EnergySaving => "Energy saving",
        }
    }
}

impl std::fmt::Display for PowerMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

impl std::str::FromStr for PowerMode {
    type Err = String;

    /// The wire name, with `-` accepted for `_` so a CLI argument reads
    /// naturally. Nothing else: not a number, not a near miss.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.replace('-', "_").as_str() {
            "automatic" => Ok(Self::Automatic),
            "high_performance" => Ok(Self::HighPerformance),
            "energy_saving" => Ok(Self::EnergySaving),
            _ => Err(format!("{s:?} is not a power mode; use automatic, high-performance or energy-saving")),
        }
    }
}

/// The L1 Mac tab's answer (`#260`). Read fresh on every request; every
/// reading that fails is `null` or empty with a line in `notes`, never a
/// failed request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PowerModeReport {
    /// `false` off macOS: nothing here applies, and nothing below is read.
    pub applicable: bool,
    /// The modes this host offers, in `PowerMode::ALL` order -- empty when
    /// `pmset -g cap` lists neither `lowpowermode` nor `highpowermode`.
    pub supported: Vec<PowerMode>,
    /// The mode set for AC power, `null` when unreadable or not reported.
    pub ac: Option<PowerMode>,
    /// The mode set for battery power, `null` on a Mac with no battery.
    pub battery: Option<PowerMode>,
    /// The modes the sudoers rule lets this daemon set right now, found
    /// with a command-specific verbose sudo listing requiring explicit
    /// NOPASSWD in its matching rule; never executes the command.
    pub permitted: Vec<PowerMode>,
    /// Every supported mode is permitted: the control is live.
    pub can_change: bool,
    /// The one step that stays a person's: the rule to install.
    pub sudoers: SudoersRule,
    /// Why a reading is missing, in words.
    #[serde(default)]
    pub notes: Vec<String>,
    /// The newest changes, oldest first.
    #[serde(default)]
    pub changes: Vec<PowerModeChange>,
}

/// The sudoers drop-in that lets the daemon run exactly the three
/// `pmset -a powermode` commands as root, and nothing else.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SudoersRule {
    /// Where it goes: `/etc/sudoers.d/factory-pmset`.
    pub path: String,
    /// The user the daemon runs as, which the rule names.
    pub user: String,
    /// The rule's one line.
    pub rule: String,
    /// A shell command that checks the rule with `visudo -cf` and only then
    /// installs it, root-owned and `0440`.
    pub install: String,
    /// The whole-configuration check to run afterwards.
    pub check: String,
    /// The host's administrator accounts -- members of the `admin` group,
    /// without `root` and `_`-prefixed system accounts -- read without
    /// root (`dscl`). Installing the rule needs one of them: the daemon's
    /// own user usually cannot `sudo` at all. Empty when none was found.
    #[serde(default)]
    pub admins: Vec<String>,
    /// Whether `user` is itself one of `admins`, so it can install the rule
    /// from its own account.
    #[serde(default)]
    pub user_is_admin: bool,
}

/// One step of installing the rule: what to do, and the command to paste
/// for it, if any -- each in a block of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallStep {
    pub text: String,
    pub command: Option<String>,
}

impl SudoersRule {
    /// The administrator to switch to first, when the daemon's own user is
    /// not one and one is known.
    pub fn switch_to(&self) -> Option<&str> {
        if self.user_is_admin {
            None
        } else {
            self.admins.first().map(String::as_str)
        }
    }

    /// The install as steps a person runs one at a time -- the same wording
    /// the L1 Mac tab's `installSteps` uses. `su - <admin>` is a step of its
    /// own: pasted together with the install, `su`'s password prompt
    /// swallows the typed-ahead lines and opens a shell that ran nothing.
    pub fn install_steps(&self) -> Vec<InstallStep> {
        let step = |text: String, command: Option<String>| InstallStep { text, command };
        match self.switch_to() {
            Some(admin) => vec![
                step(format!("Run this alone and enter {admin}'s password:"), Some(format!("su - {admin}"))),
                step("Then, in that shell, paste:".into(), Some(self.install.clone())),
                step("Then `exit`, and Refresh.".into(), None),
            ],
            None if self.user_is_admin => vec![step(
                "This checks the rule with `visudo -cf` before installing it, root-owned and read-only:".into(),
                Some(self.install.clone()),
            )],
            None => vec![step(
                "This checks the rule with `visudo -cf` before installing it, root-owned and read-only. \
                 Run it from an administrator account:"
                    .into(),
                Some(self.install.clone()),
            )],
        }
    }
}

/// One journaled power-mode change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PowerModeChange {
    pub at: chrono::DateTime<chrono::Utc>,
    pub by: String,
    pub from_ac: Option<PowerMode>,
    pub from_battery: Option<PowerMode>,
    pub to: PowerMode,
    pub message: String,
}

/// The daemon answering, and where it keeps its state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonFacts {
    pub version: String,
    pub pid: u32,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub root: String,
    pub store: StoreFacts,
    /// The control socket, relative to `root` when it sits under it -- a
    /// deep root puts it in the temporary directory instead, and then this
    /// is that absolute path.
    pub socket: String,
    pub interfaces: Vec<InterfaceFacts>,
    /// The instance's default runtime adapter, `daemon.default_runtime`.
    pub runtime: String,
    /// The herdr session the daemon's own runtime calls go to, from its
    /// `HERDR_SESSION`. `null` when the runtime is not herdr or none is set.
    pub herdr_session: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoreFacts {
    /// The instance's task-store adapter, `daemon.task_store`.
    pub kind: String,
    /// The daemon's database, relative to `root`.
    pub path: String,
    /// `null` when the file could not be read.
    pub size_bytes: Option<u64>,
}

pub use factory_infrastructure::interfaces::InterfaceFacts;

/// One declared AI account and every agent it serves. Never a value: `env`
/// is the name of the variable an api-key provider's key lives in, as the
/// config wrote it, and nothing ever reads the variable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderRow {
    pub name: String,
    pub vendor: String,
    pub kind: factory_composition::config::ProviderKind,
    pub plan: Option<String>,
    pub env: Option<String>,
    pub agents: Vec<ProviderAgent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub windows: Vec<ProviderWindow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_runs: Vec<ProviderRun>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_unknown: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderWindow {
    pub window_minutes: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_percent: Option<f64>,
    pub resets_at: String,
    /// When the runtime sampled the reading (`SessionUsage::sampled_at`).
    pub sampled_at: chrono::DateTime<chrono::Utc>,
    /// The runtime did not say when it sampled, so `sampled_at` is when
    /// Factory asked -- and the reading is never called fresh.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sample_time_estimated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution_quality: Option<String>,
    /// How the change that ended at this reading was charged: to one run
    /// (`direct`) or split across several (`apportioned`). Fixed by the
    /// interval the reading closed, not by who is running now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<factory_process::usage::PlanShareAttribution>,
    /// Why that change could not be attributed, when it could not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution_unknown: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trend_percent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderRun {
    pub run_id: String,
    pub task_id: String,
    pub scope: String,
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderAgent {
    pub scope: String,
    pub agent: String,
    pub harness: String,
    /// `agent` when the agent's own `provider:` chose this account,
    /// `harness` when its harness's default did.
    pub via: factory_composition::config::ProviderVia,
}

/// A model agent no provider claims. Not an error: it tells a person what
/// to declare next. `shell` agents are never listed -- they make no model
/// call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnassignedAgent {
    pub scope: String,
    pub agent: String,
    pub harness: String,
}

/// How big a scope is on disk, for the site plan's hall footprint. The
/// prototype this is ported from sized a hall as `2.6 + sqrt(k) * 0.85`
/// where `k` is the codebase's megabytes; this is where `k` comes from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeFootprint {
    pub name: String,
    /// `None` when the scope's directory could not be read -- gone, or a
    /// permission the daemon does not have. A hall with no footprint is drawn
    /// at a default size and says so, rather than claiming a number nobody
    /// measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// The scope's top-level entries -- a directory, or every loose file in
    /// the root as one entry -- largest first, for the site plan to treemap
    /// onto the hall's floor. Empty both when `size_bytes` is `None` (the
    /// scope could not be walked, so there is nothing to show) and when it is
    /// genuinely `Some(0)` (walked, and there was nothing there): the two
    /// stay tellable apart by `size_bytes` alone, the way they already were
    /// before this field existed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub areas: Vec<ScopeArea>,
    /// Whether the walk that produced `size_bytes` and `areas` stopped at
    /// `ENTRY_CAP` before it finished the tree. A truncated walk's numbers
    /// are a lower bound, not a measurement -- enough to size a hall next to
    /// its neighbours, not enough for the floor's proportions to be trusted,
    /// so the hall has to say "partial" rather than draw them as exact.
    pub truncated: bool,
    /// The same walk's counts, gathered into the shape the size mapping
    /// takes: files, source files, bytes and the directories holding them.
    /// `bytes` and `truncated` repeat what is above rather than being read
    /// from it, so that `building::size_score` has one whole input and no
    /// caller can hand it half of one.
    #[serde(default)]
    pub metrics: RepoMetrics,
    /// What that size makes the building: its tier, its floors, its footprint
    /// in tenths of a grid unit, and the window bays on a face. Structure
    /// only -- nothing here moves when a run starts.
    pub shape: Shape,
    /// What Factory is doing in the scope right now.
    pub activity: Activity,
    /// What that activity lights: how far up the building, what the roof
    /// beacon says, how fast it beats. Emissive only -- no dimensions.
    pub cues: Cues,
}

/// One top-level entry of a scope's root, as the floor treemap draws it. A
/// directory keeps its own name; every file lying loose in the root -- a
/// `Cargo.toml`, a `README.md` -- is one entry rather than one per file, and
/// is named for what it is rather than invented as a directory that does not
/// exist.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeArea {
    pub name: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteFootprint {
    pub scopes: Vec<ScopeFootprint>,
}

pub use factory_kernel::{ProductionBin, ProductionBucket, ProductionFact as Production};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_wire_defaults_to_scope_and_remains_read_only() {
        let env: Envelope = serde_json::from_str(r#"{"op":"budget","params":{}}"#).unwrap();
        assert!(matches!(
            env.request,
            Request::Budget {
                scope: None,
                group_by: factory_process::usage::CostGroupBy::Scope
            }
        ));
        let env: Envelope = serde_json::from_str(
            r#"{"op":"budget","params":{"scope":"work","group_by":"workflow"}}"#,
        )
        .unwrap();
        assert!(matches!(
            env.request,
            Request::Budget {
                scope: Some(_),
                group_by: factory_process::usage::CostGroupBy::Workflow
            }
        ));
    }

    // `flatten` over an adjacently tagged enum routes through a content buffer,
    // which is a rough edge in serde. Prove it round-trips before anything is
    // built on top of it.
    #[test]
    fn an_envelope_carries_a_request_and_a_token() {
        let json = r#"{"op":"task.list","params":{},"token":"abc"}"#;
        let env: Envelope = serde_json::from_str(json).expect("envelope parses");
        assert_eq!(env.token.as_deref(), Some("abc"));
        assert!(matches!(env.request, Request::TaskList(_)));

        let back = serde_json::to_string(&env).unwrap();
        let again: Envelope = serde_json::from_str(&back).unwrap();
        assert_eq!(again.token.as_deref(), Some("abc"));
        assert!(matches!(again.request, Request::TaskList(_)));
    }

    #[test]
    fn a_request_without_a_token_still_parses() {
        let env: Envelope = serde_json::from_str(r#"{"op":"status"}"#).expect("no token is fine");
        assert!(env.token.is_none());
        assert!(matches!(env.request, Request::Status));
    }

    #[test]
    fn a_runtime_connection_request_is_a_read_without_parameters() {
        let env: Envelope =
            serde_json::from_str(r#"{"op":"runtime.connections"}"#).expect("request parses");
        assert!(matches!(env.request, Request::RuntimeConnections));
    }

    #[test]
    fn an_environment_request_is_a_read_without_parameters() {
        let env: Envelope =
            serde_json::from_str(r#"{"op":"environment"}"#).expect("request parses");
        assert!(matches!(env.request, Request::Environment));
    }

    #[test]
    fn a_backup_restore_request_carries_the_snapshot_and_new_root() {
        let json = r#"{"op":"backup.restore","params":{"snapshot":"factory-backup-demo-20260925T030000Z.tar.zst","into":"/tmp/restored"}}"#;
        let env: Envelope = serde_json::from_str(json).expect("request parses");
        assert!(matches!(
            &env.request,
            Request::BackupRestore { snapshot, into, identity: None }
                if snapshot.ends_with(".tar.zst") && into == &PathBuf::from("/tmp/restored")
        ));
        let back = serde_json::to_string(&env).unwrap();
        let again: Envelope = serde_json::from_str(&back).unwrap();
        assert!(matches!(again.request, Request::BackupRestore { .. }));
    }

    /// `#152`: an identity is opt in on the wire, and old clients that never
    /// send one still parse.
    #[test]
    fn backup_verify_and_restore_carry_an_optional_identity_path() {
        let json = r#"{"op":"backup.verify","params":{"snapshot":"s.tar.zst.age","identity":"/home/me/key.txt"}}"#;
        let env: Envelope = serde_json::from_str(json).expect("request parses");
        assert!(matches!(
            &env.request,
            Request::BackupVerify { snapshot: Some(s), identity: Some(path) }
                if s == "s.tar.zst.age" && path == &PathBuf::from("/home/me/key.txt")
        ));

        let no_identity: Envelope =
            serde_json::from_str(r#"{"op":"backup.verify","params":{}}"#).unwrap();
        assert!(matches!(
            no_identity.request,
            Request::BackupVerify {
                snapshot: None,
                identity: None
            }
        ));

        let restore_json = r#"{"op":"backup.restore","params":{"snapshot":"s.tar.zst.age","into":"/tmp/r","identity":"/home/me/key.txt"}}"#;
        let restore: Envelope = serde_json::from_str(restore_json).unwrap();
        assert!(matches!(
            &restore.request,
            Request::BackupRestore { identity: Some(path), .. } if path == &PathBuf::from("/home/me/key.txt")
        ));
    }

    #[test]
    fn a_knowledge_request_is_a_read_without_parameters() {
        let env: Envelope = serde_json::from_str(r#"{"op":"knowledge"}"#).expect("request parses");
        assert!(matches!(env.request, Request::Knowledge));
    }

    #[test]
    fn a_benchmarks_request_is_a_read_without_parameters() {
        let env: Envelope = serde_json::from_str(r#"{"op":"benchmarks"}"#).expect("request parses");
        assert!(matches!(env.request, Request::Benchmarks));
    }

    #[test]
    fn a_datasets_request_is_a_read_without_parameters() {
        let env: Envelope = serde_json::from_str(r#"{"op":"datasets"}"#).expect("request parses");
        assert!(matches!(env.request, Request::Datasets));
    }

    #[test]
    fn a_dashboard_request_defaults_scope_to_none_and_reads_it_when_given() {
        let env: Envelope =
            serde_json::from_str(r#"{"op":"dashboard","params":{}}"#).expect("no scope is fine");
        assert!(matches!(env.request, Request::Dashboard { scope: None }));

        let env: Envelope = serde_json::from_str(r#"{"op":"dashboard","params":{"scope":"demo"}}"#)
            .expect("request parses");
        assert!(matches!(env.request, Request::Dashboard { scope: Some(s) } if s == "demo"));
    }

    #[test]
    fn dashboard_set_and_reset_require_a_scope_named_exactly() {
        let env: Envelope = serde_json::from_str(
            r#"{"op":"dashboard.set","params":{"scope":"demo","tiles":[{"view":"kpis","size":"s"}]}}"#,
        )
        .expect("request parses");
        match &env.request {
            Request::DashboardSet { scope, tiles } => {
                assert_eq!(scope, "demo");
                assert_eq!(tiles.len(), 1);
                assert_eq!(
                    tiles[0].view,
                    Some(factory_composition::dashboard::ViewId::Kpis)
                );
            }
            other => panic!("wrong request: {other:?}"),
        }
        let back = serde_json::to_string(&env).unwrap();
        let again: Envelope = serde_json::from_str(&back).unwrap();
        assert!(matches!(again.request, Request::DashboardSet { .. }));

        // No default: unlike the read, a write must name which scope's own
        // block this is.
        let missing_scope = serde_json::from_str::<Envelope>(
            r#"{"op":"dashboard.set","params":{"tiles":[{"view":"kpis","size":"s"}]}}"#,
        );
        assert!(missing_scope.is_err());

        let env: Envelope =
            serde_json::from_str(r#"{"op":"dashboard.reset","params":{"scope":"demo"}}"#)
                .expect("request parses");
        assert!(matches!(env.request, Request::DashboardReset { scope } if scope == "demo"));
        let missing_scope =
            serde_json::from_str::<Envelope>(r#"{"op":"dashboard.reset","params":{}}"#);
        assert!(missing_scope.is_err());
    }

    #[test]
    fn a_dashboard_payload_carries_null_tiles_and_the_source_on_the_wire() {
        let payload = Payload::Dashboard {
            tiles: None,
            source: None,
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["kind"], "dashboard");
        assert_eq!(json["tiles"], serde_json::Value::Null);
        assert_eq!(
            json["source"],
            serde_json::Value::Null,
            "never a magic string like \"default\""
        );

        let tile = factory_composition::dashboard::Tile {
            metric: Some(factory_assurance::metrics::MetricId::new("throughput_week").unwrap()),
            view: None,
            size: factory_composition::dashboard::TileSize::S,
        };
        let payload = Payload::Dashboard {
            tiles: Some(vec![tile]),
            source: Some("demo".into()),
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["source"], "demo");
        assert_eq!(json["tiles"][0]["metric"], "throughput_week");
        assert_eq!(json["tiles"][0]["size"], "s");
        assert!(
            json["tiles"][0].get("view").is_none(),
            "view is omitted, not null, when absent"
        );
    }

    #[test]
    fn a_dataset_request_names_the_dataset() {
        let env: Envelope =
            serde_json::from_str(r#"{"op":"dataset","params":{"name":"registration"}}"#)
                .expect("request parses");
        assert!(matches!(env.request, Request::Dataset { name } if name == "registration"));
    }

    #[test]
    fn a_dataset_create_request_round_trips() {
        let json = r#"{"op":"dataset.create","params":{"name":"registration","description":"d"}}"#;
        let env: Envelope = serde_json::from_str(json).expect("request parses");
        match &env.request {
            Request::DatasetCreate { name, description } => {
                assert_eq!(name, "registration");
                assert_eq!(description.as_deref(), Some("d"));
            }
            other => panic!("wrong request: {other:?}"),
        }
        let back = serde_json::to_string(&env).unwrap();
        let again: Envelope = serde_json::from_str(&back).unwrap();
        assert!(matches!(again.request, Request::DatasetCreate { .. }));
    }

    #[test]
    fn a_bench_run_start_request_keeps_only_the_agent_names() {
        let json = r#"{"op":"bench.run","params":{"dataset":"registration","agents":["builder"],"attempts":2,"concurrency":2}}"#;
        let env: Envelope = serde_json::from_str(json).expect("request parses");
        match env.request {
            Request::BenchRunStart {
                dataset,
                agents,
                attempts,
                concurrency,
                cases,
            } => {
                assert_eq!(dataset, "registration");
                assert_eq!(agents, vec!["builder".to_string()]);
                assert_eq!(attempts, Some(2));
                assert_eq!(concurrency, Some(2));
                assert_eq!(cases, None);
            }
            other => panic!("wrong request: {other:?}"),
        }
    }

    #[test]
    fn a_bench_runs_request_narrows_by_an_optional_dataset() {
        let env: Envelope =
            serde_json::from_str(r#"{"op":"bench.runs","params":{}}"#).expect("no dataset is fine");
        assert!(matches!(env.request, Request::BenchRuns { dataset: None }));
        let env: Envelope =
            serde_json::from_str(r#"{"op":"bench.runs","params":{"dataset":"registration"}}"#)
                .expect("request parses");
        assert!(
            matches!(env.request, Request::BenchRuns { dataset: Some(d) } if d == "registration")
        );
    }

    #[test]
    fn a_datasets_payload_carries_the_wire_fields_the_issue_names() {
        let response = Response::ok(Payload::Datasets {
            root: "/inst/.factory/datasets".into(),
            datasets: vec![factory_assurance::dataset::DatasetSummary {
                name: "registration".into(),
                description: Some("Registering a scope".into()),
                revision: 3,
                cases: 2,
                gated: 1,
                findings: vec![],
            }],
        });
        let json = serde_json::to_value(response).unwrap();
        assert_eq!(
            json.pointer("/data/kind")
                .and_then(serde_json::Value::as_str),
            Some("datasets")
        );
        assert_eq!(
            json.pointer("/data/datasets/0/name")
                .and_then(serde_json::Value::as_str),
            Some("registration")
        );
        assert_eq!(
            json.pointer("/data/datasets/0/revision")
                .and_then(serde_json::Value::as_u64),
            Some(3)
        );
    }

    #[test]
    fn a_bench_run_payload_carries_the_result_aggregation() {
        use factory_assurance::bench::{
            BenchAttempt, BenchResult, BenchRun, BenchRunStatus, Verdict,
        };
        let mut attempt =
            BenchAttempt::pending("a1".into(), "add-scope".into(), "builder".into(), 1);
        attempt.verdict = Some(Verdict::Pass);
        let run = BenchRun {
            id: "r1".into(),
            dataset: "registration".into(),
            dataset_revision: 3,
            cases: vec![],
            case_bases: Default::default(),
            agents: vec!["builder".into()],
            attempts_per_case: 1,
            concurrency: 1,
            status: BenchRunStatus::Done,
            attempts: vec![attempt],
            started_at: chrono::Utc::now(),
            ended_at: Some(chrono::Utc::now()),
        };
        let results: Vec<BenchResult> = factory_assurance::bench::aggregate(&run.attempts);
        let response = Response::ok(Payload::BenchRun { run, results });
        let json = serde_json::to_value(response).unwrap();
        assert_eq!(
            json.pointer("/data/kind")
                .and_then(serde_json::Value::as_str),
            Some("bench_run")
        );
        assert_eq!(
            json.pointer("/data/run/status")
                .and_then(serde_json::Value::as_str),
            Some("done")
        );
        assert!(json.pointer("/data/results/0/resolve_rate").is_some());
    }

    #[test]
    fn a_knowledge_payload_carries_the_wire_fields_the_issue_names() {
        let response = Response::ok(Payload::Knowledge {
            root: "/inst/.factory/knowledge".into(),
            present: true,
            legacy: None,
            pages: vec![factory_assurance::knowledge::Page {
                id: "partners/acme".into(),
                title: "Acme GmbH".into(),
                frontmatter: true,
                area: Some("partners".into()),
                status: Some("current".into()),
                updated: Some("2026-09-01".into()),
                sources: 2,
                links: vec!["partners/jane-doe".into()],
                gaps: vec!["example.club".into()],
                backlinks: vec!["company/pricing".into()],
                tags: vec!["pricing".into()],
                documents: vec!["documents/acme-contract.pdf".into()],
            }],
            tags: vec![factory_assurance::knowledge::Tag {
                name: "pricing".into(),
                pages: vec!["partners/acme".into()],
            }],
            documents: vec![factory_assurance::knowledge::Document {
                id: "documents/acme-contract.pdf".into(),
                ext: "pdf".into(),
                bytes: 48213,
                referenced_by: vec!["partners/acme".into()],
            }],
            gaps: vec![factory_assurance::knowledge::Gap {
                target: "example.club".into(),
                from: vec!["partners/acme".into(), "partners/jane-doe".into()],
            }],
            findings: vec![factory_assurance::knowledge::Finding {
                kind: factory_assurance::knowledge::FindingKind::Unsourced,
                note: "partners/jane-doe".into(),
                detail: "no sources in frontmatter".into(),
            }],
        });
        let json = serde_json::to_value(response).unwrap();
        assert_eq!(
            json.pointer("/data/kind")
                .and_then(serde_json::Value::as_str),
            Some("knowledge")
        );
        assert_eq!(
            json.pointer("/data/pages/0/id")
                .and_then(serde_json::Value::as_str),
            Some("partners/acme")
        );
        assert_eq!(
            json.pointer("/data/tags/0/name")
                .and_then(serde_json::Value::as_str),
            Some("pricing")
        );
        assert_eq!(
            json.pointer("/data/documents/0/id")
                .and_then(serde_json::Value::as_str),
            Some("documents/acme-contract.pdf")
        );
        assert_eq!(
            json.pointer("/data/gaps/0/target")
                .and_then(serde_json::Value::as_str),
            Some("example.club")
        );
        assert_eq!(
            json.pointer("/data/findings/0/kind")
                .and_then(serde_json::Value::as_str),
            Some("unsourced")
        );
    }

    #[test]
    fn a_knowledge_import_request_parses_with_its_optional_fields_defaulted() {
        let env: Envelope = serde_json::from_str(
            r#"{"op":"knowledge.import","params":{"source":"/tmp/import-me"}}"#,
        )
        .expect("request parses");
        assert!(matches!(
            env.request,
            Request::KnowledgeImport {
                into: None,
                overwrite: false,
                ..
            }
        ));
    }

    #[test]
    fn a_knowledge_write_result_carries_the_wire_shape_the_issue_names() {
        let response = Response::ok(Payload::KnowledgeWrite {
            copied: vec!["documents/x.pdf".into()],
            skipped_existing: vec![],
            skipped_hidden: vec![],
            refused: vec![factory_assurance::knowledge::Refusal {
                path: None,
                reason: "a source under data/secrets/ is never read".into(),
            }],
            truncated: false,
        });
        let json = serde_json::to_value(response).unwrap();
        assert_eq!(
            json.pointer("/data/copied/0")
                .and_then(serde_json::Value::as_str),
            Some("documents/x.pdf")
        );
        assert!(
            json.pointer("/data/refused/0/path").is_none(),
            "a secret-source refusal must not carry even a null path field"
        );
    }

    #[test]
    fn a_benchmarks_payload_carries_the_wire_fields_the_issue_names() {
        let response = Response::ok(Payload::Benchmarks {
            configurations: vec![Configuration {
                harness: "claude-code".into(),
                model: Some("opus".into()),
                model_source: Some("args".into()),
                flags: vec!["--permission-mode …".into(), "--yolo".into()],
                sandbox: "none".into(),
                agents: vec![factory_assurance::benchmark::ConfiguredAgent {
                    scope: "projects/demo".into(),
                    agent: "builder".into(),
                    lifetime: "temporary".into(),
                    declared: true,
                }],
                missing: vec![
                    "harness version".into(),
                    "tool surface".into(),
                    "context policy".into(),
                    "retry budget".into(),
                ],
                pinned: false,
            }],
        });
        let json = serde_json::to_value(response).unwrap();
        assert_eq!(
            json.pointer("/data/kind")
                .and_then(serde_json::Value::as_str),
            Some("benchmarks")
        );
        assert_eq!(
            json.pointer("/data/configurations/0/harness")
                .and_then(serde_json::Value::as_str),
            Some("claude-code")
        );
        assert_eq!(
            json.pointer("/data/configurations/0/agents/0/scope")
                .and_then(serde_json::Value::as_str),
            Some("projects/demo")
        );
        assert_eq!(
            json.pointer("/data/configurations/0/pinned")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn a_runtime_diagnostic_flattens_into_one_wire_card() {
        let response = Response::ok(Payload::RuntimeConnections {
            runtimes: vec![RuntimeConnectionView {
                runtime: "bare".into(),
                source: "builtin".into(),
                description: "bare runtime".into(),
                scopes: vec!["demo".into()],
                checked_at: chrono::Utc::now(),
                diagnostic: RuntimeConnectionDiagnostic::unsupported(),
            }],
        });
        let json = serde_json::to_value(response).unwrap();
        assert_eq!(
            json.pointer("/data/kind")
                .and_then(serde_json::Value::as_str),
            Some("runtime_connections")
        );
        assert_eq!(
            json.pointer("/data/runtimes/0/state")
                .and_then(serde_json::Value::as_str),
            Some("unsupported")
        );
        assert_eq!(
            json.pointer("/data/runtimes/0/scopes/0")
                .and_then(serde_json::Value::as_str),
            Some("demo")
        );
    }

    #[test]
    fn a_request_with_params_survives_the_flatten() {
        let json = r#"{"op":"task.get","params":{"id":"t1"},"token":"t"}"#;
        let env: Envelope = serde_json::from_str(json).unwrap();
        match env.request {
            Request::TaskGet { id } => assert_eq!(id, "t1"),
            other => panic!("wrong request: {other:?}"),
        }
    }

    #[test]
    fn an_agent_configuration_request_round_trips_every_declaration_field() {
        let json = r#"{"op":"agent.configure","params":{"scope":"demo","agent":{"name":"reviewer","harness":"pi","lifetime":"permanent","role":"foreman","autostart":false,"args":["--model","local model"],"sandbox":"docker"}}}"#;
        let env: Envelope = serde_json::from_str(json).unwrap();
        match &env.request {
            Request::AgentConfigure { scope, agent } => {
                assert_eq!(scope, "demo");
                assert_eq!(agent.name(), "reviewer");
                assert_eq!(agent.harness, "pi");
                assert_eq!(agent.args, vec!["--model", "local model"]);
                assert!(!agent.autostart());
                assert_eq!(agent.sandbox, factory_composition::config::Sandbox::Docker);
            }
            other => panic!("wrong request: {other:?}"),
        }
        let back = serde_json::to_string(&env).unwrap();
        let again: Envelope = serde_json::from_str(&back).unwrap();
        assert!(matches!(again.request, Request::AgentConfigure { .. }));
    }

    #[test]
    fn an_agent_deletion_request_names_its_scope_and_declaration() {
        let json = r#"{"op":"agent.delete","params":{"scope":"demo","name":"reviewer"}}"#;
        let env: Envelope = serde_json::from_str(json).unwrap();
        assert!(matches!(
            env.request,
            Request::AgentDelete { scope, name }
                if scope == "demo" && name == "reviewer"
        ));
    }

    /// The two lines `factory task turn-ended` actually wrote to a socket
    /// when a real `claude` 2.1.280 fired Factory's generated hooks -- one
    /// normal turn, one ended on an API error (an unknown `--model`) -- so
    /// the daemon's side is held to the wire the CLI speaks, not to a shape
    /// either side only assumed.
    #[test]
    fn a_turn_end_as_the_real_hook_sent_it_parses() {
        use factory_process::task::TurnEndEvent;
        let stop = r#"{"op":"task.turn_ended","params":{"id":"task-e2e","turn":{"event":"stop","pending_background":0,"last_message":"pong","token":"secret-run-token"}},"token":"secret-run-token"}"#;
        let failure = r#"{"op":"task.turn_ended","params":{"id":"task-e2e","turn":{"event":"stop_failure","pending_background":0,"error":"model_not_found","last_message":"There's an issue with the selected model (claude-no-such-model-9). It may not exist or you may not have access to it. Run --model to pick a different model.","token":"secret-run-token"}},"token":"secret-run-token"}"#;

        let envelope: Envelope = serde_json::from_str(stop).unwrap();
        assert_eq!(envelope.token.as_deref(), Some("secret-run-token"));
        let Request::TaskTurnEnded { id, turn } = envelope.request else {
            panic!("not a turn end");
        };
        assert_eq!(id, "task-e2e");
        assert_eq!(turn.event, TurnEndEvent::Stop);
        assert_eq!(turn.last_message.as_deref(), Some("pong"));
        assert_eq!(turn.token.as_deref(), Some("secret-run-token"));

        let Request::TaskTurnEnded { turn, .. } =
            serde_json::from_str::<Envelope>(failure).unwrap().request
        else {
            panic!("not a turn end");
        };
        assert_eq!(turn.event, TurnEndEvent::StopFailure);
        assert_eq!(turn.error.as_deref(), Some("model_not_found"));
        assert!(
            turn.error_details.is_none(),
            "not every StopFailure carries details"
        );
    }

    /// The issue's own example, key for key: this shape is the contract the
    /// L1 page is built against, so the test is the example itself rather
    /// than a paraphrase of it. The only addition is `kind`, which every
    /// payload carries.
    const INFRASTRUCTURE_EXAMPLE: &str = r#"{
      "kind": "infrastructure",
      "host": {
        "hostname": "factory-mac",
        "model": "Mac17,7",
        "chip": "Apple M5 Max",
        "cores": 18,
        "memory_bytes": 68719476736,
        "os": "macOS 26.6.1",
        "arch": "aarch64",
        "uptime_seconds": 864000,
        "load": [2.1, 1.8, 1.6],
        "disk": { "mount": "/", "total_bytes": 1995218165760, "free_bytes": 1539316654080 }
      },
      "daemon": {
        "version": "0.1.0",
        "pid": 4242,
        "started_at": "2026-09-24T08:00:00Z",
        "root": "/Users/factory/business-factory",
        "store": { "kind": "sqlite", "path": ".factory/factory.sqlite", "size_bytes": 12582912 },
        "socket": ".factory/factory.sock",
        "interfaces": [ { "kind": "cli" }, { "kind": "http", "bind": "192.168.188.92:8791" } ],
        "runtime": "herdr",
        "herdr_session": "factory"
      },
      "providers": [
        {
          "name": "claude-max",
          "vendor": "anthropic",
          "kind": "subscription",
          "plan": "Max 20x",
          "env": null,
          "agents": [ { "scope": "factory", "agent": "claude-code", "harness": "claude-code", "via": "harness" } ]
        }
      ],
      "unassigned": [ { "scope": "model-lab", "agent": "model-lab", "harness": "opencode" } ]
    }"#;

    fn infrastructure_example() -> Payload {
        use factory_composition::config::{ProviderKind, ProviderVia};
        Payload::Infrastructure {
            host: HostFacts {
                hostname: Some("factory-mac".into()),
                model: Some("Mac17,7".into()),
                chip: Some("Apple M5 Max".into()),
                cores: Some(18),
                memory_bytes: Some(68_719_476_736),
                os: Some("macOS 26.6.1".into()),
                arch: Some("aarch64".into()),
                uptime_seconds: Some(864_000),
                load: Some([2.1, 1.8, 1.6]),
                disk: Some(DiskFacts {
                    mount: "/".into(),
                    total_bytes: 1_995_218_165_760,
                    free_bytes: 1_539_316_654_080,
                }),
            },
            daemon: DaemonFacts {
                version: "0.1.0".into(),
                pid: 4242,
                started_at: "2026-09-24T08:00:00Z".parse().unwrap(),
                root: "/Users/factory/business-factory".into(),
                store: StoreFacts {
                    kind: "sqlite".into(),
                    path: ".factory/factory.sqlite".into(),
                    size_bytes: Some(12_582_912),
                },
                socket: ".factory/factory.sock".into(),
                interfaces: vec![
                    InterfaceFacts {
                        kind: "cli".into(),
                        bind: None,
                    },
                    InterfaceFacts {
                        kind: "http".into(),
                        bind: Some("192.168.188.92:8791".into()),
                    },
                ],
                runtime: "herdr".into(),
                herdr_session: Some("factory".into()),
            },
            providers: vec![ProviderRow {
                name: "claude-max".into(),
                vendor: "anthropic".into(),
                kind: ProviderKind::Subscription,
                plan: Some("Max 20x".into()),
                env: None,
                agents: vec![ProviderAgent {
                    scope: "factory".into(),
                    agent: "claude-code".into(),
                    harness: "claude-code".into(),
                    via: ProviderVia::Harness,
                }],
                windows: Vec::new(),
                active_runs: Vec::new(),
                usage_unknown: None,
            }],
            unassigned: vec![UnassignedAgent {
                scope: "model-lab".into(),
                agent: "model-lab".into(),
                harness: "opencode".into(),
            }],
            harnesses: vec![],
        }
    }

    #[test]
    fn the_infrastructure_payload_is_the_issues_wire_shape_and_round_trips() {
        let expected: serde_json::Value = serde_json::from_str(INFRASTRUCTURE_EXAMPLE).unwrap();
        let written = serde_json::to_value(infrastructure_example()).unwrap();
        assert_eq!(
            written, expected,
            "serializes to exactly the documented shape"
        );

        let read: Payload = serde_json::from_value(expected.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(read).unwrap(),
            expected,
            "and reads back to it"
        );

        // Inside a response, where the page actually finds it.
        let response = serde_json::to_value(Response::ok(infrastructure_example())).unwrap();
        assert_eq!(
            response.pointer("/data/kind").and_then(|v| v.as_str()),
            Some("infrastructure")
        );
        assert_eq!(
            response
                .pointer("/data/providers/0/agents/0/via")
                .and_then(|v| v.as_str()),
            Some("harness")
        );
    }

    #[test]
    fn an_unreadable_host_fact_is_null_on_the_wire_never_missing() {
        let Payload::Infrastructure { daemon, .. } = infrastructure_example() else {
            unreachable!()
        };
        let payload = Payload::Infrastructure {
            host: HostFacts::default(),
            daemon: DaemonFacts {
                store: StoreFacts {
                    size_bytes: None,
                    ..daemon.store.clone()
                },
                herdr_session: None,
                ..daemon
            },
            providers: vec![ProviderRow {
                name: "openrouter".into(),
                vendor: "openrouter".into(),
                kind: factory_composition::config::ProviderKind::ApiKey,
                plan: None,
                env: Some("OPENROUTER_API_KEY".into()),
                agents: vec![],
                windows: Vec::new(),
                active_runs: Vec::new(),
                usage_unknown: None,
            }],
            unassigned: vec![],
            harnesses: vec![],
        };
        let json = serde_json::to_value(&payload).unwrap();
        for field in [
            "hostname",
            "model",
            "chip",
            "cores",
            "memory_bytes",
            "os",
            "arch",
            "uptime_seconds",
            "load",
            "disk",
        ] {
            assert_eq!(
                json["host"].get(field),
                Some(&serde_json::Value::Null),
                "host.{field} is present and null, not left out"
            );
        }
        assert_eq!(
            json["daemon"]["store"].get("size_bytes"),
            Some(&serde_json::Value::Null)
        );
        assert_eq!(
            json["daemon"].get("herdr_session"),
            Some(&serde_json::Value::Null)
        );
        assert_eq!(
            json["providers"][0].get("plan"),
            Some(&serde_json::Value::Null)
        );
        assert_eq!(json["providers"][0]["kind"], "api-key");
        assert_eq!(json["providers"][0]["env"], "OPENROUTER_API_KEY");
        assert!(
            json["daemon"]["interfaces"][0].get("bind").is_none(),
            "a socket interface has no address, which is left out rather than null"
        );
        let back: Payload = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(back).unwrap(), json);
    }

    #[test]
    fn the_infrastructure_request_is_a_bare_op() {
        let wire = serde_json::to_value(Envelope {
            request: Request::Infrastructure,
            token: None,
        })
        .unwrap();
        assert_eq!(wire["op"], "infrastructure");
        let env: Envelope = serde_json::from_str(r#"{"op":"infrastructure"}"#).unwrap();
        assert!(matches!(env.request, Request::Infrastructure));

        let wire = serde_json::to_value(Envelope {
            request: Request::Doctor,
            token: None,
        })
        .unwrap();
        assert_eq!(wire["op"], "doctor");
        let env: Envelope = serde_json::from_str(r#"{"op":"doctor"}"#).unwrap();
        assert!(matches!(env.request, Request::Doctor));
    }
}
