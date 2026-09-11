//! `factory` -- the command-line face of the daemon, and the thing a dispatched
//! agent calls to say how it is getting on.

mod client;

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use factory_core::event::Event;
use factory_core::protocol::{Payload, Request, Response};
use factory_core::task::{NewTask, Schedule, Task, TaskFilter, TaskReport, TaskStatus};
use std::path::PathBuf;

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

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// What the daemon is doing.
    Status,
    /// Which adapters are registered, and where each came from.
    Adapters,
    /// Follow the event stream.
    Watch,
    /// Create, run, and report on tasks.
    #[command(subcommand)]
    Task(TaskCmd),
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
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        runtime: Option<String>,
        /// `every 300`, `every 5m`, or a cron expression.
        #[arg(long)]
        schedule: Option<String>,
        /// Dispatch it immediately as well.
        #[arg(long)]
        run: bool,
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
        status: Option<TaskStatus>,
        #[arg(short, long)]
        message: Option<String>,
        #[arg(long)]
        result: Option<String>,
        #[arg(long)]
        error: Option<String>,
        /// Defaults to FACTORY_TASK_TOKEN, which the daemon sets in the session.
        #[arg(long, env = "FACTORY_TASK_TOKEN")]
        token: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = Client::locate(cli.socket.clone(), cli.root.clone())?;

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

        Command::Task(cmd) => task(cli.json, &client, cmd).await,
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
            run,
        } => {
            let schedule = schedule.as_deref().map(parse_schedule).transpose()?;
            let payload = client
                .send(Request::TaskCreate(NewTask {
                    title,
                    instructions,
                    scope,
                    agent,
                    runtime,
                    schedule,
                    labels: Default::default(),
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
                Payload::Task { task } => Some(one_line(task)),
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
                Payload::Entries { entries } => Some(if entries.is_empty() {
                    "no entries".into()
                } else {
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
                }),
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
            error,
            token,
        } => {
            if status.is_none() && message.is_none() && result.is_none() && error.is_none() {
                return Err(anyhow!(
                    "nothing to report; pass --status, --message, --result, or --error"
                ));
            }
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
                Payload::Task { task } => Some(format!("recorded; {} is {}", task.id, task.status.as_str())),
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
    format!(
        "{}  {:<12} {:<10} {:<12} {}",
        &t.id[..8.min(t.id.len())],
        t.status.as_str(),
        t.scope,
        t.agent,
        t.title
    )
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
    if let Some(session) = &t.session {
        s.push_str(&format!("  session    {} {}\n", session.runtime, session.handle));
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
        Event::TaskEntry { id, entry } => format!(
            "entry    {}  {:<8} {}",
            &id[..8.min(id.len())],
            entry.source,
            entry.message
        ),
        Event::TaskOutput { id, .. } => format!("output   {}", &id[..8.min(id.len())]),
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
