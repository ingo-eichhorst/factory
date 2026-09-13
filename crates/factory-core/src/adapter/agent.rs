use crate::error::{FactoryError, Result};
use crate::role::{Grant, Reach, RoleDef};
use crate::task::Task;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// How a runtime is to bring this agent up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchKind {
    /// A kind the runtime recognises by name and knows how to detect
    /// (`pi`, `claude`, `codex`, ...).
    Named(String),
    /// A command line to run in the session. The escape hatch that keeps
    /// plugin agents possible on a runtime with a closed list of known kinds.
    Command(Vec<String>),
}

/// What the agent adapter asks the runtime for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchSpec {
    pub kind: LaunchKind,
    /// Extra arguments handed to the agent process.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment for the session. The callback contract travels here as well
    /// as in the prompt, so an agent can read it instead of parsing prose.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// What ties a launch to one attempt at one task. Absent when the agent is
/// being started to stand there rather than to do something.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskBinding {
    pub task: Task,
    pub run_id: String,
    pub attempt: u32,
    /// The secret the agent must present when reporting on this run.
    pub token: String,
    /// Set when this run is working in a git worktree of its own rather than
    /// the scope, so the prompt can say so and name the branch. `None` for a
    /// run that used the scope directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_branch: Option<String>,
}

/// Everything an agent adapter needs to phrase a prompt and a launch.
#[derive(Debug, Clone)]
pub struct AgentContext {
    pub scope: String,
    /// The concrete agent Factory knows this session as -- a declaration's
    /// name, or a bare adapter used with no declaration. Not a harness name.
    pub agent_name: String,
    /// Absolute working directory for the run.
    pub cwd: PathBuf,
    /// Absolute path to the `factory` binary the agent is to call back with.
    pub factory_bin: PathBuf,
    /// Absolute path to the daemon's control socket.
    pub socket: PathBuf,
    /// Where a guide written to a file, rather than passed as text, lives --
    /// under the instance root's `.factory/`, never inside a scope.
    pub guides_dir: PathBuf,
    /// `None` for a standing agent: it is being started to be there, and has
    /// nothing to report on.
    pub task: Option<TaskBinding>,
    /// What this agent presents to say which agent it is, when it is a standing
    /// one. A task run says so with its run token instead.
    pub identity_token: Option<String>,
    /// The role this agent is actually working under, resolved by
    /// `Engine::effective_role` at launch, the same call `caller_for` uses for
    /// every other request. `None` when the instance no longer defines the
    /// role the agent was given -- `access.rs` then refuses every grant, so
    /// the guide says so too rather than describing one that would be denied.
    pub role: Option<RoleDef>,
}

/// Where a task run's guide file lives, given only its task id and the
/// instance's guides directory. A free function rather than only a method on
/// `AgentContext` so a caller that has neither -- `close_session`, ending a
/// run after the launch that built the context is long gone -- can name the
/// exact same path without rebuilding the naming scheme by hand and risking
/// the two drifting apart. `AgentContext::guide_path` delegates here for the
/// task case; keep it that way rather than duplicating the format string.
pub fn run_guide_path(guides_dir: &Path, task_id: &str) -> PathBuf {
    guides_dir.join(format!("run-{task_id}.md"))
}

impl AgentContext {
    pub fn binding(&self) -> Result<&TaskBinding> {
        self.task.as_ref().ok_or_else(|| {
            FactoryError::BadRequest(
                "this agent was started without a task, so there is nothing to say to it".into(),
            )
        })
    }

    /// The reporting contract, in the words every built-in agent puts in its
    /// prompt. Adapters are free to phrase it differently, but the commands
    /// have to be these.
    pub fn reporting_contract(&self) -> String {
        let Some(binding) = &self.task else {
            return String::new();
        };
        let bin = self.factory_bin.display();
        let id = &binding.task.id;
        format!(
            "Report progress by running these commands in your shell. They are how \
             this task is tracked; nothing watches your terminal to guess.\n\
             \n\
             - Starting work:   {bin} task report {id} --status running --message \"<what you are doing>\"\n\
             - A useful update: {bin} task report {id} --message \"<the update>\"\n\
             - Need a human:    {bin} task report {id} --status blocked --message \"<what you need>\"\n\
             - Finished:        {bin} task report {id} --status done --result \"<what you did>\"\n\
             - Gave up:         {bin} task report {id} --status failed --error \"<why>\"\n\
             \n\
             Report running first, then finish with exactly one of done, failed, or \
             blocked. The task stays open until you do."
        )
    }

    /// A short, adapter-neutral guide to Factory itself: what it is, who this
    /// agent is, how to call it, and exactly the commands its role allows.
    /// Adapters put this in the agent's *system* prompt rather than the task
    /// prompt, so it survives however long the session runs and whatever of
    /// the task prompt gets compacted away. It deliberately says nothing
    /// about reporting: `reporting_contract()` already carries those exact
    /// commands, in the task prompt, and this points there instead of saying
    /// them a second time in a second place.
    pub fn factory_guide(&self) -> String {
        let mut out = String::new();

        out.push_str(
            "You are running inside Factory, a daemon that hands tasks to \
             coding agents and tracks what happens. A scope is a place work \
             happens -- a project with its own agents and configuration. A \
             task is the standing intent: what to do, and with which agent; \
             a run is one attempt at it, with its own session and outcome. \
             Status comes only from an agent calling `task report`, never \
             from what a terminal looks like.\n\n",
        );

        match &self.task {
            Some(binding) => out.push_str(&format!(
                "You are {name}, in scope {scope}, working task {task_id} \
                 (\"{title}\"). This session ends with the run.\n\n",
                name = self.agent_name,
                scope = self.scope,
                task_id = binding.task.id,
                title = binding.task.title,
            )),
            None => out.push_str(&format!(
                "You are {name}, a standing agent in scope {scope}: you were \
                 started to be here, not to finish something. If nobody has \
                 given you a task, that is the job -- sit quiet and wait \
                 rather than looking for work.\n\n",
                name = self.agent_name,
                scope = self.scope,
            )),
        }

        match &self.role {
            Some(def) => {
                let reach = match def.reach {
                    Reach::Own => {
                        "your own work only -- tasks assigned to you, the run you hold, yourself"
                    }
                    Reach::Scope => "everything in your scope, and nothing past it",
                };
                out.push_str(&format!(
                    "Your role is {role} ({describe}). Its reach is {reach}. \
                     A role describes your job; it is not a lock, so follow \
                     it rather than test it. If a command is refused, the \
                     reply names the grant you are missing -- that is the \
                     answer, not a reason to try again a different way. \
                     `factory agent role` can hand you a different role \
                     while you run; `factory agents` shows the one actually \
                     in effect right now.\n\n",
                    role = def.name,
                    describe = def.describe,
                    reach = reach,
                ));
            }
            None => out.push_str(
                "You were given a role this instance no longer defines, so \
                 none of the write commands below are yours -- only reading \
                 is. `factory agents` shows the roles this instance knows.\n\n",
            ),
        }

        let bin = self.factory_bin.display();
        out.push_str(&format!(
            "Call Factory as {bin} -- the absolute path, so a `factory` \
             missing from your PATH does not matter. FACTORY_TOKEN is \
             already set in your environment and sent on every request, so \
             you never need to pass --token yourself. Add --json to any \
             command for output meant to be parsed rather than read.\n\n\
             Reading is open to every agent, whatever your role: {bin} task \
             list, task show <id>, task log <id>, task output <id>; {bin} \
             agents; {bin} run list [task-id], run show <run-id>, run log \
             <run-id>, run output <run-id>; {bin} adapters.\n\n",
        ));

        if let Some(def) = &self.role {
            let lines = self.granted_command_lines(def);
            if !lines.is_empty() {
                out.push_str("Your role also lets you:\n");
                for line in lines {
                    out.push_str(&format!("- {line}\n"));
                }
                out.push('\n');
            }
        }

        out.push_str(
            "Your reach stops where the boundary above says it does. Never \
             write anything of Factory's own -- like this guide -- into a \
             scope; a scope owns only its own config.yaml, and everything \
             else Factory keeps stays under the instance root's .factory/.",
        );

        out
    }

    /// One line per grant this role holds, each mapped to the command that
    /// exercises it. A grant with no command of its own yet -- reachable
    /// today only through the web UI -- says so honestly rather than
    /// inventing one.
    fn granted_command_lines(&self, def: &RoleDef) -> Vec<String> {
        let bin = self.factory_bin.display();
        let scope = &self.scope;
        let mut lines = Vec::new();
        for grant in Grant::ALL {
            if !def.allows(grant) {
                continue;
            }
            lines.push(match grant {
                Grant::TaskCreate => format!(
                    "task.create -> {bin} task create \"<title>\" -i \"<instructions>\" --scope {scope} --agent <agent>"
                ),
                Grant::TaskEdit => {
                    format!("task.edit -> {bin} task edit <id> ... (see --help for every field)")
                }
                Grant::TaskDelete => format!("task.delete -> {bin} task delete <id>"),
                Grant::TaskRun => format!("task.run -> {bin} task run <id>"),
                Grant::TaskCancel => format!("task.cancel -> {bin} task cancel <id>"),
                Grant::TaskReport => match &self.task {
                    // Not repeated here: the exact commands for this run are
                    // already in the task prompt.
                    Some(_) => {
                        "task.report -> the exact commands are in your task prompt, not repeated here".to_string()
                    }
                    None => format!(
                        "task.report -> {bin} task report <id> --status <running|done|failed|blocked> --message/--result/--error \"...\", once you are given a task to report on"
                    ),
                },
                Grant::AgentStart => format!("agent.start -> {bin} agent start {scope} <name>"),
                Grant::AgentConfigure => {
                    "agent.configure -> add, edit or delete a standing agent's declaration in its scope; today that is the web UI's Roster, not this CLI".to_string()
                }
                Grant::AgentStop => format!("agent.stop -> {bin} agent stop <id>"),
                Grant::AgentInput => {
                    format!("agent.input -> {bin} agent input <id> --text \"...\" --key enter")
                }
                Grant::RunInput => {
                    "run.input -> type into a run's own terminal; today that is the web UI, not this CLI".to_string()
                }
            });
        }
        lines
    }

    /// Where this session's guide file would live, whether or not anything
    /// has written it yet. Named per task, not per run: only one run of a
    /// task is ever in progress at a time, so a retry safely overwrites
    /// rather than leaving the last attempt's file behind. A standing
    /// agent's is named for its own identity, so restarting it overwrites
    /// the same file instead of piling up a new one.
    pub fn guide_path(&self) -> PathBuf {
        match &self.task {
            Some(binding) => run_guide_path(&self.guides_dir, &binding.task.id),
            None => self
                .guides_dir
                .join(format!("agent-{}-{}.md", self.scope, self.agent_name)),
        }
    }

    /// Write the guide to its file, for a harness whose own system-prompt
    /// mechanism wants a path rather than inline text. Overwrites whatever
    /// was there, per `guide_path`'s naming.
    pub fn write_guide_file(&self) -> Result<PathBuf> {
        let path = self.guide_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| FactoryError::Other(anyhow::anyhow!("making {}: {e}", parent.display())))?;
        }
        std::fs::write(&path, self.factory_guide())
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("writing {}: {e}", path.display())))?;
        Ok(path)
    }

    /// The same contract as environment, for adapters that would rather read it.
    pub fn env(&self) -> BTreeMap<String, String> {
        let mut env = BTreeMap::from([
            ("FACTORY_SCOPE".to_string(), self.scope.clone()),
            ("FACTORY_SOCKET".to_string(), self.socket.display().to_string()),
            ("FACTORY_BIN".to_string(), self.factory_bin.display().to_string()),
        ]);
        // One variable says which agent is calling, whether it is standing
        // there or working a task. The CLI sends it on every request.
        if let Some(token) = self
            .task
            .as_ref()
            .map(|b| b.token.clone())
            .or_else(|| self.identity_token.clone())
        {
            env.insert("FACTORY_TOKEN".into(), token);
        }
        if let Some(b) = &self.task {
            env.insert("FACTORY_TASK_ID".into(), b.task.id.clone());
            env.insert("FACTORY_TASK_TOKEN".into(), b.token.clone());
            env.insert("FACTORY_RUN_ID".into(), b.run_id.clone());
            env.insert("FACTORY_RUN_ATTEMPT".into(), b.attempt.to_string());
        }
        env
    }
}

/// Adapter seam 1: a coding agent. Knows how its harness is started and how to
/// say a task to it.
#[async_trait::async_trait]
pub trait Agent: Send + Sync {
    fn name(&self) -> &str;

    /// One line for `factory adapters`.
    fn description(&self) -> String {
        format!("{} agent", self.name())
    }

    /// How to bring this agent up. Called for a task run and for a standing
    /// agent alike; `ctx.task` says which.
    async fn launch_spec(&self, ctx: &AgentContext) -> Result<LaunchSpec>;

    /// The text submitted to the agent once it is up. Only called when there
    /// is a task: a standing agent is started and then left alone.
    async fn prompt(&self, ctx: &AgentContext) -> Result<String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::role::Roles;
    use crate::task::{Task, TaskStatus};
    use std::collections::BTreeSet;

    fn task() -> Task {
        let now = chrono::Utc::now();
        Task {
            id: "t1".into(),
            title: "fix the flaky test".into(),
            instructions: String::new(),
            scope: "demo".into(),
            agent: "watcher".into(),
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
            worktree: false,
            labels: Default::default(),
            created_at: now,
            updated_at: now,
            last_run_at: None,
            next_run_at: None,
        }
    }

    fn base(role: Option<RoleDef>) -> AgentContext {
        AgentContext {
            scope: "demo".into(),
            agent_name: "watcher".into(),
            cwd: PathBuf::from("/tmp/somewhere"),
            factory_bin: PathBuf::from("/usr/local/bin/factory"),
            socket: PathBuf::from("/tmp/factory.sock"),
            guides_dir: PathBuf::from("/tmp/factory-guides"),
            task: None,
            identity_token: Some("identity".into()),
            role,
        }
    }

    fn with_task(mut ctx: AgentContext) -> AgentContext {
        ctx.task = Some(TaskBinding {
            task: task(),
            run_id: "r1".into(),
            attempt: 1,
            token: "tok".into(),
            worktree_branch: None,
        });
        ctx.identity_token = None;
        ctx
    }

    fn worker() -> RoleDef {
        Roles::presets().get(&crate::role::Role::worker()).unwrap().clone()
    }

    fn foreman() -> RoleDef {
        Roles::presets().get(&crate::role::Role::foreman()).unwrap().clone()
    }

    fn custom(grants: &[Grant], reach: Reach) -> RoleDef {
        RoleDef {
            name: crate::role::Role::new("runner"),
            describe: "starts what is already on the board".into(),
            grants: grants.iter().copied().collect::<BTreeSet<_>>(),
            reach,
        }
    }

    #[test]
    fn a_worker_is_never_told_it_can_create_tasks() {
        let guide = base(Some(worker())).factory_guide();
        assert!(guide.contains("task.edit"));
        assert!(guide.contains("run.input"));
        assert!(!guide.contains("task.create"), "a worker cannot create tasks: {guide}");
        assert!(!guide.contains("agent.start"));
    }

    #[test]
    fn a_foreman_is_told_it_can_create_edit_and_assign_tasks_in_its_scope() {
        let guide = base(Some(foreman())).factory_guide();
        for grant in ["task.create", "task.edit", "task.delete", "task.run", "task.cancel", "agent.start"] {
            assert!(guide.contains(grant), "a foreman has {grant}: {guide}");
        }
        assert!(guide.contains("everything in your scope"));
    }

    #[test]
    fn a_custom_role_gets_exactly_its_grants_and_nothing_the_grants_do_not_cover() {
        let guide = base(Some(custom(&[Grant::TaskRun, Grant::TaskCancel], Reach::Scope))).factory_guide();
        assert!(guide.contains("task.run"));
        assert!(guide.contains("task.cancel"));
        assert!(!guide.contains("task.create"));
        assert!(!guide.contains("task.edit"));
        assert!(!guide.contains("agent.input"));
    }

    #[test]
    fn a_role_this_instance_no_longer_defines_claims_no_grants() {
        // `access.rs` refuses every grant when `Roles::get` finds nothing for
        // the role an agent is holding; the guide must not promise more than
        // that check would allow, so it says nothing under "your role also
        // lets you" rather than guessing.
        let guide = base(None).factory_guide();
        assert!(!guide.contains("Your role also lets you"));
        assert!(guide.contains("no longer defines"));
    }

    #[test]
    fn the_guide_never_repeats_the_reporting_contract() {
        let ctx = with_task(base(Some(worker())));
        let guide = ctx.factory_guide();
        let contract = ctx.reporting_contract();
        assert!(!contract.is_empty());
        // The reporting contract's own distinctive phrasing must appear only
        // in the contract, not duplicated into the guide.
        assert!(!guide.contains("<what you are doing>"));
        assert!(!guide.contains("Gave up:"));
        assert!(guide.contains("task prompt"), "it points at the contract instead: {guide}");
    }

    #[test]
    fn a_standing_agent_holding_task_report_gets_a_generic_form_since_it_has_no_task_prompt() {
        let guide = base(Some(worker())).factory_guide();
        assert!(!guide.contains("task prompt"), "there is no task prompt to point at: {guide}");
        assert!(guide.contains("task.report"));
        assert!(guide.contains("factory task report"));
    }

    #[test]
    fn who_you_are_differs_for_a_standing_agent_and_a_task_run() {
        let standing = base(Some(worker())).factory_guide();
        assert!(standing.contains("a standing agent"));
        assert!(standing.contains("sit quiet and wait"));

        let running = with_task(base(Some(worker()))).factory_guide();
        assert!(running.contains("working task t1"));
        assert!(!running.contains("a standing agent"));
    }

    #[test]
    fn the_guide_never_names_a_harness_or_herdr() {
        // Adapter-neutral: every harness injects the same text.
        let guide = with_task(base(Some(foreman()))).factory_guide().to_lowercase();
        for phrase in ["herdr", "claude", "codex", "opencode"] {
            assert!(!guide.contains(phrase), "{phrase} should not appear: {guide}");
        }
        let words: BTreeSet<&str> = guide
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();
        assert!(!words.contains("pi"), "pi should not appear as its own word: {guide}");
    }

    #[test]
    fn guide_paths_are_named_per_task_for_a_run_and_per_identity_for_a_standing_agent() {
        let standing = base(None);
        assert_eq!(standing.guide_path(), PathBuf::from("/tmp/factory-guides/agent-demo-watcher.md"));

        let running = with_task(base(None));
        assert_eq!(running.guide_path(), PathBuf::from("/tmp/factory-guides/run-t1.md"));
    }

    #[test]
    fn a_task_runs_guide_path_agrees_with_the_free_function_a_caller_with_no_context_would_use() {
        // `close_session`, ending a run well after the `AgentContext` that
        // launched it is gone, has only a task id and the guides directory
        // to work with. It must land on exactly the path `guide_path` built,
        // or it deletes nothing and a run leaks a file. Delegation to
        // `run_guide_path`, pinned here, is what keeps that true by
        // construction rather than by two format strings staying in sync.
        let running = with_task(base(None));
        assert_eq!(
            running.guide_path(),
            run_guide_path(&PathBuf::from("/tmp/factory-guides"), "t1")
        );
    }
}
