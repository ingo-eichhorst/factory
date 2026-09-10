//! `factory_task::template`'s own test suite: creating, reading, revising,
//! and pausing/closing task templates (design §11, §12.3; ADR 0021
//! decision 1).
//!
//! See `tests/create.rs`'s own module docs for why some fixtures here bypass
//! this crate's public API and seed rows directly with raw SQL — the same
//! reasoning applies wherever a test needs a neighbouring table this slice
//! does not own the creation of (`scopes`).

use factory_store::Store;
use factory_task::create::{create, create_from_template, show};
use factory_task::template::{TemplateError, TemplateState, create as create_template};

/// A deterministic, distinct, syntactically valid UUID — mirrors
/// `tests/create.rs::uid`. `uuid` is pinned workspace-wide without the `v4`
/// feature, so tests build UUIDs by hand from a seed.
fn uid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

/// Insert a scope row directly, bypassing `factory-registry` (which this
/// crate does not depend on) — mirrors `tests/create.rs::seed_scope`.
fn seed_scope(store: &mut Store, seed: u32, name: &str, canonical_path: &str) -> uuid::Uuid {
    let id = uid(seed);
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO scopes (id, name, declared_path, canonical_path) VALUES (?1, ?2, ?3, ?3)",
        (id.to_string(), name, canonical_path),
    )
    .expect("insert scope");
    tx.commit().expect("commit");
    id
}

// create -----------------------------------------------------------------

/// The plain path: a template with no target scope at all is legal —
/// design §11's run that "remains queued for the central agent to assign."
/// `version` starts at 1 (schema default) and `state` starts `open`.
#[test]
fn create_with_no_target_scope_starts_open_at_version_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let template_id = uid(1);
    create_template(
        &mut store,
        template_id,
        "nightly-report",
        None,
        None,
        "write the nightly report",
        None,
    )
    .expect("create a template with no target");

    let template = factory_task::template::get_by_id(&store, template_id).expect("get_by_id");
    assert_eq!(template.name, "nightly-report");
    assert_eq!(
        template.target_scope_id, None,
        "no target means central assignment, design §11"
    );
    assert_eq!(template.prompt, "write the nightly report");
    assert_eq!(template.acceptance_criteria, None);
    assert_eq!(template.version, 1);
    assert_eq!(template.state, TemplateState::Open);
}

#[test]
fn create_records_a_target_scope_and_agent_and_acceptance_criteria_when_given() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");

    let template_id = uid(2);
    create_template(
        &mut store,
        template_id,
        "weekly-audit",
        Some(scope_id),
        Some("auditor"),
        "audit the week",
        Some("every finding has a linked commit"),
    )
    .expect("create a template with a target");

    let template = factory_task::template::get_by_id(&store, template_id).expect("get_by_id");
    assert_eq!(template.target_scope_id, Some(scope_id));
    assert_eq!(template.target_agent_name, Some("auditor".to_string()));
    assert_eq!(
        template.acceptance_criteria,
        Some("every finding has a linked commit".to_string())
    );
}

/// `task_templates_name` is a real unique index, not merely a convention —
/// a duplicate name must come back as a typed error a caller can match on,
/// never a panic and never a raw `rusqlite` error leaking through.
#[test]
fn a_duplicate_name_is_a_typed_error_not_a_panic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    create_template(
        &mut store,
        uid(1),
        "nightly-report",
        None,
        None,
        "first prompt",
        None,
    )
    .expect("create the first template");

    let err = create_template(
        &mut store,
        uid(2),
        "nightly-report",
        None,
        None,
        "a different prompt entirely",
        None,
    )
    .expect_err("a second template with the same name must be refused");
    assert!(
        matches!(&err, TemplateError::NameTaken(name) if name == "nightly-report"),
        "expected NameTaken, got {err:?}"
    );

    // The refused create must not have touched the first template's row.
    let survivor = factory_task::template::get_by_name(&store, "nightly-report")
        .expect("the first template is still there");
    assert_eq!(survivor.id, uid(1));
    assert_eq!(survivor.prompt, "first prompt");
}

/// `create`'s own pre-check turns a duplicate name into `NameTaken` before
/// any `INSERT` is attempted, so nothing above exercises
/// `task_templates_name` itself. This is the backstop, proved directly:
/// `factory_session::begin_start`'s module docs make the identical argument
/// for `sessions_one_live_lease_per_workspace` — a Rust-level pre-check
/// "is deliberately not treated as *the* enforcement mechanism," and the
/// index is what remains true if the check ever had a bug, were bypassed, or
/// were deleted outright. A raw `INSERT` naming an already-used name, run
/// straight against the connection rather than through `create`, must still
/// be refused.
#[test]
fn the_name_uniqueness_index_itself_refuses_a_duplicate_bypassing_the_typed_precheck() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    create_template(
        &mut store,
        uid(1),
        "nightly-report",
        None,
        None,
        "first prompt",
        None,
    )
    .expect("create the first template");

    let result = store.connection().execute(
        "INSERT INTO task_templates (id, name, prompt) VALUES (?1, 'nightly-report', 'do it')",
        [uid(2).to_string()],
    );
    assert!(
        result.is_err(),
        "task_templates_name must reject a duplicate even when create's own \
         pre-check is bypassed entirely"
    );
}

// get_by_id / get_by_name / list -------------------------------------------

#[test]
fn get_by_id_of_a_nonexistent_template_is_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path()).expect("open");

    let err =
        factory_task::template::get_by_id(&store, uid(999)).expect_err("no such template exists");
    assert!(matches!(err, TemplateError::NotFound(id) if id == uid(999)));
}

#[test]
fn get_by_name_of_a_nonexistent_template_is_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path()).expect("open");

    let err = factory_task::template::get_by_name(&store, "does-not-exist")
        .expect_err("no such template exists");
    assert!(matches!(err, TemplateError::NoSuchName(name) if name == "does-not-exist"));
}

#[test]
fn list_returns_every_template_oldest_first() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let first = uid(1);
    let second = uid(2);
    create_template(&mut store, first, "first", None, None, "do it", None).expect("create first");
    create_template(&mut store, second, "second", None, None, "do it", None)
        .expect("create second");

    let templates = factory_task::template::list(&store).expect("list");
    let ids: Vec<uuid::Uuid> = templates.iter().map(|t| t.id).collect();
    assert_eq!(ids, vec![first, second]);
}

// revise -------------------------------------------------------------------

/// §12.3's hook, read back directly: `version` starts at 1 and one `revise`
/// call makes it 2.
#[test]
fn revise_bumps_version_from_one_to_two() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let template_id = uid(1);
    create_template(
        &mut store,
        template_id,
        "nightly-report",
        None,
        None,
        "first prompt",
        None,
    )
    .expect("create");

    let new_version =
        factory_task::template::revise(&mut store, template_id, Some("revised prompt"), None)
            .expect("revise");
    assert_eq!(new_version, 2);

    let template = factory_task::template::get_by_id(&store, template_id).expect("get_by_id");
    assert_eq!(template.version, 2);
    assert_eq!(template.prompt, "revised prompt");
    assert_eq!(
        template.acceptance_criteria, None,
        "acceptance_criteria was not passed to revise, so it must be untouched"
    );
}

/// Revising only `acceptance_criteria` must leave `prompt` untouched — the
/// two fields are independently optional, not an all-or-nothing pair.
#[test]
fn revise_can_change_only_acceptance_criteria_and_leaves_prompt_untouched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let template_id = uid(1);
    create_template(
        &mut store,
        template_id,
        "nightly-report",
        None,
        None,
        "the original prompt",
        None,
    )
    .expect("create");

    factory_task::template::revise(&mut store, template_id, None, Some("must compile"))
        .expect("revise");

    let template = factory_task::template::get_by_id(&store, template_id).expect("get_by_id");
    assert_eq!(template.prompt, "the original prompt");
    assert_eq!(
        template.acceptance_criteria,
        Some("must compile".to_string())
    );
}

#[test]
fn revise_of_a_nonexistent_template_is_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let err = factory_task::template::revise(&mut store, uid(999), Some("x"), None)
        .expect_err("no such template exists");
    assert!(matches!(err, TemplateError::NotFound(id) if id == uid(999)));
}

/// **The load-bearing test.** ADR 0021 decision 2 / backlog §11: "every run
/// records the template version it executed, and that record does not
/// change when the template is later revised." Create a template, create a
/// run from it, revise the template twice, and confirm the run still
/// reports the version it executed at creation — not the template's current
/// version, and not the version at the moment the run finished, if it ever
/// does.
#[test]
fn a_run_keeps_the_template_version_it_executed_across_later_revisions() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");

    let template_id = uid(1);
    create_template(
        &mut store,
        template_id,
        "nightly-report",
        Some(scope_id),
        None,
        "version 1 of the prompt",
        None,
    )
    .expect("create");

    // Revise once *before* the run exists, so the run executes version 2 and
    // not version 1. Without this the test cannot tell "frozen at the version
    // it executed" apart from "always writes 1" — both leave a `Some(1)` on a
    // run made from a fresh template, and the second is a real mutation this
    // assertion must kill.
    factory_task::template::revise(
        &mut store,
        template_id,
        Some("version 2 of the prompt"),
        None,
    )
    .expect("revision before the run");

    let task_id = uid(50);
    create_from_template(
        &mut store,
        task_id,
        None,
        scope_id,
        None,
        None,
        "version 2 of the prompt",
        &[scope_id],
        template_id,
    )
    .expect("create a run from the template");

    let task = show(&store, task_id).expect("show");
    assert_eq!(task.template_id, Some(template_id));
    assert_eq!(
        task.template_version,
        Some(2),
        "the run must record the version that existed when it was created"
    );

    factory_task::template::revise(
        &mut store,
        template_id,
        Some("version 3 of the prompt"),
        None,
    )
    .expect("first revision after the run");
    factory_task::template::revise(
        &mut store,
        template_id,
        Some("version 4 of the prompt"),
        None,
    )
    .expect("second revision after the run");

    let template = factory_task::template::get_by_id(&store, template_id).expect("get_by_id");
    assert_eq!(
        template.version, 4,
        "the template itself has moved on to version 4"
    );

    let task_after = show(&store, task_id).expect("show again");
    assert_eq!(
        task_after.template_version,
        Some(2),
        "the run's recorded version must not move when the template is revised"
    );
    assert_eq!(
        task_after.template_id,
        Some(template_id),
        "template_id itself is never touched by revise either"
    );
}

// create_from_template -------------------------------------------------

/// A run created without a template has NULL `template_id` and NULL
/// `template_version`, and otherwise behaves exactly as `create` always
/// has — read back through `show` and moved through `cancel`, the two
/// paths every pre-existing caller of `create` already exercises.
#[test]
fn a_run_created_without_a_template_has_null_template_fields_and_behaves_as_before() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = seed_scope(&mut store, 1, "irrlicht", "/instance");

    let task_id = uid(50);
    create(
        &mut store,
        task_id,
        None,
        scope_id,
        None,
        None,
        "do the thing",
        &[],
    )
    .expect("create a plain task");

    let task = show(&store, task_id).expect("show");
    assert_eq!(task.template_id, None);
    assert_eq!(task.template_version, None);
    assert_eq!(task.status, factory_task::TaskStatus::Queued);

    let outcome = factory_task::create::cancel(&mut store, task_id).expect("cancel");
    assert_eq!(
        outcome,
        factory_task::TaskStatus::Cancelled,
        "cancellation of a template-less queued task is unaffected by this station's changes"
    );
}

// `create_from_template` rejecting a `template_id` that names no real
// template is proved in `tests/create.rs`
// (`create_from_template_rejects_a_template_id_that_names_no_real_template`)
// — that is `create`'s own INSERT and its own REFERENCES backstop, so it
// lives with `create`'s other tests of the same shape rather than here.

// set_state ------------------------------------------------------------

/// Round trip through the typed API for all three states, then prove the
/// schema's own CHECK is still a backstop for anything that reaches this
/// table by a path other than `set_state` — mirrors
/// `tests::every_adapter_blocked_reason_maps_to_a_check_accepted_string` in
/// `src/lib.rs`, which makes the identical argument for `blocked_reason`.
#[test]
fn state_round_trips_through_the_typed_api_and_the_schema_check_is_a_backstop() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let template_id = uid(1);
    create_template(
        &mut store,
        template_id,
        "nightly-report",
        None,
        None,
        "do it",
        None,
    )
    .expect("create");

    for state in [
        TemplateState::Paused,
        TemplateState::Closed,
        TemplateState::Open,
    ] {
        factory_task::template::set_state(&mut store, template_id, state).expect("set_state");
        let template = factory_task::template::get_by_id(&store, template_id).expect("get_by_id");
        assert_eq!(template.state, state);
    }

    // The backstop: a value outside the vocabulary, written directly,
    // must still be refused by `task_templates.state`'s own CHECK.
    let result = store.connection().execute(
        "UPDATE task_templates SET state = 'archived' WHERE id = ?1",
        [template_id.to_string()],
    );
    assert!(
        result.is_err(),
        "the schema's CHECK constraint must reject a state outside open/paused/closed"
    );

    let template = factory_task::template::get_by_id(&store, template_id).expect("get_by_id");
    assert_eq!(
        template.state,
        TemplateState::Open,
        "the rejected UPDATE must not have changed the stored state"
    );
}

#[test]
fn set_state_of_a_nonexistent_template_is_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    let err = factory_task::template::set_state(&mut store, uid(999), TemplateState::Paused)
        .expect_err("no such template exists");
    assert!(matches!(err, TemplateError::NotFound(id) if id == uid(999)));
}
