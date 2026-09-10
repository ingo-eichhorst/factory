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

#[derive(Default)]
struct FakeState {
    observations: HashMap<String, Observation>,
    send_fail: bool,
    send_calls: Vec<(String, uuid::Uuid, String)>,
    stop_calls: Vec<String>,
    start_fail: bool,
    start_confidence: Option<Confidence>,
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

    /// This double reports no cost data. Stated rather than omitted: the
    /// trait has no default, so an implementor cannot answer `None` by
    /// forgetting the method. ADR 0021 decision 6 makes `None` a valid
    /// answer, and it is this fake's real one.
    fn cost_sample(
        &self,
        _pane: &PaneId,
    ) -> Result<Option<factory_adapter::CostSample>, AdapterError> {
        Ok(None)
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
