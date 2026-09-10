//! The `factory` command surface, wired to decision 9's operation
//! vocabulary. Every subcommand here is one of `init`, `start|stop|status|
//! doctor`, `scope`, `agent`, `task`, `schedule`, `context` — station 10's
//! declared scope plus station 11's `schedule` group (ADR 0021 decision 11).
//! `secret`, `knowledge`, `memory`, and `agent list` are still absent, not
//! stubbed: there is no variant for them anywhere below.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(name = "factory", about = "The Factory command surface (design §7).")]
pub struct Cli {
    /// The Factory instance root. Every command but `init` resolves it, when
    /// this is absent, from `FACTORY_ROOT` or by walking up from the current
    /// directory for a `.factory/` directory; `init` defaults to the current
    /// directory instead, since its root need not exist yet.
    #[arg(long, global = true)]
    pub root: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create the configuration and database a daemon needs. Idempotent:
    /// running it twice on the same root is safe.
    Init {
        /// The instance name recorded in `config.yaml`. Defaults to the
        /// root directory's own name.
        #[arg(long)]
        name: Option<String>,
    },

    /// Start the daemon in the background.
    Start {
        /// Which harness the daemon's one adapter drives (`pi` or
        /// `claude-code`; see `factory daemon run --help`).
        #[arg(long, default_value = "pi")]
        harness: String,
    },

    /// Stop the running daemon.
    Stop,

    /// Report whether the daemon is running, and its basic facts.
    Status,

    /// Read-only diagnosis of the instance (works without a daemon). Exits
    /// non-zero when it reports any finding.
    Doctor,

    /// The daemon itself (ADR 0002 decision 1) — every other command is a
    /// client of it. `factory start` is what runs this in the background.
    Daemon {
        #[command(subcommand)]
        action: DaemonCommand,
    },

    /// The registered-scope registry (ADR 0016).
    Scope {
        #[command(subcommand)]
        action: ScopeCommand,
    },

    /// Harness sessions (design §2.2, §2.3).
    Agent {
        #[command(subcommand)]
        action: AgentCommand,
    },

    /// Work items (design §2.4, §5).
    Task {
        #[command(subcommand)]
        action: TaskCommand,
    },

    /// Durable cron rules against a task template (design §11; ADR 0021).
    /// A schedule only creates a run; it never executes an untracked
    /// prompt.
    Schedule {
        #[command(subcommand)]
        action: ScheduleCommand,
    },

    /// Compiled context inspection (design §2.5).
    Context {
        #[command(subcommand)]
        action: ContextCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum DaemonCommand {
    /// Run the daemon in the foreground: bind the socket, hold the
    /// installation lock, and serve requests until signalled to stop.
    Run {
        /// Which harness this daemon process's one adapter drives. Only
        /// `pi` and `claude-code` have an adapter at all right now.
        #[arg(long, default_value = "pi")]
        harness: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ScopeCommand {
    /// Register a new scope. Always refused today (decision 9): the daemon
    /// has no YAML writer for `.factory/config.yaml`. Edit it directly, then
    /// `scope reconcile --apply`.
    Add {
        #[arg(long)]
        path: PathBuf,
        #[arg(long)]
        name: String,
    },

    /// Compare `config.yaml` against the registered `scopes` table (and the
    /// filesystem), optionally applying what it is safe to apply.
    Reconcile {
        #[arg(long)]
        apply: bool,
    },

    /// List every registered scope.
    List,
}

#[derive(Subcommand, Debug)]
pub enum AgentCommand {
    /// Start a harness session for `agent_name` in a scope.
    Start {
        #[arg(long)]
        scope: crate::scope_ref::ScopeRef,
        #[arg(long)]
        agent_name: String,
        /// Defaults to the scope's own canonical path.
        #[arg(long)]
        workspace: Option<PathBuf>,
    },

    /// Stop a running session.
    Stop {
        #[arg(long)]
        session: Uuid,
        #[arg(long)]
        reason: Option<String>,
    },

    /// Show one session (and its leases), or every session in a scope.
    Status {
        #[arg(long)]
        scope: crate::scope_ref::ScopeRef,
        #[arg(long)]
        session: Option<Uuid>,
    },

    /// Print the argv the daemon says will attach a terminal to a session's
    /// pane, then exec it — the daemon cannot hand a terminal to you from
    /// inside a worker thread, so this command runs what it returns.
    Attach {
        #[arg(long)]
        session: Uuid,
    },
}

#[derive(Subcommand, Debug)]
pub enum TaskCommand {
    /// Send a prompt as a new task.
    Send {
        #[arg(long)]
        scope: crate::scope_ref::ScopeRef,
        #[arg(long)]
        prompt: String,
        /// Required unless `--target-session` is given.
        #[arg(long)]
        agent_name: Option<String>,
        #[arg(long)]
        target_session: Option<Uuid>,
        #[arg(long)]
        workspace: Option<PathBuf>,
        #[arg(long)]
        sender_session: Option<Uuid>,
        /// Block until the task is terminal (or `blocked`) rather than
        /// returning as soon as it is sent.
        #[arg(long)]
        wait: bool,
        /// Seconds to wait, overriding the default. Requires `--wait`.
        #[arg(long, requires = "wait")]
        timeout: Option<u64>,
    },

    /// Cancel a task.
    Cancel {
        #[arg(long)]
        task_id: Uuid,
    },

    /// Report a task as done. Run by the agent working the task, from
    /// inside its own session — the task id is in its prompt.
    Done {
        #[arg(long)]
        task_id: Uuid,
        #[arg(long)]
        summary: Option<String>,
        #[arg(long = "artifact")]
        artifacts: Vec<String>,
    },

    /// Report a task as failed. See `task done`.
    Fail {
        #[arg(long)]
        task_id: Uuid,
        #[arg(long)]
        summary: Option<String>,
        #[arg(long = "artifact")]
        artifacts: Vec<String>,
    },

    /// Report a task as blocked (clarification, permission, interrupted, or
    /// external). See `task done`.
    Block {
        #[arg(long)]
        task_id: Uuid,
        #[arg(long)]
        reason: String,
    },

    /// Authorise one further delivery of a task a restart left `blocked:
    /// interrupted`.
    Resume {
        #[arg(long)]
        task_id: Uuid,
    },

    /// List every task.
    List,

    /// Show one task, including its delegation chain.
    Show {
        #[arg(long)]
        task_id: Uuid,
    },
}

#[derive(Subcommand, Debug)]
pub enum ScheduleCommand {
    /// Register a durable cron rule against a task template. Never itself
    /// creates a run (design §11).
    ///
    /// Takes exactly one of `--template` or `--task`/`--name` (ADR 0021
    /// decision 11) — both together, or neither, are refused by the daemon,
    /// which owns this rule (ADR 0014).
    Create {
        /// The scope this schedule concerns, resolved the same way `task
        /// send`'s own scope is (`scope_ref::resolve`). Used only when
        /// `--task`/`--name` creates a new template — a schedule against an
        /// existing `--template` runs in whatever scope that template
        /// already names.
        scope: crate::scope_ref::ScopeRef,

        /// Use an existing task template by name. Refused when its state is
        /// not `open`. Exactly one of this or `--task`/`--name` is required.
        #[arg(long)]
        template: Option<String>,

        /// The new template's prompt. Requires `--name`; creates the
        /// template and the schedule together, in one transaction.
        #[arg(long)]
        task: Option<String>,

        /// The new template's name (`task_templates.name` is unique across
        /// the instance). Requires `--task`.
        #[arg(long)]
        name: Option<String>,

        /// Five-field cron expression (minute hour day-of-month month
        /// day-of-week). Checked by the daemon (`factory_task::schedule::
        /// validate`), never by this command.
        #[arg(long)]
        cron: String,

        /// An IANA timezone name, e.g. `Europe/Berlin` — not a fixed
        /// offset, which cannot express a rule that survives a
        /// daylight-saving change. Checked by the daemon.
        #[arg(long)]
        tz: String,

        /// The new template's target agent. Only meaningful with
        /// `--task`/`--name`.
        #[arg(long)]
        agent: Option<String>,

        /// The new template's acceptance criteria. Only meaningful with
        /// `--task`/`--name`.
        #[arg(long)]
        acceptance: Option<String>,
    },

    /// List every schedule: its template, its cron and timezone exactly as
    /// typed, whether it is enabled, its last run, and its next run.
    List,

    /// Turn a schedule on. Does not touch its history or any run it already
    /// produced.
    Enable { schedule_id: Uuid },

    /// Turn a schedule off. Does not touch its history or any run it
    /// already produced.
    Disable { schedule_id: Uuid },
}

#[derive(Subcommand, Debug)]
pub enum ContextCommand {
    /// Print the context an agent would be started with.
    Show {
        #[arg(long)]
        scope: crate::scope_ref::ScopeRef,
        #[arg(long)]
        agent_name: String,
        #[arg(long)]
        task_prompt: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        let mut full = vec!["factory"];
        full.extend_from_slice(args);
        Cli::try_parse_from(full)
    }

    #[test]
    fn out_of_scope_commands_are_absent_not_stubbed() {
        for name in ["secret", "knowledge", "memory"] {
            let err = parse(&[name]).unwrap_err();
            assert_eq!(
                err.kind(),
                clap::error::ErrorKind::InvalidSubcommand,
                "`{name}` must not parse as a command at all"
            );
        }
    }

    #[test]
    fn agent_list_is_absent() {
        let err = parse(&["agent", "list"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn task_send_requires_scope_and_prompt() {
        assert!(parse(&["task", "send"]).is_err());
        // A non-UUID `--scope` is a scope *name* now, not a parse error:
        // design §7 addresses scopes by name throughout (`factory task send
        // irrlicht "…"`). Whether the name exists is the registry's question,
        // answered at resolution time against `scope.list`, not the parser's.
        assert!(parse(&["task", "send", "--scope", "irrlicht", "--prompt", "hi"]).is_ok());
        assert!(
            parse(&[
                "task",
                "send",
                "--scope",
                "00000000-0000-0000-0000-000000000000",
                "--prompt",
                "hi"
            ])
            .is_ok()
        );
    }

    #[test]
    fn timeout_without_wait_is_rejected() {
        let err = parse(&[
            "task",
            "send",
            "--scope",
            "00000000-0000-0000-0000-000000000000",
            "--prompt",
            "hi",
            "--timeout",
            "5",
        ])
        .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn root_flag_is_recognised_before_the_subcommand() {
        let cli = parse(&["--root", "/tmp/x", "status"]).unwrap();
        assert_eq!(cli.root, Some(PathBuf::from("/tmp/x")));
    }
}
