//! Integration tests for `factory-registry`: the projection (`resolve`), the
//! read-only comparison (`reconcile`), and the transactional repair
//! (`apply`).
//!
//! Every scenario builds its own tiny instance under `tempfile::tempdir()`
//! and its own configuration text, parsed with `factory_config::parse`, per
//! ADR 0016 decision 1 — "the configuration declares, the table projects."
//! The one exception is `fixtures/registration/`, read via a
//! `CARGO_MANIFEST_DIR`-anchored path because `cargo test` sets the working
//! directory to the package root, not the workspace root.
//!
//! Every scenario's scope directories get an `AGENTS.md` by default (via
//! [`write_agents_md`]) so that `Drift::UnreadableContext` — itself checked
//! in its own dedicated test — does not show up as noise in every other
//! scenario. Assertions elsewhere therefore look for "does the report
//! contain variant X", not "is the report exactly one item", except where a
//! scenario is specifically built to produce exactly one.

use std::fs;
use std::path::Path;

use factory_registry::{Drift, DriftReport, RegistryError, apply, reconcile, resolve};
use factory_store::Store;

// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------

/// A deterministic, distinct, syntactically valid UUID. `factory_config`
/// only requires `Uuid::parse_str` to succeed (see
/// `factory-config/src/validate.rs`), not any particular version, and the
/// `uuid` dependency here is pinned without the `v4` feature, so this is the
/// generator tests use instead of `Uuid::new_v4`.
fn uid(seed: u32) -> String {
    format!("00000000-0000-4000-8000-{seed:012x}")
}

/// Write a minimal, readable `AGENTS.md` into `dir`. Scenarios that
/// specifically test `Drift::UnreadableContext` skip this.
fn write_agents_md(dir: &Path) {
    fs::write(dir.join("AGENTS.md"), "# agents\n").expect("write AGENTS.md");
}

/// One entry for [`config_text`].
struct ScopeSpec {
    id: String,
    name: &'static str,
    path: &'static str,
    git: Option<&'static str>,
}

fn scope(id: &str, name: &'static str, path: &'static str) -> ScopeSpec {
    ScopeSpec {
        id: id.to_string(),
        name,
        path,
        git: None,
    }
}

/// Build instance configuration YAML text in the shape
/// `fixtures/registration/.factory/config.yaml` uses, so parsing behaviour
/// exercised here matches the one real fixture read in `resolves_the_real_registration_fixture`.
fn config_text(instance_id: &str, scopes: &[ScopeSpec]) -> String {
    let mut text =
        format!("version: 1\n\ninstance:\n  id: {instance_id}\n  name: test-instance\n\nscopes:\n");
    for s in scopes {
        text.push_str(&format!(
            "  - id: {id}\n    name: {name}\n    path: {path}\n",
            id = s.id,
            name = s.name,
            path = s.path,
        ));
        if let Some(git) = s.git {
            text.push_str(&format!("    git: {git}\n"));
        }
        text.push_str(&format!(
            "    agent:\n      name: {name}Agent\n      harness: claude-code\n      max_sessions: 1\n",
            name = s.name,
        ));
    }
    text
}

fn parse(text: &str) -> factory_config::InstanceConfig {
    factory_config::parse(text, "test-config.yaml").expect("valid test configuration")
}

/// Every `scopes` row's substantive columns (everything but the `created_at`
/// audit timestamp, which a rebuild cannot reproduce and is not part of the
/// projection ADR 0016 requires to be stable), sorted by id so the
/// comparison does not depend on insertion order.
#[allow(clippy::type_complexity)]
fn scopes_snapshot(
    store: &mut Store,
) -> Vec<(
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<String>,
)> {
    let tx = store.transaction().expect("begin read");
    let mut stmt = tx
        .prepare(
            "SELECT id, name, declared_path, canonical_path, git, dev, ino, parent_id \
             FROM scopes ORDER BY id",
        )
        .expect("prepare snapshot query");
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
            ))
        })
        .expect("run snapshot query")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect snapshot rows");
    drop(stmt);
    tx.commit().expect("commit read-only snapshot transaction");
    rows
}

fn drift_id(drift: &Drift) -> uuid::Uuid {
    match drift {
        Drift::DeclaredNotProjected { id, .. }
        | Drift::ProjectedNotDeclared { id, .. }
        | Drift::PathChangedSameIdentity { id, .. }
        | Drift::PathChangedDifferentIdentity { id, .. }
        | Drift::MissingPath { id, .. }
        | Drift::UnreadableContext { id, .. } => *id,
    }
}

fn contains_variant(report: &DriftReport, matches: impl Fn(&Drift) -> bool) -> bool {
    report.items.iter().any(matches)
}

// ---------------------------------------------------------------------
// 1. The rebuild test — ADR 0016 decision 1.
// ---------------------------------------------------------------------

#[test]
fn rebuilding_the_table_from_scratch_reproduces_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agents_md(dir.path());
    let child = dir.path().join("child");
    fs::create_dir(&child).expect("create child");
    write_agents_md(&child);

    let instance_id = uid(1);
    let config = parse(&config_text(
        &instance_id,
        &[
            scope(&uid(2), "root", "."),
            scope(&uid(3), "child", "child"),
        ],
    ));

    let scopes = resolve(&config, dir.path()).expect("resolve");
    assert_eq!(scopes.len(), 2);

    let mut store = Store::open(dir.path()).expect("open store");

    let report = reconcile(&store, &scopes).expect("reconcile");
    apply(&mut store, &scopes, &report).expect("apply");
    let first_snapshot = scopes_snapshot(&mut store);
    assert_eq!(first_snapshot.len(), 2, "both scopes projected once");

    // Wipe every row in `scopes` and project again.
    {
        let tx = store.transaction().expect("begin wipe");
        tx.execute("DELETE FROM scopes", [])
            .expect("delete every scope row");
        tx.commit().expect("commit wipe");
    }
    assert_eq!(scopes_snapshot(&mut store).len(), 0, "table is now empty");

    // Re-resolve from scratch too, not just re-apply the same `scopes`
    // value: ADR 0016 decision 1 is that re-running the *whole* projection
    // (configuration + filesystem -> resolve -> reconcile -> apply)
    // reproduces the table, not merely that re-inserting an already-resolved
    // value is idempotent. Reusing the first `scopes` vector here would let
    // this test pass even if `resolve` were nondeterministic.
    let rebuilt_scopes = resolve(&config, dir.path()).expect("resolve after wipe");
    let rebuild_report = reconcile(&store, &rebuilt_scopes).expect("reconcile after wipe");
    assert_eq!(
        rebuild_report.items.len(),
        2,
        "every wiped scope is DeclaredNotProjected again: {rebuild_report:?}"
    );
    apply(&mut store, &rebuilt_scopes, &rebuild_report).expect("apply after wipe");
    let second_snapshot = scopes_snapshot(&mut store);

    assert_eq!(
        first_snapshot, second_snapshot,
        "deleting every row and re-running the projection must reproduce the table"
    );
}

// ---------------------------------------------------------------------
// 2. Each of the six `Drift` variants, individually.
// ---------------------------------------------------------------------

#[test]
fn declared_not_projected_when_config_names_a_scope_the_table_has_never_seen() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agents_md(dir.path());

    let instance_id = uid(10);
    let scope_id = uid(11);
    let config = parse(&config_text(&instance_id, &[scope(&scope_id, "root", ".")]));
    let scopes = resolve(&config, dir.path()).expect("resolve");

    let store = Store::open(dir.path()).expect("open store");
    let report = reconcile(&store, &scopes).expect("reconcile");

    assert_eq!(report.items.len(), 1);
    assert!(matches!(
        &report.items[0],
        Drift::DeclaredNotProjected { id, .. } if *id == scopes[0].id
    ));
    assert_eq!(scope_id, scopes[0].id.to_string());
}

#[test]
fn projected_not_declared_when_a_human_removes_a_scope_from_the_configuration() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agents_md(dir.path());

    let instance_id = uid(20);
    let scope_id = uid(21);
    let config = parse(&config_text(&instance_id, &[scope(&scope_id, "root", ".")]));
    let scopes = resolve(&config, dir.path()).expect("resolve");

    let mut store = Store::open(dir.path()).expect("open store");
    let report = reconcile(&store, &scopes).expect("reconcile");
    apply(&mut store, &scopes, &report).expect("apply");

    // Reconcile again with no scopes at all — as if the entry had been
    // deleted from the configuration file.
    let report_after_removal = reconcile(&store, &[]).expect("reconcile with empty config");

    assert_eq!(report_after_removal.items.len(), 1);
    assert!(matches!(
        &report_after_removal.items[0],
        Drift::ProjectedNotDeclared { id, .. } if id.to_string() == scope_id
    ));
    assert!(
        !report_after_removal.items[0].is_applicable(),
        "removing a scope is never automatic"
    );
}

#[test]
fn missing_path_when_the_declared_directory_does_not_exist() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agents_md(dir.path());

    let instance_id = uid(30);
    let scope_id = uid(31);
    let config = parse(&config_text(
        &instance_id,
        &[scope(&scope_id, "ghost", "does-not-exist")],
    ));
    let scopes = resolve(&config, dir.path()).expect("resolve does not fail on a missing path");

    assert_eq!(scopes.len(), 1);
    assert!(scopes[0].canonical_path.is_none());
    assert!(scopes[0].file_id.is_none());

    let store = Store::open(dir.path()).expect("open store");
    let report = reconcile(&store, &scopes).expect("reconcile");

    assert!(contains_variant(&report, |d| matches!(
        d,
        Drift::MissingPath { path, .. } if path == Path::new("does-not-exist")
    )));
}

#[test]
fn unreadable_context_when_a_scope_has_no_agents_md() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agents_md(dir.path());
    let child = dir.path().join("child");
    fs::create_dir(&child).expect("create child");
    // Deliberately no AGENTS.md written here.

    let instance_id = uid(40);
    let scope_id = uid(41);
    let config = parse(&config_text(
        &instance_id,
        &[scope(&scope_id, "child", "child")],
    ));
    let scopes = resolve(&config, dir.path()).expect("resolve");
    assert!(scopes[0].canonical_path.is_some(), "the directory exists");

    let store = Store::open(dir.path()).expect("open store");
    let report = reconcile(&store, &scopes).expect("reconcile");

    // `child` may still contain an unresolved symlink component (macOS
    // tempdirs live under `/var/...`, itself a symlink to `/private/var/...`)
    // that `resolve` — which canonicalizes — has already stripped, so the
    // expected path must be canonicalized the same way before comparing.
    let expected_path = child
        .canonicalize()
        .expect("canonicalize child")
        .join("AGENTS.md");
    assert!(
        contains_variant(&report, |d| matches!(
            d,
            Drift::UnreadableContext { path, .. } if path == &expected_path
        )),
        "expected UnreadableContext naming {expected_path:?} in {report:?}"
    );
    assert!(
        !contains_variant(&report, |d| matches!(d, Drift::MissingPath { .. })),
        "the directory exists, so this must not also be MissingPath: {report:?}"
    );
}

// `PathChangedSameIdentity` and `PathChangedDifferentIdentity` are each
// covered by their own dedicated scenario below (the move test and the
// replaced-directory test), since both need more setup than the other four
// variants and the mandatory tests already require exactly those scenarios.

// ---------------------------------------------------------------------
// 3. A move is recognised.
// ---------------------------------------------------------------------

#[test]
fn a_move_is_recognised_as_path_changed_same_identity() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agents_md(dir.path());
    let original = dir.path().join("original");
    fs::create_dir(&original).expect("create original");
    write_agents_md(&original);

    let instance_id = uid(50);
    let scope_id = uid(51);
    let config_before = parse(&config_text(
        &instance_id,
        &[scope(&scope_id, "movable", "original")],
    ));
    let scopes_before = resolve(&config_before, dir.path()).expect("resolve before move");

    let mut store = Store::open(dir.path()).expect("open store");
    let report = reconcile(&store, &scopes_before).expect("reconcile");
    apply(&mut store, &scopes_before, &report).expect("apply initial projection");

    let original_file_id = scopes_before[0].file_id.expect("directory exists");

    fs::rename(&original, dir.path().join("moved")).expect("rename");

    let config_after = parse(&config_text(
        &instance_id,
        &[scope(&scope_id, "movable", "moved")],
    ));
    let scopes_after = resolve(&config_after, dir.path()).expect("resolve after move");
    assert_eq!(
        scopes_after[0].file_id,
        Some(original_file_id),
        "a rename preserves the inode"
    );

    let report_after = reconcile(&store, &scopes_after).expect("reconcile after move");
    assert!(
        contains_variant(&report_after, |d| matches!(
            d,
            Drift::PathChangedSameIdentity { id, .. } if *id == scopes_after[0].id
        )),
        "expected PathChangedSameIdentity in {report_after:?}"
    );

    apply(&mut store, &scopes_after, &report_after).expect("apply the move");

    let snapshot = scopes_snapshot(&mut store);
    assert_eq!(snapshot.len(), 1);
    let (id, _name, declared_path, canonical_path, _git, dev, ino, _parent_id) = &snapshot[0];
    assert_eq!(id, &scope_id, "the UUID is identity and does not change");
    assert_eq!(declared_path, "moved");
    let expected_canonical_path = dir
        .path()
        .join("moved")
        .canonicalize()
        .expect("canonicalize moved");
    assert_eq!(
        canonical_path.as_deref(),
        Some(expected_canonical_path.to_str().unwrap())
    );
    assert_eq!(*dev, Some(original_file_id.dev() as i64));
    assert_eq!(*ino, Some(original_file_id.ino() as i64));

    // Reconciling once more against the now-current state must be clean.
    let clean_report = reconcile(&store, &scopes_after).expect("reconcile after apply");
    assert!(
        clean_report.is_clean(),
        "expected no drift immediately after applying the move: {clean_report:?}"
    );
}

// ---------------------------------------------------------------------
// 4. A replaced directory is not recognised as a move.
// ---------------------------------------------------------------------

#[test]
fn a_replaced_directory_is_not_recognised_as_the_same_scope() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agents_md(dir.path());
    let child = dir.path().join("child");
    fs::create_dir(&child).expect("create child");
    write_agents_md(&child);

    let instance_id = uid(60);
    let scope_id = uid(61);
    let config = parse(&config_text(
        &instance_id,
        &[scope(&scope_id, "child", "child")],
    ));
    let scopes = resolve(&config, dir.path()).expect("resolve");

    let mut store = Store::open(dir.path()).expect("open store");
    let report = reconcile(&store, &scopes).expect("reconcile");
    apply(&mut store, &scopes, &report).expect("apply initial projection");

    let original_file_id = scopes[0].file_id.expect("directory exists");
    let before_replace = scopes_snapshot(&mut store);

    // Delete and recreate at the same path: ADR 0009 is explicit this yields
    // a new inode.
    fs::remove_dir_all(&child).expect("remove child");
    fs::create_dir(&child).expect("recreate child");
    write_agents_md(&child);

    let scopes_after = resolve(&config, dir.path()).expect("resolve after replacement");
    assert_ne!(
        scopes_after[0].file_id,
        Some(original_file_id),
        "delete-and-recreate must not preserve the inode on this filesystem, \
         or this test cannot exercise the scenario it exists to check"
    );

    let report_after = reconcile(&store, &scopes_after).expect("reconcile after replacement");
    assert!(
        contains_variant(&report_after, |d| matches!(
            d,
            Drift::PathChangedDifferentIdentity { id, .. } if *id == scopes_after[0].id
        )),
        "expected PathChangedDifferentIdentity in {report_after:?}"
    );
    for item in &report_after.items {
        assert!(
            !item.is_applicable() || drift_id(item) != scopes_after[0].id,
            "PathChangedDifferentIdentity must never be applicable: {item:?}"
        );
    }

    // `apply` must refuse to touch it: nothing in the report for this scope
    // is applicable, so the row is untouched.
    apply(&mut store, &scopes_after, &report_after).expect("apply must not fail");
    let after_replace = scopes_snapshot(&mut store);
    assert_eq!(
        before_replace, after_replace,
        "apply must not have touched the row for a different-identity scope"
    );
}

// ---------------------------------------------------------------------
// 5. The case-only rename, per ADR 0009's correction.
// ---------------------------------------------------------------------

#[test]
fn a_case_only_rename_is_recognised_as_the_same_scope() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agents_md(dir.path());

    let mixed_case = dir.path().join("Child");
    fs::create_dir(&mixed_case).expect("create Child");
    write_agents_md(&mixed_case);

    let lower_case = dir.path().join("child");
    if !lower_case.exists() {
        eprintln!(
            "SKIPPED a_case_only_rename_is_recognised_as_the_same_scope: \
             {} is on a case-sensitive volume, so `Child` and `child` are \
             genuinely different directories here and this test cannot \
             exercise the case-insensitivity behaviour ADR 0009's \
             2026-09-08 correction describes. This is expected on a \
             case-sensitive volume and not a failure, but it means this \
             test verified nothing.",
            dir.path().display()
        );
        return;
    }

    let instance_id = uid(70);
    let scope_id = uid(71);
    // The declared path keeps its original spelling throughout: only the
    // filesystem entry's case changes, never the configuration text.
    let config = parse(&config_text(
        &instance_id,
        &[scope(&scope_id, "child", "Child")],
    ));
    let scopes = resolve(&config, dir.path()).expect("resolve");

    let mut store = Store::open(dir.path()).expect("open store");
    let report = reconcile(&store, &scopes).expect("reconcile");
    apply(&mut store, &scopes, &report).expect("apply initial projection");

    let original_file_id = scopes[0].file_id.expect("directory exists");

    fs::rename(&mixed_case, &lower_case).expect("case-only rename");

    let scopes_after = resolve(&config, dir.path()).expect("resolve after case-only rename");
    assert_eq!(
        scopes_after[0].file_id,
        Some(original_file_id),
        "a case-only rename preserves the inode"
    );

    let report_after = reconcile(&store, &scopes_after).expect("reconcile after case-only rename");
    assert!(
        contains_variant(&report_after, |d| matches!(
            d,
            Drift::PathChangedSameIdentity { id, .. } if *id == scopes_after[0].id
        )),
        "a case-only rename must be recognised via (dev, ino), not string \
         comparison, and reported as PathChangedSameIdentity: {report_after:?}"
    );
    assert!(
        !contains_variant(&report_after, |d| matches!(
            d,
            Drift::PathChangedDifferentIdentity { .. }
        )),
        "must not be mistaken for a different-identity change: {report_after:?}"
    );

    apply(&mut store, &scopes_after, &report_after).expect("apply the case-only rename");
    let snapshot = scopes_snapshot(&mut store);
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].0, scope_id, "the UUID is unchanged");
}

// ---------------------------------------------------------------------
// 6. `path: .` for the instance's own root scope.
// ---------------------------------------------------------------------

#[test]
fn root_scope_path_dot_resolves_and_is_not_rejected_as_an_escape() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agents_md(dir.path());

    let instance_id = uid(80);
    let scope_id = uid(81);
    let config = parse(&config_text(&instance_id, &[scope(&scope_id, "root", ".")]));

    let scopes = resolve(&config, dir.path()).expect("`path: .` must resolve, not escape");
    assert_eq!(scopes.len(), 1);
    assert!(scopes[0].canonical_path.is_some());
    assert_eq!(scopes[0].parent_id, None);
}

// ---------------------------------------------------------------------
// 7. `parent_id` is the nearest ancestor, three deep.
// ---------------------------------------------------------------------

#[test]
fn parent_id_is_the_nearest_ancestor_three_levels_deep() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agents_md(dir.path());

    let level1 = dir.path().join("a");
    let level2 = level1.join("b");
    let level3 = level2.join("c");
    fs::create_dir_all(&level3).expect("create nested directories");
    write_agents_md(&level1);
    write_agents_md(&level2);
    write_agents_md(&level3);

    let instance_id = uid(90);
    let (id_a, id_b, id_c) = (uid(91), uid(92), uid(93));
    let config = parse(&config_text(
        &instance_id,
        &[
            scope(&id_a, "a", "a"),
            scope(&id_b, "b", "a/b"),
            scope(&id_c, "c", "a/b/c"),
        ],
    ));

    let scopes = resolve(&config, dir.path()).expect("resolve");
    let find = |id: &str| {
        scopes
            .iter()
            .find(|s| s.id.to_string() == id)
            .unwrap_or_else(|| panic!("scope {id} missing from {scopes:?}"))
    };

    let a = find(&id_a);
    let b = find(&id_b);
    let c = find(&id_c);

    assert_eq!(
        a.parent_id, None,
        "the shallowest scope has no registered parent"
    );
    assert_eq!(b.parent_id, Some(a.id), "b's nearest ancestor is a");
    assert_eq!(
        c.parent_id,
        Some(b.id),
        "c's nearest ancestor is b, not the shallower a"
    );
}

// ---------------------------------------------------------------------
// 8. A symlink escape cannot make a non-descendant look like a child.
// ---------------------------------------------------------------------

#[test]
fn a_symlink_escape_is_rejected() {
    let instance_dir = tempfile::tempdir().expect("instance tempdir");
    let outside_dir = tempfile::tempdir().expect("outside tempdir");
    write_agents_md(outside_dir.path());

    let escape = instance_dir.path().join("escape");
    std::os::unix::fs::symlink(outside_dir.path(), &escape).expect("create symlink");

    let instance_id = uid(100);
    let scope_id = uid(101);
    let config = parse(&config_text(
        &instance_id,
        &[scope(&scope_id, "escape", "escape")],
    ));

    let err = resolve(&config, instance_dir.path())
        .expect_err("a symlink pointing outside the instance root must be rejected");
    match err {
        RegistryError::EscapesInstance { name, path, .. } => {
            assert_eq!(name, "escape");
            assert_eq!(path, Path::new("escape"));
        }
        other => panic!("expected RegistryError::EscapesInstance, got {other:?}"),
    }
}

// ---------------------------------------------------------------------
// 9. `apply` rolls back completely on a mid-way failure.
// ---------------------------------------------------------------------

#[test]
fn apply_rolls_back_completely_on_a_midway_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agents_md(dir.path());

    let instance_id = uid(110);
    let scope_id = uid(111);
    let config = parse(&config_text(&instance_id, &[scope(&scope_id, "root", ".")]));
    let scopes = resolve(&config, dir.path()).expect("resolve");

    let mut store = Store::open(dir.path()).expect("open store");
    assert_eq!(scopes_snapshot(&mut store).len(), 0, "starts empty");

    // A hand-built report that tries to insert the same new scope twice in
    // one `apply` call. The first INSERT succeeds; the second is a primary
    // key collision. Both must vanish together, because they run in one
    // transaction.
    let bogus_report = DriftReport {
        items: vec![
            Drift::DeclaredNotProjected {
                id: scopes[0].id,
                name: scopes[0].name.clone(),
            },
            Drift::DeclaredNotProjected {
                id: scopes[0].id,
                name: scopes[0].name.clone(),
            },
        ],
    };

    let result = apply(&mut store, &scopes, &bogus_report);
    assert!(
        result.is_err(),
        "the second insert of the same id must fail"
    );

    assert_eq!(
        scopes_snapshot(&mut store).len(),
        0,
        "a failure partway through apply must leave no partial state"
    );
}

// ---------------------------------------------------------------------
// 10. The real `fixtures/registration/` instance.
// ---------------------------------------------------------------------

#[test]
fn resolves_the_real_registration_fixture() {
    let fixture_root = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/registration"
    ));
    let config_path = fixture_root.join(".factory/config.yaml");

    let config = factory_config::load(&config_path).expect("load the real registration fixture");
    let scopes = resolve(&config, fixture_root).expect("resolve the real registration fixture");

    assert_eq!(scopes.len(), 2, "the fixture declares exactly two scopes");

    let root = scopes
        .iter()
        .find(|s| s.name == "fixture-co")
        .expect("root scope present");
    let child = scopes
        .iter()
        .find(|s| s.name == "example-project")
        .expect("child scope present");

    assert!(root.canonical_path.is_some());
    assert!(child.canonical_path.is_some());
    assert_eq!(root.parent_id, None);
    assert_eq!(
        child.parent_id,
        Some(root.id),
        "example-project's nearest registered ancestor is the company root"
    );
    assert_eq!(
        child.git.as_deref(),
        Some("https://example.invalid/example-project.git")
    );
}

// ---------------------------------------------------------------------
// Reconcile is read-only.
// ---------------------------------------------------------------------

#[test]
fn reconcile_does_not_change_the_scopes_table() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agents_md(dir.path());

    let instance_id = uid(120);
    let scope_id = uid(121);
    let config = parse(&config_text(&instance_id, &[scope(&scope_id, "root", ".")]));
    let scopes = resolve(&config, dir.path()).expect("resolve");

    let mut store = Store::open(dir.path()).expect("open store");
    let report = reconcile(&store, &scopes).expect("reconcile");
    apply(&mut store, &scopes, &report).expect("apply");

    let before = scopes_snapshot(&mut store);
    let _ = reconcile(&store, &scopes).expect("reconcile again");
    let after = scopes_snapshot(&mut store);

    assert_eq!(before, after, "reconcile must not modify the scopes table");
}
