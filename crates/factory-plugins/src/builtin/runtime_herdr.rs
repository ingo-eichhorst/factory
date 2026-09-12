//! The only runtime there is for now: herdr, driven through its CLI. The CLI
//! answers in JSON on stdout, so nothing here has to speak the socket protocol.
//!
//! One workspace per task. It is the unit herdr can close cleanly, and it makes
//! a running task something a person can find and watch.
//!
//! `watch()` pushes the same way: `herdr agent wait <pane> --until idle
//! --until working --until blocked --until done`, blocked on in a loop, one
//! child process per session being watched. herdr also offers
//! `events.subscribe` over its socket -- one subscription for the whole
//! runtime, no per-agent process, and pane lifecycle and output alongside
//! status -- and that is the better answer once this needs more than status
//! changes. It is not what is built here: the CLI route keeps this adapter
//! inside its existing shape, at the price named above, plus a race between
//! one `wait` returning and the next starting. That race is closed by
//! re-reading `status()` itself the moment a wait comes back, rather than
//! trusting the state it happened to wait for.

use async_trait::async_trait;
use factory_core::adapter::agent::LaunchKind;
use factory_core::adapter::runtime::{
    AgentRuntime, RuntimeEvent, RuntimeEventKind, RuntimeEventStream, RuntimeStatus, Screen,
    StartRequest,
};
use factory_core::error::{FactoryError, Result};
use factory_core::task::SessionRef;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::mpsc;

const ADAPTER: &str = "herdr";
/// How long a single `agent wait` blocks before rearming itself. A bound here
/// is what stops a watched agent's child process outliving its session
/// forever if herdr never reports a matching transition -- it is not a poll
/// interval; the wait still returns the moment a real change happens.
const WAIT_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Which herdr states `agent wait` should block for, given the one Factory
/// currently believes the agent is in. Excludes whatever `current` already
/// is, so the wait genuinely blocks rather than matching immediately on
/// entry -- `herdr agent wait` is satisfied the instant the state it names is
/// already true, not only on a transition into it.
///
/// `status()` folds herdr's own `done` into `RuntimeStatus::Idle` (an agent
/// sitting on a finished turn reads the same as an empty bay to the
/// occupancy chart), so when `current` is `Idle` there is no way to tell
/// from that folded value alone whether the *raw* state is `idle` or `done`.
/// Excluding both is the safe answer: a transition between the two would not
/// change what Factory reports anyway, so failing to distinguish them here
/// costs nothing.
fn until_args(current: RuntimeStatus) -> &'static [&'static str] {
    match current {
        RuntimeStatus::Working => &["idle", "blocked", "done"],
        RuntimeStatus::Blocked => &["idle", "working", "done"],
        RuntimeStatus::Idle => &["working", "blocked"],
        RuntimeStatus::Starting | RuntimeStatus::Unknown | RuntimeStatus::Gone => {
            &["idle", "working", "blocked", "done"]
        }
    }
}

#[derive(Clone)]
pub struct HerdrRuntime {
    bin: String,
    /// How long to wait for an agent to become ready for input.
    start_timeout: Duration,
    /// Which herdr session this daemon talks to, so the attach command it
    /// hands a person points at the same one.
    herdr_session: Option<String>,
    /// Shared with every clone of this runtime. `watch()` hands out the
    /// sender once; a session noted afterward through any of the calls below
    /// gets a wait loop of its own, fed into that same channel.
    watch: Arc<Mutex<WatchState>>,
}

#[derive(Default)]
struct WatchState {
    tx: Option<mpsc::Sender<RuntimeEvent>>,
    /// Panes that already have a wait loop running, so a session seen twice
    /// -- once from `start()`, again from the next `status()` poll -- does
    /// not get two.
    watching: HashSet<String>,
}

impl HerdrRuntime {
    pub fn new() -> Self {
        let bin = std::env::var("FACTORY_HERDR_BIN").unwrap_or_else(|_| "herdr".into());
        let herdr_session = detect_session(&bin);
        Self {
            bin,
            start_timeout: Duration::from_secs(60),
            herdr_session,
            watch: Arc::new(Mutex::new(WatchState::default())),
        }
    }

    pub fn with_bin(bin: impl Into<String>) -> Self {
        Self {
            bin: bin.into(),
            ..Self::new()
        }
    }

    /// Make sure a session that just flowed through here has a wait loop
    /// backing it, if anyone is watching. Cheap to call from every method
    /// that receives a `SessionRef`: `start()` catches a session the moment
    /// it exists, and `status()` catches the rest, including one adopted from
    /// a store row on restart that never passed through `start()` here at
    /// all.
    fn note_session(&self, session: &SessionRef) {
        // A shell-mode session has no agent for herdr to detect, so `agent
        // wait` can never see it settle -- it fails `agent_not_found`
        // immediately and forever. There is nothing to watch; the poll
        // already covers it exactly as it does today.
        if session.meta.get("mode").map(String::as_str) != Some("agent") {
            return;
        }
        let pane = Self::pane_of(session).to_string();
        let tx = match self.watch.lock() {
            Ok(mut w) => match w.tx.clone() {
                Some(tx) if w.watching.insert(pane.clone()) => tx,
                _ => return,
            },
            Err(_) => return,
        };
        let this = self.clone();
        let session = session.clone();
        tokio::spawn(async move { this.watch_loop(session, tx).await });
    }

    fn forget_session(&self, pane: &str) {
        if let Ok(mut w) = self.watch.lock() {
            w.watching.remove(pane);
        }
    }

    /// One session's half of the push side: block on `herdr agent wait` for
    /// any state other than the one Factory currently believes the agent is
    /// in, then -- whatever it answered -- ask `status()` what is true right
    /// now. That closes the race named in the adapter's header: between a
    /// wait returning and the next one starting, a flip could otherwise go
    /// unseen, so what is published is always a fresh read rather than the
    /// state that happened to be waited for.
    ///
    /// A bounded `--timeout` is the safety net under that: it rearms the wait
    /// periodically even if herdr never reports a matching transition, so a
    /// child process cannot outlive its session forever. On a timeout `wait`
    /// answers an error, which is treated exactly like any other -- the
    /// status re-read afterward decides whether anything is worth sending.
    async fn watch_loop(&self, session: SessionRef, tx: mpsc::Sender<RuntimeEvent>) {
        let pane = Self::pane_of(&session).to_string();
        let mut current = self.status(&session).await.unwrap_or(RuntimeStatus::Gone);
        if current == RuntimeStatus::Gone {
            let _ = tx
                .send(RuntimeEvent {
                    session,
                    kind: RuntimeEventKind::SessionGone,
                })
                .await;
            self.forget_session(&pane);
            return;
        }

        loop {
            let mut args = vec![s("agent"), s("wait"), pane.clone()];
            for state in until_args(current) {
                args.push(s("--until"));
                args.push(s(state));
            }
            args.push(s("--timeout"));
            args.push(WAIT_TIMEOUT.as_millis().to_string());

            let waited = self.run(&args).await;
            let status = self.status(&session).await.unwrap_or(RuntimeStatus::Gone);

            // A real match (`waited` succeeded) is, by construction, a state
            // other than `current` -- always worth sending, even in the rare
            // case where it has already moved on again by the time of the
            // re-read above. A timed-out or otherwise failed wait proves
            // nothing changed, so only send if the fresh read disagrees with
            // what we already believed.
            if waited.is_ok() || status != current {
                let kind = if status == RuntimeStatus::Gone {
                    RuntimeEventKind::SessionGone
                } else {
                    RuntimeEventKind::StatusChanged(status)
                };
                if tx
                    .send(RuntimeEvent {
                        session: session.clone(),
                        kind,
                    })
                    .await
                    .is_err()
                {
                    // Nobody reading any more -- the listener side went away,
                    // not this session.
                    self.forget_session(&pane);
                    return;
                }
            }
            if status == RuntimeStatus::Gone {
                self.forget_session(&pane);
                return;
            }
            current = status;
            // A failure that was not the timeout above -- an internal
            // hiccup, or a target herdr does not recognise as an agent yet --
            // is not a reason to spin. Give it a moment before asking again
            // rather than hammering a call that may keep failing the same
            // way.
            if waited.is_err() {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    }

    async fn run(&self, args: &[String]) -> Result<Value> {
        let output = Command::new(&self.bin)
            .args(args)
            // `agent wait` can block for up to `WAIT_TIMEOUT`; if the future
            // driving it is ever dropped before that -- the daemon shutting
            // down while a watch loop is mid-wait, in particular -- this is
            // what stops the child outliving it as an orphan against
            // whatever herdr server the daemon was talking to.
            .kill_on_drop(true)
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
            .kill_on_drop(true)
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

    /// An agent herdr already knows by this name, if there is one.
    async fn adopt_named(&self, name: &str) -> Option<SessionRef> {
        let got = self.run(&[s("agent"), s("get"), s(name)]).await.ok()?;
        let agent = got.get("agent").unwrap_or(&got);
        let pane = agent.get("pane_id").and_then(Value::as_str)?.to_string();
        let workspace = agent
            .get("workspace_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Some(SessionRef {
            runtime: ADAPTER.into(),
            handle: pane.clone(),
            meta: BTreeMap::from([
                ("workspace_id".to_string(), workspace),
                ("pane_id".to_string(), pane),
                ("mode".to_string(), "agent".to_string()),
                ("agent_name".to_string(), name.to_string()),
                ("adopted".to_string(), "true".to_string()),
            ]),
        })
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

/// Drop the colour escapes and keep the characters. Only needed to measure a
/// line; the frame itself is handed on with its escapes intact.
fn strip_sgr(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    out
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
        // A standing agent's name is stable, so an agent already carrying it is
        // ours -- left behind by a daemon that stopped, or by a database that
        // was rebuilt under it. Adopt it instead of failing with
        // `agent_name_taken` and leaving the real session orphaned.
        if matches!(req.launch.kind, LaunchKind::Named(_)) {
            if let Some(session) = self.adopt_named(&req.name).await {
                tracing::info!(name = %req.name, "adopting an agent that was already running");
                self.note_session(&session);
                return Ok(session);
            }
        }

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

        let session = SessionRef {
            runtime: ADAPTER.into(),
            handle: pane,
            meta,
        };
        self.note_session(&session);
        Ok(session)
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

    /// Literal bytes into the pane. herdr passes them through untouched -- an
    /// `ESC [ A` arrives at the agent as an arrow key and `\x03` as Ctrl-C --
    /// which is what makes a real terminal in the browser possible at all.
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
        self.note_session(session);
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
            // herdr says `done` when an agent has finished its turn and is
            // sitting there. For occupancy that is the same as idle: the bay
            // is free. What the agent *achieved* comes from its callback, and
            // never from here.
            "done" => RuntimeStatus::Idle,
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

    async fn screen(&self, session: &SessionRef) -> Result<Option<Screen>> {
        let pane = Self::pane_of(session);
        // `visible` is the viewport -- the grid a person attached to this pane
        // would be looking at. `recent`, which `read` uses, is scrollback and
        // has no geometry to speak of.
        let frame = self
            .run_text(&[
                s("pane"),
                s("read"),
                s(pane),
                s("--source"),
                s("visible"),
                s("--format"),
                s("ansi"),
            ])
            .await?;

        // The grid's size is the pane's, not the browser window's: herdr owns
        // the layout, and a viewer that guesses wrong wraps every line.
        let (mut cols, mut rows) = (0u16, 0u16);
        if let Ok(v) = self
            .run(&[s("pane"), s("layout"), s("--pane"), s(pane)])
            .await
        {
            let rect = v
                .get("layout")
                .and_then(|l| l.get("panes"))
                .and_then(Value::as_array)
                .and_then(|panes| {
                    panes
                        .iter()
                        .find(|p| p.get("pane_id").and_then(Value::as_str) == Some(pane))
                })
                .and_then(|p| p.get("rect"));
            if let Some(rect) = rect {
                cols = rect.get("width").and_then(Value::as_u64).unwrap_or(0) as u16;
                rows = rect.get("height").and_then(Value::as_u64).unwrap_or(0) as u16;
            }
        }
        // A pane whose geometry we could not read still has a frame worth
        // showing; the widest line is a lower bound good enough to lay it out.
        if cols == 0 {
            cols = frame
                .lines()
                .map(|l| strip_sgr(l).chars().count())
                .max()
                .unwrap_or(80) as u16;
        }
        if rows == 0 {
            rows = frame.lines().count().max(1) as u16;
        }
        Ok(Some(Screen { cols, rows, frame }))
    }

    async fn watch(&self) -> Result<Option<RuntimeEventStream>> {
        let (tx, rx) = mpsc::channel(64);
        match self.watch.lock() {
            Ok(mut w) => w.tx = Some(tx),
            // A poisoned lock is not a reason to fail the daemon over a
            // backchannel; the poll still covers everything without it.
            Err(_) => return Ok(None),
        }
        Ok(Some(rx))
    }

    async fn stop(&self, session: &SessionRef) -> Result<()> {
        self.forget_session(Self::pane_of(session));
        if let Some(ws) = session.meta.get("workspace_id").filter(|w| !w.is_empty()) {
            self.run(&[s("workspace"), s("close"), ws.clone()]).await?;
        } else {
            self.run(&[s("pane"), s("close"), s(Self::pane_of(session))])
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! `until_args` is the one piece of branching logic in the push side
    //! that a stub runtime cannot exercise -- everything else there talks
    //! straight to the `herdr` binary. Pinned here instead: never include the
    //! state a wait is built from, or it is satisfied on entry instead of
    //! blocking for a real transition.

    use super::*;

    #[test]
    fn a_wait_never_includes_the_state_it_is_built_from() {
        for current in [
            RuntimeStatus::Idle,
            RuntimeStatus::Working,
            RuntimeStatus::Blocked,
            RuntimeStatus::Starting,
            RuntimeStatus::Unknown,
        ] {
            let until = until_args(current);
            assert!(!until.is_empty(), "{current:?} names nothing to wait for");
            let raw = match current {
                RuntimeStatus::Working => "working",
                RuntimeStatus::Blocked => "blocked",
                // `Idle` folds two raw herdr states (`idle` and `done`); both
                // must be excluded, since the folded value cannot say which
                // one is actually current.
                _ => continue,
            };
            assert!(
                !until.contains(&raw),
                "{current:?} must not wait for its own state ({raw})"
            );
        }
    }

    #[test]
    fn idle_excludes_both_raw_states_that_fold_into_it() {
        let until = until_args(RuntimeStatus::Idle);
        assert!(!until.contains(&"idle"));
        assert!(!until.contains(&"done"));
    }
}
