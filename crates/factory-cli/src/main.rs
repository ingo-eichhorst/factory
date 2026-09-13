//! `factory` -- the command-line face of the daemon, and the thing a dispatched
//! agent calls to say how it is getting on.

mod client;

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use factory_core::event::Event;
use factory_core::knowledge::FindingKind;
use factory_core::protocol::{Payload, Request, Response};
use factory_core::run::{Run, RunStatus};
use factory_core::task::{
    NewTask, Schedule, Task, TaskFilter, TaskPatch, TaskReport, TaskStatus,
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
    /// The L5 Knowledge tab: an index of the instance's wiki, rebuilt from
    /// the files on every call. Read-only -- nothing here writes a note.
    Knowledge,
    /// The L5 Benchmarks tab: one configuration per distinct harness, full
    /// arguments and sandbox a task could be dispatched with today, and what
    /// each one is still missing to be a comparable score. Declares and
    /// displays; nothing here runs or scores anything.
    Bench,
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
        /// Repeatable: `--label area=infra`.
        #[arg(long = "label")]
        labels: Vec<String>,
        /// Give this task its own git worktree, made fresh before each run.
        /// On unless you pass `--no-worktree`.
        #[arg(long)]
        worktree: bool,
        #[arg(long)]
        no_worktree: bool,
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
        #[arg(long)]
        schedule: Option<String>,
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
        /// Repeatable; replaces the whole set.
        #[arg(long = "label")]
        labels: Vec<String>,
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

        Command::Knowledge => {
            let payload = client.send(Request::Knowledge).await?;
            print(&payload, cli.json, |p| match p {
                Payload::Knowledge { root, present, notes, gaps, pages, findings } => {
                    if !*present {
                        return Some(format!("no wiki at {root}"));
                    }
                    let total_links: usize = notes.iter().map(|n| n.links.len()).sum();
                    let mut out = format!(
                        "{root}  ({} notes, {total_links} links, {} gaps, {} pages)\n",
                        notes.len(),
                        gaps.len(),
                        pages.len(),
                    );
                    if !notes.is_empty() {
                        out.push_str("\nNOTES\n");
                        for n in notes {
                            out.push_str(&format!(
                                "  {:<28} {:<28} area={:<12} status={:<10} sources={:<3} links={:<3} backlinks={}\n",
                                n.id,
                                n.title,
                                n.area.as_deref().unwrap_or("-"),
                                n.status.as_deref().unwrap_or("-"),
                                n.sources,
                                n.links.len(),
                                n.backlinks.len(),
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
                    if !pages.is_empty() {
                        out.push_str("\nPAGES\n");
                        for page in pages {
                            out.push_str(&format!("  {page}\n"));
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

        Command::Bench => {
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
            estimate,
            ack_timeout,
            timeout,
            blocked_timeout,
            labels,
            worktree,
            no_worktree,
            run,
        } => {
            let schedule = schedule.as_deref().map(parse_schedule).transpose()?;
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
                    labels: parse_labels(&labels)?,
                    worktree,
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
            no_schedule,
            estimate,
            no_estimate,
            ack_timeout,
            timeout,
            blocked_timeout,
            default_timeouts,
            labels,
        } => {
            let patch = TaskPatch {
                title,
                instructions,
                scope,
                agent,
                runtime,
                schedule: schedule.as_deref().map(parse_schedule).transpose()?,
                clear_schedule: no_schedule,
                estimate_seconds: estimate,
                clear_estimate: no_estimate,
                ack_timeout_seconds: ack_timeout,
                timeout_seconds: timeout,
                blocked_timeout_seconds: blocked_timeout,
                clear_ack_timeout: default_timeouts,
                clear_timeout: default_timeouts,
                clear_blocked_timeout: default_timeouts,
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

fn parse_schedule(text: &str) -> Result<Schedule> {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix("every ").or_else(|| text.strip_prefix("every")) {
        let rest = rest.trim();
        let (digits, unit): (String, String) = rest
            .chars()
            .partition(|c| c.is_ascii_digit());
        let n: u64 = digits
            .parse()
            .map_err(|_| anyhow!("`every` wants a number, as in `every 300` or `every 5m`"))?;
        let unit = unit.trim().to_lowercase();
        let seconds = match unit.as_str() {
            "" | "s" | "sec" | "secs" | "second" | "seconds" => n,
            "m" | "min" | "mins" | "minute" | "minutes" => n * 60,
            "h" | "hr" | "hrs" | "hour" | "hours" => n * 3600,
            other => return Err(anyhow!("unknown unit {other:?}; use s, m, or h")),
        };
        return Ok(Schedule::Every { seconds });
    }
    Ok(Schedule::Cron(
        text.strip_prefix("cron ").unwrap_or(text).trim().to_string(),
    ))
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
    }
    if let Some(next) = t.next_run_at {
        s.push_str(&format!("  next run   {}\n", next.to_rfc3339()));
    }
    if let Some(v) = t.estimate_seconds {
        s.push_str(&format!("  estimate   {v}s\n"));
    }
    if t.worktree {
        s.push_str("  worktree   yes, a fresh one before each run\n");
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
        Schedule::Cron(expr) => format!("cron {expr}"),
        Schedule::Every { seconds } => format!("every {seconds}s"),
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
}
