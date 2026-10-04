//! `factory` -- the command-line face of the daemon, and the thing a dispatched
//! agent calls to say how it is getting on.

mod client;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use factory_core::bench::{BenchResult, BenchRun, Verdict};
use factory_core::dataset::{Case, Dataset, DatasetFinding, DatasetSummary};
use factory_core::dependencies::{AttachmentKind, DependenciesReport};
use factory_core::event::Event;
use factory_core::goals::{self as goals_core, Band, CycleStatus, KrRef};
use factory_core::knowledge::FindingKind;
use factory_core::metrics::{MetricId, MetricsWindow};
use factory_core::operations::{self as ops, HealthWindow, OperationsReport};
use factory_core::policy::{self, ControlRef};
use factory_core::reporting_clock::{self, ClockDeadlineKind, ClockItemRef, ClockMark};
use factory_core::protocol::{
    GoalsReport, Payload, PolicyControlDetail, PolicyReport, Request, Response, ScenarioPromoteResult, ScenarioResult, ScenariosReport,
};
use factory_core::scenario;
use factory_core::usage::{CostRowExt, TokenSumsExt};
use factory_core::run::{Run, RunStatus};
use factory_core::task::{
    CloseReason, CronSchedule, NewTask, RetryPolicy, Schedule, Task, TaskFilter, TaskPatch, TaskReport,
    TaskStatus,
};
use std::path::{Path, PathBuf};

use client::Client;

const BUILD_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("FACTORY_GIT_SHA"),
    ")"
);

#[derive(Parser)]
#[command(name = "factory", about = "Talk to a Factory daemon", version = BUILD_VERSION)]
struct Cli {
    /// Instance root. Defaults to the nearest ancestor holding a .factory/.
    #[arg(long, global = true, env = "FACTORY_ROOT")]
    root: Option<PathBuf>,

    /// Control socket. Overrides everything else.
    #[arg(long, global = true)]
    socket: Option<PathBuf>,

    /// Reach the daemon over its http interface instead of the socket, as
    /// http://host:port. For an agent that cannot see the socket -- a run in
    /// an OpenShell sandbox, which Factory starts with FACTORY_URL set. The
    /// same request and token go to the same handler, so roles apply as over
    /// the socket.
    #[arg(long, global = true, env = "FACTORY_URL")]
    url: Option<String>,

    /// Print the daemon's answer as JSON.
    #[arg(long, global = true)]
    json: bool,

    /// Say which agent is calling. Factory sets FACTORY_TOKEN in every session
    /// it opens, so an agent rarely has to pass this.
    #[arg(long, global = true, env = "FACTORY_TOKEN")]
    token: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// What the daemon is doing.
    Status,
    /// Which adapters are registered, and where each came from.
    Adapters,
    /// The scopes, their agents, and what each is doing right now.
    Agents,
    /// L1 Infrastructure: the host, the daemon on it, and which declared AI
    /// account each agent's model calls go to. Read-only; never reads a
    /// credential.
    Infra,
    /// L1 Mac (`#260`): the host's macOS power mode for AC and battery, and
    /// whether Factory may change it. With a mode -- automatic,
    /// high-performance or energy-saving -- sets it for every power source
    /// (`pmset -a powermode`), which works once the sudoers rule it prints is
    /// installed. Needs `host.power` when an agent asks.
    #[command(name = "power-mode")]
    PowerMode {
        /// automatic, high-performance or energy-saving.
        #[arg(value_parser = parse_power_mode)]
        mode: Option<factory_core::protocol::PowerMode>,
    },
    /// Important dates: expiry metadata, dependencies, renewal lines and native clocks.
    Dates { #[arg(long)] scope: Option<String> },
    /// L1 Backup: whether the instance's own state -- the database, the
    /// authored content, the configs -- is backed up, how recently, and
    /// whether a backup has been proved to restore. With no subcommand,
    /// prints the status.
    Backup {
        #[command(subcommand)]
        command: Option<BackupCmd>,
    },
    /// Operations (`#185`): the environments what this company builds is
    /// deployed to, what runs on each, whether it is healthy and whether it
    /// meets its SLO. With a name, that environment's checks, incidents and
    /// deployments.
    Env {
        /// One environment. Every one when left out.
        name: Option<String>,
        /// Only this scope and the scopes under it.
        #[arg(long)]
        scope: Option<String>,
    },
    /// Start a frozen promotion workflow; deployment waits for owner approval.
    Promote {
        environment: String,
        #[arg(long)]
        deployment: String,
    },
    /// Restart/repair an installed environment through an owner-approved task.
    Recover { environment: String, #[arg(long)] reason: String },
    /// Record an explicit script recovery offline; never dispatches a task or connects to the daemon.
    #[command(subcommand)]
    RecoveryJournal(RecoveryJournalCmd),
    /// Run declared health checks now and record their answers; failed checks exit nonzero.
    EnvironmentCheck { environment: String },
    /// Record deployments as they start and finish, or list them.
    #[command(subcommand)]
    Deploy(DeployCmd),
    /// The release catalogue: what is available to deploy.
    #[command(subcommand)]
    Release(ReleaseCmd),
    /// Start, stop and type at standing agents.
    #[command(subcommand)]
    Agent(AgentCmd),
    /// Look at individual runs.
    #[command(subcommand)]
    Run(RunCmd),
    /// Follow the event stream.
    Watch,
    /// Create, run, and report on tasks.
    #[command(subcommand)]
    Task(TaskCmd),
    /// Workflows: define, start (with `--input`), follow and cancel runs;
    /// `lint` previews the control plan a run is held to.
    #[command(subcommand)]
    Workflow(WorkflowCmd),
    /// Read one scope's dependency inventory, or merge its authored VEX.
    Dependencies {
        /// `<scope>`, or `vex <scope>`.
        #[arg(num_args = 1..=2)]
        args: Vec<String>,
    },
    /// The L5 Knowledge tab: an index of the instance's knowledge vault,
    /// rebuilt from the files on every call. With no subcommand, prints the
    /// index; `import`/`add` are the only way anything is ever written --
    /// files are copied in, never edited or deleted.
    Knowledge {
        #[command(subcommand)]
        command: Option<KnowledgeCmd>,
    },
    /// The L5 Benchmarks tab: one configuration per distinct harness, full
    /// arguments and sandbox a task could be dispatched with today, and what
    /// each one is still missing to be a comparable score. With no
    /// subcommand, keeps that v1 output; `run`, `runs`, `show`, `cancel` and
    /// `clean` are v2's bench runs.
    Bench {
        #[command(subcommand)]
        cmd: Option<BenchCmd>,
    },
    /// Datasets: sets of cases a bench run attempts. `<root>/.factory/datasets/<name>.yaml`
    /// is the source of truth; every read re-parses it, so a hand edit shows
    /// up on the next call.
    #[command(subcommand)]
    Dataset(DatasetCmd),
    /// The L6 Policy tab: ADR 0004's catalogue of controls checked against
    /// evidence Factory already has -- `<root>/.factory/policies/<framework>.yaml`,
    /// re-read on every call, never a status table kept in step. With no
    /// subcommand, prints the status board: one line per framework with its
    /// rollup and whether it is compliant, then every open or stale control,
    /// then findings.
    Policy {
        /// Only this scope and its descendants (default: the whole instance).
        #[arg(long)]
        scope: Option<String>,
        /// Only this framework.
        #[arg(long)]
        framework: Option<String>,
        #[command(subcommand)]
        command: Option<PolicyCmd>,
    },
    /// Every named metric, computed fresh off data Factory already records
    /// -- the shared vocabulary the L6 Goals tab's key results (and, later,
    /// Scenarios) read from. With no ids, every non-parameterised metric
    /// plus whatever the loaded goals and policy catalogues imply.
    Metrics {
        /// e.g. `throughput_week`, `compliance.cra`, `bench.resolve_rate.eval-set-a`.
        ids: Vec<String>,
        /// Only this scope and its descendants (instance-wide metrics say so
        /// in their registry definition and ignore this selection).
        #[arg(long)]
        scope: Option<String>,
        /// Override run-backed metric intervals: day, 14d, or 90d.
        #[arg(long)]
        window: Option<MetricsWindow>,
    },
    /// The dashboard's resolved layout for a scope (`#159`): the nearest
    /// `dashboard:` block down its path, the instance root's own, or the
    /// built-in default when nothing overrides it.
    Dashboard {
        /// The scope to resolve for (default: the instance root itself).
        #[arg(long)]
        scope: Option<String>,
    },
    /// What runs used and cost, summed per task, GitHub issue (`issue=<n>`
    /// label), scope, agent, provider or workflow (#117, #164). Usage comes
    /// from the agent runtime; a run it could not measure is counted as
    /// unknown, never as free.
    Cost {
        /// task, issue, scope, agent, provider or workflow.
        #[arg(long, default_value = "task")]
        by: String,
        /// Runs started since this: `7d`, `12h`, `2026-09-01` or RFC 3339.
        /// Default: 30 days ago.
        #[arg(long)]
        since: Option<String>,
        /// Runs started before this, the same spellings. Default: now.
        #[arg(long)]
        until: Option<String>,
        /// Only this scope and its descendants.
        #[arg(long)]
        scope: Option<String>,
    },
    /// L6 monthly scope budgets and month-to-date spend. Limits are authored
    /// in .factory/budgets/limits.yaml, never inferred from provider plans.
    Budget {
        #[arg(long)]
        scope: Option<String>,
        #[arg(long, default_value = "scope")]
        by: String,
    },
    /// The L6 Goals tab: vision, mission, the north star and its inputs,
    /// every cycle's own summary, the asked (or current) cycle's graded
    /// report, and the roadmap. With no subcommand, prints the status view
    /// -- the same thing `factory goals status` prints.
    Goals {
        /// Only this scope and its descendants (default: the whole instance).
        #[arg(long)]
        scope: Option<String>,
        /// The cycle to report on (default: the current one, if any).
        #[arg(long)]
        cycle: Option<String>,
        #[command(subcommand)]
        command: Option<GoalsCmd>,
    },
    /// The L6 Scenarios tab (`#100`): every scenario at
    /// `<root>/.factory/scenarios/`, played against the exact policy
    /// evaluator, a seeded Monte Carlo forecast, the driver tree and
    /// signposts -- alongside the baseline they are compared against. A
    /// scenario file never changes the real config; everything here is
    /// recomputed on every call. With no subcommand, prints the board --
    /// the same thing `factory scenario list` prints.
    Scenario {
        /// Only this scope and its descendants (default: the whole instance).
        #[arg(long)]
        scope: Option<String>,
        #[command(subcommand)]
        command: Option<ScenarioCmd>,
    },
    /// The L6 Quality attributes tab (`#107`): which ISO 25010 qualities
    /// each scope declares (`.factory/quality/` profiles, bound by the
    /// root's `quality:` and each scope's `scope.quality`), how much they
    /// matter, and whether each scenario is met -- measured, never claimed.
    /// With no subcommand, prints the status board -- the same thing
    /// `factory quality status` prints.
    Quality {
        /// Only this scope and its descendants (default: the whole instance).
        #[arg(long)]
        scope: Option<String>,
        #[command(subcommand)]
        command: Option<QualityCmd>,
    },
    /// The L4 Operations tab (`#106`, design §12.5's `factory stats`): what
    /// needs a human now, work in flight and how old it is, the process's
    /// health over the last 7 or 30 days against the window before it, and
    /// every schedule's state. A read projection over tasks, runs and the
    /// journal, computed fresh on every call. With no subcommand, prints
    /// the summary -- the same thing `factory stats summary` prints.
    Stats {
        /// Only this scope and the scopes below it (default: every scope).
        #[arg(long)]
        scope: Option<String>,
        /// The health window: `7d` or `30d`.
        #[arg(long, value_parser = parse_window)]
        window: Option<HealthWindow>,
        #[command(subcommand)]
        command: Option<StatsCmd>,
    },
    /// Intake (`#119`), the inbound quality gate: work handed in here is
    /// held outside the dispatchable queue until triage -- the seven
    /// readiness axes, category, priority and estimate -- releases it
    /// (`ready`), sends it back (`needs-info`) or closes it (`wontfix`).
    /// With no subcommand, prints the board: received, triaging, needs-info
    /// and recently released.
    Intake {
        /// Only this scope and the scopes below it (default: every scope).
        #[arg(long)]
        scope: Option<String>,
        #[command(subcommand)]
        command: Option<IntakeCmd>,
    },
}

#[derive(Subcommand)]
enum IntakeCmd {
    /// The board: every item in intake, by column, oldest first.
    List {
        #[arg(long)]
        scope: Option<String>,
    },
    /// Hand an item in. It is not a task anyone can run until it is
    /// released.
    Add {
        title: String,
        /// What is being asked for, as the requester put it.
        #[arg(short, long, default_value = "")]
        instructions: String,
        /// The scope it is thought to belong to; triage routes it.
        #[arg(long)]
        scope: Option<String>,
        /// `email` or `chat`: relayed on somebody else's behalf (`#167`) --
        /// open to any caller holding `intake.add`, recorded as relayed,
        /// never trusted. Absent means an ordinary item from whoever is
        /// asking.
        #[arg(long, value_parser = ["email", "chat"])]
        source: Option<String>,
        /// Where it came from: an issue URL, a mail id -- and, with
        /// `--source email|chat`, the provider's own message id, required.
        #[arg(long)]
        reference: Option<String>,
        /// The system that relayed it -- `apple-mail`, `imessage`. Only
        /// with `--source email|chat`.
        #[arg(long)]
        provider: Option<String>,
        /// On whose behalf, when that is not you -- with `--source
        /// email|chat` this is the sender's own address or handle, and is
        /// required.
        #[arg(long)]
        requester: Option<String>,
        /// RFC 3339: the provider's own receipt time. Absent means now;
        /// refused if it is in the future. Only with `--source email|chat`.
        #[arg(long)]
        received_at: Option<String>,
        /// Repeatable: `--label area=infra`.
        #[arg(long = "label")]
        labels: Vec<String>,
        /// Flag it as a possible security report right away -- the same as
        /// `flag-security` straight after (`#170`).
        #[arg(long)]
        security: bool,
    },
    /// One item: its text, where it stands, its assessment and decision.
    Show { id: String },
    /// Start the triage node: a run that assesses the item and submits it.
    Triage {
        id: String,
        /// The agent to triage with (default: the scope's own).
        #[arg(long)]
        agent: Option<String>,
    },
    /// Record an assessment, read as JSON from a file (`-` for stdin) --
    /// the shape a triage run's instructions show.
    Assess {
        id: String,
        #[arg(long)]
        file: PathBuf,
        /// Also apply what the rules give: ready releases, needs-info sends
        /// it back, and a complete executable split expands into tasks.
        #[arg(long)]
        decide: bool,
    },
    /// Decide: `ready` releases, `needs-info` sends it back with questions,
    /// `split` replaces it with smaller intake items, `wontfix` closes it --
    /// only with a verified reason and evidence.
    Decide {
        id: String,
        #[arg(value_parser = ["ready", "needs-info", "split", "wontfix"])]
        decision: String,
        /// split: the parts as a JSON array of `{id, title, instructions,
        /// depends_on, acceptance, owns, interface, estimate_seconds}` (`-` for stdin). Absent takes the
        /// assessment's proposal.
        #[arg(long)]
        file: Option<PathBuf>,
        /// ready: dispatch the released task at once.
        #[arg(long)]
        run: bool,
        /// needs-info: repeatable. Absent asks the assessment's questions,
        /// or its failed axes.
        #[arg(long = "question")]
        questions: Vec<String>,
        /// wontfix: duplicate, invalid or out-of-scope.
        #[arg(long, value_parser = ["duplicate", "invalid", "out-of-scope"])]
        reason: Option<String>,
        /// wontfix: what verifies the reason.
        #[arg(long)]
        evidence: Option<String>,
        /// wontfix duplicate: what it duplicates.
        #[arg(long)]
        duplicate_of: Option<String>,
    },
    /// Approve and post a decided GitHub item's triage comment and labels to
    /// the issue it came from (`#171`). Factory never does this on its own;
    /// this is the approval. Needs `intake.publish`, named exactly -- never
    /// a wildcard, never `foreman` or `triager`. Refused for anything not
    /// sourced from GitHub, for an item with no decision or no assessment,
    /// and for one carrying a possible or confirmed security report.
    Publish { id: String },
    /// Add information to an item -- the answer to a needs-info, which puts
    /// it back in the queue.
    Info {
        id: String,
        text: String,
        /// Then start a triage run on it straight away.
        #[arg(long)]
        triage: bool,
        /// With `--triage`: the agent to triage with.
        #[arg(long, requires = "triage")]
        agent: Option<String>,
    },
    /// Flag an item still in the gate as a possible security report
    /// (`#170`) -- anyone who could assess the item may; it only adds
    /// scrutiny. Refused once the item already carries a flag of any kind.
    FlagSecurity {
        id: String,
        #[arg(long)]
        reason: String,
    },
    /// A person confirms or dismisses a possible security report. Owner
    /// only -- an agent may flag (above), never decide.
    Security {
        id: String,
        #[arg(value_parser = ["confirm", "dismiss"])]
        verdict: String,
        /// Required to dismiss; optional to confirm.
        #[arg(long)]
        evidence: Option<String>,
    },
    /// Every confirmed security report -- the L4 fact `#157`'s reporting
    /// clock will read.
    SecurityReports {
        #[arg(long)]
        scope: Option<String>,
    },
}

#[derive(Subcommand)]
enum QualityCmd {
    /// One line per scope and attribute: importance/difficulty and the
    /// attribute's rollup, the worst of its scenarios. No score -- there is
    /// none, per scope or overall.
    Status {
        #[arg(long)]
        scope: Option<String>,
    },
    /// One scope's whole utility tree: every attribute, every scenario with
    /// its measure, status, reasons and any open remediation task, then its
    /// trade-offs.
    Scope { name: String },
    /// Everything wrong with the profiles or the chains that bind them --
    /// unknown ids, loosening, more than seven attributes, an H attribute
    /// nobody measures.
    Findings {
        #[arg(long)]
        scope: Option<String>,
    },
    /// Create the task that closes one scenario's gap in `--scope`, labelled
    /// `quality=<scope>/<attribute>/<scenario>`. Needs `task.create` there,
    /// the same rule `factory task create` is checked against. Refused for
    /// a met scenario or a draft; answers the already-open task, creating
    /// nothing, when there is one.
    Remediate {
        /// `<attribute>/<scenario>`, e.g. `reliability.recoverability/daemon-restart`.
        scenario: String,
        #[arg(long)]
        scope: String,
        /// Overrides which agent the task runs as; the scope's own default
        /// otherwise.
        #[arg(long)]
        agent: Option<String>,
    },
}

#[derive(Subcommand)]
enum StatsCmd {
    /// Attention, flow, aging, health and schedules, briefly.
    Summary {
        #[arg(long)]
        scope: Option<String>,
        #[arg(long, value_parser = parse_window)]
        window: Option<HealthWindow>,
    },
    /// Only what needs a human: every exception, most severe and oldest
    /// first, with its reason and the actions it allows.
    Attention {
        #[arg(long)]
        scope: Option<String>,
    },
}

#[derive(Subcommand)]
enum ScenarioCmd {
    /// The scenario board: the baseline (its own forecast and policy
    /// rollup), then one summary line per scenario, then every currently
    /// triggered signpost, then findings.
    List {
        #[arg(long)]
        scope: Option<String>,
    },
    /// One scenario's full detail: its exact policy delta (subtree and per
    /// scope), driver outcomes and tornado, forecast, goal-scenario
    /// probabilities, and signposts.
    Show {
        name: String,
        #[arg(long)]
        scope: Option<String>,
    },
    /// Turn a scenario into real work: one task per newly-open control in
    /// `--scope`'s own slice of the scenario's policy delta, through the
    /// exact path `factory task create`/`factory policy remediate`
    /// themselves use. Skips a control that already has a non-terminal
    /// task labelled `policy=<framework>/<id>` there. Needs `task.create`
    /// in `--scope`, the same reach rule `factory task create` itself is
    /// checked against. A scenario itself never writes anything -- this is
    /// always an explicit, named action.
    Promote {
        name: String,
        #[arg(long)]
        scope: String,
        /// Overrides which agent every created task runs as; each scope's
        /// own default otherwise.
        #[arg(long)]
        agent: Option<String>,
    },
}

#[derive(Subcommand)]
enum GoalsCmd {
    /// Vision, mission, north star, objectives and their key results,
    /// roadmap, and findings.
    Status {
        #[arg(long)]
        scope: Option<String>,
        #[arg(long)]
        cycle: Option<String>,
    },
    /// Every cycle on disk, oldest first, with its own status and score.
    Cycles,
    /// Record a check-in against a manual key result -- the one way its
    /// value ever moves. `goals.checkin`, owner or a root-scope foreman
    /// only. Refused for a key result that is computed (backed by a
    /// metric), that no loaded cycle defines, or for a confidence outside
    /// `0..=10`.
    #[command(name = "checkin")]
    CheckIn {
        /// `objective/kr`.
        kr: String,
        #[arg(long)]
        value: f64,
        /// 0..=10.
        #[arg(long)]
        confidence: u8,
        #[arg(long)]
        note: Option<String>,
    },
}

/// What a release is, as `deploy start`, `deploy record` and `release add`
/// take it.
#[derive(clap::Args)]
struct ReleaseArgs {
    /// The commit being released.
    #[arg(long)]
    commit: String,
    /// `git describe`'s answer.
    #[arg(long)]
    describe: Option<String>,
    /// A version or tag.
    #[arg(long)]
    version: Option<String>,
    /// The build profile, e.g. release or debug.
    #[arg(long)]
    profile: Option<String>,
    /// Built from a working tree with uncommitted changes.
    #[arg(long)]
    dirty: bool,
    /// Where it was built from: a ref, a branch or a worktree path.
    #[arg(long)]
    source: Option<String>,
    /// The commit's committer time (RFC 3339). Asked of the scope's git
    /// repository when left out; lead time for changes starts here.
    #[arg(long)]
    committed_at: Option<String>,
    /// Explicit producing run. Evidence must match this scope and source commit.
    #[arg(long)]
    build_run: Option<String>,
    /// The producing scope, when different from the release's scope.
    #[arg(long)]
    build_scope: Option<String>,
    /// Compare against this immutable Git revision; otherwise use the previous release.
    #[arg(long)]
    compare_to: Option<String>,
}

impl ReleaseArgs {
    fn facts(self) -> Result<factory_core::environments::ReleaseFacts> {
        Ok(factory_core::environments::ReleaseFacts {
            commit: self.commit,
            describe: self.describe,
            version: self.version,
            profile: self.profile,
            dirty: self.dirty,
            source: self.source,
            committed_at: self.committed_at.as_deref().map(parse_rfc3339).transpose()?,
            build_run: self.build_run,
            build_scope: self.build_scope,
            compare_to: self.compare_to,
            changes: None,
        })
    }
}

fn parse_rfc3339(text: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    Ok(chrono::DateTime::parse_from_rfc3339(text.trim())
        .map_err(|e| anyhow!("{text:?} is not an RFC 3339 time: {e}"))?
        .with_timezone(&chrono::Utc))
}

fn parse_deploy_status(text: &str) -> Result<factory_core::environments::DeployStatus> {
    use factory_core::environments::DeployStatus;
    match text.trim().replace('-', "_").as_str() {
        "succeeded" | "success" | "ok" => Ok(DeployStatus::Succeeded),
        "failed" | "failure" => Ok(DeployStatus::Failed),
        "rolled_back" => Ok(DeployStatus::RolledBack),
        other => Err(anyhow!("{other:?} is not how a deployment ends: succeeded, failed or rolled-back")),
    }
}

#[derive(Subcommand)]
enum DeployCmd {
    /// Inspect the exact opt-in GitHub destination/metadata and its approval digest. No outbound write.
    MirrorPlan { id: String },
    /// Explicitly approve publishing that frozen plan to GitHub; requires deploy.publish.
    Publish { id: String, #[arg(long)] approval: String },
    /// A deployment has begun. Prints its id, which `finish` names.
    /// `deploy.record`, in the environment's scope.
    Start {
        /// The environment, e.g. staging.
        #[arg(long = "env")]
        environment: String,
        /// Skipped, paused or missing checks can never count as success.
        #[arg(long)]
        strict_verification: bool,
        /// The scope, for an environment nobody declared. A declared one's
        /// own scope is used.
        #[arg(long)]
        scope: Option<String>,
        #[command(flatten)]
        release: ReleaseArgs,
        /// What is recording it, when not a person by hand: release.sh.
        #[arg(long)]
        via: Option<String>,
        /// When it began (RFC 3339), for recording one after the fact.
        #[arg(long)]
        started_at: Option<String>,
    },
    /// A deployment has ended. A success runs the environment's own checks
    /// first, and is recorded as failed if they do not pass -- then this
    /// exits non-zero.
    Finish {
        id: String,
        /// succeeded, failed or rolled-back.
        #[arg(long)]
        status: String,
        #[arg(long)]
        reason: Option<String>,
        /// Record a success without running the checks. Said on the record.
        #[arg(long)]
        no_verify: bool,
    },
    /// Start and finish in one go, for a deployment done by hand.
    Record {
        #[arg(long = "env")]
        environment: String,
        #[arg(long)]
        scope: Option<String>,
        #[command(flatten)]
        release: ReleaseArgs,
        #[arg(long)]
        status: String,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        via: Option<String>,
        #[arg(long)]
        started_at: Option<String>,
        #[arg(long)]
        no_verify: bool,
    },
    /// Deployments, running ones first, then newest first.
    List {
        #[arg(long = "env")]
        environment: Option<String>,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
}

#[derive(Subcommand)]
enum RecoveryJournalCmd {
    /// Persist a start before acting. Requires an explicit existing instance root.
    Start {
        #[arg(long)] scope: String,
        #[arg(long = "env")] environment: String,
        #[arg(long)] source: String,
        #[arg(long)] actor: String,
        #[arg(long)] reason: String,
        #[arg(long)] command: String,
        #[arg(long)] commit: Option<String>,
    },
    /// Persist the script's reported exit and observed route checks; immutable and retryable.
    Finish {
        id: String,
        #[arg(long)] exit_code: u8,
        #[arg(long, action = clap::ArgAction::Set)] local_http: Option<bool>,
        #[arg(long, action = clap::ArgAction::Set)] network_routes: Option<bool>,
        #[arg(long)] detail: Option<String>,
    },
}

fn offline_recovery(root: &Path, command: &RecoveryJournalCmd, json: bool) -> Result<()> {
    use factory_core::recovery_journal::{self, ScriptRecoveryAction, ScriptRecoveryFinish};
    let action = match command {
        RecoveryJournalCmd::Start { scope, environment, source, actor, reason, command, commit } => recovery_journal::start(root,
            ScriptRecoveryAction { id: String::new(), scope: scope.clone(), environment: environment.clone(),
                source: source.clone(), actor: actor.clone(), reason: reason.clone(), command: command.clone(),
                expected_commit: commit.clone(), started_at: chrono::Utc::now(), finish: None })?,
        RecoveryJournalCmd::Finish { id, exit_code, local_http, network_routes, detail } => recovery_journal::finish(root, id,
            ScriptRecoveryFinish { at: chrono::Utc::now(), exit_code: *exit_code, local_http: *local_http,
                network_routes: *network_routes, detail: detail.clone() })?,
    };
    if json { println!("{}", serde_json::to_string(&action)?); }
    else { println!("{}", action.id); }
    Ok(())
}

#[derive(Subcommand)]
enum ReleaseCmd {
    /// Put a release in the catalogue without deploying it.
    Add {
        #[arg(long)]
        scope: String,
        #[command(flatten)]
        release: ReleaseArgs,
    },
    /// Every release, newest first, with where each is running.
    List {
        #[arg(long)]
        scope: Option<String>,
    },
}

#[derive(Subcommand)]
enum BackupCmd {
    /// The status: the newest backup's age against its schedule, the
    /// destination, the last verification and every warning -- the same
    /// thing `factory backup` with no subcommand prints.
    Status,
    /// Take a backup now, the same one the schedule takes, then apply
    /// retention. `backup.run`: the owner, or a root-scope agent holding it.
    Run,
    /// Every snapshot of this instance in the destination, newest first,
    /// with whether each was verified and which retention rule keeps it.
    List,
    /// Unpack a snapshot into a temporary directory and prove it would
    /// restore: its checksums, the database's integrity_check and every
    /// authored-content loader. Exits non-zero when a check fails. An
    /// encrypted snapshot needs `--identity`, and decrypting it that way is
    /// the owner's alone (`#152`).
    Verify {
        /// The snapshot's file name, as `list` shows it. The newest when
        /// left out.
        snapshot: Option<String>,
        /// A file holding the one native age identity (`AGE-SECRET-KEY-1…`)
        /// that decrypts this snapshot. Required for an encrypted one;
        /// refused for a path inside this instance's own `.factory/`. Never
        /// stored, logged or sent anywhere but read once by the daemon.
        #[arg(long)]
        identity: Option<PathBuf>,
    },
    /// Verify a snapshot, then materialize it into a new instance root --
    /// plaintext as before, or encrypted with `--identity` (`#152`).
    /// Owner-only. The destination must not exist or must be empty; this
    /// never stops the current daemon or switches roots for you.
    Restore {
        /// The snapshot's file name, as `list` shows it.
        snapshot: String,
        /// A new or empty instance root. Relative paths are resolved by this
        /// CLI before the request reaches the daemon.
        #[arg(long)]
        into: PathBuf,
        /// See `verify --identity`.
        #[arg(long)]
        identity: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum PolicyCmd {
    /// The status board -- the same thing `factory policy` with no
    /// subcommand prints.
    Status {
        #[arg(long)]
        scope: Option<String>,
        #[arg(long)]
        framework: Option<String>,
    },
    /// One control's full detail: its checks, freshness window, status and
    /// attestation history. Without `--scope`, looked up wherever the whole
    /// instance's status board shows it applying, as long as that is
    /// exactly one scope -- name one explicitly if it applies in several.
    Show {
        /// `framework/id`, e.g. `cra/annex-i-2-1`.
        control: String,
        #[arg(long)]
        scope: Option<String>,
    },
    /// Record an attestation -- the one check a person satisfies by saying
    /// so, and the only new state ADR 0004 introduces. `policy.attest`,
    /// owner or a root-scope foreman only.
    Attest {
        /// `framework/id`.
        control: String,
        #[arg(long)]
        scope: String,
        /// A pointer to the evidence -- a document, a ticket, a page --
        /// not the evidence itself.
        #[arg(long)]
        evidence: String,
        /// `30d`, `12w`, a bare date (`2027-01-01`), or a full RFC3339
        /// timestamp. Defaults to `520w` (chosen since `Duration` has no
        /// years) when `--clock-item` or `--corrective-item` is given -- CRA evidence is kept far
        /// longer than an ordinary attestation's expiry ever matters for --
        /// and is otherwise required.
        #[arg(long)]
        expires: Option<String>,
        #[arg(long)]
        note: Option<String>,
        /// Also submit against a CRA Art. 14 reporting-clock item (`#157`,
        /// phase 1): `finding:<scope>:<vulnerability>` or
        /// `report:<task-id>`, from `factory policy clock`. Requires
        /// `--deadline`.
        #[arg(long = "clock-item", requires = "deadline", conflicts_with = "corrective_item")]
        clock_item: Option<String>,
        /// Which deadline this submits: `early-warning`, `notification`
        /// or `final-report`. Requires `--clock-item`.
        #[arg(long, requires = "clock_item")]
        deadline: Option<String>,
        /// Record evidenced corrective/mitigating measure availability for
        /// this clock item, instead of a submission.
        #[arg(long, requires = "available_at", conflicts_with = "clock_item")]
        corrective_item: Option<String>,
        /// When the measure became available (RFC3339), not the record time.
        #[arg(long, requires = "corrective_item")]
        available_at: Option<chrono::DateTime<chrono::Utc>>,
    },
    /// The CRA Art. 14 reporting clock (`#157`, phase 1): every exploited L2
    /// finding and confirmed L4 security report's 24-hour early-warning and
    /// 72-hour notification deadlines, plus the 14-day final report once
    /// corrective-measure availability is evidenced. `--scope` narrows to that subtree,
    /// the whole instance when absent -- the same rollup `status` itself
    /// uses.
    Clock {
        #[arg(long)]
        scope: Option<String>,
    },
    /// Withdraw a previously recorded attestation. Appends a new row; the
    /// one it names is never edited.
    Withdraw {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// The catalogues on disk and their control counts.
    Frameworks,
    /// Close a gap: create the task that carries the control's own
    /// `remediation:` guidance and its current missing evidence, in
    /// `--scope`. Needs `task.create` there, under the same rule
    /// `factory task create` itself needs -- this is not a way around it.
    /// Refused, naming the existing task, when a non-terminal task labelled
    /// `policy=<framework>/<id>` is already open in that scope; refused
    /// outright when the control is already satisfied, attested, or n/a.
    Remediate {
        /// `framework/id`.
        control: String,
        #[arg(long)]
        scope: String,
        /// Overrides which agent the task runs as; the scope's own default
        /// otherwise.
        #[arg(long)]
        agent: Option<String>,
    },
    /// A snapshot for an auditor: every framework's rollup, then each
    /// control's status, reasons, evidence pointers and attestation
    /// history, and every n/a rationale -- printed to stdout.
    Export {
        /// Only this scope and its descendants (default: the whole
        /// instance).
        #[arg(long)]
        scope: Option<String>,
        #[arg(long, default_value = "md")]
        format: String,
    },
}

#[derive(Subcommand)]
enum BenchCmd {
    /// Start a bench run: dataset@revision x agents x attempts.
    Run {
        dataset: String,
        /// `[<scope>/]<agent>`, repeatable. Only the trailing name is used --
        /// the agent must resolve in each case's own scope, not the scope it
        /// happened to be found under in the roster.
        #[arg(long = "agent", required = true)]
        agents: Vec<String>,
        #[arg(long, default_value_t = 1)]
        attempts: u32,
        #[arg(long, default_value_t = 1)]
        concurrency: u32,
        /// A case id to attempt, repeatable. Omit for every case.
        #[arg(long = "case")]
        cases: Vec<String>,
    },
    /// List bench runs, most recently updated first.
    Runs {
        #[arg(long)]
        dataset: Option<String>,
    },
    /// One bench run: its results table and every attempt.
    Show { id: String },
    /// Cancel a bench run still going: cancels every in-flight attempt and
    /// marks the rest cancelled.
    Cancel { id: String },
    /// Remove exactly this finished run's worktrees and branches. Owner
    /// only; refused while the run is still going.
    Clean { id: String },
}

#[derive(Subcommand)]
enum DatasetCmd {
    /// List every dataset.
    List,
    /// One dataset: its cases, description, and findings.
    Show { name: String },
    /// Create an empty dataset.
    Create {
        name: String,
        #[arg(long)]
        description: Option<String>,
    },
    /// Add or remove a case.
    #[command(subcommand)]
    Case(CaseCmd),
    /// Bulk import cases from a file: `.jsonl` (one case per line), `.json`
    /// (an array), `.yaml`/`.yml` (a whole dataset or a list of cases), or
    /// `.csv` (a header row of case field names; `origin` is not accepted).
    /// All or nothing -- a bad case anywhere refuses the whole file. Creates
    /// the dataset if it does not exist yet.
    Import {
        name: String,
        file: PathBuf,
        /// Overrides the format normally inferred from the file's extension.
        #[arg(long)]
        format: Option<String>,
        /// Replace every existing case instead of appending.
        #[arg(long)]
        replace: bool,
    },
    /// Turn recorded tasks into cases. Each becomes one case, with its id a
    /// slug of the title, de-duplicated; a generated case never carries a
    /// `gate`.
    FromTasks {
        name: String,
        /// Explicit task ids. Omit and use --scope/--status to select in
        /// bulk instead.
        task_ids: Vec<String>,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long)]
        status: Option<TaskStatus>,
    },
    /// Delete a whole dataset.
    Rm { name: String },
}

#[derive(Subcommand)]
enum CaseCmd {
    Add {
        name: String,
        #[arg(long)]
        id: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        scope: String,
        #[arg(long)]
        instructions: String,
        /// A commit to branch attempts from. Absent means the scope's HEAD
        /// when the bench run starts.
        #[arg(long)]
        base: Option<String>,
        /// Runs in the attempt worktree before the agent starts.
        #[arg(long)]
        reset: Option<String>,
        /// Runs in the attempt worktree after the attempt ends; its exit
        /// status is the verdict. Absent means the case runs `unverified`.
        #[arg(long)]
        gate: Option<String>,
        #[arg(long)]
        timeout: Option<u64>,
    },
    Rm { name: String, id: String },
}

#[derive(Subcommand)]
enum AgentCmd {
    /// Bring a declared standing agent up.
    Start { scope: String, name: String },
    /// Take one down and leave it down.
    Stop { id: String },
    /// What its terminal shows right now.
    Output {
        id: String,
        #[arg(long, default_value_t = 200)]
        lines: u32,
    },
    /// Give it a role, or clear the one it was given.
    Role {
        id: String,
        /// The role to give it. Leave it out to go back to the config's.
        role: Option<String>,
    },
    /// Type at it: text, then any keys. `--key enter` submits.
    Input {
        id: String,
        #[arg(short, long)]
        text: Option<String>,
        /// Repeatable: `--key enter`, `--key esc`, `--key ctrl-c`.
        #[arg(long = "key")]
        keys: Vec<String>,
    },
}

#[derive(Subcommand)]
enum KnowledgeCmd {
    /// Copy a directory tree into the vault, preserving relative paths so
    /// `[[area/page]]` still resolves afterwards. Pages and documents travel
    /// together; nothing already there is ever edited or deleted.
    Import {
        source: PathBuf,
        /// Where under the vault the tree lands. Defaults to the vault root.
        #[arg(long)]
        into: Option<String>,
        /// Replace a target that already exists, instead of skipping it.
        #[arg(long)]
        overwrite: bool,
    },
    /// Add one or more files to the vault. Without `--into`, each file finds
    /// its own default: the vault root for a `.md` file, `documents/` for
    /// anything else.
    Add {
        files: Vec<PathBuf>,
        #[arg(long)]
        into: Option<String>,
        #[arg(long)]
        overwrite: bool,
    },
    /// Find pages by words and tags, best first. Prints each page's path and
    /// why it matched -- never its text; open the file to read it.
    Search {
        /// Words to look for. Quote them or not; they are joined either way.
        words: Vec<String>,
        /// Only pages carrying this tag. Repeat for more; a page needs all.
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Weigh pages whose `area` is this scope. An agent's own scope is
        /// used when it gives none.
        #[arg(long)]
        scope: Option<String>,
        /// How many pages at most (default 10, never more than 50).
        #[arg(long)]
        limit: Option<usize>,
    },
}

#[derive(Subcommand)]
enum WorkflowCmd {
    /// The control plan (`#118`) a workflow, a task or a scope's category is
    /// held to: the steps its policy controls and quality attributes
    /// require, which of them a run gets injected as locked gates, which
    /// authored gate nodes already satisfy one, and the before/after rules
    /// the graph breaks. A preview for authors -- the daemon enforces the
    /// same plan when a run reports done.
    Lint {
        /// A stored workflow's id.
        workflow: Option<String>,
        /// Lint a task instead, as the one-node workflow it runs as.
        #[arg(long, conflicts_with = "workflow")]
        task: Option<String>,
        /// With neither a workflow nor a task: the scope whose plan to show.
        #[arg(long)]
        scope: Option<String>,
        /// The category to plan for, instead of the one the workflow or
        /// task names (or `default`).
        #[arg(long)]
        category: Option<String>,
    },
    /// Every workflow definition, or a scope's.
    List {
        #[arg(long)]
        scope: Option<String>,
    },
    /// One definition: its inputs, nodes, edges and ordered exits.
    Show { id: String },
    /// Create a workflow from a YAML or JSON file shaped like
    /// `WorkflowDraft` -- `name`, `scope`, `inputs`, `nodes`, `edges`.
    Create {
        #[arg(long)]
        file: std::path::PathBuf,
    },
    /// Replace a workflow's definition with a file's. A run already going
    /// keeps the revision it started with.
    Update {
        id: String,
        #[arg(long)]
        file: std::path::PathBuf,
    },
    /// Start a run. Each input the workflow declares is `--input name=value`.
    Start {
        id: String,
        #[arg(long = "input", value_name = "NAME=VALUE")]
        inputs: Vec<String>,
    },
    /// A workflow's runs, newest first -- or every run, without an id.
    Runs {
        id: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// One run: every node's status, task, route, and repeated round.
    Run { run_id: String },
    /// Cancel a run and every task of it still going.
    Cancel { run_id: String },
}

#[derive(Subcommand)]
enum RunCmd {
    /// The runs of a task, newest first.
    List {
        task_id: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// One run.
    Show { id: String },
    /// The evidence a run's required steps left (`#118`): who ran each
    /// gate, on which commit, and what it found. Append-only.
    Attestations { id: String },
    /// Immutable release artifact evidence, as in-toto / SLSA v1 statements.
    Provenance { id: String },
    /// Pass a required person approval before the subject is dispatched.
    Approve {
        id: String,
        #[arg(long)]
        reason: String,
    },
    /// Reject a required approval and keep the line stopped.
    Reject {
        id: String,
        #[arg(long)]
        reason: String,
    },
    /// Accept the verifier's evidence-backed rework proposal.
    Rework { id: String },
    /// One run's usage snapshots as the runtime answered them -- at
    /// dispatch, each turn end and the end -- the record behind the usage
    /// `run show` prints (#117).
    Usage { id: String },
    /// One run's journal.
    Log {
        id: String,
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },
    /// One run's terminal: live while it runs, its transcript afterwards.
    Output {
        id: String,
        #[arg(long, default_value_t = 200)]
        lines: u32,
    },
    /// Answer a blocked run: type the text into its own session and press
    /// enter. Only while the run is blocked. The reason is required and is
    /// journaled, with who gave it, as an intervention; the text is not.
    Answer {
        id: String,
        text: String,
        #[arg(long)]
        reason: String,
    },
}

#[derive(Subcommand)]
enum TaskCmd {
    /// List tasks.
    List {
        /// A status, or `failed` (blocked by a failed run) or `closed`
        /// (done or cancelled).
        #[arg(long)]
        status: Option<StatusFilter>,
        #[arg(long)]
        scope: Option<String>,
        /// Only direct children produced from this parent task.
        #[arg(long)]
        parent: Option<String>,
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Create a task. Creating does not start it: nothing dispatches a
    /// pending task on its own, so pass `--run`, give it a `--schedule`/`--after`, or
    /// run it later with `factory task run <id>`.
    Create {
        title: String,
        /// What the agent is being asked to do.
        #[arg(short, long, default_value = "")]
        instructions: String,
        #[arg(long)]
        scope: Option<String>,
        /// An agent the scope declares (`assistant`), or any adapter (`pi`).
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        runtime: Option<String>,
        /// `every 300`, `every 5m`, or a cron expression.
        #[arg(long)]
        schedule: Option<String>,
        /// Start once all these tasks are done. Repeat for multiple parents.
        #[arg(long, conflicts_with_all = ["schedule", "run"])]
        after: Vec<String>,
        /// The IANA timezone a cron schedule's fields are read in, as in
        /// `Europe/Berlin`. Without it they are UTC.
        #[arg(long, requires = "schedule")]
        timezone: Option<String>,
        /// Expected seconds one run will occupy its agent (advisory only).
        #[arg(long)]
        estimate: Option<u64>,
        /// Low and high seconds for a range; `--estimate` is the expected value.
        #[arg(long)]
        estimate_low: Option<u64>,
        #[arg(long)]
        estimate_high: Option<u64>,
        /// Low, expected and high API-equivalent dollar estimate.
        #[arg(long)]
        estimate_cost_low: Option<f64>,
        #[arg(long)]
        estimate_cost: Option<f64>,
        #[arg(long)]
        estimate_cost_high: Option<f64>,
        /// Seconds this task's agent has to acknowledge a run.
        #[arg(long)]
        ack_timeout: Option<u64>,
        /// Seconds a run of this task may take.
        #[arg(long)]
        timeout: Option<u64>,
        /// Seconds a run may sit `blocked` waiting for a human before the
        /// daemon gives up on it too.
        #[arg(long)]
        blocked_timeout: Option<u64>,
        /// How a failed *scheduled* run of this task is retried: `none`, or
        /// `<attempts>x<backoff>` as in `3x5m`. Unset uses the daemon's
        /// default -- see `factory task show` on an existing task for what
        /// that resolves to.
        #[arg(long)]
        retry: Option<String>,
        /// Repeatable: `--label area=infra`.
        #[arg(long = "label")]
        labels: Vec<String>,
        /// Give this task its own git worktree, made fresh before each run.
        /// On unless you pass `--no-worktree`.
        #[arg(long)]
        worktree: bool,
        #[arg(long)]
        no_worktree: bool,
        /// Hand each run the knowledge pages that match this task's title and
        /// instructions, searched when the run starts.
        #[arg(long)]
        knowledge_hints: bool,
        /// What kind of work this is -- `feature`, `bugfix`, `release`,
        /// `docs` -- which picks the control plan its runs are held to.
        /// Without it the task is planned as `default`.
        #[arg(long)]
        category: Option<String>,
        /// Dispatch it immediately as well.
        #[arg(long)]
        run: bool,
    },
    /// Change a task. Only what you pass is touched.
    Edit {
        id: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(short, long)]
        instructions: Option<String>,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        runtime: Option<String>,
        /// `every 300`, `every 5m`, or a cron expression.
        #[arg(long)]
        schedule: Option<String>,
        /// The IANA timezone a cron schedule's fields are read in, as in
        /// `Europe/Berlin`. Without it they are UTC. Goes with `--schedule`,
        /// which it is part of: an edit restates the whole schedule.
        #[arg(long, requires = "schedule")]
        timezone: Option<String>,
        /// Drop the schedule and go back to manual.
        #[arg(long)]
        no_schedule: bool,
        /// Expected seconds one run will occupy its agent (advisory only).
        #[arg(long)]
        estimate: Option<u64>,
        #[arg(long)]
        estimate_low: Option<u64>,
        #[arg(long)]
        estimate_high: Option<u64>,
        #[arg(long)]
        estimate_cost_low: Option<f64>,
        #[arg(long)]
        estimate_cost: Option<f64>,
        #[arg(long)]
        estimate_cost_high: Option<f64>,
        /// Remove the task's duration estimate.
        #[arg(long)]
        no_estimate: bool,
        #[arg(long)]
        ack_timeout: Option<u64>,
        #[arg(long)]
        timeout: Option<u64>,
        #[arg(long)]
        blocked_timeout: Option<u64>,
        /// Go back to the instance defaults.
        #[arg(long)]
        default_timeouts: bool,
        /// `none`, or `<attempts>x<backoff>` as in `3x5m`.
        #[arg(long)]
        retry: Option<String>,
        /// Go back to the daemon's default retry policy.
        #[arg(long)]
        default_retry: bool,
        /// Repeatable; replaces the whole set.
        #[arg(long = "label")]
        labels: Vec<String>,
        /// Hand each run the knowledge pages that match this task.
        #[arg(long, conflicts_with = "no_knowledge_hints")]
        knowledge_hints: bool,
        /// Stop handing runs knowledge pages.
        #[arg(long)]
        no_knowledge_hints: bool,
        /// Stop the schedule firing, keeping it: nothing runs on its own
        /// until it is resumed.
        #[arg(long, conflicts_with = "resume_schedule")]
        pause_schedule: bool,
        /// Start a paused schedule again, from now -- the slots it passed
        /// while paused are not caught up.
        #[arg(long)]
        resume_schedule: bool,
        /// Why -- journaled with who asked when the edit pauses or resumes
        /// the schedule.
        #[arg(long)]
        reason: Option<String>,
        /// Set the category its runs are planned as.
        #[arg(long, conflicts_with = "default_category")]
        category: Option<String>,
        /// Go back to the default category.
        #[arg(long)]
        default_category: bool,
    },
    /// Show one task.
    Show { id: Option<String> },
    /// Dispatch a task now. Journaled with who asked, and why if you say.
    Run {
        id: Option<String>,
        #[arg(long)]
        reason: Option<String>,
        /// Resume the task's newest run's harness session (`#178`) instead
        /// of starting fresh. Refused unless that run is terminal and ended
        /// on an infrastructure failure (an ack timeout, a run timeout, a
        /// session gone, or a dispatch failure after a session had already
        /// come up); from there Factory falls back to a fresh session --
        /// journaled with the reason -- for anything that stops the resume
        /// itself from going through.
        #[arg(long = "continue")]
        continue_run: bool,
        /// Run before upstream releases it. Requires a journaled --reason.
        #[arg(long, requires = "reason")]
        override_wait: bool,
    },
    /// Stop a running task and close its session. Journaled with who
    /// asked, and why if you say.
    Cancel {
        id: Option<String>,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Close a task on purpose, with a reason: `completed`, `not_planned`
    /// (won't do) or `duplicate`. Works with no run in progress -- a task
    /// blocked by a failure waits for exactly this. Journaled with who
    /// closed it, and the note if you give one.
    Close {
        id: Option<String>,
        /// completed, not_planned or duplicate.
        #[arg(long, value_parser = parse_close_reason)]
        reason: CloseReason,
        /// The task this one duplicates (with --reason duplicate).
        #[arg(long)]
        duplicate_of: Option<String>,
        /// Why, in a few words.
        #[arg(long)]
        note: Option<String>,
    },
    /// Put a closed task back to pending. Journaled with who asked, and
    /// why if you say.
    Reopen {
        id: Option<String>,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Pass over a scheduled task's next slot: the one after it is the next
    /// firing. A queued retry is the next slot, and is dropped. Journaled
    /// with who asked, and why if you say.
    SkipNext {
        id: Option<String>,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Delete a task.
    Delete { id: Option<String> },
    /// The task's journal.
    Log {
        id: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Recent terminal output from the task's session.
    Output {
        id: Option<String>,
        #[arg(long, default_value_t = 120)]
        lines: u32,
    },
    /// Say how a task is going. This is what a dispatched agent calls.
    Report {
        id: Option<String>,
        #[arg(long)]
        status: Option<RunStatus>,
        #[arg(short, long)]
        message: Option<String>,
        #[arg(long)]
        result: Option<String>,
        /// On `done`, select one declared `agent:` exit of this workflow node.
        #[arg(long = "send-to", requires = "status")]
        send_to: Option<String>,
        /// Read the result's body from a file instead of (or alongside)
        /// --result -- the `shell` agent's own report line uses this to
        /// carry a command's captured stdout, which can hold quotes, `$`,
        /// backticks and newlines that would not survive being typed inline.
        /// Combines with --result rather than conflicting with it:
        /// --result becomes a header, this file's content becomes the body,
        /// joined by a blank line -- unless the file is empty, in which case
        /// the header alone is the result. Tail-truncated to the last 64 KiB
        /// on a UTF-8 boundary, with non-UTF-8 bytes decoded lossily and a
        /// truncation marker prepended, so a noisy command cannot blow up
        /// the task record.
        #[arg(long = "result-file")]
        result_file: Option<PathBuf>,
        /// Capture a release artifact from this run's working directory.
        /// Repeat for multiple files; only valid with --status done.
        #[arg(long = "artifact", requires = "status")]
        artifacts: Vec<String>,
        #[arg(long)]
        error: Option<String>,
        /// Defaults to FACTORY_TASK_TOKEN, which the daemon sets in the
        /// session alongside FACTORY_TOKEN.
        #[arg(long = "run-token", env = "FACTORY_TASK_TOKEN")]
        token: Option<String>,
    },
    /// Attach an immutable CycloneDX scan document to this run.
    Attach {
        #[arg(long)]
        kind: AttachmentKind,
        file: PathBuf,
        /// Defaults to FACTORY_TASK_ID.
        #[arg(long)]
        id: Option<String>,
        #[arg(long = "run-token", env = "FACTORY_TASK_TOKEN")]
        token: Option<String>,
    },
    /// What a harness's turn-end hook runs -- Claude Code's `Stop` and
    /// `StopFailure`, from the settings file Factory generates for a run.
    /// Not for agents: it reads the hook's JSON from stdin, prints nothing,
    /// and exits 0 whatever happens, because it runs inside the harness and
    /// must never be the thing that disturbs a turn.
    #[command(hide = true)]
    TurnEnded {
        id: Option<String>,
        #[arg(long, value_enum)]
        event: HookEvent,
        #[arg(long = "run-token", env = "FACTORY_TASK_TOKEN")]
        token: Option<String>,
    },
}

/// Which hook fired, as `task turn-ended --event` spells it.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum HookEvent {
    Stop,
    StopFailure,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Command::RecoveryJournal(command) = &cli.command {
        let root = cli.root.as_deref().ok_or_else(|| anyhow!("offline recovery-journal requires --root (or FACTORY_ROOT)"))?;
        return offline_recovery(root, command, cli.json);
    }
    let client = Client::locate(cli.socket.clone(), cli.url.clone(), cli.root.clone(), cli.token.clone())?;

    match cli.command {
        Command::RecoveryJournal(_) => unreachable!("offline command is handled before locating any client"),
        Command::Status => {
            let payload = client.send(Request::Status).await?;
            print(&payload, cli.json, |p| match p {
                Payload::Status { status } => {
                    let mut out = format!(
                        "{}  ({})\n  root        {}\n  version     {}\n  uptime      {}s\n  tasks       {} total, {} active\n  interfaces  {}\n  watching    {} subscriber(s)\n  socket      {}",
                        status.instance,
                        status.instance_id,
                        status.root,
                        status.version,
                        status.uptime_seconds,
                        status.tasks_total,
                        status.tasks_active,
                        status.interfaces.join(", "),
                        status.subscribers,
                        client.endpoint(),
                    );
                    if !status.capacity.is_empty() {
                        out.push_str("\n  capacity    ");
                        let rows: Vec<String> = status
                            .capacity
                            .iter()
                            .map(|c| {
                                if c.waiting > 0 {
                                    format!("{} {}/{}, {} waiting", c.agent, c.in_use, c.max, c.waiting)
                                } else {
                                    format!("{} {}/{}", c.agent, c.in_use, c.max)
                                }
                            })
                            .collect();
                        out.push_str(&rows.join("; "));
                    }
                    Some(out)
                }
                _ => None,
            })
        }

        Command::Env { name, scope } => {
            let payload = client.send(Request::Environments { scope }).await?;
            match name {
                None => print(&payload, cli.json, env_list_text),
                Some(name) => {
                    let Payload::Environments { report } = &payload else {
                        return print(&payload, cli.json, |_| None);
                    };
                    let card = report
                        .environments
                        .iter()
                        .find(|c| c.name == name)
                        .ok_or_else(|| anyhow!("no environment named {name:?} is declared or deployed to"))?;
                    if cli.json {
                        println!("{}", serde_json::to_string_pretty(card)?);
                        Ok(())
                    } else {
                        println!("{}", env_detail_text(card, &report.deployments, report.generated_at));
                        Ok(())
                    }
                }
            }
        }

        Command::Promote { environment, deployment } => {
            let payload = client.send(Request::EnvironmentPromote(factory_core::environments::Promote { environment, deployment })).await?;
            print(&payload, cli.json, |payload| match payload {
                Payload::WorkflowRun { run } => Some(format!("promotion workflow {}; deployment waits for owner approval", run.id)),
                _ => None,
            })
        }
        Command::Recover { environment, reason } => {
            let payload = client.send(Request::EnvironmentRecover(factory_core::environments::Recover { environment, reason })).await?;
            print(&payload, cli.json, |payload| match payload {
                Payload::WorkflowRun { run } => Some(format!("recovery workflow {}; command waits for owner approval", run.id)),
                _ => None,
            })
        }
        Command::EnvironmentCheck { environment } => {
            let payload = client.send(Request::EnvironmentCheck { environment }).await?;
            let passed = matches!(&payload, Payload::EnvironmentVerification { verification } if verification.ok && !verification.checks.is_empty());
            print(&payload, cli.json, |payload| match payload {
                Payload::EnvironmentVerification { verification } => Some(format!("{}: {} health checks", if passed { "passed" } else { "FAILED" }, verification.checks.len())),
                _ => None,
            })?;
            if !passed { return Err(anyhow!("environment verification did not pass")); }
            Ok(())
        }
        Command::Deploy(cmd) => match cmd {
            DeployCmd::MirrorPlan { id } => {
                let payload = client.send(Request::DeployMirrorPlan { id }).await?;
                print(&payload, cli.json, |payload| match payload {
                    Payload::DeploymentMirrorPlan { plan } => Some(format!("{}: {} {} at {} → {}\nverified: {:?}\napproval: {}\nThis creates GitHub deployment events (task factory:mirror). Inspect downstream automation before approving.",
                        plan.deployment, plan.repository, plan.environment, plan.commit, plan.state, plan.verified, plan.approval)),
                    _ => None,
                })
            }
            DeployCmd::Publish { id, approval } => {
                let payload = client.send(Request::DeployPublish { id, approval }).await?;
                let published = matches!(&payload, Payload::DeploymentMirror { receipt } if receipt.phase == factory_core::environments::DeploymentMirrorPhase::Published);
                print(&payload, cli.json, |payload| match payload {
                    Payload::DeploymentMirror { receipt } => Some(format!("{:?}: {} {}{}", receipt.phase, receipt.plan.repository, receipt.plan.deployment,
                        receipt.error.as_ref().map(|error| format!(" — {error}")).unwrap_or_default())),
                    _ => None,
                })?;
                if !published { return Err(anyhow!("deployment mirror was not published; its failure receipt is stored")); }
                Ok(())
            }
            DeployCmd::Start { environment, strict_verification, scope, release, via, started_at } => {
                let req = factory_core::environments::DeployStart {
                    environment,
                    strict_verification,
                    scope,
                    release: release.facts()?,
                    via,
                    started_at: started_at.as_deref().map(parse_rfc3339).transpose()?,
                };
                let payload = client.send(Request::DeployStart(req)).await?;
                // The bare id, so a script can keep it: id=$(factory deploy start ...)
                print(&payload, cli.json, |p| match p {
                    Payload::Deployment { deployment } => Some(deployment.id.clone()),
                    _ => None,
                })
            }
            DeployCmd::Finish { id, status, reason, no_verify } => {
                let wanted = parse_deploy_status(&status)?;
                let req = factory_core::environments::DeployFinish { id, status: wanted, reason, verify: !no_verify };
                let payload = client.send(Request::DeployFinish(req)).await?;
                print(&payload, cli.json, deployment_text)?;
                refused_success(&payload, wanted)
            }
            DeployCmd::Record { environment, scope, release, status, reason, via, started_at, no_verify } => {
                let wanted = parse_deploy_status(&status)?;
                let start = factory_core::environments::DeployStart {
                    environment,
                    strict_verification: false,
                    scope,
                    release: release.facts()?,
                    via,
                    started_at: started_at.as_deref().map(parse_rfc3339).transpose()?,
                };
                let Payload::Deployment { deployment } = client.send(Request::DeployStart(start)).await? else {
                    return Err(anyhow!("the daemon did not answer with a deployment"));
                };
                let finish = factory_core::environments::DeployFinish {
                    id: deployment.id.clone(),
                    status: wanted,
                    reason,
                    verify: !no_verify,
                };
                let payload = client.send(Request::DeployFinish(finish)).await?;
                print(&payload, cli.json, deployment_text)?;
                refused_success(&payload, wanted)
            }
            DeployCmd::List { environment, scope, limit } => {
                let payload = client.send(Request::Environments { scope }).await?;
                let Payload::Environments { report } = &payload else {
                    return print(&payload, cli.json, |_| None);
                };
                let rows: Vec<&factory_core::environments::Deployment> = report
                    .deployments
                    .iter()
                    .filter(|d| environment.as_ref().is_none_or(|e| &d.environment == e))
                    .take(limit)
                    .collect();
                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&rows)?);
                } else {
                    println!("{}", deployments_table(&rows, report.generated_at));
                }
                Ok(())
            }
        },

        Command::Release(cmd) => match cmd {
            ReleaseCmd::Add { scope, release } => {
                let req = factory_core::environments::ReleaseAdd { scope, release: release.facts()? };
                let payload = client.send(Request::ReleaseAdd(req)).await?;
                print(&payload, cli.json, |p| match p {
                    Payload::ReleaseAdded { scope, release } => {
                        Some(format!("release {} added to {scope}", short(&release.commit)))
                    }
                    _ => None,
                })
            }
            ReleaseCmd::List { scope } => {
                let payload = client.send(Request::Environments { scope }).await?;
                let Payload::Environments { report } = &payload else {
                    return print(&payload, cli.json, |_| None);
                };
                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&report.releases)?);
                    return Ok(());
                }
                if report.releases.is_empty() {
                    println!("no releases recorded yet");
                    return Ok(());
                }
                println!("{:<10} {:<28} {:<16} {:<18} {:>7}  RUNNING ON", "COMMIT", "DESCRIBE", "SCOPE", "FIRST SEEN", "DEPLOYS");
                for r in &report.releases {
                    println!(
                        "{:<10} {:<28} {:<16} {:<18} {:>7}  {}",
                        short(&r.facts.commit),
                        r.facts.version.as_deref().or(r.facts.describe.as_deref()).unwrap_or("--"),
                        r.scope,
                        r.first_seen.format("%Y-%m-%d %H:%M"),
                        if r.failed_deployments > 0 {
                            format!("{}/{}!", r.deployments, r.failed_deployments)
                        } else {
                            r.deployments.to_string()
                        },
                        if r.running_on.is_empty() { "--".into() } else { r.running_on.join(", ") }
                    );
                }
                Ok(())
            }
        },

        Command::Infra => {
            let payload = client.send(Request::Infrastructure).await?;
            print(&payload, cli.json, infrastructure_text)
        }

        Command::PowerMode { mode } => {
            let request = match mode {
                Some(mode) => Request::HostPowerModeSet { mode },
                None => Request::HostPowerMode,
            };
            let payload = client.send(request).await?;
            print(&payload, cli.json, power_mode_text)
        }

        Command::Dates { scope } => {
            let payload = client.send(Request::ImportantDates { scope }).await?;
            print(&payload, cli.json, |payload| match payload {
                Payload::ImportantDates { report } => Some(report.entries.iter().map(|entry| {
                    let item = &entry.observation;
                    let date = item.expires_at.map(|date| date.to_rfc3339()).unwrap_or_else(|| if item.no_expiry { if item.source == factory_core::renewals::DateSource::GithubAuth { "no reported expiry".into() } else { "no expiry".into() } } else { "unknown".into() });
                    let dependencies = item.affects.iter().map(|dependency| dependency.label.as_str()).collect::<Vec<_>>().join(", ");
                    format!("{}  {date}  {:?} / {:?}{}\n  {}\n  affects: {dependencies}\n  renew: {} · owner: {}\n", item.name, entry.state, item.basis, if entry.resolved { " (resolved by native clock)" } else { "" }, item.detail, item.renew, item.owner)
                }).collect::<Vec<_>>().join("\n")),
                _ => None,
            })
        }

        Command::Backup { command } => match command.unwrap_or(BackupCmd::Status) {
            BackupCmd::Status => {
                let payload = client.send(Request::Backup).await?;
                print(&payload, cli.json, backup_status_text)
            }
            BackupCmd::List => {
                let payload = client.send(Request::Backup).await?;
                print(&payload, cli.json, backup_list_text)
            }
            BackupCmd::Run => {
                let payload = client.send(Request::BackupRun).await?;
                print(&payload, cli.json, backup_run_text)
            }
            BackupCmd::Verify { snapshot, identity } => {
                let identity = identity.as_deref().map(absolute);
                let payload = client.send(Request::BackupVerify { snapshot, identity }).await?;
                print(&payload, cli.json, backup_verify_text)?;
                match payload {
                    Payload::BackupVerify { verification } if !verification.ok => {
                        Err(anyhow!("verification of {} failed", verification.snapshot))
                    }
                    _ => Ok(()),
                }
            }
            BackupCmd::Restore { snapshot, into, identity } => {
                let into = absolute(&into);
                let identity = identity.as_deref().map(absolute);
                let payload = client.send(Request::BackupRestore { snapshot, into, identity }).await?;
                print(&payload, cli.json, backup_restore_text)
            }
        },

        Command::Adapters => {
            let payload = client.send(Request::Adapters).await?;
            print(&payload, cli.json, |p| match p {
                Payload::Adapters { adapters } => {
                    let mut out = String::new();
                    let mut kind = "";
                    for a in adapters {
                        if a.kind != kind {
                            kind = &a.kind;
                            out.push_str(&format!("\n{}\n", kind.to_uppercase()));
                        }
                        out.push_str(&format!(
                            "  {:<14} {:<46} {}\n",
                            a.name, a.description, a.source
                        ));
                    }
                    Some(out.trim_start_matches('\n').trim_end().to_string())
                }
                _ => None,
            })
        }

        Command::Watch => {
            eprintln!("watching {} -- ctrl-c to stop", client.endpoint());
            client
                .subscribe(|msg| {
                    match msg {
                        Response::Ok {
                            data: Payload::Event { event },
                        } => {
                            if cli.json {
                                println!("{}", serde_json::to_string(&event).unwrap_or_default());
                            } else {
                                println!("{}", describe_event(&event));
                            }
                        }
                        Response::Error { message, .. } => eprintln!("! {message}"),
                        _ => {}
                    }
                    true
                })
                .await
        }

        Command::Agents => {
            let payload = client.send(Request::Agents).await?;
            print(&payload, cli.json, |p| match p {
                Payload::Scopes {
                    scopes,
                    roles,
                    scope_roles,
                    ..
                } => {
                    let mut out = String::new();
                    for s in scopes {
                        out.push_str(&format!(
                            "{}  {}\n              id {} · default {} on {}\n",
                            s.name, s.path, s.id, s.default_agent, s.runtime
                        ));
                        for a in &s.agents {
                            let mut flags = vec![
                                match &a.assigned_role {
                                    Some(r) => format!("{r}, given"),
                                    None => a.role.clone(),
                                },
                                a.lifetime.clone(),
                            ];
                            if a.is_default {
                                flags.push("default".into());
                            }
                            if a.autostart {
                                flags.push("autostart".into());
                            }
                            if !a.declared {
                                flags.push("undeclared".into());
                            }
                            out.push_str(&format!(
                                "  {:<20} {:<12} [{}]\n",
                                a.name,
                                a.state,
                                flags.join(", ")
                            ));
                            if let Some(cmd) = &a.attach {
                                out.push_str(&format!("      attach: {cmd}\n"));
                            }
                            if let Some(err) = &a.error {
                                out.push_str(&format!("      error:  {err}\n"));
                            }
                            for w in &a.active {
                                out.push_str(&format!(
                                    "      {} attempt {} ({}) -- {}\n",
                                    w.status, w.attempt, w.trigger, w.task_title
                                ));
                            }
                        }
                        out.push('\n');
                    }
                    // What there is to give an agent, so nobody has to guess a
                    // name and be told no: the roles every scope has, then the
                    // ones a scope has only because it, or a scope above it,
                    // defines them.
                    for r in roles {
                        out.push_str(&format!(
                            "role {:<14} {:<6} {}\n",
                            r.name, r.reach, r.describe
                        ));
                    }
                    for (scope, scoped) in scope_roles {
                        for r in scoped.iter().filter(|r| {
                            matches!(r.origin, factory_core::role::RoleOrigin::Scope { .. })
                        }) {
                            let from = match &r.origin {
                                factory_core::role::RoleOrigin::Scope { scope: from } => from.as_str(),
                                _ => "",
                            };
                            out.push_str(&format!(
                                "role {:<14} {:<6} {}  (in {scope}, from {from})\n",
                                r.name, r.reach, r.describe
                            ));
                        }
                    }
                    Some(out.trim_end().to_string())
                }
                _ => None,
            })
        }

        Command::Agent(cmd) => agent_cmd(cli.json, &client, cmd).await,
        Command::Run(cmd) => run_cmd(cli.json, &client, cmd).await,
        Command::Task(cmd) => task(cli.json, &client, cmd).await,
        Command::Workflow(cmd) => workflow_cmd(cli.json, &client, cmd).await,
        Command::Dependencies { args } => {
            let request = match args.as_slice() {
                [scope] => Request::Dependencies { scope: scope.clone() },
                [verb, scope] if verb == "vex" => Request::DependenciesVex { scope: scope.clone() },
                _ => return Err(anyhow!("use `factory dependencies <scope>` or `factory dependencies vex <scope>`")),
            };
            let payload = client.send(request).await?;
            print(&payload, cli.json, |p| match p {
                Payload::Dependencies { report } => Some(dependencies_text(report)),
                Payload::Text { text } => Some(text.clone()),
                _ => None,
            })
        }

        Command::Knowledge { command: None } => {
            let payload = client.send(Request::Knowledge).await?;
            print(&payload, cli.json, |p| match p {
                Payload::Knowledge { root, present, legacy, pages, tags, documents, gaps, findings } => {
                    if !*present {
                        return Some(match legacy {
                            Some(old) => format!(
                                "no vault at {root}\n\nfound the old wiki at {old} -- bring it in with:\n  factory knowledge import {old}"
                            ),
                            None => format!("no vault at {root}\n\nstart one with:\n  factory knowledge import <dir>"),
                        });
                    }
                    let total_links: usize = pages.iter().map(|p| p.links.len()).sum();
                    let mut out = format!(
                        "{root}  ({} pages, {total_links} links, {} tags, {} documents, {} gaps)\n",
                        pages.len(),
                        tags.len(),
                        documents.len(),
                        gaps.len(),
                    );
                    if !pages.is_empty() {
                        out.push_str("\nPAGES\n");
                        for p in pages {
                            out.push_str(&format!(
                                "  {:<28} {:<28} area={:<12} status={:<10} sources={:<3} links={:<3} backlinks={:<3} tags={:<3} documents={}\n",
                                p.id,
                                p.title,
                                p.area.as_deref().unwrap_or("-"),
                                p.status.as_deref().unwrap_or("-"),
                                p.sources,
                                p.links.len(),
                                p.backlinks.len(),
                                p.tags.len(),
                                p.documents.len(),
                            ));
                        }
                    }
                    if !tags.is_empty() {
                        out.push_str("\nTAGS\n");
                        for t in tags {
                            out.push_str(&format!("  #{:<24} {} page(s)\n", t.name, t.pages.len()));
                        }
                    }
                    if !documents.is_empty() {
                        out.push_str("\nDOCUMENTS\n");
                        for d in documents {
                            out.push_str(&format!(
                                "  {:<40} {:<6} {:>10} bytes  referenced by {}\n",
                                d.id,
                                d.ext,
                                d.bytes,
                                d.referenced_by.len()
                            ));
                        }
                    }
                    if !gaps.is_empty() {
                        out.push_str("\nGAPS\n");
                        for g in gaps {
                            out.push_str(&format!(
                                "  {:<28} {} pointer(s): {}\n",
                                g.target,
                                g.from.len(),
                                g.from.join(", ")
                            ));
                        }
                    }
                    if !findings.is_empty() {
                        out.push_str("\nFINDINGS\n");
                        for f in findings {
                            out.push_str(&format!(
                                "  {:<22} {:<28} {}\n",
                                finding_kind_str(&f.kind),
                                f.note,
                                f.detail
                            ));
                        }
                    }
                    Some(out.trim_end().to_string())
                }
                _ => None,
            })
        }

        Command::Knowledge { command: Some(cmd) } => knowledge_cmd(cli.json, &client, cmd).await,

        Command::Bench { cmd: None } => {
            let payload = client.send(Request::Benchmarks).await?;
            print(&payload, cli.json, |p| match p {
                Payload::Benchmarks { configurations } => {
                    let pinned = configurations.iter().filter(|c| c.pinned).count();
                    let mut out =
                        format!("{} configuration(s), {pinned} pinned\n", configurations.len());
                    for c in configurations {
                        out.push_str(&format!(
                            "\n{}  model={}{}  sandbox={}  [{}]\n",
                            c.harness,
                            c.model.as_deref().unwrap_or("harness default, not recorded"),
                            c.model_source
                                .as_deref()
                                .map(|s| format!(" ({s})"))
                                .unwrap_or_default(),
                            c.sandbox,
                            if c.pinned { "pinned" } else { "unpinned" },
                        ));
                        if !c.flags.is_empty() {
                            out.push_str(&format!("  flags: {}\n", c.flags.join(", ")));
                        }
                        for a in &c.agents {
                            out.push_str(&format!(
                                "  agent: {}/{}  ({}{})\n",
                                a.scope,
                                a.agent,
                                a.lifetime,
                                if a.declared { "" } else { ", synthesized" },
                            ));
                        }
                        out.push_str(&format!("  missing: {}\n", c.missing.join(", ")));
                    }
                    Some(out.trim_end().to_string())
                }
                _ => None,
            })
        }
        Command::Bench { cmd: Some(cmd) } => bench_cmd(cli.json, &client, cmd).await,
        Command::Dataset(cmd) => dataset_cmd(cli.json, &client, cmd).await,

        Command::Policy { scope, framework, command } => {
            // `--scope`/`--framework` before the subcommand name (or with
            // none at all) are the same flags `status` itself takes after
            // it -- clap hands each occurrence to whichever level asked for
            // it, so `factory policy --scope S status` must not let the
            // leading one quietly vanish under `status`'s own (absent) one.
            let cmd = match command {
                None => PolicyCmd::Status { scope, framework },
                Some(PolicyCmd::Status { scope: s, framework: f }) => {
                    PolicyCmd::Status { scope: s.or(scope), framework: f.or(framework) }
                }
                // `export` takes its own `--scope` too (no `--framework` --
                // an export is not narrowed by one), so the leading flag
                // merges the same way `status`'s does.
                Some(PolicyCmd::Export { scope: s, format }) if framework.is_none() => {
                    PolicyCmd::Export { scope: s.or(scope), format }
                }
                Some(_) if scope.is_some() || framework.is_some() => {
                    return Err(anyhow!(
                        "--scope/--framework before the subcommand only apply to `status`/`export`; \
                         repeat them after the subcommand name if it takes its own"
                    ));
                }
                Some(other) => other,
            };
            policy_cmd(cli.json, &client, cmd).await
        }

        Command::Metrics { ids, scope, window } => {
            let ids: std::result::Result<Vec<MetricId>, String> = ids.into_iter().map(|s| s.parse()).collect();
            let ids = ids.map_err(|e| anyhow!(e))?;
            let payload = client.send(Request::Metrics { ids, scope, window }).await?;
            print(&payload, cli.json, |p| match p {
                Payload::Metrics { values, series, registry } => Some(metrics_text(values, series, registry)),
                _ => None,
            })
        }

        Command::Dashboard { scope } => {
            let payload = client.send(Request::Dashboard { scope }).await?;
            print(&payload, cli.json, |p| match p {
                Payload::Dashboard { tiles, source } => Some(dashboard_text(tiles, source)),
                _ => None,
            })
        }

        Command::Cost { by, since, until, scope } => {
            let group_by: factory_core::usage::CostGroupBy = by.parse().map_err(|e: String| anyhow!(e))?;
            let now = chrono::Utc::now();
            let from = since.as_deref().map(|t| parse_when(t, now)).transpose()?;
            let to = until.as_deref().map(|t| parse_when(t, now)).transpose()?;
            let payload = client.send(Request::Costs { group_by, from, to, scope }).await?;
            print(&payload, cli.json, |p| match p {
                Payload::Costs { report } => Some(costs_text(report)),
                _ => None,
            })
        }

        Command::Budget { scope, by } => {
            let group_by = by.parse().map_err(|e: String| anyhow!(e))?;
            let payload = client.send(Request::Budget { scope, group_by }).await?;
            print(&payload, cli.json, |p| match p {
                Payload::Budget { report } => Some(budget_text(report)),
                _ => None,
            })
        }
        Command::Goals { scope, cycle, command } => {
            // `--scope`/`--cycle` before the subcommand name (or with none
            // at all) are the same flags `status` itself takes after it --
            // clap hands each occurrence to whichever level asked for it,
            // the same merge `Command::Policy` does above.
            let cmd = match command {
                None => GoalsCmd::Status { scope, cycle },
                Some(GoalsCmd::Status { scope: s, cycle: c }) => GoalsCmd::Status { scope: s.or(scope), cycle: c.or(cycle) },
                Some(_) if scope.is_some() || cycle.is_some() => {
                    return Err(anyhow!(
                        "--scope/--cycle before the subcommand only apply to `status`; repeat \
                         them after the subcommand name if it takes its own"
                    ));
                }
                Some(other) => other,
            };
            goals_cmd(cli.json, &client, cmd).await
        }

        Command::Scenario { scope, command } => {
            // `--scope` before the subcommand name (or with none at all) is
            // the same flag `list` itself takes after it -- the same merge
            // `Command::Policy`/`Command::Goals` do above.
            let cmd = match command {
                None => ScenarioCmd::List { scope },
                Some(ScenarioCmd::List { scope: s }) => ScenarioCmd::List { scope: s.or(scope) },
                Some(_) if scope.is_some() => {
                    return Err(anyhow!(
                        "--scope before the subcommand only applies to `list`; repeat it after \
                         the subcommand name if it takes its own"
                    ));
                }
                Some(other) => other,
            };
            scenario_cmd(cli.json, &client, cmd).await
        }

        Command::Quality { scope, command } => {
            // `--scope` before the subcommand name (or with none at all) is
            // the same flag `status`/`findings` take after it -- the merge
            // `Command::Scenario` does above.
            let cmd = match command {
                None => QualityCmd::Status { scope },
                Some(QualityCmd::Status { scope: s }) => QualityCmd::Status { scope: s.or(scope) },
                Some(QualityCmd::Findings { scope: s }) => QualityCmd::Findings { scope: s.or(scope) },
                Some(_) if scope.is_some() => {
                    return Err(anyhow!(
                        "--scope before the subcommand only applies to `status` and `findings`; repeat \
                         it after the subcommand name if it takes its own"
                    ));
                }
                Some(other) => other,
            };
            quality_cmd(cli.json, &client, cmd).await
        }

        Command::Stats { scope, window, command } => {
            // The same merge `Command::Scenario` does: flags before the
            // subcommand name count for it too.
            let cmd = match command {
                None => StatsCmd::Summary { scope, window },
                Some(StatsCmd::Summary { scope: s, window: w }) => StatsCmd::Summary { scope: s.or(scope), window: w.or(window) },
                Some(StatsCmd::Attention { scope: s }) => StatsCmd::Attention { scope: s.or(scope) },
            };
            stats_cmd(cli.json, &client, cmd).await
        }

        Command::Intake { scope, command } => {
            // The same merge `Command::Scenario` does: `--scope` before the
            // subcommand name counts for `list`.
            let cmd = match command {
                None => IntakeCmd::List { scope },
                Some(IntakeCmd::List { scope: s }) => IntakeCmd::List { scope: s.or(scope) },
                Some(IntakeCmd::Add {
                    scope: s,
                    title,
                    instructions,
                    source,
                    reference,
                    provider,
                    requester,
                    received_at,
                    labels,
                    security,
                }) => IntakeCmd::Add {
                    scope: s.or(scope),
                    title,
                    instructions,
                    source,
                    reference,
                    provider,
                    requester,
                    received_at,
                    labels,
                    security,
                },
                Some(IntakeCmd::SecurityReports { scope: s }) => IntakeCmd::SecurityReports { scope: s.or(scope) },
                Some(_) if scope.is_some() => {
                    return Err(anyhow!(
                        "--scope before the subcommand only applies to `list`, `add` and `security-reports`; \
                         the others name an item"
                    ));
                }
                Some(other) => other,
            };
            intake_cmd(cli.json, &client, cmd).await
        }
    }
}

async fn intake_cmd(json: bool, client: &Client, cmd: IntakeCmd) -> Result<()> {
    use factory_core::intake::{Decision, NewIntake, SecurityVerdict, SourceKind, WontfixReason};
    let payload = match cmd {
        IntakeCmd::List { scope } => client.send(Request::IntakeBoard { scope }).await?,
        IntakeCmd::Add { title, instructions, scope, source, reference, provider, requester, received_at, labels, security } => {
            let source = match source.as_deref() {
                Some("email") => SourceKind::Email,
                Some("chat") => SourceKind::Chat,
                _ => SourceKind::Cli,
            };
            let received_at = match received_at {
                Some(t) => Some(
                    chrono::DateTime::parse_from_rfc3339(t.trim())
                        .map(|t| t.with_timezone(&chrono::Utc))
                        .map_err(|e| anyhow!("--received-at {t:?} is not RFC 3339: {e}"))?,
                ),
                None => None,
            };
            client
                .send(Request::IntakeAdd(NewIntake {
                    title,
                    instructions,
                    scope,
                    source: Some(source),
                    reference,
                    provider,
                    requester,
                    received_at,
                    labels: parse_labels(&labels)?,
                    security,
                }))
                .await?
        }
        IntakeCmd::Show { id } => client.send(Request::TaskGet { id }).await?,
        IntakeCmd::Triage { id, agent } => client.send(Request::IntakeTriage { id, agent }).await?,
        IntakeCmd::Assess { id, file, decide } => {
            let text = read_file_or_stdin(&file)?;
            let assessment = serde_json::from_str(&text).map_err(|e| anyhow!("not an assessment: {e}"))?;
            client.send(Request::IntakeAssess { id, assessment, decide }).await?
        }
        IntakeCmd::Decide { id, decision, file, run, questions, reason, evidence, duplicate_of } => {
            if file.is_some() && decision != "split" {
                return Err(anyhow!("--file only goes with split"));
            }
            let decision = match decision.as_str() {
                "ready" => Decision::Ready { run },
                "needs-info" => Decision::NeedsInfo { questions },
                "split" => Decision::Split {
                    parts: match file {
                        Some(file) => serde_json::from_str(&read_file_or_stdin(&file)?)
                            .map_err(|e| anyhow!("not a list of parts: {e}"))?,
                        None => Vec::new(),
                    },
                },
                _ => Decision::Wontfix {
                    reason: match reason.as_deref() {
                        Some("duplicate") => WontfixReason::Duplicate,
                        Some("invalid") => WontfixReason::Invalid,
                        Some("out-of-scope") => WontfixReason::OutOfScope,
                        _ => return Err(anyhow!("wontfix needs --reason duplicate|invalid|out-of-scope")),
                    },
                    evidence: evidence.unwrap_or_default(),
                    duplicate_of,
                },
            };
            client.send(Request::IntakeDecide { id, decision }).await?
        }
        IntakeCmd::Publish { id } => client.send(Request::IntakePublish { id }).await?,
        IntakeCmd::Info { id, text, triage, agent } => {
            let answered = client.send(Request::IntakeInfo { id: id.clone(), text }).await?;
            if triage {
                client.send(Request::IntakeTriage { id, agent }).await?
            } else {
                answered
            }
        }
        IntakeCmd::FlagSecurity { id, reason } => client.send(Request::IntakeFlagSecurity { id, reason }).await?,
        IntakeCmd::Security { id, verdict, evidence } => {
            let verdict = match verdict.as_str() {
                "confirm" => SecurityVerdict::Confirm,
                _ => SecurityVerdict::Dismiss,
            };
            client.send(Request::IntakeSecurity { id, verdict, evidence: evidence.unwrap_or_default() }).await?
        }
        IntakeCmd::SecurityReports { scope } => client.send(Request::IntakeSecurityReports { scope }).await?,
    };
    print(&payload, json, |p| match p {
        Payload::IntakeBoard { board } => Some(intake_board_text(board)),
        Payload::Task { task } => Some(intake_item_text(task)),
        Payload::IntakeSecurityReports { reports } => Some(intake_security_reports_text(reports)),
        _ => None,
    })
}

/// A card's line: full id (it is what every other `intake` command takes),
/// age, priority/category/estimate once assessed, the seven axes as a row
/// of marks, the title.
fn read_file_or_stdin(file: &std::path::Path) -> Result<String> {
    if file.as_os_str() == "-" {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
        Ok(text)
    } else {
        std::fs::read_to_string(file).map_err(|e| anyhow!("cannot read {}: {e}", file.display()))
    }
}

/// What would move a held-back item, one line per action with its reasons
/// under it -- and the command that does it.
fn next_actions_text(id: &str, actions: &[factory_core::intake::NextAction], indent: &str) -> String {
    use factory_core::intake::NextActionKind;
    let mut out = String::new();
    for a in actions {
        let command = match a.action {
            NextActionKind::CloseDuplicate => format!(
                "factory intake decide {id} wontfix --reason duplicate --duplicate-of {} --evidence \"...\"",
                a.reference.as_deref().unwrap_or("<ref>")
            ),
            NextActionKind::Split => format!("factory intake decide {id} split [--file parts.json]"),
            NextActionKind::AddInfo => format!("factory intake info {id} \"...\" --triage"),
        };
        out.push_str(&format!("{indent}-> {}: {}\n{indent}   {command}\n", a.action.as_str().replace('_', "-"), a.hint));
        for r in &a.reasons {
            out.push_str(&format!("{indent}   because {r}\n"));
        }
    }
    out
}

/// `email/apple-mail (<abc@x>) relayed by the owner` -- what a card and
/// `intake show` print for where an item came from (`#167`): the kind, its
/// provider when there is one, the reference, and who relayed it when that
/// was not the source itself.
fn intake_source_text(source: &factory_core::intake::IntakeSource) -> String {
    let mut text = source.kind.as_str().to_string();
    if let Some(p) = &source.provider {
        text.push('/');
        text.push_str(p);
    }
    if let Some(r) = &source.reference {
        text.push_str(&format!(" ({r})"));
    }
    if let Some(by) = &source.relayed_by {
        text.push_str(&format!(" relayed by {by}"));
    }
    text
}

fn intake_card_line(c: &factory_core::intake::IntakeCard) -> String {
    let (verdict, marks) = match &c.triage {
        Some(t) => (
            format!(
                "{} {} {}",
                t.priority.as_str(),
                t.assessment.category,
                t.estimate.map(|e| e.describe()).unwrap_or_else(|| "-".into())
            ),
            t.assessment.axes.iter().map(|a| if a.pass { '+' } else { 'x' }).collect::<String>(),
        ),
        None => ("-".into(), ".......".into()),
    };
    // A possible or confirmed security report's own fast-lane mark
    // (`#170`) -- `board()` already sorted it first; this is only the
    // visible reason why.
    let sec_mark = match c.security.as_ref().map(|f| f.state) {
        Some(factory_core::intake::SecurityState::Possible) => "!P",
        Some(factory_core::intake::SecurityState::Confirmed) => "!C",
        _ => "  ",
    };
    let mut line = format!(
        "{sec_mark}{}  {:>7}  {}  {:<24} {} [{}] from {} ({})",
        c.id,
        duration(c.age_seconds.max(0) as u64),
        marks,
        verdict,
        c.title,
        c.scope,
        c.requester,
        intake_source_text(&c.source),
    );
    if let Some(f) = &c.security {
        line.push_str(&format!(
            "\n      security: {} ({})",
            f.state.as_str(),
            if f.reason.trim().is_empty() { "no reason given" } else { f.reason.trim() }
        ));
    }
    if c.stage == factory_core::intake::IntakeStage::Triaging && c.triage.is_none() {
        if let Some(t) = &c.triage_task {
            line.push_str(&format!("\n      triage run in task {t}"));
        }
    }
    if c.stage == factory_core::intake::IntakeStage::Triaging
        && c.triage_task_ended
        && c.triage.is_none()
    {
        line.push_str("\n      the triage run ended without an assessment");
    }
    if let Some(parent) = &c.parent {
        line.push_str(&format!("\n      part of {parent}"));
    }
    for q in &c.questions {
        line.push_str(&format!("\n      ? {q}"));
    }
    let next = next_actions_text(&c.id, &c.next_actions, "      ");
    if !next.is_empty() {
        line.push('\n');
        line.push_str(next.trim_end());
    }
    line
}

fn intake_board_text(board: &factory_core::intake::IntakeBoard) -> String {
    let columns = [
        ("RECEIVED", &board.columns.received),
        ("TRIAGING", &board.columns.triaging),
        ("NEEDS INFO", &board.columns.needs_info),
        ("READY (released, last 14 days)", &board.columns.ready),
    ];
    let mut out = String::new();
    for (name, cards) in columns {
        out.push_str(&format!("{name}  {}\n", cards.len()));
        for c in cards.iter() {
            out.push_str(&intake_card_line(c));
            out.push('\n');
        }
    }
    out.push_str(&format!(
        "split in the last {} days: {}\nclosed as wontfix in the last {0} days: {}\naxes: {}",
        board.ready_window_days,
        board.split,
        board.wontfix,
        board.axes.iter().map(|a| a.label.as_str()).collect::<Vec<_>>().join(", ")
    ));
    let definitions = intake_definitions_text(&board.routes);
    if !definitions.is_empty() {
        out.push_str("\ndefinitions of ready (#169):\n");
        out.push_str(&definitions);
    }
    out
}

/// Each route's effective definition of ready and its findings (`#169`),
/// for `factory intake` / `factory intake list --scope`. Silent for a scope
/// whose chain adds nothing and reads cleanly -- the common case, and
/// exactly today's seven axes.
fn intake_definitions_text(routes: &[factory_core::intake::RouteOptions]) -> String {
    let mut out = String::new();
    for r in routes {
        let d = &r.definition;
        let default = d.checks.is_empty()
            && d.max_complexity == factory_core::ready::DEFAULT_MAX_COMPLEXITY
            && d.observability_tolerance == factory_core::ready::Tolerance::default()
            && d.unreadable.is_empty()
            && r.findings.is_empty();
        if default {
            continue;
        }
        out.push_str(&format!("  scope `{}`\n", r.scope));
        if !d.unreadable.is_empty() {
            for line in &d.unreadable {
                out.push_str(&format!("    ! {line}\n"));
            }
        }
        for c in &d.checks {
            let categories = if c.categories.is_empty() { "every category".to_string() } else { c.categories.join(", ") };
            out.push_str(&format!("    check `{}` ({categories}) -- {}\n", c.id, c.pass_condition));
        }
        if d.max_complexity != factory_core::ready::DEFAULT_MAX_COMPLEXITY {
            out.push_str(&format!("    max_complexity: {}\n", d.max_complexity));
        }
        if d.observability_tolerance != factory_core::ready::Tolerance::default() {
            out.push_str(&format!("    observability_tolerance: {}\n", d.observability_tolerance.as_str()));
        }
        // `Unreadable` findings are left out here: `d.unreadable` above
        // already said the same thing as the blocker sentence that gates
        // every assessment for this scope, and printing both reads as the
        // same fact twice.
        for f in r.findings.iter().filter(|f| f.kind != factory_core::ready::FindingKind::Unreadable) {
            out.push_str(&format!("    finding [{}] {:?}: {}\n", f.subject, f.kind, f.detail));
        }
    }
    out
}

/// `factory intake security-reports` -- every confirmed security report,
/// awareness time first (`#170`): the L4 fact `#157`'s reporting clock will
/// read.
fn intake_security_reports_text(reports: &[factory_core::intake::ConfirmedSecurityReport]) -> String {
    if reports.is_empty() {
        return "no confirmed security reports".into();
    }
    let mut out = String::new();
    for r in reports {
        out.push_str(&format!(
            "  {}  {}  aware since {}  confirmed by {} at {}  from {}{}\n",
            r.item,
            r.scope,
            r.awareness_at.to_rfc3339(),
            r.confirmed_by,
            r.confirmed_at.to_rfc3339(),
            r.source.kind.as_str(),
            r.source.reference.as_ref().map(|s| format!(" ({s})")).unwrap_or_default(),
        ));
    }
    out
}

fn intake_item_text(task: &Task) -> String {
    let Some(i) = &task.intake else { return detail(task) };
    let mut out = format!(
        "{}\n  {}\n  status     {} ({})\n  scope      {}\n  from       {} via {}\n  received   {}\n",
        task.id,
        task.title,
        task.status.as_str(),
        i.stage.as_str().replace('_', "-"),
        task.scope,
        i.requester,
        intake_source_text(&i.source),
        i.received_at.to_rfc3339(),
    );
    if let Some(f) = &i.security {
        out.push_str(&format!(
            "  security   {} -- flagged by {} at {}: {}\n",
            f.state.as_str(),
            f.flagged_by,
            f.flagged_at.to_rfc3339(),
            if f.reason.trim().is_empty() { "no reason given" } else { f.reason.trim() },
        ));
        if let (Some(by), Some(at)) = (&f.decided_by, f.decided_at) {
            out.push_str(&format!(
                "             decided by {by} at {}{}\n",
                at.to_rfc3339(),
                f.evidence.as_ref().map(|e| format!(": {e}")).unwrap_or_default(),
            ));
        }
    }
    if let Some(t) = &i.triage_task {
        out.push_str(&format!("  triage run task {t}\n"));
    }
    if let Some(t) = &i.triage {
        out.push_str(&format!(
            "  assessed   by {} at {}: {}\n  category   {}\n  priority   {} (impact {}, urgency {})\n  estimate   {} (complexity {}){}\n  route      {}{}{}{}{}\n",
            t.by,
            t.at.to_rfc3339(),
            t.verdict.as_str().replace('_', "-"),
            t.assessment.category,
            t.priority.as_str(),
            t.assessment.impact.as_str(),
            t.assessment.urgency.as_str(),
            t.estimate.map(|e| e.describe()).unwrap_or_else(|| "none".into()),
            t.assessment.complexity,
            t.estimate_basis.as_ref().map(|b| format!("\n  basis      {}", b.describe())).unwrap_or_default(),
            t.assessment.routing.scope,
            t.assessment.routing.agent.as_ref().map(|a| format!(" as {a}")).unwrap_or_default(),
            t.assessment.routing.workflow.as_ref().map(|w| format!(", workflow {w}")).unwrap_or_default(),
            if t.assessment.routing.inputs.is_empty() {
                String::new()
            } else {
                let inputs: Vec<String> = t.assessment.routing.inputs.iter().map(|(k, v)| format!("{k}={v}")).collect();
                format!(" with {}", inputs.join(", "))
            },
            if t.assessment.routing.agents.is_empty() {
                String::new()
            } else {
                let agents: Vec<String> =
                    t.assessment.routing.agents.iter().map(|(k, v)| format!("{k} by {v}")).collect();
                format!("; {}", agents.join(", "))
            },
        ));
        for a in &t.assessment.axes {
            out.push_str(&format!(
                "    {} {:<14} {}{}\n",
                if a.pass { "+" } else { "x" },
                a.axis.as_str(),
                a.evidence,
                a.cost.map(|c| format!(" [cost {c:?}]").to_lowercase()).unwrap_or_default()
            ));
        }
        if !t.assessment.summary.is_empty() {
            out.push_str(&format!("  summary    {}\n", t.assessment.summary));
        }
        if !t.assessment.split.is_empty() {
            let executable = t
                .assessment
                .split
                .iter()
                .any(|p| !p.owns.is_empty() || p.estimate_seconds.is_some());
            out.push_str(&format!(
                "  {} into {} parts:\n",
                if executable { "executable plan" } else { "proposed split" },
                t.assessment.split.len()
            ));
            for p in &t.assessment.split {
                let after = if p.depends_on.is_empty() {
                    String::new()
                } else {
                    format!(" (after {})", p.depends_on.join(", "))
                };
                out.push_str(&format!("    {:<14} {}{after}\n", p.id, p.title));
            }
        }
    }
    let candidates = factory_core::intake::candidates_with_verdicts(i);
    if !candidates.is_empty() {
        out.push_str("  possible duplicates:\n");
        for c in &candidates {
            let matched = match c.matched {
                factory_core::intake::DuplicateMatch::Source => "source".to_string(),
                factory_core::intake::DuplicateMatch::Text => format!("text {}%", c.score.unwrap_or(0)),
            };
            out.push_str(&format!(
                "    {:<10} {:<12} {} ({}) [{matched}] {}\n",
                c.verdict.as_str(),
                c.kind.as_str(),
                c.reference,
                c.title,
                c.evidence,
            ));
        }
    }
    for q in &i.questions {
        out.push_str(&format!("  ? {q}\n"));
    }
    // `show` fetches one task, not the board, so it has no per-scope
    // definition of ready (`#169`) to hold this against; the seven built-in
    // axes' own next actions still show correctly, and the scope-aware ones
    // (`factory intake` / `factory intake list --scope`, which read
    // `board.routes`) are authoritative for a scope's extra checks and caps.
    let next = next_actions_text(
        &task.id,
        &factory_core::intake::next_actions(i, &factory_core::ready::ReadyDefinition::default()),
        "  ",
    );
    if !next.is_empty() {
        out.push_str("  what moves it:\n");
        out.push_str(&next);
    }
    if let Some(d) = &i.decision {
        out.push_str(&format!("  decided    {} by {} at {}", d.decision.as_str().replace('_', "-"), d.by, d.at.to_rfc3339()));
        if let Some(run) = &d.workflow_run {
            out.push_str(&format!(" (workflow run {run})"));
        }
        if !d.parts.is_empty() {
            out.push_str(&format!(" into {}", d.parts.join(", ")));
        }
        out.push('\n');
    }
    if let Some(o) = &i.outbound {
        out.push_str(&format!("  outbound   {}", o.state.as_str()));
        if let Some(url) = &o.comment_url {
            out.push_str(&format!(" -- {url}"));
        }
        out.push('\n');
        if !o.labels_skipped.is_empty() {
            out.push_str(&format!("             labels skipped (missing in the repository): {}\n", o.labels_skipped.join(", ")));
        }
        if let Some(error) = &o.last_error {
            out.push_str(&format!("             error: {error}\n"));
        }
    }
    if let Some(r) = &task.result {
        out.push_str(&format!("  result     {r}\n"));
    }
    // The item as handed in, and anything added to it since.
    if !task.instructions.trim().is_empty() {
        out.push_str(&format!("\n{}\n", task.instructions.trim()));
    }
    out.trim_end().to_string()
}

/// `factory infra`, for a person: the host, the daemon on it, then each
/// declared provider with the agents it serves, then the model agents none
/// claims. A fact the daemon could not read prints as `--`.
fn parse_power_mode(s: &str) -> std::result::Result<factory_core::protocol::PowerMode, String> {
    s.parse()
}

/// `factory power-mode`: the mode per source, what is offered, and -- until
/// the daemon may change it -- the rule that would let it.
fn power_mode_text(payload: &Payload) -> Option<String> {
    let Payload::HostPowerMode { report } = payload else { return None };
    let mut out = String::new();
    if !report.applicable {
        out.push_str("power mode: not applicable (this host is not macOS)\n");
        return Some(out.trim_end().to_string());
    }
    if report.supported.is_empty() {
        out.push_str("power mode: not supported (pmset -g cap lists neither lowpowermode nor highpowermode)\n");
    }
    let shown = |m: Option<factory_core::protocol::PowerMode>| m.map(|m| m.label()).unwrap_or("--");
    match (report.ac, report.battery) {
        (a, b) if b.is_some() && a != b => {
            out.push_str(&format!("power mode: mixed\n  AC          {}\n  battery     {}\n", shown(a), shown(b)))
        }
        (a, b) => out.push_str(&format!("power mode: {}\n", shown(a.or(b)))),
    }
    if !report.supported.is_empty() {
        let offered: Vec<&str> = report.supported.iter().map(|m| m.label()).collect();
        out.push_str(&format!("  offered     {}\n", offered.join(", ")));
        if report.can_change {
            out.push_str("  change      factory power-mode automatic|high-performance|energy-saving\n");
        } else {
            out.push_str(&format!(
                "  read-only   Factory may not change it until {} holds:\n    {}\n  install     {}\n  check       {}\n",
                report.sudoers.path, report.sudoers.rule, report.sudoers.install, report.sudoers.check
            ));
        }
    }
    for note in &report.notes {
        out.push_str(&format!("  note        {note}\n"));
    }
    if let Some(last) = report.changes.last() {
        out.push_str(&format!("  last change {} · {}\n", last.at.format("%Y-%m-%d %H:%M"), last.message));
    }
    Some(out.trim_end().to_string())
}

fn infrastructure_text(payload: &Payload) -> Option<String> {
    let Payload::Infrastructure { host, daemon, providers, unassigned, harnesses } = payload else {
        return None;
    };
    const UNKNOWN: &str = "--";
    let or = |v: Option<String>| v.unwrap_or_else(|| UNKNOWN.to_string());
    let mut out = String::new();

    out.push_str(&format!("HOST  {}\n", or(host.hostname.clone())));
    out.push_str(&format!("  model       {}\n", or(host.model.clone())));
    out.push_str(&format!("  chip        {}\n", or(host.chip.clone())));
    out.push_str(&format!("  cores       {}\n", or(host.cores.map(|c| c.to_string()))));
    out.push_str(&format!("  memory      {}\n", or(host.memory_bytes.map(bytes))));
    out.push_str(&format!(
        "  os          {} ({})\n",
        or(host.os.clone()),
        or(host.arch.clone())
    ));
    out.push_str(&format!("  uptime      {}\n", or(host.uptime_seconds.map(duration))));
    out.push_str(&format!(
        "  load        {}\n",
        or(host.load.map(|l| format!("{:.2} {:.2} {:.2}", l[0], l[1], l[2])))
    ));
    out.push_str(&format!(
        "  disk        {}\n",
        or(host.disk.as_ref().map(|d| {
            let used = d.total_bytes.saturating_sub(d.free_bytes);
            let percent = if d.total_bytes == 0 { 0.0 } else { used as f64 * 100.0 / d.total_bytes as f64 };
            format!(
                "{}  {} free of {} ({percent:.0}% used)",
                d.mount,
                bytes(d.free_bytes),
                bytes(d.total_bytes)
            )
        }))
    ));

    let uptime = (chrono::Utc::now() - daemon.started_at).num_seconds().max(0) as u64;
    out.push_str(&format!("\nDAEMON  factory {}  (pid {})\n", daemon.version, daemon.pid));
    out.push_str(&format!(
        "  up          {}, since {}\n",
        duration(uptime),
        daemon.started_at.format("%Y-%m-%d %H:%M:%S UTC")
    ));
    out.push_str(&format!("  root        {}\n", daemon.root));
    out.push_str(&format!(
        "  store       {}  {}  {}\n",
        daemon.store.kind,
        daemon.store.path,
        or(daemon.store.size_bytes.map(bytes))
    ));
    out.push_str(&format!("  socket      {}\n", daemon.socket));
    let interfaces: Vec<String> = daemon
        .interfaces
        .iter()
        .map(|i| match &i.bind {
            Some(bind) => format!("{} {bind}", i.kind),
            None => i.kind.clone(),
        })
        .collect();
    out.push_str(&format!("  interfaces  {}\n", interfaces.join(", ")));
    out.push_str(&format!(
        "  runtime     {}{}\n",
        daemon.runtime,
        daemon
            .herdr_session
            .as_deref()
            .map(|s| format!(" (session {s})"))
            .unwrap_or_default()
    ));

    out.push_str("\nPROVIDERS\n");
    if providers.is_empty() {
        out.push_str(
            "  none declared -- add them under infrastructure.providers in the root .factory/config.yaml\n",
        );
    }
    for p in providers {
        let mut badge = p.kind.as_str().to_string();
        if let Some(env) = &p.env {
            badge.push_str(&format!(", key in ${env}"));
        }
        if let Some(plan) = &p.plan {
            badge.push_str(&format!(", {plan}"));
        }
        out.push_str(&format!("  {}  {}  ({badge})\n", p.name, p.vendor));
        if p.agents.is_empty() {
            out.push_str("    no agents\n");
        }
        for a in &p.agents {
            out.push_str(&format!(
                "    {:<32} {:<14} via {}\n",
                format!("{}/{}", a.scope, a.agent),
                a.harness,
                a.via.as_str()
            ));
        }
    }

    if !unassigned.is_empty() {
        out.push_str("\nUNASSIGNED  model agents no provider claims\n");
        for a in unassigned {
            out.push_str(&format!("    {:<32} {}\n", format!("{}/{}", a.scope, a.agent), a.harness));
        }
    }

    // #131: whether each harness starts, as the last probe before a
    // dispatch found it. Nothing is probed to print this.
    if !harnesses.is_empty() {
        out.push_str("\nHARNESSES  checked with --version before a dispatch\n");
        for h in harnesses {
            let said = match h.state {
                factory_core::harness::HarnessState::Healthy => h.version.clone().unwrap_or_default(),
                factory_core::harness::HarnessState::Unprobed => "not probed since the daemon started".into(),
                factory_core::harness::HarnessState::Unhealthy => h.reason.clone().unwrap_or_default(),
            };
            out.push_str(&format!("  {:<10} {:<10} {:<40} {said}\n", h.harness, h.state.as_str(), h.binary));
            if let Some(repair) = &h.repair {
                out.push_str(&format!("    repair: {repair}\n"));
            }
            if !h.held.is_empty() {
                let held: Vec<String> = h.held.iter().map(|t| format!("{} ({})", t.task_id, t.title)).collect();
                out.push_str(&format!("    held: {} task(s) -- {}\n", held.len(), held.join(", ")));
            }
            if let Some(auto) = &h.auto_repair {
                out.push_str(&format!("    automatic repair: {auto}\n"));
            }
        }
    }
    Some(out.trim_end().to_string())
}

fn utc(t: &chrono::DateTime<chrono::Utc>) -> String {
    t.format("%Y-%m-%d %H:%M UTC").to_string()
}

/// How long ago `then` was, against the daemon's own clock.
fn ago(now: chrono::DateTime<chrono::Utc>, then: chrono::DateTime<chrono::Utc>) -> String {
    format!("{} ago", duration((now - then).num_seconds().max(0) as u64))
}

fn kept_by_text(kept: &[factory_core::backup::KeptBy]) -> String {
    use factory_core::backup::KeptBy;
    if kept.is_empty() {
        return "deleted at the next backup".into();
    }
    kept.iter()
        .map(|k| match k {
            KeptBy::Newest => "newest",
            KeptBy::Daily => "daily",
            KeptBy::Weekly => "weekly",
            KeptBy::Monthly => "monthly",
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn short(commit: &str) -> String {
    commit.chars().take(9).collect()
}

fn pct_or(v: Option<f64>) -> String {
    v.map(|v| format!("{:.2}%", v * 100.0)).unwrap_or_else(|| "--".into())
}

fn env_status(s: factory_core::environments::EnvStatus) -> &'static str {
    use factory_core::environments::EnvStatus;
    match s {
        EnvStatus::Up => "up",
        EnvStatus::Degraded => "DEGRADED",
        EnvStatus::Down => "DOWN",
        EnvStatus::Unknown => "unknown",
    }
}

fn tier(t: factory_core::environments::Tier) -> &'static str {
    use factory_core::environments::Tier;
    match t {
        Tier::Production => "production",
        Tier::Staging => "staging",
        Tier::Ephemeral => "ephemeral",
    }
}

/// A finish that asked for a success and was recorded as a failure -- its
/// checks did not pass -- is an error to whoever asked, so a script stops.
fn refused_success(payload: &Payload, wanted: factory_core::environments::DeployStatus) -> Result<()> {
    use factory_core::environments::DeployStatus;
    match payload {
        Payload::Deployment { deployment } if wanted == DeployStatus::Succeeded && deployment.status != DeployStatus::Succeeded => {
            Err(anyhow!("recorded as {}: {}", deployment.status.as_str(), deployment.reason.as_deref().unwrap_or("")))
        }
        _ => Ok(()),
    }
}

fn deployment_text(payload: &Payload) -> Option<String> {
    let Payload::Deployment { deployment: d } = payload else { return None };
    let mut out = format!(
        "deployment {}  {} of {} to {}",
        d.id,
        d.status.as_str(),
        short(&d.release.commit),
        d.environment
    );
    if let Some(secs) = d.duration_seconds() {
        out.push_str(&format!(" in {}", duration(secs as u64)));
    }
    if let Some(v) = &d.verification {
        for s in &v.checks {
            out.push_str(&format!(
                "
  {} {}  {}",
                if s.ok { "ok  " } else { "FAIL" },
                s.check,
                s.detail.as_deref().unwrap_or("")
            ));
        }
    } else if d.finished() {
        out.push_str("
  not verified");
    }
    if let Some(r) = &d.reason {
        out.push_str(&format!("
  reason: {r}"));
    }
    Some(out)
}

fn deployments_table(rows: &[&factory_core::environments::Deployment], now: chrono::DateTime<chrono::Utc>) -> String {
    if rows.is_empty() {
        return "no deployments recorded yet".into();
    }
    let mut out = format!("{:<14} {:<10} {:<12} {:>8} {:<16} {:<14} REASON
", "ENV", "COMMIT", "STATUS", "TOOK", "WHO", "WHEN");
    for d in rows {
        let who = match &d.via {
            Some(via) => format!("{} via {via}", d.actor.name),
            None if d.manual => format!("{} (by hand)", d.actor.name),
            None => d.actor.name.clone(),
        };
        out.push_str(&format!(
            "{:<14} {:<10} {:<12} {:>8} {:<16} {:<14} {}
",
            d.environment,
            short(&d.release.commit),
            d.status.as_str(),
            d.duration_seconds().map(|s| duration(s as u64)).unwrap_or_else(|| "--".into()),
            who,
            ago(now, d.started_at),
            d.reason.as_deref().unwrap_or("")
        ));
    }
    out.trim_end().to_string()
}

/// `factory env`: one line per environment, in promotion order.
fn env_list_text(payload: &Payload) -> Option<String> {
    let Payload::Environments { report } = payload else { return None };
    if report.environments.is_empty() {
        return Some(
            "no environments declared or deployed to -- declare them under `environments:` in a scope's \
             .factory/config.yaml"
                .into(),
        );
    }
    let mut out = format!(
        "{:<14} {:<11} {:<9} {:<10} {:>9} {:>9} {:>9} {:>8}  SINCE
",
        "ENV", "TIER", "STATUS", "RUNNING", "24H", "7D", "SLO", "BUDGET"
    );
    for c in &report.environments {
        let slo = match &c.slo {
            Some(s) => format!("{}/{:.1}%", pct_or(c.uptime_window), s.target * 100.0),
            None => pct_or(c.uptime_window),
        };
        out.push_str(&format!(
            "{:<14} {:<11} {:<9} {:<10} {:>9} {:>9} {:>9} {:>8}  {}
",
            c.name,
            tier(c.tier),
            if c.paused { "paused" } else { env_status(c.status) },
            c.current.as_ref().map(|d| short(&d.release.commit)).unwrap_or_else(|| "--".into()),
            pct_or(c.uptime_24h),
            pct_or(c.uptime_7d),
            slo,
            c.error_budget.map(|b| format!("{:.0}%", b * 100.0)).unwrap_or_else(|| "--".into()),
            c.status_since.map(|t| ago(report.generated_at, t)).unwrap_or_else(|| "--".into())
        ));
    }
    Some(out.trim_end().to_string())
}

fn env_detail_text(
    c: &factory_core::environments::EnvironmentCard,
    deployments: &[factory_core::environments::Deployment],
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    let mut out = format!(
        "{}  {}  {}  (scope {}{})
",
        c.name,
        tier(c.tier),
        if c.paused { "paused" } else { env_status(c.status) },
        c.scope,
        if c.declared { "" } else { ", not declared" }
    );
    if let Some(url) = &c.url {
        out.push_str(&format!("  url          {url}
"));
    }
    if let Some(next) = &c.promotes_to {
        out.push_str(&format!("  promotes to  {next}
"));
    }
    match &c.current {
        Some(d) => out.push_str(&format!(
            "  running      {} {}  since {}
",
            short(&d.release.commit),
            d.release.version.as_deref().or(d.release.describe.as_deref()).unwrap_or(""),
            ago(now, d.finished_at.unwrap_or(d.started_at))
        )),
        None => out.push_str("  running      nothing recorded
"),
    }
    if let Some(d) = &c.running {
        out.push_str(&format!("  deploying    {} since {}
", short(&d.release.commit), ago(now, d.started_at)));
    }
    out.push_str(&format!("  uptime       24h {}  7d {}  window {}
", pct_or(c.uptime_24h), pct_or(c.uptime_7d), pct_or(c.uptime_window)));
    if let Some(slo) = &c.slo {
        out.push_str(&format!(
            "  slo          {:.2}% over {}d, error budget {}
",
            slo.target * 100.0,
            slo.window_days,
            c.error_budget.map(|b| format!("{:.0}% left", b * 100.0)).unwrap_or_else(|| "--".into())
        ));
    }
    let d = &c.dora;
    out.push_str(&format!(
        "  dora         {}/week, lead time {}, change failure {}, restore {} (mttr {})
",
        d.deploy_frequency.map(|v| format!("{v:.1}")).unwrap_or_else(|| "--".into()),
        d.lead_time_p50.map(|v| duration(v as u64)).unwrap_or_else(|| "--".into()),
        pct_or(d.change_failure_rate),
        d.time_to_restore_p50.map(|v| duration(v as u64)).unwrap_or_else(|| "--".into()),
        d.mttr.map(|v| duration(v as u64)).unwrap_or_else(|| "--".into()),
    ));
    if !c.checks.is_empty() {
        out.push_str("
CHECKS
");
        for ch in &c.checks {
            let last = match &ch.last {
                Some(s) => format!("{} {}  {}", if s.ok { "ok  " } else { "FAIL" }, ago(now, s.at), s.detail.as_deref().unwrap_or("")),
                None => "not checked yet".into(),
            };
            out.push_str(&format!("  {:<24} every {:>4}s  {}
", ch.name, ch.every_seconds, last));
        }
    }
    if !c.incidents.is_empty() {
        out.push_str("
INCIDENTS
");
        for i in &c.incidents {
            out.push_str(&format!(
                "  {}  {}  {}
",
                utc(&i.started_at),
                match i.ended_at {
                    Some(_) => format!("lasted {}", duration(i.duration_seconds(now) as u64)),
                    None => format!("OPEN for {}", duration(i.duration_seconds(now) as u64)),
                },
                i.checks.join(", ")
            ));
        }
    }
    let mine: Vec<&factory_core::environments::Deployment> =
        deployments.iter().filter(|d| d.environment == c.name).take(10).collect();
    out.push_str("
DEPLOYMENTS
");
    out.push_str(&deployments_table(&mine, now));
    out
}

/// `factory backup status`, for a person: the hero line, the facts under
/// it, then every warning.
fn backup_status_text(payload: &Payload) -> Option<String> {
    use factory_core::backup::{AgeLevel, RepositoryState, TimeMachineFact, WarningLevel};
    let Payload::Backup { report } = payload else { return None };
    let mut out = String::new();
    let level = match report.age {
        AgeLevel::Fresh => "fresh",
        AgeLevel::Stale => "STALE",
        AgeLevel::Overdue => "OVERDUE",
        AgeLevel::None => "NO BACKUP",
    };
    match report.snapshots.first() {
        Some(newest) => out.push_str(&format!(
            "BACKUP  {level}  newest {} ({}), {}\n",
            ago(report.now, newest.at),
            utc(&newest.at),
            bytes(newest.size_bytes)
        )),
        None => out.push_str(&format!("BACKUP  {level}\n")),
    }
    if let Some(config) = &report.config {
        if let Some(d) = &report.destination {
            let device = match d.same_device {
                Some(true) => "SAME DEVICE as the instance",
                Some(false) => "another device",
                None => "device unknown",
            };
            let free = d.free_bytes.map(|b| format!(", {} free", bytes(b))).unwrap_or_default();
            let state = if d.exists { format!("{device}{free}") } else { "MISSING".into() };
            out.push_str(&format!("  destination  {}  ({state})\n", d.path));
        }
        out.push_str(&format!(
            "  schedule     {}\n",
            config.schedule.as_ref().map(|s| s.describe()).unwrap_or_else(|| "none -- only when someone runs one".into())
        ));
        if let Some(next) = &report.next_run {
            out.push_str(&format!("  next         {}\n", utc(next)));
        }
        out.push_str(&format!(
            "  verify       {}\n",
            config.verify_schedule.as_ref().map(|s| s.describe()).unwrap_or_else(|| "no drill scheduled".into())
        ));
        if let Some(next) = &report.next_verify {
            out.push_str(&format!("  next drill   {}\n", utc(next)));
        }
        if let Some(reason) = &report.verify_skipped {
            out.push_str(&format!("  drill skip   {reason}\n"));
        }
        out.push_str(&format!(
            "  keep         {} daily, {} weekly, {} monthly{}\n",
            config.keep.daily,
            config.keep.weekly,
            config.keep.monthly,
            if config.include_logs { "; logs included" } else { "" }
        ));
        out.push_str(&format!("  snapshots    {}\n", report.snapshots.len()));
        out.push_str(&format!(
            "  verified     {}\n",
            match &report.last_verified {
                Some(v) => format!("{}  {}  {}", utc(&v.at), if v.ok { "ok" } else { "FAILED" }, v.snapshot),
                None => "never".into(),
            }
        ));
        out.push_str(&format!(
            "  encrypted    {}\n",
            match &config.encrypt_to {
                Some(recipient) => format!("yes, to {recipient}"),
                None => "no".into(),
            }
        ));
        if report.running {
            out.push_str("  running      a backup operation is in progress\n");
        }
    }
    // `#155`: scope source code is backed up by pushing it, not by the
    // snapshot above, so this reads whether or not one is even configured.
    if !report.code.is_empty() {
        out.push_str("\nCODE\n");
        for repo in &report.code {
            let scopes = repo.scopes.join(", ");
            let remote = repo.remote_url.as_deref().unwrap_or("--");
            let state = match &repo.state {
                RepositoryState::Tracked { upstream, ahead, .. } if *ahead > 0 => {
                    format!("{upstream}, {ahead} unpushed (as of last fetch)")
                }
                RepositoryState::Tracked { upstream, .. } => format!("{upstream}, up to date (as of last fetch)"),
                RepositoryState::NoDirectory => "no such directory".into(),
                RepositoryState::NotARepository => "not a git repository".into(),
                RepositoryState::NoCommits => "no commits yet".into(),
                RepositoryState::DetachedHead => "detached HEAD".into(),
                RepositoryState::NoRemote => "no remote configured".into(),
                RepositoryState::NoUpstream => "no upstream branch".into(),
                RepositoryState::InspectionFailed { reason } => format!("unknown -- {reason}"),
            };
            out.push_str(&format!("  {scopes:<24} {remote:<44} {state}\n"));
        }
    }
    out.push_str("\nTIME MACHINE  ");
    out.push_str(&match &report.time_machine {
        Some(TimeMachineFact::Configured { destinations }) => format!("configured -- {}\n", destinations.join(", ")),
        Some(TimeMachineFact::NotConfigured) => "not configured\n".to_string(),
        Some(TimeMachineFact::Unavailable { reason }) => format!("unknown -- {reason}\n"),
        Some(TimeMachineFact::Unsupported) => "not supported on this platform\n".to_string(),
        None => "unknown -- this daemon did not report it\n".to_string(),
    });
    if !report.warnings.is_empty() {
        out.push_str("\nWARNINGS\n");
        for w in &report.warnings {
            let mark = match w.level {
                WarningLevel::Bad => "!!",
                WarningLevel::Warn => "! ",
            };
            out.push_str(&format!("  {mark} {}\n", w.message));
        }
    }
    Some(out.trim_end().to_string())
}

/// `factory backup list`: one line per snapshot, newest first.
fn backup_list_text(payload: &Payload) -> Option<String> {
    let Payload::Backup { report } = payload else { return None };
    let Some(config) = &report.config else {
        return Some("no backup is configured -- add infrastructure.backup to the root .factory/config.yaml".into());
    };
    if report.snapshots.is_empty() {
        return Some(format!("no snapshots in {}", config.destination.display()));
    }
    let mut out = format!("{:<62} {:>10} {:>6} {:<3} {:<22} KEPT BY\n", "SNAPSHOT", "SIZE", "FILES", "ENC", "VERIFIED");
    for s in &report.snapshots {
        let verified = match &s.verified {
            Some(v) => format!("{} {}", if v.ok { "ok" } else { "FAILED" }, v.at.format("%Y-%m-%d %H:%M")),
            None => "--".into(),
        };
        out.push_str(&format!(
            "{:<62} {:>10} {:>6} {:<3} {:<22} {}\n",
            s.name,
            bytes(s.size_bytes),
            s.files.map(|n| n.to_string()).unwrap_or_else(|| "--".into()),
            if s.encrypted { "yes" } else { "" },
            verified,
            kept_by_text(&s.kept_by)
        ));
    }
    Some(out.trim_end().to_string())
}

fn backup_run_text(payload: &Payload) -> Option<String> {
    let Payload::BackupRun { snapshot } = payload else { return None };
    let mut out = format!(
        "backed up to {}\n  {} files, {} archived ({} database), in {:.1}s\n",
        snapshot.path,
        snapshot.files,
        bytes(snapshot.size_bytes),
        bytes(snapshot.database_bytes),
        snapshot.duration_ms as f64 / 1000.0
    );
    if snapshot.pruned.is_empty() {
        out.push_str("  retention deleted nothing");
    } else {
        out.push_str(&format!("  retention deleted {}: {}", snapshot.pruned.len(), snapshot.pruned.join(", ")));
    }
    Some(out)
}

fn backup_verify_text(payload: &Payload) -> Option<String> {
    use factory_core::backup::CheckStatus;
    let Payload::BackupVerify { verification } = payload else { return None };
    let mut out = format!(
        "{}  {}  in {:.1}s\n",
        if verification.ok { "VERIFIED" } else { "FAILED" },
        verification.snapshot,
        verification.duration_ms as f64 / 1000.0
    );
    for c in &verification.checks {
        let status = match c.status {
            CheckStatus::Ok => "ok  ",
            CheckStatus::Warn => "warn",
            CheckStatus::Fail => "FAIL",
        };
        out.push_str(&format!("  {status}  {:<10} {}\n", c.name, c.detail));
    }
    Some(out.trim_end().to_string())
}

fn backup_restore_text(payload: &Payload) -> Option<String> {
    let Payload::BackupRestore { restoration } = payload else { return None };
    let root = shell_word(&restoration.into);
    Some(format!(
        "RESTORED  {}\n  {} files -> {}  in {:.1}s\n\
         Nothing was switched or started. To switch over:\n\
         1. Stop the current factory-daemon.\n\
         2. Start the restored instance: factory-daemon --root {root} run\n\
         3. Point the CLI at it: factory --root {root} status",
        restoration.snapshot,
        restoration.files,
        restoration.into,
        restoration.duration_ms as f64 / 1000.0,
    ))
}

/// One shell word for the concrete commands printed after restore.
fn shell_word(value: &str) -> String {
    if !value.is_empty() && value.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+:@%=,".contains(c)) {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

/// Binary units, one decimal: `64.0 GiB`.
fn bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// The two largest units that apply: `10d 4h`, `3h 12m`, `42s`.
fn duration(seconds: u64) -> String {
    let (d, h, m, s) = (seconds / 86_400, seconds / 3_600 % 24, seconds / 60 % 60, seconds % 60);
    match (d, h, m) {
        (0, 0, 0) => format!("{s}s"),
        (0, 0, _) => format!("{m}m {s}s"),
        (0, _, _) => format!("{h}h {m}m"),
        _ => format!("{d}d {h}h"),
    }
}

/// The exact strings `Payload::Knowledge`'s findings carry on the wire.
fn finding_kind_str(kind: &FindingKind) -> &'static str {
    match kind {
        FindingKind::Unsourced => "unsourced",
        FindingKind::SecretSource => "secret_source",
        FindingKind::MissingSource => "missing_source",
        FindingKind::IncompleteFrontmatter => "incomplete_frontmatter",
        FindingKind::Orphan => "orphan",
        FindingKind::AmbiguousLink => "ambiguous_link",
        FindingKind::Truncated => "truncated",
    }
}

/// A source path, made absolute against the current directory when it is
/// not already -- the daemon's refusals compare a source against the
/// instance root purely as strings, never by asking the filesystem, so a
/// relative path has to be resolved here rather than there.
fn absolute(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().map(|cwd| cwd.join(p)).unwrap_or_else(|_| p.to_path_buf())
    }
}

async fn knowledge_cmd(json: bool, client: &Client, cmd: KnowledgeCmd) -> Result<()> {
    match cmd {
        KnowledgeCmd::Import { source, into, overwrite } => {
            let payload = client
                .send(Request::KnowledgeImport {
                    source: absolute(&source).display().to_string(),
                    into,
                    overwrite,
                })
                .await?;
            print(&payload, json, knowledge_write_text)
        }
        KnowledgeCmd::Add { files, into, overwrite } => {
            let sources = files.iter().map(|f| absolute(f).display().to_string()).collect();
            let payload = client
                .send(Request::KnowledgeAdd { sources, into, overwrite })
                .await?;
            print(&payload, json, knowledge_write_text)
        }
        KnowledgeCmd::Search { words, tags, scope, limit } => {
            let payload = client
                .send(Request::KnowledgeSearch { text: words.join(" "), tags, scope, limit })
                .await?;
            print(&payload, json, |p| match p {
                Payload::KnowledgeHits { provider, vault, hits } => Some(if hits.is_empty() {
                    format!("no page matches ({provider} search of {vault})")
                } else {
                    hits.iter()
                        .map(|h| format!("{vault}/{}.md  {}\n    {}", h.page, h.title, h.why))
                        .collect::<Vec<_>>()
                        .join("\n")
                }),
                _ => None,
            })
        }
    }
}

async fn dataset_cmd(json: bool, client: &Client, cmd: DatasetCmd) -> Result<()> {
    match cmd {
        DatasetCmd::List => {
            let payload = client.send(Request::Datasets).await?;
            print(&payload, json, |p| match p {
                Payload::Datasets { root, datasets } => Some(if datasets.is_empty() {
                    format!("no datasets in {root}")
                } else {
                    datasets.iter().map(dataset_summary_line).collect::<Vec<_>>().join("\n")
                }),
                _ => None,
            })
        }
        DatasetCmd::Show { name } => {
            let payload = client.send(Request::Dataset { name }).await?;
            print(&payload, json, |p| match p {
                Payload::Dataset { dataset, findings } => Some(dataset_detail(dataset, findings)),
                _ => None,
            })
        }
        DatasetCmd::Create { name, description } => {
            let payload = client.send(Request::DatasetCreate { name, description }).await?;
            print(&payload, json, |p| match p {
                Payload::Dataset { dataset, findings } => Some(dataset_detail(dataset, findings)),
                _ => None,
            })
        }
        DatasetCmd::Case(CaseCmd::Add {
            name,
            id,
            title,
            scope,
            instructions,
            base,
            reset,
            gate,
            timeout,
        }) => {
            let case = Case {
                id,
                title,
                scope,
                instructions,
                base,
                reset,
                gate,
                timeout_seconds: timeout,
                origin: None,
            };
            let payload = client
                .send(Request::DatasetAddCases { name, cases: vec![case] })
                .await?;
            print(&payload, json, |p| match p {
                Payload::Dataset { dataset, findings } => Some(dataset_detail(dataset, findings)),
                _ => None,
            })
        }
        DatasetCmd::Case(CaseCmd::Rm { name, id }) => {
            let payload = client.send(Request::DatasetDeleteCase { name, id }).await?;
            print(&payload, json, |p| match p {
                Payload::Dataset { dataset, findings } => Some(dataset_detail(dataset, findings)),
                _ => None,
            })
        }
        DatasetCmd::Import { name, file, format, replace } => {
            let format = format.or_else(|| {
                file.extension().and_then(|e| e.to_str()).map(str::to_string)
            }).ok_or_else(|| anyhow!("cannot tell the import format from {file:?}; pass --format"))?;
            let content = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;
            let payload = client
                .send(Request::DatasetImport { name, format, content, replace })
                .await?;
            print(&payload, json, |p| match p {
                Payload::Dataset { dataset, findings } => Some(dataset_detail(dataset, findings)),
                _ => None,
            })
        }
        DatasetCmd::FromTasks { name, task_ids, scope, status } => {
            let task_ids = if !task_ids.is_empty() {
                task_ids
            } else if scope.is_some() || status.is_some() {
                let payload = client
                    .send(Request::TaskList(TaskFilter { status, scope, ..Default::default() }))
                    .await?;
                match payload {
                    Payload::Tasks { tasks } => tasks.into_iter().map(|t| t.id).collect(),
                    _ => return Err(anyhow!("unexpected answer to task.list")),
                }
            } else {
                return Err(anyhow!(
                    "name task ids directly, or select in bulk with --scope/--status"
                ));
            };
            if task_ids.is_empty() {
                return Err(anyhow!("no tasks matched; nothing to generate cases from"));
            }
            let payload = client
                .send(Request::DatasetFromTasks { name, task_ids })
                .await?;
            print(&payload, json, |p| match p {
                Payload::Dataset { dataset, findings } => Some(dataset_detail(dataset, findings)),
                _ => None,
            })
        }
        DatasetCmd::Rm { name } => {
            let payload = client.send(Request::DatasetDelete { name: name.clone() }).await?;
            print(&payload, json, |p| match p {
                Payload::Deleted { deleted } => Some(if *deleted {
                    format!("deleted {name}")
                } else {
                    format!("no such dataset: {name}")
                }),
                _ => None,
            })
        }
    }
}

fn dataset_summary_line(d: &DatasetSummary) -> String {
    format!(
        "{:<20} rev {:<4} {:>3} case(s), {:>3} gated{}",
        d.name,
        d.revision,
        d.cases,
        d.gated,
        if d.findings.is_empty() {
            String::new()
        } else {
            format!(", {} finding(s)", d.findings.len())
        }
    )
}

fn dataset_detail(d: &Dataset, findings: &[DatasetFinding]) -> String {
    let mut out = format!(
        "{}  rev {}{}\n",
        d.name,
        d.revision,
        d.description.as_deref().map(|s| format!("  -- {s}")).unwrap_or_default()
    );
    if d.cases.is_empty() {
        out.push_str("  no cases\n");
    }
    for c in &d.cases {
        out.push_str(&format!(
            "\n  {}  {}\n    scope {}  base {}  gate {}  reset {}\n",
            c.id,
            c.title,
            c.scope,
            c.base.as_deref().unwrap_or("scope HEAD"),
            c.gate.as_deref().unwrap_or("-- unverified"),
            c.reset.as_deref().unwrap_or("--"),
        ));
        if let Some(origin) = &c.origin {
            out.push_str(&format!(
                "    origin: task {} run {} outcome {} recorded {}\n",
                origin.task, origin.run, origin.outcome, origin.recorded_at.to_rfc3339()
            ));
        }
    }
    if !findings.is_empty() {
        out.push_str("\nFINDINGS\n");
        for f in findings {
            out.push_str(&format!("  {:?}  {}  {}\n", f.kind, f.case, f.detail));
        }
    }
    out.trim_end().to_string()
}

async fn policy_cmd(json: bool, client: &Client, cmd: PolicyCmd) -> Result<()> {
    match cmd {
        PolicyCmd::Status { scope, framework } => {
            let payload = client.send(Request::Policy { scope }).await?;
            print(&payload, json, |p| match p {
                Payload::Policy { report } => Some(policy_status_text(report, framework.as_deref())),
                _ => None,
            })
        }

        PolicyCmd::Show { control, scope } => {
            let control_ref: ControlRef = control.parse().map_err(|e: String| anyhow!(e))?;
            let scope = match scope {
                Some(s) => s,
                None => {
                    // Not named: find every scope the whole instance's
                    // status board shows this control applying in, and use
                    // it if that is exactly one -- the common case, since
                    // most controls apply company-wide via the root's own
                    // `policies:`.
                    let payload = client.send(Request::Policy { scope: None }).await?;
                    let Payload::Policy { report } = payload else {
                        return Err(anyhow!("unexpected answer to policy"));
                    };
                    let matches: Vec<String> = report
                        .rows
                        .iter()
                        .filter(|r| r.statuses.iter().any(|s| s.control == control_ref))
                        .map(|r| r.scope.clone())
                        .collect();
                    match matches.as_slice() {
                        [one] => one.clone(),
                        [] => return Err(anyhow!("{control_ref} applies in no scope")),
                        many => {
                            return Err(anyhow!(
                                "{control_ref} applies in {} scopes ({}); name one with --scope",
                                many.len(),
                                many.join(", ")
                            ))
                        }
                    }
                }
            };
            let payload = client
                .send(Request::PolicyControl { control: control_ref, scope })
                .await?;
            print(&payload, json, |p| match p {
                Payload::PolicyControl { detail } => Some(policy_control_text(detail)),
                _ => None,
            })
        }

        PolicyCmd::Attest { control, scope, evidence, expires, note, clock_item, deadline, corrective_item, available_at } => {
            let control: ControlRef = control.parse().map_err(|e: String| anyhow!(e))?;
            let clock = match (clock_item, deadline) {
                (Some(item), Some(deadline)) => {
                    let item: ClockItemRef = item.parse().map_err(|e: String| anyhow!(e))?;
                    let deadline: ClockDeadlineKind = deadline.parse().map_err(|e: String| anyhow!(e))?;
                    Some(ClockMark { item, deadline })
                }
                _ => None,
            };
            let corrective = match (corrective_item, available_at) {
                (Some(item), Some(available_at)) => Some(reporting_clock::CorrectiveMeasureMark {
                    item: item.parse().map_err(|e: String| anyhow!(e))?, available_at,
                }),
                _ => None,
            };
            let expires = match expires {
                Some(e) => e,
                None if clock.is_some() || corrective.is_some() => "520w".to_string(),
                None => return Err(anyhow!("--expires is required unless --clock-item or --corrective-item is given")),
            };
            let expires_at = policy::parse_expiry(&expires, chrono::Utc::now()).map_err(|e| anyhow!(e))?;
            let payload = client
                .send(Request::PolicyAttest { control, scope, evidence, note, expires_at, clock, corrective })
                .await?;
            print(&payload, json, |p| match p {
                Payload::PolicyAttestation { attestation } => Some(attestation_line(attestation)),
                _ => None,
            })
        }

        PolicyCmd::Clock { scope } => {
            let payload = client.send(Request::PolicyClock { scope }).await?;
            print(&payload, json, |p| match p {
                Payload::PolicyClock { clock } => Some(policy_clock_text(clock)),
                _ => None,
            })
        }

        PolicyCmd::Withdraw { id, reason } => {
            let payload = client.send(Request::PolicyWithdraw { id, reason }).await?;
            print(&payload, json, |p| match p {
                Payload::PolicyAttestation { attestation } => Some(attestation_line(attestation)),
                _ => None,
            })
        }

        PolicyCmd::Frameworks => {
            let payload = client.send(Request::Policy { scope: None }).await?;
            print(&payload, json, |p| match p {
                Payload::Policy { report } => Some(if report.catalogues.is_empty() {
                    "no catalogues on disk".to_string()
                } else {
                    report
                        .catalogues
                        .iter()
                        .map(|c| {
                            format!(
                                "{:<12} {:<12} {:<40} {:>3} control(s)",
                                c.framework,
                                policy_kind_str(c.kind),
                                c.title,
                                c.controls
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                }),
                _ => None,
            })
        }

        PolicyCmd::Remediate { control, scope, agent } => {
            let control: ControlRef = control.parse().map_err(|e: String| anyhow!(e))?;
            let payload = client
                .send(Request::PolicyRemediate { control, scope, agent })
                .await?;
            print(&payload, json, |p| match p {
                Payload::Task { task } => Some(format!(
                    "created {}  \"{}\"  scope {}  agent {}\n  {}",
                    task.id,
                    task.title,
                    task.scope,
                    task.agent,
                    task.instructions.lines().collect::<Vec<_>>().join("\n  "),
                )),
                _ => None,
            })
        }

        PolicyCmd::Export { scope, format } => {
            let payload = client.send(Request::PolicyExport { scope, format }).await?;
            print(&payload, json, |p| match p {
                Payload::PolicyExport { body, .. } => Some(body.clone()),
                _ => None,
            })
        }
    }
}

fn policy_kind_str(kind: policy::Kind) -> &'static str {
    match kind {
        policy::Kind::Regulation => "regulation",
        policy::Kind::Standard => "standard",
        policy::Kind::BestPractice => "best-practice",
    }
}

/// One line per framework's rollup over the asked scope's subtree, plainly
/// noting that "compliant" means the evidence is complete, never certified
/// (ADR 0004); then every open or stale control (the worst status across
/// every scope it applies to, via the same `policy::worst_across_scopes`
/// the daemon's own rollup is built from); then every finding.
fn policy_status_text(report: &PolicyReport, framework: Option<&str>) -> String {
    let mut out = String::new();
    let titles: std::collections::BTreeMap<&str, &str> = report
        .catalogues
        .iter()
        .map(|c| (c.framework.as_str(), c.title.as_str()))
        .collect();

    let rollups: Vec<&factory_core::policy::FrameworkRollup> = report
        .rollup
        .iter()
        .filter(|r| framework.is_none_or(|f| r.framework == f))
        .collect();

    if rollups.is_empty() {
        out.push_str("no applicable frameworks\n");
    }
    for r in &rollups {
        let title = titles.get(r.framework.as_str()).copied().unwrap_or("");
        out.push_str(&format!(
            "{:<12} {:<32} {} satisfied, {} attested, {} stale, {} open, {} n/a  [{}]\n",
            r.framework,
            title,
            r.counts.satisfied,
            r.counts.attested,
            r.counts.stale,
            r.counts.open,
            r.counts.not_applicable,
            if r.compliant { "compliant" } else { "not compliant" },
        ));
        let bp = &r.best_practice;
        if bp.satisfied + bp.attested + bp.stale + bp.open + bp.not_applicable > 0 {
            out.push_str(&format!(
                "             best practice (shown, never counted): {} satisfied, {} attested, {} stale, {} open, {} n/a\n",
                bp.satisfied, bp.attested, bp.stale, bp.open, bp.not_applicable,
            ));
        }
    }
    out.push_str("\n\"compliant\" means the evidence is complete -- not that anything is certified.\n");

    let per_scope: Vec<Vec<factory_core::policy::ControlStatus>> =
        report.rows.iter().map(|r| r.statuses.clone()).collect();
    let mut worst = factory_core::policy::worst_across_scopes(&per_scope);
    worst.retain(|s| framework.is_none_or(|f| s.control.framework == f));
    worst.sort_by(|a, b| a.control.cmp(&b.control));

    let mut section = |label: &str, kind: factory_core::policy::StatusKind| {
        let matching: Vec<&factory_core::policy::ControlStatus> =
            worst.iter().filter(|s| s.status.kind() == kind).collect();
        if matching.is_empty() {
            return;
        }
        out.push_str(&format!("\n{label}\n"));
        for s in matching {
            out.push_str(&format!("  {:<24} {}\n", s.control.to_string(), s.title));
            for reason in s.status.reasons() {
                out.push_str(&format!("      {reason}\n"));
            }
        }
    };
    section("OPEN", factory_core::policy::StatusKind::Open);
    section("STALE", factory_core::policy::StatusKind::Stale);

    if !report.findings.is_empty() {
        out.push_str("\nFINDINGS\n");
        for f in &report.findings {
            out.push_str(&format!("  {:?}  {}  {}\n", f.kind, f.subject, f.detail));
        }
    }

    out.trim_end().to_string()
}

fn policy_control_text(d: &PolicyControlDetail) -> String {
    let mut out = format!(
        "{}  {}  [{}]\n  status: {}\n",
        d.control,
        d.title,
        policy_kind_str(d.kind),
        // `StatusKind::as_str()` -- moved to `factory_core::policy` (`#83`)
        // so this and `policy_export::export_markdown` agree on the wire
        // spelling from one place rather than two copies of the same match.
        d.status.kind().as_str(),
    );
    for reason in d.status.reasons() {
        out.push_str(&format!("    {reason}\n"));
    }
    if let Some(age) = d.max_age {
        out.push_str(&format!("  max_age: {age}\n"));
    }
    if !d.maps_to.is_empty() {
        out.push_str(&format!(
            "  maps_to: {}\n",
            d.maps_to.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", ")
        ));
    }
    if let Some(na) = &d.not_applicable {
        out.push_str(&format!("  not applicable at {}: {}\n", na.scope, na.rationale));
    }
    if let Some(remediation) = &d.remediation {
        out.push_str(&format!("\nREMEDIATION\n  {}\n", remediation.trim()));
    }
    out.push_str("\nEVIDENCE\n");
    for check in &d.checks {
        // `Check::describe()` -- moved to `factory_core::policy` (`#83`) so
        // this, a remediation task's own instructions
        // (`policy::remediation_instructions`), and `policy-model.js`'s
        // documented port all read the same wording from one place.
        out.push_str(&format!("  {}\n", check.describe()));
    }
    if !d.attestations.is_empty() {
        out.push_str("\nATTESTATIONS\n");
        for a in &d.attestations {
            out.push_str(&format!("  {}\n", attestation_line(a)));
        }
    }
    out.trim_end().to_string()
}

fn attestation_line(a: &factory_core::policy::Attestation) -> String {
    let withdrawn = match &a.withdrawn {
        Some(w) => format!(
            "  withdrawn {} by {}{}",
            w.at.to_rfc3339(),
            w.by,
            w.reason.as_deref().map(|r| format!(" ({r})")).unwrap_or_default()
        ),
        None => String::new(),
    };
    let note = a.note.as_deref().map(|n| format!("  note={n}")).unwrap_or_default();
    format!(
        "{}  {} at {}  by {}  evidence={}  attested {} expires {}{note}{withdrawn}",
        a.id,
        a.control,
        a.scope,
        a.attested_by,
        a.evidence,
        a.attested_at.to_rfc3339(),
        a.expires_at.to_rfc3339(),
    )
}

/// `factory policy clock [--scope]`: one block per item, its awareness time
/// and whether the newest evidence still reports it, then either why it is
/// excluded or its deadlines and whatever submission met (or missed)
/// each one.
fn policy_clock_text(clock: &reporting_clock::ReportingClock) -> String {
    if clock.items.is_empty() {
        return "no reporting-clock items".to_string();
    }
    let mut out = String::new();
    for item in &clock.items {
        out.push_str(&format!(
            "{}  scope={}  aware {}{}\n",
            item.item,
            item.scope,
            item.awareness_at.to_rfc3339(),
            if item.reported_now { "" } else { "  (not in the newest scan)" },
        ));
        if let Some(state) = &item.excluded {
            out.push_str(&format!("  excluded: {state}\n"));
            continue;
        }
        match &item.corrective_measure {
            Some(m) => out.push_str(&format!("  corrective measure available {}  evidence={}  ({} by {})\n", m.available_at.to_rfc3339(), m.evidence, m.attestation, m.by)),
            None => out.push_str("  final_report awaiting corrective-measure evidence\n"),
        }
        for deadline in &item.deadlines {
            let submission = deadline
                .submission
                .as_ref()
                .map(|s| format!("  ({} by {} at {})", s.attestation, s.by, s.at.to_rfc3339()))
                .unwrap_or_default();
            out.push_str(&format!(
                "  {:<14} due {}  {}{submission}\n",
                deadline.deadline, deadline.due_at.to_rfc3339(), deadline.state,
            ));
        }
    }
    out.trim_end().to_string()
}

// ================================================================== metrics

/// `factory metrics [ids…] [--json]`: one line per value, its trend if it
/// has a series, and why it is `--` when it is `None`.
fn metrics_text(values: &[factory_core::metrics::MetricValue], series: &[factory_core::metrics::MetricSeries], registry: &[factory_core::protocol::MetricDefView]) -> String {
    if values.is_empty() {
        return "no metrics computed".to_string();
    }
    let mut out = String::new();
    for v in values {
        let def = registry.iter().find(|d| d.id == v.id.as_str());
        let title = def.map(|d| d.title.as_str()).unwrap_or(v.id.as_str());
        let value = match v.value {
            Some(n) => format!("{n:.4}"),
            None => "--".to_string(),
        };
        let trend = series
            .iter()
            .find(|s| s.id == v.id)
            .and_then(factory_core::metrics::trend)
            .map(|t| {
                format!(
                    "  {}",
                    match t {
                        factory_core::metrics::Trend::Up => "up",
                        factory_core::metrics::Trend::Down => "down",
                        factory_core::metrics::Trend::Flat => "flat",
                    }
                )
            })
            .unwrap_or_default();
        let reason = v.reason.as_deref().map(|r| format!("  ({r})")).unwrap_or_default();
        out.push_str(&format!("{:<40} {:<12} {value:>10}{trend}{reason}\n", v.id.as_str(), title));
    }
    out.trim_end().to_string()
}

fn dashboard_text(tiles: &Option<Vec<factory_core::dashboard::Tile>>, source: &Option<String>) -> String {
    // The server pairs `tiles: null` with `source: null` always -- see
    // `Config::dashboard`'s doc comment -- so `tiles == None` is the whole
    // answer for "built-in default"; a name in `source` otherwise, never a
    // magic string like `"root"` or `"default"`.
    let tiles = match tiles {
        Some(tiles) => tiles,
        None => return "(built-in default)".to_string(),
    };
    let mut out = match source {
        Some(name) => format!("source: {name}\n"),
        None => String::new(),
    };
    for tile in tiles {
        let what = match (&tile.metric, &tile.view) {
            (Some(metric), _) => format!("metric {}", metric.as_str()),
            (None, Some(view)) => format!("view {}", view.as_str()),
            (None, None) => "?".to_string(),
        };
        out.push_str(&format!("{:<8} {what}\n", format!("[{}]", tile.size.as_str())));
    }
    out.trim_end().to_string()
}

// =================================================================== goals

async fn goals_cmd(json: bool, client: &Client, cmd: GoalsCmd) -> Result<()> {
    match cmd {
        GoalsCmd::Status { scope, cycle } => {
            let payload = client.send(Request::Goals { scope, cycle }).await?;
            print(&payload, json, |p| match p {
                Payload::Goals { report } => Some(goals_status_text(report)),
                _ => None,
            })
        }
        GoalsCmd::Cycles => {
            let payload = client.send(Request::Goals { scope: None, cycle: None }).await?;
            print(&payload, json, |p| match p {
                Payload::Goals { report } => Some(goals_cycles_text(report)),
                _ => None,
            })
        }
        GoalsCmd::CheckIn { kr, value, confidence, note } => {
            let kr: KrRef = kr.parse().map_err(|e: String| anyhow!(e))?;
            let payload = client
                .send(Request::GoalsCheckIn { kr, value, confidence, note })
                .await?;
            print(&payload, json, |p| match p {
                Payload::GoalsCheckIn { checkin } => Some(format!(
                    "{}  {}  value={}  confidence={}/10  by {} at {}{}",
                    checkin.id,
                    checkin.kr,
                    checkin.value,
                    checkin.confidence,
                    checkin.by,
                    checkin.at.to_rfc3339(),
                    checkin.note.as_deref().map(|n| format!("  note={n}")).unwrap_or_default(),
                )),
                _ => None,
            })
        }
    }
}

fn band_str(band: Band) -> &'static str {
    match band {
        Band::Green => "green",
        Band::Yellow => "yellow",
        Band::Red => "red",
    }
}

fn cycle_status_str(status: CycleStatus) -> &'static str {
    match status {
        CycleStatus::Future => "future",
        CycleStatus::Current => "current",
        CycleStatus::Past => "past",
    }
}

/// `factory goals [status]`: vision/mission, the north star and its inputs,
/// then per objective its key results (value, target, score, band), then
/// the roadmap by lane, then findings.
fn goals_status_text(report: &GoalsReport) -> String {
    let mut out = String::new();
    if let Some(d) = &report.direction {
        out.push_str(&format!("vision:  {}\nmission: {}\n", d.vision, d.mission));
    }
    if let Some(ns) = &report.north_star {
        let value = ns.value.value.map(|v| format!("{v:.4}")).unwrap_or_else(|| "--".to_string());
        out.push_str(&format!("\nnorth star: {} = {value}  ({})\n", ns.metric, ns.why));
    }
    if !report.inputs.is_empty() {
        out.push_str("inputs:\n");
        for i in &report.inputs {
            let value = i.value.value.map(|v| format!("{v:.4}")).unwrap_or_else(|| "--".to_string());
            out.push_str(&format!("  {:<30} {value}\n", i.metric.to_string()));
        }
    }

    match &report.report {
        Some(cr) => {
            out.push_str(&format!(
                "\ncycle {} ({}, {:.0}% elapsed)\n",
                cr.cycle_id,
                cycle_status_str(cr.status),
                cr.elapsed * 100.0
            ));
            for objective in &cr.objectives {
                let score = objective.score.map(|s| format!("{:.0}%", s * 100.0)).unwrap_or_else(|| "--".to_string());
                out.push_str(&format!("  {} ({})  score {score}\n", objective.title, objective.objective));
                for kr in &objective.key_results {
                    let value = kr.value.map(|v| format!("{v:.4}")).unwrap_or_else(|| "--".to_string());
                    let band = kr.band.map(band_str).unwrap_or("unscored");
                    let kind = match kr.kind {
                        goals_core::KrKind::Committed => "committed",
                        goals_core::KrKind::Aspirational => "aspirational",
                    };
                    out.push_str(&format!(
                        "    [{band:<8}] {:<40} {kind:<12} value={value}  {}\n",
                        kr.title, kr.source
                    ));
                    for reason in &kr.reasons {
                        out.push_str(&format!("               {reason}\n"));
                    }
                }
            }
        }
        None => out.push_str("\nno current cycle\n"),
    }

    if !report.roadmap.is_empty() {
        out.push_str("\nroadmap:\n");
        for lane in [goals_core::Lane::Now, goals_core::Lane::Next, goals_core::Lane::Later] {
            let items: Vec<&factory_core::goals::RoadmapItem> = report.roadmap.iter().filter(|r| r.lane == lane).collect();
            if items.is_empty() {
                continue;
            }
            let lane_name = match lane {
                goals_core::Lane::Now => "now",
                goals_core::Lane::Next => "next",
                goals_core::Lane::Later => "later",
            };
            out.push_str(&format!("  {lane_name}\n"));
            for item in items {
                out.push_str(&format!("    {}  [{}]\n", item.title, item.objectives.join(", ")));
            }
        }
    }

    if !report.findings.is_empty() {
        out.push_str("\nfindings:\n");
        for f in &report.findings {
            out.push_str(&format!("  {:?}  {}  {}\n", f.kind, f.subject, f.detail));
        }
    }

    out.trim_end().to_string()
}

/// `factory goals cycles`: every cycle on disk with its own status and score.
fn goals_cycles_text(report: &GoalsReport) -> String {
    if report.cycles.is_empty() {
        return "no cycles on disk".to_string();
    }
    report
        .cycles
        .iter()
        .map(|c| {
            let score = c.score.map(|s| format!("{:.0}%", s * 100.0)).unwrap_or_else(|| "--".to_string());
            format!("{:<12} {}..{}  {:<8} score {score}", c.id, c.from, c.to, cycle_status_str(c.status))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// =============================================================== quality

async fn quality_cmd(json: bool, client: &Client, cmd: QualityCmd) -> Result<()> {
    match cmd {
        QualityCmd::Status { scope } => {
            let payload = client.send(Request::Quality { scope }).await?;
            print(&payload, json, |p| match p {
                Payload::Quality { report } => Some(quality_status_text(report)),
                _ => None,
            })
        }
        QualityCmd::Scope { name } => {
            let payload = client.send(Request::Quality { scope: Some(name.clone()) }).await?;
            print(&payload, json, |p| match p {
                // The report covers `name` and everything below it; this
                // view is `name`'s own tree. Canonical names come back, so
                // match on the report's own `scope`, not what was typed.
                Payload::Quality { report } => Some(
                    report
                        .scopes
                        .iter()
                        .find(|s| Some(&s.report.scope) == report.scope.as_ref())
                        .map(quality_scope_text)
                        .unwrap_or_else(|| format!("no quality profile applies at {name}")),
                ),
                _ => None,
            })
        }
        QualityCmd::Findings { scope } => {
            let payload = client.send(Request::Quality { scope }).await?;
            print(&payload, json, |p| match p {
                Payload::Quality { report } => Some(quality_findings_text(&report.findings)),
                _ => None,
            })
        }
        QualityCmd::Remediate { scenario, scope, agent } => {
            let (attribute, scenario) = parse_quality_target(&scenario)?;
            let payload = client
                .send(Request::QualityRemediate { scope, attribute, scenario, agent })
                .await?;
            print(&payload, json, |p| match p {
                Payload::QualityRemediate { result } => Some(format!(
                    "{}  {}  {}",
                    if result.created { "created" } else { "already open" },
                    result.task.id,
                    result.task.title
                )),
                _ => None,
            })
        }
    }
}

/// `<attribute>/<scenario>` -> the two halves. Neither half can hold a `/`
/// (an attribute id is `characteristic[.sub]`, a scenario id a slug), so
/// the first one is the split.
fn parse_quality_target(s: &str) -> Result<(String, String)> {
    match s.split_once('/') {
        Some((a, b)) if !a.is_empty() && !b.is_empty() && !b.contains('/') => Ok((a.to_string(), b.to_string())),
        _ => Err(anyhow!("{s:?} is not <attribute>/<scenario>, e.g. reliability.recoverability/daemon-restart")),
    }
}

fn level_str(level: factory_core::quality::Level) -> &'static str {
    match level {
        factory_core::quality::Level::High => "H",
        factory_core::quality::Level::Medium => "M",
        factory_core::quality::Level::Low => "L",
    }
}

/// `factory quality [status]`: per scope, one line per attribute --
/// (importance, difficulty) and its rollup -- then how many findings.
fn quality_status_text(report: &factory_core::protocol::QualityReport) -> String {
    let mut out = String::new();
    if report.scopes.is_empty() {
        out.push_str("no quality profile applies anywhere asked\n");
    }
    for scope in &report.scopes {
        let r = &scope.report;
        out.push_str(&format!("{}  ({})\n", r.scope, r.profiles.join(", ")));
        for a in &r.attributes {
            out.push_str(&format!(
                "  ({},{})  {:<8} {}\n",
                level_str(a.importance),
                level_str(a.difficulty),
                a.status.as_str(),
                a.id
            ));
        }
    }
    if !report.findings.is_empty() {
        out.push_str(&format!("\n{} finding(s) -- factory quality findings\n", report.findings.len()));
    }
    out.trim_end().to_string()
}

/// `factory quality scope <name>`: the whole utility tree of one scope.
fn quality_scope_text(scope: &factory_core::protocol::ScopeQuality) -> String {
    let r = &scope.report;
    let mut out = format!("{}  (profiles: {})\n", r.scope, r.profiles.join(", "));
    for a in &r.attributes {
        out.push_str(&format!(
            "\n{}  importance {}, difficulty {}  -- {}  (from {} at {})\n",
            a.id,
            level_str(a.importance),
            level_str(a.difficulty),
            a.status.as_str(),
            a.declared_at.profile,
            a.declared_at.scope
        ));
        for s in &a.scenarios {
            let sc = &s.scenario.scenario;
            let measure = sc
                .measure
                .as_ref()
                .map(factory_core::quality::describe_measure)
                .unwrap_or_else(|| "no measure yet".to_string());
            out.push_str(&format!("  [{:<7}] {}  {measure}\n", s.status.as_str(), sc.id));
            if let Some(task) = scope.open_tasks.get(&format!("{}/{}", a.id, sc.id)) {
                out.push_str(&format!("            remediation task open: {task}\n"));
            }
            for reason in &s.reasons {
                out.push_str(&format!("            {reason}\n"));
            }
        }
    }
    if !r.tradeoffs.is_empty() {
        out.push_str("\ntrade-offs:\n");
        for t in &r.tradeoffs {
            let decision = t.tradeoff.decision.as_deref().map(|d| format!("  ({d})")).unwrap_or_default();
            out.push_str(&format!(
                "  {} <-> {}: {}{decision}\n",
                t.tradeoff.between[0], t.tradeoff.between[1], t.tradeoff.point
            ));
        }
    }
    out.trim_end().to_string()
}

fn quality_findings_text(findings: &[factory_core::quality::Finding]) -> String {
    if findings.is_empty() {
        return "no findings".to_string();
    }
    findings
        .iter()
        .map(|f| {
            let kind = serde_json::to_value(f.kind).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
            format!("{kind:<30} {}  {}", f.subject, f.detail)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ============================================================= scenarios

async fn scenario_cmd(json: bool, client: &Client, cmd: ScenarioCmd) -> Result<()> {
    match cmd {
        ScenarioCmd::List { scope } => {
            let payload = client.send(Request::Scenarios { scope }).await?;
            print(&payload, json, |p| match p {
                Payload::Scenarios { report } => Some(scenarios_list_text(report)),
                _ => None,
            })
        }

        ScenarioCmd::Show { name, scope } => {
            let payload = client.send(Request::Scenarios { scope }).await?;
            let Payload::Scenarios { report } = &payload else {
                return Err(anyhow!("unexpected answer to scenarios"));
            };
            if !report.scenarios.iter().any(|s| s.scenario.name == name) {
                return Err(anyhow!("no such scenario: {name:?}"));
            }
            // `--json` prints the whole report, not just this one scenario:
            // a scenario's numbers (its delta, its forecast) only mean
            // something read next to `baseline`, which a lone
            // `ScenarioResult` would not carry.
            print(&payload, json, |p| match p {
                Payload::Scenarios { report } => report.scenarios.iter().find(|s| s.scenario.name == name).map(|s| scenario_detail_text(report, s)),
                _ => None,
            })
        }

        ScenarioCmd::Promote { name, scope, agent } => {
            let payload = client.send(Request::ScenarioPromote { scenario: name, scope, agent }).await?;
            print(&payload, json, |p| match p {
                Payload::ScenarioPromote { result } => Some(scenario_promote_text(result)),
                _ => None,
            })
        }
    }
}

fn signpost_state_str(state: scenario::SignpostState) -> &'static str {
    match state {
        scenario::SignpostState::Quiet => "quiet",
        scenario::SignpostState::Triggered => "TRIGGERED",
        scenario::SignpostState::NotYetActive => "not yet active",
        scenario::SignpostState::NoData => "no data",
    }
}

/// `p10=..w p50=..w p90=..w (N samples)`, `"never"` for a percentile that
/// never clears the backlog within the horizon, or the forecast's own
/// `reason` when there was nothing to sample from at all.
fn forecast_summary(f: &scenario::Forecast) -> String {
    match &f.reason {
        Some(reason) => format!("n/a ({reason})"),
        None => {
            let week = |w: Option<u32>| w.map(|w| format!("{w}w")).unwrap_or_else(|| "never".to_string());
            format!(
                "p10={} p50={} p90={}  ({} samples, seed {})",
                week(f.completion_week.p10),
                week(f.completion_week.p50),
                week(f.completion_week.p90),
                f.samples,
                f.seed
            )
        }
    }
}

fn policy_delta_line(d: &scenario::PolicyDelta) -> String {
    let mut out = format!(
        "{} newly open, {} newly stale, {} newly applicable (already covered), {} unchanged",
        d.newly_open.len(),
        d.newly_stale.len(),
        d.newly_applicable_but_covered.len(),
        d.unchanged,
    );
    if !d.missing_from_scenario.is_empty() {
        out.push_str(&format!(", {} missing from the scenario (!)", d.missing_from_scenario.len()));
    }
    out
}

/// `factory scenario [list] [--scope S] [--json]`: the baseline, one
/// summary line per scenario, every currently triggered signpost, then
/// findings.
fn scenarios_list_text(report: &ScenariosReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("BASELINE{}\n", report.scope.as_ref().map(|s| format!(" ({s})")).unwrap_or_default()));
    out.push_str(&format!("  forecast: {}\n", forecast_summary(&report.baseline.forecast)));
    for r in &report.baseline.policy {
        out.push_str(&format!(
            "  policy {:<12} {} satisfied, {} attested, {} stale, {} open, {} n/a  [{}]\n",
            r.framework,
            r.counts.satisfied,
            r.counts.attested,
            r.counts.stale,
            r.counts.open,
            r.counts.not_applicable,
            if r.compliant { "compliant" } else { "not compliant" },
        ));
    }

    if report.scenarios.is_empty() {
        out.push_str("\nno scenarios on disk\n");
    } else {
        out.push_str("\nSCENARIOS\n");
        for s in &report.scenarios {
            let triggered = s.signposts.iter().filter(|sp| sp.state == scenario::SignpostState::Triggered).count();
            out.push_str(&format!(
                "  {:<20} {:<40} backlog={:<5} {}{}\n",
                s.scenario.name,
                s.scenario.title,
                s.backlog.total,
                forecast_summary(&s.forecast),
                if triggered > 0 { format!("  [{triggered} signpost(s) triggered]") } else { String::new() },
            ));
        }
    }

    if !report.triggered.is_empty() {
        out.push_str("\nTRIGGERED SIGNPOSTS (observation only -- no automatic consequence)\n");
        for t in &report.triggered {
            out.push_str(&format!("  {:<20} {:<20} {}\n", t.scenario, t.metric, t.reason));
        }
    }

    if !report.findings.is_empty() {
        out.push_str("\nFINDINGS\n");
        for f in &report.findings {
            out.push_str(&format!("  {:?}  {}  {}\n", f.kind, f.subject, f.detail));
        }
    }
    if !report.policy_findings.is_empty() {
        out.push_str("\nPOLICY FINDINGS\n");
        for f in &report.policy_findings {
            out.push_str(&format!("  {:?}  {}  {}\n", f.kind, f.subject, f.detail));
        }
    }

    out.trim_end().to_string()
}

/// `factory scenario show <name> [--scope S] [--json]`: one scenario's
/// whole computed answer, laid out in the order the issue's own display
/// section lists it -- forecast, drivers, policy delta, goal scenarios,
/// signposts.
fn scenario_detail_text(report: &ScenariosReport, s: &ScenarioResult) -> String {
    let mut out = format!("{}  {}\n", s.scenario.name, s.scenario.title);
    if let Some(a) = &s.scenario.assumptions {
        out.push_str(&format!("  {}\n", a.trim()));
    }
    out.push_str(&format!("  kind: {}   horizon: {}\n", s.scenario.kind.join(", "), s.scenario.horizon));

    out.push_str("\nFORECAST\n");
    out.push_str(&format!(
        "  backlog: {} (newly-open controls: {}, open goal tasks: {})\n",
        s.backlog.total, s.backlog.newly_open_controls, s.backlog.open_goal_tasks
    ));
    out.push_str(&format!("  scenario: {}\n", forecast_summary(&s.forecast)));
    out.push_str(&format!("  baseline: {}\n", forecast_summary(&report.baseline.forecast)));

    out.push_str("\nDRIVERS\n");
    let mut ids: Vec<&String> = report.baseline.drivers.keys().chain(s.drivers.overridden.keys()).collect();
    ids.sort();
    ids.dedup();
    for id in ids {
        let base = report.baseline.drivers.get(id).map(|v| format!("{v:.3}")).unwrap_or_else(|| "--".to_string());
        let over = s.drivers.overridden.get(id).map(|v| format!("{v:.3}")).unwrap_or_else(|| "--".to_string());
        out.push_str(&format!("  {id:<20} baseline={base:<10} scenario={over:<10}\n"));
    }
    out.push_str("  outcomes:\n");
    for (id, after) in &s.drivers.outcomes_after {
        let before = s.drivers.outcomes_before.get(id).copied().unwrap_or(0.0);
        out.push_str(&format!("    {id:<24} {before:.3} -> {after:.3}\n"));
    }
    if !s.drivers.tornado.is_empty() {
        out.push_str("  tornado:\n");
        for bar in &s.drivers.tornado {
            out.push_str(&format!(
                "    {:<20} {:.3} .. {:.3}  (span {:.3})\n",
                bar.driver, bar.low_outcome, bar.high_outcome, bar.span
            ));
        }
    }

    if !s.policy.is_empty() || s.policy_subtree.unchanged > 0 || !s.policy_subtree.newly_open.is_empty() {
        out.push_str("\nPOLICY DELTA\n");
        out.push_str(&format!("  subtree: {}\n", policy_delta_line(&s.policy_subtree)));
        for row in &s.policy {
            out.push_str(&format!("  {:<20} {}\n", row.scope, policy_delta_line(&row.delta)));
        }
    }

    if !s.goals.is_empty() {
        out.push_str("\nGOAL SCENARIOS\n");
        for g in &s.goals {
            let p = match g.probability.probability {
                Some(p) => format!("p={p:.2}"),
                None => "p=n/a".to_string(),
            };
            let target = g.target.map(|t| format!("{t}")).unwrap_or_else(|| "--".to_string());
            let by = g.by.map(|b| b.to_string()).unwrap_or_else(|| "--".to_string());
            let reason = g.probability.reason.as_deref().map(|r| format!("  ({r})")).unwrap_or_default();
            out.push_str(&format!("  {:<20} target={target} by={by}  {p}{reason}\n", g.kr.to_string()));
        }
    }

    if !s.signposts.is_empty() {
        out.push_str("\nSIGNPOSTS\n");
        for (status, def) in s.signposts.iter().zip(&s.scenario.signposts) {
            out.push_str(&format!("  {:<20} [{}]  {}\n", def.metric, signpost_state_str(status.state), status.reason));
        }
    }

    if !s.findings.is_empty() {
        out.push_str("\nFINDINGS\n");
        for f in &s.findings {
            out.push_str(&format!("  {:?}  {}\n", f.kind, f.detail));
        }
    }

    out.trim_end().to_string()
}

fn scenario_promote_text(r: &ScenarioPromoteResult) -> String {
    let mut out = format!("scenario {} -> scope {}\n", r.scenario, r.scope);
    if r.created.is_empty() {
        out.push_str("  created: none\n");
    } else {
        out.push_str("  created:\n");
        for c in &r.created {
            out.push_str(&format!("    {}  {}  \"{}\"\n", c.control, c.task.id, c.task.title));
        }
    }
    if !r.skipped.is_empty() {
        out.push_str("  skipped (already an open task for it):\n");
        for sk in &r.skipped {
            out.push_str(&format!("    {}  existing task {}\n", sk.control, sk.existing_task));
        }
    }
    out.trim_end().to_string()
}

/// `--window 7d|30d`, spelled the way the wire spells it.
fn parse_window(text: &str) -> Result<HealthWindow> {
    serde_json::from_value(serde_json::Value::String(text.to_string()))
        .map_err(|_| anyhow!("--window takes 7d or 30d, not {text:?}"))
}

async fn stats_cmd(json: bool, client: &Client, cmd: StatsCmd) -> Result<()> {
    let (scope, window, attention_only) = match cmd {
        StatsCmd::Summary { scope, window } => (scope, window, false),
        StatsCmd::Attention { scope } => (scope, None, true),
    };
    let payload = client
        .send(Request::Operations { scope, window: window.unwrap_or_default(), detail: false })
        .await?;
    print(&payload, json, |p| match p {
        Payload::Operations { report } if attention_only => Some(attention_text(report)),
        Payload::Operations { report } => Some(stats_text(report)),
        _ => None,
    })
}

/// How many attention rows the summary shows before pointing at
/// `factory stats attention` for the rest.
const SUMMARY_ATTENTION_ROWS: usize = 5;

fn severity_str(s: ops::Severity) -> &'static str {
    match s {
        ops::Severity::High => "high",
        ops::Severity::Medium => "medium",
        ops::Severity::Low => "low",
    }
}

fn action_str(a: ops::Action) -> &'static str {
    match a {
        ops::Action::RunAgain => "run again",
        ops::Action::Cancel => "cancel",
        ops::Action::Answer => "answer",
        ops::Action::RunNow => "run now",
        ops::Action::SkipNext => "skip next",
        ops::Action::PauseSchedule => "pause schedule",
        ops::Action::ResumeSchedule => "resume schedule",
        ops::Action::Approve => "approve",
        ops::Action::Reject => "reject",
        ops::Action::AcceptRework => "accept rework",
    }
}

/// One exception as a line, then its reason and actions beneath it.
fn exception_text(e: &ops::Exception) -> String {
    let what = e
        .title
        .clone()
        .or_else(|| e.agent.clone())
        .unwrap_or_else(|| "-".into());
    let mut flags = Vec::new();
    if e.suspicion {
        flags.push("suspicion, not a status".to_string());
    }
    if e.observation {
        flags.push("observation".to_string());
    }
    if !e.also.is_empty() {
        flags.push(format!("also {}", e.also.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(", ")));
    }
    let mut out = format!(
        "  {:<6} {:<18} {:<14} {:<32} {:>8}{}\n      {}\n",
        severity_str(e.severity),
        e.kind.as_str(),
        e.scope.as_deref().unwrap_or("-"),
        what,
        duration(e.age_s as u64),
        if flags.is_empty() { String::new() } else { format!("  [{}]", flags.join("; ")) },
        e.reason,
    );
    let mut ids = Vec::new();
    if let Some(run) = &e.run_id {
        ids.push(format!("run {run}"));
    } else if let Some(task) = &e.task_id {
        ids.push(format!("task {task}"));
    }
    if !e.actions.is_empty() {
        ids.push(format!("can: {}", e.actions.iter().map(|a| action_str(*a)).collect::<Vec<_>>().join(", ")));
    }
    if !ids.is_empty() {
        out.push_str(&format!("      {}\n", ids.join("  ")));
    }
    out
}

/// `factory stats attention`: every exception, as the report sorted them.
fn attention_text(report: &OperationsReport) -> String {
    if report.attention.is_empty() {
        return "Nothing needs you.".into();
    }
    let mut out = format!("NEEDS ATTENTION  {}\n", report.attention.len());
    for e in &report.attention {
        out.push_str(&exception_text(e));
    }
    out.trim_end().to_string()
}

/// A figure, or why there is none. `fmt` formats a present value.
fn figure(f: &ops::Figure, fmt: impl Fn(f64) -> String) -> String {
    match f.value {
        Some(v) => fmt(v),
        None => format!("-- ({})", f.reason.as_deref().unwrap_or("no data")),
    }
}

fn secs(v: f64) -> String {
    duration(v.round() as u64)
}

fn pct(v: f64) -> String {
    format!("{:.0}%", v * 100.0)
}

/// `factory stats` / `factory stats summary`.
fn stats_text(report: &OperationsReport) -> String {
    let mut out = format!(
        "OPERATIONS  {}  as of {}\n\n",
        report.scope.as_deref().unwrap_or("every scope"),
        report.generated_at.format("%Y-%m-%d %H:%M UTC")
    );

    if report.attention.is_empty() {
        out.push_str("NEEDS ATTENTION  none -- nothing needs you\n");
    } else {
        out.push_str(&format!("NEEDS ATTENTION  {}\n", report.attention.len()));
        for e in report.attention.iter().take(SUMMARY_ATTENTION_ROWS) {
            out.push_str(&exception_text(e));
        }
        if report.attention.len() > SUMMARY_ATTENTION_ROWS {
            out.push_str(&format!(
                "  ... and {} more: factory stats attention\n",
                report.attention.len() - SUMMARY_ATTENTION_ROWS
            ));
        }
    }

    out.push_str("\nFLOW NOW\n");
    if report.flow.is_empty() {
        out.push_str("  nothing in flight, queued or retrying\n");
    } else {
        out.push_str(&format!(
            "  {:<14} {:>6} {:>6} {:>7} {:>7} {:>8}  {:<22} {}\n",
            "scope", "queued", "disp.", "running", "blocked", "retrying", "wait p50 / p95", "sessions"
        ));
        for f in &report.flow {
            out.push_str(&format!(
                "  {:<14} {:>6} {:>6} {:>7} {:>7} {:>8}  {:<22} {}\n",
                f.scope,
                f.wip.queued,
                f.wip.dispatching,
                f.wip.running,
                f.wip.blocked,
                f.retrying,
                format!(
                    "{} / {}",
                    f.wait_p50.value.map(secs).unwrap_or_else(|| "--".into()),
                    f.wait_p95.value.map(secs).unwrap_or_else(|| "--".into())
                ),
                match f.sessions_max {
                    Some(max) => format!("{} of {max}", f.sessions_in_use),
                    None => format!("{} (no limit declared)", f.sessions_in_use),
                },
            ));
        }
    }

    out.push_str("\nAGING WIP  oldest first\n");
    if report.aging.items.is_empty() {
        out.push_str("  nothing in progress\n");
    }
    for item in &report.aging.items {
        let pace = match (item.pace, item.basis) {
            (Some(p), basis) => format!(
                "{} ({})",
                serde_json::to_value(p).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(),
                match basis {
                    ops::PaceBasis::Task => "vs this task",
                    _ => "vs its scope",
                }
            ),
            (None, ops::PaceBasis::NotPaced) => "not paced yet".into(),
            (None, _) => "not enough history".into(),
        };
        let stage = serde_json::to_value(item.stage).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        out.push_str(&format!(
            "  {:<14} {:<32} {:<11} {:>8}  {}\n",
            item.scope,
            item.title,
            stage,
            secs(item.age_s),
            pace
        ));
    }

    let h = &report.health;
    let days = h.window.days();
    out.push_str(&format!("\nHEALTH  last {days}d  (the {days}d before it)\n"));
    let row = |name: &str, now: String, before: String| format!("  {name:<20} {now:<36} ({before})\n");
    // The window before only needs its number: why the current one has
    // none is said once, beside it.
    let was = |f: &ops::Figure, fmt: &dyn Fn(f64) -> String| f.value.map(fmt).unwrap_or_else(|| "--".into());
    let (c, p) = (&h.current, &h.previous);
    out.push_str(&row("finished", c.finished.to_string(), p.finished.to_string()));
    out.push_str(&row("throughput / day", figure(&c.throughput_day, |v| format!("{v:.1}")), was(&p.throughput_day, &|v| format!("{v:.1}"))));
    out.push_str(&row("cycle time p50", figure(&c.cycle_p50, secs), was(&p.cycle_p50, &secs)));
    out.push_str(&row("cycle time p85", figure(&c.cycle_p85, secs), was(&p.cycle_p85, &secs)));
    out.push_str(&row("first-pass yield", figure(&c.first_pass_yield, pct), was(&p.first_pass_yield, &pct)));
    out.push_str(&row("rework rate", figure(&c.rework_rate, pct), was(&p.rework_rate, &pct)));
    out.push_str(&row("scrap rate", figure(&c.scrap_rate, pct), was(&p.scrap_rate, &pct)));
    out.push_str(&row("fail rate", figure(&c.fail_rate, pct), was(&p.fail_rate, &pct)));
    out.push_str(&row("time to recover p50", figure(&c.recover_p50, secs), was(&p.recover_p50, &secs)));
    out.push_str(&row("queue wait p95", figure(&c.queue_wait_p95, secs), was(&p.queue_wait_p95, &secs)));
    out.push_str(&row(
        "interventions / 100",
        figure(&c.interventions_per_100, |v| format!("{v:.1} ({} in all)", c.interventions)),
        was(&p.interventions_per_100, &|v| format!("{v:.1}")),
    ));
    if !c.fail_by_kind.is_empty() || c.unclassified > 0 {
        let mut kinds: Vec<String> = c
            .fail_by_kind
            .iter()
            .map(|(k, n)| format!("{} {n}", serde_json::to_value(k).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()))
            .collect();
        if c.unclassified > 0 {
            kinds.push(format!("unclassified {}", c.unclassified));
        }
        out.push_str(&format!("  scrapped by why      {}\n", kinds.join(", ")));
    }

    if !report.schedules.is_empty() {
        out.push_str("\nSCHEDULES\n");
        for row in &report.schedules {
            let state = serde_json::to_value(row.state).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
            out.push_str(&format!(
                "  {:<14} {:<32} {:<8} {} ({})  next {}{}\n",
                row.scope,
                row.title,
                state,
                row.schedule,
                row.timezone.as_deref().unwrap_or("UTC"),
                row.next_run_at.map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string()).unwrap_or_else(|| "--".into()),
                if row.skipped > 0 { format!("  ({} slot(s) passed over)", row.skipped) } else { String::new() },
            ));
        }
    }

    match report.recorded_since {
        Some(since) => out.push_str(&format!(
            "\nqueue waits, slots and fail kinds are on record from {}\n",
            since.format("%Y-%m-%d")
        )),
        None => out.push_str("\nno run records queue waits, slots or fail kinds yet\n"),
    }
    out.trim_end().to_string()
}

async fn bench_cmd(json: bool, client: &Client, cmd: BenchCmd) -> Result<()> {
    match cmd {
        BenchCmd::Run { dataset, agents, attempts, concurrency, cases } => {
            let payload = client
                .send(Request::BenchRunStart {
                    dataset,
                    agents,
                    attempts: Some(attempts),
                    concurrency: Some(concurrency),
                    cases: if cases.is_empty() { None } else { Some(cases) },
                })
                .await?;
            print(&payload, json, |p| match p {
                Payload::BenchRun { run, results } => Some(bench_run_detail(run, results)),
                _ => None,
            })
        }
        BenchCmd::Runs { dataset } => {
            let payload = client.send(Request::BenchRuns { dataset }).await?;
            print(&payload, json, |p| match p {
                Payload::BenchRuns { runs } => Some(if runs.is_empty() {
                    "no bench runs".to_string()
                } else {
                    runs.iter().map(bench_run_line).collect::<Vec<_>>().join("\n")
                }),
                _ => None,
            })
        }
        BenchCmd::Show { id } => {
            let payload = client.send(Request::BenchRunGet { id }).await?;
            print(&payload, json, |p| match p {
                Payload::BenchRun { run, results } => Some(bench_run_detail(run, results)),
                _ => None,
            })
        }
        BenchCmd::Cancel { id } => {
            let payload = client.send(Request::BenchRunCancel { id }).await?;
            print(&payload, json, |p| match p {
                Payload::BenchRun { run, results } => Some(bench_run_detail(run, results)),
                _ => None,
            })
        }
        BenchCmd::Clean { id } => {
            let payload = client.send(Request::BenchRunClean { id }).await?;
            print(&payload, json, |p| match p {
                Payload::BenchRun { run, .. } => Some(format!(
                    "cleaned {} worktree(s) for {}",
                    run.attempts.iter().filter(|a| a.run_id.is_some()).count(),
                    run.id
                )),
                _ => None,
            })
        }
    }
}

fn knowledge_write_text(p: &Payload) -> Option<String> {
    match p {
        Payload::KnowledgeWrite { copied, skipped_existing, skipped_hidden, refused, truncated } => {
            let mut out = format!(
                "{} copied, {} skipped (existing), {} skipped (hidden), {} refused",
                copied.len(),
                skipped_existing.len(),
                skipped_hidden.len(),
                refused.len(),
            );
            if *truncated {
                out.push_str(" -- truncated: stopped past the 10,000-file cap");
            }
            out.push('\n');
            for path in copied {
                out.push_str(&format!("  copied    {path}\n"));
            }
            for path in skipped_existing {
                out.push_str(&format!("  existing  {path}\n"));
            }
            for r in refused {
                match &r.path {
                    Some(path) => out.push_str(&format!("  refused   {path} -- {}\n", r.reason)),
                    None => out.push_str(&format!("  refused   {}\n", r.reason)),
                }
            }
            Some(out.trim_end().to_string())
        }
        _ => None,
    }
}

fn bench_run_line(r: &BenchRun) -> String {
    let settled = r.attempts.iter().filter(|a| a.verdict.is_some()).count();
    format!(
        "{}  {}@{}  agents={}  {:<9} {}/{} settled",
        &r.id[..8.min(r.id.len())],
        r.dataset,
        r.dataset_revision,
        r.agents.join(","),
        r.status.as_str(),
        settled,
        r.attempts.len(),
    )
}

fn bench_run_detail(r: &BenchRun, results: &[BenchResult]) -> String {
    let settled = r.attempts.iter().filter(|a| a.verdict.is_some()).count();
    let mut out = format!(
        "{}\n  dataset    {}@{}\n  agents     {}\n  status     {}\n  progress   {}/{} settled\n  started    {}\n",
        r.id,
        r.dataset,
        r.dataset_revision,
        r.agents.join(", "),
        r.status.as_str(),
        settled,
        r.attempts.len(),
        r.started_at.to_rfc3339(),
    );
    if let Some(ended) = r.ended_at {
        out.push_str(&format!("  ended      {}\n", ended.to_rfc3339()));
    }
    out.push_str("\nRESULTS\n");
    for res in results {
        out.push_str(&format!(
            "  {:<40} attempts={:<3} pass={:<3} fail={:<3} unverified={:<3} skipped={:<3} cancelled={:<3} error={:<3} resolve_rate={} mean_wall_clock={} cost=not recorded\n",
            res.label,
            res.attempts,
            res.pass,
            res.fail,
            res.unverified,
            res.skipped,
            res.cancelled,
            res.error,
            res.resolve_rate.map(|r| format!("{r:.3}")).unwrap_or_else(|| "n/a".into()),
            res.mean_wall_clock_seconds.map(|s| format!("{s:.0}s")).unwrap_or_else(|| "n/a".into()),
        ));
    }
    out.push_str("\nATTEMPTS\n");
    for a in &r.attempts {
        out.push_str(&format!(
            "  {:<24} {:<12} #{:<2} {:<10} {}\n",
            a.case_id,
            a.agent,
            a.attempt,
            a.verdict.map(verdict_str).unwrap_or("pending"),
            a.task_id.as_deref().unwrap_or("--"),
        ));
        if let Some(reason) = &a.reason {
            out.push_str(&format!("      {reason}\n"));
        }
    }
    out.trim_end().to_string()
}

fn verdict_str(v: Verdict) -> &'static str {
    v.as_str()
}

async fn agent_cmd(json: bool, client: &Client, cmd: AgentCmd) -> Result<()> {
    match cmd {
        AgentCmd::Start { scope, name } => {
            let payload = client.send(Request::AgentStart { scope, name }).await?;
            print(&payload, json, |p| match p {
                Payload::Agent { agent } => Some(agent_line(agent)),
                _ => None,
            })
        }
        AgentCmd::Stop { id } => {
            let payload = client.send(Request::AgentStop { id }).await?;
            print(&payload, json, |p| match p {
                Payload::Agent { agent } => Some(agent_line(agent)),
                _ => None,
            })
        }
        AgentCmd::Role { id, role } => {
            let payload = client.send(Request::AgentRole { id, role }).await?;
            print(&payload, json, |p| match p {
                Payload::Agent { agent } => Some(format!(
                    "{}  {}{}",
                    agent.id,
                    agent.role,
                    if agent.assigned_role.is_some() {
                        " (given)"
                    } else {
                        " (from the config)"
                    }
                )),
                _ => None,
            })
        }
        AgentCmd::Output { id, lines } => {
            let payload = client
                .send(Request::AgentOutput {
                    id,
                    lines: Some(lines),
                })
                .await?;
            print(&payload, json, |p| match p {
                Payload::Text { text } => Some(if text.trim().is_empty() {
                    "(nothing on its terminal)".into()
                } else {
                    text.clone()
                }),
                _ => None,
            })
        }
        AgentCmd::Input { id, text, keys } => {
            if text.is_none() && keys.is_empty() {
                return Err(anyhow!("nothing to send; pass --text or --key"));
            }
            client.send(Request::AgentInput { id, text, keys }).await?;
            println!("sent");
            Ok(())
        }
    }
}

fn agent_line(a: &factory_core::agent::AgentSession) -> String {
    let mut s = format!(
        "{}  {:<9} {} on {}",
        a.id,
        a.state.as_str(),
        a.agent,
        a.runtime
    );
    if let Some(cmd) = &a.attach {
        s.push_str(&format!("\n  attach: {cmd}"));
    }
    if let Some(err) = &a.error {
        s.push_str(&format!("\n  error:  {err}"));
    }
    s
}

async fn run_cmd(json: bool, client: &Client, cmd: RunCmd) -> Result<()> {
    match cmd {
        RunCmd::List { task_id, limit } => {
            let payload = client
                .send(Request::RunList {
                    task_id: need_id(task_id)?,
                    limit: Some(limit),
                })
                .await?;
            print(&payload, json, |p| match p {
                Payload::Runs { runs } => Some(if runs.is_empty() {
                    "no runs yet".into()
                } else {
                    runs.iter().map(run_line).collect::<Vec<_>>().join("\n")
                }),
                _ => None,
            })
        }
        RunCmd::Show { id } => {
            let payload = client.send(Request::RunGet { id }).await?;
            print(&payload, json, |p| match p {
                Payload::Run { run } => Some(run_detail(run)),
                _ => None,
            })
        }
        RunCmd::Attestations { id } => {
            let payload = client.send(Request::RunAttestations { id }).await?;
            print(&payload, json, |p| match p {
                Payload::Attestations { attestations } => Some(attestations_text(attestations)),
                _ => None,
            })
        }
        RunCmd::Provenance { id } => {
            let payload = client.send(Request::RunProvenance { id }).await?;
            print(&payload, json, |p| match p {
                Payload::RunProvenance { records } => Some(if records.is_empty() {
                    "no published artifact provenance".into()
                } else {
                    records.iter().map(|r| format!("{}  sha256:{}  {} bytes", r.artifact.name, r.artifact.sha256, r.artifact.size_bytes)).collect::<Vec<_>>().join("\n")
                }),
                _ => None,
            })
        }
        RunCmd::Approve { id, reason } => {
            let payload = client
                .send(Request::RunApprove {
                    id: id.clone(),
                    reason,
                })
                .await?;
            print(&payload, json, |p| match p {
                Payload::Run { run } => Some(format!("approved {id}: {}", run.status.as_str())),
                _ => None,
            })
        }
        RunCmd::Reject { id, reason } => {
            let payload = client
                .send(Request::RunReject {
                    id: id.clone(),
                    reason,
                })
                .await?;
            print(&payload, json, |p| match p {
                Payload::Run { run } => Some(format!("rejected {id}: {}", run.status.as_str())),
                _ => None,
            })
        }
        RunCmd::Rework { id } => {
            let payload = client.send(Request::RunRework { id: id.clone() }).await?;
            print(&payload, json, |p| match p {
                Payload::Run { run } => {
                    Some(format!("accepted rework for {id}: {}", run.status.as_str()))
                }
                _ => None,
            })
        }
        RunCmd::Usage { id } => {
            let payload = client.send(Request::RunUsage { id }).await?;
            print(&payload, json, |p| match p {
                Payload::UsageSnapshots { snapshots } => Some(snapshots_text(snapshots)),
                _ => None,
            })
        }
        RunCmd::Log { id, limit } => {
            let payload = client
                .send(Request::RunEntries {
                    id,
                    limit: Some(limit),
                })
                .await?;
            print(&payload, json, |p| match p {
                Payload::Entries { entries } => Some(entries_text(entries)),
                _ => None,
            })
        }
        RunCmd::Answer { id, text, reason } => {
            let payload = client.send(Request::RunAnswer { id: id.clone(), text, reason }).await?;
            print(&payload, json, |p| match p {
                Payload::Ok => Some(format!("answered {id}")),
                _ => None,
            })
        }

        RunCmd::Output { id, lines } => {
            let payload = client
                .send(Request::RunOutput {
                    id,
                    lines: Some(lines),
                })
                .await?;
            print(&payload, json, |p| match p {
                Payload::Text { text } => Some(if text.trim().is_empty() {
                    "(no terminal output yet)".into()
                } else {
                    text.clone()
                }),
                _ => None,
            })
        }
    }
}

async fn task(json: bool, client: &Client, cmd: TaskCmd) -> Result<()> {
    match cmd {
        TaskCmd::List {
            status,
            scope,
            parent,
            limit,
        } => {
            // `failed` and `closed` are not one stored status each, so the
            // daemon is asked for the nearest thing and the rest is sorted
            // out here -- and the limit applied after, not before.
            let narrowed = status.is_some_and(|f| f.narrows());
            let mut payload = client
                .send(Request::TaskList(TaskFilter {
                    status: status.and_then(StatusFilter::wire),
                    scope,
                    parent_task_id: parent,
                    limit: if narrowed { None } else { limit },
                }))
                .await?;
            if let (Some(filter), Payload::Tasks { tasks }) = (status, &mut payload) {
                tasks.retain(|t| filter.keeps(t));
                if let Some(limit) = limit {
                    tasks.truncate(limit as usize);
                }
            }
            print(&payload, json, |p| match p {
                Payload::Tasks { tasks } => Some(if tasks.is_empty() {
                    "no tasks".into()
                } else {
                    tasks.iter().map(one_line).collect::<Vec<_>>().join("\n")
                }),
                _ => None,
            })
        }

        TaskCmd::Create {
            title,
            instructions,
            scope,
            agent,
            runtime,
            schedule,
            after,
            timezone,
            estimate,
            estimate_low,
            estimate_high,
            estimate_cost_low,
            estimate_cost,
            estimate_cost_high,
            ack_timeout,
            timeout,
            blocked_timeout,
            retry,
            labels,
            worktree,
            no_worktree,
            knowledge_hints,
            category,
            run,
        } => {
            let schedule = schedule
                .as_deref()
                .map(|text| parse_schedule(text, timezone.as_deref()))
                .transpose()?;
            let retry = retry.as_deref().map(parse_retry).transpose()?;
            let estimate_range = estimate_from_args(
                estimate_low,
                estimate,
                estimate_high,
                estimate_cost_low,
                estimate_cost,
                estimate_cost_high,
            )?;
            // Absent means on -- so passing neither flag says the same thing
            // as passing `--worktree` does. `--no-worktree` is the only way
            // to mean off, and it wins if both are somehow given.
            let worktree = if no_worktree {
                Some(false)
            } else if worktree {
                Some(true)
            } else {
                None
            };
            let payload = client
                .send(Request::TaskCreate(NewTask {
                    after: (!after.is_empty()).then_some(after),
                    after_condition: None,
                    title,
                    instructions,
                    scope,
                    agent,
                    runtime,
                    parent_task_id: None,
                    decomposition_part: None,
                    depends_on: Vec::new(),
                    schedule,
                    estimate_seconds: estimate_range.as_ref().map(|value| value.time.expected),
                    estimate: estimate_range,
                    ack_timeout_seconds: ack_timeout,
                    timeout_seconds: timeout,
                    blocked_timeout_seconds: blocked_timeout,
                    retry,
                    labels: parse_labels(&labels)?,
                    worktree,
                    knowledge_hints,
                    category,
                }))
                .await?;
            let created = match &payload {
                Payload::Task { task } => task.clone(),
                _ => return Err(anyhow!("unexpected answer to task.create")),
            };
            if run {
                client
                    .send(Request::TaskRun {
                        override_wait: false,
                        id: created.id.clone(),
                        reason: None,
                        continue_run: false,
                    })
                    .await?;
            }
            print(&payload, json, |p| match p {
                Payload::Task { task } => Some(format!(
                    "{}\n{}\n{}",
                    task.id,
                    one_line(task),
                    created_note(task, run)
                )),
                _ => None,
            })
        }

        TaskCmd::Edit {
            id,
            title,
            instructions,
            scope,
            agent,
            runtime,
            schedule,
            timezone,
            no_schedule,
            estimate,
            estimate_low,
            estimate_high,
            estimate_cost_low,
            estimate_cost,
            estimate_cost_high,
            no_estimate,
            ack_timeout,
            timeout,
            blocked_timeout,
            default_timeouts,
            retry,
            default_retry,
            labels,
            knowledge_hints,
            no_knowledge_hints,
            pause_schedule,
            resume_schedule,
            reason,
            category,
            default_category,
        } => {
            let retry = retry.as_deref().map(parse_retry).transpose()?;
            let estimate_range = estimate_from_args(
                estimate_low,
                estimate,
                estimate_high,
                estimate_cost_low,
                estimate_cost,
                estimate_cost_high,
            )?;
            let patch = TaskPatch {
                title,
                instructions,
                scope,
                agent,
                runtime,
                schedule: schedule
                    .as_deref()
                    .map(|text| parse_schedule(text, timezone.as_deref()))
                    .transpose()?,
                clear_schedule: no_schedule,
                estimate_seconds: estimate_range.as_ref().map(|value| value.time.expected),
                estimate: estimate_range,
                clear_estimate: no_estimate,
                ack_timeout_seconds: ack_timeout,
                timeout_seconds: timeout,
                blocked_timeout_seconds: blocked_timeout,
                clear_ack_timeout: default_timeouts,
                clear_timeout: default_timeouts,
                clear_blocked_timeout: default_timeouts,
                retry,
                clear_retry: default_retry,
                knowledge_hints: match (knowledge_hints, no_knowledge_hints) {
                    (true, _) => Some(true),
                    (_, true) => Some(false),
                    _ => None,
                },
                schedule_paused: match (pause_schedule, resume_schedule) {
                    (true, _) => Some(true),
                    (_, true) => Some(false),
                    _ => None,
                },
                labels: if labels.is_empty() {
                    None
                } else {
                    Some(parse_labels(&labels)?)
                },
                category,
                clear_category: default_category,
                ..Default::default()
            };
            let payload = client
                .send(Request::TaskUpdate {
                    id: need_id(id)?,
                    patch,
                    reason,
                })
                .await?;
            print(&payload, json, |p| match p {
                Payload::Task { task } => Some(detail(task)),
                _ => None,
            })
        }

        TaskCmd::Show { id } => {
            let id = need_id(id)?;
            let payload = client.send(Request::TaskGet { id: id.clone() }).await?;
            // The usage block is a second read, for people only: `--json`
            // keeps answering exactly the task, as it always has. A daemon
            // that predates #117 refuses the request, and the task still
            // prints.
            let usage = if json {
                None
            } else {
                match client.send(Request::TaskUsage { id }).await {
                    Ok(Payload::TaskUsage { usage }) => Some(usage),
                    _ => None,
                }
            };
            print(&payload, json, |p| match p {
                Payload::Task { task } => Some(match &usage {
                    Some(u) if !u.runs.is_empty() => format!("{}\n\n{}", detail(task), task_usage_text(u)),
                    _ => detail(task),
                }),
                _ => None,
            })
        }

        TaskCmd::Run { id, reason, continue_run, override_wait } => {
            let id = need_id(id)?;
            client.send(Request::TaskRun { id: id.clone(), reason, continue_run, override_wait }).await?;
            if continue_run {
                println!("continuing {id}");
            } else {
                println!("dispatching {id}");
            }
            Ok(())
        }

        TaskCmd::Cancel { id, reason } => {
            let payload = client.send(Request::TaskCancel { id: need_id(id)?, reason, run: None }).await?;
            print(&payload, json, |p| match p {
                Payload::Run { run } => Some(run_line(run)),
                _ => None,
            })
        }

        TaskCmd::Close { id, reason, duplicate_of, note } => {
            let payload = client.send(Request::TaskClose { id: need_id(id)?, reason, duplicate_of, note }).await?;
            print(&payload, json, |p| match p {
                Payload::Task { task } => Some(format!(
                    "closed {} as {}",
                    task.id,
                    task.close_reason().map_or("closed", |r| r.label())
                )),
                _ => None,
            })
        }

        TaskCmd::Reopen { id, reason } => {
            let payload = client.send(Request::TaskReopen { id: need_id(id)?, reason }).await?;
            print(&payload, json, |p| match p {
                Payload::Task { task } => Some(format!("reopened {}; it is {}", task.id, task.status.as_str())),
                _ => None,
            })
        }

        TaskCmd::SkipNext { id, reason } => {
            let payload = client.send(Request::TaskSkipNext { id: need_id(id)?, reason, slot: None }).await?;
            print(&payload, json, |p| match p {
                Payload::Task { task } => Some(match task.next_run_at {
                    Some(next) => format!("skipped; next firing at {}", next.to_rfc3339()),
                    None => "skipped".to_string(),
                }),
                _ => None,
            })
        }

        TaskCmd::Delete { id } => {
            let id = need_id(id)?;
            let payload = client.send(Request::TaskDelete { id: id.clone() }).await?;
            print(&payload, json, |p| match p {
                Payload::Deleted { deleted } => Some(if *deleted {
                    format!("deleted {id}")
                } else {
                    format!("no such task: {id}")
                }),
                _ => None,
            })
        }

        TaskCmd::Log { id, limit } => {
            let payload = client
                .send(Request::TaskEntries {
                    id: need_id(id)?,
                    limit: Some(limit),
                    task_only: false,
                })
                .await?;
            print(&payload, json, |p| match p {
                Payload::Entries { entries } => Some(entries_text(entries)),
                _ => None,
            })
        }

        TaskCmd::Output { id, lines } => {
            let payload = client
                .send(Request::TaskOutput {
                    id: need_id(id)?,
                    lines: Some(lines),
                })
                .await?;
            print(&payload, json, |p| match p {
                Payload::Text { text } => Some(text.clone()),
                _ => None,
            })
        }

        TaskCmd::Report {
            id,
            status,
            message,
            result,
            send_to,
            result_file,
            artifacts,
            error,
            token,
        } => {
            if status.is_none()
                && message.is_none()
                && result.is_none()
                && send_to.is_none()
                && result_file.is_none()
                && error.is_none()
            {
                return Err(anyhow!(
                    "nothing to report; pass --status, --message, --result, --result-file, or --error"
                ));
            }
            let result = combine_result(result, result_file.as_deref())?;
            let payload = client
                .send(Request::TaskReport {
                    id: need_id(id)?,
                    report: TaskReport {
                        artifacts,
                        status,
                        message,
                        result,
                        send_to,
                        error,
                        token,
                    },
                })
                .await?;
            print(&payload, json, |p| match p {
                Payload::Run { run } => Some(format!(
                    "recorded; attempt {} is {}",
                    run.attempt,
                    run.status.as_str()
                )),
                _ => None,
            })
        }

        TaskCmd::Attach { id, kind, file, token } => {
            let token = token.ok_or_else(|| anyhow!(
                "task attach needs the run callback token in FACTORY_TASK_TOKEN or --run-token"
            ))?;
            let bytes = std::fs::read(&file)
                .with_context(|| format!("reading {}", file.display()))?;
            let filename = file.file_name().and_then(|n| n.to_str()).unwrap_or("attachment.cdx.json").to_string();
            let payload = client.send(Request::TaskAttach {
                id: need_id(id)?, token, kind, filename, bytes,
            }).await?;
            print(&payload, json, |p| match p {
                Payload::Attachment { attachment } => Some(format!(
                    "attached {} for run {} attempt {}", attachment.kind, attachment.run_id, attachment.attempt
                )),
                _ => None,
            })
        }

        // Every failure is written to stderr (Claude Code keeps an async
        // hook's in its debug log) and swallowed: nothing this can say would
        // be seen by anyone who could act on it mid-turn, and the daemon's
        // own timeouts still stand behind it if the call never lands.
        TaskCmd::TurnEnded { id, event, token } => {
            let hook = read_hook_input().await;
            let turn = turn_ended_from_hook(event, &hook, token);
            let sent = match need_id(id) {
                Ok(id) => client.send(Request::TaskTurnEnded { id, turn }).await.map(|_| ()),
                Err(e) => Err(e),
            };
            if let Err(e) = sent {
                eprintln!("factory task turn-ended: {e}");
            }
            Ok(())
        }
    }
}

fn dependencies_text(report: &DependenciesReport) -> String {
    let mut out = format!("{} dependencies\n", report.scope);
    if report.documents.is_empty() { out.push_str("  no scans\n"); }
    for document in &report.documents {
        let identity = document
            .sbom
            .identity
            .as_ref()
            .map(|identity| format!(" v{} {}", identity.version, identity.git_sha))
            .unwrap_or_default();
        out.push_str(&format!(
            "  {:<8} {}{}{}\n", document.state, document.sbom.attachment.attached_at, identity,
            if document.vulnerabilities.is_some() { " + vulnerabilities" } else { "" }
        ));
    }
    if !report.findings.is_empty() {
        out.push_str("findings\n");
        for finding in &report.findings {
            out.push_str(&format!(
                "  {:<10} {:<8} {} {} {}\n",
                format!("{:?}", finding.status).to_ascii_lowercase(),
                finding.severity.as_str(), finding.id, finding.affected.name, finding.state
            ));
            out.push_str(&format!(
                "    reported reachability: {}\n",
                finding.reachability.as_deref().unwrap_or("unknown")
            ));
            if let Some(detail) = &finding.analysis_detail {
                out.push_str(&format!("    analysis detail: {detail}\n"));
            }
            if let Some(scan) = &finding.analysis_scan {
                out.push_str(&format!(
                    "    analysis evidence: document {} run {} at {}\n",
                    scan.attachment.id, scan.attachment.run_id, scan.attachment.attached_at
                ));
            }
        }
    }
    if !report.services.is_empty() {
        out.push_str("services\n");
        for service in &report.services {
            out.push_str(&format!("  {} ({:?})", service.service.name, service.service.transport));
            if let Some(credential) = &service.service.credential {
                out.push_str(&format!(" credential {credential}: {}", match service.credential_present {
                    Some(true) => "present", Some(false) => "absent", None => "unknown",
                }));
            }
            out.push('\n');
        }
    }
    out.push_str("sandbox service observations (partial enforcement evidence, not a verdict)\n");
    match &report.service_evidence {
        None => out.push_str("  unknown: no sandbox evidence in this response\n"),
        Some(evidence) => {
            if evidence.captures.iter().all(|capture| capture.accesses.is_empty()) {
                out.push_str("  unknown: no supported access records in the available evidence\n");
            }
            for capture in &evidence.captures {
                out.push_str(&format!("  run {} task {} agent {} sandbox {}\n    {} captured {} (partial)\n",
                    capture.run_id, capture.task_id, capture.agent, capture.sandbox, capture.source, capture.captured_at));
                if let Some(issue) = &capture.issue { out.push_str(&format!("    {issue}\n")); }
                for access in &capture.accesses {
                    out.push_str(&format!("    {} {:?} {} {:?} process {} policy {}\n", access.at,
                        access.transport, access.target, access.disposition,
                        access.process.as_deref().unwrap_or("unknown"), access.policy.as_deref().unwrap_or("unknown")));
                }
            }
            for finding in &evidence.findings { out.push_str(&format!("  {finding}\n")); }
        }
    }
    out.push_str("  Socket and file use are unknown without actual access records; configured paths and listeners are not observations.\n");
    out.trim_end().to_string()
}

/// The JSON a hook is handed on stdin, or `Null` when there is none to read
/// -- run by hand at a terminal, or piped something that is not JSON. Bounded
/// in time: Claude Code writes the payload and closes the pipe, and a hook
/// left waiting on one that never closes would be a process nobody reaps.
async fn read_hook_input() -> serde_json::Value {
    use std::io::IsTerminal;
    use tokio::io::AsyncReadExt;
    if std::io::stdin().is_terminal() {
        return serde_json::Value::Null;
    }
    let mut buf = Vec::new();
    let read = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::io::stdin().read_to_end(&mut buf),
    )
    .await;
    match read {
        Ok(Ok(_)) => serde_json::from_slice(&buf).unwrap_or(serde_json::Value::Null),
        _ => serde_json::Value::Null,
    }
}

/// The most of the agent's last message a turn-end call carries. The daemon
/// caps it again before it lands in a run's `error`.
const HOOK_LAST_MESSAGE_BYTE_CAP: usize = 4096;

/// Translate a Claude Code hook payload into what the daemon is told. Pure,
/// so the payload's actual shape -- captured from the installed `claude`,
/// which does not always match its own documentation -- can be pinned in a
/// test:
///
/// * `last_assistant_message` is a plain string on the wire; the documented
///   `{ "type": "text", "text": ... }` object is accepted too.
/// * `background_tasks` lists what the harness will wake the agent for,
///   each entry with a `status`; one still `running` (or in any state not
///   known to be over) means the turn paused rather than finished.
///   `session_crons` wake it too, so every one of those counts.
/// * `error` and `error_details` are `StopFailure`'s.
fn turn_ended_from_hook(
    event: HookEvent,
    hook: &serde_json::Value,
    token: Option<String>,
) -> factory_core::task::TurnEnded {
    use factory_core::task::{TurnEndEvent, TurnEnded};
    const OVER: [&str; 6] = ["completed", "failed", "killed", "stopped", "cancelled", "done"];

    let text = |key: &str| {
        hook.get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let background = hook
        .get("background_tasks")
        .and_then(|v| v.as_array())
        .map(|tasks| {
            tasks
                .iter()
                .filter(|t| {
                    let status = t.get("status").and_then(|s| s.as_str()).unwrap_or("");
                    !OVER.contains(&status)
                })
                .count()
        })
        .unwrap_or(0);
    let crons = hook
        .get("session_crons")
        .and_then(|v| v.as_array())
        .map(Vec::len)
        .unwrap_or(0);
    let last_message = match hook.get("last_assistant_message") {
        Some(serde_json::Value::String(s)) => Some(s.as_str()),
        Some(v) => v.get("text").and_then(|t| t.as_str()),
        None => None,
    }
    .map(str::trim)
    .filter(|s| !s.is_empty())
    .map(|s| factory_core::adapter::agent::truncate_tail(s, HOOK_LAST_MESSAGE_BYTE_CAP).into_owned());

    TurnEnded {
        event: match event {
            HookEvent::Stop => TurnEndEvent::Stop,
            HookEvent::StopFailure => TurnEndEvent::StopFailure,
        },
        pending_background: u32::try_from(background + crons).unwrap_or(u32::MAX),
        error: text("error"),
        error_details: text("error_details"),
        last_message,
        token: token.filter(|t| !t.is_empty()),
        // `#178`: Claude Code's Stop/StopFailure payload always names its
        // own session -- kept on the run as `--continue`'s fallback source
        // for which session to resume.
        session_id: text("session_id"),
    }
}

/// An agent's own task id is in its environment, so it need not repeat it.
fn need_id(id: Option<String>) -> Result<String> {
    id.or_else(|| std::env::var("FACTORY_TASK_ID").ok().filter(|s| !s.is_empty()))
        .ok_or_else(|| anyhow!("no task id given and FACTORY_TASK_ID is not set"))
}

/// The most `--result-file` will contribute to a report -- the tail, since
/// the end of a command's output is usually the part that matters, kept
/// small enough that a runaway command cannot bloat a task record.
const RESULT_FILE_BYTE_CAP: usize = 64 * 1024;

/// `--result` and `--result-file` combine rather than conflict: `--result`
/// is a short header, `--result-file`'s content is the body, joined by a
/// blank line -- except when the file is empty, where the header alone is
/// the result and there is no trailing blank line. A caller that passes only
/// one of the two behaves exactly as if the other did not exist.
fn combine_result(header: Option<String>, path: Option<&Path>) -> Result<Option<String>> {
    let Some(path) = path else { return Ok(header) };
    let body = read_result_file(path)?;
    Ok(Some(match (header, body.is_empty()) {
        (Some(header), false) => format!("{header}\n\n{body}"),
        (Some(header), true) => header,
        (None, _) => body,
    }))
}

/// Read `--result-file`'s content as the report's body. The file is
/// arbitrary command output, not guaranteed to be valid UTF-8, so it is
/// decoded lossily rather than rejected outright.
fn read_result_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).map_err(|e| anyhow!("reading {}: {e}", path.display()))?;
    Ok(tail_lossy(&bytes, RESULT_FILE_BYTE_CAP))
}

/// Keep at most `max_bytes` of `bytes`' tail and decode it lossily, marking
/// the cut when there is one. `start` is walked forward past any UTF-8
/// continuation bytes so a valid multi-byte character at the front of the
/// kept slice survives intact; `from_utf8_lossy` replaces anything actually
/// invalid with U+FFFD regardless.
fn tail_lossy(bytes: &[u8], max_bytes: usize) -> String {
    if bytes.len() <= max_bytes {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let marker = format!("[... truncated to the last {max_bytes} bytes ...]\n");
    let keep = max_bytes.saturating_sub(marker.len());
    let mut start = bytes.len().saturating_sub(keep);
    while start < bytes.len() && bytes[start] & 0xC0 == 0x80 {
        start += 1;
    }
    format!("{marker}{}", String::from_utf8_lossy(&bytes[start..]))
}

fn parse_labels(pairs: &[String]) -> Result<std::collections::BTreeMap<String, String>> {
    parse_pairs(pairs, "labels")
}

fn parse_pairs(pairs: &[String], what: &str) -> Result<std::collections::BTreeMap<String, String>> {
    pairs
        .iter()
        .map(|p| {
            p.split_once('=')
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                .ok_or_else(|| anyhow!("{what} look like key=value, not {p:?}"))
        })
        .collect()
}

/// `timezone` is `--timezone`'s value. Whether it names a real zone is the
/// daemon's to say, where every way of setting a schedule is checked alike.
fn parse_schedule(text: &str, timezone: Option<&str>) -> Result<Schedule> {
    let text = text.trim();
    let timezone = timezone.map(str::trim).filter(|t| !t.is_empty());
    if let Some(rest) = text.strip_prefix("every ").or_else(|| text.strip_prefix("every")) {
        if timezone.is_some() {
            return Err(anyhow!(
                "an `every` schedule is an interval, and no timezone changes it; --timezone is for a cron schedule"
            ));
        }
        let seconds = parse_duration_seconds(rest.trim())
            .map_err(|_| anyhow!("`every` wants a number, as in `every 300` or `every 5m`"))?;
        return Ok(Schedule::Every { seconds });
    }
    Ok(Schedule::Cron(CronSchedule {
        expr: text.strip_prefix("cron ").unwrap_or(text).trim().to_string(),
        timezone: timezone.map(str::to_string),
    }))
}

/// A plain number of seconds, or one suffixed `s`/`m`/`h` -- `300`, `5m`,
/// `1h`. Shared by `parse_schedule`'s `every` form and `parse_retry`'s
/// backoff, so the two do not drift into accepting slightly different
/// spellings of the same thing.
fn parse_duration_seconds(text: &str) -> Result<u64> {
    let (digits, unit): (String, String) = text.chars().partition(|c| c.is_ascii_digit());
    let n: u64 = digits.parse().map_err(|_| anyhow!("{text:?} is not a duration"))?;
    let unit = unit.trim().to_lowercase();
    Ok(match unit.as_str() {
        "" | "s" | "sec" | "secs" | "second" | "seconds" => n,
        "m" | "min" | "mins" | "minute" | "minutes" => n * 60,
        "h" | "hr" | "hrs" | "hour" | "hours" => n * 3600,
        other => return Err(anyhow!("unknown unit {other:?}; use s, m, or h")),
    })
}

/// `none`, or `<attempts>x<backoff>` as in `3x5m` -- three retries, five
/// minutes apart.
fn parse_retry(text: &str) -> Result<RetryPolicy> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("none") {
        return Ok(RetryPolicy::None);
    }
    let (attempts, backoff) = text
        .split_once('x')
        .ok_or_else(|| anyhow!("retry wants `none` or `<attempts>x<backoff>`, as in `3x5m`"))?;
    let max_attempts: u32 = attempts
        .trim()
        .parse()
        .map_err(|_| anyhow!("retry's attempt count must be a number, as in `3x5m`"))?;
    let backoff_seconds = parse_duration_seconds(backoff.trim())?;
    Ok(RetryPolicy::Backoff { max_attempts, backoff_seconds })
}

fn estimate_from_args(
    low: Option<u64>,
    expected: Option<u64>,
    high: Option<u64>,
    cost_low: Option<f64>,
    cost_expected: Option<f64>,
    cost_high: Option<f64>,
) -> Result<Option<factory_core::task::Estimate>> {
    let any_time = low.is_some() || expected.is_some() || high.is_some();
    let any_cost = cost_low.is_some() || cost_expected.is_some() || cost_high.is_some();
    if any_cost && !any_time {
        return Err(anyhow!("a cost estimate needs a time estimate too"));
    }
    if !any_time {
        return Ok(None);
    }
    let expected = expected.ok_or_else(|| anyhow!("--estimate is required as the expected duration"))?;
    let time = match (low, high) {
        (None, None) => factory_core::task::TimeEstimateRange::point(expected),
        (Some(low), Some(high)) => factory_core::task::TimeEstimateRange { low, expected, high },
        _ => return Err(anyhow!("--estimate-low and --estimate-high must be given together")),
    };
    let cost = match (cost_low, cost_expected, cost_high) {
        (None, None, None) => None,
        (Some(low), Some(expected), Some(high)) => {
            Some(factory_core::task::CostEstimateRange { low, expected, high })
        }
        _ => {
            return Err(anyhow!(
                "--estimate-cost-low, --estimate-cost and --estimate-cost-high must all be given together"
            ))
        }
    };
    let estimate = factory_core::task::Estimate { time, cost };
    estimate.validate().map_err(anyhow::Error::msg)?;
    Ok(Some(estimate))
}

/// What `--status` takes on `task list`: a stored status, or one of the two
/// a person asks for that are not one (`#122`) -- `failed`, a task blocked
/// by a failed run, and `closed`, done or cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusFilter {
    Is(TaskStatus),
    Failed,
    Closed,
}

impl std::str::FromStr for StatusFilter {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(match s {
            "failed" => Self::Failed,
            "closed" => Self::Closed,
            other => Self::Is(other.parse()?),
        })
    }
}

impl StatusFilter {
    /// The status the daemon is asked for.
    fn wire(self) -> Option<TaskStatus> {
        match self {
            Self::Is(status) => Some(status),
            Self::Failed => Some(TaskStatus::Blocked),
            Self::Closed => None,
        }
    }

    /// Whether the daemon's answer still needs sorting here.
    fn narrows(self) -> bool {
        !matches!(self, Self::Is(_))
    }

    fn keeps(self, t: &Task) -> bool {
        match self {
            Self::Is(status) => t.status == status,
            Self::Failed => t.has_failed(),
            Self::Closed => t.status.is_terminal(),
        }
    }
}

fn parse_close_reason(text: &str) -> std::result::Result<CloseReason, String> {
    text.parse()
}

/// Why a task is where it is, when its status alone does not say: the
/// failure it is blocked on, or how it was closed (`#122`).
fn standing(t: &Task) -> Option<String> {
    if t.status == TaskStatus::Pending {
        if let Some(after) = &t.after {
            let waiting = if after.is_empty() { "waiting for workflow release".into() } else { format!("waiting on {}", after.join(", ")) };
            return Some(match &t.after_condition {
                Some(condition) => format!("{waiting}; {condition}"),
                None => waiting,
            });
        }
    }
    if t.has_failed() {
        let kind = t.failure.as_ref().and_then(|f| f.kind).map_or("unclassified", |k| k.as_str());
        return Some(format!("last attempt failed: {kind}"));
    }
    let reason = t.close_reason()?;
    Some(match t.closure.as_ref().and_then(|c| c.duplicate_of.as_deref()) {
        Some(other) => format!("duplicate of {other}"),
        None => reason.label().to_string(),
    })
}

/// What `task create` says after the task line: whether anything will start
/// it. Nothing dispatches a pending task on its own -- there is no queue
/// (`#124`) -- so a task created without `--run` or a schedule sits there
/// until someone runs it, and the output has to say so rather than let
/// `pending` read as "waiting its turn".
fn created_note(t: &Task, dispatched: bool) -> String {
    if dispatched {
        return format!("dispatched; `factory task show {}` follows it", t.id);
    }
    if let Some(after) = &t.after {
        return format!("created, waiting on {}; starts once its upstream tasks finish", after.join(", "));
    }
    match (&t.schedule, t.next_run_at) {
        (Some(_), Some(next)) => format!(
            "created, not dispatched; its schedule first fires it at {} -- `factory task run {}` runs it now",
            next.to_rfc3339(),
            t.id
        ),
        _ => format!(
            "created, not dispatched; run it with `factory task run {}` -- nothing starts it on its own",
            t.id
        ),
    }
}

fn one_line(t: &Task) -> String {
    let runs = match t.runs {
        0 => "         ".to_string(),
        1 => "  1 run  ".to_string(),
        n => format!("{n:>3} runs "),
    };
    let why = standing(t).map(|w| format!("  [{w}]")).unwrap_or_default();
    format!(
        "{}  {:<12} {} {:<10} {:<12} {}{why}",
        &t.id[..8.min(t.id.len())],
        t.status.as_str(),
        runs,
        t.scope,
        t.agent,
        t.title
    )
}

fn run_line(r: &Run) -> String {
    format!(
        "{}  attempt {:<3} {:<12} {:<9} {}",
        &r.id[..8.min(r.id.len())],
        r.attempt,
        r.status.as_str(),
        r.trigger.as_str(),
        r.started_at.format("%Y-%m-%d %H:%M:%S")
    )
}

async fn workflow_cmd(json: bool, client: &Client, cmd: WorkflowCmd) -> Result<()> {
    match cmd {
        WorkflowCmd::Lint { workflow, task, scope, category } => {
            let payload = client.send(Request::WorkflowLint { workflow, task, scope, category }).await?;
            print(&payload, json, |p| match p {
                Payload::WorkflowLint { lint } => Some(lint_text(lint)),
                _ => None,
            })
        }
        WorkflowCmd::List { scope } => {
            let payload = client.send(Request::WorkflowList { scope }).await?;
            print(&payload, json, |p| match p {
                Payload::Workflows { workflows } => Some(
                    workflows
                        .iter()
                        .map(|w| format!("{}  {}  r{}  {} nodes  ({})", w.id, w.name, w.revision, w.nodes.len(), w.scope))
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                _ => None,
            })
        }
        WorkflowCmd::Show { id } => {
            let payload = client.send(Request::WorkflowGet { id }).await?;
            print(&payload, json, |p| match p {
                Payload::Workflow { workflow } => Some(workflow_text(workflow)),
                _ => None,
            })
        }
        WorkflowCmd::Create { file } => {
            let draft = read_workflow_file(&file)?;
            let payload = client.send(Request::WorkflowCreate(draft)).await?;
            print(&payload, json, |p| match p {
                Payload::Workflow { workflow } => Some(format!("created {}\n\n{}", workflow.id, workflow_text(workflow))),
                _ => None,
            })
        }
        WorkflowCmd::Update { id, file } => {
            let workflow = read_workflow_file(&file)?;
            let payload = client.send(Request::WorkflowUpdate { id, workflow }).await?;
            print(&payload, json, |p| match p {
                Payload::Workflow { workflow } => Some(format!("updated to r{}\n\n{}", workflow.revision, workflow_text(workflow))),
                _ => None,
            })
        }
        WorkflowCmd::Start { id, inputs } => {
            let inputs = parse_pairs(&inputs, "inputs")?;
            let payload = client.send(Request::WorkflowStart { id, inputs }).await?;
            print(&payload, json, |p| match p {
                Payload::WorkflowRun { run } => Some(workflow_run_text(run)),
                _ => None,
            })
        }
        WorkflowCmd::Runs { id, limit } => {
            let payload =
                client.send(Request::WorkflowRunList { workflow_id: id, scope: None, limit: Some(limit) }).await?;
            print(&payload, json, |p| match p {
                Payload::WorkflowRuns { runs } => Some(
                    runs.iter()
                        .map(|r| {
                            format!(
                                "{}  {:?}  {}  r{}  {}",
                                r.id,
                                r.status,
                                r.definition.name,
                                r.revision,
                                r.created_at.to_rfc3339()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                _ => None,
            })
        }
        WorkflowCmd::Run { run_id } => {
            let payload = client.send(Request::WorkflowRunGet { id: run_id }).await?;
            print(&payload, json, |p| match p {
                Payload::WorkflowRun { run } => Some(workflow_run_text(run)),
                _ => None,
            })
        }
        WorkflowCmd::Cancel { run_id } => {
            let payload = client.send(Request::WorkflowRunCancel { id: run_id }).await?;
            print(&payload, json, |p| match p {
                Payload::WorkflowRun { run } => Some(workflow_run_text(run)),
                _ => None,
            })
        }
    }
}

/// A workflow file: YAML, which JSON also is.
fn read_workflow_file(path: &std::path::Path) -> Result<factory_core::workflow::WorkflowDraft> {
    let text = std::fs::read_to_string(path).map_err(|e| anyhow!("reading {}: {e}", path.display()))?;
    serde_yaml_ng::from_str(&text).map_err(|e| anyhow!("{} is not a workflow: {e}", path.display()))
}

fn workflow_text(w: &factory_core::workflow::WorkflowDefinition) -> String {
    let mut s = format!("{}  {}  r{}  (scope {})\n", w.id, w.name, w.revision, w.scope);
    if !w.description.trim().is_empty() {
        s.push_str(&format!("{}\n", w.description.trim()));
    }
    for input in &w.inputs {
        s.push_str(&format!("input {}  {}\n", input.name, input.description));
    }
    if w.part.is_some() {
        match w.part_shape() {
            Ok(part) => s.push_str(&format!(
                "part workflow: entry {}, deliverable {}, terminal {}\n",
                part.entry, part.deliverable, part.terminal
            )),
            Err(error) => s.push_str(&format!("part workflow: {error}\n")),
        }
    }
    let order = w.validate().unwrap_or_else(|_| w.nodes.iter().map(|n| n.id.clone()).collect());
    for id in order {
        let Some(node) = w.nodes.iter().find(|n| n.id == id) else { continue };
        let after: Vec<&str> = w.edges.iter().filter(|e| e.to == id).map(|e| e.from.as_str()).collect();
        s.push_str(&format!("  {}  {}", node.id, node.task.title));
        if let Some(agent) = &node.task.agent {
            s.push_str(&format!("  [{agent}]"));
        }
        if !after.is_empty() {
            s.push_str(&format!("  after {}", after.join(", ")));
        }
        for (index, exit) in node.exits.iter().enumerate() {
            let condition = exit
                .check
                .as_ref()
                .map(|check| format!("check {check:?}"))
                .unwrap_or_else(|| format!("agent {}", exit.agent.as_deref().unwrap_or("")));
            let rounds = exit
                .max_rounds
                .map(|rounds| format!(", at most {rounds}x"))
                .unwrap_or_default();
            s.push_str(&format!(
                "  exit {} -> {} ({condition}{rounds})",
                index + 1,
                exit.to
            ));
        }
        s.push('\n');
    }
    s
}

fn workflow_run_text(r: &factory_core::workflow::WorkflowRun) -> String {
    let mut s = format!("{}  {:?}  {}  r{}\n", r.id, r.status, r.definition.name, r.revision);
    for (name, value) in &r.inputs {
        s.push_str(&format!("input {name}={value}\n"));
    }
    if let Some(error) = &r.error {
        s.push_str(&format!("error: {error}\n"));
    }
    for node in &r.nodes {
        s.push_str(&format!("  {}  {:?}", node.node_id, node.status));
        if let Some(task) = &node.task_id {
            s.push_str(&format!("  task {task}"));
        }
        if node.round > 0 {
            s.push_str(&format!("  rework {}", node.round));
        }
        if let Some(to) = &node.routed_to {
            s.push_str(&format!("  -> {to}"));
        }
        if let Some(reason) = &node.skip_reason {
            s.push_str(&format!("  -- {reason}"));
        }
        if let Some(error) = &node.error {
            s.push_str(&format!("  -- {error}"));
        }
        s.push('\n');
    }
    s
}

/// `factory workflow lint`, for a person: each category's plan, then what a
/// run would get injected, then what the graph gets wrong.
fn lint_text(lint: &factory_core::workflow::WorkflowLint) -> String {
    let mut s = String::new();
    if !lint.subject.is_empty() {
        s.push_str(&format!("{}  (scope {})\n", lint.subject, lint.scope));
    }
    if let Some(part) = &lint.part {
        s.push_str(&format!(
            "part workflow: entry {}, deliverable {}, terminal {} -- shown as every part gets it, over two sample parts (b after a)\n",
            part.entry, part.deliverable, part.terminal
        ));
    }
    for plan in &lint.plans {
        s.push_str(&format!("\nplan for {} at {}:\n", plan.category, plan.scope));
        if plan.steps.is_empty() {
            s.push_str("  nothing required\n");
        }
        for step in &plan.steps {
            let command = step.command.as_deref().map(|c| format!(" `{c}`")).unwrap_or_default();
            let enforced = if step.enforced { "" } else { "  [not enforced until v2]" };
            let order = [
                step.before.iter().map(|b| format!("before {b}")).collect::<Vec<_>>(),
                step.after.iter().map(|a| format!("after {a}")).collect::<Vec<_>>(),
            ]
            .concat();
            let order = if order.is_empty() { String::new() } else { format!(" [{}]", order.join(", ")) };
            s.push_str(&format!(
                "  {:<14} {}{command}{order}  <- {}{enforced}\n",
                step.id,
                step.kind.as_str(),
                step.required_by.join(", ")
            ));
        }
        for waiver in &plan.waived {
            s.push_str(&format!(
                "  waived: {} ({}) n/a at {}: {}\n",
                waiver.control,
                waiver.steps.join(", "),
                waiver.scope,
                waiver.rationale
            ));
        }
        for finding in &plan.findings {
            s.push_str(&format!("  ! {finding}\n"));
        }
    }
    if !lint.injections.is_empty() {
        s.push_str("\ninjected at run start:\n");
        for i in &lint.injections {
            let how = if i.satisfied_by_authored {
                format!("satisfied by authored gate {}", i.gate_node_id)
            } else {
                format!("+ locked gate {}", i.gate_node_id)
            };
            s.push_str(&format!("  {:<14} after {:<18} {how}\n", i.step, i.node_id));
        }
    }
    if !lint.violations.is_empty() {
        s.push_str("\nordering violations:\n");
        for v in &lint.violations {
            s.push_str(&format!("  ! {v}\n"));
        }
    }
    s.trim_end().to_string()
}

fn attestations_text(attestations: &[factory_core::control_plan::StepAttestation]) -> String {
    if attestations.is_empty() {
        return "no attestations".into();
    }
    attestations
        .iter()
        .map(|a| {
            let code = a.exit_code.map(|c| format!("exit {c}")).unwrap_or_else(|| "did not finish".into());
            let commit = a.commit.as_deref().map(|c| &c[..c.len().min(12)]).unwrap_or("-");
            let dirty = if a.dirty == Some(true) { "+dirty" } else { "" };
            let by = if a.required_by.is_empty() { String::new() } else { format!("  <- {}", a.required_by.join(", ")) };
            format!(
                "{}  {:<14} {:<4} {code:<14} by {}  on {commit}{dirty}{by}",
                a.at.to_rfc3339(),
                a.step,
                a.verdict.as_str(),
                a.actor
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn run_detail(r: &Run) -> String {
    let mut s = format!(
        "{}\n  task       {}\n  attempt    {}\n  status     {}\n  trigger    {}\n  agent      {} on {}\n  started    {}\n",
        r.id,
        r.task_id,
        r.attempt,
        r.status.as_str(),
        r.trigger.as_str(),
        r.agent,
        r.runtime,
        r.started_at.to_rfc3339(),
    );
    if let Some(ended) = r.ended_at {
        s.push_str(&format!(
            "  ended      {} ({}s)\n",
            ended.to_rfc3339(),
            (ended - r.started_at).num_seconds()
        ));
    }
    if let Some(session) = &r.session {
        s.push_str(&format!("  session    {} {}\n", session.runtime, session.handle));
    }
    if let Some(provider) = &r.provider_account {
        s.push_str(&format!("  provider   {provider}\n"));
    }
    if let Some(estimate) = &r.original_estimate {
        s.push_str(&format!(
            "  estimated  {}s / {}s / {}s",
            estimate.time.low, estimate.time.expected, estimate.time.high
        ));
        if let Some(cost) = &estimate.cost {
            s.push_str(&format!("; {} / {} / {}", fmt_usd(cost.low), fmt_usd(cost.expected), fmt_usd(cost.high)));
        }
        s.push('\n');
    }
    if let Some(estimate) = &r.re_estimate {
        if let Some(time) = &estimate.time {
            s.push_str(&format!(
                "  re-estimate {}s / {}s / {}s ({} samples)\n",
                time.low, time.expected, time.high, estimate.sample_count
            ));
        } else if let Some(reason) = &estimate.reason {
            s.push_str(&format!("  re-estimate unavailable -- {reason}\n"));
        }
    }
    if let Some(since) = r.blocked_since {
        let source = r.blocked_source.map(|s| s.as_str()).unwrap_or("?");
        s.push_str(&format!("  blocked    since {} ({source})\n", since.to_rfc3339()));
    }
    if let Some(since) = r.block_suspected_since {
        s.push_str(&format!(
            "  suspected  blocked since {} -- a guess from the screen, unconfirmed\n",
            since.to_rfc3339()
        ));
    }
    if let Some(branch) = &r.worktree_branch {
        s.push_str(&format!("  branch     {branch}\n"));
    }
    if let Some(path) = &r.worktree_path {
        s.push_str(&format!("  worktree   {path}\n"));
    }
    for (i, step) in r.required_steps.iter().enumerate() {
        let label = if i == 0 { "requires" } else { "" };
        let command = step.command.as_deref().map(|c| format!(" `{c}`")).unwrap_or_default();
        let by = if step.required_by.is_empty() { String::new() } else { format!(" ({})", step.required_by.join(", ")) };
        s.push_str(&format!("  {label:<10} {}{command}{by}\n", step.step));
    }
    if let Some(u) = &r.usage {
        s.push_str(&format!("\n{}\n", usage_block(u)));
    }
    if let Some(v) = &r.result {
        s.push_str(&format!("\nresult:\n{v}\n"));
    }
    if let Some(v) = &r.error {
        s.push_str(&format!("\nerror:\n{v}\n"));
    }
    s.trim_end().to_string()
}

// -- usage and cost (#117) -----------------------------------------------------

/// Dollars, to the cent; a non-zero amount under a cent says so rather than
/// printing as free.
fn fmt_usd(v: f64) -> String {
    if v > 0.0 && v < 0.005 {
        "<$0.01".into()
    } else {
        format!("${v:.2}")
    }
}

fn fmt_tokens(n: u64) -> String {
    match n {
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1_000_000.0),
        n if n >= 1_000 => format!("{:.1}k", n as f64 / 1_000.0),
        n => n.to_string(),
    }
}

fn fmt_count(v: Option<u64>) -> String {
    v.map(fmt_tokens).unwrap_or_else(|| "?".into())
}

/// A run's usage, the way `run show` prints it. `?` is a count the runtime
/// could not observe -- never a zero.
fn usage_block(u: &factory_core::usage::RunUsage) -> String {
    use factory_core::usage::UsageState;
    if u.state == UsageState::Unknown {
        return format!(
            "usage      unknown -- {}",
            u.reason.as_deref().unwrap_or("no reason recorded")
        );
    }
    let t = &u.tokens;
    let mut s = format!(
        "usage{}\n  tokens     {} in, {} out, {} cache read, {} cache write (total {})\n  cost       {}",
        if u.partial { " (at least)" } else { "" },
        fmt_count(t.input),
        fmt_count(t.output),
        fmt_count(t.cache_read),
        fmt_count(t.cache_write),
        fmt_count(t.total()),
        u.cost_usd.map(fmt_usd).unwrap_or_else(|| "unknown".into()),
    );
    if !u.pricing_sources.is_empty() {
        s.push_str(&format!(" priced by {}", u.pricing_sources.join(", ")));
    }
    if !u.models.is_empty() {
        s.push_str(&format!("\n  model      {}", u.models.join(", ")));
    }
    if let Some(seconds) = u.elapsed_seconds {
        s.push_str(&format!("\n  elapsed    {seconds:.1}s"));
    }
    if let Some(seconds) = u.active_seconds {
        s.push_str(&format!("\n  active     {seconds:.1}s"));
    }
    for share in &u.plan_share {
        s.push_str(&format!(
            "\n  plan share {:.2}% of {}m {} ({:?})",
            share.used_percent, share.window_minutes, share.provider_account, share.attribution
        ));
    }
    if let Some(reason) = &u.plan_share_unknown {
        s.push_str(&format!("\n  plan share unknown -- {reason}"));
    }
    s.push_str(&format!(
        "\n  sessions   {} harness session{}",
        u.sessions,
        if u.sessions == 1 { "" } else { "s" }
    ));
    if let (Some(at), Some(point)) = (u.as_of, u.as_of_point) {
        s.push_str(&format!("\n  as of      {} ({})", at.format("%Y-%m-%d %H:%M:%S"), point.as_str()));
    }
    for note in &u.notes {
        s.push_str(&format!("\n  note       {note}"));
    }
    s
}

fn task_usage_text(u: &factory_core::usage::TaskUsage) -> String {
    let mut s = format!("usage over {} run{}\n", u.total.runs, if u.total.runs == 1 { "" } else { "s" });
    if u.total.runs_unknown == u.total.runs {
        s.push_str("  total      unknown -- no run's usage was measured\n");
    } else {
        s.push_str(&format!(
            "  total      {} tokens, {}{}\n",
            fmt_tokens(u.total.tokens.total()),
            sum_usd(&u.total),
            unknown_suffix(&u.total)
        ));
    }
    // The task's figures are sums over its runs, estimates included: say
    // so, or a retry's second estimate reads as the task's own.
    let over = if u.runs.len() == 1 { String::new() } else { format!(" over {} runs", u.runs.len()) };
    if let Some(comparison) = &u.time_comparison {
        s.push_str(&format!(
            "  time       {} actual / {}s expected{over} ({})\n",
            comparison
                .actual
                .map(|seconds| format!("{seconds}s"))
                .unwrap_or_else(|| "unknown".into()),
            comparison.expected,
            comparison
                .actual_over_expected
                .map(|ratio| format!("{ratio:.2}x"))
                .unwrap_or_else(|| "unknown".into())
        ));
    }
    if let Some(comparison) = &u.cost_comparison {
        s.push_str(&format!(
            "  cost       {} actual / {} expected{over}\n",
            comparison.actual.map(fmt_usd).unwrap_or_else(|| "unknown".into()),
            fmt_usd(comparison.expected)
        ));
    }
    for r in &u.runs {
        let what = match r.usage.state {
            factory_core::usage::UsageState::Known => format!(
                "{} tokens, {}{}",
                fmt_count(r.usage.tokens.total()),
                r.usage.cost_usd.map(fmt_usd).unwrap_or_else(|| "cost unknown".into()),
                if r.usage.partial { " (at least)" } else { "" }
            ),
            factory_core::usage::UsageState::Unknown => format!(
                "unknown -- {}",
                r.usage.reason.as_deref().unwrap_or("no reason recorded")
            ),
        };
        s.push_str(&format!(
            "  attempt {:<3} {:<10} {:>6}s  {what}\n",
            r.attempt,
            r.status.as_str(),
            r.wall_seconds
        ));
    }
    s.trim_end().to_string()
}

/// A group's cost, or `?` when not one of its runs had a measured cost --
/// a sum of nothing is not $0.00.
fn sum_usd(row: &factory_core::usage::CostRow) -> String {
    if row.runs_costed() == 0 {
        "?".into()
    } else {
        fmt_usd(row.cost_usd)
    }
}

/// What a sum is missing, said beside it.
fn unknown_suffix(row: &factory_core::usage::CostRow) -> String {
    let mut parts = Vec::new();
    if row.runs_unknown > 0 {
        parts.push(format!("{} unknown", row.runs_unknown));
    }
    if row.runs_cost_unknown > 0 {
        parts.push(format!("{} without a cost", row.runs_cost_unknown));
    }
    if row.runs_partial > 0 {
        parts.push(format!("{} partial", row.runs_partial));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" (runs not fully in the sum: {})", parts.join(", "))
    }
}

/// `3/5 in range, 1.20x median` -- `#168`'s estimate vs actual for one
/// group, or `-` when nothing in it carries an `original_estimate` at all.
fn estimate_vs_actual(row: &factory_core::usage::CostRow) -> String {
    if row.estimated_runs == 0 {
        return "-".into();
    }
    match row.median_actual_over_expected {
        Some(ratio) => format!("{}/{} in range, {ratio:.2}x median", row.within_range, row.estimated_runs),
        None => format!("{}/{} in range", row.within_range, row.estimated_runs),
    }
}

fn budget_text(report: &factory_core::budget::Report) -> String {
    let mut text = format!("Budget — UTC month {} through {} (as of {})\nAuthored limits: {}\nSubtree caps overlap: they are not summed. Projection is linear observed pace.\n\n", report.month.from, report.month.until, report.month.as_of, report.catalogue);
    for row in &report.budgets {
        let limit = row.monthly_usd.map(|v| format!("${v:.2}")).unwrap_or_else(|| "unconfigured".into());
        let remaining = row.assessment.remaining_usd.map(|v| format!("${v:.2}")).unwrap_or_else(|| "unknown".into());
        let projected = row.assessment.projected_month_usd.map(|v| format!("${v:.2}")).unwrap_or_else(|| "unknown".into());
        text.push_str(&format!("{} [{}]: limit {limit}; ${:.2} known; remaining {remaining}; month projection {projected}\n  {}\n", row.scope, row.relation, row.spent.cost_usd, row.assessment.reason));
    }
    for finding in &report.findings { text.push_str(&format!("Finding: {finding}\n")); }
    text.push_str("\nSelected spend:\n");
    text.push_str(&costs_text(&report.spend));
    text
}

fn costs_text(r: &factory_core::usage::CostReport) -> String {
    let mut s = format!(
        "cost by {}, runs started {} to {}{}\n",
        r.group_by.as_str(),
        r.from.format("%Y-%m-%d %H:%M"),
        r.to.format("%Y-%m-%d %H:%M"),
        r.scope.as_deref().map(|sc| format!(", scope {sc}")).unwrap_or_default()
    );
    if r.unattributed_runs > 0 {
        s.push_str(&format!("{} runs have unknown scope attribution; scoped sums may exclude their spend.\n", r.unattributed_runs));
    }
    if r.rows.is_empty() {
        s.push_str("no runs in that window");
        return s;
    }
    let key_width = r.rows.iter().map(|row| row.key.chars().count().min(40)).max().unwrap_or(3).max(5);
    s.push_str(&format!(
        "{:<key_width$}  {:>5}  {:>7}  {:>8}  {:>9}  {:<28}  {}\n",
        "GROUP", "RUNS", "UNKNOWN", "TOKENS", "COST", "ESTIMATE", ""
    ));
    let line = |row: &factory_core::usage::CostRow| {
        let key: String = row.key.chars().take(40).collect();
        format!(
            "{:<key_width$}  {:>5}  {:>7}  {:>8}  {:>9}  {:<28}  {}",
            key,
            row.runs,
            row.runs_unknown + row.runs_cost_unknown,
            if row.runs_unknown == row.runs { "?".to_string() } else { fmt_tokens(row.tokens.total()) },
            sum_usd(row),
            estimate_vs_actual(row),
            row.label.as_deref().unwrap_or("")
        )
    };
    for row in &r.rows {
        s.push_str(&line(row));
        s.push('\n');
    }
    s.push_str(&line(&r.total));
    let missing = r.total.runs_unknown + r.total.runs_cost_unknown;
    if missing > 0 {
        s.push_str(&format!(
            "\n\n{missing} of {} runs have no measured cost (UNKNOWN); the sums leave them out rather than count them as free.",
            r.total.runs
        ));
    }
    s.trim_end().to_string()
}

fn snapshots_text(snapshots: &[factory_core::usage::UsageSnapshot]) -> String {
    if snapshots.is_empty() {
        return "no usage snapshots for this run".into();
    }
    snapshots
        .iter()
        .map(|snap| {
            let what = match (&snap.usage, &snap.unknown) {
                (Some(u), _) => {
                    let cost: Option<f64> = u
                        .sessions
                        .iter()
                        .flat_map(|h| std::iter::once(h.cost.usd).chain(h.subagents.iter().map(|a| a.cost.usd)))
                        .try_fold(0.0, |sum, c| c.map(|c| sum + c));
                    format!(
                        "{} session{}, cumulative {}",
                        u.sessions.len(),
                        if u.sessions.len() == 1 { "" } else { "s" },
                        cost.map(fmt_usd).unwrap_or_else(|| "cost unknown".into())
                    )
                }
                (None, why) => format!("no answer -- {}", why.as_deref().unwrap_or("no reason recorded")),
            };
            format!("{}  {:<10} {}", snap.at.format("%Y-%m-%d %H:%M:%S"), snap.point.as_str(), what)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `7d`, `12h`, `30m`, a date (`2026-09-01`, midnight UTC) or RFC 3339.
fn parse_when(text: &str, now: chrono::DateTime<chrono::Utc>) -> Result<chrono::DateTime<chrono::Utc>> {
    let text = text.trim();
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(text) {
        return Ok(t.with_timezone(&chrono::Utc));
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return Ok(d.and_hms_opt(0, 0, 0).expect("midnight exists").and_utc());
    }
    if let Some(days) = text.strip_suffix('d').and_then(|n| n.trim().parse::<i64>().ok()) {
        return Ok(now - chrono::Duration::days(days));
    }
    let seconds = parse_duration_seconds(text)
        .map_err(|_| anyhow!("{text:?} is not a time: use 7d, 12h, 2026-09-01 or RFC 3339"))?;
    Ok(now - chrono::Duration::seconds(seconds as i64))
}

fn entries_text(entries: &[factory_core::task::TaskEntry]) -> String {
    if entries.is_empty() {
        return "no entries".into();
    }
    entries
        .iter()
        .map(|e| {
            format!(
                "{}  {:<8} {:<12} {}",
                e.at.format("%H:%M:%S"),
                e.source,
                e.kind,
                e.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn detail(t: &Task) -> String {
    let mut s = format!(
        "{}\n  title      {}\n  status     {}\n  scope      {}\n  agent      {} on {}\n  created    {}\n",
        t.id,
        t.title,
        t.status.as_str(),
        t.scope,
        t.agent,
        t.runtime,
        t.created_at.to_rfc3339(),
    );
    if let Some(category) = &t.category {
        s.push_str(&format!("  category   {category}\n"));
    }
    if let Some(sched) = &t.schedule {
        let paused = if t.schedule_paused { " (paused)" } else { "" };
        s.push_str(&format!("  schedule   {}{paused}\n", describe_schedule(sched)));
        s.push_str(&format!(
            "  retry      {}\n",
            t.retry.map(describe_retry).unwrap_or_else(|| "(daemon default)".into())
        ));
    }
    if let Some(next) = t.next_run_at {
        s.push_str(&format!("  next run   {}\n", next.to_rfc3339()));
    }
    // The one thing `AGENTS.md` warns loudest about getting wrong: a task
    // sitting `pending` on a stale `error` from an attempt a retry already
    // superseded. Surfaced here, right next to `next run`, so a retry in
    // flight is visible from `factory task show` alone -- no daemon log
    // needed to notice a scheduled run failed and is being tried again.
    if let Some(retry) = &t.pending_retry {
        s.push_str(&format!(
            "  retrying   attempt {} queued, resuming the regular schedule at {} once it settles\n",
            retry.attempts,
            retry.resume_at.to_rfc3339(),
        ));
    }
    if let Some(f) = t.failure.as_ref().filter(|_| t.has_failed()) {
        let kind = f.kind.map_or("unclassified", |k| k.as_str());
        let attempt = f.attempt.map(|a| format!("attempt {a} ")).unwrap_or_default();
        s.push_str(&format!(
            "  failed     {attempt}{kind} at {} -- run it again, or close it with a reason\n",
            f.at.to_rfc3339()
        ));
    }
    if let Some(reason) = t.close_reason() {
        let mut line = format!("  closed     {}", reason.label());
        if let Some(c) = &t.closure {
            if let Some(other) = &c.duplicate_of {
                line.push_str(&format!(" of {other}"));
            }
            line.push_str(&format!(" by {} at {}", c.by, c.at.to_rfc3339()));
            if let Some(note) = &c.note {
                line.push_str(&format!(": {note}"));
            }
        }
        s.push_str(&line);
        s.push('\n');
    }
    if let Some(estimate) = t.effective_estimate() {
        s.push_str(&format!(
            "  estimate   {}s / {}s / {}s",
            estimate.time.low, estimate.time.expected, estimate.time.high
        ));
        if let Some(cost) = estimate.cost {
            s.push_str(&format!("; {} / {} / {}", fmt_usd(cost.low), fmt_usd(cost.expected), fmt_usd(cost.high)));
        }
        s.push('\n');
    }
    if t.worktree {
        s.push_str("  worktree   yes, a fresh one before each run\n");
    }
    if t.knowledge_hints {
        s.push_str("  knowledge  matching pages handed to each run\n");
    }
    if t.runs > 0 {
        s.push_str(&format!("  runs       {}\n", t.runs));
    }
    if let Some(v) = t.ack_timeout_seconds {
        s.push_str(&format!("  ack after  {v}s\n"));
    }
    if let Some(v) = t.timeout_seconds {
        s.push_str(&format!("  timeout    {v}s\n"));
    }
    if let Some(v) = t.blocked_timeout_seconds {
        s.push_str(&format!("  blocked    {v}s\n"));
    }
    if !t.labels.is_empty() {
        let labels: Vec<String> = t.labels.iter().map(|(k, v)| format!("{k}={v}")).collect();
        s.push_str(&format!("  labels     {}\n", labels.join(" ")));
    }
    if !t.instructions.trim().is_empty() {
        s.push_str(&format!("\n{}\n", t.instructions.trim()));
    }
    if let Some(r) = &t.result {
        s.push_str(&format!("\nresult:\n{r}\n"));
    }
    if let Some(e) = &t.error {
        s.push_str(&format!("\nerror:\n{e}\n"));
    }
    s.trim_end().to_string()
}

fn describe_schedule(s: &Schedule) -> String {
    match s {
        Schedule::Cron(cron) => format!("cron {}", cron.describe()),
        Schedule::Every { seconds } => format!("every {seconds}s"),
    }
}

fn describe_retry(p: RetryPolicy) -> String {
    match p {
        RetryPolicy::None => "none".into(),
        RetryPolicy::Backoff { max_attempts, backoff_seconds } => {
            format!("{max_attempts}x{backoff_seconds}s")
        }
    }
}

fn describe_event(e: &Event) -> String {
    match e {
        Event::ImportantDatesUpdated { .. } => "important dates refreshed".into(),
        Event::DaemonStarted { instance, .. } => format!("daemon up: {instance}"),
        Event::TaskCreated { task } => format!("created  {}", one_line(task)),
        Event::TaskUpdated { task } => format!("updated  {}", one_line(task)),
        Event::TaskDeleted { id } => format!("deleted  {id}"),
        Event::WorkflowCreated { workflow } => format!("workflow created  {}", workflow.name),
        Event::WorkflowUpdated { workflow } => format!("workflow updated  {} r{}", workflow.name, workflow.revision),
        Event::WorkflowDeleted { id } => format!("workflow deleted  {id}"),
        Event::WorkflowRunUpdated { run } => format!("workflow run  {}  {:?}", run.id, run.status),
        Event::BenchRunUpdated { run } => format!(
            "bench run  {}  {}  {}/{} settled",
            run.id,
            run.dataset,
            run.attempts.iter().filter(|a| a.verdict.is_some()).count(),
            run.attempts.len(),
        ),
        Event::TaskEntry { id, entry } => format!(
            "entry    {}  {:<8} {}",
            &id[..8.min(id.len())],
            entry.source,
            entry.message
        ),
        Event::RunStarted { run } => format!("run      {}", run_line(run)),
        Event::RunUpdated { run } => format!("run      {}", run_line(run)),
        Event::AgentUpdated { agent } => {
            format!("agent    {}  {}", agent.id, agent.state.as_str())
        }
        Event::AgentRemoved { id } => format!("agent    {id}  removed"),
        Event::AgentConfigured { scope, name } => {
            format!("agent    {scope}/{name}  configured")
        }
        Event::AgentDeleted { scope, name } => {
            format!("agent    {scope}/{name}  deleted")
        }
        Event::RolesChanged { scope, name } => {
            format!("role     {name} in {scope}  changed")
        }
        Event::DashboardChanged { scope } => {
            format!("dashboard {scope}  changed")
        }
        Event::PolicyChanged { scope, control } => {
            format!("policy   {control} in {scope}  changed")
        }
        Event::GoalsChanged { kr } => {
            format!("goals    {kr}  checked in")
        }
        Event::QualityChanged { profiles } => {
            format!("quality  profiles changed ({})", profiles.join(", "))
        }
        Event::BackupCompleted { snapshot } => {
            format!("backup   {}  completed ({} files)", snapshot.name, snapshot.files)
        }
        Event::BackupFailed { reason, .. } => format!("backup   failed: {reason}"),
        Event::BackupVerified { verification } => format!(
            "backup   {}  verified {}",
            verification.snapshot,
            if verification.ok { "ok" } else { "FAILED" }
        ),
        Event::DeploymentUpdated { deployment: d } => {
            format!("deploy   {} to {}  {}  ({})", short(&d.release.commit), d.environment, d.status.as_str(), d.id)
        }
        Event::EnvironmentStatusChanged { environment, status, .. } => {
            format!("env      {environment}  {}", env_status(*status))
        }
        Event::AgentActivity {
            subject, status, ..
        } => format!("activity {subject}  {}", status.as_str()),
    }
}

/// Human-readable unless `--json`, and never silently empty: a payload the
/// formatter does not know about is printed as JSON rather than swallowed.
fn print<F>(payload: &Payload, json: bool, human: F) -> Result<()>
where
    F: Fn(&Payload) -> Option<String>,
{
    if json {
        println!("{}", serde_json::to_string_pretty(payload)?);
        return Ok(());
    }
    match human(payload) {
        Some(text) => {
            if !text.is_empty() {
                println!("{text}");
            }
            Ok(())
        }
        None => match payload {
            Payload::Ok => Ok(()),
            other => {
                println!("{}", serde_json::to_string_pretty(other)?);
                Ok(())
            }
        },
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn offline_recovery_cli_requires_explicit_reported_fields_and_keeps_unknown_checks_optional() {
        let cli = Cli::try_parse_from(["factory", "--root", "/tmp/instance", "recovery-journal", "start", "--scope", "demo",
            "--env", "prod", "--source", "ensure.sh", "--actor", "operator", "--reason", "not running", "--command", "restart installed"]).unwrap();
        assert!(matches!(cli.command, Command::RecoveryJournal(RecoveryJournalCmd::Start { .. })));
        let cli = Cli::try_parse_from(["factory", "--root", "/tmp/instance", "recovery-journal", "finish", "uuid", "--exit-code", "1"]).unwrap();
        assert!(matches!(cli.command, Command::RecoveryJournal(RecoveryJournalCmd::Finish { local_http: None, network_routes: None, .. })));
        assert!(Cli::try_parse_from(["factory", "recovery-journal", "finish", "uuid", "--exit-code", "999"]).is_err());
    }

    #[test]
    fn release_recording_accepts_an_explicit_producing_run_scope_and_comparison() {
        let parsed = Cli::try_parse_from(["factory", "release", "add", "--scope", "demo", "--commit", "sha",
            "--build-run", "build-id", "--build-scope", "source", "--compare-to", "base-sha"]).unwrap();
        let Command::Release(ReleaseCmd::Add { release, .. }) = parsed.command else { panic!("release add"); };
        let facts = release.facts().unwrap();
        assert_eq!(facts.build_run.as_deref(), Some("build-id"));
        assert_eq!(facts.build_scope.as_deref(), Some("source"));
        assert_eq!(facts.compare_to.as_deref(), Some("base-sha"));
        assert!(facts.changes.is_none(), "captured comparisons belong to the daemon, not the CLI");
        assert!(Cli::try_parse_from(["factory", "deploy", "start", "--env", "review", "--commit", "sha", "--build-run", "build-id"]).is_ok());
    }

    #[test]
    fn recovery_requires_a_reason_and_health_check_is_a_separate_command() {
        let parsed = Cli::try_parse_from(["factory", "recover", "production", "--reason", "restart installed system"]).unwrap();
        assert!(matches!(parsed.command, Command::Recover { environment, reason } if environment == "production" && reason == "restart installed system"));
        assert!(Cli::try_parse_from(["factory", "recover", "production"]).is_err());
        let parsed = Cli::try_parse_from(["factory", "environment-check", "production"]).unwrap();
        assert!(matches!(parsed.command, Command::EnvironmentCheck { environment } if environment == "production"));
    }

    #[test]
    fn environment_promotion_requires_a_deployment_selection_and_strict_start_is_explicit() {
        let parsed = Cli::try_parse_from(["factory", "promote", "staging", "--deployment", "verified-id"]).unwrap();
        assert!(matches!(parsed.command, Command::Promote { environment, deployment } if environment == "staging" && deployment == "verified-id"));
        assert!(Cli::try_parse_from(["factory", "promote", "staging"]).is_err());
        assert!(Cli::try_parse_from(["factory", "deploy", "start", "--env", "production", "--commit", "abc", "--strict-verification"]).is_ok());
        assert!(Cli::try_parse_from(["factory", "env", "production"]).is_ok(), "existing environment read syntax is unchanged");
    }

    #[test]
    fn provenance_cli_supports_repeated_artifacts_and_run_lookup() {
        let cli = Cli::try_parse_from([
            "factory",
            "task",
            "report",
            "t1",
            "--status",
            "done",
            "--artifact",
            "a.bin",
            "--artifact",
            "b.bin",
        ])
        .unwrap();
        match cli.command {
            Command::Task(TaskCmd::Report { artifacts, .. }) => {
                assert_eq!(artifacts, vec!["a.bin", "b.bin"])
            }
            _ => panic!("expected task report"),
        }
        assert!(
            Cli::try_parse_from(["factory", "task", "report", "t1", "--artifact", "a.bin"])
                .is_err()
        );
        assert!(Cli::try_parse_from(["factory", "run", "provenance", "r1", "--json"]).is_ok());
    }
    #[test]
    fn corrective_measure_cli_requires_paired_flags_and_refuses_a_submission_mix() {
        let base = ["factory", "policy", "attest", "cra/art-14", "--scope", "demo", "--evidence", "fix"];
        let parse = |extra: &[&str]| Cli::try_parse_from(base.iter().copied().chain(extra.iter().copied()));
        assert!(parse(&["--corrective-item", "report:t1", "--available-at", "2026-10-01T09:00:00Z"]).is_ok());
        assert!(parse(&["--corrective-item", "report:t1"]).is_err());
        assert!(parse(&["--available-at", "2026-10-01T09:00:00Z"]).is_err());
        assert!(parse(&["--corrective-item", "report:t1", "--available-at", "not-a-date"]).is_err());
        assert!(parse(&["--corrective-item", "report:t1", "--available-at", "2026-10-01T09:00:00Z", "--clock-item", "report:t1", "--deadline", "final-report"]).is_err());
    }

    use super::*;

    fn write_temp(bytes: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("factory-cli-test-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn created(schedule: Option<factory_core::task::Schedule>) -> Task {
        let mut t = factory_core::adapter::store::task_from_new(
            NewTask { title: "t".into(), schedule, ..Default::default() },
            "demo".into(),
            "shell".into(),
            "herdr".into(),
        );
        t.id = "abc123".into();
        t
    }

    #[test]
    fn backup_restore_requires_a_snapshot_and_destination_and_prints_cutover_steps() {
        let cli = Cli::try_parse_from([
            "factory",
            "backup",
            "restore",
            "factory-backup-demo-20260925T030000Z.tar.zst",
            "--into",
            "/tmp/restored factory",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Backup {
                command: Some(BackupCmd::Restore { snapshot, into, identity: None })
            } if snapshot.ends_with(".tar.zst") && into == PathBuf::from("/tmp/restored factory")
        ));
        assert!(Cli::try_parse_from(["factory", "backup", "restore", "snapshot"]).is_err());

        // `#260`: three names, nothing else.
        assert!(matches!(
            Cli::try_parse_from(["factory", "power-mode"]).unwrap().command,
            Command::PowerMode { mode: None }
        ));
        assert!(matches!(
            Cli::try_parse_from(["factory", "power-mode", "high-performance"]).unwrap().command,
            Command::PowerMode { mode: Some(factory_core::protocol::PowerMode::HighPerformance) }
        ));
        for bad in ["3", "auto; rm", "", "high"] {
            assert!(Cli::try_parse_from(["factory", "power-mode", bad]).is_err(), "{bad:?}");
        }

        // Read-only until the rule is installed: the rule and its install
        // command are printed; mixed sources are both shown.
        use factory_core::protocol::{PowerMode, PowerModeReport, SudoersRule};
        let report = PowerModeReport {
            applicable: true,
            supported: PowerMode::ALL.to_vec(),
            ac: Some(PowerMode::HighPerformance),
            battery: Some(PowerMode::Automatic),
            permitted: Vec::new(),
            can_change: false,
            sudoers: SudoersRule {
                path: "/etc/sudoers.d/factory-pmset".into(),
                user: "factory".into(),
                rule: "factory ALL=(root) NOPASSWD: ...".into(),
                install: "sudo visudo -cf ...".into(),
                check: "sudo visudo -c".into(),
            },
            notes: Vec::new(),
            changes: Vec::new(),
        };
        let text = power_mode_text(&Payload::HostPowerMode { report }).unwrap();
        assert!(text.starts_with("power mode: mixed"), "{text}");
        assert!(text.contains("AC          High performance"), "{text}");
        assert!(text.contains("battery     Automatic"), "{text}");
        assert!(text.contains("factory ALL=(root) NOPASSWD: ..."), "{text}");
        assert!(text.contains("install     sudo visudo -cf ..."), "{text}");

        let with_identity = Cli::try_parse_from([
            "factory",
            "backup",
            "restore",
            "factory-backup-demo-20260925T030000Z.tar.zst.age",
            "--into",
            "/tmp/restored",
            "--identity",
            "/tmp/key.txt",
        ])
        .unwrap();
        assert!(matches!(
            with_identity.command,
            Command::Backup { command: Some(BackupCmd::Restore { identity: Some(path), .. }) }
                if path == Path::new("/tmp/key.txt")
        ));

        let payload = Payload::BackupRestore {
            restoration: factory_core::backup::Restoration {
                snapshot: "snapshot.tar.zst".into(),
                into: "/tmp/restored factory".into(),
                files: 12,
                checks: vec![],
                duration_ms: 1500,
            },
        };
        let text = backup_restore_text(&payload).unwrap();
        assert!(text.contains("12 files -> /tmp/restored factory"), "{text}");
        assert!(text.contains("Nothing was switched or started"), "{text}");
        assert!(text.contains("factory-daemon --root '/tmp/restored factory' run"), "{text}");
        assert!(text.contains("factory --root '/tmp/restored factory' status"), "{text}");
    }

    /// `#152`: `--identity` is optional on `backup verify`, and absent by
    /// default so a plaintext snapshot's verify is unchanged.
    #[test]
    fn backup_verify_takes_an_optional_identity() {
        let cli = Cli::try_parse_from(["factory", "backup", "verify"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Backup { command: Some(BackupCmd::Verify { snapshot: None, identity: None }) }
        ));
        let cli = Cli::try_parse_from(["factory", "backup", "verify", "snap.tar.zst.age", "--identity", "/tmp/key.txt"])
            .unwrap();
        assert!(matches!(
            cli.command,
            Command::Backup { command: Some(BackupCmd::Verify { snapshot: Some(s), identity: Some(path) }) }
                if s == "snap.tar.zst.age" && path == Path::new("/tmp/key.txt")
        ));
    }

    /// A minimal but complete `BackupReport` with one encrypted snapshot, for
    /// `#152`'s status/list text.
    fn encrypted_report() -> factory_core::backup::BackupReport {
        use factory_core::backup::*;
        let now = chrono::Utc::now();
        BackupReport {
            now,
            config: Some(BackupConfig {
                destination: PathBuf::from("/Volumes/Backup"),
                schedule: None,
                verify_schedule: None,
                keep: Keep::default(),
                include_logs: false,
                encrypt_to: Some("age1exampleexampleexampleexampleexampleexampleexampleexample".into()),
            }),
            destination: Some(DestinationFacts {
                path: "/Volumes/Backup".into(),
                exists: true,
                same_device: Some(false),
                free_bytes: None,
                total_bytes: None,
            }),
            age: AgeLevel::Fresh,
            due_by: None,
            next_run: None,
            next_verify: None,
            verify_skipped: None,
            running: false,
            last_verified: None,
            last_failure: None,
            warnings: vec![],
            snapshots: vec![SnapshotRow {
                name: "factory-backup-demo-20260925T030000Z.tar.zst.age".into(),
                at: now,
                size_bytes: 1024,
                files: Some(3),
                verified: None,
                kept_by: vec![KeptBy::Newest],
                encrypted: true,
            }],
            include: vec![],
            exclude: vec![],
            code: vec![],
            time_machine: None,
        }
    }

    #[test]
    fn backup_status_names_the_configured_recipient_when_encrypted() {
        let payload = Payload::Backup { report: Box::new(encrypted_report()) };
        let text = backup_status_text(&payload).unwrap();
        assert!(text.contains("encrypted    yes, to age1example"), "{text}");
    }

    /// `#156`: `factory backup status` prints the drill's own schedule, its
    /// next slot and, when set, why it would currently skip -- right beside
    /// the backup's own "next".
    #[test]
    fn backup_status_prints_the_next_drill_and_its_skip_reason_when_set() {
        use factory_core::backup::BackupSchedule;
        let mut report = encrypted_report();
        report.config.as_mut().unwrap().verify_schedule =
            Some(BackupSchedule { cron: "*/30 * * * *".into(), timezone: None });
        report.next_verify = Some(report.now + chrono::Duration::minutes(10));
        report.verify_skipped = Some("newest snapshot is encrypted; verify it with --identity".into());
        let payload = Payload::Backup { report: Box::new(report) };
        let text = backup_status_text(&payload).unwrap();
        assert!(text.contains("verify       */30 * * * * (UTC)"), "{text}");
        assert!(text.contains("next drill   "), "{text}");
        assert!(text.contains("drill skip   newest snapshot is encrypted; verify it with --identity"), "{text}");
    }

    #[test]
    fn backup_status_names_no_drill_scheduled_when_verify_schedule_is_unset() {
        let payload = Payload::Backup { report: Box::new(encrypted_report()) };
        let text = backup_status_text(&payload).unwrap();
        assert!(text.contains("verify       no drill scheduled"), "{text}");
        assert!(!text.contains("next drill"), "{text}");
        assert!(!text.contains("drill skip"), "{text}");
    }

    #[test]
    fn backup_list_marks_an_encrypted_snapshot() {
        let payload = Payload::Backup { report: Box::new(encrypted_report()) };
        let text = backup_list_text(&payload).unwrap();
        assert!(text.contains("ENC"), "{text}");
        let row = text.lines().find(|l| l.contains("tar.zst.age")).unwrap();
        assert!(row.contains("yes"), "{row}");
    }

    #[test]
    fn creating_an_unscheduled_task_says_it_is_not_dispatched_and_how_to_run_it() {
        let note = created_note(&created(None), false);
        assert!(note.starts_with("created, not dispatched"), "{note}");
        assert!(note.contains("`factory task run abc123`"), "{note}");
    }

    #[test]
    fn creating_a_scheduled_task_names_its_first_firing() {
        let mut t = created(Some(factory_core::task::Schedule::Every { seconds: 300 }));
        let at = chrono::DateTime::parse_from_rfc3339("2026-09-25T18:00:00Z").unwrap().with_timezone(&chrono::Utc);
        t.next_run_at = Some(at);
        let note = created_note(&t, false);
        assert!(note.contains("first fires it at 2026-09-25T18:00:00+00:00"), "{note}");
        assert!(note.contains("`factory task run abc123`"), "{note}");
    }

    #[test]
    fn creating_with_run_says_it_was_dispatched() {
        let note = created_note(&created(None), true);
        assert!(note.starts_with("dispatched"), "{note}");
        assert!(!note.contains("not dispatched"), "{note}");
    }

    #[test]
    fn result_file_alone_becomes_the_result() {
        let path = write_temp(b"hello from the command\n");
        let result = combine_result(None, Some(&path)).unwrap();
        assert_eq!(result, Some("hello from the command\n".to_string()));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn result_and_result_file_combine_with_a_header_and_a_blank_line() {
        let path = write_temp(b"the stdout");
        let result = combine_result(Some("command exited 0".into()), Some(&path)).unwrap();
        assert_eq!(result, Some("command exited 0\n\nthe stdout".to_string()));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn an_empty_result_file_leaves_the_header_alone_with_no_trailing_blank_line() {
        let path = write_temp(b"");
        let result = combine_result(Some("command exited 0".into()), Some(&path)).unwrap();
        assert_eq!(result, Some("command exited 0".to_string()));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn neither_result_nor_result_file_reports_nothing() {
        assert_eq!(combine_result(None, None).unwrap(), None);
    }

    #[test]
    fn a_missing_result_file_is_a_readable_error() {
        let path = std::env::temp_dir().join(format!("factory-cli-test-missing-{}", uuid::Uuid::new_v4()));
        let err = combine_result(None, Some(&path)).unwrap_err();
        assert!(err.to_string().contains("reading"), "{err}");
    }

    #[test]
    fn result_file_content_under_the_cap_is_untouched() {
        let body = "short and sweet";
        assert_eq!(tail_lossy(body.as_bytes(), RESULT_FILE_BYTE_CAP), body);
    }

    #[test]
    fn result_file_content_over_the_cap_keeps_the_tail_and_marks_the_cut() {
        let body = "0123456789".repeat(1000); // 10,000 bytes
        let kept = tail_lossy(body.as_bytes(), 100);
        assert!(kept.len() <= 100, "{} bytes, over the cap", kept.len());
        assert!(kept.ends_with('9'), "the tail survives, not the head: {kept:?}");
        assert!(kept.contains("truncated"), "the cut is marked: {kept}");
    }

    #[test]
    fn non_utf8_result_file_content_is_decoded_lossily() {
        // A lone continuation byte (0x80) is not valid UTF-8 on its own.
        let mut bytes = b"before ".to_vec();
        bytes.push(0x80);
        bytes.extend_from_slice(b" after");
        let text = tail_lossy(&bytes, RESULT_FILE_BYTE_CAP);
        assert!(text.contains("before"));
        assert!(text.contains("after"));
        assert!(text.contains('\u{FFFD}'), "the invalid byte becomes a replacement character: {text:?}");
    }

    #[test]
    fn a_truncation_cut_never_splits_a_multibyte_character() {
        let body = "é".repeat(100); // each 'é' is 2 bytes in UTF-8
        let kept = tail_lossy(body.as_bytes(), 51); // an odd cap forces the issue
        assert!(!kept.contains('\u{FFFD}'), "a clean cut needs no replacement character: {kept:?}");
    }

    // -- `task turn-ended`, the hook's side of issue #69 --------------------

    /// Verbatim, less the paths, from the installed `claude` 2.1.280's
    /// `Stop` hook -- not its documentation, which shows
    /// `last_assistant_message` as an object and a `stop_reason` that never
    /// arrives. A fixture written from the docs is how #67's first pass
    /// passed its tests while reading a key that does not exist.
    const LIVE_STOP: &str = r#"{"session_id":"93b16ca2-62a5-493a-ad7a-fd2d2d4df973","transcript_path":"/x.jsonl","cwd":"/x","prompt_id":"9180f541-fb48-423f-9e34-c3b5fc5aedf1","permission_mode":"default","effort":{"level":"high"},"hook_event_name":"Stop","stop_hook_active":false,"last_assistant_message":"pong","background_tasks":[],"session_crons":[]}"#;

    /// The same, captured with a `run_in_background` shell still going.
    const LIVE_STOP_WITH_BACKGROUND: &str = r#"{"hook_event_name":"Stop","stop_hook_active":false,"last_assistant_message":"started","background_tasks":[{"id":"b5lt1eoho","type":"shell","status":"running","description":"Sleep for 25 seconds in background","command":"sleep 25"}],"session_crons":[]}"#;

    fn hook(json: &str) -> serde_json::Value {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn a_live_stop_payload_is_a_finished_turn_with_its_last_message() {
        let turn = turn_ended_from_hook(HookEvent::Stop, &hook(LIVE_STOP), Some("tok".into()));
        assert_eq!(turn.event, factory_core::task::TurnEndEvent::Stop);
        assert_eq!(turn.pending_background, 0);
        assert_eq!(turn.last_message.as_deref(), Some("pong"));
        assert_eq!(turn.token.as_deref(), Some("tok"));
        assert!(turn.error.is_none());
    }

    /// `#178`: Claude Code's own session id, carried on the run as
    /// `--continue`'s fallback source for which session to resume.
    #[test]
    fn a_live_stop_payload_carries_its_own_session_id() {
        let turn = turn_ended_from_hook(HookEvent::Stop, &hook(LIVE_STOP), Some("tok".into()));
        assert_eq!(turn.session_id.as_deref(), Some("93b16ca2-62a5-493a-ad7a-fd2d2d4df973"));
    }

    #[test]
    fn a_payload_naming_no_session_leaves_it_unset() {
        let turn = turn_ended_from_hook(HookEvent::Stop, &hook(LIVE_STOP_WITH_BACKGROUND), None);
        assert!(turn.session_id.is_none());
    }

    /// `#178`: `factory task run --continue <id>` parses to `continue_run`.
    #[test]
    fn task_run_continue_flag_parses() {
        let cli = Cli::try_parse_from(["factory", "task", "run", "t1", "--continue"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Task(TaskCmd::Run { id: Some(ref id), continue_run: true, .. }) if id == "t1"
        ));
    }

    #[test]
    fn task_run_without_the_flag_does_not_continue() {
        let cli = Cli::try_parse_from(["factory", "task", "run", "t1"]).unwrap();
        assert!(matches!(cli.command, Command::Task(TaskCmd::Run { continue_run: false, .. })));
    }

    #[test]
    fn a_running_background_task_means_the_turn_only_paused() {
        let turn = turn_ended_from_hook(HookEvent::Stop, &hook(LIVE_STOP_WITH_BACKGROUND), None);
        assert_eq!(turn.pending_background, 1);
    }

    #[test]
    fn finished_background_tasks_do_not_count_but_session_crons_do() {
        let payload = hook(
            r#"{"background_tasks":[{"status":"completed"},{"status":"failed"},{"status":"killed"}],
                "session_crons":[{"id":"c1"}]}"#,
        );
        let turn = turn_ended_from_hook(HookEvent::Stop, &payload, None);
        assert_eq!(turn.pending_background, 1, "only the cron will wake it");

        // A state this build has never heard of is not assumed to be over.
        let unknown = hook(r#"{"background_tasks":[{"status":"queued"}]}"#);
        assert_eq!(turn_ended_from_hook(HookEvent::Stop, &unknown, None).pending_background, 1);
    }

    #[test]
    fn the_documented_object_form_of_the_last_message_is_read_too() {
        let payload = hook(r#"{"last_assistant_message":{"type":"text","text":"I'll help"}}"#);
        let turn = turn_ended_from_hook(HookEvent::Stop, &payload, None);
        assert_eq!(turn.last_message.as_deref(), Some("I'll help"));
    }

    #[test]
    fn a_stop_failure_carries_the_api_error() {
        let payload = hook(
            r#"{"hook_event_name":"StopFailure","error":"server_error","error_details":"Connection reset"}"#,
        );
        let turn = turn_ended_from_hook(HookEvent::StopFailure, &payload, None);
        assert_eq!(turn.event, factory_core::task::TurnEndEvent::StopFailure);
        assert_eq!(turn.error.as_deref(), Some("server_error"));
        assert_eq!(turn.error_details.as_deref(), Some("Connection reset"));
        assert!(turn.last_message.is_none(), "absent when no message was started");
    }

    #[test]
    fn no_payload_at_all_is_still_a_turn_that_ended() {
        // Run by hand, or a harness that sends nothing: the event itself is
        // the fact, and nothing pending is the honest reading of silence.
        let turn = turn_ended_from_hook(HookEvent::Stop, &serde_json::Value::Null, Some(String::new()));
        assert_eq!(turn.pending_background, 0);
        assert!(turn.last_message.is_none());
        assert!(turn.token.is_none(), "an empty token is no token");
    }

    #[test]
    fn a_long_last_message_keeps_its_end() {
        let long = format!("{}THE END", "x".repeat(HOOK_LAST_MESSAGE_BYTE_CAP * 2));
        let payload = serde_json::json!({ "last_assistant_message": long });
        let turn = turn_ended_from_hook(HookEvent::Stop, &payload, None);
        let kept = turn.last_message.unwrap();
        assert!(kept.len() <= HOOK_LAST_MESSAGE_BYTE_CAP);
        assert!(kept.ends_with("THE END"));
    }

    // -- quality -------------------------------------------------------------

    #[test]
    fn quality_parses_with_and_without_a_subcommand() {
        let cli = Cli::try_parse_from(["factory", "quality"]).unwrap();
        assert!(matches!(cli.command, Command::Quality { scope: None, command: None }));

        let cli = Cli::try_parse_from(["factory", "quality", "--scope", "demo", "status"]).unwrap();
        assert!(matches!(cli.command, Command::Quality { scope: Some(_), command: Some(QualityCmd::Status { scope: None }) }));

        let cli = Cli::try_parse_from(["factory", "quality", "scope", "demo", "--json"]).unwrap();
        assert!(cli.json);
        assert!(matches!(cli.command, Command::Quality { command: Some(QualityCmd::Scope { ref name }), .. } if name == "demo"));

        let cli = Cli::try_parse_from(["factory", "quality", "findings", "--scope", "demo"]).unwrap();
        assert!(matches!(cli.command, Command::Quality { command: Some(QualityCmd::Findings { scope: Some(_) }), .. }));

        let cli = Cli::try_parse_from([
            "factory", "quality", "remediate", "reliability.recoverability/daemon-restart", "--scope", "demo",
        ])
        .unwrap();
        assert!(matches!(cli.command, Command::Quality { command: Some(QualityCmd::Remediate { .. }), .. }));
        assert!(
            Cli::try_parse_from(["factory", "quality", "remediate", "reliability/x"]).is_err(),
            "--scope is required: a remediation task lands in one named scope"
        );
    }

    #[test]
    fn a_quality_target_is_attribute_then_scenario() {
        assert_eq!(
            parse_quality_target("reliability.recoverability/daemon-restart").unwrap(),
            ("reliability.recoverability".to_string(), "daemon-restart".to_string())
        );
        for bad in ["reliability", "/x", "reliability/", "a/b/c"] {
            assert!(parse_quality_target(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_hook_command_factory_writes_parses() {
        // The exact argv `claude_turn_end_settings` puts in a hook, minus the
        // binary: if this stops parsing, every hook silently does nothing.
        for event in ["stop", "stop-failure"] {
            let cli = Cli::try_parse_from(["factory", "task", "turn-ended", "t1", "--event", event])
                .unwrap_or_else(|e| panic!("{event}: {e}"));
            assert!(matches!(cli.command, Command::Task(TaskCmd::TurnEnded { .. })));
        }
    }

    #[test]
    fn dependency_commands_parse_in_the_documented_forms() {
        let attach = Cli::try_parse_from([
            "factory", "task", "attach", "--kind", "sbom", "scan.cdx.json",
        ]).unwrap();
        assert!(matches!(attach.command, Command::Task(TaskCmd::Attach {
            kind: AttachmentKind::Sbom, ref file, id: None, ..
        }) if file == Path::new("scan.cdx.json")));

        let read = Cli::try_parse_from(["factory", "dependencies", "demo"]).unwrap();
        assert!(matches!(read.command, Command::Dependencies { ref args } if args == &["demo"]));
        let vex = Cli::try_parse_from(["factory", "dependencies", "vex", "demo"]).unwrap();
        assert!(matches!(vex.command, Command::Dependencies { ref args } if args == &["vex", "demo"]));
    }

    #[test]
    fn dependency_rows_show_the_product_version_and_commit_when_present() {
        let report: DependenciesReport = serde_json::from_value(serde_json::json!({
            "scope": "factory",
            "documents": [{
                "state": "built",
                "sbom": {
                    "attachment": {
                        "id": "s1", "kind": "sbom", "scope": "factory", "run_id": "r1",
                        "task_id": "t1", "attempt": 1, "attached_at": "2026-09-25T12:00:00Z",
                        "filename": "build.cdx.json", "spec_version": "1.6", "states": ["built"]
                    },
                    "identity": { "version": "0.1.0", "git_sha": "0123456789abcdef" }
                }
            }],
            "findings": [],
            "services": []
        }))
        .unwrap();
        let text = dependencies_text(&report);
        assert!(text.contains("built"));
        assert!(text.contains("v0.1.0 0123456789abcdef"));
    }

    #[test]
    fn dependency_reachability_text_preserves_raw_evidence_and_original_scan() {
        let mut report: DependenciesReport = serde_json::from_value(serde_json::json!({
            "scope":"demo", "documents":[], "services":[], "findings":[{
                "id":"CVE-REACH", "state":"built", "status":"open", "severity":"high",
                "affected":{"bom_ref":"pkg:cargo/demo@1", "name":"demo", "path":[]},
                "scan":{"attachment":{"id":"s", "kind":"vulnerabilities", "scope":"demo", "run_id":"r", "task_id":"t", "attempt":1,
                    "attached_at":"2026-10-01T00:00:00Z", "filename":"v.cdx.json", "spec_version":"1.6", "states":[]}},
                "reachability":"unreachable", "analysis_detail":"raw evidence; not a verdict"
            }]
        })).unwrap();
        let text = dependencies_text(&report);
        assert!(text.contains("reported reachability: unreachable"));
        assert!(text.contains("analysis detail: raw evidence; not a verdict"));
        assert!(text.contains("open"));
        report.findings[0].analysis_scan = Some(report.findings[0].scan.clone());
        report.findings[0].scan.attachment.id = "newer-absence".into();
        report.findings[0].reachability = None;
        let text = dependencies_text(&report);
        assert!(text.contains("reported reachability: unknown"));
        assert!(text.contains("analysis evidence: document s run r at"));
        assert!(!text.contains("document newer-absence"));
    }

    #[test]
    fn dependency_observation_text_preserves_dispositions_provenance_and_unknowns() {
        let report: DependenciesReport = serde_json::from_value(serde_json::json!({
            "scope": "demo", "documents": [], "findings": [], "services": [],
            "service_evidence": {"scope": "demo", "captures": [{"id": "capture", "run_id": "run", "task_id": "task",
                "agent": "curator", "sandbox": "factory-run", "source": "OpenShell supervisor/proxy OCSF", "partial": true,
                "captured_at": "2026-10-04T12:00:00Z", "accesses": [{"at": "2026-10-04T11:59:00Z", "transport": "network",
                    "target": "api.github.com:443", "disposition": "denied"}]}]}
        })).unwrap();
        let text = dependencies_text(&report);
        assert!(text.contains("api.github.com:443") && text.contains("Denied"));
        assert!(text.contains("run run task task agent curator") && text.contains("partial"));
        assert!(text.contains("not a verdict") && text.contains("Socket and file use are unknown"));
    }

    // -- --timezone ----------------------------------------------------------

    #[test]
    fn a_cron_schedule_takes_its_timezone_from_the_flag() {
        let s = parse_schedule("0 9 * * 1", Some("Europe/Berlin")).unwrap();
        assert_eq!(
            s,
            Schedule::Cron(CronSchedule { expr: "0 9 * * 1".into(), timezone: Some("Europe/Berlin".into()) })
        );
        assert_eq!(describe_schedule(&s), "cron 0 9 * * 1 (Europe/Berlin)");
        assert_eq!(parse_schedule("cron 0 7 * * 1", None).unwrap(), Schedule::Cron("0 7 * * 1".into()));
    }

    #[test]
    fn an_interval_with_a_timezone_is_refused() {
        let err = parse_schedule("every 5m", Some("Europe/Berlin")).unwrap_err().to_string();
        assert!(err.contains("--timezone is for a cron schedule"), "{err}");
    }

    #[test]
    fn a_timezone_on_its_own_is_refused_by_the_parser() {
        // An edit restates the whole schedule; a zone with no expression
        // would have to guess which schedule it belongs to.
        for sub in ["create", "edit"] {
            let args: Vec<&str> = match sub {
                "create" => vec!["factory", "task", "create", "t", "--timezone", "Europe/Berlin"],
                _ => vec!["factory", "task", "edit", "id", "--timezone", "Europe/Berlin"],
            };
            assert!(Cli::try_parse_from(args).is_err(), "{sub}");
        }
        assert!(Cli::try_parse_from([
            "factory", "task", "edit", "id", "--schedule", "0 9 * * 1", "--timezone", "Europe/Berlin",
        ])
        .is_ok());
    }

    #[test]
    fn task_estimate_shorthand_and_ranges_parse_without_ambiguity() {
        let point = estimate_from_args(None, Some(900), None, None, None, None)
            .unwrap()
            .unwrap();
        assert_eq!(point.time, factory_core::task::TimeEstimateRange::point(900));
        let range = estimate_from_args(
            Some(600),
            Some(900),
            Some(1800),
            Some(1.0),
            Some(2.0),
            Some(4.0),
        )
        .unwrap()
        .unwrap();
        assert_eq!(range.time.low, 600);
        assert_eq!(range.cost.unwrap().expected, 2.0);
        assert!(estimate_from_args(Some(600), Some(900), None, None, None, None).is_err());
        assert!(estimate_from_args(None, None, None, Some(1.0), Some(2.0), Some(3.0)).is_err());
    }

    #[test]
    fn infra_prints_unreadable_facts_as_dashes_and_lists_the_unassigned() {
        use factory_core::config::{ProviderKind, ProviderVia};
        use factory_core::protocol::*;
        let payload = Payload::Infrastructure {
            host: HostFacts { arch: Some("aarch64".into()), ..Default::default() },
            daemon: DaemonFacts {
                version: "0.1.0".into(),
                pid: 42,
                started_at: chrono::Utc::now(),
                root: "/inst".into(),
                store: StoreFacts { kind: "sqlite".into(), path: ".factory/factory.sqlite".into(), size_bytes: None },
                socket: ".factory/factory.sock".into(),
                interfaces: vec![
                    InterfaceFacts { kind: "cli".into(), bind: None },
                    InterfaceFacts { kind: "http".into(), bind: Some("127.0.0.1:8787".into()) },
                ],
                runtime: "herdr".into(),
                herdr_session: Some("factory".into()),
            },
            providers: vec![ProviderRow {
                name: "openrouter".into(),
                vendor: "openrouter".into(),
                kind: ProviderKind::ApiKey,
                plan: None,
                env: Some("OPENROUTER_API_KEY".into()),
                agents: vec![ProviderAgent {
                    scope: "lab".into(),
                    agent: "model-lab".into(),
                    harness: "opencode".into(),
                    via: ProviderVia::Agent,
                }],
                windows: Vec::new(),
                active_runs: Vec::new(),
                usage_unknown: None,
            }],
            unassigned: vec![UnassignedAgent { scope: "demo".into(), agent: "helper".into(), harness: "codex".into() }],
            harnesses: vec![factory_core::harness::HarnessRow {
                harness: "codex".into(),
                binary: "/opt/homebrew/bin/codex".into(),
                state: factory_core::harness::HarnessState::Unhealthy,
                checked_at: Some(chrono::Utc::now()),
                unhealthy_since: Some(chrono::Utc::now()),
                version: None,
                reason: Some("`/opt/homebrew/bin/codex --version` did not answer in 10s".into()),
                repair: Some("scripts/repair-harness codex".into()),
                held: vec![factory_core::harness::HeldTask {
                    task_id: "t1".into(),
                    scope: "demo".into(),
                    title: "fix it".into(),
                }],
                auto_repair: None,
            }],
        };
        let text = infrastructure_text(&payload).unwrap();
        assert!(text.contains("HARNESSES") && text.contains("unhealthy"), "{text}");
        assert!(text.contains("repair: scripts/repair-harness codex"), "{text}");
        assert!(text.contains("held: 1 task(s) -- t1 (fix it)"), "{text}");
        assert!(text.contains("HOST  --"), "{text}");
        assert!(text.contains("chip        --"), "{text}");
        assert!(text.contains("-- (aarch64)"), "{text}");
        assert!(text.contains("cli, http 127.0.0.1:8787"), "{text}");
        assert!(text.contains("herdr (session factory)"), "{text}");
        assert!(text.contains("api-key, key in $OPENROUTER_API_KEY"), "{text}");
        assert!(text.contains("lab/model-lab") && text.contains("via agent"), "{text}");
        assert!(text.contains("UNASSIGNED") && text.contains("demo/helper"), "{text}");
    }

    #[test]
    fn infra_sizes_and_durations_read_like_a_person_wrote_them() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(68_719_476_736), "64.0 GiB");
        assert_eq!(bytes(1_995_218_165_760), "1.8 TiB");
        assert_eq!(duration(42), "42s");
        assert_eq!(duration(3 * 3600 + 12 * 60), "3h 12m");
        assert_eq!(duration(864_000 + 4 * 3600), "10d 4h");
    }

    // -- factory stats, and the actions' reasons (#106) --------------------

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("factory").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn deployment_mirror_publication_requires_a_specific_approval() {
        assert!(matches!(parse(&["deploy", "mirror-plan", "d1"]).command,
            Command::Deploy(DeployCmd::MirrorPlan { id }) if id == "d1"));
        assert!(matches!(parse(&["deploy", "publish", "d1", "--approval", "digest"]).command,
            Command::Deploy(DeployCmd::Publish { id, approval }) if id == "d1" && approval == "digest"));
        assert!(Cli::try_parse_from(["factory", "deploy", "publish", "d1"]).is_err());
    }

    #[test]
    fn intake_parses_its_subcommands_and_refuses_an_unknown_decision() {
        assert!(matches!(parse(&["intake"]).command, Command::Intake { scope: None, command: None }));
        match parse(&["intake", "add", "Broken link", "-i", "the footer", "--scope", "web", "--reference", "mail-7"]).command {
            Command::Intake { command: Some(IntakeCmd::Add { title, scope: Some(s), reference: Some(r), .. }), .. } => {
                assert_eq!((title.as_str(), s.as_str(), r.as_str()), ("Broken link", "web", "mail-7"));
            }
            _ => panic!("not an add"),
        }
        match parse(&["intake", "decide", "abc", "needs-info", "--question", "which page?", "--question", "since when?"])
            .command
        {
            Command::Intake { command: Some(IntakeCmd::Decide { decision, questions, .. }), .. } => {
                assert_eq!(decision, "needs-info");
                assert_eq!(questions.len(), 2);
            }
            _ => panic!("not a decide"),
        }
        assert!(matches!(
            parse(&["intake", "assess", "abc", "--file", "-", "--decide"]).command,
            Command::Intake { command: Some(IntakeCmd::Assess { decide: true, .. }), .. }
        ));
        let bad = Cli::try_parse_from(["factory", "intake", "decide", "abc", "maybe"]);
        assert!(bad.is_err(), "only ready, needs-info, split or wontfix");
        match parse(&["intake", "decide", "abc", "split", "--file", "parts.json"]).command {
            Command::Intake { command: Some(IntakeCmd::Decide { decision, file: Some(f), .. }), .. } => {
                assert_eq!((decision.as_str(), f.to_str().unwrap()), ("split", "parts.json"));
            }
            _ => panic!("not a split"),
        }
        assert!(matches!(
            parse(&["intake", "info", "abc", "the answer", "--triage", "--agent", "codex"]).command,
            Command::Intake { command: Some(IntakeCmd::Info { triage: true, agent: Some(_), .. }), .. }
        ));
        assert!(Cli::try_parse_from(["factory", "intake", "info", "abc", "x", "--agent", "codex"]).is_err());
        match parse(&["intake", "publish", "abc"]).command {
            Command::Intake { command: Some(IntakeCmd::Publish { id }), .. } => assert_eq!(id, "abc"),
            _ => panic!("not a publish"),
        }
    }

    #[test]
    fn intake_parses_the_relay_flags_and_refuses_an_unknown_source() {
        match parse(&[
            "intake",
            "add",
            "Invoice question",
            "--source",
            "email",
            "--provider",
            "apple-mail",
            "--reference",
            "<abc@x>",
            "--requester",
            "a@b.c",
            "--received-at",
            "2026-09-26T08:00:00Z",
        ])
        .command
        {
            Command::Intake {
                command:
                    Some(IntakeCmd::Add {
                        source: Some(source),
                        provider: Some(provider),
                        reference: Some(reference),
                        requester: Some(requester),
                        received_at: Some(received_at),
                        ..
                    }),
                ..
            } => {
                assert_eq!(
                    (source.as_str(), provider.as_str(), reference.as_str(), requester.as_str(), received_at.as_str()),
                    ("email", "apple-mail", "<abc@x>", "a@b.c", "2026-09-26T08:00:00Z")
                );
            }
            _ => panic!("not a relayed add"),
        }
        match parse(&["intake", "add", "A chat request", "--source", "chat"]).command {
            Command::Intake { command: Some(IntakeCmd::Add { source: Some(s), .. }), .. } => assert_eq!(s, "chat"),
            _ => panic!("not a chat add"),
        }
        assert!(
            Cli::try_parse_from(["factory", "intake", "add", "x", "--source", "github"]).is_err(),
            "github is never a caller-chosen source"
        );
    }

    #[test]
    fn intake_parses_the_security_fast_lane_subcommands() {
        match parse(&["intake", "add", "Possible RCE", "--scope", "web", "--security"]).command {
            Command::Intake { command: Some(IntakeCmd::Add { security: true, .. }), .. } => {}
            _ => panic!("expected --security to parse"),
        }
        match parse(&["intake", "add", "Ordinary bug", "--scope", "web"]).command {
            Command::Intake { command: Some(IntakeCmd::Add { security: false, .. }), .. } => {}
            _ => panic!("--security absent should default to false"),
        }
        match parse(&["intake", "flag-security", "abc", "--reason", "looks like an injection"]).command {
            Command::Intake { command: Some(IntakeCmd::FlagSecurity { id, reason }), .. } => {
                assert_eq!((id.as_str(), reason.as_str()), ("abc", "looks like an injection"));
            }
            _ => panic!("not a flag-security"),
        }
        match parse(&["intake", "security", "abc", "confirm"]).command {
            Command::Intake { command: Some(IntakeCmd::Security { id, verdict, evidence: None }), .. } => {
                assert_eq!((id.as_str(), verdict.as_str()), ("abc", "confirm"));
            }
            _ => panic!("not a confirm"),
        }
        match parse(&["intake", "security", "abc", "dismiss", "--evidence", "false positive"]).command {
            Command::Intake { command: Some(IntakeCmd::Security { id, verdict, evidence: Some(e) }), .. } => {
                assert_eq!((id.as_str(), verdict.as_str(), e.as_str()), ("abc", "dismiss", "false positive"));
            }
            _ => panic!("not a dismiss"),
        }
        assert!(
            Cli::try_parse_from(["factory", "intake", "security", "abc", "maybe"]).is_err(),
            "only confirm or dismiss"
        );
        match parse(&["intake", "security-reports"]).command {
            Command::Intake { command: Some(IntakeCmd::SecurityReports { scope: None }), .. } => {}
            _ => panic!("not a security-reports"),
        }
        match parse(&["intake", "security-reports", "--scope", "web"]).command {
            Command::Intake { command: Some(IntakeCmd::SecurityReports { scope: Some(s) }), .. } => {
                assert_eq!(s, "web");
            }
            _ => panic!("not a scoped security-reports"),
        }
        // `--scope` before the subcommand is left for the run loop's own
        // merge to fold in (the same as `list` and `add`) -- parsing alone
        // leaves it on the outer `Command::Intake`.
        match parse(&["intake", "--scope", "web", "security-reports"]).command {
            Command::Intake { scope: Some(outer), command: Some(IntakeCmd::SecurityReports { scope: None }) } => {
                assert_eq!(outer, "web");
            }
            _ => panic!("not a pre-subcommand scope waiting to be merged"),
        }
    }

    #[test]
    fn stats_takes_its_flags_before_or_after_the_subcommand() {
        match parse(&["stats"]).command {
            Command::Stats { scope: None, window: None, command: None } => {}
            _ => panic!("bare `factory stats`"),
        }
        match parse(&["stats", "--scope", "demo", "--window", "30d"]).command {
            Command::Stats { scope: Some(s), window: Some(HealthWindow::Month), command: None } => assert_eq!(s, "demo"),
            _ => panic!("flags before"),
        }
        match parse(&["stats", "attention", "--scope", "demo"]).command {
            Command::Stats { command: Some(StatsCmd::Attention { scope: Some(s) }), .. } => assert_eq!(s, "demo"),
            _ => panic!("attention"),
        }
        match parse(&["stats", "summary", "--window", "7d"]).command {
            Command::Stats { command: Some(StatsCmd::Summary { window: Some(HealthWindow::Week), .. }), .. } => {}
            _ => panic!("summary"),
        }
        assert!(Cli::try_parse_from(["factory", "stats", "--window", "9d"]).is_err());
    }

    #[test]
    fn metrics_takes_scope_and_dashboard_window_presets() {
        match parse(&[
            "metrics",
            "agent_hours",
            "--scope",
            "demo",
            "--window",
            "14d",
        ])
        .command
        {
            Command::Metrics {
                ids,
                scope: Some(scope),
                window: Some(MetricsWindow::FourteenDays),
            } => {
                assert_eq!(ids, vec!["agent_hours"]);
                assert_eq!(scope, "demo");
            }
            _ => panic!("metrics flags"),
        }
        assert!(Cli::try_parse_from(["factory", "metrics", "--window", "7d"]).is_err());
    }

    #[test]
    fn upstream_triggers_and_explicit_overrides_have_unambiguous_cli_flags() {
        match parse(&["task", "create", "child", "--after", "a", "--after", "b"]).command {
            Command::Task(TaskCmd::Create { after, .. }) => assert_eq!(after, vec!["a", "b"]),
            _ => panic!("after create"),
        }
        assert!(Cli::try_parse_from(["factory", "task", "create", "child", "--after", "a", "--run"]).is_err());
        assert!(Cli::try_parse_from(["factory", "task", "create", "child", "--after", "a", "--schedule", "every 5m"]).is_err());
        assert!(Cli::try_parse_from(["factory", "task", "run", "child", "--override-wait"]).is_err());
        match parse(&["task", "run", "child", "--override-wait", "--reason", "investigate"]).command {
            Command::Task(TaskCmd::Run { override_wait: true, reason: Some(reason), .. }) => assert_eq!(reason, "investigate"),
            _ => panic!("after override"),
        }
    }

    #[test]
    fn a_created_upstream_wait_is_not_described_as_manual_work() {
        let mut task = created(None);
        task.after = Some(vec!["parent-id".into()]);
        task.after_condition = Some("conditional: review route".into());
        assert!(one_line(&task).contains("waiting on parent-id; conditional: review route"));
        let note = created_note(&task, false);
        assert!(note.contains("starts once its upstream tasks finish"));
        assert!(!note.contains("nothing starts it"));
    }

    #[test]
    fn the_actions_carry_a_reason_and_an_answer_needs_one() {
        match parse(&["task", "run", "t1", "--reason", "try again"]).command {
            Command::Task(TaskCmd::Run { reason: Some(r), .. }) => assert_eq!(r, "try again"),
            _ => panic!("run"),
        }
        match parse(&["task", "cancel", "t1"]).command {
            Command::Task(TaskCmd::Cancel { reason: None, .. }) => {}
            _ => panic!("cancel"),
        }
        match parse(&["task", "skip-next", "t1", "--reason", "holiday"]).command {
            Command::Task(TaskCmd::SkipNext { id: Some(id), reason: Some(_) }) => assert_eq!(id, "t1"),
            _ => panic!("skip-next"),
        }
        match parse(&["task", "edit", "t1", "--pause-schedule", "--reason", "stop the line"]).command {
            Command::Task(TaskCmd::Edit { pause_schedule: true, reason: Some(_), .. }) => {}
            _ => panic!("edit"),
        }
        match parse(&["run", "answer", "r1", "use staging", "--reason", "it asked"]).command {
            Command::Run(RunCmd::Answer { id, text, reason }) => {
                assert_eq!((id.as_str(), text.as_str(), reason.as_str()), ("r1", "use staging", "it asked"));
            }
            _ => panic!("answer"),
        }
        assert!(Cli::try_parse_from(["factory", "run", "answer", "r1", "yes"]).is_err(), "no reason, no answer");
        match parse(&["task", "close", "t1", "--reason", "not_planned", "--note", "the client dropped it"]).command {
            Command::Task(TaskCmd::Close { id: Some(id), reason: CloseReason::NotPlanned, duplicate_of: None, note: Some(note) }) => {
                assert_eq!((id.as_str(), note.as_str()), ("t1", "the client dropped it"));
            }
            _ => panic!("close"),
        }
        match parse(&["task", "close", "t1", "--reason", "duplicate", "--duplicate-of", "t0"]).command {
            Command::Task(TaskCmd::Close { reason: CloseReason::Duplicate, duplicate_of: Some(of), .. }) => assert_eq!(of, "t0"),
            _ => panic!("close duplicate"),
        }
        assert!(Cli::try_parse_from(["factory", "task", "close", "t1"]).is_err(), "no reason, no close");
        assert!(Cli::try_parse_from(["factory", "task", "close", "t1", "--reason", "meh"]).is_err());
        match parse(&["task", "reopen", "t1"]).command {
            Command::Task(TaskCmd::Reopen { id: Some(_), reason: None }) => {}
            _ => panic!("reopen"),
        }
    }

    #[test]
    fn task_list_status_takes_failed_and_closed_as_well_as_a_stored_status() {
        assert_eq!("failed".parse::<StatusFilter>().unwrap(), StatusFilter::Failed);
        assert_eq!("closed".parse::<StatusFilter>().unwrap(), StatusFilter::Closed);
        assert_eq!("pending".parse::<StatusFilter>().unwrap(), StatusFilter::Is(TaskStatus::Pending));
        assert!("nonsense".parse::<StatusFilter>().is_err());
        assert_eq!(StatusFilter::Failed.wire(), Some(TaskStatus::Blocked));
        assert_eq!(StatusFilter::Closed.wire(), None);
        match parse(&["task", "list", "--parent", "parent-id"]).command {
            Command::Task(TaskCmd::List { parent: Some(parent), .. }) => assert_eq!(parent, "parent-id"),
            _ => panic!("task list --parent"),
        }
    }

    #[test]
    fn an_empty_report_reads_as_nothing_needing_anyone() {
        let now = chrono::Utc::now();
        let report = ops::report(&ops::OperationsInput { now, ..Default::default() });
        assert_eq!(attention_text(&report), "Nothing needs you.");
        let text = stats_text(&report);
        assert!(text.contains("NEEDS ATTENTION  none"), "{text}");
        assert!(text.contains("HEALTH  last 7d"), "{text}");
        assert!(text.contains("no run records queue waits"), "{text}");
    }

    // -- usage and cost (#117) ----------------------------------------------

    #[test]
    fn budget_cli_defaults_to_scope_and_accepts_provider_account_grouping() {
        let cli = Cli::try_parse_from(["factory", "budget"]).unwrap();
        assert!(matches!(cli.command, Command::Budget { by, scope: None } if by == "scope"));
        let cli = Cli::try_parse_from(["factory", "budget", "--scope", "work", "--by", "provider"]).unwrap();
        assert!(matches!(cli.command, Command::Budget { by, scope: Some(scope) } if by == "provider" && scope == "work"));
    }

    #[test]
    fn budget_cli_shows_unknown_remaining_and_unattributed_scoped_spend() {
        use factory_core::budget;
        use factory_core::usage::{CostGroupBy, CostReport, CostRow};
        let month = budget::Month::at(chrono::Utc::now()).unwrap();
        let spent = CostRow { runs: 1, runs_unknown: 1, ..CostRow::new("total", None) };
        let assessment = budget::assess(Some(50.0), &spent, 1, &month);
        let report = budget::Report { catalogue: ".factory/budgets/limits.yaml".into(), group_by: CostGroupBy::Scope,
            spend: CostReport { basis: Default::default(), finished: None, group_by: CostGroupBy::Scope, from: month.from, to: month.as_of, scope: Some("work".into()), rows: vec![spent.clone()], total: spent.clone(), unattributed_runs: 1, daily: Vec::new() },
            budgets: vec![budget::ScopeBudget { id: "stable".into(), scope: "work".into(), path: "projects/work".into(), relation: "ancestor".into(), monthly_usd: Some(50.0), spent, unattributed_runs: 1, daily: Vec::new(), assessment }],
            month, findings: Vec::new() };
        let text = budget_text(&report);
        assert!(text.contains("remaining unknown") && text.contains("month projection unknown"), "{text}");
        assert!(text.contains("unknown scope attribution") && text.contains("[ancestor]"), "{text}");
        assert!(text.contains("Subtree caps overlap: they are not summed."), "{text}");
    }

    #[test]
    fn a_when_is_a_span_back_a_date_or_rfc3339() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-25T12:00:00Z").unwrap().with_timezone(&chrono::Utc);
        assert_eq!(parse_when("7d", now).unwrap().to_rfc3339(), "2026-09-18T12:00:00+00:00");
        assert_eq!(parse_when("12h", now).unwrap().to_rfc3339(), "2026-09-25T00:00:00+00:00");
        assert_eq!(parse_when("2026-09-01", now).unwrap().to_rfc3339(), "2026-09-01T00:00:00+00:00");
        assert_eq!(
            parse_when("2026-09-01T10:00:00+02:00", now).unwrap().to_rfc3339(),
            "2026-09-01T08:00:00+00:00"
        );
        assert!(parse_when("last tuesday", now).is_err());
    }

    #[test]
    fn an_unknown_count_prints_as_a_question_mark_and_an_unknown_run_says_why() {
        use factory_core::usage::{RunUsage, TokenCounts, UsageState};
        let mut u = RunUsage::unknown("the herdr runtime has no source for usage", 1);
        assert_eq!(usage_block(&u), "usage      unknown -- the herdr runtime has no source for usage");
        u.state = UsageState::Known;
        u.reason = None;
        u.tokens = TokenCounts { input: Some(61_000), output: Some(900), cache_read: None, cache_write: Some(0) };
        u.cost_usd = Some(2.7);
        u.pricing_sources = vec!["litellm@x".into()];
        let text = usage_block(&u);
        assert!(text.contains("61.0k in, 900 out, ? cache read, 0 cache write (total ?)"), "{text}");
        assert!(text.contains("$2.70 priced by litellm@x"), "{text}");
        assert_eq!(fmt_usd(0.001), "<$0.01");
        assert_eq!(fmt_usd(0.0), "$0.00");
    }

    #[test]
    fn the_cost_table_says_how_many_runs_are_not_in_its_sums() {
        use factory_core::usage::{CostGroupBy, CostReport, CostRow};
        let mut row = CostRow::new("issue=117", None);
        row.add(None);
        let report = CostReport {
            basis: Default::default(), finished: None,
            group_by: CostGroupBy::Issue,
            from: chrono::Utc::now() - chrono::Duration::days(30),
            to: chrono::Utc::now(),
            scope: None,
            rows: vec![row.clone()],
            total: CostRow { key: "total".into(), ..row },
            unattributed_runs: 0,
            daily: Vec::new(),
        };
        let text = costs_text(&report);
        assert!(text.contains("issue=117"), "{text}");
        assert!(!text.contains("$0.00"), "a group with nothing measured is ?, not free: {text}");
        assert!(text.contains("1 of 1 runs have no measured cost"), "{text}");
    }

    #[test]
    fn the_cost_table_shows_estimate_vs_actual_per_group_when_there_is_one() {
        use factory_core::usage::{CostGroupBy, CostReport, CostRow};
        let mut row = CostRow::new("scope=web", None);
        row.runs = 5;
        row.estimated_runs = 5;
        row.within_range = 4;
        row.median_actual_over_expected = Some(1.2);
        let mut nothing = CostRow::new("scope=demo", None);
        nothing.runs = 1;
        let report = CostReport {
            basis: Default::default(), finished: None,
            group_by: CostGroupBy::Scope,
            from: chrono::Utc::now() - chrono::Duration::days(30),
            to: chrono::Utc::now(),
            scope: None,
            rows: vec![row, nothing],
            total: CostRow::new("total", None),
            unattributed_runs: 0,
            daily: Vec::new(),
        };
        let text = costs_text(&report);
        assert!(text.contains("4/5 in range, 1.20x median"), "{text}");
        assert!(text.contains("scope=demo"), "{text}");
        // A group with nothing estimated says so plainly, not "0/0".
        let demo_line = text.lines().find(|l| l.contains("scope=demo")).unwrap();
        assert!(demo_line.contains(" - "), "{demo_line}");
    }

    #[test]
    fn intake_definitions_text_is_silent_for_a_scope_with_nothing_declared() {
        let plain = factory_core::intake::RouteOptions { scope: "demo".into(), ..Default::default() };
        assert_eq!(intake_definitions_text(&[plain]), "");
    }

    #[test]
    fn intake_definitions_text_names_the_scope_its_extra_checks_limits_and_findings() {
        use factory_core::ready::{AppliedCheck, Finding, FindingKind, Origin, ReadyDefinition, Tolerance};
        let route = factory_core::intake::RouteOptions {
            scope: "security-team".into(),
            definition: ReadyDefinition {
                scope: "security-team".into(),
                checks: vec![AppliedCheck {
                    id: "threat-model".into(),
                    pass_condition: "names a threat model".into(),
                    categories: vec!["security-report".into()],
                    declared_at: Origin { scope: "security-team".into(), file: "security".into() },
                }],
                max_complexity: 6,
                observability_tolerance: Tolerance::Low,
                unreadable: vec![],
            },
            findings: vec![Finding {
                kind: FindingKind::Loosening,
                subject: "security-team".into(),
                detail: "max_complexity 8 loosens the inherited 6".into(),
            }],
            ..Default::default()
        };
        let text = intake_definitions_text(&[route]);
        assert!(text.contains("scope `security-team`"), "{text}");
        assert!(text.contains("threat-model") && text.contains("security-report"), "{text}");
        assert!(text.contains("max_complexity: 6"), "{text}");
        assert!(text.contains("observability_tolerance: low"), "{text}");
        assert!(text.contains("loosens the inherited 6"), "{text}");
    }

    #[test]
    fn intake_definitions_text_shows_an_unreadable_definition_even_with_nothing_else_declared() {
        let route = factory_core::intake::RouteOptions {
            scope: "demo".into(),
            definition: factory_core::ready::ReadyDefinition {
                scope: "demo".into(),
                unreadable: vec!["definition of ready for demo could not be read: ...".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let text = intake_definitions_text(&[route]);
        assert!(text.contains("could not be read"), "{text}");
    }

    #[test]
    fn intake_definitions_text_never_says_unreadable_twice() {
        use factory_core::ready::{Finding, FindingKind, ReadyDefinition};
        let route = factory_core::intake::RouteOptions {
            scope: "demo".into(),
            definition: ReadyDefinition {
                scope: "demo".into(),
                unreadable: vec!["definition of ready for demo could not be read: security binds \"security\", but no security.yaml file exists".into()],
                ..Default::default()
            },
            findings: vec![Finding {
                kind: FindingKind::Unreadable,
                subject: "demo".into(),
                detail: "binds \"security\", but no security.yaml file exists".into(),
            }],
            ..Default::default()
        };
        let text = intake_definitions_text(&[route]);
        assert_eq!(text.matches("could not be read").count(), 1, "{text}");
        assert!(!text.contains("finding ["), "an Unreadable finding restates the blocker sentence -- left out: {text}");
    }

    #[test]
    fn a_part_workflow_says_which_node_plays_which_role() {
        let draft: factory_core::workflow::WorkflowDraft =
            serde_yaml_ng::from_str(include_str!("../../../workflows/epic-part.yaml")).unwrap();
        let definition = factory_core::workflow::WorkflowDefinition::from_draft(draft);
        let text = super::workflow_text(&definition);
        assert!(text.contains("part workflow: entry implement, deliverable implement, terminal review"), "{text}");
        let lint = factory_core::workflow::WorkflowLint {
            subject: definition.id.clone(),
            scope: "factory".into(),
            plans: Vec::new(),
            injections: Vec::new(),
            violations: Vec::new(),
            injected: None,
            part: Some(definition.part_shape().unwrap()),
        };
        assert!(super::lint_text(&lint).contains("over two sample parts (b after a)"));
    }
}
