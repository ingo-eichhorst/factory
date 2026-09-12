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

impl RuntimeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Gone => "gone",
            Self::Unknown => "unknown",
        }
    }

    /// Anything unrecognised is `Unknown` rather than an error: a runtime is
    /// free to grow a state we have never heard of, and a chart that says
    /// "unknown" is better than one that refuses to draw.
    pub fn parse(raw: &str) -> Self {
        match raw {
            "starting" => Self::Starting,
            "idle" => Self::Idle,
            "working" => Self::Working,
            "blocked" => Self::Blocked,
            "gone" => Self::Gone,
            _ => Self::Unknown,
        }
    }
}

/// One frame of a session's screen: what a person attached to it would be
/// looking at right now.
///
/// The frame is the rendered grid, not a byte stream -- the runtime has already
/// applied every cursor move and scroll, and what is left is text plus colour.
/// That is why it can be polled: each frame stands alone, so a dropped one
/// costs nothing and a late viewer needs no replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Screen {
    pub cols: u16,
    pub rows: u16,
    /// The visible grid, one line per row, carrying SGR escapes for colour.
    pub frame: String,
}

/// A push from a runtime that can push, rather than only answer when asked.
/// Narrow, and about sessions rather than panes -- this vocabulary must never
/// grow the nouns of whichever runtime happens to implement it. A variant is
/// added because the daemon needs it, not because one runtime's protocol
/// happens to have it.
#[derive(Debug, Clone)]
pub struct RuntimeEvent {
    pub session: SessionRef,
    pub kind: RuntimeEventKind,
}

#[derive(Debug, Clone)]
pub enum RuntimeEventKind {
    /// The session's liveness changed. Carries `RuntimeStatus`, which already
    /// degrades anything unrecognised to `Unknown` rather than erroring, so a
    /// push naming a state core has never heard of still lands somewhere
    /// sane.
    StatusChanged(RuntimeStatus),
    /// The session produced output. Nothing reads this yet -- output is a
    /// second pass, coalesced, once there is something to judge the volume
    /// against: unthrottled, it would blow the activity view's 500-row
    /// client-side window in seconds.
    OutputMoved,
    /// The session is no longer there.
    SessionGone,
}

/// A stream of a runtime's own pushes. A plain channel rather than a `Stream`
/// trait object -- nothing downstream needs combinators, and this way core
/// gains no dependency to get it.
pub type RuntimeEventStream = tokio::sync::mpsc::Receiver<RuntimeEvent>;

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

    /// The screen as it looks now, for a viewer that wants the terminal rather
    /// than the transcript. `None` is an honest answer from a runtime that can
    /// only produce scrollback -- the caller falls back to `read`.
    async fn screen(&self, _session: &SessionRef) -> Result<Option<Screen>> {
        Ok(None)
    }

    /// Events this runtime pushes on its own, for one that can. `None` is an
    /// honest "poll me" -- exactly what happens today -- not a failure:
    /// an out-of-process runtime cannot offer this yet (`proxy.rs` has no
    /// runtime proxy, and the plugin host is request/response with no
    /// notification path) and should not have to pretend otherwise. The poll
    /// stays the floor underneath this either way: a subscription that later
    /// drops falls back to it rather than the daemon going quiet.
    async fn watch(&self) -> Result<Option<RuntimeEventStream>> {
        Ok(None)
    }

    async fn stop(&self, session: &SessionRef) -> Result<()>;
}
