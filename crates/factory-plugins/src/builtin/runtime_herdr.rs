//! The only runtime there is for now: herdr, driven through its CLI. The CLI
//! answers in JSON on stdout, so nothing here has to speak the socket protocol.
//!
//! One herdr workspace per Factory scope, not per session: `start()` resolves
//! the scope's workspace by matching `label == scope` in `workspace list`
//! (stateless, so it survives a daemon restart, and if a person already made
//! a workspace by hand with that label, Factory joins it on purpose) and
//! creates one if none exists yet. Each session -- a standing agent or a task
//! run -- gets its own tab inside that workspace, opened with `herdr tab
//! create --workspace <id>` and closed on `stop()` with `herdr tab close`,
//! never `workspace close`: the workspace is shared by the whole scope, so
//! closing it out from under a sibling agent would take it down too. Closing
//! a scope's *last* tab makes herdr drop the now-empty workspace on its own,
//! though, so a tab close races the same resolve-or-create step a `start()`
//! elsewhere might be mid-way through -- both are serialized behind
//! `workspace_lock`, re-reading `workspace list` after acquiring it, so two
//! workspaces for one scope are never created and a `tab create` never lands
//! on a workspace id that closing just made stale.
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
    AgentRuntime, RuntimeConnectionDiagnostic, RuntimeConnectionState, RuntimeEvent,
    RuntimeEventKind, RuntimeEventStream, RuntimePeer, RuntimeStatus, Screen, StartRequest,
    StatusReport, StatusSource,
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
/// A diagnostic is polled while its view is open. It must not leave the page
/// spinning forever if a broken client never answers.
const DIAGNOSTIC_TIMEOUT: Duration = Duration::from_secs(5);

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
    /// Serializes the resolve-or-create-workspace step in `start()` against
    /// each other, and against `close_tab_or_pane` -- closing a scope's last
    /// tab drops its workspace, so a `start()` mid-resolve elsewhere must
    /// never see that workspace as still there right before `tab create`
    /// fails against it. Held only across `workspace list` and whichever of
    /// `workspace create`/`tab create`/`tab close`/`pane close` follows it --
    /// never across `agent start`, which can block for up to `start_timeout`
    /// -- so concurrent `start()`s in one scope (the normal case when
    /// reconcile autostarts every agent in it back to back) never race into
    /// creating the scope's workspace twice. `close_tab_or_pane` is never
    /// called while a caller already holds this lock -- `resolve_pane`
    /// itself never calls it -- so this can never deadlock; keep it that way.
    workspace_lock: Arc<tokio::sync::Mutex<()>>,
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
        Self::with_bin(bin)
    }

    pub fn with_bin(bin: impl Into<String>) -> Self {
        let bin = bin.into();
        let herdr_session = detect_session(&bin);
        Self {
            bin,
            start_timeout: Duration::from_secs(60),
            herdr_session,
            watch: Arc::new(Mutex::new(WatchState::default())),
            workspace_lock: Arc::new(tokio::sync::Mutex::new(())),
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
        let tab = agent
            .get("tab_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Some(SessionRef {
            runtime: ADAPTER.into(),
            handle: pane.clone(),
            meta: BTreeMap::from([
                ("workspace_id".to_string(), workspace),
                ("tab_id".to_string(), tab),
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

    /// Resolve or create this session's pane inside its scope's workspace,
    /// returning `(pane_id, workspace_id, tab_id)`. Locked so two `start()`s
    /// racing in the same scope -- the normal shape of a reconcile autostart
    /// -- never both decide the workspace is missing and each create one.
    ///
    /// `req.scope` empty means an old caller or plugin that predates a
    /// scope's workspace: fall back to a workspace of this session's own,
    /// labelled by `req.label`, which is exactly what every session used to
    /// get.
    async fn resolve_pane(&self, req: &StartRequest) -> Result<(String, String, String)> {
        let _guard = self.workspace_lock.lock().await;

        if req.scope.is_empty() {
            return self.create_workspace(&req.label, req).await;
        }

        if let Some(workspace) = self.find_workspace(&req.scope).await? {
            return self.create_tab(&workspace, req).await;
        }

        let (pane, workspace, tab) = self.create_workspace(&req.scope, req).await?;
        // `workspace create` always names the root tab "1"; give it this
        // session's own label instead, same as any other tab in the scope.
        // Best-effort: the workspace and pane are already live and every
        // lookup resolves by *workspace* label, never a tab's, so a failed
        // rename is a cosmetic defect, not a reason to fail the whole start
        // and leak what was just created.
        let _ = self
            .run(&[s("tab"), s("rename"), tab.clone(), req.label.clone()])
            .await;
        Ok((pane, workspace, tab))
    }

    /// The scope's workspace, if `workspace list` already has one labelled
    /// with it. More than one match is possible -- two people racing to
    /// create a workspace by hand, say -- so the lowest `number` wins,
    /// deterministically, rather than whichever `workspace list` happened to
    /// return first.
    async fn find_workspace(&self, scope: &str) -> Result<Option<String>> {
        let listed = self.run(&[s("workspace"), s("list")]).await?;
        let workspaces = listed
            .get("workspaces")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut matches: Vec<(u64, String)> = workspaces
            .iter()
            .filter(|w| w.get("label").and_then(Value::as_str) == Some(scope))
            .filter_map(|w| {
                let id = w.get("workspace_id").and_then(Value::as_str)?.to_string();
                let number = w.get("number").and_then(Value::as_u64).unwrap_or(u64::MAX);
                Some((number, id))
            })
            .collect();
        matches.sort_by_key(|(number, _)| *number);
        Ok(matches.into_iter().next().map(|(_, id)| id))
    }

    /// `herdr workspace create`, returning `(pane_id, workspace_id, tab_id)`.
    async fn create_workspace(&self, label: &str, req: &StartRequest) -> Result<(String, String, String)> {
        let mut args = vec![
            s("workspace"),
            s("create"),
            s("--cwd"),
            req.cwd.display().to_string(),
            s("--label"),
            label.to_string(),
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
        let tab = created
            .pointer("/root_pane/tab_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok((pane, workspace, tab))
    }

    /// `herdr tab create` inside an already-resolved workspace, returning
    /// `(pane_id, workspace_id, tab_id)`.
    async fn create_tab(&self, workspace: &str, req: &StartRequest) -> Result<(String, String, String)> {
        let mut args = vec![
            s("tab"),
            s("create"),
            s("--workspace"),
            workspace.to_string(),
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
            .ok_or_else(|| FactoryError::adapter(ADAPTER, "tab create returned no root pane id"))?
            .to_string();
        let tab = created
            .pointer("/root_pane/tab_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok((pane, workspace.to_string(), tab))
    }

    /// Close this session's tab, or -- for a row persisted before tabs were
    /// tracked, which has no `tab_id` -- its pane directly. That cascade also
    /// closes a legacy one-pane-per-workspace session's now-empty workspace
    /// by itself, herdr's own doing, not a `workspace close` call here. Never
    /// closes the workspace directly: it is shared by the rest of the scope.
    ///
    /// Guarded by `workspace_lock`: closing a scope's last tab is exactly the
    /// same herdr-side event as a `start()` elsewhere finding no workspace to
    /// resolve, so the two must never interleave -- otherwise a `start()`
    /// that just read the workspace as present could `tab create` against an
    /// id this call made stale a moment later. Never call this while already
    /// holding `workspace_lock` (nothing here does; `resolve_pane` never
    /// calls this method), or the lock would deadlock against itself.
    async fn close_tab_or_pane(&self, tab: &str, pane: &str) -> Result<()> {
        let _guard = self.workspace_lock.lock().await;
        if !tab.is_empty() {
            self.run(&[s("tab"), s("close"), tab.to_string()]).await?;
        } else {
            self.run(&[s("pane"), s("close"), pane.to_string()]).await?;
        }
        Ok(())
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

fn peer(value: &Value, side: &str) -> Result<RuntimePeer> {
    let value = value.get(side).and_then(Value::as_object).ok_or_else(|| {
        FactoryError::adapter(ADAPTER, format!("Herdr status omitted its {side} details"))
    })?;
    let version = value
        .get("version")
        .and_then(Value::as_str)
        .filter(|version| !version.is_empty())
        .ok_or_else(|| {
            FactoryError::adapter(ADAPTER, format!("Herdr status omitted the {side} version"))
        })?;
    let protocol = value
        .get("protocol")
        .and_then(Value::as_u64)
        .and_then(|protocol| u32::try_from(protocol).ok())
        .ok_or_else(|| {
            FactoryError::adapter(ADAPTER, format!("Herdr status omitted the {side} protocol"))
        })?;
    Ok(RuntimePeer {
        version: version.to_string(),
        protocol,
    })
}

/// Translate the Herdr CLI's vocabulary at the adapter boundary. Nothing
/// outside this file should know which object owns `restart_needed`, or that
/// capabilities are a map of flags rather than a list.
fn connection_from_status(value: &Value) -> Result<RuntimeConnectionDiagnostic> {
    let client = peer(value, "client")?;
    let server_value = value
        .get("server")
        .and_then(Value::as_object)
        .ok_or_else(|| FactoryError::adapter(ADAPTER, "Herdr status omitted its server details"))?;
    let running = server_value
        .get("running")
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            FactoryError::adapter(
                ADAPTER,
                "Herdr status omitted whether its server is running",
            )
        })?;
    let server = if running {
        Some(peer(value, "server")?)
    } else {
        None
    };
    let compatible = server_value.get("compatible").and_then(Value::as_bool);
    let restart_needed = server_value
        .get("restart_needed")
        .and_then(Value::as_bool)
        .or_else(|| {
            value
                .get("update")
                .and_then(|update| update.get("restart_needed"))
                .and_then(Value::as_bool)
        });
    let state = if !running {
        RuntimeConnectionState::Stopped
    } else if compatible == Some(false) {
        RuntimeConnectionState::Incompatible
    } else if restart_needed == Some(true) {
        RuntimeConnectionState::Degraded
    } else {
        RuntimeConnectionState::Healthy
    };
    let capabilities = server_value
        .get("capabilities")
        .and_then(Value::as_object)
        .map(|values| {
            values
                .iter()
                .filter(|(_, enabled)| enabled.as_bool() == Some(true))
                .map(|(name, _)| name.clone())
                .collect()
        })
        .unwrap_or_default();

    Ok(RuntimeConnectionDiagnostic {
        state,
        session: server_value
            .get("session")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/client/session").and_then(Value::as_str))
            .map(str::to_string),
        endpoint: server_value
            .get("socket")
            .and_then(Value::as_str)
            .map(str::to_string),
        client: Some(client),
        server,
        compatible,
        restart_needed,
        capabilities,
        error: None,
    })
}

#[async_trait]
impl AgentRuntime for HerdrRuntime {
    fn name(&self) -> &str {
        ADAPTER
    }

    fn description(&self) -> String {
        "one herdr workspace per scope, one tab per agent or run".into()
    }

    async fn connection_diagnostic(&self) -> Result<RuntimeConnectionDiagnostic> {
        let mut command = Command::new(&self.bin);
        command.args(["status", "--json"]).kill_on_drop(true);
        let output = match tokio::time::timeout(DIAGNOSTIC_TIMEOUT, command.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => {
                return Ok(RuntimeConnectionDiagnostic::unreachable(format!(
                    "could not run the configured Herdr client: {error}"
                )))
            }
            Err(_) => {
                return Ok(RuntimeConnectionDiagnostic::unreachable(
                    "the configured Herdr client did not report status within 5 seconds",
                ))
            }
        };
        if !output.status.success() {
            let code = output
                .status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "signal".into());
            return Ok(RuntimeConnectionDiagnostic::unreachable(format!(
                "the configured Herdr client could not report status (exit {code})"
            )));
        }

        // Do not include stdout in the error: the connection endpoint is an
        // explicitly modelled field, while arbitrary command output is not a
        // safe API response.
        let value: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
            FactoryError::adapter(
                ADAPTER,
                format!("could not read Herdr status JSON: {error}"),
            )
        })?;
        let mut diagnostic = connection_from_status(&value)?;
        if diagnostic.session.is_none() {
            diagnostic.session.clone_from(&self.herdr_session);
        }
        Ok(diagnostic)
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

        let (pane, workspace, tab) = self.resolve_pane(req).await?;

        let mut meta = BTreeMap::from([
            ("workspace_id".to_string(), workspace.clone()),
            ("tab_id".to_string(), tab.clone()),
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
                    // If the agent will not come up, close this session's tab
                    // rather than leaving an orphan pane behind -- never the
                    // workspace, which the rest of the scope may already be
                    // using.
                    let _ = self.close_tab_or_pane(&tab, &pane).await;
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
                        let _ = self.close_tab_or_pane(&tab, &pane).await;
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

    /// `status`, plus whether herdr is reporting a hook's word or guessing
    /// from the screen. Worth asking about for `blocked` and, since issue
    /// #62, `idle` too: `pane get` already answers every other status for
    /// free, and `agent explain` is a second subprocess herdr has to run --
    /// paying for it every five seconds on every active run, for a question
    /// that only these two statuses need answered, is not a trade worth
    /// making. `idle` joined `blocked` here because the daemon can only fail
    /// a run whose turn ended without a report the moment the *harness*
    /// says so (see `AGENTS.md` and `occupancy::turn_ended_action`) -- a
    /// `pane get` that merely looks idle is exactly the screen guess that
    /// rule forbids acting on.
    async fn status_report(&self, session: &SessionRef) -> Result<StatusReport> {
        let status = self.status(session).await?;
        let ask_why = match status {
            RuntimeStatus::Blocked => true,
            // A shell-mode pane (`LaunchKind::Command`, see `start()`) never
            // has an agent on it for herdr to explain -- there is nothing
            // for `agent explain` to say about it but an error. Restricting
            // the new `idle` case to a pane herdr actually put an agent on
            // costs nothing when that guess is right, and avoids a doomed
            // subprocess on every tick of every shell-agent run if it is
            // ever wrong.
            RuntimeStatus::Idle => session.meta.get("mode").map(String::as_str) == Some("agent"),
            _ => false,
        };
        if !ask_why {
            return Ok(StatusReport {
                status,
                source: StatusSource::Unknown,
            });
        }

        let pane = Self::pane_of(session);
        let source = match self
            .run(&[s("agent"), s("explain"), s(pane), s("--format"), s("json")])
            .await
        {
            Ok(explained) => {
                // `pane get`'s answer is enveloped as `{"id":..,"result":{..}}`;
                // `explain`'s is the bare object. `run` already unwraps either
                // shape (it hands back `result` when there is one, the whole
                // object otherwise), but look in both places explicitly rather
                // than lean on that alone -- the two commands are not guaranteed
                // to agree on their envelope forever.
                let field = |name: &str| -> Option<&Value> {
                    explained
                        .get(name)
                        .or_else(|| explained.get("result").and_then(|r| r.get(name)))
                };
                let skipped = field("screen_detection_skipped")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !skipped {
                    // `claude`, `codex` and `opencode` panes come back with a
                    // `matched_rule` from herdr's screen-region regexes. That
                    // is inference, however confident it looks.
                    StatusSource::Inferred
                } else if status == RuntimeStatus::Blocked {
                    // A `pi` pane looks like this: `screen_detection_skipped:
                    // true`, `screen_detection_skip_reason:
                    // "full_lifecycle_hook_authority"`, `evaluated_rules: []`.
                    // The harness told herdr; herdr is just the messenger.
                    // (Verified against a live `herdr agent explain --json`;
                    // an earlier version of this comment named the field
                    // `skip_reason`, which does not exist on the wire and
                    // cost issue #62's first pass its `idle` case entirely.)
                    StatusSource::Reported
                } else {
                    // `idle` asks for more proof than `blocked` does before
                    // trusting `skipped`. Ending a run outright is far less
                    // reversible than moving it into `Blocked` -- a wrongly
                    // `Reported` block only pauses a run for a human, a
                    // wrongly `Reported` idle kills it (see `AGENTS.md` and
                    // issue #62) -- and `screen_detection_skipped` alone
                    // does not distinguish "the harness told me" from herdr
                    // skipping detection for some other reason entirely (no
                    // agent on the pane yet, detection turned off, a pane
                    // that never became ready). Only the specific
                    // `screen_detection_skip_reason` above actually means a
                    // hook is speaking, so the `idle` path insists on it by
                    // name rather than trusting the bool on its own.
                    let reason = field("screen_detection_skip_reason")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if reason == "full_lifecycle_hook_authority" {
                        StatusSource::Reported
                    } else {
                        StatusSource::Inferred
                    }
                }
            }
            // `explain` failing tells us nothing either way about how the
            // status we already have came about. A guess is the safe
            // assumption -- never claim a report we could not actually read.
            Err(_) => StatusSource::Inferred,
        };
        Ok(StatusReport { status, source })
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
        let tab = session.meta.get("tab_id").cloned().unwrap_or_default();
        self.close_tab_or_pane(&tab, Self::pane_of(session)).await
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

    fn status(running: bool, compatible: bool, restart_needed: bool) -> Value {
        serde_json::json!({
            "client": {
                "version": "0.8.0",
                "protocol": 19,
                "session": "factory"
            },
            "server": {
                "status": if running { "running" } else { "stopped" },
                "running": running,
                "version": "0.8.1",
                "protocol": 19,
                "compatible": compatible,
                "socket": "/tmp/herdr/factory/herdr.sock",
                "session": "factory",
                "restart_needed": restart_needed,
                "capabilities": {
                    "live_handoff": true,
                    "future_flag": false
                }
            },
            "update": { "restart_needed": restart_needed }
        })
    }

    #[test]
    fn a_running_compatible_herdr_connection_is_translated_without_raw_json() {
        let got = connection_from_status(&status(true, true, false)).unwrap();
        assert_eq!(got.state, RuntimeConnectionState::Healthy);
        assert_eq!(got.session.as_deref(), Some("factory"));
        assert_eq!(
            got.endpoint.as_deref(),
            Some("/tmp/herdr/factory/herdr.sock")
        );
        assert_eq!(got.client.unwrap().version, "0.8.0");
        assert_eq!(got.server.unwrap().protocol, 19);
        assert_eq!(got.capabilities, vec!["live_handoff"]);
        assert_eq!(got.compatible, Some(true));
        assert_eq!(got.restart_needed, Some(false));
    }

    #[test]
    fn stopped_incompatible_and_restart_needed_are_distinct_states() {
        assert_eq!(
            connection_from_status(&status(false, true, false))
                .unwrap()
                .state,
            RuntimeConnectionState::Stopped
        );
        assert_eq!(
            connection_from_status(&status(true, false, false))
                .unwrap()
                .state,
            RuntimeConnectionState::Incompatible
        );
        assert_eq!(
            connection_from_status(&status(true, true, true))
                .unwrap()
                .state,
            RuntimeConnectionState::Degraded
        );
    }

    #[test]
    fn malformed_status_is_an_error_instead_of_a_healthy_guess() {
        let error = connection_from_status(&serde_json::json!({
            "client": { "version": "0.8.0", "protocol": 19 },
            "server": {}
        }))
        .unwrap_err()
        .to_string();
        assert!(error.contains("whether its server is running"), "{error}");
    }

    #[tokio::test]
    async fn a_missing_configured_client_is_unreachable_without_failing_the_probe() {
        let runtime = HerdrRuntime::with_bin("/definitely/no/such/herdr");
        let got = runtime.connection_diagnostic().await.unwrap();
        assert_eq!(got.state, RuntimeConnectionState::Unreachable);
        assert!(got
            .error
            .unwrap_or_default()
            .contains("configured Herdr client"));
    }

    /// A fake `herdr` binary that answers `status` (for `with_bin`'s own
    /// session-detection probe), `pane get` with the given `agent_status`,
    /// and `agent explain` with the given raw JSON -- and, for the
    /// `explain`-was-never-called test below, touches a marker file first
    /// so a test can prove that branch was never reached rather than only
    /// checking the source it would have produced. Returns the binary's
    /// path and the directory it lives in, which the caller owns and must
    /// clean up once the runtime built from it is done being used.
    #[cfg(unix)]
    fn fake_herdr_for_explain(agent_status: &str, explain_json: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "factory-herdr-status-report-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("explain-called");
        let bin = dir.join("fake-herdr");
        std::fs::write(
            &bin,
            format!(
                r#"#!/bin/sh
if [ "$1" = "status" ] && [ "$2" != "--json" ]; then
  printf 'socket: /tmp/factory-herdr-status-report-test/herdr.sock\n'
elif [ "$1" = "pane" ] && [ "$2" = "get" ]; then
  printf '%s\n' '{{"result":{{"pane":{{"agent_status":"{agent_status}"}}}}}}'
elif [ "$1" = "agent" ] && [ "$2" = "explain" ]; then
  : > '{marker}'
  printf '%s\n' '{explain_json}'
else
  exit 9
fi
"#,
                marker = marker.display(),
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&bin).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&bin, permissions).unwrap();
        (bin, dir)
    }

    #[cfg(unix)]
    fn agent_mode_session() -> SessionRef {
        SessionRef {
            runtime: ADAPTER.into(),
            handle: "pane1".into(),
            meta: BTreeMap::from([("mode".to_string(), "agent".to_string())]),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_full_lifecycle_hook_authority_idle_is_a_report_not_a_guess() {
        // Captured verbatim from a live `herdr --session factory agent
        // explain <pi-pane> --json` against a real `pi` pane, precisely so
        // this test fails if herdr's field name ever drifts again the way
        // it already did once: the first pass of this function read
        // `skip_reason`, which does not exist on the wire -- the real key
        // is `screen_detection_skip_reason` -- so the `idle` case never
        // fired for a single pane, hook authority included, and this test
        // still passed because its own fixture used the same wrong name.
        // Keep this fixture byte-for-byte what the binary actually said.
        let (bin, dir) = fake_herdr_for_explain(
            "idle",
            r#"{"agent":"pi","cached_remote_version":null,"evaluated_rules":[],"fallback_reason":null,"local_override_shadowing_remote":false,"manifest_source":null,"manifest_version":null,"matched_rule":null,"remote_update_error":null,"remote_update_status":null,"screen_detection_skip_reason":"full_lifecycle_hook_authority","screen_detection_skipped":true,"skip_state_update":false,"skipped_update_reason":null,"state":"idle","visible_blocker":false,"visible_idle":false,"visible_working":false,"warning":null}"#,
        );
        let runtime = HerdrRuntime::with_bin(bin.to_string_lossy());
        let report = runtime.status_report(&agent_mode_session()).await.unwrap();
        assert_eq!(report.status, RuntimeStatus::Idle);
        assert_eq!(
            report.source,
            StatusSource::Reported,
            "a pane with full lifecycle hook authority is the harness speaking, not herdr guessing"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_claude_panes_idle_is_a_guess_not_a_report() {
        // Trimmed from a live `herdr --session factory agent explain
        // <claude-pane> --json` against a real `claude-code` pane (the
        // evaluated_rules array is cut down from ~15 entries to the one
        // that actually matched; everything else -- crucially,
        // `screen_detection_skipped: false` and the absence of any
        // `screen_detection_skip_reason` key at all -- is what herdr
        // really said). A `claude` pane's idle is screen inference through
        // and through: herdr has no hook telling it the turn ended, only a
        // regex match on the prompt box. See issue #62's follow-up: this is
        // exactly why the incident's own run (a `claude-code` pane) is not
        // fixed by this file alone.
        let (bin, dir) = fake_herdr_for_explain(
            "idle",
            r#"{"agent":"claude","cached_remote_version":"2026.09.11.1","evaluated_rules":[{"id":"live_prompt_box","matched":true,"priority":950,"region":"prompt_box_body","state":"idle"}],"fallback_reason":null,"local_override_shadowing_remote":false,"manifest_source":"remote:/Users/factory/.local/state/herdr/agent-detection/remote/claude.toml","manifest_version":"2026.09.11.1","matched_rule":{"id":"live_prompt_box","priority":950,"region":"prompt_box_body","state":"idle"},"remote_update_error":null,"remote_update_status":"current","screen_detection_skipped":false,"skip_state_update":false,"skipped_update_reason":null,"state":"idle","visible_blocker":false,"visible_idle":true,"visible_working":false,"warning":null}"#,
        );
        let runtime = HerdrRuntime::with_bin(bin.to_string_lossy());
        let report = runtime.status_report(&agent_mode_session()).await.unwrap();
        assert_eq!(report.status, RuntimeStatus::Idle);
        assert_eq!(report.source, StatusSource::Inferred);
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_skipped_idle_with_some_other_reason_is_still_a_guess() {
        // `screen_detection_skipped: true` alone is not proof of a hook --
        // herdr sets it for reasons other than "the harness told me" too.
        // Only `screen_detection_skip_reason:
        // "full_lifecycle_hook_authority"` may promote `idle` to
        // `Reported`; anything else stays a guess, because ending a run on
        // a false positive here is a run lost, not merely a run paused
        // (see `AGENTS.md` and issue #62).
        let (bin, dir) = fake_herdr_for_explain(
            "idle",
            r#"{"screen_detection_skipped":true,"screen_detection_skip_reason":"no_agent_on_pane"}"#,
        );
        let runtime = HerdrRuntime::with_bin(bin.to_string_lossy());
        let report = runtime.status_report(&agent_mode_session()).await.unwrap();
        assert_eq!(report.status, RuntimeStatus::Idle);
        assert_eq!(report.source, StatusSource::Inferred);
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_shell_mode_panes_idle_never_pays_for_explain() {
        // A shell pane has no agent for herdr to explain -- `start()` never
        // runs `agent start` for `LaunchKind::Command`. Paying for the extra
        // subprocess there would only ever buy a failure, so `status_report`
        // must not even try: proven here by the marker `agent explain`
        // would have written never appearing, not merely by the source it
        // would have produced.
        let (bin, dir) = fake_herdr_for_explain("idle", r#"{"screen_detection_skipped":true}"#);
        let runtime = HerdrRuntime::with_bin(bin.to_string_lossy());
        let session = SessionRef {
            runtime: ADAPTER.into(),
            handle: "pane1".into(),
            meta: BTreeMap::from([("mode".to_string(), "shell".to_string())]),
        };
        let report = runtime.status_report(&session).await.unwrap();
        assert_eq!(report.status, RuntimeStatus::Idle);
        assert_eq!(report.source, StatusSource::Unknown);
        assert!(
            !dir.join("explain-called").exists(),
            "a shell-mode pane must never trigger `agent explain`"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn diagnostics_use_the_configured_binary_and_its_session() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "factory-herdr-diagnostic-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("chosen-herdr");
        std::fs::write(
            &bin,
            r#"#!/bin/sh
if [ "$1" = status ] && [ "$2" = --json ]; then
  printf '%s\n' '{"client":{"version":"1.0","protocol":20,"session":"chosen"},"server":{"running":true,"version":"1.0","protocol":20,"compatible":true,"session":"chosen","socket":"/tmp/chosen/herdr.sock","restart_needed":false}}'
elif [ "$1" = status ]; then
  printf 'socket: /tmp/chosen/herdr.sock\n'
else
  exit 9
fi
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&bin).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&bin, permissions).unwrap();

        let runtime = HerdrRuntime::with_bin(bin.to_string_lossy());
        let got = runtime.connection_diagnostic().await.unwrap();
        assert_eq!(got.state, RuntimeConnectionState::Healthy);
        assert_eq!(got.session.as_deref(), Some("chosen"));
        assert_eq!(got.endpoint.as_deref(), Some("/tmp/chosen/herdr.sock"));

        std::fs::remove_dir_all(dir).ok();
    }
}
