//! The harness adapter contract, and the Herdr-backed adapter for Pi.
//!
//! ADR 0017 settles where Pi's state comes from, and the answer is Herdr rather
//! than Irrlicht. Herdr does not infer it: Pi loads a Herdr-installed extension
//! that reports lifecycle events over a socket, which `herdr agent explain`
//! confirms as `screen_detection_skip_reason: full_lifecycle_hook_authority`.
//! Irrlicht parses a transcript to *infer* state; Herdr is *told* it by the
//! harness. Irrlicht remains the source for `claude-code` and for token and cost
//! metrics Herdr does not report.
//!
//! This crate implements design §3's full adapter boundary —
//! `start(scope, session_id, workspace, generated_context)`, `send(task_id,
//! prompt)`, `observe()`, `interrupt()`, `stop()` — plus `attach_command` and
//! `runtime_version`, which design §3 does not name but every caller needs.
//! [`PiAdapter`] is the one implementation today; ADR 0010 decision on hosting
//! keeps this in-process rather than a supervised subprocess, and a Claude Code
//! adapter (Slice 10) is expected to satisfy the identical [`Adapter`] trait
//! and the identical [`contract::run_contract_suite`].
//!
//! Four rules run through everything here:
//!
//! - **A wrong observation is worse than none**, because it looks like an
//!   answer. Every path that cannot establish state returns
//!   [`Confidence::Unavailable`] rather than a guess.
//! - **The pane is the join key.** It is the one identifier Factory chose and
//!   recorded. Working directory is never used: two sessions of one agent may
//!   share a workspace, and `assistant` is configured for exactly that.
//! - **No harness state closes a task.** See [`TaskSignal`].
//! - **A `working` pane cannot confirm a new submission.** `herdr agent prompt
//!   --wait --until working` does not track turns: run against a pane already
//!   `working`, the wait matches whichever turn is already running and returns
//!   immediately, without ever observing *this* prompt at all (measured:
//!   `herdr agent prompt --help`). [`Adapter::send`] refuses instead of risking
//!   that false confirmation — see [`AdapterError::SessionBusy`].

use std::path::{Path, PathBuf};
use std::process::Command;

use factory_paths::CanonicalPath;
use serde_json::Value;

mod claude;
pub use claude::{ClaudeAdapter, IrrlichtAccess, IrrlichtHttp};

/// A Herdr pane, such as `wE:p1`.
///
/// Recorded by Factory when it starts a session, and the primary correlation
/// key from then on.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PaneId(pub String);

/// How much an observation can be trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    /// Reported by the harness through the Herdr lifecycle hook.
    Authoritative,
    /// Herdr answered, but without hook authority — it fell back to reading the
    /// screen. Recorded, never acted on: at-most-once delivery must not be
    /// built on a heuristic.
    Degraded,
    /// No usable answer. The caller falls back to manual confirmation.
    Unavailable,
}

/// What an observation implies for the running task, if anything.
///
/// Deliberately narrow. Neither `idle` nor `done` closes a task:
///
/// - `idle` means the harness waits for input. That is equally true before a
///   task is delivered and after one finishes, so it carries no completion
///   information.
/// - `done` is Herdr's word for a finished *turn*. A task may span many turns,
///   and mapping it to task completion would close tasks at the first pause.
///
/// Completion is recorded from the result the agent reports, never inferred
/// from the harness going quiet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskSignal {
    /// Leave the task as it is.
    NoChange,
    /// The harness is working on it.
    Running,
    /// The harness is waiting on a human. Design §2.4 requires a reason.
    Blocked(BlockedReason),
}

/// Design §2.4's blocked reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockedReason {
    Clarification,
    Permission,
    Interrupted,
    External,
}

/// One reading of a session's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub pane: PaneId,
    /// The harness's own word for its state, stored verbatim.
    ///
    /// Kept alongside the interpretation so an audit can tell what Factory was
    /// told from what Factory concluded. The two vocabularies drift with
    /// harness versions; only one of them is a fact.
    pub harness_state: String,
    pub confidence: Confidence,
    /// Whether the pane still exists and holds the expected agent.
    pub session_alive: bool,
    pub task_signal: TaskSignal,
    /// The harness's transcript, when it reports one. Outlives the pane, so it
    /// is the fallback correlation key after a restart.
    pub transcript_path: Option<PathBuf>,
    /// The harness session UUID, parsed from the transcript filename.
    pub harness_session_id: Option<String>,
}

/// design §3's `start(scope, session_id, workspace, generated_context)`,
/// carried as one struct so every adapter takes the same four inputs in the
/// same shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartRequest {
    pub scope_id: uuid::Uuid,
    pub session_id: uuid::Uuid,
    pub workspace: CanonicalPath,
    /// The compiled context text (design §2.5: company context → ancestor
    /// scope contexts → current scope context → agent definition).
    ///
    /// Carried on every `StartRequest` because design §3's contract is
    /// uniform across harnesses, not because [`PiAdapter`] sends it anywhere.
    /// It does not: Pi reads `AGENTS.md` natively at process start (design
    /// §2.5 — `pi --help` lists `--no-context-files` as the way to *disable*
    /// that discovery, so loading it is the default), so writing this text
    /// into the pane over the wire would duplicate what Pi already loaded and,
    /// sent as a prompt, would consume Pi's first turn on context instead of a
    /// task. `PiAdapter::start` never reads this field; see its doc comment.
    pub generated_context: String,
    /// The model the scope's configuration named for this agent, or `None`
    /// when it named none.
    ///
    /// An adapter passes this to its harness as that harness's own `--model`
    /// argument and does nothing else with it: Factory names a model, the
    /// harness resolves it (ADR 0024). A name the harness does not know is
    /// the harness's refusal to report, not a validation this crate performs
    /// — there is no catalogue here to check it against.
    pub model: Option<String>,
}

/// What `start` hands back: at minimum the pane design §3's architecture
/// diagram shows the adapter creating, and — when Herdr reports one — the
/// harness's own session identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedSession {
    pub pane: PaneId,
    pub harness_session_id: Option<String>,
    /// The confidence of the observation `start` used to confirm the session
    /// actually came up.
    ///
    /// ADR 0017 decision 2: a Pi session started without the Herdr lifecycle
    /// extension degrades silently to screen detection rather than failing
    /// outright, so `start` must not error on a missing hook marker — it
    /// reports [`Confidence::Degraded`] (or [`Confidence::Unavailable`], if
    /// even that could not be read) and lets the caller fall back to manual
    /// confirmation, exactly as [`Adapter::observe`]'s own contract already
    /// does elsewhere.
    pub confidence: Confidence,
}

/// A cost sample for one pane, or `None` when the harness reports nothing
/// usable right now — design §12.6's model identifier, token counts, and
/// duration, in the shape ADR 0021 decision 6 settles on.
///
/// Every field but `source` is nullable, and that is not caution for its own
/// sake: two of four live Pi sessions and every `opencode` session reported
/// no usable metrics when ADR 0021 measured this. "The harness told us
/// nothing" is the ordinary case this type exists to carry, not the edge.
///
/// # A sample, not a run's own figures
///
/// The Claude Code source (Irrlicht's `metrics`) is cumulative for the life
/// of the session — `cum_input_tokens` only grows. So "what did *this run*
/// cost" can only be answered as a difference between two samples: one taken
/// when a task is delivered, held in `tasks.cost_baseline` so the difference
/// survives a daemon restart (ADR 0021 decision 6), and one taken when the
/// run finishes. The Pi source is per-message and could be summed for just
/// the messages one run's window covers, but [`sum_pi_transcript_usage`]
/// sums the *whole transcript to date* instead, so both sources present the
/// same "sample now, sample later, subtract" shape and that rule lives in
/// one place, [`CostSample::since`], rather than two.
///
/// # Why `source` exists
///
/// Nothing about the type system stops a caller from handing
/// [`CostSample::since`] two samples from different adapters — a Pi baseline
/// and a Claude Code terminal reading, say, after a bug reassigns a task
/// across harnesses. Subtracting their token counts anyway would produce a
/// number that means nothing but looks like an answer, which this crate's
/// own rule (see the module docs) says is worse than no number at all.
/// `source` lets `since` catch that at the one place it can be caught: an
/// `Err`, not a silently wrong figure.
///
/// # Why `context_utilization_percent` is its own nullable field
///
/// ADR 0021 decision 7, measured across all 8 live sessions carrying the
/// three fields, with no exceptions: Irrlicht's `metrics.total_tokens`
/// equals `context_window × context_utilization_percentage / 100`. It is how
/// full the context window is *right now*, and it falls when a session
/// compacts. It is never a token count, and it is never summed or
/// subtracted like one — [`CostSample::since`] carries it through verbatim
/// instead of diffing it.
///
/// # No currency field
///
/// ADR 0021 decision 8: Irrlicht's `estimated_cost_usd` is an estimate
/// against Irrlicht's own price table, and Pi's per-message `cost` is its
/// harness's own billing figure. They are not the same kind of number, and
/// one field holding either would mean something different depending on
/// which adapter wrote it. Model and tokens are stored; money is derived
/// outside Factory, where the price table has one owner.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CostSample {
    /// Which adapter produced this sample. [`CostSample::since`] refuses to
    /// subtract two samples that disagree here.
    pub source: CostSource,
    /// The harness's own model identifier — Pi's `message.model`
    /// (e.g. `"gpt-6-astra"`) or Irrlicht's `metrics.model_name`.
    pub model: Option<String>,
    /// Cumulative input tokens as of this sample. Claude Code:
    /// `metrics.cum_input_tokens`, never `total_tokens` (ADR 0021 decision
    /// 7). Pi: every message's `usage.input`, summed from the start of the
    /// transcript, never `usage.cacheRead`/`cacheWrite`/`reasoning` — see
    /// [`sum_pi_transcript_usage`] for why those are dropped rather than
    /// folded in.
    pub input_tokens: u64,
    /// Cumulative output tokens as of this sample. See `input_tokens`.
    pub output_tokens: u64,
    /// Milliseconds, matching `tasks.cost_duration_ms`'s unit. `None` when
    /// the source reports no duration — Pi's transcript does not (see this
    /// crate's module docs' "measured facts"), so
    /// [`sum_pi_transcript_usage`] leaves it `None` rather than guessing at
    /// a field that was never measured.
    pub duration_ms: Option<u64>,
    /// How full the context window was when this sample was taken, 0-100.
    /// Context *pressure*, never usage — see this struct's own docs. `None`
    /// for Pi, which reports no window size to compute it against.
    pub context_utilization_percent: Option<f64>,
}

/// Which adapter produced a [`CostSample`]. Distinct sources may never be
/// subtracted from one another — see [`CostSample::since`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CostSource {
    /// Summed from the harness's own session transcript — see [`PiAdapter`].
    Pi,
    /// Read from Irrlicht's `metrics` object — see [`ClaudeAdapter`].
    ClaudeCode,
}

/// What can go wrong turning a stored `tasks.cost_baseline` value back into
/// a [`CostSample`], or subtracting one sample from another.
#[derive(Debug, thiserror::Error)]
pub enum CostSampleError {
    /// [`CostSample::since`] was asked to subtract two samples from
    /// different adapters — see that method's doc comment for why this is a
    /// caller bug rather than a runtime condition to degrade quietly.
    #[error("cannot subtract a {baseline:?} baseline from a {later:?} sample\n  help: {help}")]
    MismatchedSource {
        later: CostSource,
        baseline: CostSource,
        help: String,
    },

    /// `tasks.cost_baseline` held something other than what [`CostSample`]'s
    /// own `Display` produces — a hand-edited row, or a schema this adapter
    /// version does not know.
    #[error("cost sample is not valid JSON: {detail}\n  help: {help}")]
    Malformed { detail: String, help: String },
}

/// One run's own cost figures — [`CostSample::since`]'s result, and the
/// shape `tasks.cost_model`, `cost_input_tokens`, `cost_output_tokens`,
/// `cost_duration_ms`, and `context_utilization_percent` are written from
/// directly.
///
/// Deliberately not a [`CostSample`]: a `CostSample` carries `source` and is
/// the *cumulative-to-date* shape `tasks.cost_baseline` stores, so a value
/// that already **is** a difference must be a different type, or
/// `delta.since(other)` would compile and produce a meaningless second
/// difference. `RunCost` cannot be handed to [`CostSample::since`] at all.
#[derive(Debug, Clone, PartialEq)]
pub struct RunCost {
    /// The later sample's own model — not diffed. A model rarely changes
    /// mid-run, and the terminal reading is what a run that just finished
    /// actually used.
    pub model: Option<String>,
    /// `later.input_tokens - baseline.input_tokens`.
    pub input_tokens: u64,
    /// `later.output_tokens - baseline.output_tokens`.
    pub output_tokens: u64,
    /// `later.duration_ms - baseline.duration_ms`, when both sides have one.
    pub duration_ms: Option<u64>,
    /// The later sample's own context pressure — not diffed. It is
    /// occupancy right now (ADR 0021 decision 7), never a cumulative
    /// counter, so subtracting it would produce a number with no meaning at
    /// all.
    pub context_utilization_percent: Option<f64>,
}

impl CostSample {
    /// The run's own figures: `self` (the later sample) minus `baseline`
    /// (typically `tasks.cost_baseline`, taken at delivery). ADR 0021
    /// decision 6: a cumulative source can only answer "what did *this run*
    /// cost" as a difference between two samples, and this crate applies the
    /// same "sample now, sample later, subtract" rule to Pi's per-message
    /// sums too, so the rule has one home rather than two.
    ///
    /// `Err` is a caller bug: `baseline` was taken by a different adapter
    /// than `self` (see [`CostSampleError::MismatchedSource`]) — a run's
    /// baseline and terminal samples must come from the one adapter that
    /// started it.
    ///
    /// `Ok(None)` is a real operating condition, not a bug: `self` reports
    /// fewer tokens than `baseline`. That happens when a Claude Code session
    /// compacts, or a Pi transcript is rotated, between the two samples.
    /// Clamping to zero was considered and rejected: it would assert "this
    /// run used zero tokens," a lie of the same shape ADR 0021 decision 7
    /// already forbids for `total_tokens`. So this reports "no reliable run
    /// figure" — the same shape every other `None` this trait returns
    /// already means — rather than inventing a number. The caller still
    /// holds `self`, so the run's own context-pressure reading is not lost
    /// even when the token delta is; that is on the caller to read from
    /// `self` directly, not something this method needs to preserve.
    pub fn since(&self, baseline: &CostSample) -> Result<Option<RunCost>, CostSampleError> {
        if self.source != baseline.source {
            return Err(CostSampleError::MismatchedSource {
                later: self.source,
                baseline: baseline.source,
                help: "a run's baseline and terminal cost samples must come from the same \
                       adapter"
                    .to_string(),
            });
        }

        let (Some(input_tokens), Some(output_tokens)) = (
            self.input_tokens.checked_sub(baseline.input_tokens),
            self.output_tokens.checked_sub(baseline.output_tokens),
        ) else {
            return Ok(None);
        };

        // Duration is independently nullable throughout this type, so a
        // duration that cannot be subtracted (missing on either side, or it
        // would itself go negative) degrades to `None` for that one field
        // rather than discarding token counts that are still trustworthy.
        let duration_ms = match (self.duration_ms, baseline.duration_ms) {
            (Some(later), Some(earlier)) => later.checked_sub(earlier),
            _ => None,
        };

        Ok(Some(RunCost {
            model: self.model.clone(),
            input_tokens,
            output_tokens,
            duration_ms,
            context_utilization_percent: self.context_utilization_percent,
        }))
    }
}

impl std::fmt::Display for CostSample {
    /// Renders as JSON. This is Factory's own on-disk shape for
    /// `tasks.cost_baseline` (TEXT), not a wire contract with Pi or
    /// Irrlicht, so JSON is simply the least code to write: the struct
    /// already derives `Serialize`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let json = serde_json::to_string(self).map_err(|_| std::fmt::Error)?;
        f.write_str(&json)
    }
}

impl std::str::FromStr for CostSample {
    type Err = CostSampleError;

    /// Parses a `tasks.cost_baseline` value back into a sample so it can be
    /// subtracted from a later one — the whole reason that column exists
    /// (ADR 0021 decision 6: a difference held only in memory does not
    /// survive the daemon restart this project drills for).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s).map_err(|source| CostSampleError::Malformed {
            detail: source.to_string(),
            help: "tasks.cost_baseline holds what CostSample's own Display produces; if this \
                   came from anywhere else, that is the bug"
                .to_string(),
        })
    }
}

/// What every harness adapter provides — design §3's adapter boundary in
/// full.
pub trait Adapter {
    /// Start a harness session per design §3's `start(scope, session_id,
    /// workspace, generated_context)`.
    ///
    /// Returns `Err` when the underlying harness or Herdr could not be
    /// brought up at all — never a fabricated [`StartedSession`]. A missing
    /// Herdr lifecycle-hook marker is *not* one of those failures; see
    /// [`StartedSession::confidence`].
    fn start(&self, req: &StartRequest) -> Result<StartedSession, AdapterError>;

    /// Submit one task's prompt to `pane` and confirm the harness accepted it
    /// before returning.
    ///
    /// `Ok(())` means **confirmed**: the harness was observed moving to
    /// `working` after submission. It does not mean the task finished — only
    /// that it started. Design §5 step 3 journals the delivery attempt before
    /// this call is even made, so when the underlying keystrokes may already
    /// have reached the pane but nothing confirmed it, that is reported as
    /// [`AdapterError::SubmissionUnconfirmed`] — distinct from every other
    /// error variant, because the caller must be able to tell "never sent"
    /// apart from "sent, outcome unknown."
    ///
    /// Refuses with [`AdapterError::SessionBusy`], sending nothing, when the
    /// session is already `working` a turn — see the module docs.
    fn send(&self, pane: &PaneId, task_id: uuid::Uuid, prompt: &str) -> Result<(), AdapterError>;

    /// Read the current state of the session in `pane`.
    ///
    /// Returns an `Observation` with [`Confidence::Unavailable`] rather than an
    /// error when the harness is simply not answerable — a missing pane, a
    /// stopped Herdr. An `Err` means the adapter itself failed.
    fn observe(&self, pane: &PaneId) -> Result<Observation, AdapterError>;

    /// Interrupt whatever `pane` is doing (Ctrl-C to the harness) without
    /// closing the session.
    fn interrupt(&self, pane: &PaneId) -> Result<(), AdapterError>;

    /// Stop the session in `pane`. Design §3: Herdr owns panes; this closes
    /// the one the session occupied.
    fn stop(&self, pane: &PaneId) -> Result<(), AdapterError>;

    /// The argv the CLI should exec to attach an operator's terminal to
    /// `pane`. This method never attaches anything itself.
    ///
    /// The adapter runs inside the daemon (design §3: "Factory supervisor
    /// owns ... starting, stopping, observing, and reconciling agents"). A
    /// daemon thread has no controlling terminal to hand to a PTY takeover,
    /// and must not try — that belongs to the CLI process an operator
    /// actually runs. This method only computes the command; `factory agent
    /// attach` execs it.
    fn attach_command(&self, pane: &PaneId) -> Result<Vec<String>, AdapterError>;

    /// The harness runtime's version, recorded with each session so that later
    /// behaviour changes are attributable.
    fn runtime_version(&self) -> Result<String, AdapterError>;

    /// A point-in-time cost sample for `pane`, or `None` when the harness
    /// reports nothing usable right now — see [`CostSample`] for the shape
    /// and why `None` is the ordinary answer, not the edge.
    ///
    /// # No default, deliberately
    ///
    /// A `Ok(None)` default would let every implementor that simply forgot
    /// this method report "no cost data" as if that were an answer, and one
    /// implementor in this workspace makes that concrete: `ArcAdapter` in
    /// `factory-daemon`'s `delivery_journal_order` suite is a forwarding
    /// wrapper that delegates all seven other methods. Under a default it
    /// would answer `None` while the adapter it wraps had real figures, and
    /// nothing would fail. That is ADR 0017's own rule with a different
    /// subject: a wrong observation is worse than none, because it looks like
    /// an answer.
    ///
    /// Every other method on this trait is required, so this one is too. An
    /// adapter whose harness reports nothing writes `Ok(None)` and says so in
    /// one line, which is a statement rather than an omission.
    fn cost_sample(&self, pane: &PaneId) -> Result<Option<CostSample>, AdapterError>;
}

/// The Herdr commands the Pi adapter needs, behind a trait so tests can supply
/// recorded payloads instead of running a terminal multiplexer.
pub trait HerdrAccess {
    /// Raw stdout of `herdr agent get <pane>`.
    fn agent_get(&self, pane: &PaneId) -> Result<String, AdapterError>;
    /// Raw stdout of `herdr agent explain <pane>`.
    fn agent_explain(&self, pane: &PaneId) -> Result<String, AdapterError>;
    /// Raw stdout of `herdr --version`.
    fn version(&self) -> Result<String, AdapterError>;

    /// Raw stdout of `herdr workspace create --cwd <path> --no-focus`.
    ///
    /// Creates a fresh Herdr workspace/tab/pane rooted at `cwd` — the pane
    /// `start` then hands to `agent_start`. `--no-focus` is not optional: a
    /// daemon-driven start must never steal an operator's UI focus (measured:
    /// `herdr workspace create --help` documents `--focus`/`--no-focus` as a
    /// pair; this adapter always passes the latter).
    fn workspace_create(&self, cwd: &Path) -> Result<String, AdapterError>;

    /// Raw stdout of `herdr agent start <name> --kind <kind> --pane <pane>
    /// [-- <args...>]`.
    fn agent_start(
        &self,
        name: &str,
        kind: &str,
        pane: &PaneId,
        args: &[String],
    ) -> Result<String, AdapterError>;

    /// Raw stdout of `herdr agent prompt <pane> <text> --wait --until
    /// working`.
    ///
    /// A stalled or timed-out wait (measured shape: `herdr agent prompt
    /// --help` — "otherwise it returns `agent_prompt_stalled`... a shorter
    /// `--timeout` returns `timeout` instead") is reported as
    /// [`AdapterError::SubmissionUnconfirmed`] rather than folded into the
    /// generic [`AdapterError::UnreadableOutput`] every other command uses.
    fn agent_prompt(&self, pane: &PaneId, text: &str) -> Result<String, AdapterError>;

    /// Raw stdout of `herdr agent send-keys <pane> ctrl+c`.
    fn agent_interrupt(&self, pane: &PaneId) -> Result<String, AdapterError>;

    /// Raw stdout of `herdr pane close <pane>`.
    fn pane_close(&self, pane: &PaneId) -> Result<String, AdapterError>;
}

/// Runs the real `herdr` binary as a subprocess.
pub struct HerdrCli {
    binary: PathBuf,
}

impl HerdrCli {
    #[must_use]
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
        }
    }

    /// Runs `herdr <args>`, capturing stdout.
    ///
    /// A missing binary (the process cannot even be spawned) becomes
    /// [`AdapterError::RuntimeUnavailable`]. A spawned process that exits
    /// non-zero becomes [`AdapterError::UnreadableOutput`] — there is no
    /// `io::Error` to attach in that case, so the exit status and stderr are
    /// folded into `detail` instead.
    fn run(&self, args: &[&str]) -> Result<String, AdapterError> {
        let output = Command::new(&self.binary)
            .args(args)
            .output()
            .map_err(|source| AdapterError::RuntimeUnavailable {
                binary: self.binary.clone(),
                help: "install Herdr and ensure the `herdr` binary is on PATH, or pass its \
                           full path to `HerdrCli::new`"
                    .to_string(),
                source,
            })?;

        if !output.status.success() {
            return Err(AdapterError::UnreadableOutput {
                command: format!("herdr {}", args.join(" ")),
                detail: format!(
                    "exited with {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
                help: "run the command directly outside Factory to see the underlying Herdr error"
                    .to_string(),
            });
        }

        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Like [`Self::run`], for `herdr agent prompt` specifically: a non-zero
    /// exit is classified by [`classify_prompt_failure`] rather than folded
    /// into the generic [`AdapterError::UnreadableOutput`] every other command
    /// gets — `agent prompt`'s own documented stalled/timeout outcomes must
    /// stay distinguishable from a hard failure.
    fn run_prompt(&self, pane: &PaneId, args: &[&str]) -> Result<String, AdapterError> {
        let output = Command::new(&self.binary)
            .args(args)
            .output()
            .map_err(|source| AdapterError::RuntimeUnavailable {
                binary: self.binary.clone(),
                help: "install Herdr and ensure the `herdr` binary is on PATH, or pass its \
                           full path to `HerdrCli::new`"
                    .to_string(),
                source,
            })?;

        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
        }

        Err(classify_prompt_failure(
            &pane.0,
            &String::from_utf8_lossy(&output.stderr),
        ))
    }
}

fn owned_str_args(args: &[String]) -> Vec<&str> {
    args.iter().map(String::as_str).collect()
}

impl HerdrAccess for HerdrCli {
    fn agent_get(&self, pane: &PaneId) -> Result<String, AdapterError> {
        self.run(&["agent", "get", &pane.0])
    }
    fn agent_explain(&self, pane: &PaneId) -> Result<String, AdapterError> {
        self.run(&["agent", "explain", &pane.0])
    }
    fn version(&self) -> Result<String, AdapterError> {
        self.run(&["--version"])
    }

    fn workspace_create(&self, cwd: &Path) -> Result<String, AdapterError> {
        let args = workspace_create_args(cwd);
        self.run(&owned_str_args(&args))
    }

    fn agent_start(
        &self,
        name: &str,
        kind: &str,
        pane: &PaneId,
        args: &[String],
    ) -> Result<String, AdapterError> {
        let full = agent_start_args(name, kind, pane, args);
        self.run(&owned_str_args(&full))
    }

    fn agent_prompt(&self, pane: &PaneId, text: &str) -> Result<String, AdapterError> {
        let args = prompt_args(pane, text);
        self.run_prompt(pane, &owned_str_args(&args))
    }

    fn agent_interrupt(&self, pane: &PaneId) -> Result<String, AdapterError> {
        let args = interrupt_args(pane);
        self.run(&owned_str_args(&args))
    }

    fn pane_close(&self, pane: &PaneId) -> Result<String, AdapterError> {
        let args = pane_close_args(pane);
        self.run(&owned_str_args(&args))
    }
}

/// The Pi adapter, reading state from and driving Herdr.
pub struct PiAdapter<A: HerdrAccess> {
    herdr: A,
}

impl<A: HerdrAccess> PiAdapter<A> {
    #[must_use]
    pub fn new(herdr: A) -> Self {
        Self { herdr }
    }

    /// `herdr agent start` immediately after `herdr workspace create`,
    /// retrying a handful of times on `agent_pane_busy`.
    ///
    /// Measured live on this machine: a pane Herdr just reports as created
    /// can still fail `agent start` with `{"error":{"code":"agent_pane_busy",
    /// "message":"...is not an available shell"}}` — the shell has not
    /// finished reaching its interactive prompt yet, especially under load.
    /// A second attempt moments later succeeds; this is a brief startup race,
    /// not a real failure, and `start`'s caller should not see it as one.
    /// Every other `agent_start` failure (Herdr down, `pi` not installed) is
    /// returned immediately, unretried.
    fn start_pi_with_retry(
        &self,
        name: &str,
        pane: &PaneId,
        model: Option<&str>,
    ) -> Result<String, AdapterError> {
        const ATTEMPTS: u32 = 5;
        const RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(200);

        let mut last_err = None;
        for attempt in 0..ATTEMPTS {
            match self.herdr.agent_start(name, "pi", pane, &pi_args(model)) {
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
        // Unreachable: the loop above always returns on its last iteration
        // (`is_last_attempt` is true when `attempt + 1 == ATTEMPTS`), but
        // `last_err` still has to be a real value for the type checker.
        Err(last_err.expect("the loop always returns before exhausting every attempt"))
    }
}

/// Whether `err` is Herdr's transient "the pane exists but its shell has not
/// finished reaching an interactive prompt yet" answer to `agent start`,
/// rather than a real failure.
fn is_transient_pane_not_ready(err: &AdapterError) -> bool {
    matches!(
        err,
        AdapterError::UnreadableOutput { detail, .. }
            if detail.contains("\"code\":\"agent_pane_busy\"")
    )
}

/// Herdr's `blocked` status does not say *which* kind of block this is, only
/// that the harness is waiting on a human. One of Design §2.4's four reasons
/// must still be recorded, so the choice is which claim is safest when wrong.
///
/// [`BlockedReason::Clarification`] is the weakest of the four, and that is why
/// it is used:
///
/// - `Interrupted` asserts a turn was cut short by a human or the system. That
///   is a specific event, and none was observed.
/// - `External` names a *non-human* dependency, which contradicts the meaning of
///   `blocked` outright.
/// - `Permission` reads as an approval gate. Design §6 and §12 route external
///   side effects through approval, so a false `permission` invites a reader —
///   or a later automation — to believe a side effect is pending sign-off. That
///   is the costliest of the four to get wrong.
/// - `Clarification` claims only that the agent wants something from a human,
///   which is precisely what Herdr told us and nothing more.
///
/// The harness's own word is kept verbatim in `Observation::harness_state`, so
/// this interpretation never hides what Factory was actually told.
const UNSPECIFIED_BLOCK_REASON: BlockedReason = BlockedReason::Clarification;

/// The exact `agent explain` line that proves Herdr was *told* Pi's state by
/// the lifecycle hook, rather than falling back to screen detection.
const HOOK_AUTHORITY_KEY: &str = "screen_detection_skip_reason";
const HOOK_AUTHORITY_VALUE: &str = "full_lifecycle_hook_authority";

/// `agent explain` is plain text, `key: value` per line — not JSON.
fn explain_has_hook_authority(text: &str) -> bool {
    text.lines().any(|line| match line.split_once(':') {
        Some((key, value)) => {
            key.trim() == HOOK_AUTHORITY_KEY && value.trim() == HOOK_AUTHORITY_VALUE
        }
        None => false,
    })
}

/// Extracts the harness session UUID from a transcript filename shaped
/// `<ISO-timestamp>_<uuid>.jsonl`. Returns `None` on any deviation rather
/// than guessing from a substring.
fn parse_harness_session_id(path: &Path) -> Option<String> {
    let file_name = path.file_name()?.to_str()?;
    let stem = file_name.strip_suffix(".jsonl")?;
    let (timestamp, uuid) = stem.rsplit_once('_')?;
    if !looks_like_iso_timestamp(timestamp) {
        return None;
    }
    looks_like_uuid(uuid).then(|| uuid.to_string())
}

fn looks_like_iso_timestamp(s: &str) -> bool {
    !s.is_empty()
        && s.contains('T')
        && s.ends_with('Z')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

pub(crate) fn looks_like_uuid(s: &str) -> bool {
    const GROUP_LENGTHS: [usize; 5] = [8, 4, 4, 4, 12];
    let groups: Vec<&str> = s.split('-').collect();
    groups.len() == GROUP_LENGTHS.len()
        && groups
            .iter()
            .zip(GROUP_LENGTHS)
            .all(|(group, len)| group.len() == len && group.chars().all(|c| c.is_ascii_hexdigit()))
}

/// The observation recorded when Herdr could not answer at all — a stopped
/// Herdr, a pane it no longer knows about. Not an error: the caller falls
/// back to manual confirmation, per the `observe` contract.
fn unavailable_observation(pane: &PaneId) -> Observation {
    Observation {
        pane: pane.clone(),
        harness_state: String::new(),
        confidence: Confidence::Unavailable,
        session_alive: false,
        task_signal: TaskSignal::NoChange,
        transcript_path: None,
        harness_session_id: None,
    }
}

fn unreadable_agent_get(pane: &PaneId, detail: impl Into<String>) -> AdapterError {
    AdapterError::UnreadableOutput {
        command: format!("herdr agent get {}", pane.0),
        detail: detail.into(),
        help: "confirm the running Herdr is 0.8.0, the version this adapter was built against; \
               the JSON shape may have changed"
            .to_string(),
    }
}

// --- Pure argv builders -----------------------------------------------
//
// Every flag Herdr actually receives is built by one of these free functions,
// deliberately kept separate from `HerdrCli`'s `Command` plumbing (which the
// crate docs already note no test exercises — it runs the real binary). That
// split is what makes each flag mutation-testable: delete `--no-focus` or
// reorder `--wait`/`--until working` here and a unit test below fails, rather
// than the change being invisible to `cargo test --workspace`.

fn workspace_create_args(cwd: &Path) -> Vec<String> {
    vec![
        "workspace".to_string(),
        "create".to_string(),
        "--cwd".to_string(),
        cwd.to_string_lossy().into_owned(),
        "--no-focus".to_string(),
    ]
}

fn agent_start_args(name: &str, kind: &str, pane: &PaneId, extra: &[String]) -> Vec<String> {
    let mut args = vec![
        "agent".to_string(),
        "start".to_string(),
        name.to_string(),
        "--kind".to_string(),
        kind.to_string(),
        "--pane".to_string(),
        pane.0.clone(),
    ];
    if !extra.is_empty() {
        args.push("--".to_string());
        args.extend(extra.iter().cloned());
    }
    args
}

fn prompt_args(pane: &PaneId, text: &str) -> Vec<String> {
    vec![
        "agent".to_string(),
        "prompt".to_string(),
        pane.0.clone(),
        text.to_string(),
        "--wait".to_string(),
        "--until".to_string(),
        "working".to_string(),
    ]
}

fn interrupt_args(pane: &PaneId) -> Vec<String> {
    vec![
        "agent".to_string(),
        "send-keys".to_string(),
        pane.0.clone(),
        "ctrl+c".to_string(),
    ]
}

fn pane_close_args(pane: &PaneId) -> Vec<String> {
    vec!["pane".to_string(), "close".to_string(), pane.0.clone()]
}

/// The argv `factory agent attach` execs — see [`Adapter::attach_command`]'s
/// doc comment for why this crate only computes it.
fn attach_argv(pane: &PaneId) -> Vec<String> {
    vec![
        "herdr".to_string(),
        "agent".to_string(),
        "attach".to_string(),
        pane.0.clone(),
    ]
}

/// The Herdr lifecycle-hook extension argument, passed to `pi` after `--` so
/// Pi (not Herdr) parses it. Mirrors the live `assistant` configuration ADR
/// 0017 decision 2 cites: `--extension
/// ${HOME}/.pi/agent/extensions/herdr-agent-state.ts`. Without it Pi never
/// loads the extension that gives `observe` hook authority at all.
fn pi_extension_args() -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    vec![
        "--extension".to_string(),
        format!("{home}/.pi/agent/extensions/herdr-agent-state.ts"),
    ]
}

/// Everything after `herdr agent start ... --` for a Pi session: the
/// lifecycle-hook extension, and the model when the scope named one.
///
/// The model goes last, and only when there is one. Pi's own settings carry a
/// `defaultModel`, so omitting the flag is not "no model" — it is "whatever
/// this machine's Pi is configured for", which is exactly what every scope got
/// before this field existed (ADR 0024).
fn pi_args(model: Option<&str>) -> Vec<String> {
    let mut args = pi_extension_args();
    // Trust the workspace's own `.pi/` for this run.
    //
    // Pi discovers project-local extensions and settings only in a directory a
    // human has approved, and refuses them everywhere else — a sensible
    // default for a tool a person points at a checkout they just cloned. Under
    // Factory the situation is different in one specific way: the workspace is
    // not somewhere this process wandered into, it is the canonical path of a
    // scope an operator declared in `.factory/config.yaml`, and Factory is the
    // one putting an agent there.
    //
    // Without this flag project-level harness configuration is inert under
    // Factory: a provider registered by the scope's own extension never loads,
    // so a `model:` naming it (ADR 0024) cannot resolve, and the failure
    // arrives as "model not found" with nothing pointing at the real cause.
    //
    // What it grants is bounded by what was already granted: the session about
    // to start has tool access in this very directory. A workspace that may
    // not be trusted to register a provider is a workspace no agent should
    // have been started in.
    args.push("--approve".to_string());
    if let Some(model) = model {
        args.push("--model".to_string());
        args.push(model.to_string());
    }
    args
}

/// A Herdr agent name for `session_id`: `[a-z][a-z0-9_-]{0,31}`, unique among
/// live agents (measured: `herdr --skill`). A bare UUID can start with a
/// digit and is longer than the limit; the `factory-` prefix and an 8-hex-char
/// suffix satisfy both, and stay unique as long as `session_id` is (Factory's
/// own guarantee, not Herdr's).
///
/// The suffix is the *last* 8 hex characters of the UUID's simple form, not
/// the first 8: this workspace's own deterministic test ids are all shaped
/// `00000000-0000-4000-8000-<seed>`, varying only in the final group, so a
/// prefix slice would collide across every seeded test id in the codebase.
fn agent_name_for(session_id: uuid::Uuid) -> String {
    let simple = session_id.simple().to_string();
    format!("factory-{}", &simple[simple.len() - 8..])
}

/// Classifies a non-zero `herdr agent prompt` exit.
///
/// `"code":"agent_prompt_stalled"` and `"code":"timeout"` are the two
/// documented non-fatal outcomes (measured: `herdr agent prompt --help` —
/// "otherwise it returns `agent_prompt_stalled`... a shorter `--timeout`
/// returns `timeout` instead"); the surrounding JSON envelope is inferred
/// from the shape measured live for every other Herdr error on this machine
/// (`{"error":{"code":"...","message":"..."},"id":"..."}`), not captured for
/// this exact command. Everything else — including the `agent_not_found`
/// shape this crate's tests do capture live, from `herdr agent prompt` run
/// against a pane holding no agent — is a hard failure.
fn classify_prompt_failure(pane: &str, stderr: &str) -> AdapterError {
    let trimmed = stderr.trim();
    let stalled = trimmed.contains("\"code\":\"agent_prompt_stalled\"");
    let timed_out = trimmed.contains("\"code\":\"timeout\"");
    if stalled || timed_out {
        AdapterError::SubmissionUnconfirmed {
            pane: pane.to_string(),
            detail: trimmed.to_string(),
            help: "the prompt may have reached the pane without Herdr observing it start \
                   working; run `herdr agent get <pane>` by hand before deciding whether to retry"
                .to_string(),
        }
    } else {
        AdapterError::UnreadableOutput {
            command: format!("herdr agent prompt {pane}"),
            detail: trimmed.to_string(),
            help: "run the command directly outside Factory to see the underlying Herdr error"
                .to_string(),
        }
    }
}

/// Parses `.result.root_pane.pane_id` from `herdr workspace create`'s stdout.
fn parse_created_pane(raw: &str) -> Result<PaneId, AdapterError> {
    let unreadable = |detail: &str| AdapterError::UnreadableOutput {
        command: "herdr workspace create".to_string(),
        detail: detail.to_string(),
        help: "confirm the running Herdr is 0.8.0, the version this adapter was built against; \
               the JSON shape may have changed"
            .to_string(),
    };
    let payload: Value =
        serde_json::from_str(raw).map_err(|source| unreadable(&source.to_string()))?;
    payload
        .get("result")
        .and_then(|result| result.get("root_pane"))
        .and_then(|pane| pane.get("pane_id"))
        .and_then(Value::as_str)
        .map(|id| PaneId(id.to_string()))
        .ok_or_else(|| unreadable("missing `result.root_pane.pane_id`"))
}

/// Sums every per-message `usage` record in a Pi transcript already read
/// into memory. Pure, so tests supply fixed JSONL text instead of a real
/// file — the disk read itself lives in [`PiAdapter::cost_sample`], the one
/// part a test cannot exercise without a real file, mirroring this crate's
/// own `HerdrCli` split (see this module's docs on the pure argv builders).
///
/// ADR 0021 decision 6: the Pi source is per-message, unlike Claude Code's
/// cumulative session totals, but this function sums the *whole transcript
/// to date* rather than a caller-supplied window — the same "sample now,
/// sample later, subtract" rule this crate applies to the Claude Code source
/// applies here too, so [`CostSample::since`] is the one place that rule
/// lives, not two.
///
/// Only `usage.input` and `usage.output` are summed. `usage.cacheRead`,
/// `usage.cacheWrite`, and `usage.reasoning` are read nowhere: folding them
/// into `input_tokens`/`output_tokens` would make those two columns mean
/// "input and output, plus Pi's cache and reasoning tokens" for Pi and
/// "input and output, nothing else" for Claude Code (whose
/// `cum_input_tokens`/`cum_output_tokens` carry no such extras) — one column
/// meaning two different things depending on which adapter wrote it, ADR
/// 0021 decision 8's defect worn as token composition instead of currency.
/// `usage.totalTokens` is dropped for the same reason `total_tokens` is
/// dropped on the Claude Code side (decision 7): it is not `input + output`,
/// so it is not a token count this crate trusts.
///
/// Returns `None`, not a zeroed sample, when the transcript carries no
/// `message.usage` record at all — ADR 0021 decision 6's "remains valid when
/// the adapter reports none of them" is the ordinary case, not the edge.
fn sum_pi_transcript_usage(jsonl: &str) -> Option<CostSample> {
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut model: Option<String> = None;
    let mut seen_any = false;

    for line in jsonl.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // One unparseable line does not invalidate the rest of an
        // append-only transcript — Pi may still be mid-write to the last
        // line when this is read. Skipped, not fatal.
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if record.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let Some(message) = record.get("message") else {
            continue;
        };
        let Some(usage) = message.get("usage") else {
            continue;
        };

        input_tokens += usage.get("input").and_then(Value::as_u64).unwrap_or(0);
        output_tokens += usage.get("output").and_then(Value::as_u64).unwrap_or(0);
        seen_any = true;

        // The most recently seen model wins — a session's model can change
        // mid-transcript, and the terminal reading is what a run that ended
        // just now actually used, mirroring `RunCost::model`'s own rule.
        if let Some(name) = message.get("model").and_then(Value::as_str) {
            model = Some(name.to_string());
        }
    }

    if !seen_any {
        return None;
    }

    Some(CostSample {
        source: CostSource::Pi,
        model,
        input_tokens,
        output_tokens,
        // Pi's transcript carries no duration or context-window field (this
        // crate's module docs' "measured facts") — both stay `None` rather
        // than a fabricated value.
        duration_ms: None,
        context_utilization_percent: None,
    })
}

impl<A: HerdrAccess> Adapter for PiAdapter<A> {
    fn start(&self, req: &StartRequest) -> Result<StartedSession, AdapterError> {
        let created = self.herdr.workspace_create(req.workspace.as_path())?;
        let pane = parse_created_pane(&created)?;

        let name = agent_name_for(req.session_id);
        if let Err(err) = self.start_pi_with_retry(&name, &pane, req.model.as_deref()) {
            // Nothing else will release a pane whose agent never started —
            // best-effort cleanup so a failed start does not leak a Herdr
            // pane. Its own failure is not reported: the caller already has
            // the more useful error from `agent_start` below.
            let _ = self.herdr.pane_close(&pane);
            return Err(err);
        }

        // Reuse `observe`'s own hook-authority and session-id parsing rather
        // than re-deriving it — ADR 0017 decision 2's degraded fallback is
        // `observe`'s rule already, not a second copy of it.
        let observed = self.observe(&pane)?;
        Ok(StartedSession {
            pane,
            harness_session_id: observed.harness_session_id,
            confidence: observed.confidence,
        })
    }

    fn send(&self, pane: &PaneId, task_id: uuid::Uuid, prompt: &str) -> Result<(), AdapterError> {
        // See the module docs' fourth rule: a pane already `working` cannot
        // safely confirm a *new* submission, so this never reaches
        // `agent_prompt` for one.
        let observed = self.observe(pane)?;
        if observed.harness_state == "working" {
            return Err(AdapterError::SessionBusy {
                pane: pane.0.clone(),
                harness_state: observed.harness_state,
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
        // "Herdr could not answer at all" — a stopped Herdr, an unknown pane —
        // is not an adapter failure. It is recorded as Unavailable so the
        // caller falls back to manual confirmation.
        let raw = match self.herdr.agent_get(pane) {
            Ok(raw) => raw,
            Err(_) => return Ok(unavailable_observation(pane)),
        };

        let payload: Value = serde_json::from_str(&raw)
            .map_err(|source| unreadable_agent_get(pane, source.to_string()))?;

        let agent = payload
            .get("result")
            .and_then(|result| result.get("agent"))
            .ok_or_else(|| unreadable_agent_get(pane, "missing `result.agent`"))?;

        let found = agent.get("agent").and_then(Value::as_str).unwrap_or("");
        if found != "pi" {
            return Err(AdapterError::WrongHarness {
                pane: pane.0.clone(),
                found: found.to_string(),
                help: "point the adapter at a pane running `pi`, or use the adapter built for \
                       that harness instead"
                    .to_string(),
            });
        }

        let agent_status = agent
            .get("agent_status")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let transcript_path = agent
            .get("agent_session")
            .and_then(|session| session.get("value"))
            .and_then(Value::as_str)
            .map(PathBuf::from);
        let session_alive = agent
            .get("interactive_ready")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let harness_session_id = transcript_path
            .as_deref()
            .and_then(parse_harness_session_id);

        // Confidence comes from whether Herdr answered `agent explain` at
        // all, and whether its answer carries hook authority — never from
        // the status string itself.
        let mut confidence = match self.herdr.agent_explain(pane) {
            Err(_) => Confidence::Unavailable,
            Ok(text) if explain_has_hook_authority(&text) => Confidence::Authoritative,
            Ok(_) => Confidence::Degraded,
        };

        // `unknown`, or anything this adapter does not recognise, means
        // Herdr explicitly told us it has no information — that overrides
        // even a present hook-authority marker.
        let is_known_status = matches!(
            agent_status.as_str(),
            "working" | "idle" | "blocked" | "done"
        );
        if !is_known_status {
            confidence = Confidence::Unavailable;
        }

        let mut task_signal = match agent_status.as_str() {
            "working" => TaskSignal::Running,
            "blocked" => TaskSignal::Blocked(UNSPECIFIED_BLOCK_REASON),
            // idle, done, unknown, and any unrecognised status all leave the
            // task alone — see the module docs and ADR 0017 decision 4.
            _ => TaskSignal::NoChange,
        };
        // A degraded or unavailable reading must never move a task, whatever
        // the status string says.
        if confidence != Confidence::Authoritative {
            task_signal = TaskSignal::NoChange;
        }

        Ok(Observation {
            pane: pane.clone(),
            harness_state: agent_status,
            confidence,
            session_alive,
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
        // Reuse `observe`'s own transcript-path resolution rather than
        // re-deriving it — the same reasoning `start` already uses for
        // reusing `observe`'s hook-authority parsing.
        let observed = self.observe(pane)?;
        let Some(transcript_path) = observed.transcript_path else {
            // No transcript at all: Herdr unreachable, pane unknown, or a
            // live reading with no `agent_session.value`. ADR 0021 decision
            // 6: `None` is the ordinary answer here, not the edge.
            return Ok(None);
        };

        let jsonl = std::fs::read_to_string(&transcript_path).map_err(|source| {
            AdapterError::UnreadableOutput {
                command: format!("read {}", transcript_path.display()),
                detail: source.to_string(),
                help: "confirm the Pi transcript Herdr reported still exists and is readable"
                    .to_string(),
            }
        })?;

        Ok(sum_pi_transcript_usage(&jsonl))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error("cannot run {binary}: {source}\n  help: {help}")]
    RuntimeUnavailable {
        binary: PathBuf,
        help: String,
        #[source]
        source: std::io::Error,
    },

    #[error("{command} returned unreadable output: {detail}\n  help: {help}")]
    UnreadableOutput {
        command: String,
        detail: String,
        help: String,
    },

    #[error("pane {pane} runs `{found}`, not `pi`\n  help: {help}")]
    WrongHarness {
        pane: String,
        found: String,
        help: String,
    },

    /// The prompt may have reached `pane` — the keystrokes may have landed —
    /// but nothing confirmed the harness started working on it. Distinct from
    /// [`Self::RuntimeUnavailable`] and [`Self::UnreadableOutput`] on
    /// purpose: the caller has already journalled the delivery attempt
    /// (design §5 step 3) before calling `send`, so "never sent" and "sent,
    /// outcome unknown" must never collapse into one variant.
    #[error("prompt to {pane} was not confirmed: {detail}\n  help: {help}")]
    SubmissionUnconfirmed {
        pane: String,
        detail: String,
        help: String,
    },

    /// `send` refused: `pane` is already `working` a turn, and `herdr agent
    /// prompt --wait --until working` cannot distinguish confirming a new
    /// submission from matching the turn already in flight. Nothing was
    /// submitted — unlike [`Self::SubmissionUnconfirmed`], there is no
    /// ambiguity about whether keystrokes landed, so this carries no
    /// at-most-once delivery risk on its own.
    #[error("pane {pane} is still {harness_state}\n  help: {help}")]
    SessionBusy {
        pane: String,
        harness_state: String,
        help: String,
    },
}

/// The backlog §10 contract every harness adapter must satisfy: start, send,
/// observe, interrupt, stop, failed start, and one-task-per-session, run
/// against [`PiAdapter`] in this crate's own `tests/contract_pi.rs`.
///
/// This module is exported — not kept as a private test helper — because the
/// alternative is duplication. Backlog §10 requires that "Pi and Claude Code
/// pass the same contract tests," and a second harness adapter re-deriving
/// "does `send` refuse a busy pane," "does a failed `start` return `Err`
/// rather than a fabricated session" in its own words is exactly the kind of
/// drift ADR 0011 and ADR 0017 both record this codebase paying for once
/// already, for observation alone. One suite, run against every `Adapter`
/// impl through [`ContractFixture`], is the only way "the same contract
/// tests" stays true rather than becoming two suites that happen to agree
/// today and diverge the first time either one is edited.
pub mod contract {
    use super::{Adapter, AdapterError, StartRequest};

    /// What [`run_contract_suite`] needs from a test's own fixture: an
    /// adapter, plus one [`StartRequest`] guaranteed to succeed and one
    /// guaranteed to fail.
    ///
    /// Deliberately minimal. Everything else the suite needs — a live
    /// session, a busy one, a stopped one — it derives itself by calling the
    /// adapter, so a fixture cannot shortcut the state transitions the suite
    /// exists to check by pre-seeding them.
    pub trait ContractFixture {
        type A: Adapter;

        /// The adapter under test.
        fn adapter(&self) -> &Self::A;

        /// A `StartRequest` this fixture guarantees `start` accepts.
        fn startable(&self) -> StartRequest;

        /// A `StartRequest` this fixture guarantees `start` rejects —
        /// backlog §10's "failed start".
        fn unstartable(&self) -> StartRequest;
    }

    /// Runs backlog §10's seven scenarios — start, send, observe, interrupt,
    /// stop, failed start, and one-task-per-session — against one
    /// `ContractFixture`. Later scenarios build on the one live session the
    /// earlier ones proved, rather than each starting its own.
    ///
    /// Panics (via the usual `assert!`/`expect`) on the first violated claim,
    /// so this is meant to be called from inside a `#[test]` function.
    pub fn run_contract_suite<F: ContractFixture>(fixture: &F) {
        let adapter = fixture.adapter();

        // Failed start: `Err`, never a fabricated session.
        adapter
            .start(&fixture.unstartable())
            .expect_err("an unstartable request must return Err, not a fabricated StartedSession");

        // Start: a startable request produces a session `observe` can read
        // back.
        let started = adapter
            .start(&fixture.startable())
            .expect("a startable request must succeed");

        // Observe: reports the pane it was asked about.
        let observed = adapter
            .observe(&started.pane)
            .expect("observe must succeed for a pane this adapter just started");
        assert_eq!(
            observed.pane, started.pane,
            "observe must report the pane it was asked about, not a substitute"
        );

        // Send: a freshly started, idle session confirms.
        adapter
            .send(&started.pane, task_uuid(1), "first task")
            .expect("send to a freshly started, idle session must confirm and return Ok");

        // One-task-per-session: a second send while the first is still
        // `working` is refused, never falsely confirmed.
        let busy = adapter
            .send(&started.pane, task_uuid(2), "second task")
            .expect_err(
                "a session still working its first task must refuse a second send, not risk a \
                 false confirmation",
            );
        assert!(
            matches!(busy, AdapterError::SessionBusy { .. }),
            "a busy session's refusal must be AdapterError::SessionBusy, not a generic error: \
             {busy:?}"
        );

        // Interrupt: succeeds against the live, busy session.
        adapter
            .interrupt(&started.pane)
            .expect("interrupt must succeed against a live session");

        // Stop: ends the session; a later observe must show it gone.
        adapter
            .stop(&started.pane)
            .expect("stop must succeed against a live session");
        let after_stop = adapter
            .observe(&started.pane)
            .expect("observe must still answer for a pane whose session was stopped");
        assert!(
            !after_stop.session_alive,
            "a stopped session must no longer be reported alive"
        );
    }

    /// A deterministic task id, distinct per `seed`. `uuid` is pinned
    /// workspace-wide without the `v4` feature (see this workspace's other
    /// `uid`-style test helpers), so the suite cannot call `Uuid::new_v4`.
    fn task_uuid(seed: u8) -> uuid::Uuid {
        uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::rc::Rc;

    use super::*;

    type Resp = Result<String, AdapterError>;

    /// A `HerdrAccess` built from recorded payloads instead of a terminal
    /// multiplexer. `HerdrCli` itself is never exercised by these tests — it
    /// runs the real `herdr` binary, which this crate's test suite must not
    /// touch.
    ///
    /// Each new command added for the full adapter contract gets its own
    /// closure field with an inert-but-valid default, so existing `observe`
    /// tests built before those commands existed keep compiling unchanged.
    type PaneFn = Box<dyn Fn(&PaneId) -> Resp>;
    type AgentStartFn = Box<dyn Fn(&str, &str, &PaneId, &[String]) -> Resp>;
    type AgentPromptFn = Box<dyn Fn(&PaneId, &str) -> Resp>;

    struct FakeHerdr {
        get: PaneFn,
        explain: PaneFn,
        version: Box<dyn Fn() -> Resp>,
        workspace_create: Box<dyn Fn(&Path) -> Resp>,
        agent_start: AgentStartFn,
        agent_prompt: AgentPromptFn,
        agent_interrupt: PaneFn,
        pane_close: PaneFn,
    }

    impl FakeHerdr {
        fn new(
            get: impl Fn(&PaneId) -> Resp + 'static,
            explain: impl Fn(&PaneId) -> Resp + 'static,
        ) -> Self {
            Self {
                get: Box::new(get),
                explain: Box::new(explain),
                version: Box::new(|| Ok("herdr 0.8.0\n".to_string())),
                workspace_create: Box::new(|_| {
                    Ok(r#"{"result":{"root_pane":{"pane_id":"wZ:p1"}}}"#.to_string())
                }),
                agent_start: Box::new(|_, _, _, _| {
                    Ok(r#"{"result":{"type":"agent_started"}}"#.to_string())
                }),
                agent_prompt: Box::new(|_, _| Ok(r#"{"result":{"type":"ok"}}"#.to_string())),
                agent_interrupt: Box::new(|_| Ok(r#"{"result":{"type":"ok"}}"#.to_string())),
                pane_close: Box::new(|_| Ok(r#"{"result":{"type":"ok"}}"#.to_string())),
            }
        }

        fn with_version(mut self, version: impl Fn() -> Resp + 'static) -> Self {
            self.version = Box::new(version);
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
            (self.get)(pane)
        }
        fn agent_explain(&self, pane: &PaneId) -> Resp {
            (self.explain)(pane)
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

    /// A closure that ignores the pane and always returns the same body —
    /// the common case, one fixture answering every call.
    fn always(body: String) -> impl Fn(&PaneId) -> Resp {
        move |_| Ok(body.clone())
    }

    /// A closure that dispatches on pane id, for tests that must prove two
    /// panes are never conflated.
    fn keyed(responses: HashMap<String, String>) -> impl Fn(&PaneId) -> Resp {
        move |pane| {
            responses
                .get(&pane.0)
                .cloned()
                .ok_or_else(|| AdapterError::UnreadableOutput {
                    command: format!("herdr agent get {}", pane.0),
                    detail: "test fixture has no response for this pane".to_string(),
                    help: "fix the test".to_string(),
                })
        }
    }

    fn fixture(name: &str) -> String {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/herdr");
        std::fs::read_to_string(format!("{dir}/{name}"))
            .unwrap_or_else(|e| panic!("read fixture {name}: {e}"))
    }

    fn pi_get() -> String {
        fixture("agent-get-pi-authoritative.json")
    }

    fn pi_explain() -> String {
        fixture("agent-explain-pi-authoritative.txt")
    }

    /// One `{"type":"message", ...}` transcript line shaped exactly like
    /// this crate's module docs' "measured facts" for Pi, `cacheRead`/
    /// `cacheWrite`/`reasoning`/`totalTokens` included so tests can prove
    /// they are read nowhere.
    fn pi_message_line(input: u64, output: u64, model: &str) -> String {
        serde_json::json!({
            "type": "message",
            "message": {
                "usage": {
                    "input": input,
                    "output": output,
                    "cacheRead": 1_408,
                    "cacheWrite": 0,
                    "reasoning": 0,
                    "totalTokens": input + output + 1_408
                },
                "model": model
            }
        })
        .to_string()
    }

    /// Builds a fresh `agent get` payload for a `pi` pane with `agent_status`
    /// overwritten, by editing the real fixture's JSON in memory rather than
    /// hand-writing a payload — so the surrounding shape stays real.
    fn pi_get_with_status(status: &str) -> String {
        let mut value: Value = serde_json::from_str(&pi_get()).expect("fixture is valid JSON");
        value["result"]["agent"]["agent_status"] = Value::String(status.to_string());
        value.to_string()
    }

    /// Wraps a single `agent-list.json` entry in the shape `agent get`
    /// actually returns: `{"id":..., "result":{"agent":{...},"type":"agent_info"}}`.
    fn wrap_as_agent_get(agent: Value) -> String {
        serde_json::json!({
            "id": "cli:agent:get",
            "result": { "agent": agent, "type": "agent_info" },
        })
        .to_string()
    }

    fn agent_list_entries() -> Vec<Value> {
        let list: Value =
            serde_json::from_str(&fixture("agent-list.json")).expect("fixture is valid JSON");
        list["result"]["agents"]
            .as_array()
            .expect("agent-list.json has result.agents")
            .clone()
    }

    fn find_pane(entries: &[Value], pane_id: &str) -> Value {
        entries
            .iter()
            .find(|entry| entry["pane_id"] == pane_id)
            .unwrap_or_else(|| panic!("fixture is missing pane {pane_id}"))
            .clone()
    }

    fn herdr_not_running() -> AdapterError {
        AdapterError::RuntimeUnavailable {
            binary: PathBuf::from("herdr"),
            help: "install Herdr and ensure the `herdr` binary is on PATH, or pass its full \
                   path to `HerdrCli::new`"
                .to_string(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "no such file or directory"),
        }
    }

    /// A deterministic, distinct uuid — mirrors the `uid` helper other
    /// crates in this workspace use for the same reason: `uuid` is pinned
    /// without the `v4` feature.
    fn uid(seed: u32) -> uuid::Uuid {
        uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
    }

    // 1. The real Pi fixture yields Authoritative, the correct harness_state,
    //    the transcript path, and a parsed UUID.
    #[test]
    fn real_pi_fixture_is_authoritative() {
        let herdr = FakeHerdr::new(always(pi_get()), always(pi_explain()));
        let adapter = PiAdapter::new(herdr);
        let pane = PaneId("wE:p1".to_string());

        let obs = adapter
            .observe(&pane)
            .expect("real pi fixture must observe cleanly");

        assert_eq!(obs.pane, pane);
        assert_eq!(obs.confidence, Confidence::Authoritative);
        assert_eq!(obs.harness_state, "idle");
        // idle never moves a task, even when the reading is authoritative.
        assert_eq!(obs.task_signal, TaskSignal::NoChange);
        assert!(obs.session_alive);
        assert_eq!(
            obs.transcript_path.as_deref(),
            Some(Path::new(
                "/Users/operator/.pi/agent/sessions/--Users-operator-example-company-projects-factory--/2026-09-03T18-51-41-499Z_00000000-0000-0000-0000-000000000000.jsonl"
            ))
        );
        assert_eq!(
            obs.harness_session_id.as_deref(),
            Some("00000000-0000-0000-0000-000000000000")
        );
    }

    // 2. The real claude fixture yields AdapterError::WrongHarness.
    #[test]
    fn real_claude_fixture_is_wrong_harness() {
        let herdr = FakeHerdr::new(always(fixture("agent-get-claude.json")), |_| {
            unreachable!("agent_explain must not be called once the harness is wrong")
        });
        let adapter = PiAdapter::new(herdr);
        let pane = PaneId("wD:p1".to_string());

        let err = adapter
            .observe(&pane)
            .expect_err("a claude pane must not be accepted as pi");

        match err {
            AdapterError::WrongHarness { pane, found, .. } => {
                assert_eq!(pane, "wD:p1");
                assert_eq!(found, "claude");
            }
            other => panic!("expected WrongHarness, got {other:?}"),
        }
    }

    // 3. Each of the five agent_status values maps as ADR 0017 decision 4
    //    says, built by editing the fixture JSON in memory.
    #[test]
    fn agent_status_maps_to_task_signal_per_table() {
        let cases = [
            ("working", TaskSignal::Running),
            ("idle", TaskSignal::NoChange),
            ("blocked", TaskSignal::Blocked(BlockedReason::Clarification)),
            ("done", TaskSignal::NoChange),
            ("unknown", TaskSignal::NoChange),
        ];

        for (status, expected_signal) in cases {
            let herdr = FakeHerdr::new(always(pi_get_with_status(status)), always(pi_explain()));
            let adapter = PiAdapter::new(herdr);

            let obs = adapter
                .observe(&PaneId("wE:p1".to_string()))
                .unwrap_or_else(|e| panic!("status {status} must observe cleanly: {e}"));

            assert_eq!(obs.harness_state, status);
            assert_eq!(obs.task_signal, expected_signal, "status {status}");
        }

        // "unknown" is recognised but explicitly carries no information, so
        // even with the hook-authority marker present, confidence must not
        // be Authoritative.
        let herdr = FakeHerdr::new(always(pi_get_with_status("unknown")), always(pi_explain()));
        let obs = PiAdapter::new(herdr)
            .observe(&PaneId("wE:p1".to_string()))
            .expect("unknown status must still observe cleanly");
        assert_eq!(obs.confidence, Confidence::Unavailable);

        // Any other unrecognised string behaves the same way as "unknown".
        let herdr = FakeHerdr::new(
            always(pi_get_with_status("frobnicated")),
            always(pi_explain()),
        );
        let obs = PiAdapter::new(herdr)
            .observe(&PaneId("wE:p1".to_string()))
            .expect("an unrecognised status must still observe cleanly");
        assert_eq!(obs.confidence, Confidence::Unavailable);
        assert_eq!(obs.task_signal, TaskSignal::NoChange);
    }

    // 4. Missing hook marker => Degraded, and task_signal == NoChange even
    //    when the status is "working". The most important test in the crate.
    #[test]
    fn missing_hook_marker_degrades_and_never_moves_a_task() {
        let explain_without_marker: String = pi_explain()
            .lines()
            .filter(|line| !line.starts_with("screen_detection_skip_reason"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !explain_without_marker.contains("full_lifecycle_hook_authority"),
            "test setup must actually remove the marker"
        );

        let herdr = FakeHerdr::new(
            always(pi_get_with_status("working")),
            always(explain_without_marker),
        );
        let obs = PiAdapter::new(herdr)
            .observe(&PaneId("wE:p1".to_string()))
            .expect("a degraded reading must still observe cleanly");

        assert_eq!(obs.confidence, Confidence::Degraded);
        assert_eq!(obs.harness_state, "working");
        assert_eq!(
            obs.task_signal,
            TaskSignal::NoChange,
            "a degraded reading must never move a task, whatever the status string says"
        );
    }

    // 5. Herdr unavailable (fake returns Err) => Confidence::Unavailable, no
    //    panic, and an error message naming an action.
    #[test]
    fn herdr_unavailable_is_recorded_not_raised() {
        let message = herdr_not_running().to_string();
        assert!(
            message.contains("help:"),
            "error must carry a help line: {message}"
        );
        assert!(
            message.contains("install") || message.contains("PATH"),
            "help must name a corrective action: {message}"
        );

        let herdr = FakeHerdr::new(
            |_| Err(herdr_not_running()),
            |_| unreachable!("agent_explain must not be called once agent_get has already failed"),
        );
        let adapter = PiAdapter::new(herdr);

        let obs = adapter.observe(&PaneId("wE:p1".to_string())).expect(
            "a stopped Herdr must not fail observe() — it falls back to manual confirmation",
        );

        assert_eq!(obs.confidence, Confidence::Unavailable);
        assert_eq!(obs.task_signal, TaskSignal::NoChange);
        assert!(!obs.session_alive);
        assert!(obs.transcript_path.is_none());
        assert!(obs.harness_session_id.is_none());
    }

    // 6. A transcript path that does not match the expected filename shape
    //    => harness_session_id: None, and the observation is still returned.
    #[test]
    fn unrecognisable_transcript_filename_yields_no_session_id() {
        let mut value: Value = serde_json::from_str(&pi_get()).expect("fixture is valid JSON");
        value["result"]["agent"]["agent_session"]["value"] = Value::String(
            "/Users/operator/.pi/agent/sessions/not-a-recognisable-name.jsonl".to_string(),
        );
        let get_body = value.to_string();

        let herdr = FakeHerdr::new(always(get_body), always(pi_explain()));
        let obs = PiAdapter::new(herdr)
            .observe(&PaneId("wE:p1".to_string()))
            .expect("an unrecognisable transcript name must still observe cleanly");

        assert!(obs.harness_session_id.is_none());
        // The path itself is still recorded verbatim — only the derived UUID
        // is withheld.
        assert_eq!(
            obs.transcript_path.as_deref(),
            Some(Path::new(
                "/Users/operator/.pi/agent/sessions/not-a-recognisable-name.jsonl"
            ))
        );
    }

    // 7. Two Pi sessions of one agent in the same workspace produce two
    //    distinct observations keyed by pane, using the real
    //    agent-list.json fixture: `assistant` (w6:p1) and `assistant-chat`
    //    (w6:p4), the two live sessions ADR 0017 decision 3 names for
    //    `assistant`'s `max_sessions: 2`. Their reported `cwd` differs by a
    //    `/chat` subdirectory rather than being byte-identical — the closest
    //    this fixture set gets to "two sessions of one agent sharing a
    //    workspace" — see the Slice 5 report.
    #[test]
    fn two_pi_sessions_of_one_scope_are_never_confused() {
        let entries = agent_list_entries();
        let assistant = find_pane(&entries, "w6:p1");
        let assistant_chat = find_pane(&entries, "w6:p4");
        assert_eq!(assistant["agent"], "pi");
        assert_eq!(assistant_chat["agent"], "pi");

        let mut responses = HashMap::new();
        responses.insert("w6:p1".to_string(), wrap_as_agent_get(assistant));
        responses.insert("w6:p4".to_string(), wrap_as_agent_get(assistant_chat));

        // Both list entries report `screen_detection_skipped: true`; the
        // only real `agent explain` capture we have is authoritative, and
        // stands in for both panes' explain output.
        let herdr = FakeHerdr::new(keyed(responses), always(pi_explain()));
        let adapter = PiAdapter::new(herdr);

        let a = adapter
            .observe(&PaneId("w6:p1".to_string()))
            .expect("the assistant pane must observe cleanly");
        let b = adapter
            .observe(&PaneId("w6:p4".to_string()))
            .expect("the assistant-chat pane must observe cleanly");

        assert_ne!(a.pane, b.pane);
        assert_eq!(a.confidence, Confidence::Authoritative);
        assert_eq!(b.confidence, Confidence::Authoritative);
        assert_ne!(
            a.transcript_path, b.transcript_path,
            "two live pi sessions must never collapse onto one transcript"
        );
        // Each fixture session carries its own synthetic UUID, so the strongest
        // form of this assertion is available: the two sessions must not
        // collapse onto one identity. Asserting a constant here would pass just
        // as well if `observe` returned the same session twice, which is the
        // defect this test exists to catch.
        assert!(a.harness_session_id.is_some() && b.harness_session_id.is_some());
        assert_ne!(
            a.harness_session_id, b.harness_session_id,
            "two live pi sessions must never resolve to one harness session id"
        );
    }

    // A second, complementary case: two panes that share a byte-identical
    // cwd — one pi, one not — proving the harness check keys off the pane's
    // own data rather than any shared cwd. (This alone would not satisfy
    // criterion 7, since a WrongHarness rejection never reaches per-session
    // state; see the test above for that.)
    #[test]
    fn two_panes_sharing_a_cwd_are_never_confused() {
        let entries = agent_list_entries();
        let pi_entry = find_pane(&entries, "wE:p1");
        let claude_entry = find_pane(&entries, "wE:p2");

        // The premise this test relies on: these two real panes share a
        // working directory, and one is pi while the other is not.
        assert_eq!(pi_entry["cwd"], claude_entry["cwd"]);
        assert_eq!(pi_entry["agent"], "pi");
        assert_eq!(claude_entry["agent"], "claude");

        let mut responses = HashMap::new();
        responses.insert("wE:p1".to_string(), wrap_as_agent_get(pi_entry));
        responses.insert("wE:p2".to_string(), wrap_as_agent_get(claude_entry));

        let herdr = FakeHerdr::new(keyed(responses), always(pi_explain()));
        let adapter = PiAdapter::new(herdr);

        let pi_obs = adapter
            .observe(&PaneId("wE:p1".to_string()))
            .expect("the pi pane must observe cleanly despite sharing a cwd with a claude pane");
        assert_eq!(pi_obs.pane, PaneId("wE:p1".to_string()));
        assert_eq!(pi_obs.harness_state, "idle");

        let claude_err = adapter
            .observe(&PaneId("wE:p2".to_string()))
            .expect_err("the claude pane sharing that cwd must not be read as pi");
        match claude_err {
            AdapterError::WrongHarness { pane, found, .. } => {
                assert_eq!(pane, "wE:p2");
                assert_eq!(found, "claude");
            }
            other => panic!("expected WrongHarness, got {other:?}"),
        }
    }

    // 8. Malformed JSON => UnreadableOutput, not a panic.
    #[test]
    fn malformed_json_is_unreadable_output_not_a_panic() {
        let herdr = FakeHerdr::new(always("not json at all {".to_string()), |_| {
            unreachable!("agent_explain must not be called once agent_get is unreadable")
        });
        let adapter = PiAdapter::new(herdr);

        let err = adapter
            .observe(&PaneId("wE:p1".to_string()))
            .expect_err("malformed JSON must be reported, not panicked on");

        assert!(
            matches!(err, AdapterError::UnreadableOutput { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn runtime_version_returns_the_trimmed_herdr_version() {
        let herdr = FakeHerdr::new(always(pi_get()), always(pi_explain()))
            .with_version(|| Ok(fixture("version.txt")));
        let adapter = PiAdapter::new(herdr);

        let version = adapter.runtime_version().expect("version must be readable");

        assert_eq!(version, "herdr 0.8.0");
    }

    // --- Pure argv builders -------------------------------------------

    #[test]
    fn workspace_create_args_passes_cwd_and_never_steals_focus() {
        let args = workspace_create_args(Path::new("/tmp/ws"));
        assert_eq!(
            args,
            ["workspace", "create", "--cwd", "/tmp/ws", "--no-focus"]
        );
    }

    #[test]
    fn agent_start_args_places_extension_after_a_bare_double_dash() {
        let pane = PaneId("wE:p1".to_string());
        let extra = vec!["--extension".to_string(), "/x.ts".to_string()];
        let args = agent_start_args("factory-abcd1234", "pi", &pane, &extra);
        assert_eq!(
            args,
            [
                "agent",
                "start",
                "factory-abcd1234",
                "--kind",
                "pi",
                "--pane",
                "wE:p1",
                "--",
                "--extension",
                "/x.ts",
            ]
        );
    }

    #[test]
    fn agent_start_args_omits_the_double_dash_with_no_extra_args() {
        let pane = PaneId("wE:p1".to_string());
        let args = agent_start_args("factory-abcd1234", "pi", &pane, &[]);
        assert!(
            !args.contains(&"--".to_string()),
            "no extra args means no bare `--` at all: {args:?}"
        );
    }

    #[test]
    fn prompt_args_waits_until_working() {
        let pane = PaneId("wE:p1".to_string());
        let args = prompt_args(&pane, "[task x] do it");
        assert_eq!(
            args,
            [
                "agent",
                "prompt",
                "wE:p1",
                "[task x] do it",
                "--wait",
                "--until",
                "working",
            ]
        );
    }

    #[test]
    fn interrupt_args_send_ctrl_c() {
        let pane = PaneId("wE:p1".to_string());
        assert_eq!(
            interrupt_args(&pane),
            ["agent", "send-keys", "wE:p1", "ctrl+c"]
        );
    }

    #[test]
    fn pane_close_args_close_the_right_pane() {
        let pane = PaneId("wE:p1".to_string());
        assert_eq!(pane_close_args(&pane), ["pane", "close", "wE:p1"]);
    }

    #[test]
    fn attach_argv_execs_herdr_agent_attach() {
        let pane = PaneId("wE:p1".to_string());
        assert_eq!(attach_argv(&pane), ["herdr", "agent", "attach", "wE:p1"]);
    }

    #[test]
    fn pi_extension_args_names_the_herdr_lifecycle_hook() {
        let args = pi_extension_args();
        assert_eq!(args[0], "--extension");
        assert!(
            args[1].ends_with("/.pi/agent/extensions/herdr-agent-state.ts"),
            "got {args:?}"
        );
    }

    #[test]
    fn pi_args_appends_the_model_when_the_scope_named_one() {
        let args = pi_args(Some("business-factory-qwen3.8/qwen3.8-27b"));
        let position = args
            .iter()
            .position(|a| a == "--model")
            .expect("--model must be present");
        assert_eq!(
            args[position + 1],
            "business-factory-qwen3.8/qwen3.8-27b",
            "the value must follow its flag, unmodified: {args:?}"
        );
        assert!(
            args.contains(&"--extension".to_string()),
            "the lifecycle hook must survive the model being added: {args:?}"
        );
    }

    #[test]
    fn pi_args_passes_no_model_flag_at_all_when_none_was_named() {
        // Not an empty `--model`: Pi's own settings carry a `defaultModel`,
        // and an empty flag would override it with nothing (ADR 0024).
        let args = pi_args(None);
        assert!(
            !args.iter().any(|a| a == "--model"),
            "an unnamed model must leave the flag off entirely: {args:?}"
        );
    }

    #[test]
    fn pi_args_always_approve_the_workspaces_own_project_config() {
        for model in [None, Some("provider/some-model")] {
            let args = pi_args(model);
            assert!(
                args.iter().any(|a| a == "--approve"),
                "without this, a provider registered by the scope's own \
                 extension never loads and `model:` cannot resolve: {args:?}"
            );
        }
    }

    #[test]
    fn agent_name_for_is_a_valid_unique_herdr_name() {
        let a = agent_name_for(uid(1));
        let b = agent_name_for(uid(2));
        assert_ne!(a, b, "two different sessions must get two different names");
        for name in [&a, &b] {
            assert!(name.len() <= 32, "{name} exceeds Herdr's 32-char limit");
            let mut chars = name.chars();
            let first = chars.next().expect("non-empty name");
            assert!(first.is_ascii_lowercase(), "{name} must start with a-z");
            assert!(
                chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-'),
                "{name} has a character outside [a-z0-9_-]"
            );
        }
    }

    #[test]
    fn classify_prompt_failure_maps_stalled_and_timeout_to_submission_unconfirmed() {
        // Code names measured from `herdr agent prompt --help`; the envelope
        // is inferred from the shape measured live for other Herdr errors on
        // this machine (see `classify_prompt_failure`'s doc comment) — not
        // captured for these two outcomes specifically.
        let stalled = r#"{"error":{"code":"agent_prompt_stalled","message":"no state change observed within 5000ms"},"id":"cli:agent:prompt"}"#;
        let err = classify_prompt_failure("wE:p1", stalled);
        assert!(
            matches!(err, AdapterError::SubmissionUnconfirmed { .. }),
            "got {err:?}"
        );

        let timeout = r#"{"error":{"code":"timeout","message":"exceeded --timeout"},"id":"cli:agent:prompt"}"#;
        let err = classify_prompt_failure("wE:p1", timeout);
        assert!(
            matches!(err, AdapterError::SubmissionUnconfirmed { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn classify_prompt_failure_maps_other_errors_to_unreadable_output() {
        // Measured live on this machine: `herdr agent prompt <pane>
        // --wait --until working` against a pane holding no agent.
        let not_found = r#"{"error":{"code":"agent_not_found","message":"agent target wP:p1 not found"},"id":"cli:agent:prompt"}"#;
        let err = classify_prompt_failure("wP:p1", not_found);
        assert!(
            matches!(err, AdapterError::UnreadableOutput { .. }),
            "got {err:?}"
        );
    }

    // --- start / send / interrupt / stop / attach_command --------------

    #[test]
    fn attach_command_never_touches_herdr() {
        let herdr = FakeHerdr::new(
            |_| unreachable!("attach_command must never call agent_get"),
            |_| unreachable!("attach_command must never call agent_explain"),
        )
        .with_workspace_create(|_| unreachable!("attach_command must never call workspace_create"))
        .with_agent_start(|_, _, _, _| unreachable!("attach_command must never call agent_start"))
        .with_agent_prompt(|_, _| unreachable!("attach_command must never call agent_prompt"))
        .with_agent_interrupt(|_| unreachable!("attach_command must never call agent_interrupt"))
        .with_pane_close(|_| unreachable!("attach_command must never call pane_close"));
        let adapter = PiAdapter::new(herdr);

        let argv = adapter
            .attach_command(&PaneId("wE:p1".to_string()))
            .expect("attach_command is pure and infallible in practice");
        assert_eq!(argv, ["herdr", "agent", "attach", "wE:p1"]);
    }

    #[test]
    fn interrupt_sends_ctrl_c_to_the_right_pane() {
        let seen: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let seen_write = Rc::clone(&seen);
        let herdr = FakeHerdr::new(always(pi_get()), always(pi_explain())).with_agent_interrupt(
            move |pane| {
                *seen_write.borrow_mut() = Some(pane.0.clone());
                Ok(r#"{"result":{"type":"ok"}}"#.to_string())
            },
        );
        let adapter = PiAdapter::new(herdr);

        adapter
            .interrupt(&PaneId("wE:p1".to_string()))
            .expect("interrupt must succeed");
        assert_eq!(seen.borrow().as_deref(), Some("wE:p1"));
    }

    #[test]
    fn stop_closes_the_right_pane() {
        let seen: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let seen_write = Rc::clone(&seen);
        let herdr =
            FakeHerdr::new(always(pi_get()), always(pi_explain())).with_pane_close(move |pane| {
                *seen_write.borrow_mut() = Some(pane.0.clone());
                Ok(r#"{"result":{"type":"ok"}}"#.to_string())
            });
        let adapter = PiAdapter::new(herdr);

        adapter
            .stop(&PaneId("wE:p1".to_string()))
            .expect("stop must succeed");
        assert_eq!(seen.borrow().as_deref(), Some("wE:p1"));
    }

    #[test]
    fn send_submits_the_prompt_verbatim_without_adding_a_second_task_id() {
        let seen: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let seen_write = Rc::clone(&seen);
        let herdr = FakeHerdr::new(always(pi_get_with_status("idle")), always(pi_explain()))
            .with_agent_prompt(move |_, text| {
                *seen_write.borrow_mut() = Some(text.to_string());
                Ok(r#"{"result":{"type":"ok"}}"#.to_string())
            });
        let adapter = PiAdapter::new(herdr);
        let task_id = uid(42);

        adapter
            .send(&PaneId("wE:p1".to_string()), task_id, "do the thing")
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
    fn send_refuses_a_working_pane_without_submitting_anything() {
        let herdr = FakeHerdr::new(always(pi_get_with_status("working")), always(pi_explain()))
            .with_agent_prompt(|_, _| {
                unreachable!("send must refuse before ever calling agent_prompt on a working pane")
            });
        let adapter = PiAdapter::new(herdr);

        let err = adapter
            .send(&PaneId("wE:p1".to_string()), uid(1), "second task")
            .expect_err("a working pane must refuse a new send");

        match err {
            AdapterError::SessionBusy { harness_state, .. } => {
                assert_eq!(harness_state, "working");
            }
            other => panic!("expected SessionBusy, got {other:?}"),
        }
    }

    #[test]
    fn send_propagates_submission_unconfirmed_from_herdr() {
        let herdr = FakeHerdr::new(always(pi_get_with_status("idle")), always(pi_explain()))
            .with_agent_prompt(|pane, _| {
                Err(AdapterError::SubmissionUnconfirmed {
                    pane: pane.0.clone(),
                    detail: "fixture: stalled".to_string(),
                    help: "fix the test".to_string(),
                })
            });
        let adapter = PiAdapter::new(herdr);

        let err = adapter
            .send(&PaneId("wE:p1".to_string()), uid(1), "task")
            .expect_err("a stalled confirmation must not be reported as Ok");
        assert!(
            matches!(err, AdapterError::SubmissionUnconfirmed { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn start_creates_a_pane_and_starts_pi_in_it() {
        let seen_cwd: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
        let seen_cwd_write = Rc::clone(&seen_cwd);
        let seen_kind: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let seen_kind_write = Rc::clone(&seen_kind);

        let herdr = FakeHerdr::new(always(pi_get_with_status("idle")), always(pi_explain()))
            .with_workspace_create(move |cwd| {
                *seen_cwd_write.borrow_mut() = Some(cwd.to_path_buf());
                Ok(r#"{"result":{"root_pane":{"pane_id":"wQ:p1"}}}"#.to_string())
            })
            .with_agent_start(move |_, kind, pane, args| {
                *seen_kind_write.borrow_mut() = Some(kind.to_string());
                assert_eq!(
                    pane.0, "wQ:p1",
                    "agent_start must target the pane just created"
                );
                assert!(
                    args.iter().any(|a| a == "--extension"),
                    "the Herdr lifecycle hook extension must be passed: {args:?}"
                );
                Ok(r#"{"result":{"type":"agent_started"}}"#.to_string())
            });
        let adapter = PiAdapter::new(herdr);

        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = CanonicalPath::resolve(dir.path()).expect("resolve tempdir");
        let req = StartRequest {
            scope_id: uid(1),
            session_id: uid(2),
            workspace,
            generated_context: "company\nscope\nagent\n".to_string(),
            model: None,
        };

        let started = adapter.start(&req).expect("start must succeed");
        assert_eq!(started.pane, PaneId("wQ:p1".to_string()));
        // Compare against the *canonicalized* workspace path, not the
        // TempDir's raw one — on this machine `/tmp` resolves through
        // `/var`'s symlink to `/private/var`, and `CanonicalPath::resolve`
        // follows it, exactly as it is documented to.
        assert_eq!(seen_cwd.borrow().as_deref(), Some(req.workspace.as_path()));
        assert_eq!(seen_kind.borrow().as_deref(), Some("pi"));
    }

    #[test]
    fn start_reports_degraded_confidence_rather_than_failing_on_a_missing_hook_marker() {
        let explain_without_marker: String = pi_explain()
            .lines()
            .filter(|line| !line.starts_with("screen_detection_skip_reason"))
            .collect::<Vec<_>>()
            .join("\n");

        let herdr = FakeHerdr::new(
            always(pi_get_with_status("idle")),
            always(explain_without_marker),
        );
        let adapter = PiAdapter::new(herdr);

        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = CanonicalPath::resolve(dir.path()).expect("resolve tempdir");
        let req = StartRequest {
            scope_id: uid(1),
            session_id: uid(2),
            workspace,
            generated_context: String::new(),
            model: None,
        };

        let started = adapter
            .start(&req)
            .expect("a missing hook marker must not fail start (ADR 0017 decision 2)");
        assert_eq!(started.confidence, Confidence::Degraded);
    }

    #[test]
    fn start_propagates_a_failed_workspace_create() {
        let herdr =
            FakeHerdr::new(always(pi_get()), always(pi_explain())).with_workspace_create(|_| {
                Err(AdapterError::UnreadableOutput {
                    command: "herdr workspace create".to_string(),
                    detail: "fixture: refused".to_string(),
                    help: "fix the test".to_string(),
                })
            });
        let adapter = PiAdapter::new(herdr);

        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = CanonicalPath::resolve(dir.path()).expect("resolve tempdir");
        let req = StartRequest {
            scope_id: uid(1),
            session_id: uid(2),
            workspace,
            generated_context: String::new(),
            model: None,
        };

        adapter
            .start(&req)
            .expect_err("a refused workspace_create must fail start, not fabricate a session");
    }

    #[test]
    fn start_closes_the_pane_it_created_when_agent_start_fails() {
        let closed: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let closed_write = Rc::clone(&closed);

        let herdr = FakeHerdr::new(always(pi_get()), always(pi_explain()))
            .with_workspace_create(|_| {
                Ok(r#"{"result":{"root_pane":{"pane_id":"wQ:p1"}}}"#.to_string())
            })
            .with_agent_start(|_, _, _, _| {
                Err(AdapterError::UnreadableOutput {
                    command: "herdr agent start".to_string(),
                    detail: "fixture: pi not installed".to_string(),
                    help: "fix the test".to_string(),
                })
            })
            .with_pane_close(move |pane| {
                closed_write.borrow_mut().push(pane.0.clone());
                Ok(r#"{"result":{"type":"ok"}}"#.to_string())
            });
        let adapter = PiAdapter::new(herdr);

        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = CanonicalPath::resolve(dir.path()).expect("resolve tempdir");
        let req = StartRequest {
            scope_id: uid(1),
            session_id: uid(2),
            workspace,
            generated_context: String::new(),
            model: None,
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

    // Measured live on this machine (see `PiAdapter::start_pi_with_retry`'s
    // doc comment): a pane Herdr just created can briefly answer
    // `agent_pane_busy` to `agent start` before its shell reaches an
    // interactive prompt. `start` must ride that out rather than fail.
    #[test]
    fn start_retries_agent_start_on_a_transient_pane_busy_error() {
        let attempts: Rc<Cell<u32>> = Rc::new(Cell::new(0));
        let attempts_write = Rc::clone(&attempts);

        let herdr = FakeHerdr::new(always(pi_get_with_status("idle")), always(pi_explain()))
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
        let adapter = PiAdapter::new(herdr);

        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = CanonicalPath::resolve(dir.path()).expect("resolve tempdir");
        let req = StartRequest {
            scope_id: uid(1),
            session_id: uid(2),
            workspace,
            generated_context: String::new(),
            model: None,
        };

        adapter
            .start(&req)
            .expect("two transient agent_pane_busy failures must be ridden out, not surfaced");
        assert_eq!(
            attempts.get(),
            3,
            "must have retried exactly until the third attempt succeeded"
        );
    }

    #[test]
    fn start_does_not_retry_a_non_transient_agent_start_failure() {
        let attempts: Rc<Cell<u32>> = Rc::new(Cell::new(0));
        let attempts_write = Rc::clone(&attempts);

        let herdr = FakeHerdr::new(always(pi_get()), always(pi_explain()))
            .with_workspace_create(|_| {
                Ok(r#"{"result":{"root_pane":{"pane_id":"wQ:p1"}}}"#.to_string())
            })
            .with_agent_start(move |_, _, _, _| {
                attempts_write.set(attempts_write.get() + 1);
                Err(AdapterError::UnreadableOutput {
                    command: "herdr agent start".to_string(),
                    detail: "fixture: pi not installed".to_string(),
                    help: "fix the test".to_string(),
                })
            });
        let adapter = PiAdapter::new(herdr);

        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = CanonicalPath::resolve(dir.path()).expect("resolve tempdir");
        let req = StartRequest {
            scope_id: uid(1),
            session_id: uid(2),
            workspace,
            generated_context: String::new(),
            model: None,
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

    // --- cost_sample: Pi (design §12.6, ADR 0021 decisions 6-8) -----------

    // 1. A Pi transcript with three messages sums to the right totals.
    #[test]
    fn sum_pi_transcript_usage_sums_three_messages() {
        let jsonl = [
            pi_message_line(100, 50, "gpt-6-astra"),
            pi_message_line(200, 75, "gpt-6-astra"),
            pi_message_line(10, 5, "gpt-6-astra"),
        ]
        .join("\n");

        let sample =
            sum_pi_transcript_usage(&jsonl).expect("three usage records must sum, not None");
        assert_eq!(sample.source, CostSource::Pi);
        assert_eq!(sample.input_tokens, 310);
        assert_eq!(sample.output_tokens, 130);
        assert_eq!(sample.model.as_deref(), Some("gpt-6-astra"));
        assert!(
            sample.duration_ms.is_none(),
            "Pi's transcript carries no duration field"
        );
        assert!(
            sample.context_utilization_percent.is_none(),
            "Pi's transcript carries no context-window field"
        );
    }

    // 2. A Pi transcript with no usage records at all yields None, not
    //    zeros.
    #[test]
    fn sum_pi_transcript_usage_with_no_usage_records_is_none_not_zeros() {
        let jsonl = [
            r#"{"type":"tool_call","tool":"bash"}"#,
            r#"{"type":"message","message":{"model":"gpt-6-astra"}}"#,
        ]
        .join("\n");

        assert_eq!(sum_pi_transcript_usage(&jsonl), None);
    }

    // ADR 0021 decision 7's defect in Pi's own vocabulary: `usage.totalTokens`
    // is not `input + output` for one message (it also folds in cache and
    // reasoning tokens), so it — and cacheRead/cacheWrite/reasoning
    // themselves — must never contribute to `input_tokens`/`output_tokens`.
    #[test]
    fn sum_pi_transcript_usage_never_reads_total_tokens_cache_or_reasoning() {
        let line = serde_json::json!({
            "type": "message",
            "message": {
                "usage": {
                    "input": 10,
                    "output": 5,
                    "cacheRead": 5_000,
                    "cacheWrite": 5_000,
                    "reasoning": 5_000,
                    "totalTokens": 999_999
                },
                "model": "gpt-6-astra"
            }
        })
        .to_string();

        let sample = sum_pi_transcript_usage(&line).expect("one usage record must sum, not None");
        assert_eq!(sample.input_tokens, 10, "must come from usage.input alone");
        assert_eq!(sample.output_tokens, 5, "must come from usage.output alone");
    }

    #[test]
    fn pi_cost_sample_reads_the_transcript_herdr_reported_and_sums_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let transcript = dir.path().join("session.jsonl");
        std::fs::write(
            &transcript,
            [
                pi_message_line(100, 50, "gpt-6-astra"),
                pi_message_line(20, 10, "gpt-6-astra"),
            ]
            .join("\n"),
        )
        .expect("write fixture transcript");

        let mut value: Value = serde_json::from_str(&pi_get()).expect("fixture is valid JSON");
        value["result"]["agent"]["agent_session"]["value"] =
            Value::String(transcript.to_string_lossy().into_owned());
        let get_body = value.to_string();

        let herdr = FakeHerdr::new(always(get_body), always(pi_explain()));
        let adapter = PiAdapter::new(herdr);

        let sample = adapter
            .cost_sample(&PaneId("wE:p1".to_string()))
            .expect("a readable transcript must produce a sample")
            .expect("a transcript with usage records must not be None");

        assert_eq!(sample.source, CostSource::Pi);
        assert_eq!(sample.input_tokens, 120);
        assert_eq!(sample.output_tokens, 60);
        assert_eq!(sample.model.as_deref(), Some("gpt-6-astra"));
    }

    #[test]
    fn pi_cost_sample_is_none_when_agent_get_reports_no_transcript() {
        let mut value: Value = serde_json::from_str(&pi_get()).expect("fixture is valid JSON");
        value["result"]["agent"]["agent_session"] = Value::Null;
        let get_body = value.to_string();

        let herdr = FakeHerdr::new(always(get_body), always(pi_explain()));
        let adapter = PiAdapter::new(herdr);

        let sample = adapter
            .cost_sample(&PaneId("wE:p1".to_string()))
            .expect("a missing transcript path must not error");
        assert!(sample.is_none());
    }

    // --- CostSample::since (ADR 0021 decision 6) ---------------------------

    // 6. Subtracting a later sample from a baseline gives the run's own
    //    figures.
    #[test]
    fn cost_sample_since_subtracts_tokens_and_duration_and_keeps_the_later_readings() {
        let baseline = CostSample {
            source: CostSource::ClaudeCode,
            model: Some("claude-x".to_string()),
            input_tokens: 1_000,
            output_tokens: 400,
            duration_ms: Some(5_000),
            context_utilization_percent: Some(10.0),
        };
        let later = CostSample {
            source: CostSource::ClaudeCode,
            model: Some("claude-y".to_string()),
            input_tokens: 1_500,
            output_tokens: 620,
            duration_ms: Some(47_000),
            context_utilization_percent: Some(63.5),
        };

        let run = later
            .since(&baseline)
            .expect("same-source samples must not error")
            .expect("a later sample larger than baseline must produce a run figure");

        assert_eq!(run.input_tokens, 500);
        assert_eq!(run.output_tokens, 220);
        assert_eq!(run.duration_ms, Some(42_000));
        assert_eq!(
            run.model.as_deref(),
            Some("claude-y"),
            "the later sample's own model, not a diff"
        );
        assert_eq!(
            run.context_utilization_percent,
            Some(63.5),
            "context pressure is the later sample's own reading, never subtracted"
        );
    }

    #[test]
    fn cost_sample_since_refuses_to_subtract_across_sources() {
        let pi_sample = CostSample {
            source: CostSource::Pi,
            model: None,
            input_tokens: 100,
            output_tokens: 50,
            duration_ms: None,
            context_utilization_percent: None,
        };
        let claude_sample = CostSample {
            source: CostSource::ClaudeCode,
            model: None,
            input_tokens: 200,
            output_tokens: 90,
            duration_ms: None,
            context_utilization_percent: None,
        };

        let err = claude_sample
            .since(&pi_sample)
            .expect_err("a Claude Code sample must refuse to subtract a Pi baseline");
        assert!(
            matches!(err, CostSampleError::MismatchedSource { .. }),
            "got {err:?}"
        );
    }

    // 7. A later sample smaller than the baseline behaves the way this crate
    //    documents: Ok(None), never a negative token count. A Claude Code
    //    session compacting, or a Pi transcript being rotated, between the
    //    two samples are both real ways this happens — see
    //    `CostSample::since`'s own doc comment for why clamping to zero was
    //    rejected instead.
    #[test]
    fn cost_sample_since_reports_none_rather_than_a_negative_token_count() {
        let baseline = CostSample {
            source: CostSource::Pi,
            model: None,
            input_tokens: 5_000,
            output_tokens: 2_000,
            duration_ms: None,
            context_utilization_percent: None,
        };
        let later = CostSample {
            source: CostSource::Pi,
            model: None,
            input_tokens: 100, // smaller than baseline: transcript rotated
            output_tokens: 50,
            duration_ms: None,
            context_utilization_percent: None,
        };

        let run = later
            .since(&baseline)
            .expect("a smaller later sample is a documented condition, not an error");
        assert_eq!(run, None);
    }

    #[test]
    fn cost_sample_round_trips_through_its_string_form() {
        let sample = CostSample {
            source: CostSource::ClaudeCode,
            model: Some("claude-x".to_string()),
            input_tokens: 42,
            output_tokens: 7,
            duration_ms: Some(1_234),
            context_utilization_percent: Some(55.5),
        };

        let text = sample.to_string();
        let parsed: CostSample = text.parse().expect("what Display produces must parse back");
        assert_eq!(parsed, sample, "tasks.cost_baseline round-trips exactly");
    }

    /// The actual daemon path decision 6 exists for: a baseline is written to
    /// `tasks.cost_baseline` as text, read back after a restart, and *then*
    /// subtracted from a later sample. The round-trip test above only proves
    /// `to_string`/`parse` agree with each other — this proves the parsed
    /// value is still subtractable and produces the same figures a baseline
    /// held in memory across no restart at all would have.
    #[test]
    fn cost_sample_parsed_back_from_its_stored_string_is_still_subtractable() {
        let baseline = CostSample {
            source: CostSource::Pi,
            model: Some("gpt-6-astra".to_string()),
            input_tokens: 1_000,
            output_tokens: 400,
            duration_ms: None,
            context_utilization_percent: None,
        };
        let later = CostSample {
            source: CostSource::Pi,
            model: Some("gpt-6-astra".to_string()),
            input_tokens: 1_300,
            output_tokens: 480,
            duration_ms: None,
            context_utilization_percent: None,
        };

        // What a fresh daemon process does with `tasks.cost_baseline`: read
        // the TEXT column, parse it, subtract.
        let stored = baseline.to_string();
        let restored: CostSample = stored
            .parse()
            .expect("a baseline this crate wrote must parse back");

        let from_memory = later
            .since(&baseline)
            .expect("same-source subtraction must not error");
        let from_restart = later
            .since(&restored)
            .expect("subtracting a restart-restored baseline must not error");

        assert_eq!(
            from_memory, from_restart,
            "a baseline that survived a daemon restart must produce the same run figures as \
             one held in memory the whole time"
        );
    }

    #[test]
    fn cost_sample_from_str_reports_malformed_not_a_panic() {
        let err = "not json at all {"
            .parse::<CostSample>()
            .expect_err("garbage must not parse as a CostSample");
        assert!(
            matches!(err, CostSampleError::Malformed { .. }),
            "got {err:?}"
        );
    }

    // Duration is independently nullable: a duration that cannot be
    // subtracted must null out only that one field, not discard token counts
    // that are still trustworthy.
    #[test]
    fn cost_sample_since_nulls_duration_alone_when_only_one_side_has_one() {
        let baseline = CostSample {
            source: CostSource::ClaudeCode,
            model: None,
            input_tokens: 100,
            output_tokens: 40,
            duration_ms: None,
            context_utilization_percent: None,
        };
        let later = CostSample {
            source: CostSource::ClaudeCode,
            model: None,
            input_tokens: 150,
            output_tokens: 60,
            duration_ms: Some(9_000),
            context_utilization_percent: None,
        };

        let run = later
            .since(&baseline)
            .expect("same-source subtraction must not error")
            .expect("larger token counts must still produce a run figure");
        assert_eq!(run.input_tokens, 50);
        assert_eq!(run.output_tokens, 20);
        assert_eq!(
            run.duration_ms, None,
            "a duration missing on one side must null out only that field"
        );
    }

    // The skip-unparseable-line branch this crate's docs describe ("Pi may
    // still be mid-write to the last line when this is read") — a truncated
    // final line must not stop the earlier, complete messages from summing.
    #[test]
    fn sum_pi_transcript_usage_skips_a_truncated_trailing_line() {
        let jsonl = format!(
            "{}\n{}\n{}",
            pi_message_line(100, 50, "gpt-6-astra"),
            pi_message_line(20, 10, "gpt-6-astra"),
            r#"{"type":"message","message":{"usage":{"input":5,"#, // truncated mid-write
        );

        let sample =
            sum_pi_transcript_usage(&jsonl).expect("the two complete messages must still sum");
        assert_eq!(sample.input_tokens, 120);
        assert_eq!(sample.output_tokens, 60);
    }

    /// An `Adapter` for a harness that reports no cost data at all, which
    /// `opencode` really is: Irrlicht carries `metrics: null` for it.
    ///
    /// It answers `Ok(None)`, and it *writes that down*. There is no default
    /// on the trait, so this arm cannot be inherited by forgetting it — the
    /// compiler refuses. That guarantee is what this double stands for, and
    /// it cannot be asserted at run time: deleting the arm below is a
    /// compile error, not a failing test.
    struct MinimalAdapter;

    impl Adapter for MinimalAdapter {
        fn start(&self, _req: &StartRequest) -> Result<StartedSession, AdapterError> {
            unimplemented!("not exercised by this test")
        }
        fn send(
            &self,
            _pane: &PaneId,
            _task_id: uuid::Uuid,
            _prompt: &str,
        ) -> Result<(), AdapterError> {
            unimplemented!("not exercised by this test")
        }
        fn observe(&self, _pane: &PaneId) -> Result<Observation, AdapterError> {
            unimplemented!("not exercised by this test")
        }
        fn interrupt(&self, _pane: &PaneId) -> Result<(), AdapterError> {
            unimplemented!("not exercised by this test")
        }
        fn stop(&self, _pane: &PaneId) -> Result<(), AdapterError> {
            unimplemented!("not exercised by this test")
        }
        fn attach_command(&self, _pane: &PaneId) -> Result<Vec<String>, AdapterError> {
            unimplemented!("not exercised by this test")
        }
        fn runtime_version(&self) -> Result<String, AdapterError> {
            unimplemented!("not exercised by this test")
        }
        fn cost_sample(&self, _pane: &PaneId) -> Result<Option<CostSample>, AdapterError> {
            Ok(None)
        }
    }

    #[test]
    fn an_adapter_whose_harness_reports_nothing_answers_none_rather_than_zeros() {
        let sample = MinimalAdapter
            .cost_sample(&PaneId("wE:p1".to_string()))
            .expect("the default implementation must not error");
        assert_eq!(sample, None);
    }
}
