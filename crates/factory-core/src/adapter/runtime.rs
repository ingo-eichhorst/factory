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

/// How a `RuntimeStatus` was arrived at. `RuntimeStatus` on its own cannot
/// tell a harness that told the runtime the truth apart from a runtime that
/// guessed from the pane's appearance, and that difference is the whole of
/// what makes `Blocked` safe to act on -- see issue #7 and `AGENTS.md`'s "a
/// task's status comes from the agent calling `factory task report`, never
/// from looking at a terminal and guessing".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusSource {
    /// The harness told the runtime through a lifecycle hook. This is a
    /// report.
    Reported,
    /// The runtime guessed from the terminal's appearance. This is a guess.
    Inferred,
    /// The runtime does not say which it did.
    Unknown,
}

impl StatusSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reported => "reported",
            Self::Inferred => "inferred",
            Self::Unknown => "unknown",
        }
    }

    /// Anything unrecognised is `Unknown`, the same convention `RuntimeStatus`
    /// uses just above: a runtime is free to grow a provenance nobody has
    /// named yet, and "we don't know how it knows" is the honest answer to
    /// that, not a parse error.
    pub fn parse(raw: &str) -> Self {
        match raw {
            "reported" => Self::Reported,
            "inferred" => Self::Inferred,
            _ => Self::Unknown,
        }
    }
}

/// A status plus how much Factory is allowed to trust it. Only a `Reported`
/// `Blocked` is a fact the daemon may act on; an `Inferred` or `Unknown` one
/// is a suspicion, and stays one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusReport {
    pub status: RuntimeStatus,
    pub source: StatusSource,
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

    /// `status`, plus whether it is a report or a guess. Deliberately a
    /// separate method rather than a change to `status`'s return type --
    /// `status` is a public seam `examples/plugins` and every existing
    /// runtime already implement, and this default keeps all of them
    /// compiling and correct: a runtime that never says how it knows is
    /// honestly `Unknown`, not silently `Reported`.
    async fn status_report(&self, session: &SessionRef) -> Result<StatusReport> {
        Ok(StatusReport {
            status: self.status(session).await?,
            source: StatusSource::Unknown,
        })
    }

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

    async fn stop(&self, session: &SessionRef) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_source_round_trips_through_its_wire_string() {
        for s in [StatusSource::Reported, StatusSource::Inferred, StatusSource::Unknown] {
            assert_eq!(StatusSource::parse(s.as_str()), s);
        }
    }

    #[test]
    fn an_unrecognised_status_source_parses_to_unknown_rather_than_erroring() {
        // The same convention as `RuntimeStatus::parse`: a runtime is free to
        // grow a provenance nobody named yet, and that is not a parse error.
        assert_eq!(StatusSource::parse("guessed-from-tea-leaves"), StatusSource::Unknown);
    }

    /// A runtime that only ever implements what the trait required before
    /// this issue -- every runtime that existed until now, and any plugin
    /// written against the old seam.
    struct BareRuntime;

    #[async_trait::async_trait]
    impl AgentRuntime for BareRuntime {
        fn name(&self) -> &str {
            "bare"
        }
        async fn start(&self, _req: &StartRequest) -> Result<SessionRef> {
            unimplemented!("not exercised by this test")
        }
        async fn submit(&self, _session: &SessionRef, _text: &str) -> Result<()> {
            unimplemented!("not exercised by this test")
        }
        async fn status(&self, _session: &SessionRef) -> Result<RuntimeStatus> {
            Ok(RuntimeStatus::Blocked)
        }
        async fn send_text(&self, _session: &SessionRef, _text: &str) -> Result<()> {
            unimplemented!("not exercised by this test")
        }
        async fn send_keys(&self, _session: &SessionRef, _keys: &[String]) -> Result<()> {
            unimplemented!("not exercised by this test")
        }
        async fn read(&self, _session: &SessionRef, _lines: u32) -> Result<String> {
            unimplemented!("not exercised by this test")
        }
        async fn stop(&self, _session: &SessionRef) -> Result<()> {
            unimplemented!("not exercised by this test")
        }
    }

    fn session() -> SessionRef {
        SessionRef {
            runtime: "bare".into(),
            handle: "h".into(),
            meta: Default::default(),
        }
    }

    #[tokio::test]
    async fn the_default_status_report_calls_through_to_status_and_says_unknown() {
        let report = BareRuntime.status_report(&session()).await.unwrap();
        assert_eq!(report.status, RuntimeStatus::Blocked, "still answers what status() says");
        assert_eq!(
            report.source,
            StatusSource::Unknown,
            "a runtime that never says how it knows must not come out Reported"
        );
    }
}
