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
        | Drift::UnreadableContext { id, .. }
        | Drift::NameChanged { id, .. }
        | Drift::GitChanged { id, .. }
        | Drift::DeclaredPathChanged { id, .. }
        | Drift::ParentChanged { id, .. } => *id,
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
// 2. Each of the ten `Drift` variants, individually.
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
    // The move also changed `declared_path`'s text ("original" -> "moved")
    // and, in principle, could have changed the computed `parent_id`. Both
    // are folded into `PathChangedSameIdentity`'s own `apply`, so
    // `DeclaredPathChanged` and `ParentChanged` must not also appear — one
    // underlying event, one report — per their doc comments' suppression
    // rule.
    assert_eq!(
        report_after.items.len(),
        1,
        "a move must not double-report as DeclaredPathChanged or ParentChanged \
         alongside PathChangedSameIdentity: {report_after:?}"
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
// 9. `NameChanged` and `GitChanged`: the two declarative columns with no
//    identity of their own.
// ---------------------------------------------------------------------

#[test]
fn name_changed_when_the_configuration_renames_a_scope() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agents_md(dir.path());

    let instance_id = uid(150);
    let scope_id = uid(151);
    let config_before = parse(&config_text(
        &instance_id,
        &[scope(&scope_id, "before-name", ".")],
    ));
    let scopes_before = resolve(&config_before, dir.path()).expect("resolve before rename");

    let mut store = Store::open(dir.path()).expect("open store");
    let report = reconcile(&store, &scopes_before).expect("reconcile");
    apply(&mut store, &scopes_before, &report).expect("apply initial projection");

    let config_after = parse(&config_text(
        &instance_id,
        &[scope(&scope_id, "after-name", ".")],
    ));
    let scopes_after = resolve(&config_after, dir.path()).expect("resolve after rename");

    let report_after = reconcile(&store, &scopes_after).expect("reconcile after rename");
    assert!(
        contains_variant(&report_after, |d| matches!(
            d,
            Drift::NameChanged { id, from, to }
                if *id == scopes_after[0].id && from == "before-name" && to == "after-name"
        )),
        "expected NameChanged in {report_after:?}"
    );

    apply(&mut store, &scopes_after, &report_after).expect("apply the rename");
    let snapshot = scopes_snapshot(&mut store);
    assert_eq!(
        snapshot[0].1, "after-name",
        "the stored name now matches the configuration"
    );

    let clean = reconcile(&store, &scopes_after).expect("reconcile after apply");
    assert!(
        clean.is_clean(),
        "expected no drift after applying the rename: {clean:?}"
    );
}

#[test]
fn git_changed_when_the_configuration_gains_or_loses_a_git_reference() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agents_md(dir.path());

    let instance_id = uid(160);
    let scope_id = uid(161);
    let git_url = "https://example.invalid/svc.git";

    let config_no_git = parse(&config_text(&instance_id, &[scope(&scope_id, "svc", ".")]));
    let scopes_no_git = resolve(&config_no_git, dir.path()).expect("resolve without git");

    let mut store = Store::open(dir.path()).expect("open store");
    let report = reconcile(&store, &scopes_no_git).expect("reconcile");
    apply(&mut store, &scopes_no_git, &report).expect("apply initial projection");

    // Gains a `git:` reference.
    let config_with_git = parse(&config_text(
        &instance_id,
        &[ScopeSpec {
            id: scope_id.clone(),
            name: "svc",
            path: ".",
            git: Some(git_url),
        }],
    ));
    let scopes_with_git = resolve(&config_with_git, dir.path()).expect("resolve after gaining git");

    let report_gain = reconcile(&store, &scopes_with_git).expect("reconcile after gaining git");
    assert!(
        contains_variant(&report_gain, |d| matches!(
            d,
            Drift::GitChanged { id, from: None, to: Some(to), .. }
                if *id == scopes_with_git[0].id && to == git_url
        )),
        "expected GitChanged (gained) in {report_gain:?}"
    );
    apply(&mut store, &scopes_with_git, &report_gain).expect("apply the gained git reference");
    let clean_after_gain = reconcile(&store, &scopes_with_git).expect("reconcile after apply");
    assert!(
        clean_after_gain.is_clean(),
        "expected no drift after applying the gained git reference: {clean_after_gain:?}"
    );

    // Loses the `git:` reference again.
    let report_loss = reconcile(&store, &scopes_no_git).expect("reconcile after losing git");
    assert!(
        contains_variant(&report_loss, |d| matches!(
            d,
            Drift::GitChanged { id, from: Some(from), to: None, .. }
                if *id == scopes_no_git[0].id && from == git_url
        )),
        "expected GitChanged (lost) in {report_loss:?}"
    );
    apply(&mut store, &scopes_no_git, &report_loss).expect("apply the lost git reference");
    let clean_after_loss = reconcile(&store, &scopes_no_git).expect("reconcile after apply");
    assert!(
        clean_after_loss.is_clean(),
        "expected no drift after applying the lost git reference: {clean_after_loss:?}"
    );
}

// ---------------------------------------------------------------------
// 10. `DeclaredPathChanged` and `ParentChanged`: drift `PathChangedSameIdentity`
//     cannot see because this scope's own canonicalization never moved.
// ---------------------------------------------------------------------

#[test]
fn declared_path_changed_when_the_declared_text_changes_without_moving() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agents_md(dir.path());

    // Two aliases inside the instance root that resolve to the same real
    // directory, so switching the declared path between them changes no
    // canonicalization and no `(dev, ino)` — the scenario `PathChanged*`
    // cannot see, per `Drift::DeclaredPathChanged`'s doc comment.
    let real = dir.path().join("real");
    fs::create_dir(&real).expect("create real");
    write_agents_md(&real);
    std::os::unix::fs::symlink(&real, dir.path().join("link-a")).expect("symlink link-a");
    std::os::unix::fs::symlink(&real, dir.path().join("link-b")).expect("symlink link-b");

    let instance_id = uid(170);
    let scope_id = uid(171);
    let config_before = parse(&config_text(
        &instance_id,
        &[scope(&scope_id, "aliased", "link-a")],
    ));
    let scopes_before = resolve(&config_before, dir.path()).expect("resolve before alias switch");

    let mut store = Store::open(dir.path()).expect("open store");
    let report = reconcile(&store, &scopes_before).expect("reconcile");
    apply(&mut store, &scopes_before, &report).expect("apply initial projection");

    let config_after = parse(&config_text(
        &instance_id,
        &[scope(&scope_id, "aliased", "link-b")],
    ));
    let scopes_after = resolve(&config_after, dir.path()).expect("resolve after alias switch");

    assert_eq!(
        scopes_before[0]
            .canonical_path
            .as_ref()
            .map(|c| c.as_path()),
        scopes_after[0].canonical_path.as_ref().map(|c| c.as_path()),
        "both aliases must resolve to the same real directory, or this test \
         cannot exercise the scenario it exists to check"
    );
    assert_eq!(
        scopes_before[0].file_id, scopes_after[0].file_id,
        "same real directory means same (dev, ino)"
    );

    let report_after = reconcile(&store, &scopes_after).expect("reconcile after alias switch");
    assert!(
        contains_variant(&report_after, |d| matches!(
            d,
            Drift::DeclaredPathChanged { id, from, to, .. }
                if *id == scopes_after[0].id && from == Path::new("link-a") && to == Path::new("link-b")
        )),
        "expected DeclaredPathChanged in {report_after:?}"
    );
    assert!(
        !contains_variant(&report_after, |d| matches!(
            d,
            Drift::PathChangedSameIdentity { .. } | Drift::PathChangedDifferentIdentity { .. }
        )),
        "canonicalization and identity are unchanged, so no PathChanged* drift \
         should also fire: {report_after:?}"
    );

    apply(&mut store, &scopes_after, &report_after).expect("apply the alias switch");
    let snapshot = scopes_snapshot(&mut store);
    assert_eq!(
        snapshot[0].2, "link-b",
        "declared_path now matches the new alias"
    );

    let clean = reconcile(&store, &scopes_after).expect("reconcile after apply");
    assert!(
        clean.is_clean(),
        "expected no drift after applying the alias switch: {clean:?}"
    );
}

#[test]
fn parent_changed_when_a_new_ancestor_scope_is_declared() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agents_md(dir.path());
    let child_dir = dir.path().join("child");
    fs::create_dir(&child_dir).expect("create child");
    write_agents_md(&child_dir);

    let instance_id = uid(180);
    let child_id = uid(181);

    // Only `child` is declared at first: it has no registered ancestor.
    let config_before = parse(&config_text(
        &instance_id,
        &[scope(&child_id, "child", "child")],
    ));
    let scopes_before =
        resolve(&config_before, dir.path()).expect("resolve before ancestor declared");
    assert_eq!(scopes_before[0].parent_id, None);

    let mut store = Store::open(dir.path()).expect("open store");
    let report = reconcile(&store, &scopes_before).expect("reconcile");
    apply(&mut store, &scopes_before, &report).expect("apply initial projection");

    // The instance root is now also declared as a scope. `child`'s own
    // directory has not moved — only another scope's declaration changed —
    // yet it now has a nearest registered ancestor.
    let root_id = uid(182);
    let config_after = parse(&config_text(
        &instance_id,
        &[
            scope(&root_id, "root", "."),
            scope(&child_id, "child", "child"),
        ],
    ));
    let scopes_after = resolve(&config_after, dir.path()).expect("resolve after ancestor declared");
    let root_after = scopes_after
        .iter()
        .find(|s| s.id.to_string() == root_id)
        .expect("root resolved");
    let child_after = scopes_after
        .iter()
        .find(|s| s.id.to_string() == child_id)
        .expect("child resolved");
    assert_eq!(child_after.parent_id, Some(root_after.id));

    let report_after = reconcile(&store, &scopes_after).expect("reconcile after ancestor declared");
    assert!(
        contains_variant(&report_after, |d| matches!(
            d,
            Drift::ParentChanged { id, from: None, to: Some(to), .. }
                if *id == child_after.id && *to == root_after.id
        )),
        "expected ParentChanged for the child in {report_after:?}"
    );
    assert!(
        !contains_variant(&report_after, |d| matches!(
            d,
            Drift::PathChangedSameIdentity { id, .. } | Drift::PathChangedDifferentIdentity { id, .. }
                if *id == child_after.id
        )),
        "the child's own path did not move: {report_after:?}"
    );

    apply(&mut store, &scopes_after, &report_after).expect("apply");
    let clean = reconcile(&store, &scopes_after).expect("reconcile after apply");
    assert!(
        clean.is_clean(),
        "expected no drift after applying: {clean:?}"
    );
}

// ---------------------------------------------------------------------
// 11. `apply` rolls back completely on a mid-way failure.
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
// 12. The real `fixtures/registration/` instance.
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

// ---------------------------------------------------------------------
// 13. Guard: every column of `scopes` must have drift coverage, or be
//     explicitly and defensibly exempt.
// ---------------------------------------------------------------------

/// Reads the *live* `scopes` schema and, for every column not explicitly
/// exempted, corrupts exactly that column on an otherwise-clean row and
/// requires `reconcile` to report something naming the corrupted scope.
///
/// This is deliberately schema-driven rather than a hand-copied column list.
/// A hand-copied list is exactly what let `name`, `git`, `declared_path`,
/// and `parent_id` go uncovered in the first place: nothing forced "columns
/// `reconcile` compares" to be updated in step with "columns migration 2
/// projects" (`crates/factory-store/src/schema.rs`), which is the gap this
/// change closes. Reading `PRAGMA table_info(scopes)` instead means a
/// *future* column reaches the `panic!` in the `match` below the moment a
/// migration adds it — before anyone has to remember to update this test by
/// hand — and the corrupt/assert/restore cycle around it proves real
/// detection, not just that a name appears on a list.
#[test]
fn every_scopes_column_has_drift_coverage_or_is_explicitly_exempt() {
    // Columns that are not, and structurally cannot be, drift. `id` is the
    // join key `reconcile` uses to find a stored row for a resolved scope in
    // the first place (`stored_by_id.remove(&scope.id)`) — it is how "this
    // row" and "this scope" are matched, not a value compared once matched.
    // `created_at` is audit metadata: nothing in the configuration or the
    // filesystem produces a value to compare it against, which is the same
    // reason `scopes_snapshot` above excludes it from equality checks (a
    // rebuild cannot reproduce a timestamp).
    const EXEMPT: &[&str] = &["id", "created_at"];

    let dir = tempfile::tempdir().expect("tempdir");
    write_agents_md(dir.path());
    let child_dir = dir.path().join("child");
    fs::create_dir(&child_dir).expect("create child");
    write_agents_md(&child_dir);

    let instance_id = uid(190);
    let root_id = uid(191);
    let child_id = uid(192);
    let config = parse(&config_text(
        &instance_id,
        &[
            scope(&root_id, "root", "."),
            scope(&child_id, "child", "child"),
        ],
    ));
    let scopes = resolve(&config, dir.path()).expect("resolve");
    let child = scopes
        .iter()
        .find(|s| s.id.to_string() == child_id)
        .expect("child scope resolved")
        .clone();
    let child_file_id = child.file_id.expect("child directory exists");
    assert_eq!(
        child.parent_id,
        Some(
            scopes
                .iter()
                .find(|s| s.id.to_string() == root_id)
                .expect("root resolved")
                .id
        ),
        "fixture assumption: child is nested under root, so parent_id starts non-null \
         and mutating it to NULL below is a real change"
    );

    let mut store = Store::open(dir.path()).expect("open store");
    let report = reconcile(&store, &scopes).expect("reconcile");
    apply(&mut store, &scopes, &report).expect("apply initial projection");
    let clean = reconcile(&store, &scopes).expect("reconcile after apply");
    assert!(clean.is_clean(), "fixture must start clean: {clean:?}");

    let original = scopes_snapshot(&mut store)
        .into_iter()
        .find(|row| row.0 == child_id)
        .expect("child row present");
    #[allow(clippy::type_complexity)]
    let (
        _,
        orig_name,
        orig_declared_path,
        orig_canonical_path,
        orig_git,
        orig_dev,
        orig_ino,
        orig_parent_id,
    ): (
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<i64>,
        Option<String>,
    ) = original.clone();

    // The live column list — read from the schema, not copied by hand.
    let columns: Vec<String> = {
        let mut stmt = store
            .connection()
            .prepare("PRAGMA table_info(scopes)")
            .expect("prepare PRAGMA table_info");
        stmt.query_map([], |row| row.get::<_, String>(1))
            .expect("run PRAGMA table_info")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect column names")
    };
    assert!(
        columns.len() > EXEMPT.len(),
        "PRAGMA table_info returned suspiciously few columns: {columns:?}"
    );

    for column in &columns {
        if EXEMPT.contains(&column.as_str()) {
            continue;
        }

        // Corrupt exactly this column on the child's row.
        {
            let tx = store.transaction().expect("begin corruption");
            let changed = match column.as_str() {
                "name" => tx.execute(
                    "UPDATE scopes SET name = 'mutated-name' WHERE id = ?1",
                    [child_id.as_str()],
                ),
                "declared_path" => tx.execute(
                    "UPDATE scopes SET declared_path = 'mutated/declared/path' WHERE id = ?1",
                    [child_id.as_str()],
                ),
                "canonical_path" => tx.execute(
                    "UPDATE scopes SET canonical_path = canonical_path || '-mutated' WHERE id = ?1",
                    [child_id.as_str()],
                ),
                "git" => tx.execute(
                    "UPDATE scopes SET git = 'https://example.invalid/mutated.git' WHERE id = ?1",
                    [child_id.as_str()],
                ),
                "dev" => tx.execute(
                    "UPDATE scopes SET dev = ?2 WHERE id = ?1",
                    (child_id.as_str(), (child_file_id.dev() as i64) + 1),
                ),
                "ino" => tx.execute(
                    "UPDATE scopes SET ino = ?2 WHERE id = ?1",
                    (child_id.as_str(), (child_file_id.ino() as i64) + 1),
                ),
                "parent_id" => tx.execute(
                    "UPDATE scopes SET parent_id = NULL WHERE id = ?1",
                    [child_id.as_str()],
                ),
                other => panic!(
                    "`scopes` has a column `{other}` this guard test does not know how \
                     to corrupt. A migration added a projected column without teaching \
                     this test about it — add a case above that mutates it and confirm \
                     `reconcile` reports the mutation (adding real `Drift` coverage in \
                     `crates/factory-registry/src/lib.rs`), or add `{other}` to `EXEMPT` \
                     with a comment justifying why no value can ever disagree there. \
                     Update ADR 0016's drift table either way."
                ),
            }
            .unwrap_or_else(|e| panic!("corrupt column `{column}`: {e}"));
            assert_eq!(
                changed, 1,
                "corrupting `{column}` should touch exactly the child row"
            );
            tx.commit().expect("commit corruption");
        }

        // Prove the mutation actually mutated something — otherwise a
        // NULL-propagating or no-op UPDATE (e.g. concatenating onto a NULL
        // column) could make this test pass for the wrong reason, which is
        // exactly the decorative-test failure mode this guard exists to
        // avoid.
        let mutated = scopes_snapshot(&mut store)
            .into_iter()
            .find(|row| row.0 == child_id)
            .expect("child row present after corruption");
        assert_ne!(
            mutated, original,
            "corrupting `{column}` did not change the child's row at all — \
             the mutation above is a no-op and proves nothing"
        );

        let report = reconcile(&store, &scopes).expect("reconcile after corruption");
        assert!(
            contains_variant(&report, |d| drift_id(d) == child.id),
            "corrupting `scopes.{column}` alone produced no drift naming the \
             child scope — add `Drift` coverage for this column: {report:?}"
        );

        // Restore this column's original value directly, not via `apply`:
        // corrupting `dev` or `ino` alone produces `PathChangedDifferentIdentity`,
        // which is deliberately never applied, so `apply` could not restore
        // it and the next column's check would start from a dirty row.
        {
            let tx = store.transaction().expect("begin restore");
            match column.as_str() {
                "name" => tx.execute(
                    "UPDATE scopes SET name = ?2 WHERE id = ?1",
                    (child_id.as_str(), &orig_name),
                ),
                "declared_path" => tx.execute(
                    "UPDATE scopes SET declared_path = ?2 WHERE id = ?1",
                    (child_id.as_str(), &orig_declared_path),
                ),
                "canonical_path" => tx.execute(
                    "UPDATE scopes SET canonical_path = ?2 WHERE id = ?1",
                    (child_id.as_str(), &orig_canonical_path),
                ),
                "git" => tx.execute(
                    "UPDATE scopes SET git = ?2 WHERE id = ?1",
                    (child_id.as_str(), &orig_git),
                ),
                "dev" => tx.execute(
                    "UPDATE scopes SET dev = ?2 WHERE id = ?1",
                    (child_id.as_str(), orig_dev),
                ),
                "ino" => tx.execute(
                    "UPDATE scopes SET ino = ?2 WHERE id = ?1",
                    (child_id.as_str(), orig_ino),
                ),
                "parent_id" => tx.execute(
                    "UPDATE scopes SET parent_id = ?2 WHERE id = ?1",
                    (child_id.as_str(), &orig_parent_id),
                ),
                other => unreachable!("already handled or panicked above: {other}"),
            }
            .unwrap_or_else(|e| panic!("restore column `{column}`: {e}"));
            tx.commit().expect("commit restore");
        }

        let restored = reconcile(&store, &scopes).expect("reconcile after restore");
        assert!(
            restored.is_clean(),
            "restoring `{column}` did not bring the row back to clean, so the \
             next column's check would start dirty: {restored:?}"
        );
    }
}

// ---------------------------------------------------------------------
// 14. `apply` must not depend on declaration order for a new nested scope.
//
// `scopes.parent_id REFERENCES scopes (id)`, and `PRAGMA foreign_keys = ON`
// is set on every connection (`factory_store::pragma`) with immediate, not
// deferred, enforcement. If `apply` ever writes a child's row — carrying its
// parent's UUID in `parent_id` — before the row that parent names exists,
// that single `INSERT` violates the constraint and SQLite rolls back the
// *whole* transaction, undoing every other statement that had already
// succeeded. This is the practical trigger the doc comment on `apply`
// describes: a human adds a nested scope to `.factory/config.yaml` and
// writes the child entry above its new ancestor.
// ---------------------------------------------------------------------

#[test]
fn a_child_declared_above_its_new_parent_still_projects_both() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agents_md(dir.path());
    let parent_dir = dir.path().join("parent-dir");
    let child_dir = parent_dir.join("child-dir");
    fs::create_dir_all(&child_dir).expect("create nested directories");
    write_agents_md(&parent_dir);
    write_agents_md(&child_dir);

    let instance_id = uid(210);
    let child_id = uid(211);
    let parent_id = uid(212);

    // The child is listed *first* — above its new ancestor — exactly as a
    // human editing the configuration by hand would do. Both are brand new,
    // so `reconcile` reports two `DeclaredNotProjected` drifts, and it walks
    // `scopes` in declaration order (see `reconcile`'s loop over `scopes`),
    // so the child's drift lands first in `report.items` unless `apply`
    // itself reorders what it executes.
    let config = parse(&config_text(
        &instance_id,
        &[
            scope(&child_id, "child", "parent-dir/child-dir"),
            scope(&parent_id, "parent", "parent-dir"),
        ],
    ));
    let scopes = resolve(&config, dir.path()).expect("resolve");
    let child = scopes
        .iter()
        .find(|s| s.id.to_string() == child_id)
        .expect("child resolved");
    let parent = scopes
        .iter()
        .find(|s| s.id.to_string() == parent_id)
        .expect("parent resolved");
    assert_eq!(
        child.parent_id,
        Some(parent.id),
        "child's nearest registered ancestor is parent"
    );

    let mut store = Store::open(dir.path()).expect("open store");
    let report = reconcile(&store, &scopes).expect("reconcile");
    assert_eq!(report.items.len(), 2, "both scopes are new: {report:?}");
    assert!(
        matches!(
            &report.items[0],
            Drift::DeclaredNotProjected { id, .. } if *id == child.id
        ),
        "the child's drift must be reported before the parent's, or this test \
         does not exercise the hazard `apply`'s doc comment describes: {report:?}"
    );

    // Before the fix, this fails: SQLite reports `FOREIGN KEY constraint
    // failed` on the child's INSERT (its parent_id names a row that does not
    // exist yet in insertion order) and the whole transaction — including
    // the parent's own INSERT, which would otherwise have succeeded — rolls
    // back.
    apply(&mut store, &scopes, &report).expect(
        "apply must project a new nested scope regardless of whether the \
         configuration declares the child before the parent",
    );

    let snapshot = scopes_snapshot(&mut store);
    assert_eq!(snapshot.len(), 2, "both scopes are projected");

    let clean = reconcile(&store, &scopes).expect("reconcile after apply");
    assert!(
        clean.is_clean(),
        "expected no drift after applying: {clean:?}"
    );
}

/// The same hazard, reached through `ParentChanged` rather than
/// `DeclaredNotProjected`: `child` is already projected with no parent, a new
/// ancestor scope is declared over it, and the configuration lists `child`
/// *before* the new ancestor. `reconcile` then reports `ParentChanged` for
/// `child` ahead of `DeclaredNotProjected` for the ancestor, and
/// `ParentChanged`'s `UPDATE` writes the same not-yet-existing `parent_id` —
/// so the fix must order by the *target* scope's nesting depth, not by
/// which `Drift` variant is involved.
#[test]
fn a_parent_changed_update_above_its_new_parents_insert_still_projects_both() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_agents_md(dir.path());
    let parent_dir = dir.path().join("parent-dir");
    let child_dir = parent_dir.join("child-dir");
    fs::create_dir_all(&child_dir).expect("create nested directories");
    write_agents_md(&parent_dir);
    write_agents_md(&child_dir);

    let instance_id = uid(220);
    let child_id = uid(221);
    let parent_id = uid(222);

    // Only `child` is declared at first: it has no registered ancestor.
    let config_before = parse(&config_text(
        &instance_id,
        &[scope(&child_id, "child", "parent-dir/child-dir")],
    ));
    let scopes_before =
        resolve(&config_before, dir.path()).expect("resolve before ancestor declared");
    assert_eq!(scopes_before[0].parent_id, None);

    let mut store = Store::open(dir.path()).expect("open store");
    let report = reconcile(&store, &scopes_before).expect("reconcile");
    apply(&mut store, &scopes_before, &report).expect("apply initial projection");

    // Now `parent-dir` is declared too, and listed *after* `child` — the
    // same practical trigger, but arriving as `ParentChanged` on an already-
    // projected row instead of `DeclaredNotProjected` on a new one.
    let config_after = parse(&config_text(
        &instance_id,
        &[
            scope(&child_id, "child", "parent-dir/child-dir"),
            scope(&parent_id, "parent", "parent-dir"),
        ],
    ));
    let scopes_after = resolve(&config_after, dir.path()).expect("resolve after ancestor declared");
    let child = scopes_after
        .iter()
        .find(|s| s.id.to_string() == child_id)
        .expect("child resolved");
    let parent = scopes_after
        .iter()
        .find(|s| s.id.to_string() == parent_id)
        .expect("parent resolved");
    assert_eq!(child.parent_id, Some(parent.id));

    let report_after = reconcile(&store, &scopes_after).expect("reconcile after ancestor declared");
    assert_eq!(report_after.items.len(), 2, "{report_after:?}");
    assert!(
        matches!(
            &report_after.items[0],
            Drift::ParentChanged { id, .. } if *id == child.id
        ),
        "the child's ParentChanged must be reported before the parent's \
         DeclaredNotProjected, or this test does not exercise the hazard: \
         {report_after:?}"
    );

    // Before the fix: the child's UPDATE runs first, naming a parent row
    // that does not exist yet, and SQLite reports `FOREIGN KEY constraint
    // failed` — rolling back the parent's INSERT too.
    apply(&mut store, &scopes_after, &report_after).expect(
        "apply must project a new ancestor regardless of whether the \
         configuration declares the already-registered child before it",
    );

    let clean = reconcile(&store, &scopes_after).expect("reconcile after apply");
    assert!(
        clean.is_clean(),
        "expected no drift after applying: {clean:?}"
    );
}
