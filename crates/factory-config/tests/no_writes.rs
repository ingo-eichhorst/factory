//! `factory-config` performs no filesystem writes at all -- not even a cache,
//! a lockfile, or a log, and not even when validation fails. Per
//! `docs/slice-1-error-corpus.md`'s "What validation must not do".
//!
//! This is checked directly rather than assumed: copy fixtures into a fresh
//! temp directory, snapshot every entry's size and mtime, run both `load` and
//! `parse` over it -- success and failure paths alike -- and assert the
//! directory tree is byte-for-byte and entry-for-entry identical afterward.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// path (relative to the temp dir root) -> (is_dir, size, mtime).
///
/// Directories are recorded too, not only files: "no new entry appeared
/// anywhere in the temp dir" must catch a stray created directory (a `.cache/`
/// with nothing in it yet, say), not only a stray file.
type Snapshot = BTreeMap<PathBuf, (bool, u64, SystemTime)>;

fn snapshot(root: &Path) -> Snapshot {
    fn walk(dir: &Path, root: &Path, out: &mut Snapshot) {
        for entry in fs::read_dir(dir).expect("temp dir is readable") {
            let entry = entry.expect("dir entry readable");
            let path = entry.path();
            let metadata = entry.metadata().expect("metadata readable");
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            let mtime = metadata
                .modified()
                .expect("mtime available on this platform");
            let is_dir = metadata.is_dir();
            out.insert(relative, (is_dir, metadata.len(), mtime));
            if is_dir {
                walk(&path, root, out);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

#[test]
fn load_and_parse_write_nothing() {
    let temp = tempfile::tempdir().expect("can create a temp dir");

    // A mix of fixtures that succeed and fixtures that fail, so both code
    // paths run against the same directory before it is snapshotted.
    let sources = [
        ("valid/root-shorthand.yaml", "root-shorthand.yaml"),
        ("valid/multi-agent.yaml", "multi-agent.yaml"),
        (
            "invalid/agent-and-agents-both-set.yaml",
            "agent-and-agents-both-set.yaml",
        ),
        ("invalid/empty.yaml", "empty.yaml"),
        ("invalid/uuid-invalid.yaml", "uuid-invalid.yaml"),
    ];

    let mut copied_paths = Vec::new();
    for (source, dest_name) in sources {
        let source_path = manifest_dir().join("fixtures").join(source);
        let dest_path = temp.path().join(dest_name);
        fs::copy(&source_path, &dest_path).expect("fixture copies into the temp dir");
        copied_paths.push(dest_path);
    }

    let before = snapshot(temp.path());
    assert_eq!(
        before.len(),
        sources.len(),
        "sanity check: every fixture was copied in"
    );

    // Exercise `load` (reads from disk) over every copy, both outcomes.
    for path in &copied_paths {
        let _ = factory_config::load(path);
    }

    // Exercise `parse` (reads already in memory) over the same content, both
    // outcomes -- `parse` never opens `origin` at all, but this proves it
    // regardless of whether `origin` names a real file.
    for path in &copied_paths {
        let text = fs::read_to_string(path).expect("just-copied fixture is readable");
        let _ = factory_config::parse(&text, path);
    }

    let after = snapshot(temp.path());

    assert_eq!(
        before, after,
        "load()/parse() must not create, delete, or modify any file, including on the failure paths"
    );

    let new_entries: Vec<_> = after
        .keys()
        .filter(|path| !before.contains_key(*path))
        .collect();
    assert!(
        new_entries.is_empty(),
        "no new entry may appear anywhere in the temp dir; found {new_entries:?}"
    );
}
