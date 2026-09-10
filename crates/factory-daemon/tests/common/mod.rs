//! Shared fixtures for `factory-daemon`'s integration tests.
//!
//! Every test builds its own `tempfile::TempDir` instance root — never the
//! real `.factory/` directory — writes a `.factory/config.yaml` and an
//! `AGENTS.md` per scope, then projects the configuration into the `scopes`
//! table exactly the way `scope.reconcile --apply` would, so fixtures never
//! drift from what a real caller could actually produce.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;

use factory_adapter::{
    Adapter, AdapterError, Confidence, Observation, PaneId, StartRequest, StartedSession,
    TaskSignal,
};

/// One scope this fixture registers, as a direct child of the fixture's own
/// root scope (so every scope built this way is at least a sibling of every
/// other one, and design §6's delegation rule has something to say about
/// them).
pub struct ScopeSpec {
    pub name: &'static str,
    pub path: &'static str,
    pub agent_name: &'static str,
    pub lifetime: &'static str,
    pub max_sessions: u32,
}

impl ScopeSpec {
    pub fn new(name: &'static str, path: &'static str) -> Self {
        Self {
            name,
            path,
            agent_name: "agent",
            lifetime: "permanent",
            max_sessions: 3,
        }
    }

    pub fn temporary(mut self) -> Self {
        self.lifetime = "temporary";
        self
    }
}

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub instance_root: PathBuf,
    pub root_scope_id: uuid::Uuid,
    pub scope_ids: HashMap<&'static str, uuid::Uuid>,
}

impl Fixture {
    pub fn instance_root(&self) -> &std::path::Path {
        &self.instance_root
    }

    pub fn scope(&self, name: &str) -> uuid::Uuid {
        *self
            .scope_ids
            .get(name)
            .unwrap_or_else(|| panic!("fixture has no scope named {name:?}"))
    }
}

fn uid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

/// Build a fixture instance root with one root scope ("company", at `.`) and
/// one child scope per `[ScopeSpec]` given, each with one agent. Registers
/// every scope into the `scopes` table before returning.
pub fn build(scopes: &[ScopeSpec]) -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let instance_root = dir.path().to_path_buf();

    std::fs::write(instance_root.join("AGENTS.md"), "# company root\n").expect("write AGENTS.md");

    let mut yaml = String::new();
    yaml.push_str("version: 1\n\n");
    yaml.push_str(&format!(
        "instance:\n  id: {}\n  name: fixture-instance\n\n",
        uid(1)
    ));
    yaml.push_str("scopes:\n");
    yaml.push_str(&format!(
        "  - id: {}\n    name: company\n    path: .\n    agent:\n      name: company-agent\n      harness: pi\n\n",
        uid(1)
    ));

    let mut scope_ids = HashMap::new();
    for (i, spec) in scopes.iter().enumerate() {
        let id = uid(2 + i as u32);
        scope_ids.insert(spec.name, id);

        let scope_dir = instance_root.join(spec.path);
        std::fs::create_dir_all(&scope_dir).expect("create scope dir");
        std::fs::write(scope_dir.join("AGENTS.md"), format!("# {}\n", spec.name))
            .expect("write AGENTS.md");

        yaml.push_str(&format!(
            "  - id: {id}\n    name: {}\n    path: {}\n    agent:\n      name: {}\n      harness: pi\n      \
             max_sessions: {}\n      lifetime: {}\n\n",
            spec.name, spec.path, spec.agent_name, spec.max_sessions, spec.lifetime
        ));
    }

    let config_path = factory_daemon::config_path(&instance_root);
    std::fs::create_dir_all(config_path.parent().unwrap()).expect("create .factory");
    std::fs::write(&config_path, yaml).expect("write config.yaml");

    // Project the configuration into `scopes`, exactly as `scope.reconcile
    // --apply` would (see `factory_registry`'s own module docs).
    {
        let mut store = factory_store::Store::open(&instance_root).expect("open store");
        let config = factory_config::load(&config_path).expect("load config");
        let resolved = factory_registry::resolve(&config, &instance_root).expect("resolve");
        let report = factory_registry::reconcile(&store, &resolved).expect("reconcile");
        factory_registry::apply(&mut store, &resolved, &report).expect("apply");
    }

    Fixture {
        dir,
        instance_root,
        root_scope_id: uid(1),
        scope_ids,
    }
}

pub fn seed(n: u32) -> uuid::Uuid {
    uid(100 + n)
}

// --- A configurable fake `Adapter` ---------------------------------------

/// What one queued [`FakeAdapter::cost_sample`] call answers.
#[derive(Clone)]
enum CostSampleReply {
    Value(Option<factory_adapter::CostSample>),
    Err,
}

/// A hook run synchronously inside every `cost_sample` call — see
/// `FakeState::cost_sample_hook`'s own doc comment. Named so the field
/// itself stays under clippy's type-complexity limit.
type CostSampleHook = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Default)]
struct FakeState {
    observations: HashMap<String, Observation>,
    send_fail: bool,
    send_calls: Vec<(String, uuid::Uuid, String)>,
    stop_calls: Vec<String>,
    start_fail: bool,
    start_confidence: Option<Confidence>,
    // Per-pane queue, popped front-first: `task.send`'s baseline sample and a
    // later `task.done`/`task.fail`/`task.cancel`'s terminal sample both call
    // `cost_sample` against the same pane, and a test needs to answer them
    // differently.
    cost_samples: HashMap<String, std::collections::VecDeque<CostSampleReply>>,
    // Every pane sampled, in call order — regardless of what was queued for
    // it — so a test can assert *that* a call happened even when the answer
    // was the unconfigured default.
    cost_sample_calls: Vec<String>,
    // Overrides every queued reply with `Err` when set — for a test that
    // wants *every* `cost_sample` call to fail, at delivery and at the
    // terminal alike, without tracking exactly how many calls that takes.
    cost_sample_fail: bool,
    // Run synchronously inside every `cost_sample` call, before it answers —
    // lets a test observe (or manufacture) database state at the exact
    // moment a sample is taken, which a call count alone cannot: see
    // `cost_sample_runs_before_session_teardown` and the write-failure tests
    // in `tests/cost.rs`.
    cost_sample_hook: Option<CostSampleHook>,
}

/// A fully in-memory [`Adapter`]: every method is driven by state a test sets
/// up ahead of time, never by an actual terminal multiplexer.
#[derive(Clone)]
pub struct FakeAdapter {
    state: std::sync::Arc<std::sync::Mutex<FakeState>>,
}

impl FakeAdapter {
    pub fn new() -> Self {
        Self {
            state: std::sync::Arc::new(std::sync::Mutex::new(FakeState::default())),
        }
    }

    pub fn set_observation(&self, pane: &str, observation: Observation) {
        self.state
            .lock()
            .unwrap()
            .observations
            .insert(pane.to_string(), observation);
    }

    pub fn set_send_fail(&self, fail: bool) {
        self.state.lock().unwrap().send_fail = fail;
    }

    pub fn set_start_fail(&self, fail: bool) {
        self.state.lock().unwrap().start_fail = fail;
    }

    pub fn set_start_confidence(&self, confidence: Confidence) {
        self.state.lock().unwrap().start_confidence = Some(confidence);
    }

    pub fn send_calls(&self) -> Vec<(String, uuid::Uuid, String)> {
        self.state.lock().unwrap().send_calls.clone()
    }

    pub fn stop_calls(&self) -> Vec<String> {
        self.state.lock().unwrap().stop_calls.clone()
    }

    /// Queue `sample` as the next `cost_sample(pane)` answer. Calls against
    /// `pane` pop this queue front-first; once it is empty, `cost_sample`
    /// answers `Ok(None)`, `FakeAdapter`'s unconfigured default.
    pub fn queue_cost_sample(&self, pane: &str, sample: Option<factory_adapter::CostSample>) {
        self.state
            .lock()
            .unwrap()
            .cost_samples
            .entry(pane.to_string())
            .or_default()
            .push_back(CostSampleReply::Value(sample));
    }

    /// Queue an `Err` as the next `cost_sample(pane)` answer.
    pub fn queue_cost_sample_err(&self, pane: &str) {
        self.state
            .lock()
            .unwrap()
            .cost_samples
            .entry(pane.to_string())
            .or_default()
            .push_back(CostSampleReply::Err);
    }

    /// Make every `cost_sample` call fail, at every pane, regardless of
    /// what is queued — for a test that wants to prove a failure to *read*
    /// cost never blocks the task action it accompanies, without having to
    /// track exactly how many calls that takes.
    pub fn set_cost_sample_fail(&self, fail: bool) {
        self.state.lock().unwrap().cost_sample_fail = fail;
    }

    /// Every pane `cost_sample` was asked about, in call order.
    pub fn cost_sample_calls(&self) -> Vec<String> {
        self.state.lock().unwrap().cost_sample_calls.clone()
    }

    /// Run `hook` synchronously inside every subsequent `cost_sample` call,
    /// before it answers, with the pane string it was asked about. See
    /// `FakeState::cost_sample_hook`'s own doc comment for why this exists
    /// instead of a plain call-order log.
    pub fn set_cost_sample_hook(&self, hook: impl Fn(&str) + Send + Sync + 'static) {
        self.state.lock().unwrap().cost_sample_hook = Some(std::sync::Arc::new(hook));
    }
}

impl Default for FakeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

fn unavailable_error(detail: &str) -> AdapterError {
    AdapterError::UnreadableOutput {
        command: "fake".to_string(),
        detail: detail.to_string(),
        help: "test fixture".to_string(),
    }
}

impl Adapter for FakeAdapter {
    fn start(&self, req: &StartRequest) -> Result<StartedSession, AdapterError> {
        let mut state = self.state.lock().unwrap();
        if state.start_fail {
            return Err(unavailable_error("start configured to fail"));
        }
        let confidence = state
            .start_confidence
            .take()
            .unwrap_or(Confidence::Authoritative);
        Ok(StartedSession {
            pane: PaneId(format!("pane-{}", req.session_id)),
            harness_session_id: Some(format!("harness-{}", req.session_id)),
            confidence,
        })
    }

    fn send(&self, pane: &PaneId, task_id: uuid::Uuid, prompt: &str) -> Result<(), AdapterError> {
        let mut state = self.state.lock().unwrap();
        state
            .send_calls
            .push((pane.0.clone(), task_id, prompt.to_string()));
        if state.send_fail {
            return Err(unavailable_error("send configured to fail"));
        }
        Ok(())
    }

    fn observe(&self, pane: &PaneId) -> Result<Observation, AdapterError> {
        let state = self.state.lock().unwrap();
        match state.observations.get(&pane.0) {
            Some(o) => Ok(o.clone()),
            None => Ok(Observation {
                pane: pane.clone(),
                harness_state: "idle".to_string(),
                confidence: Confidence::Unavailable,
                session_alive: true,
                task_signal: TaskSignal::NoChange,
                transcript_path: None,
                harness_session_id: None,
            }),
        }
    }

    fn interrupt(&self, _pane: &PaneId) -> Result<(), AdapterError> {
        Ok(())
    }

    fn stop(&self, pane: &PaneId) -> Result<(), AdapterError> {
        self.state.lock().unwrap().stop_calls.push(pane.0.clone());
        Ok(())
    }

    fn attach_command(&self, pane: &PaneId) -> Result<Vec<String>, AdapterError> {
        Ok(vec!["fake-attach".to_string(), pane.0.clone()])
    }

    fn runtime_version(&self) -> Result<String, AdapterError> {
        Ok("fake-1.0".to_string())
    }

    /// Unconfigured, this double reports no cost data — stated rather than
    /// omitted: the trait has no default, so an implementor cannot answer
    /// `None` by forgetting the method, and ADR 0021 decision 6 makes `None`
    /// a valid answer regardless. A test that needs something else queues it
    /// with [`FakeAdapter::queue_cost_sample`] / `queue_cost_sample_err` /
    /// `set_cost_sample_fail`.
    fn cost_sample(
        &self,
        pane: &PaneId,
    ) -> Result<Option<factory_adapter::CostSample>, AdapterError> {
        let hook = {
            let mut state = self.state.lock().unwrap();
            state.cost_sample_calls.push(pane.0.clone());
            state.cost_sample_hook.clone()
        };
        // Run outside the lock: a hook that itself touches this adapter (or
        // just takes a while, as the write-failure tests' hooks
        // deliberately do) must not hold `state` while it works.
        if let Some(hook) = hook {
            hook(&pane.0);
        }

        let mut state = self.state.lock().unwrap();
        if state.cost_sample_fail {
            return Err(unavailable_error("cost_sample configured to fail"));
        }
        match state
            .cost_samples
            .get_mut(&pane.0)
            .and_then(std::collections::VecDeque::pop_front)
        {
            Some(CostSampleReply::Value(sample)) => Ok(sample),
            Some(CostSampleReply::Err) => Err(unavailable_error("cost_sample configured to fail")),
            None => Ok(None),
        }
    }
}

pub fn authoritative_observation(pane: &str, task_signal: TaskSignal) -> Observation {
    Observation {
        pane: PaneId(pane.to_string()),
        harness_state: "working".to_string(),
        confidence: Confidence::Authoritative,
        session_alive: true,
        task_signal,
        transcript_path: None,
        harness_session_id: None,
    }
}
