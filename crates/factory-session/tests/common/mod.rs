//! Shared helpers for factory-session's integration tests.

#![allow(dead_code)]

use std::path::Path;

use factory_paths::CanonicalPath;
use factory_store::Store;

/// A deterministic, distinct, syntactically valid UUID. `uuid` is pinned
/// workspace-wide without the `v4` feature (see
/// `crates/factory-registry/tests/registry.rs`'s `uid` helper, which this
/// mirrors), so tests build UUIDs by hand from a seed rather than generating
/// them.
pub fn uid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

/// Resolve `path`, panicking with the test's own context on failure — every
/// caller has just created the directory itself, so a failure here means the
/// test fixture is broken, not that the behaviour under test failed.
pub fn resolve(path: &Path) -> CanonicalPath {
    CanonicalPath::resolve(path).expect("resolve a directory this test just created")
}

/// Insert a scope row directly, bypassing `factory-registry` (which this
/// crate does not depend on), so tests have a `scope_id` to satisfy
/// `sessions.scope_id REFERENCES scopes (id)`. Returns the id it used.
pub fn seed_scope(store: &mut Store, seed: u32, name: &str, canonical_path: &Path) -> uuid::Uuid {
    let id = uid(seed);
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO scopes (id, name, declared_path, canonical_path) VALUES (?1, ?2, ?3, ?3)",
        (
            id.to_string(),
            name,
            canonical_path.to_string_lossy().into_owned(),
        ),
    )
    .expect("insert scope");
    tx.commit().expect("commit");
    id
}
