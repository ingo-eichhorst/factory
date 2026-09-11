//! The agents that ship in the box. Each one is the same shape -- a harness the
//! runtime knows by name, plus the words that carry a task into it.

use async_trait::async_trait;
use factory_core::adapter::agent::{Agent, AgentContext, LaunchKind, LaunchSpec};
use factory_core::error::Result;

/// An interactive coding agent the runtime can start by name.
pub struct HarnessAgent {
    name: String,
    /// The name the runtime knows this harness by, which need not be ours:
    /// Factory calls it `claude-code`, herdr calls it `claude`.
    harness: String,
    description: String,
    args: Vec<String>,
}

impl HarnessAgent {
    pub fn new(
        name: impl Into<String>,
        harness: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            harness: harness.into(),
            description: description.into(),
            args: Vec::new(),
        }
    }

    pub fn with_args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }

    pub fn pi() -> Self {
        Self::new("pi", "pi", "the pi harness")
    }

    pub fn claude_code() -> Self {
        Self::new("claude-code", "claude", "Claude Code")
    }

    pub fn codex() -> Self {
        Self::new("codex", "codex", "OpenAI Codex CLI")
    }

    pub fn opencode() -> Self {
        Self::new("opencode", "opencode", "opencode")
    }
}

#[async_trait]
impl Agent for HarnessAgent {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    async fn launch_spec(&self, ctx: &AgentContext) -> Result<LaunchSpec> {
        Ok(LaunchSpec {
            kind: LaunchKind::Named(self.harness.clone()),
            args: self.args.clone(),
            env: ctx.env(),
        })
    }

    async fn prompt(&self, ctx: &AgentContext) -> Result<String> {
        let binding = ctx.binding()?;
        let task = &binding.task;
        let instructions = if task.instructions.trim().is_empty() {
            "(no further detail was given -- work from the title)"
        } else {
            task.instructions.trim()
        };
        Ok(format!(
            "You have been given a task by Factory.\n\
             \n\
             Task: {title}\n\
             Task id: {id}\n\
             Working directory: {cwd}\n\
             \n\
             {instructions}\n\
             \n\
             ---\n\
             {contract}",
            title = task.title,
            id = task.id,
            cwd = ctx.cwd.display(),
            instructions = instructions,
            contract = ctx.reporting_contract(),
        ))
    }
}

/// Not an AI agent at all: runs the task's instructions as a shell command and
/// reports the outcome itself. It exists so the dispatch path -- scope, launch,
/// prompt, callback, event -- can be exercised end to end without spending a
/// model call, and as the smallest possible example of the seam.
pub struct ShellAgent;

#[async_trait]
impl Agent for ShellAgent {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> String {
        "runs the instructions as a shell command and reports the exit status".into()
    }

    async fn launch_spec(&self, ctx: &AgentContext) -> Result<LaunchSpec> {
        // Nothing to start: the session's own shell is the agent.
        Ok(LaunchSpec {
            kind: LaunchKind::Command(Vec::new()),
            args: Vec::new(),
            env: ctx.env(),
        })
    }

    async fn prompt(&self, ctx: &AgentContext) -> Result<String> {
        let binding = ctx.binding()?;
        let bin = ctx.factory_bin.display();
        let id = &binding.task.id;
        let command = binding.task.instructions.trim();
        let command = if command.is_empty() { "true" } else { command };
        // One line, because it is typed into a shell prompt. The report is part
        // of the same line so a task cannot be left open by a command that
        // succeeds and then forgets to say so. The subshell is what keeps an
        // instruction ending in `exit` from taking the pane's shell with it.
        Ok(format!(
            "{bin} task report {id} --status running --message 'shell agent started' >/dev/null; \
             if ( {command} ); then {bin} task report {id} --status done --result 'command exited 0'; \
             else {bin} task report {id} --status failed --error \"command exited $?\"; fi"
        ))
    }
}
