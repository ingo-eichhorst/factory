//! The Claude Code adapter — Slice 10's second [`Adapter`] implementation,
//! running through the identical [`crate::contract::run_contract_suite`]
//! [`PiAdapter`] runs through, per backlog §10's "Pi and Claude Code pass the
//! same contract tests for start, send, observe, interrupt, stop, failed
//! start, and one-task-per-session behavior."
//!
//! # Observation comes from Irrlicht, not Herdr — and it is never authoritative
//!
//! Measured live on this machine on 2026-09-09 (backlog §10's resolution
//! note) and re-measured while writing this module: `herdr agent explain` on
//! a live Claude pane answers `rule: live_prompt_box
//! (region=prompt_box_body priority=950)`, `evidence: "❯\n"` — Herdr reads
//! Claude's *screen*, exactly the heuristic ADR 0017 built Pi's adapter to
//! avoid, and no `screen_detection_skip_reason` line appears at all. So
//! unlike Pi, Claude carries no Herdr lifecycle-hook authority to read.
//!
//! Irrlicht (ADR 0011) is the source instead. It parses Claude's own
//! transcript and infers state — `ready | working | error` — which is
//! reachable at `GET http://127.0.0.1:7837/api/v1/sessions` (see
//! [`IrrlichtAccess`]). ADR 0011's open item 1 originally found no join key
//! for Pi there; for `claude-code` the pane *is* present, nested as
//! `launcher.herdr_pane_id` alongside `launcher.herdr_socket_path`, and
//! [`ClaudeAdapter::observe`] joins on nothing else. **Working directory is
//! never a join key here, on purpose**: this machine has run three live
//! `claude-code` sessions sharing one `cwd` at once (`wE:p2`, `wE:p4`,
//! `wE:p5`, all rooted at this very repository), and this crate's own module
//! docs already name the reason — "a wrong observation is worse than none,
//! because it looks like an answer."
//!
//! That gives Irrlicht a real, working parser and a real join key, but not
//! authority. Every live `claude-code` session this adapter has ever
//! observed reports `"confidence":"medium"` and `"last_event":
//! "transcript_activity"` on the Irrlicht payload itself — state *inferred*
//! from a transcript, which is exactly what ADR 0017 distinguishes from
//! state a harness *reports about itself* through a lifecycle hook (Pi's
//! case). So [`ClaudeAdapter::observe`] returns [`Confidence::Degraded`],
//! never [`Confidence::Authoritative`], regardless of what Irrlicht's own
//! `"confidence"` field on the payload claims — that field is read nowhere
//! in this module, deliberately.
//!
//! This is not a shortfall to work around; it is the risk clause backlog §10
//! asked for, landing exactly where it was aimed.
//! `factory_recovery::evidence::may_promote_from_disconnected` requires
//! `Authoritative`, so a Claude Code session that reaches `disconnected` is
//! returned to service by a human, never by this adapter's own reading. That
//! module is not this crate's to touch, and this adapter does not try to
//! widen what it accepts.
//!
//! # Everything except `observe` goes through Herdr, exactly as [`PiAdapter`]'s does
//!
//! `start`, `send`, `interrupt`, `stop`, and `attach_command` all drive the
//! same [`HerdrAccess`] trait `PiAdapter` uses, reusing its pane-lifecycle
//! helpers (`parse_created_pane`, `agent_name_for`, `is_transient_pane_not_ready`,
//! `attach_argv`, `unavailable_observation`, `unreadable_agent_get`) rather
//! than re-deriving them — this crate has already paid three times for a
//! rule with two homes drifting; see this crate's own module docs and ADR
//! 0011/0017. `herdr agent start <NAME> --kind claude --pane <ID>` is
//! measured to accept `claude` as a `--kind` value (`herdr agent start
//! --help`), and Herdr's own vocabulary for what is running in a pane is
//! `claude`, not `claude-code` — the two adapters' names genuinely differ
//! between the two products, and [`CLAUDE_HERDR_KIND`] /
//! [`CLAUDE_CODE_IRRLICHT_ADAPTER`] name that split rather than leaving it
//! implicit.
//!
//! `PiAdapter` itself is not touched by this module — it is not this agent's
//! to restructure, and other crates depend on its shape.
//!
//! # `send`'s busy precheck reads Herdr, not Irrlicht — a deliberate exception
//!
//! [`Adapter::send`]'s contract forbids a false confirmation: sending to a
//! pane already `working` a turn must refuse, never risk matching the
//! *current* turn's completion (see this crate's module docs, fourth rule).
//! For Pi, that precheck and `observe()` read the same source (Herdr), so
//! reusing `observe()` inside `send()` is free. For Claude they must not
//! share a source: measured live, Irrlicht's `state` lags a real prompt
//! landing by one to several seconds (its own transcript-polling cadence),
//! while `herdr agent get`'s screen-detected `agent_status` flips `idle` →
//! `working` synchronously with the prompt call — the same measurement that
//! showed `herdr agent prompt <pane> <text> --wait --until working` itself
//! returning in well under a second once the screen changed. A precheck
//! built on Irrlicht could read a pane as `ready` while Herdr's own
//! `--wait --until working` matcher is already mid-turn — precisely the
//! false-confirmation hazard the precheck exists to stop. So `send`, a Herdr
//! action end to end, asks Herdr the same question `agent prompt` is about
//! to answer, via [`claude_busy_status`]. [`ClaudeAdapter::observe`] itself
//! still never touches Herdr at all — see
//! [`tests::observe_never_touches_herdr_agent_get_or_explain`].
//!
//! # Cost samples share `observe`'s Irrlicht lookup, and never read `total_tokens`
//!
//! [`ClaudeAdapter::cost_sample`] answers design §12.6's model identifier,
//! token counts, and duration from the same `metrics` object `observe`
//! ignores entirely — Herdr reports none of it (this crate's own module
//! docs: Irrlicht "remains the source of metrics ... Herdr does not
//! report"). It shares `observe`'s pane lookup through [`find_claude_agent`]
//! rather than re-deriving it, exactly the same reasoning this module's
//! second section already gives for reusing `PiAdapter`'s helpers.
//!
//! It reads only `metrics.cum_input_tokens` and `metrics.cum_output_tokens`,
//! never `metrics.total_tokens` — ADR 0021 decision 7, measured across all 8
//! live sessions carrying the three fields, with no exceptions:
//! `total_tokens == context_window × context_utilization_percentage / 100`.
//! It is context occupancy, not a token count, and it falls when a session
//! compacts; storing it in a field documented as "tokens this run used"
//! would be a number of the right type in the wrong field. `metrics: null`
//! — every `opencode` session, and two of four live Pi sessions when ADR
//! 0021 measured this — yields `Ok(None)`, the ordinary case rather than the
//! edge. See [`crate::CostSample`] for the full shape and
//! [`crate::CostSample::since`] for how a cumulative sample like this one
//! becomes one run's own figures.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

use crate::{
    Adapter, AdapterError, Confidence, CostSample, CostSource, HerdrAccess, Observation, PaneId,
    StartRequest, StartedSession, TaskSignal, agent_name_for, attach_argv,
    is_transient_pane_not_ready, parse_created_pane, unavailable_observation, unreadable_agent_get,
};

/// Herdr's own name for what runs in a Claude pane — the value `herdr agent
/// start --kind` accepts and `herdr agent get`/`explain`'s `agent` field
/// reports (measured: `herdr agent start --help` lists it among
/// `[pi, claude, codex, ...]`; a live pane answers `"agent":"claude"`).
const CLAUDE_HERDR_KIND: &str = "claude";

/// Irrlicht's own name for a Claude Code session — the `adapter` field on
/// `GET /api/v1/sessions` (measured 2026-09-09, backlog §10's resolution
/// note and this module's own re-measurement). Distinct from
/// [`CLAUDE_HERDR_KIND`] on purpose: the two products use different words for
/// the same harness.
const CLAUDE_CODE_IRRLICHT_ADAPTER: &str = "claude-code";

/// Irrlicht's documented state vocabulary for a `claude-code` session
/// (backlog §10's resolution note). Anything else is version skew — ADR
/// 0011's open item 2 says treat it as no observation, not a guess.
const KNOWN_IRRLICHT_STATES: [&str; 3] = ["ready", "working", "error"];

/// The one Irrlicht call this adapter needs, behind a trait the same way
/// [`HerdrAccess`] sits behind [`crate::PiAdapter`] — so tests supply
/// recorded payloads instead of requiring a running Irrlicht daemon.
pub trait IrrlichtAccess {
    /// Raw response body of `GET {base}/api/v1/sessions` — see this module's
    /// docs for the measured shape, and [`ClaudeAdapter::observe`] for what
    /// is actually read from it (`groups[].agents[].{adapter,state,
    /// session_id,transcript_path,launcher.herdr_pane_id}` — nothing else,
    /// and never `cwd`).
    fn sessions(&self) -> Result<String, AdapterError>;
}

/// Hits the real Irrlicht daemon over its documented loopback HTTP API (ADR
/// 0011) via `curl`, mirroring [`crate::HerdrCli`]'s own subprocess pattern
/// rather than adding an HTTP client dependency: this crate's `Cargo.toml` may
/// only add dependencies already declared in the workspace table, and none of
/// them is an HTTP client.
pub struct IrrlichtHttp {
    base_url: String,
}

impl IrrlichtHttp {
    /// Irrlicht's documented loopback address (ADR 0011).
    const DEFAULT_BASE_URL: &'static str = "http://127.0.0.1:7837";

    #[must_use]
    pub fn new() -> Self {
        Self {
            base_url: Self::DEFAULT_BASE_URL.to_string(),
        }
    }
}

impl Default for IrrlichtHttp {
    fn default() -> Self {
        Self::new()
    }
}

impl IrrlichtAccess for IrrlichtHttp {
    fn sessions(&self) -> Result<String, AdapterError> {
        let url = format!("{}/api/v1/sessions", self.base_url);
        let output = Command::new("curl")
            .args(["-sS", "-m", "5", &url])
            .output()
            .map_err(|source| AdapterError::RuntimeUnavailable {
                binary: PathBuf::from("curl"),
                help: "install curl (present on macOS by default) and ensure it is on PATH"
                    .to_string(),
                source,
            })?;

        if !output.status.success() {
            return Err(AdapterError::UnreadableOutput {
                command: format!("curl {url}"),
                detail: format!(
                    "exited with {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
                help: "confirm Irrlicht is running and reachable at 127.0.0.1:7837 \
                       (Irrlicht.app or irrlichd)"
                    .to_string(),
            });
        }

        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// The Claude Code adapter: Herdr for every action, Irrlicht for observation
/// — see this module's own docs for why the two are never conflated.
pub struct ClaudeAdapter<H: HerdrAccess, I: IrrlichtAccess> {
    herdr: H,
    irrlicht: I,
}

impl<H: HerdrAccess, I: IrrlichtAccess> ClaudeAdapter<H, I> {
    #[must_use]
    pub fn new(herdr: H, irrlicht: I) -> Self {
        Self { herdr, irrlicht }
    }

    /// Mirrors [`crate::PiAdapter`]'s own `start_pi_with_retry` — same
    /// measured `agent_pane_busy` race (that method's doc comment), same
    /// retry policy, reusing the shared [`is_transient_pane_not_ready`]
    /// classifier so "which errors are transient" has one home rather than
    /// two copies of the same rule. Not the same *method*: Pi's is hardcoded
    /// to `kind: "pi"` plus the Herdr lifecycle-hook extension argument
    /// Claude does not use, and `PiAdapter` is not this module's to
    /// restructure into something both harnesses share.
    fn start_claude_with_retry(&self, name: &str, pane: &PaneId) -> Result<String, AdapterError> {
        const ATTEMPTS: u32 = 5;
        const RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(200);

        let mut last_err = None;
        for attempt in 0..ATTEMPTS {
            match self.herdr.agent_start(name, CLAUDE_HERDR_KIND, pane, &[]) {
                Ok(raw) => return Ok(raw),
                Err(err) => {
                    let is_last_attempt = attempt + 1 == ATTEMPTS;
                    if is_last_attempt || !is_transient_pane_not_ready(&err) {
                        return Err(err);
                    }
                    last_err = Some(err);
                    std::thread::sleep(RETRY_DELAY);
                }
            }
        }
        // Unreachable: the loop above always returns on its last iteration,
        // but `last_err` still has to be a real value for the type checker —
        // see `PiAdapter::start_pi_with_retry`'s identical comment.
        Err(last_err.expect("the loop always returns before exhausting every attempt"))
    }
}

/// Searches every group's *top-level* agents — never `children` — for the
/// one entry whose `launcher.herdr_pane_id` equals `pane`. Design's rule:
/// the pane is the only join key, and this function does not read `cwd` at
/// all. Claude Code subagents (recorded under `children`) carry no
/// `launcher` object whatsoever (measured live: a running session's
/// subagents report `session_id`, `parent_session_id`, `state`, but no
/// `launcher`), so they could never match a pane id even walked into — but
/// this function does not walk into them, so that is not left to the
/// accident of an absent field.
fn find_by_pane<'a>(groups: &'a [Value], pane: &str) -> Option<&'a Value> {
    groups.iter().find_map(|group| {
        group
            .get("agents")
            .and_then(Value::as_array)
            .and_then(|agents| {
                agents.iter().find(|agent| {
                    agent
                        .get("launcher")
                        .and_then(|launcher| launcher.get("herdr_pane_id"))
                        .and_then(Value::as_str)
                        == Some(pane)
                })
            })
    })
}

fn unreadable_sessions(detail: impl Into<String>) -> AdapterError {
    AdapterError::UnreadableOutput {
        command: "GET /api/v1/sessions".to_string(),
        detail: detail.into(),
        help: "confirm Irrlicht is running the version this adapter was built against; the \
               payload shape may have changed"
            .to_string(),
    }
}

/// `GET /api/v1/sessions` → parsed `groups` → the one agent whose
/// `launcher.herdr_pane_id` matches `pane` and whose `adapter` is
/// `claude-code`, or `None` when Irrlicht has no usable answer about this
/// exact pane: unreachable, no session at that pane at all, or a session at
/// that pane belonging to a different harness. `Err` only for schema drift —
/// malformed JSON or a missing `groups` array.
///
/// [`ClaudeAdapter::observe`] and [`ClaudeAdapter::cost_sample`] both need
/// exactly this lookup, and this crate has already paid three times for a
/// rule with two homes drifting (this module's own docs, ADR 0011, ADR
/// 0017) — so it has one home here instead of being re-derived in each
/// method. Returns an owned [`Value`] rather than a borrow, because the
/// parsed payload it is found in does not outlive this function.
fn find_claude_agent<I: IrrlichtAccess>(
    irrlicht: &I,
    pane: &PaneId,
) -> Result<Option<Value>, AdapterError> {
    // "Irrlicht could not answer at all" — stopped, unreachable — is not an
    // adapter failure, exactly as a stopped Herdr is not one for Pi (ADR
    // 0011 decision 3: Factory degrades to manual confirmation without it).
    let raw = match irrlicht.sessions() {
        Ok(raw) => raw,
        Err(_) => return Ok(None),
    };

    let payload: Value =
        serde_json::from_str(&raw).map_err(|source| unreadable_sessions(source.to_string()))?;
    let groups = payload
        .get("groups")
        .and_then(Value::as_array)
        .ok_or_else(|| unreadable_sessions("missing `groups` array"))?;

    let Some(agent) = find_by_pane(groups, &pane.0) else {
        // Irrlicht answered, but has no session for this pane at all.
        return Ok(None);
    };

    let adapter_kind = agent.get("adapter").and_then(Value::as_str).unwrap_or("");
    if adapter_kind != CLAUDE_CODE_IRRLICHT_ADAPTER {
        // Irrlicht says this exact pane belongs to a *different* harness
        // than the one Factory recorded for this session — "no usable
        // answer about the Claude session we asked about" (this crate's
        // module docs: a wrong observation is worse than none), not
        // `AdapterError::WrongHarness` — that variant's own message is
        // written for Pi's vocabulary and is not this module's to reword.
        return Ok(None);
    }

    Ok(Some(agent.clone()))
}

/// Reads `result.agent.agent_status` from `herdr agent get <pane>`'s JSON —
/// the one field [`ClaudeAdapter::send`]'s busy precheck needs. Standalone
/// rather than folded into `observe`: `observe` for Claude never touches
/// Herdr at all (see this module's docs), so there is no shared parsing path
/// to reuse from it, only [`unreadable_agent_get`]'s error shape, which this
/// does reuse.
fn claude_busy_status(pane: &PaneId, raw: &str) -> Result<String, AdapterError> {
    let payload: Value = serde_json::from_str(raw)
        .map_err(|source| unreadable_agent_get(pane, source.to_string()))?;
    let agent = payload
        .get("result")
        .and_then(|result| result.get("agent"))
        .ok_or_else(|| unreadable_agent_get(pane, "missing `result.agent`"))?;
    Ok(agent
        .get("agent_status")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string())
}

impl<H: HerdrAccess, I: IrrlichtAccess> Adapter for ClaudeAdapter<H, I> {
    fn start(&self, req: &StartRequest) -> Result<StartedSession, AdapterError> {
        let created = self.herdr.workspace_create(req.workspace.as_path())?;
        let pane = parse_created_pane(&created)?;

        let name = agent_name_for(req.session_id);
        if let Err(err) = self.start_claude_with_retry(&name, &pane) {
            // Nothing else releases a pane whose agent never started — see
            // `PiAdapter::start`'s identical cleanup and identical reasoning
            // for not reporting this close's own failure.
            let _ = self.herdr.pane_close(&pane);
            return Err(err);
        }

        // `req.generated_context` is unused here for the same reason
        // `PiAdapter::start` does not send it over the wire: Claude Code
        // loads `CLAUDE.md` (project- and user-level) natively at session
        // start — this very process is a live demonstration of exactly that,
        // running under a project `CLAUDE.md` and a user one — so writing
        // this text into the pane over the wire would duplicate context
        // Claude already loaded and, sent as a prompt, would spend Claude's
        // first turn on context instead of a task.
        //
        // Reuse `observe`'s own Irrlicht lookup and confidence rule rather
        // than re-deriving it here, exactly as `PiAdapter::start` reuses its
        // own `observe`. A freshly started session may briefly return
        // `Confidence::Unavailable` rather than `Degraded`: Irrlicht indexes
        // a new transcript on its own polling cadence (measured: one to a
        // few seconds), and this adapter does not retry `observe` here to
        // paper over that lag — see this crate's `AdapterError` docs and
        // `StartedSession::confidence`'s own doc comment for why a missing
        // reading at `start` time is reported, never guessed.
        let observed = self.observe(&pane)?;
        Ok(StartedSession {
            pane,
            harness_session_id: observed.harness_session_id,
            confidence: observed.confidence,
        })
    }

    fn send(&self, pane: &PaneId, task_id: uuid::Uuid, prompt: &str) -> Result<(), AdapterError> {
        // See this module's docs: the busy precheck deliberately reads
        // Herdr, not `self.observe` (Irrlicht) — Irrlicht's lag would
        // reintroduce the exact false-confirmation hazard this precheck
        // exists to stop. A Herdr answer we cannot get at all (pane
        // unknown, Herdr down) degrades to "unknown" rather than failing
        // `send` outright — `agent_prompt` below will fail on its own if the
        // pane genuinely cannot take a prompt, mirroring
        // `PiAdapter::send`'s identical leniency at this same precheck.
        let harness_state = match self.herdr.agent_get(pane) {
            Ok(raw) => claude_busy_status(pane, &raw)?,
            Err(_) => "unknown".to_string(),
        };
        if harness_state == "working" {
            return Err(AdapterError::SessionBusy {
                pane: pane.0.clone(),
                harness_state,
                help: format!(
                    "task {task_id} was not submitted; wait for the current turn to finish, \
                     or send an explicit interrupt, before sending another task to this session"
                ),
            });
        }

        // The task id is NOT prefixed here. `factory_task::deliver` renders
        // it into the prompt (design §5: "each prompt includes its task UUID
        // so that work can be correlated with its database record"), and it
        // must, because station 7's `OperatorPromptWriter` — a human pasting
        // the prompt by hand — never passes through an adapter at all. An
        // adapter that prefixed as well produced `[task <id>] [task <id>]` on
        // a real Pi pane, which is how this was found.
        //
        // `task_id` stays in the signature: it is what an adapter names in
        // its own errors, and what a live drill correlates on.
        let text = prompt.to_string();
        self.herdr.agent_prompt(pane, &text)?;
        Ok(())
    }

    fn observe(&self, pane: &PaneId) -> Result<Observation, AdapterError> {
        // See `find_claude_agent`'s own doc comment for the full branch
        // list this collapses onto `unavailable_observation` — unreachable
        // Irrlicht, no session at this pane, or a session at this pane that
        // belongs to a different harness. `unavailable_observation`'s
        // documented case ("Herdr could not answer at all") restated here
        // for Irrlicht.
        let Some(agent) = find_claude_agent(&self.irrlicht, pane)? else {
            return Ok(unavailable_observation(pane));
        };

        let state = agent
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let transcript_path = agent
            .get("transcript_path")
            .and_then(Value::as_str)
            .map(PathBuf::from);
        // Irrlicht's `session_id` is not always the harness's session UUID.
        // Measured live on 2026-09-09: for the first seconds after `start`,
        // before Irrlicht has finished indexing the new transcript, it
        // reports a process placeholder such as `proc-19040` instead.
        //
        // Migration 5's `sessions.harness_session_id` column is documented to
        // hold the bare UUID, and `PiAdapter` enforces exactly that through
        // `parse_harness_session_id`'s `looks_like_uuid` check. Accepting a
        // placeholder here would put a value in that column that no later
        // correlation could match, and it would look like an answer.
        //
        // So an id that is not UUID-shaped becomes `None` — "not known yet",
        // which is true and which the column already allows — rather than a
        // durable wrong value. The real UUID arrives on any later `observe`.
        let harness_session_id = agent
            .get("session_id")
            .and_then(Value::as_str)
            .filter(|id| crate::looks_like_uuid(id))
            .map(str::to_string);

        // Irrlicht infers state from a transcript; it is never told state by
        // the harness the way Herdr is told Pi's (this module's own docs).
        // So confidence never reaches `Authoritative` here, whatever
        // Irrlicht's own `"confidence"` field on the payload claims — that
        // field is read nowhere in this function, on purpose. An
        // unrecognised `state` string is version skew (ADR 0011 open item
        // 2): treated as no usable reading, not guessed at.
        let confidence = if KNOWN_IRRLICHT_STATES.contains(&state.as_str()) {
            Confidence::Degraded
        } else {
            Confidence::Unavailable
        };

        // A degraded (or unavailable) reading must never move a task,
        // whatever the status string says — this crate's module docs,
        // restated for Irrlicht's vocabulary. Confidence here never reaches
        // `Authoritative`, so this is unconditional rather than a clamp
        // applied to some other computed signal: no branch above could ever
        // justify anything but `NoChange`, and that absence of a branch is
        // the point (see `tests::claude_observation_is_always_degraded_and_never_moves_a_task`).
        let task_signal = TaskSignal::NoChange;

        Ok(Observation {
            pane: pane.clone(),
            harness_state: state,
            confidence,
            session_alive: true,
            task_signal,
            transcript_path,
            harness_session_id,
        })
    }

    fn interrupt(&self, pane: &PaneId) -> Result<(), AdapterError> {
        self.herdr.agent_interrupt(pane)?;
        Ok(())
    }

    fn stop(&self, pane: &PaneId) -> Result<(), AdapterError> {
        self.herdr.pane_close(pane)?;
        Ok(())
    }

    fn attach_command(&self, pane: &PaneId) -> Result<Vec<String>, AdapterError> {
        Ok(attach_argv(pane))
    }

    fn runtime_version(&self) -> Result<String, AdapterError> {
        self.herdr.version().map(|raw| raw.trim().to_string())
    }

    fn cost_sample(&self, pane: &PaneId) -> Result<Option<CostSample>, AdapterError> {
        // Same lookup `observe` uses — see `find_claude_agent`'s doc
        // comment. `None` here already covers everything `observe` degrades
        // to `Confidence::Unavailable` for; there is no equivalent
        // "unavailable cost sample" to construct, `None` already is it.
        let Some(agent) = find_claude_agent(&self.irrlicht, pane)? else {
            return Ok(None);
        };

        let Some(metrics) = agent.get("metrics").filter(|m| !m.is_null()) else {
            // `metrics: null` — `opencode` sessions report exactly this, and
            // two of four live Pi sessions reported no metrics at all when
            // ADR 0021 measured this. `None` is the ordinary case, not the
            // edge.
            return Ok(None);
        };

        let (Some(input_tokens), Some(output_tokens)) = (
            metrics.get("cum_input_tokens").and_then(Value::as_u64),
            metrics.get("cum_output_tokens").and_then(Value::as_u64),
        ) else {
            // The cumulative counters are the only tokens this crate trusts
            // (see `total_tokens` below); without both there is nothing to
            // sum, whatever else `metrics` carries.
            return Ok(None);
        };

        Ok(Some(CostSample {
            source: CostSource::ClaudeCode,
            model: metrics
                .get("model_name")
                .and_then(Value::as_str)
                .map(str::to_string),
            input_tokens,
            output_tokens,
            // `elapsed_seconds` is the only duration Irrlicht reports;
            // converted to milliseconds to match `tasks.cost_duration_ms`'s
            // unit.
            duration_ms: metrics
                .get("elapsed_seconds")
                .and_then(Value::as_u64)
                .map(|secs| secs.saturating_mul(1000)),
            // ADR 0021 decision 7, measured across all 8 live sessions
            // carrying the three fields with no exceptions: `total_tokens ==
            // context_window × context_utilization_percentage / 100`. It is
            // context occupancy, not usage, and it is read nowhere above
            // this line — only `context_utilization_percentage`, the figure
            // that measurement was actually about, lands here, under its
            // own name.
            context_utilization_percent: metrics
                .get("context_utilization_percentage")
                .and_then(Value::as_f64),
        }))
    }
}

#[cfg(test)]
mod tests {
    /// Measured live on 2026-09-09: for a second or two after `start`,
    /// Irrlicht reports `session_id` as a process placeholder (`proc-19040`)
    /// rather than the harness session UUID. Migration 5's column is
    /// documented to hold the bare UUID, and `PiAdapter` already refuses
    /// anything else, so this adapter must too — a placeholder written there
    /// is a durable wrong value that looks like an answer.
    #[test]
    fn a_placeholder_session_id_is_reported_as_unknown_not_stored_verbatim() {
        let observe_with = |session_id: &str| {
            let payload =
                sessions_fixture().replace("11111111-1111-4111-8111-0000000000a1", session_id);
            ClaudeAdapter::new(FakeHerdr::new(), FakeIrrlicht::always(payload))
                .observe(&PaneId("wE:p2".to_string()))
                .expect("observe")
        };

        assert_eq!(
            observe_with("proc-19040").harness_session_id,
            None,
            "a non-UUID placeholder must not reach `sessions.harness_session_id`"
        );
        assert_eq!(
            observe_with("22222222-2222-4222-8222-0000000000b2")
                .harness_session_id
                .as_deref(),
            Some("22222222-2222-4222-8222-0000000000b2"),
            "a real UUID must still come through untouched"
        );
    }

    use std::cell::RefCell;
    use std::path::Path;
    use std::rc::Rc;

    use factory_paths::CanonicalPath;

    use super::*;

    type Resp = Result<String, AdapterError>;
    // Named, same as lib.rs's own `FakeHerdr` test double: `clippy::all`
    // denies an inline `Box<dyn Fn(...)>` complex enough to need factoring.
    type PaneFn = Box<dyn Fn(&PaneId) -> Resp>;
    type AgentStartFn = Box<dyn Fn(&str, &str, &PaneId, &[String]) -> Resp>;
    type AgentPromptFn = Box<dyn Fn(&PaneId, &str) -> Resp>;

    /// A `HerdrAccess` double built from closures, every method defaulting
    /// to an explicit panic — a test that forgets to configure a method it
    /// actually needs fails loudly rather than silently reading a stale
    /// default, and a test that never expects a method to be called at all
    /// gets that as a free assertion.
    struct FakeHerdr {
        agent_get: PaneFn,
        agent_explain: PaneFn,
        version: Box<dyn Fn() -> Resp>,
        workspace_create: Box<dyn Fn(&Path) -> Resp>,
        agent_start: AgentStartFn,
        agent_prompt: AgentPromptFn,
        agent_interrupt: PaneFn,
        pane_close: PaneFn,
    }

    impl FakeHerdr {
        fn new() -> Self {
            Self {
                agent_get: Box::new(|_| unreachable!("test must configure agent_get")),
                agent_explain: Box::new(|_| {
                    unreachable!(
                        "ClaudeAdapter::observe must never call agent_explain — observation for \
                         Claude comes from Irrlicht"
                    )
                }),
                version: Box::new(|| unreachable!("test must configure version")),
                workspace_create: Box::new(|_| {
                    unreachable!("test must configure workspace_create")
                }),
                agent_start: Box::new(|_, _, _, _| unreachable!("test must configure agent_start")),
                agent_prompt: Box::new(|_, _| unreachable!("test must configure agent_prompt")),
                agent_interrupt: Box::new(|_| unreachable!("test must configure agent_interrupt")),
                pane_close: Box::new(|_| unreachable!("test must configure pane_close")),
            }
        }

        fn with_agent_get(mut self, f: impl Fn(&PaneId) -> Resp + 'static) -> Self {
            self.agent_get = Box::new(f);
            self
        }
        fn with_version(mut self, f: impl Fn() -> Resp + 'static) -> Self {
            self.version = Box::new(f);
            self
        }
        fn with_workspace_create(mut self, f: impl Fn(&Path) -> Resp + 'static) -> Self {
            self.workspace_create = Box::new(f);
            self
        }
        fn with_agent_start(
            mut self,
            f: impl Fn(&str, &str, &PaneId, &[String]) -> Resp + 'static,
        ) -> Self {
            self.agent_start = Box::new(f);
            self
        }
        fn with_agent_prompt(mut self, f: impl Fn(&PaneId, &str) -> Resp + 'static) -> Self {
            self.agent_prompt = Box::new(f);
            self
        }
        fn with_agent_interrupt(mut self, f: impl Fn(&PaneId) -> Resp + 'static) -> Self {
            self.agent_interrupt = Box::new(f);
            self
        }
        fn with_pane_close(mut self, f: impl Fn(&PaneId) -> Resp + 'static) -> Self {
            self.pane_close = Box::new(f);
            self
        }
    }

    impl HerdrAccess for FakeHerdr {
        fn agent_get(&self, pane: &PaneId) -> Resp {
            (self.agent_get)(pane)
        }
        fn agent_explain(&self, pane: &PaneId) -> Resp {
            (self.agent_explain)(pane)
        }
        fn version(&self) -> Resp {
            (self.version)()
        }
        fn workspace_create(&self, cwd: &Path) -> Resp {
            (self.workspace_create)(cwd)
        }
        fn agent_start(&self, name: &str, kind: &str, pane: &PaneId, args: &[String]) -> Resp {
            (self.agent_start)(name, kind, pane, args)
        }
        fn agent_prompt(&self, pane: &PaneId, text: &str) -> Resp {
            (self.agent_prompt)(pane, text)
        }
        fn agent_interrupt(&self, pane: &PaneId) -> Resp {
            (self.agent_interrupt)(pane)
        }
        fn pane_close(&self, pane: &PaneId) -> Resp {
            (self.pane_close)(pane)
        }
    }

    /// An `IrrlichtAccess` double built from one closure.
    struct FakeIrrlicht {
        sessions: Box<dyn Fn() -> Resp>,
    }

    impl FakeIrrlicht {
        fn new(f: impl Fn() -> Resp + 'static) -> Self {
            Self {
                sessions: Box::new(f),
            }
        }

        fn always(body: String) -> Self {
            Self::new(move || Ok(body.clone()))
        }
    }

    impl IrrlichtAccess for FakeIrrlicht {
        fn sessions(&self) -> Resp {
            (self.sessions)()
        }
    }

    fn fixture(name: &str) -> String {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/irrlicht");
        std::fs::read_to_string(format!("{dir}/{name}"))
            .unwrap_or_else(|e| panic!("read fixture {name}: {e}"))
    }

    /// The real `GET /api/v1/sessions` shape captured live on this machine,
    /// 2026-09-09 — see `crates/factory-adapter/fixtures/irrlicht/sessions.json`.
    /// Keys and structure are verbatim from that capture; only the operator's
    /// username, hostnames, and per-session identifiers were replaced with
    /// clearly-synthetic values, matching this crate's existing
    /// `fixtures/herdr/` convention. It carries: an unrelated `codex` session
    /// (`w3:pD`), a `pi` session (`w6:p1`, used by the "wrong harness" test
    /// below), and three `claude-code` sessions — two (`wE:p2`, `wE:p4`)
    /// sharing one `cwd`, exactly the ambiguity this adapter's cwd rule
    /// exists to survive, plus one (`wD:p1`) in a different directory with
    /// `state: "error"`. `wE:p2`'s entry also carries one real-shaped
    /// subagent under `children`, which has no `launcher` object at all.
    fn sessions_fixture() -> String {
        fixture("sessions.json")
    }

    fn uid(seed: u32) -> uuid::Uuid {
        uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
    }

    // --- observe: the pane is the only join key, and confidence never rises ---

    #[test]
    fn claude_session_found_by_pane_is_degraded_never_authoritative() {
        let herdr = FakeHerdr::new();
        let irrlicht = FakeIrrlicht::always(sessions_fixture());
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        let obs = adapter
            .observe(&PaneId("wE:p2".to_string()))
            .expect("a real claude-code fixture must observe cleanly");

        assert_eq!(obs.pane, PaneId("wE:p2".to_string()));
        assert_eq!(obs.confidence, Confidence::Degraded);
        assert_eq!(obs.harness_state, "working");
        assert!(obs.session_alive);
        assert_eq!(
            obs.harness_session_id.as_deref(),
            Some("11111111-1111-4111-8111-0000000000a1")
        );
        assert_eq!(
            obs.transcript_path.as_deref(),
            Some(Path::new(
                "/Users/operator/.claude/projects/-Users-operator-business-factory-projects-factory/11111111-1111-4111-8111-0000000000a1.jsonl"
            ))
        );
    }

    /// The single most important property of this adapter: however many
    /// branches `observe` has, none of them may ever produce `Authoritative`
    /// — mutate the `Confidence::Degraded` literal in `observe` to
    /// `Confidence::Authoritative` and this test is the one that goes red.
    #[test]
    fn claude_observation_is_always_degraded_and_never_moves_a_task() {
        let herdr = FakeHerdr::new();
        let irrlicht = FakeIrrlicht::always(sessions_fixture());
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        for pane in ["wE:p2", "wE:p4", "wD:p1"] {
            let obs = adapter
                .observe(&PaneId(pane.to_string()))
                .unwrap_or_else(|e| panic!("pane {pane} must observe cleanly: {e}"));
            assert_ne!(
                obs.confidence,
                Confidence::Authoritative,
                "pane {pane} must never be Authoritative — Irrlicht infers state, it is not told \
                 it the way Herdr is told Pi's"
            );
            assert_eq!(
                obs.task_signal,
                TaskSignal::NoChange,
                "pane {pane}: a non-authoritative reading must never move a task"
            );
        }
    }

    /// Irrlicht's own `"confidence":"medium"` field is present on every
    /// entry in the real fixture. This test proves it is never read: forcing
    /// it to a value that would misleadingly suggest trust (`"high"`) must
    /// not change this adapter's answer at all.
    #[test]
    fn payload_confidence_field_is_never_trusted() {
        let mut value: Value =
            serde_json::from_str(&sessions_fixture()).expect("fixture is valid json");
        value["groups"][1]["agents"][1]["confidence"] = Value::String("high".to_string());
        assert_eq!(
            value["groups"][1]["agents"][1]["launcher"]["herdr_pane_id"],
            "wE:p2"
        );

        let herdr = FakeHerdr::new();
        let irrlicht = FakeIrrlicht::always(value.to_string());
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        let obs = adapter
            .observe(&PaneId("wE:p2".to_string()))
            .expect("must still observe cleanly");
        assert_eq!(obs.confidence, Confidence::Degraded);
    }

    // --- the pane is the join key: never cwd ------------------------------

    /// The critical regression test for this adapter's central rule. Two
    /// real `claude-code` sessions (`wE:p2`, `wE:p4`) share one `cwd` in the
    /// fixture; each pane must resolve to its *own* session, never the
    /// other's. Mutating `find_by_pane` to match on `cwd` instead of
    /// `launcher.herdr_pane_id` must turn this red.
    #[test]
    fn two_claude_sessions_sharing_a_cwd_are_never_confused() {
        let herdr = FakeHerdr::new();
        let irrlicht = FakeIrrlicht::always(sessions_fixture());
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        let a = adapter
            .observe(&PaneId("wE:p2".to_string()))
            .expect("wE:p2 must observe cleanly");
        let b = adapter
            .observe(&PaneId("wE:p4".to_string()))
            .expect("wE:p4 must observe cleanly");

        assert_eq!(
            a.harness_session_id.as_deref(),
            Some("11111111-1111-4111-8111-0000000000a1"),
            "wE:p2 must resolve to its own session, not wE:p4's"
        );
        assert_eq!(
            b.harness_session_id.as_deref(),
            Some("11111111-1111-4111-8111-0000000000a2"),
            "wE:p4 must resolve to its own session, not wE:p2's — this is the pane that a \
             cwd-based join would get wrong, since it is listed second in the fixture"
        );
        assert_ne!(a.transcript_path, b.transcript_path);
        assert_ne!(a.harness_state, "" as &str); // sanity: both actually observed
        assert_eq!(a.harness_state, "working");
        assert_eq!(b.harness_state, "ready");
    }

    #[test]
    fn subagents_under_children_are_never_matched() {
        // `wE:p2`'s entry carries a `children` subagent with no `launcher`
        // object. Querying the *parent* pane must resolve the parent's own
        // session, never the child's.
        let herdr = FakeHerdr::new();
        let irrlicht = FakeIrrlicht::always(sessions_fixture());
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        let obs = adapter
            .observe(&PaneId("wE:p2".to_string()))
            .expect("must observe cleanly despite a children block being present");
        assert_eq!(
            obs.harness_session_id.as_deref(),
            Some("11111111-1111-4111-8111-0000000000a1"),
            "must resolve the parent session, not the subagent's `agent-a2d8e1c8393c11d44`"
        );
    }

    #[test]
    fn pane_absent_from_irrlicht_is_unavailable_not_an_error() {
        let herdr = FakeHerdr::new();
        let irrlicht = FakeIrrlicht::always(sessions_fixture());
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        let obs = adapter
            .observe(&PaneId("zz:p9".to_string()))
            .expect("a pane Irrlicht has never heard of must still observe cleanly");

        assert_eq!(obs.confidence, Confidence::Unavailable);
        assert!(!obs.session_alive);
        assert!(obs.transcript_path.is_none());
        assert!(obs.harness_session_id.is_none());
    }

    #[test]
    fn pane_owned_by_a_different_harness_is_unavailable_not_wrong_harness() {
        // `w6:p1` belongs to a `pi` session in the fixture. A Claude adapter
        // asked about that pane has no usable answer about *its* harness —
        // and must not reuse `AdapterError::WrongHarness`, whose message is
        // written for Pi's vocabulary (`not \`pi\``) and would misreport
        // this adapter's own harness name if pressed into service here.
        let herdr = FakeHerdr::new();
        let irrlicht = FakeIrrlicht::always(sessions_fixture());
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        let obs = adapter
            .observe(&PaneId("w6:p1".to_string()))
            .expect("a pane owned by a different harness must still observe cleanly");
        assert_eq!(obs.confidence, Confidence::Unavailable);
        assert!(!obs.session_alive);
    }

    #[test]
    fn irrlicht_unavailable_is_recorded_not_raised() {
        let herdr = FakeHerdr::new();
        let irrlicht = FakeIrrlicht::new(|| {
            Err(AdapterError::UnreadableOutput {
                command: "curl".to_string(),
                detail: "fixture: connection refused".to_string(),
                help: "fix the test".to_string(),
            })
        });
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        let obs = adapter
            .observe(&PaneId("wE:p2".to_string()))
            .expect("an unreachable Irrlicht must not fail observe — manual confirmation instead");
        assert_eq!(obs.confidence, Confidence::Unavailable);
        assert_eq!(obs.task_signal, TaskSignal::NoChange);
    }

    #[test]
    fn malformed_json_is_unreadable_output_not_a_panic() {
        let herdr = FakeHerdr::new();
        let irrlicht = FakeIrrlicht::always("not json at all {".to_string());
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        let err = adapter
            .observe(&PaneId("wE:p2".to_string()))
            .expect_err("malformed JSON must be reported, not panicked on");
        assert!(
            matches!(err, AdapterError::UnreadableOutput { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn missing_groups_key_is_unreadable_output() {
        let herdr = FakeHerdr::new();
        let irrlicht = FakeIrrlicht::always(r#"{"nothing":"here"}"#.to_string());
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        let err = adapter
            .observe(&PaneId("wE:p2".to_string()))
            .expect_err("a payload with no `groups` array at all is schema drift, not a miss");
        assert!(
            matches!(err, AdapterError::UnreadableOutput { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn unrecognised_state_is_unavailable_but_keeps_its_metadata() {
        let mut value: Value =
            serde_json::from_str(&sessions_fixture()).expect("fixture is valid json");
        value["groups"][1]["agents"][2]["state"] = Value::String("syncing".to_string());
        assert_eq!(
            value["groups"][1]["agents"][2]["launcher"]["herdr_pane_id"],
            "wE:p4"
        );

        let herdr = FakeHerdr::new();
        let irrlicht = FakeIrrlicht::always(value.to_string());
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        let obs = adapter
            .observe(&PaneId("wE:p4".to_string()))
            .expect("an unrecognised but present state must still observe cleanly");
        assert_eq!(obs.confidence, Confidence::Unavailable);
        assert_eq!(
            obs.harness_state, "syncing",
            "the raw string is kept verbatim"
        );
        assert_eq!(obs.task_signal, TaskSignal::NoChange);
        // Unlike "pane not found", a present-but-unrecognised reading keeps
        // its real metadata — this is a different failure mode from "no
        // answer at all", exactly as `PiAdapter::observe`'s "unknown status"
        // branch also keeps `session_alive`/`transcript_path` rather than
        // falling back to a blank observation.
        assert!(obs.session_alive);
        assert_eq!(
            obs.harness_session_id.as_deref(),
            Some("11111111-1111-4111-8111-0000000000a2")
        );
    }

    #[test]
    fn observe_never_touches_herdr_agent_get_or_explain() {
        let herdr = FakeHerdr::new(); // both default to `unreachable!()`
        let irrlicht = FakeIrrlicht::always(sessions_fixture());
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        adapter
            .observe(&PaneId("wE:p2".to_string()))
            .expect("observe must succeed using only Irrlicht");
    }

    // --- send: the busy precheck reads Herdr, not Irrlicht ----------------

    #[test]
    fn send_refuses_a_working_pane_without_submitting_anything() {
        let herdr = FakeHerdr::new()
            .with_agent_get(
                |_| Ok(r#"{"result":{"agent":{"agent_status":"working"}}}"#.to_string()),
            )
            .with_agent_prompt(|_, _| {
                unreachable!("send must refuse before ever calling agent_prompt on a working pane")
            });
        let irrlicht = FakeIrrlicht::new(|| {
            unreachable!("send's busy precheck must read Herdr, not Irrlicht")
        });
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        let err = adapter
            .send(&PaneId("wE:p2".to_string()), uid(1), "second task")
            .expect_err("a working pane must refuse a new send");
        match err {
            AdapterError::SessionBusy { harness_state, .. } => assert_eq!(harness_state, "working"),
            other => panic!("expected SessionBusy, got {other:?}"),
        }
    }

    #[test]
    fn send_submits_the_prompt_verbatim_without_adding_a_second_task_id() {
        let seen: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let seen_write = Rc::clone(&seen);
        let herdr = FakeHerdr::new()
            .with_agent_get(|_| Ok(r#"{"result":{"agent":{"agent_status":"idle"}}}"#.to_string()))
            .with_agent_prompt(move |_, text| {
                *seen_write.borrow_mut() = Some(text.to_string());
                Ok(r#"{"result":{"type":"ok"}}"#.to_string())
            });
        let irrlicht = FakeIrrlicht::new(|| unreachable!("send must not call Irrlicht"));
        let adapter = ClaudeAdapter::new(herdr, irrlicht);
        let task_id = uid(42);

        adapter
            .send(&PaneId("wE:p2".to_string()), task_id, "do the thing")
            .expect("send to an idle session must confirm");

        let sent = seen.borrow().clone().expect("agent_prompt must be called");
        assert_eq!(
            sent, "do the thing",
            "the adapter submits the prompt verbatim: `factory_task::deliver` already rendered \
             design §5's task-id line into it. An adapter that prefixed too produced \
             `[task <id>] [task <id>]` on a real Pi pane."
        );
        assert!(
            !sent.contains(&task_id.to_string()),
            "the adapter must not add the task id a second time: {sent}"
        );
    }

    #[test]
    fn send_treats_an_unreadable_herdr_get_as_not_busy_and_lets_agent_prompt_fail_properly() {
        // Herdr cannot even answer `agent get` (pane unknown, Herdr down).
        // `send`'s precheck must not hard-fail on that — it degrades to
        // "unknown" and lets `agent_prompt` itself report the real failure,
        // mirroring `PiAdapter::send`'s identical leniency.
        let herdr = FakeHerdr::new()
            .with_agent_get(|_| {
                Err(AdapterError::UnreadableOutput {
                    command: "herdr agent get".to_string(),
                    detail: "fixture: agent_not_found".to_string(),
                    help: "fix the test".to_string(),
                })
            })
            .with_agent_prompt(|_, _| {
                Err(AdapterError::UnreadableOutput {
                    command: "herdr agent prompt".to_string(),
                    detail: "fixture: agent_not_found".to_string(),
                    help: "fix the test".to_string(),
                })
            });
        let irrlicht = FakeIrrlicht::new(|| unreachable!("send must not call Irrlicht"));
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        let err = adapter
            .send(&PaneId("zz:p9".to_string()), uid(1), "task")
            .expect_err("agent_prompt's own failure must surface, not a false SessionBusy");
        assert!(
            !matches!(err, AdapterError::SessionBusy { .. }),
            "an unanswerable precheck must not be reported as busy: {err:?}"
        );
    }

    #[test]
    fn send_malformed_herdr_json_is_a_hard_error() {
        let herdr = FakeHerdr::new()
            .with_agent_get(|_| Ok("not json at all {".to_string()))
            .with_agent_prompt(|_, _| unreachable!("must fail before ever prompting"));
        let irrlicht = FakeIrrlicht::new(|| unreachable!("send must not call Irrlicht"));
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        let err = adapter
            .send(&PaneId("wE:p2".to_string()), uid(1), "task")
            .expect_err("malformed agent_get output must be a hard error, not swallowed");
        assert!(
            matches!(err, AdapterError::UnreadableOutput { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn send_propagates_submission_unconfirmed_from_herdr() {
        let herdr = FakeHerdr::new()
            .with_agent_get(|_| Ok(r#"{"result":{"agent":{"agent_status":"idle"}}}"#.to_string()))
            .with_agent_prompt(|pane, _| {
                Err(AdapterError::SubmissionUnconfirmed {
                    pane: pane.0.clone(),
                    detail: "fixture: stalled".to_string(),
                    help: "fix the test".to_string(),
                })
            });
        let irrlicht = FakeIrrlicht::new(|| unreachable!("send must not call Irrlicht"));
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        let err = adapter
            .send(&PaneId("wE:p2".to_string()), uid(1), "task")
            .expect_err("a stalled confirmation must not be reported as Ok");
        assert!(
            matches!(err, AdapterError::SubmissionUnconfirmed { .. }),
            "got {err:?}"
        );
    }

    // --- start / interrupt / stop / attach_command / runtime_version ------

    #[test]
    fn start_creates_a_pane_and_starts_claude_in_it() {
        let seen_cwd: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
        let seen_cwd_write = Rc::clone(&seen_cwd);
        let seen_kind: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let seen_kind_write = Rc::clone(&seen_kind);
        let seen_args: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let seen_args_write = Rc::clone(&seen_args);

        let herdr = FakeHerdr::new()
            .with_workspace_create(move |cwd| {
                *seen_cwd_write.borrow_mut() = Some(cwd.to_path_buf());
                Ok(r#"{"result":{"root_pane":{"pane_id":"wQ:p1"}}}"#.to_string())
            })
            .with_agent_start(move |_, kind, pane, args| {
                *seen_kind_write.borrow_mut() = Some(kind.to_string());
                *seen_args_write.borrow_mut() = args.to_vec();
                assert_eq!(
                    pane.0, "wQ:p1",
                    "agent_start must target the pane just created"
                );
                Ok(r#"{"result":{"type":"agent_started"}}"#.to_string())
            });
        let irrlicht = FakeIrrlicht::always(
            r#"{"groups":[{"name":"g","agents":[{"adapter":"claude-code","state":"ready","session_id":"33333333-3333-4333-8333-0000000000c3","transcript_path":"/fake/s1.jsonl","launcher":{"herdr_pane_id":"wQ:p1"}}]}]}"#
                .to_string(),
        );
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = CanonicalPath::resolve(dir.path()).expect("resolve tempdir");
        let req = StartRequest {
            scope_id: uid(1),
            session_id: uid(2),
            workspace,
            generated_context: "company\nscope\nagent\n".to_string(),
        };

        let started = adapter.start(&req).expect("start must succeed");
        assert_eq!(started.pane, PaneId("wQ:p1".to_string()));
        assert_eq!(seen_cwd.borrow().as_deref(), Some(req.workspace.as_path()));
        assert_eq!(seen_kind.borrow().as_deref(), Some("claude"));
        assert!(
            seen_args.borrow().is_empty(),
            "Claude does not use Pi's Herdr lifecycle-hook extension argument: {:?}",
            seen_args.borrow()
        );
        assert_eq!(started.confidence, Confidence::Degraded);
        assert_eq!(
            started.harness_session_id.as_deref(),
            Some("33333333-3333-4333-8333-0000000000c3")
        );
    }

    #[test]
    fn start_propagates_a_failed_workspace_create() {
        let herdr = FakeHerdr::new().with_workspace_create(|_| {
            Err(AdapterError::UnreadableOutput {
                command: "herdr workspace create".to_string(),
                detail: "fixture: refused".to_string(),
                help: "fix the test".to_string(),
            })
        });
        let irrlicht = FakeIrrlicht::new(|| unreachable!("must never reach observe"));
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = CanonicalPath::resolve(dir.path()).expect("resolve tempdir");
        let req = StartRequest {
            scope_id: uid(1),
            session_id: uid(2),
            workspace,
            generated_context: String::new(),
        };

        adapter
            .start(&req)
            .expect_err("a refused workspace_create must fail start, not fabricate a session");
    }

    #[test]
    fn start_closes_the_pane_it_created_when_agent_start_fails() {
        let closed: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let closed_write = Rc::clone(&closed);

        let herdr = FakeHerdr::new()
            .with_workspace_create(|_| {
                Ok(r#"{"result":{"root_pane":{"pane_id":"wQ:p1"}}}"#.to_string())
            })
            .with_agent_start(|_, _, _, _| {
                Err(AdapterError::UnreadableOutput {
                    command: "herdr agent start".to_string(),
                    detail: "fixture: claude not installed".to_string(),
                    help: "fix the test".to_string(),
                })
            })
            .with_pane_close(move |pane| {
                closed_write.borrow_mut().push(pane.0.clone());
                Ok(r#"{"result":{"type":"ok"}}"#.to_string())
            });
        let irrlicht = FakeIrrlicht::new(|| unreachable!("must never reach observe"));
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = CanonicalPath::resolve(dir.path()).expect("resolve tempdir");
        let req = StartRequest {
            scope_id: uid(1),
            session_id: uid(2),
            workspace,
            generated_context: String::new(),
        };

        adapter
            .start(&req)
            .expect_err("a failed agent_start must fail start");
        assert_eq!(
            closed.borrow().as_slice(),
            ["wQ:p1"],
            "the pane created before agent_start failed must be closed, or it leaks"
        );
    }

    #[test]
    fn start_retries_agent_start_on_a_transient_pane_busy_error() {
        let attempts: Rc<std::cell::Cell<u32>> = Rc::new(std::cell::Cell::new(0));
        let attempts_write = Rc::clone(&attempts);

        let herdr = FakeHerdr::new()
            .with_workspace_create(|_| {
                Ok(r#"{"result":{"root_pane":{"pane_id":"wQ:p1"}}}"#.to_string())
            })
            .with_agent_start(move |_, _, _, _| {
                let n = attempts_write.get();
                attempts_write.set(n + 1);
                if n < 2 {
                    Err(AdapterError::UnreadableOutput {
                        command: "herdr agent start".to_string(),
                        detail: r#"{"error":{"code":"agent_pane_busy","message":"agent target pane wQ:p1 is not an available shell"}}"#.to_string(),
                        help: "fix the test".to_string(),
                    })
                } else {
                    Ok(r#"{"result":{"type":"agent_started"}}"#.to_string())
                }
            });
        let irrlicht = FakeIrrlicht::always(
            r#"{"groups":[{"name":"g","agents":[{"adapter":"claude-code","state":"ready","session_id":"33333333-3333-4333-8333-0000000000c3","transcript_path":"/fake/s1.jsonl","launcher":{"herdr_pane_id":"wQ:p1"}}]}]}"#
                .to_string(),
        );
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = CanonicalPath::resolve(dir.path()).expect("resolve tempdir");
        let req = StartRequest {
            scope_id: uid(1),
            session_id: uid(2),
            workspace,
            generated_context: String::new(),
        };

        adapter
            .start(&req)
            .expect("two transient agent_pane_busy failures must be ridden out, not surfaced");
        assert_eq!(attempts.get(), 3);
    }

    #[test]
    fn start_does_not_retry_a_non_transient_agent_start_failure() {
        let attempts: Rc<std::cell::Cell<u32>> = Rc::new(std::cell::Cell::new(0));
        let attempts_write = Rc::clone(&attempts);

        let herdr = FakeHerdr::new()
            .with_workspace_create(|_| {
                Ok(r#"{"result":{"root_pane":{"pane_id":"wQ:p1"}}}"#.to_string())
            })
            .with_agent_start(move |_, _, _, _| {
                attempts_write.set(attempts_write.get() + 1);
                Err(AdapterError::UnreadableOutput {
                    command: "herdr agent start".to_string(),
                    detail: "fixture: claude not installed".to_string(),
                    help: "fix the test".to_string(),
                })
            })
            .with_pane_close(|_| Ok(r#"{"result":{"type":"ok"}}"#.to_string()));
        let irrlicht = FakeIrrlicht::new(|| unreachable!("must never reach observe"));
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = CanonicalPath::resolve(dir.path()).expect("resolve tempdir");
        let req = StartRequest {
            scope_id: uid(1),
            session_id: uid(2),
            workspace,
            generated_context: String::new(),
        };

        adapter
            .start(&req)
            .expect_err("a non-transient agent_start failure must still fail start");
        assert_eq!(
            attempts.get(),
            1,
            "a non-transient failure must not be retried at all"
        );
    }

    #[test]
    fn interrupt_sends_ctrl_c_to_the_right_pane() {
        let seen: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let seen_write = Rc::clone(&seen);
        let herdr = FakeHerdr::new().with_agent_interrupt(move |pane| {
            *seen_write.borrow_mut() = Some(pane.0.clone());
            Ok(r#"{"result":{"type":"ok"}}"#.to_string())
        });
        let irrlicht = FakeIrrlicht::new(|| unreachable!("interrupt must not call Irrlicht"));
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        adapter
            .interrupt(&PaneId("wE:p2".to_string()))
            .expect("interrupt must succeed");
        assert_eq!(seen.borrow().as_deref(), Some("wE:p2"));
    }

    #[test]
    fn stop_closes_the_right_pane() {
        let seen: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let seen_write = Rc::clone(&seen);
        let herdr = FakeHerdr::new().with_pane_close(move |pane| {
            *seen_write.borrow_mut() = Some(pane.0.clone());
            Ok(r#"{"result":{"type":"ok"}}"#.to_string())
        });
        let irrlicht = FakeIrrlicht::new(|| unreachable!("stop must not call Irrlicht"));
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        adapter
            .stop(&PaneId("wE:p2".to_string()))
            .expect("stop must succeed");
        assert_eq!(seen.borrow().as_deref(), Some("wE:p2"));
    }

    #[test]
    fn attach_command_never_touches_herdr_or_irrlicht() {
        let herdr = FakeHerdr::new();
        let irrlicht = FakeIrrlicht::new(|| unreachable!("attach_command must not call Irrlicht"));
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        let argv = adapter
            .attach_command(&PaneId("wE:p2".to_string()))
            .expect("attach_command is pure and infallible in practice");
        assert_eq!(argv, ["herdr", "agent", "attach", "wE:p2"]);
    }

    #[test]
    fn runtime_version_returns_the_trimmed_herdr_version() {
        let herdr = FakeHerdr::new().with_version(|| Ok("herdr 0.8.0\n".to_string()));
        let irrlicht = FakeIrrlicht::new(|| unreachable!("runtime_version must not call Irrlicht"));
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        assert_eq!(
            adapter.runtime_version().expect("version must be readable"),
            "herdr 0.8.0"
        );
    }

    // --- cost_sample (design §12.6, ADR 0021 decisions 6-8) ---------------

    // A free complement to the explicit `metrics: null` test below: the real
    // capture carries no `metrics` key at all, so `None` covers both "the
    // key is absent" and "the key is present and null".
    #[test]
    fn cost_sample_on_the_real_fixture_is_none_metrics_key_absent() {
        let herdr = FakeHerdr::new();
        let irrlicht = FakeIrrlicht::always(sessions_fixture());
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        let sample = adapter
            .cost_sample(&PaneId("wE:p2".to_string()))
            .expect("an absent `metrics` key must not error");
        assert!(sample.is_none());
    }

    // 3. An Irrlicht payload with `metrics: null` yields None.
    #[test]
    fn cost_sample_is_none_when_metrics_is_explicitly_null() {
        // `opencode` sessions report exactly this shape (ADR 0021 decision
        // 6).
        let mut value: Value =
            serde_json::from_str(&sessions_fixture()).expect("fixture is valid json");
        value["groups"][1]["agents"][1]["metrics"] = Value::Null;
        assert_eq!(
            value["groups"][1]["agents"][1]["launcher"]["herdr_pane_id"],
            "wE:p2"
        );

        let herdr = FakeHerdr::new();
        let irrlicht = FakeIrrlicht::always(value.to_string());
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        let sample = adapter
            .cost_sample(&PaneId("wE:p2".to_string()))
            .expect("`metrics: null` must not error");
        assert!(sample.is_none());
    }

    // 4. An Irrlicht payload whose pane is absent yields the same "not
    //    found" outcome the existing code uses (observe()'s
    //    Confidence::Unavailable), not an error.
    #[test]
    fn cost_sample_pane_absent_from_irrlicht_is_none_not_an_error() {
        let herdr = FakeHerdr::new();
        let irrlicht = FakeIrrlicht::always(sessions_fixture());
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        let sample = adapter
            .cost_sample(&PaneId("zz:p9".to_string()))
            .expect("a pane Irrlicht has never heard of must not error");
        assert!(
            sample.is_none(),
            "the same 'not found' outcome observe() reports as Confidence::Unavailable"
        );
    }

    // 5. total_tokens never appears in a token count -- the test that
    //    catches ADR 0021 decision 7 being undone. `total_tokens` is set far
    //    from `cum_input_tokens + cum_output_tokens`, and
    //    `context_utilization_percentage` is set inconsistently with
    //    `total_tokens / context_window` too, so neither field can leak in
    //    through a derivation instead of a direct read.
    #[test]
    fn cost_sample_never_reads_total_tokens_as_a_token_count() {
        let mut value: Value =
            serde_json::from_str(&sessions_fixture()).expect("fixture is valid json");
        value["groups"][1]["agents"][1]["metrics"] = serde_json::json!({
            "model_name": "claude-opus-5",
            "cum_input_tokens": 111,
            "cum_output_tokens": 222,
            "elapsed_seconds": 90,
            "context_window": 200_000,
            // Deliberately inconsistent with cum_input_tokens +
            // cum_output_tokens (333) and with context_utilization_percentage
            // below -- if either field were derived from this number instead
            // of read directly, an assertion below goes wrong, not red.
            "total_tokens": 190_000,
            "context_utilization_percentage": 17.5
        });
        assert_eq!(
            value["groups"][1]["agents"][1]["launcher"]["herdr_pane_id"],
            "wE:p2"
        );

        let herdr = FakeHerdr::new();
        let irrlicht = FakeIrrlicht::always(value.to_string());
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        let sample = adapter
            .cost_sample(&PaneId("wE:p2".to_string()))
            .expect("a real-shaped metrics object must not error")
            .expect("cum_input_tokens/cum_output_tokens are present, so this must be Some");

        assert_eq!(sample.input_tokens, 111, "must come from cum_input_tokens");
        assert_eq!(
            sample.output_tokens, 222,
            "must come from cum_output_tokens"
        );
        assert_eq!(sample.model.as_deref(), Some("claude-opus-5"));
        assert_eq!(sample.duration_ms, Some(90_000));
        assert_eq!(
            sample.context_utilization_percent,
            Some(17.5),
            "must come from context_utilization_percentage directly, never derived from \
             total_tokens / context_window"
        );
    }

    #[test]
    fn cost_sample_never_touches_herdr() {
        let herdr = FakeHerdr::new(); // every method defaults to unreachable!()
        let irrlicht = FakeIrrlicht::always(sessions_fixture());
        let adapter = ClaudeAdapter::new(herdr, irrlicht);

        adapter
            .cost_sample(&PaneId("wE:p2".to_string()))
            .expect("cost_sample must succeed using only Irrlicht");
    }
}
