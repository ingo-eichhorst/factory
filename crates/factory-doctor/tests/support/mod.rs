//! Shared fixture helpers for `factory-doctor`'s integration tests.
//!
//! Fixture style mirrors `factory_session`'s own
//! `reconcile_to_disconnected_tests` module (raw-SQL scope seeding, a
//! deterministic UUID built from a seed, driving a session through the real
//! public API to a target state) for the same reason that module gives:
//! deterministic ids make failure output legible, and building a session
//! through `begin_start`/`mark_running`/`stop`/`fail` rather than
//! hand-writing its row means a fixture cannot drift from what a real
//! caller can actually produce.

#![allow(dead_code)] // Not every test file uses every helper here.

use std::path::Path;

use factory_paths::CanonicalPath;

pub fn uid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

/// Insert a `scopes` row directly, in the schema-2-and-later shape
/// (`declared_path`/`canonical_path` both required). Mirrors
/// `factory_session`'s own `seed_scope` test helper, which is private to
/// that crate and so cannot be imported.
pub fn seed_scope(store: &mut factory_store::Store, seed: u32, path: &Path) -> uuid::Uuid {
    let id = uid(seed);
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO scopes (id, name, declared_path, canonical_path) VALUES (?1, 'root', ?2, ?2)",
        (id.to_string(), path.to_string_lossy().into_owned()),
    )
    .expect("insert scope");
    tx.commit().expect("commit");
    id
}

/// Drives a fresh session, in its own workspace directory, from
/// `begin_start` through this crate's own public transitions until it
/// reaches `target` — reusing the real API rather than hand-writing rows.
pub fn session_in(
    store: &mut factory_store::Store,
    seed: u32,
    scope_id: uuid::Uuid,
    workspace: &Path,
    target: factory_session::SessionState,
) -> uuid::Uuid {
    std::fs::create_dir_all(workspace).expect("create workspace dir");
    let id = uid(seed);
    let canonical = CanonicalPath::resolve(workspace).expect("resolve workspace");
    factory_session::begin_start(store, id, scope_id, "agent", 10, &canonical)
        .expect("begin_start");

    use factory_session::SessionState;
    match target {
        SessionState::Starting => {}
        SessionState::Running => {
            factory_session::mark_running(store, id).expect("mark_running");
        }
        SessionState::Disconnected => {
            factory_session::mark_running(store, id).expect("mark_running");
            factory_session::mark_disconnected(store, id).expect("mark_disconnected");
        }
        SessionState::Stopped => {
            factory_session::mark_running(store, id).expect("mark_running");
            factory_session::stop(store, id, "test fixture").expect("stop");
        }
        SessionState::Failed => {
            factory_session::fail(store, id, "test fixture").expect("fail");
        }
    }
    id
}

/// A minimal, valid `.factory/config.yaml` naming exactly one scope at
/// `scope_path` (relative to `company_root`), named `"root"` — matching
/// [`seed_scope`]'s own hardcoded name, so a test that seeds both a scope
/// row and a config entry for the same `scope_id` does not also trip
/// `Drift::NameChanged` by accident.
pub fn write_valid_config(
    company_root: &Path,
    instance_id: uuid::Uuid,
    scope_id: uuid::Uuid,
    scope_path: &str,
) {
    let dir = company_root.join(".factory");
    std::fs::create_dir_all(&dir).expect("create .factory");
    let yaml = format!(
        "version: 1\n\
         instance:\n\
         \x20 id: \"{instance_id}\"\n\
         \x20 name: \"test-instance\"\n\
         scopes:\n\
         \x20 - id: \"{scope_id}\"\n\
         \x20   name: \"root\"\n\
         \x20   path: \"{scope_path}\"\n\
         \x20   agent:\n\
         \x20     name: \"agent\"\n\
         \x20     harness: \"pi\"\n"
    );
    std::fs::write(dir.join("config.yaml"), yaml).expect("write config.yaml");
}

/// A minimal, valid `.factory/config.yaml` declaring no scopes at all — a
/// freshly initialized instance.
pub fn write_valid_config_with_no_scopes(company_root: &Path, instance_id: uuid::Uuid) {
    let dir = company_root.join(".factory");
    std::fs::create_dir_all(&dir).expect("create .factory");
    let yaml = format!(
        "version: 1\n\
         instance:\n\
         \x20 id: \"{instance_id}\"\n\
         \x20 name: \"test-instance\"\n\
         scopes: []\n"
    );
    std::fs::write(dir.join("config.yaml"), yaml).expect("write config.yaml");
}

/// Fake [`factory_doctor::LivePanes`] returning a fixed set of pane ids.
pub struct FakePanes(pub Vec<String>);

impl factory_doctor::LivePanes for FakePanes {
    fn live_pane_ids(&self) -> Result<Vec<String>, String> {
        Ok(self.0.clone())
    }
}

/// Fake [`factory_doctor::LaunchdAccess`] returning a fixed answer for every
/// label.
pub enum FakeLaunchd {
    Loaded(String),
    NotLoaded,
    Error(String),
}

impl factory_doctor::LaunchdAccess for FakeLaunchd {
    fn list(&self, _label: &str) -> Result<Option<String>, String> {
        match self {
            FakeLaunchd::Loaded(raw) => Ok(Some(raw.clone())),
            FakeLaunchd::NotLoaded => Ok(None),
            FakeLaunchd::Error(err) => Err(err.clone()),
        }
    }
}
