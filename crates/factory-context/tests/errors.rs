//! ADR 0013 decision 2: a context source that cannot be read stops
//! compilation -- it is never replaced by an empty section -- and the error
//! names the scope, the path, and an action. ADR 0013's other hard stop is an
//! empty scope list, since context always begins at the company root.

mod support;

use factory_context::{ContextError, ScopeContext, compile};

#[test]
fn missing_context_file_is_an_unreadable_source_error() {
    let temp = tempfile::tempdir().expect("can create a temp dir");
    let missing = temp.path().join("does-not-exist/AGENTS.md");
    let agent = support::sample_agent();

    let err = compile(
        &[ScopeContext {
            scope_name: "orphan-scope".to_string(),
            context_file: missing.clone(),
        }],
        &agent,
        None,
    )
    .expect_err("a missing context file must not compile");

    match &err {
        ContextError::UnreadableSource {
            scope_name, path, ..
        } => {
            assert_eq!(scope_name, "orphan-scope");
            assert_eq!(path, &missing);
        }
        other => panic!("expected UnreadableSource, got {other:?}"),
    }

    let rendered = err.to_string();
    assert!(rendered.contains("orphan-scope"), "{rendered}");
    assert!(
        rendered.contains(&missing.display().to_string()),
        "{rendered}"
    );
    assert!(rendered.contains("help:"), "{rendered}");
}

#[cfg(unix)]
#[test]
fn permission_denied_context_file_is_also_an_unreadable_source_error() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("can create a temp dir");
    let path = temp.path().join("AGENTS.md");
    support::write_file(&path, "secret conventions");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000))
        .expect("can set permissions to unreadable");

    // Root ignores Unix permission bits, so a 0o000 file is still readable
    // when the test happens to run as root. That is a property of the
    // environment, not of `compile`, so skip the assertion rather than fail
    // on it.
    if std::fs::read_to_string(&path).is_ok() {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).ok();
        eprintln!("skipping: this process can read a 0o000 file (running as root?)");
        return;
    }

    let agent = support::sample_agent();
    let result = compile(
        &[ScopeContext {
            scope_name: "locked-scope".to_string(),
            context_file: path.clone(),
        }],
        &agent,
        None,
    );

    // Restore permissions unconditionally so the tempdir can clean itself up.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).ok();

    let err = result.expect_err("a permission-denied context file must not compile");
    match err {
        ContextError::UnreadableSource { scope_name, .. } => {
            assert_eq!(scope_name, "locked-scope");
        }
        other => panic!("expected UnreadableSource, got {other:?}"),
    }
}

#[test]
fn empty_scope_list_is_rejected() {
    let agent = support::sample_agent();
    let err = compile(&[], &agent, None).expect_err("an empty scope list must not compile");
    assert!(matches!(err, ContextError::NoScopes));
}
