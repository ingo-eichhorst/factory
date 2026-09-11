//! The `factory` command surface, wired to decision 9's operation
//! vocabulary. Every subcommand here is one of `init`, `start|stop|status|
//! doctor`, `scope`, `agent` (including `agent list`), `task`, `schedule`,
//! `knowledge`, `memory`, `context` — station 10's declared scope, station
//! 11's `schedule` group (ADR 0021 decision 11), and station 12's agent
//! discovery and durable writing (ADR 0022). `secret` (station 13) is still
//! absent, not stubbed: there is no variant for it anywhere below.

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

    /// The shared, sourced knowledge note graph (design §7, backlog §12;
    /// ADR 0022). One Markdown file per note under
    /// `.factory/knowledge/`; the filename is the stable link target — see
    /// `KnowledgeCommand::Write`'s own doc comment for why there is no
    /// rename.
    Knowledge {
        #[command(subcommand)]
        action: KnowledgeCommand,
    },

    /// Scope-local memory (design §7, backlog §12; ADR 0022). Unlike
    /// `knowledge`, an entry is scope-private and unsourced — a scratch line
    /// a scope leaves for its future self under
    /// `.factory/memory/<scope-name>/`.
    Memory {
        #[command(subcommand)]
        action: MemoryCommand,
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

    /// List every registered scope and its agents: which of them the caller
    /// may target (design §6, via the same rule `task send` enforces), and
    /// each agent's live availability (ADR 0022).
    ///
    /// Takes exactly one of `--session` or `--scope`, enforced by the daemon
    /// (this parser accepts either, both, or neither — the same stance
    /// `schedule create`'s own two forms take toward their own flags).
    /// `--session` names the caller by the session actually asking, resolved
    /// the way `task send` resolves it, so the answer here cannot drift from
    /// what sending a task will actually do. `--scope` answers the same
    /// question hypothetically, for a human with no live session.
    List {
        #[arg(long)]
        session: Option<Uuid>,
        #[arg(long)]
        scope: Option<crate::scope_ref::ScopeRef>,
    },
}

#[derive(Subcommand, Debug)]
pub enum KnowledgeCommand {
    /// Write (or, with `--update`, overwrite) one note. The body comes from
    /// standard input — there is deliberately no `--body` flag (ADR 0022
    /// decision 4): prose in argv is mangled by every shell's own quoting
    /// rules differently, and a second path into the same field would grow
    /// its own escaping bug.
    ///
    /// `--name` is a stable identifier, never derived from `--title`: a
    /// title is edited freely, a name is a link target, and there is no
    /// `knowledge rename` (ADR 0022 decision 6) — a note that needs a
    /// different name is written fresh, leaving the old one in place for
    /// whatever still points at it.
    Write {
        #[arg(long)]
        name: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        status: String,
        /// Repeatable. At least one is required.
        #[arg(long = "source")]
        source: Vec<String>,
        /// Overwrite a note that already exists under this name. Without
        /// it, writing an existing name is refused (ADR 0022 decision 5).
        #[arg(long)]
        update: bool,
        /// Record this write's provenance against a task.
        #[arg(long)]
        task_id: Option<Uuid>,
    },

    /// List every note: its title, its links, its backlinks, every
    /// unresolved `[[link]]`, and every unreadable file — all four, since a
    /// dangling link is a gap and an unreadable file is one file's problem,
    /// neither one an error that hides the rest.
    List,

    /// Print one note exactly as it is on disk. Resolves no `[[link]]` and
    /// renders nothing (ADR 0022's own consequences).
    Show {
        #[arg(long)]
        name: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum MemoryCommand {
    /// Add one entry to a scope's memory. The entry comes from standard
    /// input, for the same reason `knowledge write`'s body does.
    Add {
        #[arg(long)]
        scope: crate::scope_ref::ScopeRef,
    },

    /// List a scope's memory entries, oldest first.
    List {
        #[arg(long)]
        scope: crate::scope_ref::ScopeRef,
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

    /// Choose a session for a task that is still `queued` and record the
    /// choice — a manual re-attempt at assignment, distinct from `task send`,
    /// which creates a new task. Never delivers.
    Assign {
        #[arg(long)]
        task_id: Uuid,
        #[arg(long)]
        agent_name: String,
    },

    /// Record a progress note against a task — an annotation, not a status
    /// change.
    Progress {
        #[arg(long)]
        task_id: Uuid,
        #[arg(long)]
        note: String,
    },

    /// Record a structured decision against a task: what was decided, and
    /// why. `--rationale` is required — design §11 asks for "nachvollziehbare
    /// Entscheidungen" (traceable decisions), and a decision without a
    /// rationale is a log line, not a decision.
    Decision {
        #[arg(long)]
        task_id: Uuid,
        #[arg(long)]
        decision: String,
        #[arg(long)]
        rationale: String,
        #[arg(long)]
        alternatives: Option<String>,
        #[arg(long)]
        consequences: Option<String>,
    },

    /// Record a verification verdict against a task — design §12.1's hook.
    /// Annotates only: it never changes the task's status, never creates a
    /// rework, and never commissions an inspection (ADR 0021 decisions 4 and
    /// 5). This is a human operation in version 1, so there is no `--session`
    /// flag; the daemon's own independence guard still refuses a verdict
    /// authored from inside the scope under inspection.
    Verify {
        #[arg(long)]
        task_id: Uuid,
        #[arg(long)]
        verdict: String,
        #[arg(long)]
        note: Option<String>,
    },

    /// Create a new run that reworks an already-finished one (design §12.2's
    /// hook). The referenced run must be terminal, and is never itself
    /// changed — this only ever writes the new run's own row and its own
    /// `rework` event.
    ///
    /// `--prompt` is required: a rework's prompt always comes from the
    /// caller, never from the reworked run's template — see
    /// `factory_task::create::create_rework`'s own doc comment. `--scope`
    /// names the new run's target scope; it is never inherited from the run
    /// being reworked, so a rework can land in a different scope on purpose.
    /// Creates only — it never assigns or delivers, so a caller who wants
    /// the new run worked follows up with `factory task assign`.
    Rework {
        #[arg(long)]
        scope: crate::scope_ref::ScopeRef,
        #[arg(long)]
        prompt: String,
        /// The finished run this one reworks.
        #[arg(long)]
        reworks_task_id: Uuid,
        /// Why the referenced run is being reworked. Required — an empty
        /// finding is refused (`factory_task::TaskError::ReworkFindingRequired`).
        #[arg(long)]
        rework_finding: String,
        #[arg(long)]
        target_session: Option<Uuid>,
        #[arg(long)]
        workspace: Option<PathBuf>,
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
        let err = parse(&["secret"]).unwrap_err();
        assert_eq!(
            err.kind(),
            clap::error::ErrorKind::InvalidSubcommand,
            "`secret` must not parse as a command at all"
        );
    }

    /// Station 12 (ADR 0022): `agent list`, `knowledge`, and `memory` now
    /// parse. The daemon owns "exactly one of `--session`/`--scope`"
    /// (`ops::agent::resolve_caller`) — this parser accepts either, both, or
    /// neither, the same stance `schedule create`'s own two flag forms take.
    #[test]
    fn agent_list_parses_with_session_or_scope_or_neither() {
        assert!(parse(&["agent", "list"]).is_ok());
        assert!(
            parse(&[
                "agent",
                "list",
                "--session",
                "00000000-0000-4000-8000-000000000001"
            ])
            .is_ok()
        );
        assert!(parse(&["agent", "list", "--scope", "irrlicht"]).is_ok());
    }

    #[test]
    fn knowledge_write_reads_flags_and_takes_no_body_flag() {
        let cli = parse(&[
            "knowledge",
            "write",
            "--name",
            "my-note",
            "--title",
            "My Note",
            "--status",
            "draft",
            "--source",
            "a.md",
            "--source",
            "b.md",
        ])
        .expect("knowledge write parses");
        match cli.command {
            Command::Knowledge {
                action: KnowledgeCommand::Write { name, source, .. },
            } => {
                assert_eq!(name, "my-note");
                assert_eq!(source, vec!["a.md".to_string(), "b.md".to_string()]);
            }
            other => panic!("expected KnowledgeCommand::Write, got {other:?}"),
        }

        // There is deliberately no `--body` flag (ADR 0022 decision 4).
        let err = parse(&[
            "knowledge",
            "write",
            "--name",
            "n",
            "--title",
            "t",
            "--status",
            "s",
            "--source",
            "a.md",
            "--body",
            "nope",
        ])
        .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn knowledge_list_and_show_parse() {
        assert!(parse(&["knowledge", "list"]).is_ok());
        assert!(parse(&["knowledge", "show", "--name", "my-note"]).is_ok());
    }

    #[test]
    fn memory_add_and_list_require_a_scope() {
        assert!(parse(&["memory", "add"]).is_err());
        assert!(parse(&["memory", "add", "--scope", "irrlicht"]).is_ok());
        assert!(parse(&["memory", "list", "--scope", "irrlicht"]).is_ok());
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
