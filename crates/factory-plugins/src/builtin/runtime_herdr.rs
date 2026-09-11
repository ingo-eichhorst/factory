//! The only runtime there is for now: herdr, driven through its CLI. The CLI
//! answers in JSON on stdout, so nothing here has to speak the socket protocol.
//!
//! One workspace per task. It is the unit herdr can close cleanly, and it makes
//! a running task something a person can find and watch.

use async_trait::async_trait;
use factory_core::adapter::agent::LaunchKind;
use factory_core::adapter::runtime::{AgentRuntime, RuntimeStatus, StartRequest};
use factory_core::error::{FactoryError, Result};
use factory_core::task::SessionRef;
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::process::Command;

const ADAPTER: &str = "herdr";

pub struct HerdrRuntime {
    bin: String,
    /// How long to wait for an agent to become ready for input.
    start_timeout: Duration,
    /// Which herdr session this daemon talks to, so the attach command it
    /// hands a person points at the same one.
    herdr_session: Option<String>,
}

impl HerdrRuntime {
    pub fn new() -> Self {
        let bin = std::env::var("FACTORY_HERDR_BIN").unwrap_or_else(|_| "herdr".into());
        let herdr_session = detect_session(&bin);
        Self {
            bin,
            start_timeout: Duration::from_secs(60),
            herdr_session,
        }
    }

    pub fn with_bin(bin: impl Into<String>) -> Self {
        Self {
            bin: bin.into(),
            ..Self::new()
        }
    }

    async fn run(&self, args: &[String]) -> Result<Value> {
        let output = Command::new(&self.bin)
            .args(args)
            .output()
            .await
            .map_err(|e| {
                FactoryError::adapter(ADAPTER, format!("running `{} {}`: {e}", self.bin, args.join(" ")))
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() {
            // herdr reports failures as JSON when it can; fall back to stderr.
            let detail = serde_json::from_str::<Value>(stdout.trim())
                .ok()
                .and_then(|v| v.get("error").cloned())
                .map(|e| e.to_string())
                .unwrap_or_else(|| {
                    let text = if stderr.trim().is_empty() { stdout } else { stderr };
                    text.trim().to_string()
                });
            return Err(FactoryError::adapter(
                ADAPTER,
                format!("`herdr {}` failed: {detail}", args.join(" ")),
            ));
        }

        if stdout.trim().is_empty() {
            return Ok(Value::Null);
        }
        let parsed: Value = serde_json::from_str(stdout.trim()).map_err(|e| {
            FactoryError::adapter(ADAPTER, format!("unreadable output from `herdr {}`: {e}", args.join(" ")))
        })?;
        if let Some(err) = parsed.get("error") {
            return Err(FactoryError::adapter(
                ADAPTER,
                format!("`herdr {}`: {err}", args.join(" ")),
            ));
        }
        Ok(parsed.get("result").cloned().unwrap_or(parsed))
    }

    /// `herdr pane read` answers in plain text, not JSON like the rest of the
    /// CLI, so it needs its own runner.
    async fn run_text(&self, args: &[String]) -> Result<String> {
        let output = Command::new(&self.bin)
            .args(args)
            .output()
            .await
            .map_err(|e| {
                FactoryError::adapter(ADAPTER, format!("running `{} {}`: {e}", self.bin, args.join(" ")))
            })?;
        if !output.status.success() {
            return Err(FactoryError::adapter(
                ADAPTER,
                format!(
                    "`herdr {}` failed: {}",
                    args.join(" "),
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    async fn start_agent_when_ready(&self, args: &[String]) -> Result<()> {
        let deadline = tokio::time::Instant::now() + self.start_timeout;
        let mut last;
        loop {
            match self.run(args).await {
                Ok(_) => return Ok(()),
                Err(e) => {
                    let busy = e.to_string().contains("agent_pane_busy")
                        || e.to_string().contains("not an available shell");
                    if !busy {
                        return Err(e);
                    }
                    last = e;
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(last);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    fn pane_of(session: &SessionRef) -> &str {
        &session.handle
    }
}

impl Default for HerdrRuntime {
    fn default() -> Self {
        Self::new()
    }
}

fn s(v: &str) -> String {
    v.to_string()
}

/// `herdr status` prints the socket it is talking to; the directory above it is
/// the session's name. Cheaper and more reliable than guessing "default".
fn detect_session(bin: &str) -> Option<String> {
    let out = std::process::Command::new(bin).arg("status").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let socket = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("socket:"))?
        .trim();
    std::path::Path::new(socket)
        .parent()?
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
}

#[async_trait]
impl AgentRuntime for HerdrRuntime {
    fn name(&self) -> &str {
        ADAPTER
    }

    fn description(&self) -> String {
        "one herdr workspace per task, agent started in its root pane".into()
    }

    async fn start(&self, req: &StartRequest) -> Result<SessionRef> {
        let mut args = vec![
            s("workspace"),
            s("create"),
            s("--cwd"),
            req.cwd.display().to_string(),
            s("--label"),
            req.label.clone(),
            s("--no-focus"),
        ];
        for (k, v) in &req.launch.env {
            args.push(s("--env"));
            args.push(format!("{k}={v}"));
        }

        let created = self.run(&args).await?;
        let pane = created
            .pointer("/root_pane/pane_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                FactoryError::adapter(ADAPTER, "workspace create returned no root pane id")
            })?
            .to_string();
        let workspace = created
            .pointer("/workspace/workspace_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        let mut meta = BTreeMap::from([
            ("workspace_id".to_string(), workspace.clone()),
            ("pane_id".to_string(), pane.clone()),
        ]);

        match &req.launch.kind {
            LaunchKind::Named(harness) => {
                let session_name = req.name.clone();
                let mut start = vec![
                    s("agent"),
                    s("start"),
                    session_name.clone(),
                    s("--kind"),
                    harness.clone(),
                    s("--pane"),
                    pane.clone(),
                    s("--timeout"),
                    self.start_timeout.as_millis().to_string(),
                ];
                if !req.launch.args.is_empty() {
                    start.push(s("--"));
                    start.extend(req.launch.args.iter().cloned());
                }
                // A freshly created pane has a shell in it, but herdr will not
                // start an agent until that shell has finished coming up and
                // drawn its prompt -- which on a heavy profile takes a second
                // or two. It answers `agent_pane_busy` until then, so wait for
                // it rather than treating the race as a failure.
                if let Err(e) = self.start_agent_when_ready(&start).await {
                    // If the agent will not come up, close the workspace rather
                    // than leaving an orphan pane behind for someone to find.
                    let _ = self.run(&[s("workspace"), s("close"), workspace]).await;
                    return Err(e);
                }
                meta.insert("mode".into(), "agent".into());
                meta.insert("agent_name".into(), session_name);
            }
            LaunchKind::Command(cmd) => {
                if !cmd.is_empty() {
                    let mut run = vec![s("pane"), s("run"), pane.clone()];
                    run.extend(cmd.iter().cloned());
                    run.extend(req.launch.args.iter().cloned());
                    if let Err(e) = self.run(&run).await {
                        let _ = self.run(&[s("workspace"), s("close"), workspace]).await;
                        return Err(e);
                    }
                }
                meta.insert("mode".into(), "shell".into());
            }
        }

        Ok(SessionRef {
            runtime: ADAPTER.into(),
            handle: pane,
            meta,
        })
    }

    async fn submit(&self, session: &SessionRef, text: &str) -> Result<()> {
        let pane = Self::pane_of(session);
        let shell_mode = session.meta.get("mode").map(String::as_str) == Some("shell");
        if shell_mode {
            // `pane run` sends the text and Enter in one call, which is what a
            // shell wants and what an agent's input box does not.
            self.run(&[s("pane"), s("run"), s(pane), text.to_string()])
                .await?;
        } else {
            let target = session
                .meta
                .get("agent_name")
                .cloned()
                .unwrap_or_else(|| pane.to_string());
            self.run(&[s("agent"), s("prompt"), target, text.to_string()])
                .await?;
        }
        Ok(())
    }

    async fn send_text(&self, session: &SessionRef, text: &str) -> Result<()> {
        self.run(&[
            s("pane"),
            s("send-text"),
            s(Self::pane_of(session)),
            text.to_string(),
        ])
        .await
        .map(|_| ())
    }

    async fn send_keys(&self, session: &SessionRef, keys: &[String]) -> Result<()> {
        if keys.is_empty() {
            return Ok(());
        }
        let mut args = vec![s("pane"), s("send-keys"), s(Self::pane_of(session))];
        args.extend(keys.iter().cloned());
        self.run(&args).await.map(|_| ())
    }

    fn attach_command(&self, session: &SessionRef) -> Option<String> {
        // Only a named agent can be attached to directly. A shell pane has no
        // agent to name, and printing a command that fails is worse than
        // admitting there isn't one.
        let name = session.meta.get("agent_name")?;
        let prefix = match &self.herdr_session {
            Some(s) if s != "default" => format!("herdr --session {s} "),
            _ => "herdr ".to_string(),
        };
        Some(format!("{prefix}agent attach {name}"))
    }

    async fn status(&self, session: &SessionRef) -> Result<RuntimeStatus> {
        let pane = Self::pane_of(session);
        let got = match self.run(&[s("pane"), s("get"), s(pane)]).await {
            Ok(v) => v,
            // A pane that is not there any more is a state, not a failure.
            Err(_) => return Ok(RuntimeStatus::Gone),
        };
        let raw = got
            .get("pane")
            .and_then(|p| p.get("agent_status"))
            .or_else(|| got.get("agent_status"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        Ok(match raw {
            "idle" => RuntimeStatus::Idle,
            "working" => RuntimeStatus::Working,
            "blocked" => RuntimeStatus::Blocked,
            "starting" => RuntimeStatus::Starting,
            _ => RuntimeStatus::Unknown,
        })
    }

    async fn read(&self, session: &SessionRef, lines: u32) -> Result<String> {
        let pane = Self::pane_of(session);
        // `--lines` reads from scrollback above the viewport and comes back
        // empty for a pane that has not scrolled, so take the whole snapshot
        // and keep the tail ourselves.
        let text = self
            .run_text(&[s("pane"), s("read"), s(pane), s("--format"), s("text")])
            .await?;
        let wanted = lines.max(1) as usize;
        let all: Vec<&str> = text.lines().collect();
        let tail = &all[all.len().saturating_sub(wanted)..];
        Ok(tail.join("\n"))
    }

    async fn stop(&self, session: &SessionRef) -> Result<()> {
        if let Some(ws) = session.meta.get("workspace_id").filter(|w| !w.is_empty()) {
            self.run(&[s("workspace"), s("close"), ws.clone()]).await?;
        } else {
            self.run(&[s("pane"), s("close"), s(Self::pane_of(session))])
                .await?;
        }
        Ok(())
    }
}
