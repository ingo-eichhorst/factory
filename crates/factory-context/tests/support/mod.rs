//! Shared helpers for `factory-context`'s integration tests.
//!
//! Each integration test file is its own crate, so a helper unused by a
//! particular file would be a dead-code warning there; keep this module
//! small so every test file that includes it actually uses everything in it.

/// A minimal, valid agent definition, good enough for tests that don't care
/// about its specific fields.
pub fn sample_agent() -> factory_config::Agent {
    factory_config::Agent {
        name: "example-project".to_string(),
        harness: factory_config::Harness::Pi,
        max_sessions: 1,
        lifetime: factory_config::Lifetime::Permanent,
    }
}

/// Write `contents` to `path`, creating any missing parent directories.
///
/// Every test in this crate must build its own inputs under a `tempfile`
/// temp dir rather than touching the real repository (`fixtures/registration/`
/// is the one documented exception), so this is the one place that does the
/// writing they need.
pub fn write_file(path: &std::path::Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("can create the fixture file's parent directory");
    }
    std::fs::write(path, contents).expect("can write the fixture file");
}
