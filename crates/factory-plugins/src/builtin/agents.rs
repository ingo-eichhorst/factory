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
        let mut prompt = format!(
            "You have been given a task by Factory.\n\
             \n\
             Task: {title}\n\
             Task id: {id}\n\
             Working directory: {cwd}\n",
            title = task.title,
            id = task.id,
            cwd = ctx.cwd.display(),
        );
        // A fresh worktree holds only what git tracks -- an agent that reaches
        // for something ignored or never committed needs to hear that here,
        // not discover it as an unexplained failure partway through.
        if let Some(branch) = &binding.worktree_branch {
            prompt.push_str(&format!(
                "This directory is a git worktree of the scope, on branch {branch}. \
                 Commit your work here as you go -- a person decides what happens to \
                 the branch afterward, so nothing here merges, rebases, or pushes it \
                 for you. Anything the scope's git ignores or never tracked -- .env, \
                 node_modules, target, and the like -- is not present.\n"
            ));
        }
        prompt.push_str(&format!(
            "\n{instructions}\n\
             \n\
             ---\n\
             {contract}",
            instructions = instructions,
            contract = ctx.reporting_contract(),
        ));
        Ok(prompt)
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

#[cfg(test)]
mod tests {
    use super::*;
    use factory_core::adapter::agent::TaskBinding;
    use factory_core::task::{Task, TaskStatus};
    use std::path::PathBuf;

    fn sample_task() -> Task {
        let now = chrono::Utc::now();
        Task {
            id: "t1".into(),
            title: "fix the flaky test".into(),
            instructions: "make it stop flaking".into(),
            scope: "demo".into(),
            agent: "claude-code".into(),
            runtime: "herdr".into(),
            status: TaskStatus::Dispatching,
            schedule: None,
            result: None,
            error: None,
            runs: 1,
            ack_timeout_seconds: None,
            timeout_seconds: None,
            labels: Default::default(),
            created_at: now,
            updated_at: now,
            last_run_at: None,
            next_run_at: None,
            worktree: true,
        }
    }

    fn ctx(worktree_branch: Option<String>) -> AgentContext {
        AgentContext {
            scope: "demo".into(),
            cwd: PathBuf::from("/tmp/somewhere"),
            factory_bin: PathBuf::from("/usr/local/bin/factory"),
            socket: PathBuf::from("/tmp/factory.sock"),
            task: Some(TaskBinding {
                task: sample_task(),
                run_id: "r1".into(),
                attempt: 1,
                token: "tok".into(),
                worktree_branch,
            }),
            identity_token: None,
        }
    }

    #[tokio::test]
    async fn a_run_in_its_own_worktree_is_told_the_branch_and_told_to_commit() {
        let agent = HarnessAgent::claude_code();
        let prompt = agent
            .prompt(&ctx(Some("factory/t1-fix-the-flaky-test".into())))
            .await
            .unwrap();
        assert!(
            prompt.contains("factory/t1-fix-the-flaky-test"),
            "the branch name should be right there in the prompt: {prompt}"
        );
        assert!(prompt.contains("git worktree"), "and said plainly what this directory is");
        assert!(prompt.contains("Commit your work"), "and that committing is expected");
    }

    #[tokio::test]
    async fn a_run_in_the_scope_itself_hears_nothing_about_worktrees() {
        let agent = HarnessAgent::claude_code();
        let prompt = agent.prompt(&ctx(None)).await.unwrap();
        assert!(!prompt.contains("worktree"), "nothing to say when there is not one: {prompt}");
        assert!(!prompt.contains("Commit your work"));
    }

    #[tokio::test]
    async fn the_ordinary_prompt_is_unchanged_by_the_worktree_branch_alone() {
        // Pins the refactor from one `format!` into a `format!` plus
        // conditional `push_str`: without a worktree, the words a person saw
        // before this feature existed must be exactly the words they see now.
        let agent = HarnessAgent::claude_code();
        let prompt = agent.prompt(&ctx(None)).await.unwrap();
        assert_eq!(
            prompt,
            format!(
                "You have been given a task by Factory.\n\
                 \n\
                 Task: fix the flaky test\n\
                 Task id: t1\n\
                 Working directory: /tmp/somewhere\n\
                 \n\
                 make it stop flaking\n\
                 \n\
                 ---\n\
                 {contract}",
                contract = ctx(None).reporting_contract(),
            )
        );
    }
}
