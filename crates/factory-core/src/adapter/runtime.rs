use crate::adapter::agent::LaunchSpec;
use crate::error::Result;
use crate::task::SessionRef;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartRequest {
    /// What this session is for -- a run id or a standing agent's id. Used in
    /// logs, not by the runtime.
    pub id: String,
    /// The name the runtime should give the agent, if it names agents. This is
    /// what `attach_command` will point at, so it wants to be readable.
    pub name: String,
    /// Human-readable label for the session, so a person looking at the runtime
    /// can tell what it is.
    pub label: String,
    pub cwd: PathBuf,
    pub launch: LaunchSpec,
}

/// What the runtime can see about a session from the outside. This is a
/// liveness signal, not a completion signal -- a runtime that infers state from
/// a terminal's appearance will be wrong sometimes, so the daemon only uses
/// this to notice sessions that died or went quiet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    Starting,
    Idle,
    Working,
    Blocked,
    /// The session is no longer there.
    Gone,
    Unknown,
}

/// Adapter seam 2: where agents actually run. Opens a session, hands it a
/// prompt, and can be asked whether it is still alive.
#[async_trait::async_trait]
pub trait AgentRuntime: Send + Sync {
    fn name(&self) -> &str;

    fn description(&self) -> String {
        format!("{} runtime", self.name())
    }

    /// Bring up a session with the agent running in it, ready for input.
    async fn start(&self, req: &StartRequest) -> Result<SessionRef>;

    /// Submit text to a session that is already up.
    async fn submit(&self, session: &SessionRef, text: &str) -> Result<()>;

    async fn status(&self, session: &SessionRef) -> Result<RuntimeStatus>;

    /// Type literal text into the session without submitting it.
    async fn send_text(&self, session: &SessionRef, text: &str) -> Result<()>;

    /// Press keys: `enter`, `esc`, `up`, `down`, `ctrl-c`, and whatever else
    /// the runtime names. This is how a person answers a prompt the agent is
    /// sitting on without leaving the page.
    async fn send_keys(&self, session: &SessionRef, keys: &[String]) -> Result<()>;

    /// The command a person types to get into this session themselves, when
    /// the runtime can name one. `None` is an honest answer -- better than a
    /// command that will not work.
    fn attach_command(&self, _session: &SessionRef) -> Option<String> {
        None
    }

    /// Recent terminal output, for the UI and for post-mortems.
    async fn read(&self, session: &SessionRef, lines: u32) -> Result<String>;

    async fn stop(&self, session: &SessionRef) -> Result<()>;
}
