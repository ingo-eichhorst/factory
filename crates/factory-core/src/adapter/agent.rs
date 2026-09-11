use crate::error::{FactoryError, Result};
use crate::task::Task;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

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
}

/// Everything an agent adapter needs to phrase a prompt and a launch.
#[derive(Debug, Clone)]
pub struct AgentContext {
    pub scope: String,
    /// Absolute working directory for the run.
    pub cwd: PathBuf,
    /// Absolute path to the `factory` binary the agent is to call back with.
    pub factory_bin: PathBuf,
    /// Absolute path to the daemon's control socket.
    pub socket: PathBuf,
    /// `None` for a standing agent: it is being started to be there, and has
    /// nothing to report on.
    pub task: Option<TaskBinding>,
    /// What this agent presents to say which agent it is, when it is a standing
    /// one. A task run says so with its run token instead.
    pub identity_token: Option<String>,
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
