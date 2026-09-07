//! Per ADR 0013 decision 1, `factory-context` is a pure function: it opens
//! files for reading and creates nothing -- no output path, no cache, no
//! temp file, not even on the error path. Checked directly rather than
//! assumed: snapshot a temp dir's full recursive listing, compile against
//! files in it (success and failure alike), snapshot again, and assert the
//! two are identical.

mod support;

use factory_context::{ScopeContext, compile};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// path (relative to the temp dir root) -> (is_dir, size, mtime).
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
fn compile_never_writes_to_disk() {
    let temp = tempfile::tempdir().expect("can create a temp dir");
    let root_path = temp.path().join("AGENTS.md");
    let leaf_path = temp.path().join("leaf/AGENTS.md");
    support::write_file(&root_path, "root conventions\n");
    support::write_file(&leaf_path, "leaf conventions\n");

    let before = snapshot(temp.path());

    let agent = support::sample_agent();

    // The success path.
    let scopes = vec![
        ScopeContext {
            scope_name: "company".to_string(),
            context_file: root_path,
        },
        ScopeContext {
            scope_name: "leaf".to_string(),
            context_file: leaf_path,
        },
    ];
    let ok = compile(&scopes, &agent, Some("a task"));
    assert!(
        ok.is_ok(),
        "sanity check: the success path actually succeeds"
    );

    // The `UnreadableSource` failure path: a scope naming a file that was
    // never created under the temp dir.
    let failing_scopes = vec![ScopeContext {
        scope_name: "ghost".to_string(),
        context_file: temp.path().join("missing/AGENTS.md"),
    }];
    let missing_file_err = compile(&failing_scopes, &agent, None);
    assert!(
        missing_file_err.is_err(),
        "sanity check: the missing-file path actually fails"
    );

    // The `NoScopes` failure path, which touches no filesystem input at all.
    let no_scopes_err = compile(&[], &agent, None);
    assert!(no_scopes_err.is_err());

    let after = snapshot(temp.path());
    assert_eq!(
        before, after,
        "compile() must not create, delete, or modify any file, on success or on either failure path"
    );
}
