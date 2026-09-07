//! `SourceReport::bytes` counts each section's whole rendered text -- rule
//! lines, header, and blank line included, not only the file's own payload
//! (see the doc comment on `SourceReport::bytes`). Because `compile`
//! concatenates sections with nothing in between, that choice makes the
//! reconciliation exact: summing every report's `bytes` must equal
//! `text.len()`, with no separator-overhead constant to invent.

mod support;

use factory_context::{ScopeContext, compile};

#[test]
fn source_report_bytes_reconcile_exactly_with_compiled_text_length() {
    let temp = tempfile::tempdir().expect("can create a temp dir");
    let root_path = temp.path().join("AGENTS.md");
    let leaf_path = temp.path().join("leaf/AGENTS.md");
    support::write_file(&root_path, "root conventions, somewhat longer text here\n");
    support::write_file(&leaf_path, "leaf conventions");

    let scopes = vec![
        ScopeContext {
            scope_name: "company".to_string(),
            context_file: root_path.clone(),
        },
        ScopeContext {
            scope_name: "leaf".to_string(),
            context_file: leaf_path.clone(),
        },
    ];
    let agent = support::sample_agent();

    let compiled = compile(&scopes, &agent, Some("do the task")).expect("compiles");

    // Four sources: two scopes, the agent definition, and the task.
    assert_eq!(compiled.sources.len(), 4);

    let total: usize = compiled.sources.iter().map(|s| s.bytes).sum();
    assert_eq!(
        total,
        compiled.text.len(),
        "sources must account for the whole output, with no overhead left unexplained"
    );

    assert_eq!(compiled.sources[0].label, "Context: company");
    // The reported path is the one an operator would cross-reference against
    // the `Source:` line actually printed in `text`, not merely "some path".
    assert_eq!(compiled.sources[0].path, Some(root_path));
    assert_eq!(compiled.sources[1].label, "Context: leaf");
    assert_eq!(compiled.sources[1].path, Some(leaf_path));
    assert_eq!(compiled.sources[2].label, "Agent definition");
    assert_eq!(compiled.sources[2].path, None);
    assert_eq!(compiled.sources[3].label, "Task");
    assert_eq!(compiled.sources[3].path, None);

    for source in &compiled.sources {
        assert!(
            source.bytes > 0,
            "every contributing source reports a nonzero size: {source:?}"
        );
    }
}

#[test]
fn sources_still_reconcile_with_no_task_prompt() {
    let temp = tempfile::tempdir().expect("can create a temp dir");
    let root_path = temp.path().join("AGENTS.md");
    support::write_file(&root_path, "root conventions");

    let scopes = vec![ScopeContext {
        scope_name: "company".to_string(),
        context_file: root_path,
    }];
    let agent = support::sample_agent();

    let compiled = compile(&scopes, &agent, None).expect("compiles");
    // One scope plus the agent definition; no task section at all.
    assert_eq!(compiled.sources.len(), 2);
    assert!(compiled.sources.iter().all(|s| s.label != "Task"));

    let total: usize = compiled.sources.iter().map(|s| s.bytes).sum();
    assert_eq!(total, compiled.text.len());
}
