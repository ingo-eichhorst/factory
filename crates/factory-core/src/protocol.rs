//! One request/response envelope, spoken by every interface adapter. The CLI
//! sends it over a unix socket, the HTTP adapter maps REST onto it; adding a
//! third interface means translating to this, not inventing a new API.

use crate::adapter::{RuntimeConnectionDiagnostic, Screen};
use crate::benchmark::Configuration;
use crate::building::{Activity, Cues, RepoMetrics, Shape};
use crate::agent::AgentSession;
use crate::config::ScopeAgent;
use crate::event::Event;
use crate::knowledge::{Document, Finding, Gap, Page, Refusal, Tag};
use crate::occupancy::Occupancy;
use crate::role::{RoleOrigin, RoleSpec};
use crate::run::Run;
use crate::task::{NewTask, Task, TaskEntry, TaskFilter, TaskPatch, TaskReport, TurnEnded};
use crate::workflow::{WorkflowDefinition, WorkflowDraft, WorkflowRun};
use serde::{Deserialize, Serialize};

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
    #[serde(rename = "task.create")]
    TaskCreate(NewTask),
    #[serde(rename = "task.get")]
    TaskGet { id: String },
    #[serde(rename = "task.list")]
    TaskList(TaskFilter),
    #[serde(rename = "task.update")]
    TaskUpdate { id: String, patch: TaskPatch },
    #[serde(rename = "task.delete")]
    TaskDelete { id: String },
    #[serde(rename = "task.run")]
    TaskRun { id: String },
    #[serde(rename = "task.cancel")]
    TaskCancel { id: String },
    #[serde(rename = "task.report")]
    TaskReport { id: String, report: TaskReport },
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
    WorkflowStart { id: String },
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
        /// How far back to look. Defaults to the last twelve hours.
        #[serde(default)]
        minutes: Option<u32>,
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
    /// The L5 Knowledge tab: an index of `<root>/.factory/knowledge/`,
    /// rebuilt from the files on every request. Read-only, like
    /// `Environment` -- see `knowledge::index`, which does the actual walk.
    #[serde(rename = "knowledge")]
    Knowledge,
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
        cases: Vec<crate::dataset::Case>,
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
    Ok,
    Status { status: StatusInfo },
    Adapters { adapters: Vec<AdapterEntry> },
    RuntimeConnections { runtimes: Vec<RuntimeConnectionView> },
    Task { task: Task },
    Tasks { tasks: Vec<Task> },
    Workflow { workflow: WorkflowDefinition },
    Workflows { workflows: Vec<WorkflowDefinition> },
    WorkflowRun { run: WorkflowRun },
    WorkflowRuns { runs: Vec<WorkflowRun> },
    Run { run: Run },
    Runs { runs: Vec<Run> },
    Agent { agent: AgentSession },
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
    Roles { board: RoleBoard },
    Entries { entries: Vec<TaskEntry> },
    Text { text: String },
    Deleted { deleted: bool },
    Event { event: Event },
    Occupancy { occupancy: Occupancy },
    Production { production: Production },
    Screen { screen: Screen },
    SiteFootprint { footprint: SiteFootprint },
    /// The L2 Environment page. There is deliberately no "everything here is
    /// reachable by every agent" sentence in this payload: it is a fact about
    /// how the daemon runs its agents, true whether or not this request
    /// succeeded, so the page states it from its own markup and goes on
    /// stating it when the fetch fails.
    Environment {
        sandboxes: Vec<SandboxRow>,
        credentials: Vec<CredentialRow>,
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
    Benchmarks { configurations: Vec<Configuration> },
    /// Every dataset, summarized -- see `dataset::DatasetSummary`.
    Datasets {
        root: String,
        datasets: Vec<crate::dataset::DatasetSummary>,
    },
    /// One dataset in full, with the findings against the live config a
    /// summary already carries too.
    Dataset {
        dataset: crate::dataset::Dataset,
        findings: Vec<crate::dataset::DatasetFinding>,
    },
    /// A bench run, with its attempts and the results aggregated from them
    /// -- see `bench::aggregate`.
    BenchRun {
        run: crate::bench::BenchRun,
        results: Vec<crate::bench::BenchResult>,
    },
    /// Every bench run, most recently updated first.
    BenchRuns { runs: Vec<crate::bench::BenchRun> },
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
        Payload::Adapters { adapters: list.adapters }
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
    /// `none`, `docker`, or `srt` -- what the declaration says, unread by
    /// anything else today. See `Sandbox`'s doc comment.
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
    /// `none`, `docker`, or `srt`.
    pub sandbox: String,
    pub worktree_capable: bool,
}

/// One place on disk a credential might already sit, checked for existence
/// only -- see `Payload::Environment`. The value itself is never read, held,
/// or returned; `present` is the whole of what this says.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialRow {
    pub label: String,
    pub path: String,
    pub integration: String,
    pub present: bool,
    /// The scope this row belongs to, for the rows that belong to one at all.
    /// `None` is the honest answer for a credential in the owner's home: it
    /// sits outside every scope and is reachable from all of them, so the
    /// page goes on showing it whichever scope the rail has selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
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

/// Hour, day or week. Decided by the caller and sent with every request
/// rather than derived from the window on the server, so a view always gets
/// the granularity it actually draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductionBin {
    Hour,
    Day,
    Week,
}

/// One period of finished runs. `scrapped` and `reworked` are both read
/// against `finished`, not tallied separately from it -- a run that fails on
/// its second attempt is one run, counted once, in both.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductionBucket {
    /// The bucket's real start. Equal to the bin's own calendar boundary
    /// (the top of the hour, midnight, Monday) except for the very first
    /// bucket of a query, which is clipped forward to the window's start.
    pub from: chrono::DateTime<chrono::Utc>,
    /// The bucket's real end. Equal to the next calendar boundary except for
    /// the last bucket, which is clipped back to the moment the query ran --
    /// a bucket still filling in is not the same fact as a slow one.
    pub to: chrono::DateTime<chrono::Utc>,
    pub finished: u32,
    pub scrapped: u32,
    pub reworked: u32,
    /// True when `to - from` falls short of the bin's nominal width. Decided
    /// once, here -- so a chart never has to guess whether a short bar is a
    /// quiet period or a bucket that has not finished collecting yet.
    pub partial: bool,
}

/// The run history the dashboard draws: a bucketed window for the throughput
/// chart and the KPI sparklines that read the same series, and the year of
/// daily totals the production grid always shows regardless of what window
/// is selected above it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Production {
    /// The bin `buckets` is drawn in. `daily` is always day-grain, whatever
    /// this says.
    pub bin: ProductionBin,
    pub from: chrono::DateTime<chrono::Utc>,
    pub to: chrono::DateTime<chrono::Utc>,
    pub buckets: Vec<ProductionBucket>,
    /// Fifty-three weeks of daily totals ending today, scoped the same as
    /// `buckets`. Independent of `bin`: the production-year grid does not
    /// rebin with the window above it.
    pub daily: Vec<ProductionBucket>,
    /// The earliest finished run this query found, scoped the same as
    /// everything else here. `None` when it found none at all. This is a
    /// lower bound on the instance's life, not its birthday -- the store
    /// does not record when the instance was set up -- so a day before it is
    /// drawn as "no record", never as "before this factory existed".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub earliest_run: Option<chrono::DateTime<chrono::Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn a_knowledge_request_is_a_read_without_parameters() {
        let env: Envelope =
            serde_json::from_str(r#"{"op":"knowledge"}"#).expect("request parses");
        assert!(matches!(env.request, Request::Knowledge));
    }

    #[test]
    fn a_benchmarks_request_is_a_read_without_parameters() {
        let env: Envelope =
            serde_json::from_str(r#"{"op":"benchmarks"}"#).expect("request parses");
        assert!(matches!(env.request, Request::Benchmarks));
    }

    #[test]
    fn a_datasets_request_is_a_read_without_parameters() {
        let env: Envelope = serde_json::from_str(r#"{"op":"datasets"}"#).expect("request parses");
        assert!(matches!(env.request, Request::Datasets));
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
            Request::BenchRunStart { dataset, agents, attempts, concurrency, cases } => {
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
        let env: Envelope = serde_json::from_str(r#"{"op":"bench.runs","params":{}}"#)
            .expect("no dataset is fine");
        assert!(matches!(env.request, Request::BenchRuns { dataset: None }));
        let env: Envelope =
            serde_json::from_str(r#"{"op":"bench.runs","params":{"dataset":"registration"}}"#)
                .expect("request parses");
        assert!(matches!(env.request, Request::BenchRuns { dataset: Some(d) } if d == "registration"));
    }

    #[test]
    fn a_datasets_payload_carries_the_wire_fields_the_issue_names() {
        let response = Response::ok(Payload::Datasets {
            root: "/inst/.factory/datasets".into(),
            datasets: vec![crate::dataset::DatasetSummary {
                name: "registration".into(),
                description: Some("Registering a scope".into()),
                revision: 3,
                cases: 2,
                gated: 1,
                findings: vec![],
            }],
        });
        let json = serde_json::to_value(response).unwrap();
        assert_eq!(json.pointer("/data/kind").and_then(serde_json::Value::as_str), Some("datasets"));
        assert_eq!(
            json.pointer("/data/datasets/0/name").and_then(serde_json::Value::as_str),
            Some("registration")
        );
        assert_eq!(json.pointer("/data/datasets/0/revision").and_then(serde_json::Value::as_u64), Some(3));
    }

    #[test]
    fn a_bench_run_payload_carries_the_result_aggregation() {
        use crate::bench::{BenchAttempt, BenchResult, BenchRun, BenchRunStatus, Verdict};
        let mut attempt = BenchAttempt::pending("a1".into(), "add-scope".into(), "builder".into(), 1);
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
        let results: Vec<BenchResult> = crate::bench::aggregate(&run.attempts);
        let response = Response::ok(Payload::BenchRun { run, results });
        let json = serde_json::to_value(response).unwrap();
        assert_eq!(json.pointer("/data/kind").and_then(serde_json::Value::as_str), Some("bench_run"));
        assert_eq!(
            json.pointer("/data/run/status").and_then(serde_json::Value::as_str),
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
            pages: vec![crate::knowledge::Page {
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
            tags: vec![crate::knowledge::Tag {
                name: "pricing".into(),
                pages: vec!["partners/acme".into()],
            }],
            documents: vec![crate::knowledge::Document {
                id: "documents/acme-contract.pdf".into(),
                ext: "pdf".into(),
                bytes: 48213,
                referenced_by: vec!["partners/acme".into()],
            }],
            gaps: vec![crate::knowledge::Gap {
                target: "example.club".into(),
                from: vec!["partners/acme".into(), "partners/jane-doe".into()],
            }],
            findings: vec![crate::knowledge::Finding {
                kind: crate::knowledge::FindingKind::Unsourced,
                note: "partners/jane-doe".into(),
                detail: "no sources in frontmatter".into(),
            }],
        });
        let json = serde_json::to_value(response).unwrap();
        assert_eq!(
            json.pointer("/data/kind").and_then(serde_json::Value::as_str),
            Some("knowledge")
        );
        assert_eq!(
            json.pointer("/data/pages/0/id").and_then(serde_json::Value::as_str),
            Some("partners/acme")
        );
        assert_eq!(
            json.pointer("/data/tags/0/name").and_then(serde_json::Value::as_str),
            Some("pricing")
        );
        assert_eq!(
            json.pointer("/data/documents/0/id").and_then(serde_json::Value::as_str),
            Some("documents/acme-contract.pdf")
        );
        assert_eq!(
            json.pointer("/data/gaps/0/target").and_then(serde_json::Value::as_str),
            Some("example.club")
        );
        assert_eq!(
            json.pointer("/data/findings/0/kind").and_then(serde_json::Value::as_str),
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
            Request::KnowledgeImport { into: None, overwrite: false, .. }
        ));
    }

    #[test]
    fn a_knowledge_write_result_carries_the_wire_shape_the_issue_names() {
        let response = Response::ok(Payload::KnowledgeWrite {
            copied: vec!["documents/x.pdf".into()],
            skipped_existing: vec![],
            skipped_hidden: vec![],
            refused: vec![crate::knowledge::Refusal {
                path: None,
                reason: "a source under data/secrets/ is never read".into(),
            }],
            truncated: false,
        });
        let json = serde_json::to_value(response).unwrap();
        assert_eq!(
            json.pointer("/data/copied/0").and_then(serde_json::Value::as_str),
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
                agents: vec![crate::benchmark::ConfiguredAgent {
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
            json.pointer("/data/kind").and_then(serde_json::Value::as_str),
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
            json.pointer("/data/configurations/0/pinned").and_then(serde_json::Value::as_bool),
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
            json.pointer("/data/kind").and_then(serde_json::Value::as_str),
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
                assert_eq!(agent.sandbox, crate::config::Sandbox::Docker);
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
        use crate::task::TurnEndEvent;
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

        let Request::TaskTurnEnded { turn, .. } = serde_json::from_str::<Envelope>(failure).unwrap().request else {
            panic!("not a turn end");
        };
        assert_eq!(turn.event, TurnEndEvent::StopFailure);
        assert_eq!(turn.error.as_deref(), Some("model_not_found"));
        assert!(turn.error_details.is_none(), "not every StopFailure carries details");
    }
}
