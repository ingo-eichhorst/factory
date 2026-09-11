use crate::error::Result;
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

/// Everything an agent adapter needs to phrase a prompt and a launch.
#[derive(Debug, Clone)]
pub struct AgentContext {
    pub task: Task,
    /// The attempt this prompt is for. A retry is a different run of the same
    /// task, and an adapter may want to say so.
    pub run_id: String,
    pub attempt: u32,
    /// Absolute working directory for the run.
    pub cwd: PathBuf,
    /// Absolute path to the `factory` binary the agent is to call back with.
    pub factory_bin: PathBuf,
    /// Absolute path to the daemon's control socket.
    pub socket: PathBuf,
    /// The secret the agent must present when reporting on this task.
    pub token: String,
}

impl AgentContext {
    /// The reporting contract, in the words every built-in agent puts in its
    /// prompt. Adapters are free to phrase it differently, but the commands
    /// have to be these.
    pub fn reporting_contract(&self) -> String {
        let bin = self.factory_bin.display();
        let id = &self.task.id;
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
        BTreeMap::from([
            ("FACTORY_TASK_ID".to_string(), self.task.id.clone()),
            ("FACTORY_RUN_ID".to_string(), self.run_id.clone()),
            ("FACTORY_RUN_ATTEMPT".to_string(), self.attempt.to_string()),
            ("FACTORY_TASK_TOKEN".to_string(), self.token.clone()),
            ("FACTORY_SOCKET".to_string(), self.socket.display().to_string()),
            ("FACTORY_BIN".to_string(), self.factory_bin.display().to_string()),
        ])
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

    async fn launch_spec(&self, ctx: &AgentContext) -> Result<LaunchSpec>;

    /// The text submitted to the agent once it is up.
    async fn prompt(&self, ctx: &AgentContext) -> Result<String>;
}
