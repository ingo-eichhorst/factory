//! The agents that ship in the box. Each one is the same shape -- a harness the
//! runtime knows by name, plus the words that carry a task into it.

use async_trait::async_trait;
use factory_core::adapter::agent::{
    truncate_tail, Agent, AgentContext, LaunchKind, LaunchSpec, ResumeSpec, UpstreamOutput,
    UPSTREAM_RESULT_BYTE_CAP,
};
use factory_core::adapter::KnowledgeHints;
use factory_core::error::{FactoryError, Result};
use factory_core::harness::HealthProbe;

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
                env.insert("DISABLE_AUTOUPDATER".to_string(), "1".to_string());
                let path = ctx.write_guide_file()?;
                args.push("--append-system-prompt-file".into());
                args.push(path.display().to_string());
                // The turn-end hooks, so a turn that ends without a report
                // is known the moment it happens rather than at the run's
                // timeout -- see `claude_turn_end_settings`. A path, never
                // inline JSON, for the same reason as the guide above.
                if let Some(settings) = claude_turn_end_settings(ctx) {
                    if let Some(path) = ctx.write_hook_settings(&settings)? {
                        args.push("--settings".into());
                        args.push(path.display().to_string());
                    }
                }
            }
            "pi" => {
                env.insert("PI_SKIP_VERSION_CHECK".to_string(), "1".to_string());
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
            "codex" => {
                args.push("-c".into());
                args.push("check_for_update_on_startup=false".into());
            }
            "opencode" => {
                env.insert(
                    "OPENCODE_DISABLE_AUTOUPDATE".to_string(),
                    "true".to_string(),
                );
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

    /// `<harness> --version`: the one flag every harness this build ships
    /// answers without starting a session, a model call, or an update.
    fn health_probe(&self) -> Option<HealthProbe> {
        Some(HealthProbe::version(self.harness.clone()))
    }

    /// `#178`: only `claude` and `codex` ever get a `--continue` past
    /// `resolve_continue`'s other checks -- `pi`, `opencode` and `shell`
    /// declare none, which is what sends `--continue` straight to the
    /// fresh-session fallback for them, journaled with this exact reason.
    fn resume_spec(&self, session_id: &str) -> Option<ResumeSpec> {
        match self.harness.as_str() {
            "claude" => Some(ResumeSpec { args: vec!["--resume".into(), session_id.into()] }),
            // A subcommand, not a flag -- `Engine::dispatch` prepends this
            // ahead of everything else `launch_spec` and the scope's own
            // declared args add, which is what keeps it first.
            "codex" => Some(ResumeSpec { args: vec!["resume".into(), session_id.into()] }),
            _ => None,
        }
    }

    async fn prompt(&self, ctx: &AgentContext) -> Result<String> {
        let binding = ctx.binding()?;
        let task = &binding.task;
        // `#178`: a resumed run's prompt is the short continue note the
        // triage asked for, not the task replayed in full -- the harness's
        // own resumed conversation already has the original instructions,
        // any upstream output and knowledge hints in it from the run this
        // one picked up from.
        if let Some(session_id) = &binding.resumed_session {
            let mut prompt = String::new();
            if self.harness == "codex" {
                prompt.push_str(&ctx.factory_guide());
                prompt.push_str("\n\n---\n\n");
            }
            let contract = if self.harness == "codex" {
                ctx.reporting_contract_explicit()
            } else {
                ctx.reporting_contract()
            };
            if binding.round == 0 {
                prompt.push_str(&format!(
                    "Factory is continuing this task (\"{title}\", {id}): the previous run of it \
                     ended on an infrastructure failure, and this session ({session_id}) picks the \
                     same conversation back up in the same working directory. You are the same \
                     agent -- carry on from where you left off rather than starting over.\n",
                    title = task.title,
                    id = task.id,
                ));
            } else {
                // `#178`: a feedback round. The feedback is the prompt of the
                // resumed turn -- the task itself is already in this
                // conversation, from the round this one picks up from.
                prompt.push_str(&format!(
                    "Factory is running this task (\"{title}\", {id}) again as rework round {round}. \
                     This session ({session_id}) picks your own previous run of it back up, in the \
                     same working directory: you are the same agent, with the same history. Address \
                     what follows, then report on this run.\n",
                    title = task.title,
                    id = task.id,
                    round = binding.round,
                ));
                if !binding.upstream.is_empty() {
                    prompt.push('\n');
                    prompt.push_str(&upstream_section(&binding.upstream));
                }
            }
            if let Some(changes) = &binding.since_last_run {
                prompt.push_str(&format!("\nSince your previous run ended: {changes}\n"));
            }
            prompt.push_str(&format!("\n---\n{contract}"));
            return Ok(prompt);
        }
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
        // Direct parents only, never a transitive ancestor -- see the
        // README's Workflows section. This is task content (what upstream
        // steps produced), not an instruction about Factory itself, so it
        // lives here rather than in `factory_guide`/`reporting_contract`.
        if !binding.upstream.is_empty() {
            prompt.push('\n');
            prompt.push_str(&upstream_section(&binding.upstream));
        }
        // Task content too: which pages of the company's knowledge base
        // this task's own words matched, searched once at dispatch. Paths
        // and reasons, never page text -- the agent opens what it needs.
        if let Some(hints) = &binding.knowledge {
            prompt.push('\n');
            prompt.push_str(&knowledge_section(hints));
        }
        prompt.push_str(&format!(
            "\n{instructions}\n\
             \n\
             ---\n\
             {contract}",
            instructions = instructions,
            // `#147`: codex runs commands in a shared host that keeps some
            // earlier session's environment, so its contract carries the
            // socket and run token as flags rather than trusting FACTORY_*.
            contract = if self.harness == "codex" {
                ctx.reporting_contract_explicit()
            } else {
                ctx.reporting_contract()
            },
        ));
        Ok(prompt)
    }
}

/// Claude Code settings that make the harness itself say when a task run's
/// turn ends: its `Stop` hook for a turn that finished, `StopFailure` for one
/// an API error cut short. herdr only guesses a `claude` pane's state from the
/// screen, which `AGENTS.md` does not let the daemon act on; this is the
/// harness speaking, which it does. `None` for a standing agent, which has no
/// run to end.
///
/// Both hooks are `async`: Claude Code neither waits for them nor reads their
/// exit code or output, so a slow or absent daemon can never stall or alter a
/// turn. The command identifies the run by task id alone and takes the token
/// from `FACTORY_TASK_TOKEN`, which a hook inherits from the session -- the
/// secret stays out of the file. Claude Code merges these with the hooks in
/// the worktree's own `.claude/settings.json` rather than replacing them.
fn claude_turn_end_settings(ctx: &AgentContext) -> Option<String> {
    let binding = ctx.task.as_ref()?;
    let bin = shell_quote_always(&ctx.factory_bin.display().to_string());
    let id = shell_quote_always(&binding.task.id);
    let hook = |event: &str| {
        serde_json::json!([{
            "hooks": [{
                "type": "command",
                "command": format!("{bin} task turn-ended {id} --event {event}"),
                "async": true,
            }]
        }])
    };
    let settings = serde_json::json!({
        "hooks": {
            "Stop": hook("stop"),
            "StopFailure": hook("stop-failure"),
        }
    });
    Some(serde_json::to_string_pretty(&settings).unwrap_or_default())
}

/// The section a harness agent's prompt gets when this task followed others
/// in a workflow -- each direct parent named by its title and node/task id,
/// with the result it finished with (or a plain admission that it reported
/// none). Truncated again here, defensively: `Engine::dispatch` already caps
/// each result when it computes `upstream`, but this is what actually bounds
/// what reaches a prompt, so it does not simply trust that the value in hand
/// was already capped by whoever set it.
fn upstream_section(upstream: &[UpstreamOutput]) -> String {
    let mut out = String::from("Output from the workflow steps this task follows:\n\n");
    for parent in upstream {
        out.push_str(&format!(
            "- {title} (node {node_id}, task {task_id}):\n",
            title = parent.title,
            node_id = parent.node_id,
            task_id = parent.task_id,
        ));
        match parent.result.as_deref().map(str::trim) {
            Some(result) if !result.is_empty() => {
                out.push_str(&truncate_tail(result, UPSTREAM_RESULT_BYTE_CAP));
                out.push('\n');
            }
            _ => out.push_str("(no result reported)\n"),
        }
        out.push('\n');
    }
    out
}

/// The section a harness agent's prompt gets when its task asked for
/// knowledge hints and something matched.
fn knowledge_section(hints: &KnowledgeHints) -> String {
    let mut out = String::from(
        "Pages in the company knowledge base that matched this task. They are \
         not included here -- open the ones that look useful before you start:\n\n",
    );
    for hit in &hints.hits {
        out.push_str(&format!("- {} -- {} ({})\n", hints.path_of(hit), hit.title, hit.why));
    }
    out
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
        "runs the instructions as a shell command and reports the exit status and stdout".into()
    }

    async fn launch_spec(&self, ctx: &AgentContext) -> Result<LaunchSpec> {
        // Nothing to start: the session's own shell is the agent. The upstream
        // file, if this task has one, is written here so it exists as soon as
        // the session comes up -- `prompt` writes it too (see its own
        // comment), so neither call depends on running before the other.
        let mut env = ctx.env();
        if let Some(path) = ctx.write_upstream_file()? {
            env.insert("FACTORY_UPSTREAM_FILE".into(), path.display().to_string());
        }
        if let Some(path) = ctx.write_knowledge_file()? {
            env.insert("FACTORY_KNOWLEDGE_FILE".into(), path.display().to_string());
        }
        Ok(LaunchSpec {
            kind: LaunchKind::Command(Vec::new()),
            args: Vec::new(),
            env,
        })
    }

    async fn prompt(&self, ctx: &AgentContext) -> Result<String> {
        let binding = ctx.binding()?;
        let bin = ctx.factory_bin.display().to_string();
        let id = &binding.task.id;
        let command = binding.task.instructions.trim();
        let command = if command.is_empty() { "true" } else { command };

        // A downstream node reads its parents' outputs from a file rather
        // than having them spliced into the script below -- quotes, `$`,
        // backticks and newlines in a parent's stdout would otherwise have
        // to survive being embedded in the text that reports *this* task.
        // The path is daemon-generated -- guides_dir joined with a task id
        // -- so it never contains a quote or a newline in practice, but this
        // runs in the daemon process, where a stray one is skipped rather
        // than asserted into a panic (a plugin -- and this is one of the
        // built-in ones -- must never take the daemon down with it).
        let export_upstream = ctx
            .write_upstream_file()?
            .and_then(|path| shell_single_quote(&path.display().to_string()))
            .map(|quoted| format!("export FACTORY_UPSTREAM_FILE={quoted}\n"))
            .unwrap_or_default();
        // The knowledge hints, the same way and for the same reason: a JSON
        // file of `{vault, hits}` a command can read, never spliced in.
        let export_knowledge = ctx
            .write_knowledge_file()?
            .and_then(|path| shell_single_quote(&path.display().to_string()))
            .map(|quoted| format!("export FACTORY_KNOWLEDGE_FILE={quoted}\n"))
            .unwrap_or_default();

        // Everything the report needs -- the running/done/failed calls, the
        // stdout capture, and cleanup -- lives in a script file rather than
        // in the line typed into the pane. A pty's line discipline in
        // canonical mode caps how much it buffers before a newline (1024
        // bytes, MAX_CANON, on both Linux and macOS) and silently drops
        // anything past that with no error anywhere: a real workflow run
        // hung exactly this way against an earlier, inline draft of this
        // feature, cut off mid-line at byte 1023. Writing the wrapper to a
        // file instead means the typed line is just `. '<path>'`, whose
        // length never depends on the command's own, and the instructions
        // go into the file verbatim, so a multi-line instruction now works
        // too (the `( … )` subshell below spans lines exactly as well as it
        // spans one, which is what makes that safe). The instructions sit on
        // lines of their own inside it, never beside the closing `)`: a
        // trailing `# comment` would otherwise swallow the `)`, and a heredoc
        // terminator would never match, and either one leaves a script that
        // does not parse and a run that never reports.
        //
        // `echo $? > "$_factory_rc"` runs before the pipe to `tee` can
        // replace `$?` with `tee`'s own status, which is what makes the
        // captured code the command's rather than the pipe's. `mktemp`
        // takes an explicit template because the BSD `mktemp` on macOS
        // (unlike GNU's) refuses a bare invocation. No `PIPESTATUS`/
        // `pipestatus` and no process substitution: the script has to run
        // in both bash and zsh, the same as the old one-liner did.
        let bin_quoted = shell_quote_always(&bin);
        let script = format!(
            "# Generated by Factory's shell agent for run {run_id}; sourced\n\
             # into the pane's own shell (see ShellAgent::prompt), not run as\n\
             # a subprocess, so it behaves exactly as if typed there.\n\
             _factory_bin={bin_quoted}\n\
             _factory_id={id}\n\
             _factory_out=$(mktemp \"${{TMPDIR:-/tmp}}/factory-out-XXXXXX\")\n\
             _factory_rc=$(mktemp \"${{TMPDIR:-/tmp}}/factory-rc-XXXXXX\")\n\
             \"$_factory_bin\" task report \"$_factory_id\" --status running --message 'shell agent started' >/dev/null\n\
             {{ (\n\
             {export_upstream}{export_knowledge}{command}\n\
             ); echo $? > \"$_factory_rc\"; }} | tee \"$_factory_out\"\n\
             _factory_code=$(cat \"$_factory_rc\")\n\
             if [ \"$_factory_code\" = 0 ]; then\n\
             \"$_factory_bin\" task report \"$_factory_id\" --status done --result 'command exited 0' --result-file \"$_factory_out\"\n\
             else\n\
             \"$_factory_bin\" task report \"$_factory_id\" --status failed --result \"command exited $_factory_code\" --error \"command exited $_factory_code\" --result-file \"$_factory_out\"\n\
             fi\n\
             rm -f \"$_factory_out\" \"$_factory_rc\"\n",
            run_id = binding.run_id,
        );
        let path = ctx.write_shell_script(&script)?;

        // The path is daemon-generated (guides_dir, a fixed "shell-" prefix,
        // a run id, ".sh") and could only ever approach this by way of an
        // absurdly deep instance root; the check exists so that stays true
        // by construction rather than by assumption.
        const MAX_TYPED_LINE_BYTES: usize = 1000;
        let line = format!(". {}", shell_quote_always(&path.display().to_string()));
        if line.len() > MAX_TYPED_LINE_BYTES {
            return Err(FactoryError::BadRequest(format!(
                "the shell agent's script path is {} bytes, too long to type into a pane safely: {}",
                line.len(),
                path.display(),
            )));
        }
        Ok(line)
    }
}

/// Escape `s` for embedding inside single quotes in shell text, with no
/// fallback: used for the shell agent's own generated paths (its script
/// path in the typed line, its binary path inside the script), where there
/// is nothing sensible to skip to -- an unquoted or unescaped path there
/// breaks every report call outright, not just the one feature riding along.
fn shell_quote_always(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Escape `s` for embedding inside single quotes in the shell script above,
/// for the one case where skipping is the better fallback: the upstream
/// file's `export` line is optional, so a path that somehow held a newline
/// (which cannot be escaped inside a single quoted word) is left un-exported
/// rather than breaking the script.
fn shell_single_quote(s: &str) -> Option<String> {
    if s.contains('\n') {
        return None;
    }
    Some(format!("'{}'", s.replace('\'', r"'\''")))
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
            estimate: None,
            result: None,
            routed_to: None,
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
            knowledge_hints: false,
            workflow_origin: None,
            bench_origin: None,
            retry: None,
            pending_retry: None,
            schedule_paused: false,
            category: None,
            intake: None,
            failure: None,
            closure: None,
            slot_wait: None,
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
                resumed_session: None,
                round: 0,
                since_last_run: None,
                upstream: Vec::new(),
                knowledge: None,
                required_steps: Vec::new(),
                agent_exits: Vec::new(),
            }),
            identity_token: None,
            role: None,
            policy_frameworks: Vec::new(),
            goal: None,
            quality: Vec::new(),
        }
    }

    fn ctx_with_upstream(upstream: Vec<UpstreamOutput>) -> AgentContext {
        let mut context = ctx(None);
        context.task.as_mut().unwrap().upstream = upstream;
        context
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

    // -- upstream outputs, in the prompt for a harness agent -----------------

    #[tokio::test]
    async fn only_codex_gets_the_socket_and_run_token_written_into_its_contract() {
        // `#147`: codex's commands run where FACTORY_* may be another run's.
        let codex = HarnessAgent::codex().prompt(&ctx(None)).await.unwrap();
        assert!(codex.contains(&ctx(None).reporting_contract_explicit()), "{codex}");
        assert!(codex.contains("--run-token"), "{codex}");
        for agent in [HarnessAgent::claude_code(), HarnessAgent::opencode()] {
            let prompt = agent.prompt(&ctx(None)).await.unwrap();
            assert!(prompt.contains(&ctx(None).reporting_contract()), "{prompt}");
            assert!(!prompt.contains("--run-token"), "{prompt}");
        }
    }

    #[tokio::test]
    async fn a_root_nodes_prompt_is_unchanged_by_this_feature() {
        // Same guarantee as `the_ordinary_prompt_is_unchanged_by_the_worktree_branch_alone`,
        // for the other thing that can now be added to a task's prompt: with
        // nothing upstream, byte for byte, this must read exactly as it did
        // before `upstream` existed.
        let agent = HarnessAgent::claude_code();
        let prompt = agent.prompt(&ctx_with_upstream(Vec::new())).await.unwrap();
        assert!(!prompt.contains("Output from the workflow steps"));
        assert_eq!(prompt, agent.prompt(&ctx(None)).await.unwrap());
    }

    #[tokio::test]
    async fn a_downstream_nodes_prompt_names_every_direct_parent() {
        let agent = HarnessAgent::claude_code();
        let upstream = vec![
            UpstreamOutput {
                node_id: "a".into(),
                task_id: "ta".into(),
                title: "build it".into(),
                result: Some("build succeeded".into()),
            },
            UpstreamOutput {
                node_id: "b".into(),
                task_id: "tb".into(),
                title: "lint it".into(),
                result: None,
            },
        ];
        let prompt = agent.prompt(&ctx_with_upstream(upstream)).await.unwrap();
        assert!(prompt.contains("Output from the workflow steps this task follows"));
        assert!(prompt.contains("build it"));
        assert!(prompt.contains("node a"));
        assert!(prompt.contains("task ta"));
        assert!(prompt.contains("build succeeded"));
        assert!(prompt.contains("lint it"));
        assert!(prompt.contains("node b"));
        assert!(prompt.contains("(no result reported)"), "a parent with no result says so plainly: {prompt}");
    }

    #[tokio::test]
    async fn a_huge_parent_result_is_truncated_in_the_prompt() {
        let agent = HarnessAgent::claude_code();
        let huge = "x".repeat(UPSTREAM_RESULT_BYTE_CAP * 4);
        let upstream = vec![UpstreamOutput {
            node_id: "a".into(),
            task_id: "ta".into(),
            title: "noisy".into(),
            result: Some(huge),
        }];
        let prompt = agent.prompt(&ctx_with_upstream(upstream)).await.unwrap();
        assert!(
            prompt.len() < UPSTREAM_RESULT_BYTE_CAP * 2,
            "the huge result must not reach the prompt whole: {} bytes",
            prompt.len()
        );
        assert!(prompt.contains("truncated"), "the cut is marked: {prompt}");
    }

    // -- launch-time update checks are disabled per harness (#131) ----------

    #[tokio::test]
    async fn codex_disables_its_startup_update_check_for_task_and_standing_launches() {
        for standing in [false, true] {
            let mut context = ctx(None);
            if standing {
                context.task = None;
            }
            let launch = HarnessAgent::codex().launch_spec(&context).await.unwrap();
            assert_eq!(
                launch.args,
                ["-c", "check_for_update_on_startup=false"],
                "the exact Codex config override must be present"
            );
            assert_eq!(launch.env, context.env());
        }
    }

    #[tokio::test]
    async fn claude_disables_its_auto_updater_for_task_and_standing_launches() {
        for standing in [false, true] {
            let mut context = ctx(None);
            if standing {
                context.task = None;
            }
            let launch = HarnessAgent::claude_code()
                .launch_spec(&context)
                .await
                .unwrap();
            let mut expected_env = context.env();
            expected_env.insert("DISABLE_AUTOUPDATER".to_string(), "1".to_string());
            assert_eq!(launch.env, expected_env);
            std::fs::remove_dir_all(&context.guides_dir).ok();
        }
    }

    #[tokio::test]
    async fn pi_skips_its_version_check_for_task_and_standing_launches() {
        for standing in [false, true] {
            let mut context = ctx(None);
            if standing {
                context.task = None;
            }
            let launch = HarnessAgent::pi().launch_spec(&context).await.unwrap();
            let mut expected_env = context.env();
            expected_env.insert("PI_SKIP_VERSION_CHECK".to_string(), "1".to_string());
            assert_eq!(launch.env, expected_env);
            std::fs::remove_dir_all(&context.guides_dir).ok();
        }
    }

    #[tokio::test]
    async fn opencode_disables_auto_update_without_replacing_its_guide_config() {
        for standing in [false, true] {
            let mut context = ctx(None);
            if standing {
                context.task = None;
            }
            let launch = HarnessAgent::opencode()
                .launch_spec(&context)
                .await
                .unwrap();
            assert_eq!(
                launch
                    .env
                    .get("OPENCODE_DISABLE_AUTOUPDATE")
                    .map(String::as_str),
                Some("true")
            );
            assert!(
                launch.env.contains_key("OPENCODE_CONFIG_CONTENT"),
                "the independent guide-bearing config remains present"
            );
            assert_eq!(launch.env.len(), context.env().len() + 2);
            std::fs::remove_dir_all(&context.guides_dir).ok();
        }
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

    // -- the turn-end hooks, claude only (issue #69) ------------------------

    #[tokio::test]
    async fn a_claude_task_run_gets_async_stop_and_stop_failure_hooks_by_settings_file() {
        let context = ctx(None);
        let launch = HarnessAgent::claude_code().launch_spec(&context).await.unwrap();
        let flag = launch
            .args
            .iter()
            .position(|a| a == "--settings")
            .expect("claude gets a settings file");
        let path = &launch.args[flag + 1];
        assert_eq!(
            PathBuf::from(path),
            factory_core::adapter::agent::run_hook_settings_path(&context.guides_dir, "r1"),
            "keyed by run, so a retry never shares an earlier attempt's file"
        );
        let settings: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        for (event, flag) in [("Stop", "stop"), ("StopFailure", "stop-failure")] {
            let hook = &settings["hooks"][event][0]["hooks"][0];
            assert_eq!(hook["type"], "command", "{event}");
            assert_eq!(hook["async"], true, "{event} must never hold up a turn");
            let command = hook["command"].as_str().unwrap();
            assert_eq!(
                command,
                format!("'/usr/local/bin/factory' task turn-ended 't1' --event {flag}"),
            );
        }
        assert!(
            !std::fs::read_to_string(path).unwrap().contains("tok"),
            "the run token comes from the session's environment, never the file"
        );
        std::fs::remove_dir_all(&context.guides_dir).ok();
    }

    #[tokio::test]
    async fn a_standing_claude_agent_gets_no_turn_end_hooks() {
        let mut standing = ctx(None);
        standing.task = None;
        let launch = HarnessAgent::claude_code().launch_spec(&standing).await.unwrap();
        assert!(!launch.args.iter().any(|a| a == "--settings"), "no run, so no turn to end");
        std::fs::remove_dir_all(&standing.guides_dir).ok();
    }

    #[tokio::test]
    async fn only_claude_gets_the_turn_end_hooks() {
        for agent in [HarnessAgent::pi(), HarnessAgent::codex(), HarnessAgent::opencode()] {
            let context = ctx(None);
            let launch = agent.launch_spec(&context).await.unwrap();
            assert!(!launch.args.iter().any(|a| a == "--settings"), "{}", agent.name());
            std::fs::remove_dir_all(&context.guides_dir).ok();
        }
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
        assert_eq!(launch.args, ["-c", "check_for_update_on_startup=false"]);
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
        assert_eq!(launch.args, ["-c", "check_for_update_on_startup=false"]);
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
    async fn the_shell_agents_launch_is_unchanged_without_upstream_outputs() {
        // The shell agent is not a model and gets no guide at all; with
        // nothing upstream, its launch carries nothing new either.
        let context = ctx(None);
        let launch = ShellAgent.launch_spec(&context).await.unwrap();
        assert_eq!(launch.kind, LaunchKind::Command(Vec::new()));
        assert!(launch.args.is_empty());
        assert_eq!(launch.env, context.env());
    }

    #[tokio::test]
    async fn the_shell_agents_prompt_is_just_the_script_path() {
        let context = ctx(None);
        let prompt = ShellAgent.prompt(&context).await.unwrap();
        let script_path = context.shell_script_path().expect("a task run always gets one");
        assert_eq!(prompt, format!(". '{}'", script_path.display()));
        assert!(
            prompt.len() < 300,
            "the typed line no longer scales with the instance root or the instruction: {} bytes",
            prompt.len()
        );
        std::fs::remove_dir_all(&context.guides_dir).ok();
    }

    #[tokio::test]
    async fn the_shell_agents_script_carries_the_whole_report_wrapper() {
        let context = ctx(None);
        ShellAgent.prompt(&context).await.unwrap();
        let script_path = context.shell_script_path().unwrap();
        let script = std::fs::read_to_string(&script_path).unwrap();
        let bin = context.factory_bin.display();

        assert!(script.contains(&format!("_factory_bin='{bin}'")), "{script}");
        assert!(script.contains("_factory_id=t1"), "{script}");
        assert!(
            script.contains("--status running --message 'shell agent started' >/dev/null"),
            "{script}"
        );
        assert!(script.contains("make it stop flaking"), "the instruction, verbatim: {script}");
        assert!(
            script.contains("--status done --result 'command exited 0' --result-file \"$_factory_out\""),
            "{script}"
        );
        assert!(
            script.contains(
                "--status failed --result \"command exited $_factory_code\" \
                 --error \"command exited $_factory_code\" --result-file \"$_factory_out\""
            ),
            "{script}"
        );
        assert!(script.contains("rm -f \"$_factory_out\" \"$_factory_rc\""), "{script}");
        assert!(!script.contains("FACTORY_UPSTREAM_FILE"), "nothing upstream, nothing exported: {script}");
        std::fs::remove_dir_all(&context.guides_dir).ok();
    }

    #[tokio::test]
    async fn a_huge_instruction_produces_a_short_typed_line_via_a_script_file() {
        let mut context = ctx(None);
        let long_command = format!("printf '%s' '{}'", "x".repeat(2000));
        context.task.as_mut().unwrap().task.instructions = long_command.clone();

        let prompt = ShellAgent.prompt(&context).await.unwrap();
        assert!(
            prompt.len() < 300,
            "the typed line must not scale with the instruction's length: {} bytes",
            prompt.len()
        );
        assert!(prompt.starts_with(". '"), "{prompt}");

        let script_path = context.shell_script_path().unwrap();
        let script = std::fs::read_to_string(&script_path).unwrap();
        assert!(script.contains(&long_command), "the instruction goes into the file verbatim");
        std::fs::remove_dir_all(&context.guides_dir).ok();
    }

    #[tokio::test]
    async fn the_shell_agents_prompt_and_launch_export_the_upstream_file_only_when_there_is_one() {
        let upstream = vec![UpstreamOutput {
            node_id: "a".into(),
            task_id: "ta".into(),
            title: "build it".into(),
            result: Some("build succeeded".into()),
        }];
        let context = ctx_with_upstream(upstream);
        let expected_path = context.upstream_path().expect("this task has an upstream file");

        let launch = ShellAgent.launch_spec(&context).await.unwrap();
        assert_eq!(
            launch.env.get("FACTORY_UPSTREAM_FILE"),
            Some(&expected_path.display().to_string()),
            "launch_spec exports it too, for herdr's own --env"
        );

        let prompt = ShellAgent.prompt(&context).await.unwrap();
        assert!(prompt.starts_with(". '"), "the typed line is still just the script path: {prompt}");

        let script_path = context.shell_script_path().unwrap();
        let script = std::fs::read_to_string(&script_path).unwrap();
        assert!(
            script.contains(&format!("export FACTORY_UPSTREAM_FILE='{}'", expected_path.display())),
            "the script exports it too, for robustness: {script}"
        );
        assert!(script.contains("--result-file"), "stdout still goes through a file: {script}");

        let written: Vec<UpstreamOutput> =
            serde_json::from_str(&std::fs::read_to_string(&expected_path).unwrap()).unwrap();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].result.as_deref(), Some("build succeeded"));
        std::fs::remove_dir_all(&context.guides_dir).ok();
    }

    fn ctx_with_knowledge() -> AgentContext {
        let mut context = ctx(None);
        context.task.as_mut().unwrap().knowledge = Some(KnowledgeHints {
            vault: "/co/.factory/knowledge".into(),
            hits: vec![factory_core::adapter::KnowledgeHit {
                page: "clients/acme".into(),
                title: "Acme GmbH".into(),
                score: 4.0,
                why: "tags: invoicing".into(),
            }],
        });
        context
    }

    #[tokio::test]
    async fn a_harness_prompt_lists_the_knowledge_pages_by_path_and_reason() {
        let prompt = HarnessAgent::claude_code().prompt(&ctx_with_knowledge()).await.unwrap();
        assert!(prompt.contains("company knowledge base"), "{prompt}");
        assert!(
            prompt.contains("- /co/.factory/knowledge/clients/acme.md -- Acme GmbH (tags: invoicing)"),
            "{prompt}"
        );
        let hints_at = prompt.find("company knowledge base").unwrap();
        let contract_at = prompt.find("Report progress").unwrap();
        assert!(hints_at < contract_at, "task content comes before the reporting contract");
    }

    #[tokio::test]
    async fn a_prompt_without_knowledge_hints_says_nothing_about_them() {
        let prompt = HarnessAgent::claude_code().prompt(&ctx(None)).await.unwrap();
        assert!(!prompt.contains("knowledge base"), "{prompt}");
        let launch = ShellAgent.launch_spec(&ctx(None)).await.unwrap();
        assert!(!launch.env.contains_key("FACTORY_KNOWLEDGE_FILE"));
    }

    #[tokio::test]
    async fn the_shell_agent_exports_the_knowledge_file_in_its_launch_and_its_script() {
        let context = ctx_with_knowledge();
        let launch = ShellAgent.launch_spec(&context).await.unwrap();
        let path = launch.env.get("FACTORY_KNOWLEDGE_FILE").expect("exported for herdr's own --env").clone();

        ShellAgent.prompt(&context).await.unwrap();
        let script = std::fs::read_to_string(context.shell_script_path().unwrap()).unwrap();
        assert!(script.contains(&format!("export FACTORY_KNOWLEDGE_FILE='{path}'")), "{script}");

        let written: KnowledgeHints = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written, context.task.as_ref().unwrap().knowledge.clone().unwrap());
        std::fs::remove_dir_all(&context.guides_dir).ok();
    }

    #[test]
    fn shell_single_quote_escapes_embedded_quotes_and_refuses_a_newline() {
        assert_eq!(shell_single_quote("plain"), Some("'plain'".to_string()));
        assert_eq!(shell_single_quote("it's"), Some(r"'it'\''s'".to_string()));
        assert_eq!(shell_single_quote("a\nb"), None, "a newline cannot survive a one-line command");
    }

    // -- actually running the line, in both bash and zsh ---------------------
    //
    // Everything above pins the *text* of the line. This runs it for real, in
    // both shells this line has to work in, against a stub standing in for
    // the real `factory` binary -- so quoting, the temp-file dance, and the
    // exit code really do survive a live shell rather than just looking like
    // they should on paper.

    fn find_on_path(name: &str) -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path).find_map(|dir| {
            let candidate = dir.join(name);
            candidate.is_file().then_some(candidate)
        })
    }

    /// Stands in for the real `factory` binary: instead of talking to a
    /// daemon, it records every invocation's `--status`, `--error`,
    /// `--result`, and (reading the file `--result-file` names, since that
    /// is the whole point being tested) `--result-file`'s content, into
    /// files under `$STUB_DIR` -- each call overwrites the last, so what is
    /// left after the whole line runs is whatever the *final* report said.
    /// `--result` and `--result-file` are captured separately rather than
    /// combined here: how they combine into one report is the real CLI's
    /// own job (and its own unit tests), not something worth re-implementing
    /// in a shell script for this one.
    const STUB_FACTORY: &str = r#"#!/bin/sh
echo "$@" >> "$STUB_DIR/calls.log"
prev=""
for arg in "$@"; do
  case "$prev" in
    --status) echo "$arg" > "$STUB_DIR/last_status" ;;
    --error) printf '%s' "$arg" > "$STUB_DIR/last_error" ;;
    --result) printf '%s' "$arg" > "$STUB_DIR/last_result_header" ;;
    --result-file) cat "$arg" > "$STUB_DIR/last_result_body" 2>/dev/null || : ;;
  esac
  prev="$arg"
done
exit 0
"#;

    fn write_stub_factory(dir: &std::path::Path) -> PathBuf {
        let path = dir.join("factory");
        std::fs::write(&path, STUB_FACTORY).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        path
    }

    #[derive(Debug)]
    struct StubReport {
        status: String,
        error: String,
        result_header: String,
        result_body: String,
        pane_stdout: String,
        /// What `ShellAgent::prompt` actually returned -- `. '<script path>'`
        /// today -- so a caller can assert on its shape or length without
        /// recomputing it.
        line: String,
    }

    async fn run_shell_line(shell: &str, command: &str) -> StubReport {
        let dir = std::env::temp_dir().join(format!("factory-shell-line-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let stub = write_stub_factory(&dir);

        let mut context = ctx(None);
        context.factory_bin = stub;
        // The script file lands under the same directory this function
        // cleans up at the end, rather than the `ctx(None)` default, so one
        // `remove_dir_all` gets both the stub's recorded files and it.
        context.guides_dir = dir.join("guides");
        context.task.as_mut().unwrap().task.instructions = command.to_string();
        let line = ShellAgent.prompt(&context).await.unwrap();

        let output = std::process::Command::new(shell)
            .arg("-c")
            .arg(&line)
            .env("STUB_DIR", &dir)
            .output()
            .unwrap_or_else(|e| panic!("running `{shell} -c '{line}'`: {e}"));

        let report = StubReport {
            status: std::fs::read_to_string(dir.join("last_status")).unwrap_or_default().trim().to_string(),
            error: std::fs::read_to_string(dir.join("last_error")).unwrap_or_default(),
            result_header: std::fs::read_to_string(dir.join("last_result_header")).unwrap_or_default(),
            result_body: std::fs::read_to_string(dir.join("last_result_body")).unwrap_or_default(),
            pane_stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            line,
        };
        std::fs::remove_dir_all(&dir).ok();
        report
    }

    #[tokio::test]
    async fn the_shell_line_survives_quoting_and_carries_real_stdout_and_exit_code() {
        for shell in ["bash", "zsh"] {
            if find_on_path(shell).is_none() {
                eprintln!("skipping {shell}: not on PATH");
                continue;
            }

            // Single quotes, double quotes, `$` that must not expand, a
            // backtick, and more than one line.
            let success = run_shell_line(
                shell,
                r#"printf '%s\n' 'it'\''s "quoted" $HOME'; printf '%s\n' 'back`tick`'"#,
            )
            .await;
            assert_eq!(success.status, "done", "{shell}: {success:?}");
            assert_eq!(success.result_header, "command exited 0", "{shell}: {success:?}");
            assert_eq!(
                success.result_body,
                "it's \"quoted\" $HOME\nback`tick`\n",
                "the command's real stdout, byte for byte ({shell}): {success:?}"
            );
            assert!(
                success.pane_stdout.contains(r#"it's "quoted" $HOME"#),
                "stdout stays visible in the pane too ({shell}): {success:?}"
            );

            let failure = run_shell_line(shell, "echo out; exit 3").await;
            assert_eq!(failure.status, "failed", "{shell}: {failure:?}");
            assert_eq!(failure.error.trim(), "command exited 3", "{shell}: {failure:?}");
            assert_eq!(failure.result_header, "command exited 3", "{shell}: {failure:?}");
            assert_eq!(failure.result_body, "out\n", "{shell}: {failure:?}");

            // A real newline in the instructions themselves, not just `\n`
            // inside a printf format string -- only possible now that the
            // instruction goes into a script file verbatim rather than a
            // typed line, where an embedded newline would have submitted
            // the command early.
            let multiline = run_shell_line(shell, "printf 'one\\n'\nprintf 'two\\n'").await;
            assert_eq!(multiline.status, "done", "{shell}: {multiline:?}");
            assert_eq!(multiline.result_body, "one\ntwo\n", "{shell}: {multiline:?}");

            // The two shapes that break if the instruction shares a line
            // with the subshell's closing `)`: a trailing comment, and a
            // heredoc whose terminator has to stand alone on its line.
            let heredoc = run_shell_line(shell, "cat <<EOF\nfrom a heredoc\nEOF\necho done # trailing comment").await;
            assert_eq!(heredoc.status, "done", "{shell}: {heredoc:?}");
            assert_eq!(heredoc.result_body, "from a heredoc\ndone\n", "{shell}: {heredoc:?}");
        }
    }

    #[tokio::test]
    async fn a_huge_instruction_produces_a_short_typed_line_and_still_runs() {
        // The real proof that moving the wrapper into a file actually fixes
        // the pty problem this feature ran into: an instruction this long
        // would never have survived being typed whole (see `prompt`'s own
        // comment on `MAX_CANON`), but the typed line itself no longer
        // scales with it, so it runs exactly as any other command would.
        for shell in ["bash", "zsh"] {
            if find_on_path(shell).is_none() {
                eprintln!("skipping {shell}: not on PATH");
                continue;
            }
            let payload = "x".repeat(2000);
            let command = format!("printf '%s\\n' '{payload}'");
            assert!(command.len() > 2000, "the instruction really is the ~2000-byte case: {}", command.len());

            let report = run_shell_line(shell, &command).await;
            assert!(
                report.line.len() < 300,
                "the typed line must not scale with the instruction's length ({shell}): {} bytes",
                report.line.len()
            );
            assert_eq!(report.status, "done", "{shell}: {report:?}");
            assert!(
                report.result_body.contains(&payload),
                "{shell}: the ~2000-byte stdout should have survived intact"
            );
        }
    }
}
