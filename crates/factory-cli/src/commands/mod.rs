//! Dispatch from a parsed [`crate::cli::Cli`] to the command that does the
//! work, and the one place a command's root is resolved.

mod agent;
mod context;
mod daemon;
mod doctor;
mod init;
mod schedule;
mod scope;
mod task;

use std::path::Path;

use crate::cli::{
    AgentCommand, Cli, Command, ContextCommand, DaemonCommand, ScheduleCommand, ScopeCommand,
    TaskCommand,
};
use crate::exit;

pub fn dispatch(cli: Cli) -> i32 {
    match cli.command {
        Command::Init { name } => match crate::root::for_init(cli.root.as_deref()) {
            Ok(root) => init::run(&root, name),
            Err(message) => usage_error(&message),
        },

        Command::Start { harness } => {
            with_root(cli.root.as_deref(), |root| daemon::start(root, &harness))
        }
        Command::Stop => with_root(cli.root.as_deref(), daemon::stop),
        Command::Status => with_root(cli.root.as_deref(), daemon::status),
        Command::Doctor => with_root(cli.root.as_deref(), doctor::run),

        Command::Daemon { action } => match action {
            DaemonCommand::Run { harness } => with_root(cli.root.as_deref(), |root| {
                daemon::daemon_run(root, &harness)
            }),
        },

        Command::Scope { action } => match action {
            ScopeCommand::Add { path, name } => {
                with_root(cli.root.as_deref(), |root| scope::add(root, &path, &name))
            }
            ScopeCommand::Reconcile { apply } => {
                with_root(cli.root.as_deref(), |root| scope::reconcile(root, apply))
            }
            ScopeCommand::List => with_root(cli.root.as_deref(), scope::list),
        },

        Command::Agent { action } => match action {
            AgentCommand::Start {
                scope,
                agent_name,
                workspace,
            } => with_root(cli.root.as_deref(), |root| {
                let scope_id = match resolve_scope(root, &scope) {
                    Ok(id) => id,
                    Err(code) => return code,
                };
                agent::start(root, scope_id, &agent_name, workspace.as_deref())
            }),
            AgentCommand::Stop { session, reason } => with_root(cli.root.as_deref(), |root| {
                agent::stop(root, session, reason.as_deref())
            }),
            AgentCommand::Status { scope, session } => with_root(cli.root.as_deref(), |root| {
                let scope_id = match resolve_scope(root, &scope) {
                    Ok(id) => id,
                    Err(code) => return code,
                };
                agent::status(root, scope_id, session)
            }),
            AgentCommand::Attach { session } => {
                with_root(cli.root.as_deref(), |root| agent::attach(root, session))
            }
        },

        Command::Task { action } => match action {
            TaskCommand::Send {
                scope,
                prompt,
                agent_name,
                target_session,
                workspace,
                sender_session,
                wait,
                timeout,
            } => with_root(cli.root.as_deref(), |root| {
                let scope_id = match resolve_scope(root, &scope) {
                    Ok(id) => id,
                    Err(code) => return code,
                };
                task::send(
                    root,
                    scope_id,
                    &prompt,
                    agent_name.as_deref(),
                    target_session,
                    workspace.as_deref(),
                    sender_session,
                    wait,
                    timeout,
                )
            }),
            TaskCommand::Cancel { task_id } => {
                with_root(cli.root.as_deref(), |root| task::cancel(root, task_id))
            }
            TaskCommand::Done {
                task_id,
                summary,
                artifacts,
            } => with_root(cli.root.as_deref(), |root| {
                task::done(root, task_id, summary.as_deref(), &artifacts)
            }),
            TaskCommand::Fail {
                task_id,
                summary,
                artifacts,
            } => with_root(cli.root.as_deref(), |root| {
                task::fail(root, task_id, summary.as_deref(), &artifacts)
            }),
            TaskCommand::Block { task_id, reason } => with_root(cli.root.as_deref(), |root| {
                task::block(root, task_id, &reason)
            }),
            TaskCommand::Resume { task_id } => {
                with_root(cli.root.as_deref(), |root| task::resume(root, task_id))
            }
            TaskCommand::List => with_root(cli.root.as_deref(), task::list),
            TaskCommand::Show { task_id } => {
                with_root(cli.root.as_deref(), |root| task::show(root, task_id))
            }
        },

        Command::Schedule { action } => match action {
            ScheduleCommand::Create {
                scope,
                template,
                task,
                name,
                cron,
                tz,
                agent,
                acceptance,
            } => with_root(cli.root.as_deref(), |root| {
                let scope_id = match resolve_scope(root, &scope) {
                    Ok(id) => id,
                    Err(code) => return code,
                };
                schedule::create(
                    root,
                    scope_id,
                    template.as_deref(),
                    task.as_deref(),
                    name.as_deref(),
                    &cron,
                    &tz,
                    agent.as_deref(),
                    acceptance.as_deref(),
                )
            }),
            ScheduleCommand::List => with_root(cli.root.as_deref(), schedule::list),
            ScheduleCommand::Enable { schedule_id } => with_root(cli.root.as_deref(), |root| {
                schedule::enable(root, schedule_id)
            }),
            ScheduleCommand::Disable { schedule_id } => with_root(cli.root.as_deref(), |root| {
                schedule::disable(root, schedule_id)
            }),
        },

        Command::Context { action } => match action {
            ContextCommand::Show {
                scope,
                agent_name,
                task_prompt,
            } => with_root(cli.root.as_deref(), |root| {
                let scope_id = match resolve_scope(root, &scope) {
                    Ok(id) => id,
                    Err(code) => return code,
                };
                context::show(root, scope_id, &agent_name, task_prompt.as_deref())
            }),
        },
    }
}

/// Resolve the root (every command but `init`), or report why it could not
/// be found and stop — never a panic on a missing root.
fn with_root(root_flag: Option<&Path>, f: impl FnOnce(&Path) -> i32) -> i32 {
    match crate::root::discover(root_flag) {
        Ok(root) => f(&root),
        Err(message) => usage_error(&message),
    }
}

fn usage_error(message: &str) -> i32 {
    eprintln!("factory: {message}");
    exit::GENERIC_ERROR
}

/// Resolve a `--scope` argument to an id, or print why it could not be and
/// return the exit code. Done once here so every command below keeps taking a
/// plain `Uuid` and no command grows its own copy of the name rule.
fn resolve_scope(
    root: &std::path::Path,
    reference: &crate::scope_ref::ScopeRef,
) -> Result<uuid::Uuid, i32> {
    crate::scope_ref::resolve(root, reference).map_err(|message| {
        eprintln!("factory: {message}");
        crate::exit::GENERIC_ERROR
    })
}
