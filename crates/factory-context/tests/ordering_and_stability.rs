//! Design §2.5 fixes the compiled order (company, ancestors, current scope,
//! agent definition, task prompt) and requires the result to be byte-stable
//! for the same scope and task. Both are checked here directly against
//! `compile`'s return value rather than against a hand-written expected blob,
//! so a failure points at what actually differed.

mod support;

use factory_context::{ScopeContext, compile};

#[test]
fn same_logical_input_compiles_to_identical_bytes() {
    let temp = tempfile::tempdir().expect("can create a temp dir");
    let root_path = temp.path().join("AGENTS.md");
    let leaf_path = temp.path().join("leaf/AGENTS.md");
    support::write_file(&root_path, "root conventions\n");
    support::write_file(&leaf_path, "leaf conventions\n");

    // Two independently constructed argument sets that describe the same
    // logical compilation. If `compile` ever grew a HashMap in the middle of
    // its pipeline, or picked up a timestamp, these two calls would diverge
    // even though "company root, then leaf" never changed.
    let scopes_a = vec![
        ScopeContext {
            scope_name: "company".to_string(),
            context_file: root_path.clone(),
        },
        ScopeContext {
            scope_name: "leaf".to_string(),
            context_file: leaf_path.clone(),
        },
    ];
    let scope_names = ["company", "leaf"];
    let scope_paths = [root_path.clone(), leaf_path.clone()];
    let scopes_b: Vec<ScopeContext> = scope_names
        .into_iter()
        .zip(scope_paths)
        .map(|(name, path)| ScopeContext {
            scope_name: name.to_string(),
            context_file: path,
        })
        .collect();

    let agent_a = support::sample_agent();
    let agent_b = support::sample_agent();

    let first = compile(&scopes_a, &agent_a, Some("do the task")).expect("compiles");
    let second = compile(&scopes_b, &agent_b, Some("do the task")).expect("compiles");

    assert_eq!(first.text, second.text);
    assert_eq!(first.sources, second.sources);

    // The simplest form of the same property: compiling the very same values
    // twice must agree with itself.
    let third = compile(&scopes_a, &agent_a, Some("do the task")).expect("compiles");
    assert_eq!(first.text, third.text);
}

#[test]
fn compiled_order_is_root_then_ancestor_then_leaf_then_agent_then_task() {
    let temp = tempfile::tempdir().expect("can create a temp dir");
    let root_path = temp.path().join("AGENTS.md");
    let middle_path = temp.path().join("mid/AGENTS.md");
    let leaf_path = temp.path().join("mid/leaf/AGENTS.md");
    support::write_file(&root_path, "ROOT-MARKER-TEXT");
    support::write_file(&middle_path, "MIDDLE-MARKER-TEXT");
    support::write_file(&leaf_path, "LEAF-MARKER-TEXT");

    let scopes = vec![
        ScopeContext {
            scope_name: "company".to_string(),
            context_file: root_path,
        },
        ScopeContext {
            scope_name: "middle".to_string(),
            context_file: middle_path,
        },
        ScopeContext {
            scope_name: "leaf".to_string(),
            context_file: leaf_path,
        },
    ];
    let agent = support::sample_agent();

    let compiled = compile(&scopes, &agent, Some("TASK-MARKER-TEXT")).expect("compiles");
    let text = &compiled.text;

    let root_pos = text.find("ROOT-MARKER-TEXT").expect("root content present");
    let middle_pos = text
        .find("MIDDLE-MARKER-TEXT")
        .expect("middle content present");
    let leaf_pos = text.find("LEAF-MARKER-TEXT").expect("leaf content present");
    let agent_pos = text
        .find("Agent definition")
        .expect("agent section present");
    let task_pos = text.find("TASK-MARKER-TEXT").expect("task content present");

    assert!(
        root_pos < middle_pos,
        "company root must precede the middle (ancestor) scope"
    );
    assert!(
        middle_pos < leaf_pos,
        "the ancestor scope must precede the current (leaf) scope"
    );
    assert!(
        leaf_pos < agent_pos,
        "the agent definition must follow every scope"
    );
    assert!(agent_pos < task_pos, "the task prompt must come last");
}
