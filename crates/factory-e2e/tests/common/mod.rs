//! A throwaway Factory instance, built the way an operator builds a real one:
//! a `.factory/config.yaml`, a database opened beside it, and the scope
//! registry projected from the declared configuration (ADR 0015, ADR 0016).
//!
//! Nothing here touches the live instance at
//! `/Users/factory/business-factory/.factory/factory.sqlite`. Every drill gets
//! its own `tempfile::TempDir` and deletes it when the test ends.
//!
//! Each test file in `tests/` compiles as its own crate and uses a different
//! part of this module, so an item unused by one of them is not dead code.
#![allow(dead_code)]

use factory_paths::CanonicalPath;
use factory_store::Store;

/// The scope hierarchy every drill shares, chosen so that all five kinships
/// `factory_registry::kinship` can return are present in one tree:
///
/// ```text
/// root (.)            <- the instance root scope
/// ├── alpha
/// │   └── beta
/// └── gamma
/// ```
///
/// - `alpha` → `beta` is a descendant, and `alpha` → `gamma` a sibling: both legal.
/// - `beta` → `root` is an ancestor, `beta` → `beta` is itself: both refused.
/// - `beta` → `gamma` is an uncle, which resolves to `Unrelated` and is refused —
///   design §6's "a nephew or cousin scope is not directly targetable", read
///   from the other end.
pub struct Instance {
    pub dir: tempfile::TempDir,
    pub store: Store,
    pub root: uuid::Uuid,
    pub alpha: uuid::Uuid,
    pub beta: uuid::Uuid,
    pub gamma: uuid::Uuid,
}

impl Instance {
    /// A fresh, empty directory under the instance root, usable as a session
    /// workspace. Not a scope — design §2.3's workspaces are directories a
    /// session runs in, not registered scopes.
    pub fn workspace(&self, name: &str) -> CanonicalPath {
        let path = self.dir.path().join(name);
        std::fs::create_dir_all(&path).expect("create workspace directory");
        CanonicalPath::resolve(&path).expect("resolve workspace")
    }
}

const ROOT_ID: &str = "11111111-1111-4111-8111-111111111111";
const ALPHA_ID: &str = "22222222-2222-4222-8222-222222222222";
const BETA_ID: &str = "33333333-3333-4333-8333-333333333333";
const GAMMA_ID: &str = "44444444-4444-4444-8444-444444444444";

/// Build the instance: directories, `config.yaml`, database, and a registry
/// projected through the real `resolve` → `reconcile` → `apply` path rather
/// than seeded with raw SQL. A drill that seeded `scopes` directly would not
/// be end-to-end; it would be a fixture pretending to be one.
pub fn build() -> Instance {
    let dir = tempfile::tempdir().expect("tempdir");
    for relative in ["alpha", "alpha/beta", "gamma", ".factory"] {
        std::fs::create_dir_all(dir.path().join(relative)).expect("create scope directory");
    }

    let config_yaml = format!(
        r#"version: 1

instance:
  id: 99999999-9999-4999-8999-999999999999
  name: e2e-instance

scopes:
  - id: {ROOT_ID}
    name: root
    path: .
    agent:
      name: root-agent
      harness: pi
      max_sessions: 1

  - id: {ALPHA_ID}
    name: alpha
    path: alpha
    agents:
      - name: alpha-agent
        harness: pi
        max_sessions: 2
      - name: alpha-chat
        harness: pi
        max_sessions: 1

  - id: {BETA_ID}
    name: beta
    path: alpha/beta
    agent:
      name: beta-agent
      harness: pi
      max_sessions: 1

  - id: {GAMMA_ID}
    name: gamma
    path: gamma
    agent:
      name: gamma-agent
      harness: pi
      max_sessions: 1
"#
    );
    // Design §2.5: every scope provides one human-maintained `AGENTS.md`, and
    // it stays in the scope directory. Without it `reconcile` reports
    // `UnreadableContext` — which the first run of this drill demonstrated,
    // and which is the registry telling the truth, not a fixture problem.
    for (relative, body) in [
        (".", "# root\n\nCompany-wide instructions.\n"),
        ("alpha", "# alpha\n\nAlpha's own instructions.\n"),
        ("alpha/beta", "# beta\n\nBeta's own instructions.\n"),
        ("gamma", "# gamma\n\nGamma's own instructions.\n"),
    ] {
        std::fs::write(dir.path().join(relative).join("AGENTS.md"), body).expect("write AGENTS.md");
    }

    let config_path = dir.path().join(".factory/config.yaml");
    std::fs::write(&config_path, config_yaml).expect("write config.yaml");

    let config = factory_config::load(&config_path).expect("the drill's own config must load");
    let mut store = Store::open(dir.path()).expect("open store");

    let scopes = factory_registry::resolve(&config, dir.path()).expect("resolve");
    let report = factory_registry::reconcile(&store, &scopes).expect("reconcile");
    factory_registry::apply(&mut store, &scopes, &report).expect("apply");

    let after = factory_registry::reconcile(&store, &scopes).expect("reconcile again");
    assert!(
        after.is_clean(),
        "a freshly applied registry must reconcile clean: {after:?}"
    );

    Instance {
        dir,
        store,
        root: uuid(ROOT_ID),
        alpha: uuid(ALPHA_ID),
        beta: uuid(BETA_ID),
        gamma: uuid(GAMMA_ID),
    }
}

pub fn uuid(s: &str) -> uuid::Uuid {
    uuid::Uuid::parse_str(s).expect("valid uuid")
}

pub fn uid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

/// Records what was written to which session, standing in for a real terminal.
/// Design §5 hands the prompt to an operator or a harness; a drill needs to
/// see *that* it was handed over exactly once, and to whom.
#[derive(Default)]
pub struct RecordingWriter {
    pub calls: Vec<(uuid::Uuid, String)>,
}

impl factory_task::deliver::PromptWriter for RecordingWriter {
    fn write_prompt(
        &mut self,
        session_id: uuid::Uuid,
        prompt: &str,
    ) -> Result<(), factory_task::deliver::PromptWriteError> {
        self.calls.push((session_id, prompt.to_string()));
        Ok(())
    }
}
