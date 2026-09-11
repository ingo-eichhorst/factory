//! A stateful `HerdrAccess` double, and the [`ContractFixture`] that wires it
//! to [`PiAdapter`] for `tests/contract_pi.rs`.
//!
//! This is deliberately richer than `factory-adapter`'s own `src/lib.rs` unit
//! tests: those configure exactly one call's response per test (a single
//! `agent_get`, a single `agent_prompt`), which is enough to test one method
//! in isolation. [`contract::run_contract_suite`] drives one `Adapter`
//! through a *sequence* — start, then observe, then send, then send again,
//! then interrupt, then stop, then observe again — and each call's answer
//! depends on what the calls before it did. A pane must actually go
//! `working` after a successful `send` (so the very next `send` sees it busy)
//! and actually stop answering after `stop` (so the closing `observe` sees it
//! gone). That needs a double with real state, not a fixed script.

#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use factory_adapter::contract::ContractFixture;
use factory_adapter::{AdapterError, HerdrAccess, PaneId, PiAdapter, StartRequest};
use factory_paths::CanonicalPath;

#[derive(Clone)]
struct PaneState {
    status: String,
    closed: bool,
    hook_authority: bool,
}

impl PaneState {
    fn fresh(hook_authority: bool) -> Self {
        Self {
            status: "idle".to_string(),
            closed: false,
            hook_authority,
        }
    }
}

struct Inner {
    panes: RefCell<HashMap<String, PaneState>>,
    next_pane_num: Cell<u32>,
    fail_next_workspace_create: Cell<bool>,
    fail_next_agent_start: Cell<bool>,
    pane_close_calls: RefCell<Vec<String>>,
    workspace_create_calls: RefCell<Vec<PathBuf>>,
}

/// A `HerdrAccess` that behaves like Herdr well enough to run the full
/// start → observe → send → send-while-busy → interrupt → stop → observe
/// sequence [`contract::run_contract_suite`] exercises, entirely in memory.
#[derive(Clone)]
pub struct StatefulFakeHerdr(Rc<Inner>);

impl StatefulFakeHerdr {
    pub fn new() -> Self {
        Self(Rc::new(Inner {
            panes: RefCell::new(HashMap::new()),
            next_pane_num: Cell::new(0),
            fail_next_workspace_create: Cell::new(false),
            fail_next_agent_start: Cell::new(false),
            pane_close_calls: RefCell::new(Vec::new()),
            workspace_create_calls: RefCell::new(Vec::new()),
        }))
    }

    /// The next `workspace_create` call fails instead of creating a pane —
    /// backlog §10's "failed start", triggered without depending on any real
    /// filesystem or Herdr condition.
    pub fn fail_next_workspace_create(&self) {
        self.0.fail_next_workspace_create.set(true);
    }

    /// The next `agent_start` call fails — used by tests proving `start`
    /// cleans up the pane it just created rather than leaking it.
    pub fn fail_next_agent_start(&self) {
        self.0.fail_next_agent_start.set(true);
    }

    /// How many times `pane_close` was actually called, and on which panes —
    /// for tests asserting cleanup happened (or didn't leak).
    pub fn pane_close_calls(&self) -> Vec<String> {
        self.0.pane_close_calls.borrow().clone()
    }

    fn fixture_error(command: &str, detail: &str) -> AdapterError {
        AdapterError::UnreadableOutput {
            command: command.to_string(),
            detail: detail.to_string(),
            help: "fix the test fixture".to_string(),
        }
    }
}

impl Default for StatefulFakeHerdr {
    fn default() -> Self {
        Self::new()
    }
}

impl HerdrAccess for StatefulFakeHerdr {
    fn agent_get(&self, pane: &PaneId) -> Result<String, AdapterError> {
        let panes = self.0.panes.borrow();
        let Some(state) = panes.get(&pane.0) else {
            return Err(Self::fixture_error(
                "herdr agent get",
                "fixture: agent_not_found (no such pane)",
            ));
        };
        if state.closed {
            return Err(Self::fixture_error(
                "herdr agent get",
                "fixture: agent_not_found (pane closed)",
            ));
        }
        Ok(format!(
            r#"{{"id":"cli:agent:get","result":{{"agent":{{"agent":"pi","agent_status":"{status}","agent_session":{{"value":"/fake/{pane_id}.jsonl"}},"interactive_ready":true}},"type":"agent_info"}}}}"#,
            status = state.status,
            pane_id = pane.0,
        ))
    }

    fn agent_explain(&self, pane: &PaneId) -> Result<String, AdapterError> {
        let panes = self.0.panes.borrow();
        let Some(state) = panes.get(&pane.0) else {
            return Err(Self::fixture_error(
                "herdr agent explain",
                "fixture: agent_not_found (no such pane)",
            ));
        };
        let mut text = format!("agent: pi\nstate: {}\n", state.status);
        if state.hook_authority {
            text.push_str("screen_detection_skip_reason: full_lifecycle_hook_authority\n");
        }
        Ok(text)
    }

    fn version(&self) -> Result<String, AdapterError> {
        Ok("herdr 0.8.0-fake\n".to_string())
    }

    fn workspace_create(&self, cwd: &Path) -> Result<String, AdapterError> {
        self.0
            .workspace_create_calls
            .borrow_mut()
            .push(cwd.to_path_buf());
        if self.0.fail_next_workspace_create.replace(false) {
            return Err(Self::fixture_error(
                "herdr workspace create",
                "fixture: configured to fail",
            ));
        }
        let n = self.0.next_pane_num.get();
        self.0.next_pane_num.set(n + 1);
        let pane_id = format!("fake-w{n}:p1");
        self.0
            .panes
            .borrow_mut()
            .insert(pane_id.clone(), PaneState::fresh(true));
        Ok(format!(
            r#"{{"id":"cli:workspace:create","result":{{"root_pane":{{"pane_id":"{pane_id}"}}}}}}"#
        ))
    }

    fn agent_start(
        &self,
        _name: &str,
        kind: &str,
        pane: &PaneId,
        _args: &[String],
    ) -> Result<String, AdapterError> {
        if self.0.fail_next_agent_start.replace(false) {
            return Err(Self::fixture_error(
                "herdr agent start",
                "fixture: configured to fail",
            ));
        }
        let mut panes = self.0.panes.borrow_mut();
        let state = panes
            .entry(pane.0.clone())
            .or_insert_with(|| PaneState::fresh(true));
        state.status = "idle".to_string();
        Ok(format!(
            r#"{{"id":"cli:agent:start","result":{{"type":"agent_started","kind":"{kind}"}}}}"#
        ))
    }

    fn agent_prompt(&self, pane: &PaneId, _text: &str) -> Result<String, AdapterError> {
        let mut panes = self.0.panes.borrow_mut();
        let Some(state) = panes.get_mut(&pane.0) else {
            return Err(Self::fixture_error(
                "herdr agent prompt",
                "fixture: agent_not_found (no such pane)",
            ));
        };
        if state.closed {
            return Err(Self::fixture_error(
                "herdr agent prompt",
                "fixture: agent_not_found (pane closed)",
            ));
        }
        // `--wait --until working` only returns once Herdr observed the
        // harness move to `working`; simulate that by actually moving it.
        state.status = "working".to_string();
        Ok(r#"{"id":"cli:agent:prompt","result":{"type":"ok"}}"#.to_string())
    }

    fn agent_interrupt(&self, pane: &PaneId) -> Result<String, AdapterError> {
        let panes = self.0.panes.borrow();
        if !panes.contains_key(&pane.0) {
            return Err(Self::fixture_error(
                "herdr agent send-keys",
                "fixture: agent_not_found (no such pane)",
            ));
        }
        Ok(r#"{"id":"cli:agent:send-keys","result":{"type":"ok"}}"#.to_string())
    }

    fn pane_close(&self, pane: &PaneId) -> Result<String, AdapterError> {
        self.0.pane_close_calls.borrow_mut().push(pane.0.clone());
        let mut panes = self.0.panes.borrow_mut();
        if let Some(state) = panes.get_mut(&pane.0) {
            state.closed = true;
        }
        Ok(r#"{"id":"cli:pane:close","result":{"type":"ok"}}"#.to_string())
    }
}

/// The [`ContractFixture`] wiring [`StatefulFakeHerdr`] to [`PiAdapter`].
pub struct PiFixture {
    adapter: PiAdapter<StatefulFakeHerdr>,
    fake: StatefulFakeHerdr,
    tempdirs: RefCell<Vec<tempfile::TempDir>>,
    next_seed: Cell<u32>,
}

impl PiFixture {
    pub fn new() -> Self {
        let fake = StatefulFakeHerdr::new();
        let adapter = PiAdapter::new(fake.clone());
        Self {
            adapter,
            fake,
            tempdirs: RefCell::new(Vec::new()),
            next_seed: Cell::new(0),
        }
    }

    /// The next `agent_start` call the adapter makes will fail — exposed so
    /// a Pi-specific test can prove `start` cleans up the pane it created
    /// rather than leaking it.
    pub fn fail_next_agent_start(&self) {
        self.fake.fail_next_agent_start();
    }

    /// Every pane `pane_close` was actually called on, in order.
    pub fn pane_close_calls(&self) -> Vec<String> {
        self.fake.pane_close_calls()
    }

    fn fresh_workspace(&self) -> CanonicalPath {
        let dir = tempfile::tempdir().expect("tempdir");
        let canonical = CanonicalPath::resolve(dir.path()).expect("resolve tempdir");
        self.tempdirs.borrow_mut().push(dir);
        canonical
    }

    fn fresh_seed(&self) -> u32 {
        let seed = self.next_seed.get();
        self.next_seed.set(seed + 1);
        seed
    }

    fn fresh_uuid(&self) -> uuid::Uuid {
        let seed = self.fresh_seed();
        uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
    }
}

impl Default for PiFixture {
    fn default() -> Self {
        Self::new()
    }
}

impl ContractFixture for PiFixture {
    type A = PiAdapter<StatefulFakeHerdr>;

    fn adapter(&self) -> &Self::A {
        &self.adapter
    }

    fn startable(&self) -> StartRequest {
        StartRequest {
            scope_id: self.fresh_uuid(),
            session_id: self.fresh_uuid(),
            workspace: self.fresh_workspace(),
            generated_context: "company\nscope\nagent\n".to_string(),
            model: None,
        }
    }

    fn unstartable(&self) -> StartRequest {
        self.fake.fail_next_workspace_create();
        StartRequest {
            scope_id: self.fresh_uuid(),
            session_id: self.fresh_uuid(),
            workspace: self.fresh_workspace(),
            generated_context: String::new(),
            model: None,
        }
    }
}

// === Claude Code ========================================================
//
// A second stateful double, sharing this module's doc comment's own
// reasoning: `run_contract_suite` drives one `Adapter` through a *sequence*,
// which needs real state, not a fixed script. This one is richer than
// `StatefulFakeHerdr` in one way that matters: `ClaudeAdapter` reads two
// different sources (Herdr for actions and `send`'s busy precheck, Irrlicht
// for `observe`), and both must agree about the *same* pane at all times —
// so one pane map, and two thin views onto it (`ClaudeFakeHerdr`,
// `ClaudeFakeIrrlicht`), each rendering that one map into the JSON shape its
// own protocol uses. A `send` that flips a pane to `working` must be visible
// to the *next* `observe`'s Irrlicht rendering of that same pane, exactly as
// on the real machine a prompt landing is visible, eventually, to Irrlicht's
// transcript poll.

use factory_adapter::{ClaudeAdapter, IrrlichtAccess};

#[derive(Clone)]
struct ClaudePaneState {
    /// Herdr-shaped ground truth for this pane: `idle` | `working`. Both
    /// `ClaudeFakeHerdr::agent_get` and `ClaudeFakeIrrlicht::sessions`
    /// render the same value, each into its own protocol's vocabulary —
    /// see this section's own doc comment.
    status: String,
    closed: bool,
    session_id: String,
    transcript_path: String,
    /// The workspace `start` was asked to root this pane in. Rendered into
    /// `ClaudeFakeIrrlicht::sessions`'s `"cwd"` field, matching the real
    /// endpoint — every live entry carries one — and needed so a fixture can
    /// actually express "two sessions in one directory" for the cwd-join
    /// mutation drill, rather than the join simply finding nothing.
    cwd: PathBuf,
}

impl ClaudePaneState {
    fn fresh(n: u32, cwd: PathBuf) -> Self {
        Self {
            status: "idle".to_string(),
            closed: false,
            // UUID-shaped, because the real endpoint's `session_id` is one
            // and `ClaudeAdapter` refuses anything else — Irrlicht reports a
            // `proc-<pid>` placeholder for a second or two after `start`, and
            // migration 5's column is documented to hold the bare UUID. A
            // fixture with a non-UUID id would let a placeholder through here
            // and nowhere else, which is a fixture testing itself.
            session_id: format!("00000000-0000-4000-9000-{n:012x}"),
            transcript_path: format!("/fake/claude/session-{n}.jsonl"),
            cwd,
        }
    }

    /// Herdr's own vocabulary (measured: `idle | working | blocked | done`,
    /// though this fixture only ever produces the first two) translated
    /// into Irrlicht's (measured: `ready | working | error`) — one ground
    /// truth per pane, rendered two ways, per this section's doc comment.
    fn irrlicht_state(&self) -> &'static str {
        if self.status == "working" {
            "working"
        } else {
            "ready"
        }
    }
}

struct ClaudeInner {
    panes: RefCell<HashMap<String, ClaudePaneState>>,
    next_pane_num: Cell<u32>,
    fail_next_workspace_create: Cell<bool>,
    fail_next_agent_start: Cell<bool>,
    pane_close_calls: RefCell<Vec<String>>,
}

impl ClaudeInner {
    fn new() -> Self {
        Self {
            panes: RefCell::new(HashMap::new()),
            next_pane_num: Cell::new(0),
            fail_next_workspace_create: Cell::new(false),
            fail_next_agent_start: Cell::new(false),
            pane_close_calls: RefCell::new(Vec::new()),
        }
    }

    fn fixture_error(command: &str, detail: &str) -> AdapterError {
        AdapterError::UnreadableOutput {
            command: command.to_string(),
            detail: detail.to_string(),
            help: "fix the test fixture".to_string(),
        }
    }
}

/// The `HerdrAccess` half of the shared pane map — every action `ClaudeAdapter`
/// drives through Herdr (`start`, `send`'s busy precheck, `interrupt`,
/// `stop`), plus `agent_get`/`agent_explain`'s absence proving `observe`
/// never reaches this double at all (see `agent_explain` below).
#[derive(Clone)]
pub struct ClaudeFakeHerdr(Rc<ClaudeInner>);

impl ClaudeFakeHerdr {
    fn new(inner: Rc<ClaudeInner>) -> Self {
        Self(inner)
    }

    pub fn fail_next_workspace_create(&self) {
        self.0.fail_next_workspace_create.set(true);
    }

    pub fn fail_next_agent_start(&self) {
        self.0.fail_next_agent_start.set(true);
    }

    pub fn pane_close_calls(&self) -> Vec<String> {
        self.0.pane_close_calls.borrow().clone()
    }

    /// The session id this fixture itself recorded for `pane`, read directly
    /// off the pane map rather than through the adapter — the independent
    /// ground truth a mutation drill compares `ClaudeAdapter::observe`'s
    /// answer against, so that assertion cannot tautologically pass by
    /// comparing two calls into the same (possibly mutated) join.
    pub fn session_id_for(&self, pane: &PaneId) -> Option<String> {
        self.0
            .panes
            .borrow()
            .get(&pane.0)
            .map(|state| state.session_id.clone())
    }
}

impl HerdrAccess for ClaudeFakeHerdr {
    fn agent_get(&self, pane: &PaneId) -> Result<String, AdapterError> {
        let panes = self.0.panes.borrow();
        let Some(state) = panes.get(&pane.0) else {
            return Err(ClaudeInner::fixture_error(
                "herdr agent get",
                "fixture: agent_not_found (no such pane)",
            ));
        };
        if state.closed {
            return Err(ClaudeInner::fixture_error(
                "herdr agent get",
                "fixture: agent_not_found (pane closed)",
            ));
        }
        Ok(format!(
            r#"{{"id":"cli:agent:get","result":{{"agent":{{"agent":"claude","agent_status":"{status}"}},"type":"agent_info"}}}}"#,
            status = state.status,
        ))
    }

    fn agent_explain(&self, _pane: &PaneId) -> Result<String, AdapterError> {
        unreachable!(
            "ClaudeAdapter must never call agent_explain — observation for Claude comes from \
             Irrlicht, not Herdr's screen-detection explanation"
        )
    }

    fn version(&self) -> Result<String, AdapterError> {
        Ok("herdr 0.8.0-fake\n".to_string())
    }

    fn workspace_create(&self, cwd: &Path) -> Result<String, AdapterError> {
        if self.0.fail_next_workspace_create.replace(false) {
            return Err(ClaudeInner::fixture_error(
                "herdr workspace create",
                "fixture: configured to fail",
            ));
        }
        let n = self.0.next_pane_num.get();
        self.0.next_pane_num.set(n + 1);
        let pane_id = format!("fake-claude-w{n}:p1");
        self.0.panes.borrow_mut().insert(
            pane_id.clone(),
            ClaudePaneState::fresh(n, cwd.to_path_buf()),
        );
        Ok(format!(
            r#"{{"id":"cli:workspace:create","result":{{"root_pane":{{"pane_id":"{pane_id}"}}}}}}"#
        ))
    }

    fn agent_start(
        &self,
        _name: &str,
        _kind: &str,
        pane: &PaneId,
        _args: &[String],
    ) -> Result<String, AdapterError> {
        if self.0.fail_next_agent_start.replace(false) {
            return Err(ClaudeInner::fixture_error(
                "herdr agent start",
                "fixture: configured to fail",
            ));
        }
        let mut panes = self.0.panes.borrow_mut();
        let state = panes
            .entry(pane.0.clone())
            .or_insert_with(|| ClaudePaneState::fresh(0, PathBuf::new()));
        state.status = "idle".to_string();
        Ok(r#"{"id":"cli:agent:start","result":{"type":"agent_started"}}"#.to_string())
    }

    fn agent_prompt(&self, pane: &PaneId, _text: &str) -> Result<String, AdapterError> {
        let mut panes = self.0.panes.borrow_mut();
        let Some(state) = panes.get_mut(&pane.0) else {
            return Err(ClaudeInner::fixture_error(
                "herdr agent prompt",
                "fixture: agent_not_found (no such pane)",
            ));
        };
        if state.closed {
            return Err(ClaudeInner::fixture_error(
                "herdr agent prompt",
                "fixture: agent_not_found (pane closed)",
            ));
        }
        // Herdr's screen-detected state flips synchronously with a
        // confirmed prompt (measured live) — simulate that by actually
        // moving it, exactly as `StatefulFakeHerdr::agent_prompt` does for
        // Pi.
        state.status = "working".to_string();
        Ok(r#"{"id":"cli:agent:prompt","result":{"type":"ok"}}"#.to_string())
    }

    fn agent_interrupt(&self, pane: &PaneId) -> Result<String, AdapterError> {
        if !self.0.panes.borrow().contains_key(&pane.0) {
            return Err(ClaudeInner::fixture_error(
                "herdr agent send-keys",
                "fixture: agent_not_found (no such pane)",
            ));
        }
        Ok(r#"{"id":"cli:agent:send-keys","result":{"type":"ok"}}"#.to_string())
    }

    fn pane_close(&self, pane: &PaneId) -> Result<String, AdapterError> {
        self.0.pane_close_calls.borrow_mut().push(pane.0.clone());
        if let Some(state) = self.0.panes.borrow_mut().get_mut(&pane.0) {
            state.closed = true;
        }
        Ok(r#"{"id":"cli:pane:close","result":{"type":"ok"}}"#.to_string())
    }
}

/// The `IrrlichtAccess` half of the shared pane map — the only source
/// `ClaudeAdapter::observe` reads. Closed panes are omitted entirely,
/// simulating what this crate's own live measurement found: Irrlicht drops
/// a session from `/api/v1/sessions` within a couple of seconds of the
/// underlying process exiting (kqueue `NOTE_EXIT`, ADR 0011).
#[derive(Clone)]
pub struct ClaudeFakeIrrlicht(Rc<ClaudeInner>);

impl ClaudeFakeIrrlicht {
    fn new(inner: Rc<ClaudeInner>) -> Self {
        Self(inner)
    }
}

impl IrrlichtAccess for ClaudeFakeIrrlicht {
    fn sessions(&self) -> Result<String, AdapterError> {
        let panes = self.0.panes.borrow();
        let agents: Vec<String> = panes
            .iter()
            .filter(|(_, state)| !state.closed)
            .map(|(pane_id, state)| {
                format!(
                    r#"{{"adapter":"claude-code","state":"{state}","cwd":"{cwd}","session_id":"{session_id}","transcript_path":"{transcript_path}","launcher":{{"herdr_pane_id":"{pane_id}"}}}}"#,
                    state = state.irrlicht_state(),
                    cwd = state.cwd.display(),
                    session_id = state.session_id,
                    transcript_path = state.transcript_path,
                )
            })
            .collect();
        Ok(format!(
            r#"{{"groups":[{{"name":"fake","agents":[{}]}}]}}"#,
            agents.join(",")
        ))
    }
}

/// The [`ContractFixture`] wiring [`ClaudeFakeHerdr`]/[`ClaudeFakeIrrlicht`]
/// to [`ClaudeAdapter`].
pub struct ClaudeFixture {
    adapter: ClaudeAdapter<ClaudeFakeHerdr, ClaudeFakeIrrlicht>,
    herdr: ClaudeFakeHerdr,
    tempdirs: RefCell<Vec<tempfile::TempDir>>,
    next_seed: Cell<u32>,
}

impl ClaudeFixture {
    pub fn new() -> Self {
        let inner = Rc::new(ClaudeInner::new());
        let herdr = ClaudeFakeHerdr::new(Rc::clone(&inner));
        let irrlicht = ClaudeFakeIrrlicht::new(Rc::clone(&inner));
        let adapter = ClaudeAdapter::new(herdr.clone(), irrlicht);
        Self {
            adapter,
            herdr,
            tempdirs: RefCell::new(Vec::new()),
            next_seed: Cell::new(0),
        }
    }

    /// The next `agent_start` call the adapter makes will fail — exposed so
    /// a Claude-specific test can prove `start` cleans up the pane it
    /// created rather than leaking it, mirroring `PiFixture`'s identical
    /// method.
    pub fn fail_next_agent_start(&self) {
        self.herdr.fail_next_agent_start();
    }

    /// Every pane `pane_close` was actually called on, in order.
    pub fn pane_close_calls(&self) -> Vec<String> {
        self.herdr.pane_close_calls()
    }

    /// The session id this fixture itself recorded for `pane` — read
    /// directly off the fixture's own pane map, independent of
    /// `ClaudeAdapter::observe`'s Irrlicht-side join. A test comparing
    /// `observe`'s answer against *this* cannot pass tautologically the way
    /// comparing it against `start`'s own (also-joined) answer can.
    pub fn session_id_for(&self, pane: &PaneId) -> Option<String> {
        self.herdr.session_id_for(pane)
    }

    fn fresh_seed(&self) -> u32 {
        let seed = self.next_seed.get();
        self.next_seed.set(seed + 1);
        seed
    }

    fn fresh_uuid(&self) -> uuid::Uuid {
        let seed = self.fresh_seed();
        uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
    }

    /// A `StartRequest` this fixture guarantees `start` accepts, rooted at
    /// `workspace` rather than a fresh one of its own — lets a test put two
    /// sessions in one directory deliberately, rather than each `start`
    /// silently getting its own. That is exactly the ambiguity backlog
    /// §10's 2026-09-09 resolution note and this adapter's cwd rule exist to
    /// survive: this machine has run three live `claude-code` sessions
    /// sharing one `cwd` at once.
    pub fn startable_at(&self, workspace: CanonicalPath) -> StartRequest {
        StartRequest {
            scope_id: self.fresh_uuid(),
            session_id: self.fresh_uuid(),
            workspace,
            generated_context: "company\nscope\nagent\n".to_string(),
            model: None,
        }
    }

    fn fresh_workspace(&self) -> CanonicalPath {
        let dir = tempfile::tempdir().expect("tempdir");
        let canonical = CanonicalPath::resolve(dir.path()).expect("resolve tempdir");
        self.tempdirs.borrow_mut().push(dir);
        canonical
    }
}

impl Default for ClaudeFixture {
    fn default() -> Self {
        Self::new()
    }
}

impl ContractFixture for ClaudeFixture {
    type A = ClaudeAdapter<ClaudeFakeHerdr, ClaudeFakeIrrlicht>;

    fn adapter(&self) -> &Self::A {
        &self.adapter
    }

    fn startable(&self) -> StartRequest {
        self.startable_at(self.fresh_workspace())
    }

    fn unstartable(&self) -> StartRequest {
        self.herdr.fail_next_workspace_create();
        self.startable_at(self.fresh_workspace())
    }
}
