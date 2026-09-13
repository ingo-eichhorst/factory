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
        let mut args = self.args.clone();
        let mut env = ctx.env();

        // Every harness gets the guide to Factory itself, through whichever
        // mechanism its own CLI offers for extending the system prompt --
        // never the task prompt, so a standing agent (which never sees one)
        // gets it too. A path is preferred over inline text wherever the
        // harness accepts one: `herdr agent start … -- <args>` types this
        // into a pane's shell, and a bare path has no newline in it to worry
        // about surviving that intact.
        match self.harness.as_str() {
            "claude" => {
                let path = ctx.write_guide_file()?;
                args.push("--append-system-prompt-file".into());
                args.push(path.display().to_string());
            }
            "pi" => {
                // pi's own flag reads a path's contents when given one.
                let path = ctx.write_guide_file()?;
                args.push("--append-system-prompt".into());
                args.push(path.display().to_string());
            }
            // codex's `developer_instructions` config key takes the text
            // itself, not a path, and its own CLI otherwise offers nothing
            // else that would carry a system prompt in -- verified live: a
            // one-line value works, but herdr's own `agent start` refuses a
            // multi-paragraph argument outright ("agent arguments cannot be
            // encoded safely for the target shell") rather than mistype it.
            // So codex takes the documented fallback instead: `prompt()`
            // puts the guide above the task for a task run, and a standing
            // codex agent -- which never gets a `prompt()` call -- gets
            // nothing, exactly as the table in the README says.
            "codex" => {}
            "opencode" => {
                // Merged as a local config layer alongside a person's own
                // `opencode.json` -- its `instructions` array gains an entry
                // rather than losing whatever was already there.
                let path = ctx.write_guide_file()?;
                let content = serde_json::json!({ "instructions": [path.display().to_string()] });
                env.insert("OPENCODE_CONFIG_CONTENT".to_string(), content.to_string());
            }
            // A harness this build does not recognise gets no guide rather
            // than a guess at a flag that might not exist.
            _ => {}
        }

        Ok(LaunchSpec {
            kind: LaunchKind::Named(self.harness.clone()),
            args,
            env,
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
        let mut prompt = String::new();
        // codex has no system-prompt mechanism this build could get working
        // (see `launch_spec`), so it takes the fallback: the guide goes above
        // the task here instead, and never in both places for the same run.
        if self.harness == "codex" {
            prompt.push_str(&ctx.factory_guide());
            prompt.push_str("\n\n---\n\n");
        }
        prompt.push_str(&format!(
            "You have been given a task by Factory.\n\
             \n\
             Task: {title}\n\
             Task id: {id}\n\
             Working directory: {cwd}\n",
            title = task.title,
            id = task.id,
            cwd = ctx.cwd.display(),
        ));
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
            estimate_seconds: None,
            result: None,
            error: None,
            runs: 1,
            ack_timeout_seconds: None,
            timeout_seconds: None,
            blocked_timeout_seconds: None,
            labels: Default::default(),
            created_at: now,
            updated_at: now,
            last_run_at: None,
            next_run_at: None,
            worktree: true,
            workflow_origin: None,
        }
    }

    /// A fresh, real directory every call, so tests that write a guide file
    /// never race each other or a previous run's leftovers.
    fn fresh_guides_dir() -> PathBuf {
        std::env::temp_dir().join(format!("factory-builtin-agents-test-{}", uuid::Uuid::new_v4()))
    }

    fn ctx(worktree_branch: Option<String>) -> AgentContext {
        AgentContext {
            scope: "demo".into(),
            agent_name: "claude-code".into(),
            cwd: PathBuf::from("/tmp/somewhere"),
            factory_bin: PathBuf::from("/usr/local/bin/factory"),
            socket: PathBuf::from("/tmp/factory.sock"),
            guides_dir: fresh_guides_dir(),
            task: Some(TaskBinding {
                task: sample_task(),
                run_id: "r1".into(),
                attempt: 1,
                token: "tok".into(),
                worktree_branch,
            }),
            identity_token: None,
            role: None,
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

    // -- the guide, injected per harness in `launch_spec` -------------------

    #[tokio::test]
    async fn claude_gets_the_guide_as_a_system_prompt_file() {
        let context = ctx(None);
        let launch = HarnessAgent::claude_code().launch_spec(&context).await.unwrap();
        let flag = launch
            .args
            .iter()
            .position(|a| a == "--append-system-prompt-file")
            .expect("claude gets the file flag");
        let path = &launch.args[flag + 1];
        assert_eq!(std::fs::read_to_string(path).unwrap(), context.factory_guide());
        std::fs::remove_dir_all(&context.guides_dir).ok();
    }

    #[tokio::test]
    async fn pi_gets_the_guide_as_a_system_prompt_file() {
        let context = ctx(None);
        let launch = HarnessAgent::pi().launch_spec(&context).await.unwrap();
        let flag = launch
            .args
            .iter()
            .position(|a| a == "--append-system-prompt")
            .expect("pi gets the file flag");
        let path = &launch.args[flag + 1];
        assert_eq!(std::fs::read_to_string(path).unwrap(), context.factory_guide());
        std::fs::remove_dir_all(&context.guides_dir).ok();
    }

    #[tokio::test]
    async fn codex_gets_no_guide_in_launch_spec_and_no_file_either() {
        // Verified live that herdr refuses a multi-paragraph argument
        // outright ("agent arguments cannot be encoded safely for the target
        // shell"), and codex's own `developer_instructions` config key takes
        // text rather than a path -- so codex takes the documented fallback
        // in `prompt()` instead of anything here.
        let context = ctx(None);
        let launch = HarnessAgent::codex().launch_spec(&context).await.unwrap();
        assert!(launch.args.is_empty());
        assert!(!context.guides_dir.exists(), "nothing was written to a file");
    }

    #[tokio::test]
    async fn a_codex_task_run_gets_the_guide_above_the_task_in_its_prompt() {
        let context = ctx(None);
        let prompt = HarnessAgent::codex().prompt(&context).await.unwrap();
        let guide = context.factory_guide();
        assert!(prompt.starts_with(&guide), "the guide leads, the task follows: {prompt}");
        assert!(prompt.contains("You have been given a task by Factory."));
    }

    #[tokio::test]
    async fn a_codex_standing_agent_gets_no_guide_at_all() {
        // `prompt()` is never called for a standing agent, and `launch_spec`
        // gives codex nothing to carry one in -- the fallback the README
        // documents for a harness whose own mechanism cannot be made to work.
        let mut standing = ctx(None);
        standing.task = None;
        let launch = HarnessAgent::codex().launch_spec(&standing).await.unwrap();
        assert!(launch.args.is_empty());
        assert!(launch.env.keys().all(|k| k.starts_with("FACTORY_")));
    }

    #[tokio::test]
    async fn opencode_gets_the_guide_through_a_config_content_env_var() {
        let context = ctx(None);
        let launch = HarnessAgent::opencode().launch_spec(&context).await.unwrap();
        let content = launch
            .env
            .get("OPENCODE_CONFIG_CONTENT")
            .expect("opencode gets the env var");
        let value: serde_json::Value = serde_json::from_str(content).unwrap();
        let path = value["instructions"][0]
            .as_str()
            .expect("instructions names the guide's path");
        assert_eq!(std::fs::read_to_string(path).unwrap(), context.factory_guide());
        std::fs::remove_dir_all(&context.guides_dir).ok();
    }

    #[tokio::test]
    async fn an_unrecognised_harness_carries_no_guide() {
        let agent = HarnessAgent::new("mystery", "not-a-real-harness", "made up for this test");
        let context = ctx(None);
        let launch = agent.launch_spec(&context).await.unwrap();
        assert!(launch.args.is_empty(), "nothing to inject for a harness this build cannot name a flag for");
        assert!(!launch.env.contains_key("OPENCODE_CONFIG_CONTENT"));
        assert!(!context.guides_dir.exists());
    }

    #[tokio::test]
    async fn declared_args_still_land_after_the_guide_claude_code_injects() {
        // `append_declared_args` itself lives in factory-daemon and is
        // exercised there; this pins the half `launch_spec` owns -- that its
        // own defaults come first and the injected flag follows them, so
        // whatever the caller appends after still lands last of all.
        let context = ctx(None);
        let launch = HarnessAgent::claude_code()
            .with_args(vec!["--model".into(), "opus".into()])
            .launch_spec(&context)
            .await
            .unwrap();
        assert_eq!(launch.args[0], "--model");
        assert_eq!(launch.args[1], "opus");
        assert_eq!(launch.args[2], "--append-system-prompt-file");
        std::fs::remove_dir_all(&context.guides_dir).ok();
    }

    #[tokio::test]
    async fn the_shell_agents_launch_and_prompt_stay_unchanged() {
        // The shell agent is not a model and gets no guide at all: its launch
        // and its prompt must be exactly what they were before this feature
        // existed, byte for byte.
        let context = ctx(None);
        let launch = ShellAgent.launch_spec(&context).await.unwrap();
        assert_eq!(launch.kind, LaunchKind::Command(Vec::new()));
        assert!(launch.args.is_empty());
        assert_eq!(launch.env, context.env());

        let prompt = ShellAgent.prompt(&context).await.unwrap();
        let bin = context.factory_bin.display();
        assert_eq!(
            prompt,
            format!(
                "{bin} task report t1 --status running --message 'shell agent started' >/dev/null; \
                 if ( make it stop flaking ); then {bin} task report t1 --status done --result 'command exited 0'; \
                 else {bin} task report t1 --status failed --error \"command exited $?\"; fi"
            )
        );
    }
}
