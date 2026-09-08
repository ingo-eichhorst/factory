//! Aliasing must not defeat exclusivity (ADR 0009 and its 2026-09-08
//! correction).
//!
//! [`case_variant_lease_conflict_spans_a_rename`] is the centerpiece: same
//! inode, stored string goes stale across a case-only rename. Only the
//! `(st_dev, st_ino)` scan over a re-canonicalized *stored* path catches
//! this; the database's `BINARY` collation cannot (proved directly in
//! `crates/factory-store/tests/lease.rs::the_database_index_alone_does_not_see_case_variant_paths`).
//!
//! [`delete_and_recreate_at_the_same_path_is_still_rejected`] is the scenario
//! ADR 0009 uses to argue neither rule subsumes the other — and it was
//! expected, going in, to isolate the database index's necessity the same
//! way the case-variant test isolates the Rust scan's. Measuring it showed
//! otherwise: for *this* crate specifically (which persists no inode — see
//! `lib.rs`'s "rejected alternative" section), the Rust scan re-derives
//! `(st_dev, st_ino)` from the stored path *text* fresh on every check, so
//! an identical path string always resolves both sides to the same live
//! directory and the scan catches this case too, redundantly with the
//! index. The database index's independent value — that it survives the
//! Rust scan being buggy, bypassed, or deleted — is demonstrated instead by
//! mutation in the task report, and by
//! `crates/factory-store/tests/lease.rs::dropping_the_lease_index_allows_an_exact_duplicate`,
//! which removes the index and inserts through raw SQL with no Rust scan in
//! the path at all.
//!
//! [`naive_single_instant_check_proves_nothing`] is deliberately left in
//! this file, not deleted after use, as the negative control: it performs
//! the same case-variant swap as the real test but resolves both spellings
//! at one instant, and it passes regardless of whether the aliasing scan is
//! implemented at all (see the task report for the mutation run that
//! confirms this). Its purpose is to make legible, to the next reader, why
//! the real test below insists on spanning time — a claim otherwise easy to
//! mistake for caution rather than necessity.
//!
//! [`a_stale_lease_does_not_block_an_unrelated_start`] and
//! [`scan_continues_past_a_stale_row_to_find_a_later_conflict`] cover a
//! third scenario, found by mutation rather than anticipated going in: the
//! `Err(PathError::NotFound { .. }) => continue` arm inside
//! `find_aliasing_conflict`'s loop had no test able to notice if `continue`
//! were replaced with `return Ok(None)`. That distinction only matters when
//! a stale row is *not* the last lease-holder in the scan — with only one
//! holder in the database, both spellings produce the same outcome. The
//! first test pins the policy (a stale lease must not block anything); the
//! second is the one that actually depends on — and therefore pins — the
//! scan continuing past it to reach a later, genuine conflict.

mod common;

use factory_paths::{CanonicalPath, FileId, same_file};
use factory_session::{SessionError, begin_start};
use factory_store::Store;

/// Not a real test of this crate's exclusivity — a demonstration of why the
/// real one (below) must span time. Rust's `std::fs::canonicalize`
/// normalises case (ADR 0009's 2026-09-08 correction, which also measured
/// equal canonical *strings* for two spellings resolved at one instant), so
/// two spellings of one directory resolved *at the same instant* already
/// produce equal `(st_dev, st_ino)` pairs, with no code of this crate's
/// involved at all. A test built this way cannot fail against a broken
/// `begin_start`; confirmed by mutation (see the task report): stubbing
/// `find_aliasing_conflict` to always report "no conflict" left this test
/// passing, because `same_file` here never touches a stored database row,
/// only two fresh `stat` calls made moments apart. That is the vacuity the
/// backlog warns about: "a lease test that cannot fail is worse than no
/// lease test, because it reports confidence it has not earned."
#[test]
fn naive_single_instant_check_proves_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mixed_case_dir = dir.path().join("Workspace");
    std::fs::create_dir(&mixed_case_dir).expect("create");
    let lower_case_dir = dir.path().join("workspace");

    if !lower_case_dir.exists() {
        eprintln!(
            "SKIPPED naive_single_instant_check_proves_nothing: {} is on a \
             case-sensitive volume, so this probe cannot even set up the \
             scenario it exists to warn about.",
            dir.path().display()
        );
        return;
    }

    // Passes trivially — proves nothing about `begin_start`, which this test
    // never calls.
    assert!(same_file(&mixed_case_dir, &lower_case_dir).unwrap());
}

/// The real test: resolve, rename changing only case, resolve again. This is
/// the shape the backlog demands — "a test that resolves two spellings of
/// two case-variant paths at one instant... passes trivially and proves
/// nothing" — applied to lease acquisition specifically, not just to path
/// identity (which `factory-paths` already covers for `is_descendant`).
///
/// Session A takes the lease while the directory is spelled `Workspace`.
/// The directory is then renamed, changing only its case — an
/// inode-preserving operation on the production APFS default (ADR 0009
/// §3a) — to `workspace`. Session B then attempts to start in `workspace`.
/// A stored-string comparison would see `"…/Workspace"` (what A's row still
/// holds) against `"…/workspace"` (B's freshly canonicalized candidate) and
/// wrongly conclude "different directory." The re-canonicalizing scan in
/// `find_aliasing_conflict` re-resolves A's *stored* path after the rename —
/// which still succeeds, because case-insensitive lookup follows the
/// directory to its new name — recovers the current `(st_dev, st_ino)`, and
/// correctly reports a conflict.
#[test]
fn case_variant_lease_conflict_spans_a_rename() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mixed_case_dir = dir.path().join("Workspace");
    std::fs::create_dir(&mixed_case_dir).expect("create");
    let lower_case_dir = dir.path().join("workspace");

    if !lower_case_dir.exists() {
        eprintln!(
            "SKIPPED case_variant_lease_conflict_spans_a_rename: {} is on a \
             case-sensitive volume, so `Workspace` and `workspace` are \
             genuinely different directories here and this test cannot \
             exercise the aliasing defect ADR 0009's correction describes. \
             This is expected on a case-sensitive volume and not a failure, \
             but it means this test verified nothing.",
            dir.path().display()
        );
        return;
    }

    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());

    // Session A leases the directory under its original spelling.
    let original = common::resolve(&mixed_case_dir);
    begin_start(
        &mut store,
        common::uid(1),
        scope_id,
        "agent-a",
        10,
        &original,
    )
    .expect("session A takes the lease as `Workspace`");

    // A case-only rename. Same inode; the on-disk spelling changes under
    // session A's still-open lease.
    std::fs::rename(&mixed_case_dir, &lower_case_dir).expect("rename changing only case");

    // The freshly resolved candidate cannot share a string prefix with the
    // stale stored value: they were pinned to different points in time, not
    // just different spellings of "now". If this assertion fails, the test
    // below no longer discriminates a string-based scan from the required
    // FileId-based one — see the `is_descendant` test in `factory-paths`
    // this mirrors.
    let candidate = common::resolve(&lower_case_dir);
    assert_ne!(
        original.as_path(),
        candidate.as_path(),
        "expected the pre-rename stored path to disagree in case with the \
         post-rename candidate path; if they match, this test no longer \
         proves anything about re-canonicalization"
    );
    // And yet it is still, undeniably, the same directory.
    assert_eq!(
        FileId::of(original.as_path()).unwrap(),
        FileId::of(candidate.as_path()).unwrap()
    );

    // Session B attempts to start in the renamed directory. Must be
    // rejected: it is the same directory session A already leases.
    let err = begin_start(
        &mut store,
        common::uid(2),
        scope_id,
        "agent-b",
        10,
        &candidate,
    )
    .expect_err("a case-only rename must not let a second session lease the same directory");
    assert!(
        matches!(&err, SessionError::WorkspaceLeased { holder_agent, .. } if holder_agent == "agent-a"),
        "expected WorkspaceLeased naming session A's agent, got {err:?}"
    );
}

/// Measured, not assumed (see the file header): this scenario does **not**
/// isolate the database index from the Rust scan for session leases,
/// although it does for `factory-registry`'s scope-drift detection, which
/// compares a *persisted* inode. `begin_start`'s scan re-derives
/// `(st_dev, st_ino)` from the stored path text fresh on every check, so an
/// identical path string resolves both the stored side and the candidate
/// side to the same live (recreated) directory, and the Rust scan reports a
/// conflict on its own — the database index never gets a chance to be the
/// deciding factor here. The two layers agree, which is a legitimate outcome
/// for defense-in-depth to produce; the index's independent value is proven
/// elsewhere (see the file header and the task report's mutation run).
#[test]
fn delete_and_recreate_at_the_same_path_is_still_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace_dir = dir.path().join("workspace");
    std::fs::create_dir(&workspace_dir).expect("create");

    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());

    let original = common::resolve(&workspace_dir);
    let original_file_id = FileId::of(original.as_path()).unwrap();
    begin_start(
        &mut store,
        common::uid(1),
        scope_id,
        "agent-a",
        10,
        &original,
    )
    .expect("session A takes the lease");

    // Delete and recreate at the identical path. Same string; SQLite has no
    // way to know the underlying directory changed. A brand new inode is
    // allocated.
    std::fs::remove_dir(&workspace_dir).expect("remove");
    std::fs::create_dir(&workspace_dir).expect("recreate");
    let recreated = common::resolve(&workspace_dir);
    let recreated_file_id = FileId::of(recreated.as_path()).unwrap();
    assert_ne!(
        original_file_id, recreated_file_id,
        "delete-and-recreate must yield a new inode, or this test exercises nothing"
    );
    assert_eq!(
        original.as_path(),
        recreated.as_path(),
        "the path text must be identical, or this is not the scenario under test"
    );

    // Rejected either way — but, as measured, via the Rust scan
    // (`WorkspaceLeased`), because it re-resolves session A's *stored* path
    // text and lands on the very directory `recreated` also names.
    let err = begin_start(
        &mut store,
        common::uid(2),
        scope_id,
        "agent-b",
        10,
        &recreated,
    )
    .expect_err(
        "a delete-and-recreate at the same path must still be rejected — the \
         workspace is, by path, still what session A holds",
    );
    assert!(
        matches!(
            &err,
            SessionError::WorkspaceLeased { holder_agent, .. } if holder_agent == "agent-a"
        ),
        "expected the Rust scan to catch this via WorkspaceLeased (see this \
         test's doc comment for why), got {err:?}"
    );
}

/// Symlink aliasing at a single instant — no rename needed, since a symlink
/// does not encode case normalisation. Included for completeness alongside
/// the two time-spanning scenarios above: `find_aliasing_conflict` compares
/// `(st_dev, st_ino)`, which sees through a symlink exactly as
/// `factory_paths::same_file` does.
#[test]
fn symlink_alias_is_also_caught() {
    let dir = tempfile::tempdir().expect("tempdir");
    let real = dir.path().join("real");
    std::fs::create_dir(&real).expect("create real");
    let link = dir.path().join("link");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, &link).expect("symlink");

    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());

    let via_real: CanonicalPath = common::resolve(&real);
    let via_link = common::resolve(&link);
    // Both resolve to the same canonical path here, since resolving a
    // symlink already lands on its target — this is the "obvious" case, kept
    // as a smoke test rather than the load-bearing one.
    assert_eq!(via_real, via_link);

    begin_start(
        &mut store,
        common::uid(1),
        scope_id,
        "agent-a",
        10,
        &via_real,
    )
    .expect("start via real");
    let err = begin_start(
        &mut store,
        common::uid(2),
        scope_id,
        "agent-b",
        10,
        &via_link,
    )
    .expect_err("the same directory reached through a symlink must be rejected");
    assert!(matches!(err, SessionError::WorkspaceLeased { .. }));
}

/// Policy: a lease whose workspace directory has since been deleted must not
/// block a start anywhere else. This is what
/// `find_aliasing_conflict`'s `Err(PathError::NotFound { .. }) => continue`
/// comment already states — a directory that does not currently exist
/// cannot be the directory a fresh candidate names, so it is never a
/// conflict, only a stale-lease signal for Slice 9's recovery action.
///
/// This test alone does **not** distinguish `continue` from `return
/// Ok(None)`: with only one session in the database, both produce the same
/// "no conflict" outcome for `find_aliasing_conflict`. See
/// [`scan_continues_past_a_stale_row_to_find_a_later_conflict`] for the test
/// that actually depends on the difference.
#[test]
fn a_stale_lease_does_not_block_an_unrelated_start() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stale_dir = dir.path().join("stale");
    std::fs::create_dir(&stale_dir).expect("create");
    let elsewhere_dir = dir.path().join("elsewhere");
    std::fs::create_dir(&elsewhere_dir).expect("create");

    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());

    let stale = common::resolve(&stale_dir);
    begin_start(
        &mut store,
        common::uid(1),
        scope_id,
        "agent-stale",
        10,
        &stale,
    )
    .expect("session A takes the lease");

    // A's directory is gone. Its `sessions` row still says `starting` — a
    // live lease-holding state — but nothing exists at its stored path
    // anymore.
    std::fs::remove_dir(&stale_dir).expect("remove");

    let elsewhere = common::resolve(&elsewhere_dir);
    begin_start(
        &mut store,
        common::uid(2),
        scope_id,
        "agent-elsewhere",
        10,
        &elsewhere,
    )
    .expect(
        "a stale lease on a deleted directory must not block a start on an \
         unrelated, currently-existing workspace",
    );
}

/// The test that kills the `continue -> return Ok(None)` mutation by name.
///
/// Session A holds a lease on a directory that gets deleted; session B holds
/// a lease on a *different*, real directory. A is inserted before B, and
/// `find_aliasing_conflict`'s query is `ORDER BY rowid` (see its doc
/// comment), so the scan is guaranteed to visit A — the stale, unresolvable
/// row — before it ever reaches B.
///
/// The candidate is a case-only rename of B's directory, exactly like
/// [`case_variant_lease_conflict_spans_a_rename`]: the database's `BINARY`
/// collation cannot see this conflict at all (proved in
/// `crates/factory-store/tests/lease.rs`), so the *only* thing standing
/// between "rejected" and "two live sessions holding one workspace" is the
/// Rust scan reaching B's row. If `continue` above A's stale row were ever
/// replaced by `return Ok(None)`, the scan would abandon the search at A —
/// before it ever examines B — and this `begin_start` would wrongly
/// **succeed**, which the task report's mutation run confirms.
#[test]
fn scan_continues_past_a_stale_row_to_find_a_later_conflict() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stale_dir = dir.path().join("stale");
    std::fs::create_dir(&stale_dir).expect("create");
    let mixed_case_dir = dir.path().join("Real");
    std::fs::create_dir(&mixed_case_dir).expect("create");
    let lower_case_dir = dir.path().join("real");

    if !lower_case_dir.exists() {
        eprintln!(
            "SKIPPED scan_continues_past_a_stale_row_to_find_a_later_conflict: \
             {} is on a case-sensitive volume, so `Real` and `real` are \
             genuinely different directories here and this test cannot \
             exercise the aliasing defect it exists to check. This is \
             expected on a case-sensitive volume and not a failure, but it \
             means this test verified nothing.",
            dir.path().display()
        );
        return;
    }

    let mut store = Store::open(dir.path()).expect("open");
    let scope_id = common::seed_scope(&mut store, 100, "irrlicht", dir.path());

    // A is inserted first, so it is scanned first under `ORDER BY rowid`.
    let stale = common::resolve(&stale_dir);
    begin_start(
        &mut store,
        common::uid(1),
        scope_id,
        "agent-stale",
        10,
        &stale,
    )
    .expect("session A takes the lease");

    // B is inserted second, on a real, distinct directory.
    let original = common::resolve(&mixed_case_dir);
    begin_start(
        &mut store,
        common::uid(2),
        scope_id,
        "agent-b",
        10,
        &original,
    )
    .expect("session B takes the lease as `Real`");

    // Now A's directory is deleted — its row becomes the stale, unresolvable
    // one the scan must skip past, not stop at.
    std::fs::remove_dir(&stale_dir).expect("remove A's directory");

    // And B's directory is renamed, changing only its case — same inode,
    // stale stored string, exactly as in
    // `case_variant_lease_conflict_spans_a_rename`.
    std::fs::rename(&mixed_case_dir, &lower_case_dir).expect("rename changing only case");
    let candidate = common::resolve(&lower_case_dir);
    assert_ne!(
        original.as_path(),
        candidate.as_path(),
        "expected B's pre-rename stored path to disagree in case with the \
         post-rename candidate path, or this test proves nothing about \
         re-canonicalization"
    );

    let err = begin_start(
        &mut store,
        common::uid(3),
        scope_id,
        "agent-c",
        10,
        &candidate,
    )
    .expect_err(
        "the scan must continue past A's stale row and still find B's \
         genuine conflict; if it stops at A, this begin_start wrongly \
         succeeds and two live sessions end up holding B's workspace",
    );
    assert!(
        matches!(&err, SessionError::WorkspaceLeased { holder_agent, .. } if holder_agent == "agent-b"),
        "expected WorkspaceLeased naming session B specifically — the row \
         after the stale one — got {err:?}"
    );
}
