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
//! Three rules run through everything here:
//!
//! - **A wrong observation is worse than none**, because it looks like an
//!   answer. Every path that cannot establish state returns
//!   [`Confidence::Unavailable`] rather than a guess.
//! - **The pane is the join key.** It is the one identifier Factory chose and
//!   recorded. Working directory is never used: two sessions of one agent may
//!   share a workspace, and `assistant` is configured for exactly that.
//! - **No harness state closes a task.** See [`TaskSignal`].

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

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

/// What every harness adapter provides.
///
/// Slice 5 implements only `observe`. Starting, sending, interrupting, and
/// stopping remain documented operator procedures: design §5 requires a
/// delivery attempt to be recorded before input reaches the PTY, and building
/// that on an unproven state source is the wrong order. Slice 10 automates what
/// this slice proves.
pub trait Adapter {
    /// Read the current state of the session in `pane`.
    ///
    /// Returns an `Observation` with [`Confidence::Unavailable`] rather than an
    /// error when the harness is simply not answerable — a missing pane, a
    /// stopped Herdr. An `Err` means the adapter itself failed.
    fn observe(&self, pane: &PaneId) -> Result<Observation, AdapterError>;

    /// The harness runtime's version, recorded with each session so that later
    /// behaviour changes are attributable.
    fn runtime_version(&self) -> Result<String, AdapterError>;
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
}

/// The Pi adapter, reading state from Herdr.
pub struct PiAdapter<A: HerdrAccess> {
    herdr: A,
}

impl<A: HerdrAccess> PiAdapter<A> {
    #[must_use]
    pub fn new(herdr: A) -> Self {
        Self { herdr }
    }
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

fn looks_like_uuid(s: &str) -> bool {
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

impl<A: HerdrAccess> Adapter for PiAdapter<A> {
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

    fn runtime_version(&self) -> Result<String, AdapterError> {
        self.herdr.version().map(|raw| raw.trim().to_string())
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
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    type Resp = Result<String, AdapterError>;

    /// A `HerdrAccess` built from recorded payloads instead of a terminal
    /// multiplexer. `HerdrCli` itself is never exercised by these tests — it
    /// runs the real `herdr` binary, which this crate's test suite must not
    /// touch.
    struct FakeHerdr {
        get: Box<dyn Fn(&PaneId) -> Resp>,
        explain: Box<dyn Fn(&PaneId) -> Resp>,
        version: Box<dyn Fn() -> Resp>,
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
            }
        }

        fn with_version(mut self, version: impl Fn() -> Resp + 'static) -> Self {
            self.version = Box::new(version);
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
}
