//! `factory` -- the command-line face of the daemon, and the thing a dispatched
//! agent calls to say how it is getting on.

mod client;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use factory_core::bench::{BenchResult, BenchRun, Verdict};
use factory_core::dataset::{Case, Dataset, DatasetFinding, DatasetSummary};
use factory_core::event::Event;
use factory_core::goals::{self as goals_core, Band, CycleStatus, KrRef};
use factory_core::knowledge::FindingKind;
use factory_core::metrics::MetricId;
use factory_core::policy::{self, ControlRef};
use factory_core::protocol::{GoalsReport, Payload, PolicyControlDetail, PolicyReport, Request, Response};
use factory_core::run::{Run, RunStatus};
use factory_core::task::{
    CronSchedule, NewTask, RetryPolicy, Schedule, Task, TaskFilter, TaskPatch, TaskReport, TaskStatus,
};
use std::path::{Path, PathBuf};

use client::Client;

#[derive(Parser)]
#[command(name = "factory", about = "Talk to a Factory daemon", version)]
struct Cli {
    /// Instance root. Defaults to the nearest ancestor holding a .factory/.
    #[arg(long, global = true, env = "FACTORY_ROOT")]
    root: Option<PathBuf>,

    /// Control socket. Overrides everything else.
    #[arg(long, global = true)]
    socket: Option<PathBuf>,

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
    },
    /// The L6 Goals tab: vision, mission, the north star and its inputs,
    /// every cycle's own summary, the asked (or current) cycle's graded
    /// report, and the roadmap. With no subcommand, prints the status view
    /// -- the same thing `factory goals status` prints.
    Goals {
        #[command(subcommand)]
        command: Option<GoalsCmd>,
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
        /// timestamp.
        #[arg(long)]
        expires: String,
        #[arg(long)]
        note: Option<String>,
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
enum RunCmd {
    /// The runs of a task, newest first.
    List {
        task_id: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// One run.
    Show { id: String },
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
}

#[derive(Subcommand)]
enum TaskCmd {
    /// List tasks.
    List {
        #[arg(long)]
        status: Option<TaskStatus>,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Create a task.
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
        /// The IANA timezone a cron schedule's fields are read in, as in
        /// `Europe/Berlin`. Without it they are UTC.
        #[arg(long, requires = "schedule")]
        timezone: Option<String>,
        /// Expected seconds one run will occupy its agent (advisory only).
        #[arg(long)]
        estimate: Option<u64>,
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
    },
    /// Show one task.
    Show { id: Option<String> },
    /// Dispatch a task now.
    Run { id: Option<String> },
    /// Stop a running task and close its session.
    Cancel { id: Option<String> },
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
        #[arg(long)]
        error: Option<String>,
        /// Defaults to FACTORY_TASK_TOKEN, which the daemon sets in the
        /// session alongside FACTORY_TOKEN.
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
    let client = Client::locate(cli.socket.clone(), cli.root.clone(), cli.token.clone())?;

    match cli.command {
        Command::Status => {
            let payload = client.send(Request::Status).await?;
            print(&payload, cli.json, |p| match p {
                Payload::Status { status } => Some(format!(
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
                    client.socket_path().display(),
                )),
                _ => None,
            })
        }

        Command::Infra => {
            let payload = client.send(Request::Infrastructure).await?;
            print(&payload, cli.json, infrastructure_text)
        }

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
            eprintln!("watching {} -- ctrl-c to stop", client.socket_path().display());
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

        Command::Metrics { ids } => {
            let ids: std::result::Result<Vec<MetricId>, String> = ids.into_iter().map(|s| s.parse()).collect();
            let ids = ids.map_err(|e| anyhow!(e))?;
            let payload = client.send(Request::Metrics { ids }).await?;
            print(&payload, cli.json, |p| match p {
                Payload::Metrics { values, series, registry } => Some(metrics_text(values, series, registry)),
                _ => None,
            })
        }

        Command::Goals { command } => {
            let cmd = command.unwrap_or(GoalsCmd::Status { scope: None, cycle: None });
            goals_cmd(cli.json, &client, cmd).await
        }
    }
}

/// `factory infra`, for a person: the host, the daemon on it, then each
/// declared provider with the agents it serves, then the model agents none
/// claims. A fact the daemon could not read prints as `--`.
fn infrastructure_text(payload: &Payload) -> Option<String> {
    let Payload::Infrastructure { host, daemon, providers, unassigned } = payload else {
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
    Some(out.trim_end().to_string())
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
                    .send(Request::TaskList(TaskFilter { status, scope, limit: None }))
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

        PolicyCmd::Attest { control, scope, evidence, expires, note } => {
            let control: ControlRef = control.parse().map_err(|e: String| anyhow!(e))?;
            let expires_at = policy::parse_expiry(&expires, chrono::Utc::now()).map_err(|e| anyhow!(e))?;
            let payload = client
                .send(Request::PolicyAttest { control, scope, evidence, note, expires_at })
                .await?;
            print(&payload, json, |p| match p {
                Payload::PolicyAttestation { attestation } => Some(attestation_line(attestation)),
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
            limit,
        } => {
            let payload = client
                .send(Request::TaskList(TaskFilter {
                    status,
                    scope,
                    limit,
                }))
                .await?;
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
            timezone,
            estimate,
            ack_timeout,
            timeout,
            blocked_timeout,
            retry,
            labels,
            worktree,
            no_worktree,
            knowledge_hints,
            run,
        } => {
            let schedule = schedule
                .as_deref()
                .map(|text| parse_schedule(text, timezone.as_deref()))
                .transpose()?;
            let retry = retry.as_deref().map(parse_retry).transpose()?;
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
                    title,
                    instructions,
                    scope,
                    agent,
                    runtime,
                    schedule,
                    estimate_seconds: estimate,
                    ack_timeout_seconds: ack_timeout,
                    timeout_seconds: timeout,
                    blocked_timeout_seconds: blocked_timeout,
                    retry,
                    labels: parse_labels(&labels)?,
                    worktree,
                    knowledge_hints,
                }))
                .await?;
            let created = match &payload {
                Payload::Task { task } => task.clone(),
                _ => return Err(anyhow!("unexpected answer to task.create")),
            };
            if run {
                client
                    .send(Request::TaskRun {
                        id: created.id.clone(),
                    })
                    .await?;
            }
            print(&payload, json, |p| match p {
                Payload::Task { task } => Some(format!(
                    "{}\n{}",
                    task.id,
                    one_line(task)
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
        } => {
            let retry = retry.as_deref().map(parse_retry).transpose()?;
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
                estimate_seconds: estimate,
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
                labels: if labels.is_empty() {
                    None
                } else {
                    Some(parse_labels(&labels)?)
                },
                ..Default::default()
            };
            let payload = client
                .send(Request::TaskUpdate {
                    id: need_id(id)?,
                    patch,
                })
                .await?;
            print(&payload, json, |p| match p {
                Payload::Task { task } => Some(detail(task)),
                _ => None,
            })
        }

        TaskCmd::Show { id } => {
            let payload = client.send(Request::TaskGet { id: need_id(id)? }).await?;
            print(&payload, json, |p| match p {
                Payload::Task { task } => Some(detail(task)),
                _ => None,
            })
        }

        TaskCmd::Run { id } => {
            let id = need_id(id)?;
            client.send(Request::TaskRun { id: id.clone() }).await?;
            println!("dispatching {id}");
            Ok(())
        }

        TaskCmd::Cancel { id } => {
            let payload = client.send(Request::TaskCancel { id: need_id(id)? }).await?;
            print(&payload, json, |p| match p {
                Payload::Run { run } => Some(run_line(run)),
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
            result_file,
            error,
            token,
        } => {
            if status.is_none()
                && message.is_none()
                && result.is_none()
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
                        status,
                        message,
                        result,
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
    pairs
        .iter()
        .map(|p| {
            p.split_once('=')
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                .ok_or_else(|| anyhow!("labels look like key=value, not {p:?}"))
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

fn one_line(t: &Task) -> String {
    let runs = match t.runs {
        0 => "         ".to_string(),
        1 => "  1 run  ".to_string(),
        n => format!("{n:>3} runs "),
    };
    format!(
        "{}  {:<12} {} {:<10} {:<12} {}",
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
    if let Some(v) = &r.result {
        s.push_str(&format!("\nresult:\n{v}\n"));
    }
    if let Some(v) = &r.error {
        s.push_str(&format!("\nerror:\n{v}\n"));
    }
    s.trim_end().to_string()
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
    if let Some(sched) = &t.schedule {
        s.push_str(&format!("  schedule   {}\n", describe_schedule(sched)));
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
    if let Some(v) = t.estimate_seconds {
        s.push_str(&format!("  estimate   {v}s\n"));
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
        Event::PolicyChanged { scope, control } => {
            format!("policy   {control} in {scope}  changed")
        }
        Event::GoalsChanged { kr } => {
            format!("goals    {kr}  checked in")
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
    use super::*;

    fn write_temp(bytes: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("factory-cli-test-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, bytes).unwrap();
        path
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
            }],
            unassigned: vec![UnassignedAgent { scope: "demo".into(), agent: "helper".into(), harness: "codex".into() }],
        };
        let text = infrastructure_text(&payload).unwrap();
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
}
