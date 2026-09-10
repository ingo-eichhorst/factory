//! Integration tests for `factory_doctor::diagnose_with`, one per check
//! named in backlog §10, each built from a fixture that is broken in that
//! specific way — never a healthy fixture asserting "no findings," which
//! would still pass if the check under test were deleted entirely.
//!
//! Every test opens `tempfile::TempDir` only; none of them ever touches the
//! real company root or `/Users/factory/business-factory/.factory/factory.sqlite`.

mod support;

use support::{FakeLaunchd, FakePanes};

fn open_db(company_root: &std::path::Path) {
    factory_store::Store::open(company_root).expect("create the database");
}

fn set_user_version(company_root: &std::path::Path, version: i64) {
    let raw = rusqlite::Connection::open(company_root.join(".factory").join("factory.sqlite"))
        .expect("reopen raw connection");
    raw.pragma_update(None, "user_version", version)
        .expect("set user_version");
}

/// Writes `dispatcher_state`'s one row directly with rusqlite, the way the
/// tests below build a fresh or stale tick without a clock parameter into
/// `diagnose_with`. Uses `to_rfc3339()`, matching
/// `factory_daemon::dispatch::record_tick` exactly — this is what makes a
/// "fresh tick" test pin the cross-crate contract between what the daemon
/// writes and what doctor parses, rather than passing for the wrong reason.
fn write_dispatcher_tick(company_root: &std::path::Path, when: chrono::DateTime<chrono::Utc>) {
    let raw = rusqlite::Connection::open(company_root.join(".factory").join("factory.sqlite"))
        .expect("reopen raw connection");
    raw.execute(
        "INSERT OR REPLACE INTO dispatcher_state (id, last_tick_at) VALUES (1, ?1)",
        [when.to_rfc3339()],
    )
    .expect("insert dispatcher_state row");
}

/// Drops `dispatcher_state` entirely, so a schema-gate test can exercise the
/// real shape of a pre-migration-6 database (the table absent) rather than
/// only a hand-lowered `user_version` pragma over a table that still
/// physically exists.
fn drop_dispatcher_state_table(company_root: &std::path::Path) {
    let raw = rusqlite::Connection::open(company_root.join(".factory").join("factory.sqlite"))
        .expect("reopen raw connection");
    raw.execute("DROP TABLE dispatcher_state", [])
        .expect("drop dispatcher_state");
}

// --- Check 1: configuration validity -----------------------------------

/// Mutation target: delete the `Err(err) => findings.push(Finding::ConfigInvalid(err))`
/// arm in `diagnose_with`, or delete the call to `config::check` entirely.
#[test]
fn config_invalid_is_reported_for_an_unsupported_version() {
    let dir = tempfile::tempdir().expect("tempdir");
    let company_root = dir.path();
    open_db(company_root);

    let config_dir = company_root.join(".factory");
    let bad_yaml = "version: 2\ninstance:\n  id: \"00000000-0000-4000-8000-000000000001\"\n  name: \"x\"\nscopes: []\n";
    std::fs::write(config_dir.join("config.yaml"), bad_yaml).expect("write invalid config");

    let expected_err = factory_config::parse(bad_yaml, config_dir.join("config.yaml"))
        .expect_err("version 2 must fail to validate under SUPPORTED_VERSION 1");

    let report = factory_doctor::diagnose_with(
        company_root,
        &FakePanes(vec![]),
        &FakeLaunchd::Loaded("irrelevant".to_string()),
    )
    .expect("diagnose");

    assert!(
        report
            .findings
            .contains(&factory_doctor::Finding::ConfigInvalid(expected_err)),
        "expected a ConfigInvalid finding, got {:#?}",
        report.findings
    );
}

// --- Check 2: schema version / whether migrations are pending ----------

/// Mutation target: invert `schema::compare`'s `Ordering::Less` arm, or
/// delete the `SchemaBehindBuild` push in `diagnose_with`.
#[test]
fn schema_behind_build_is_reported_when_the_database_predates_this_builds_migrations() {
    let dir = tempfile::tempdir().expect("tempdir");
    let company_root = dir.path();
    open_db(company_root);
    // Simulate an update having been installed with nothing yet migrating
    // the database: hand-lower `user_version` without touching any table.
    // Doctor's schema check reads only this pragma, so this isolates the
    // comparison from the rest of the schema.
    set_user_version(company_root, 3);

    let report = factory_doctor::diagnose_with(
        company_root,
        &FakePanes(vec![]),
        &FakeLaunchd::Loaded("irrelevant".to_string()),
    )
    .expect("diagnose");

    assert_eq!(report.schema_version, 3);
    assert!(
        report
            .findings
            .contains(&factory_doctor::Finding::SchemaBehindBuild {
                database_schema: 3,
                built_schema: factory_doctor::latest_schema_version(),
            }),
        "expected SchemaBehindBuild, got {:#?}",
        report.findings
    );
}

/// The other side of the same comparison: a database migrated by a *newer*
/// build than this one. Mutation target: invert the `Ordering::Greater` arm.
#[test]
fn schema_ahead_of_build_is_reported_when_the_database_is_newer_than_this_build() {
    let dir = tempfile::tempdir().expect("tempdir");
    let company_root = dir.path();
    open_db(company_root);
    set_user_version(company_root, factory_doctor::latest_schema_version() + 1);

    let report = factory_doctor::diagnose_with(
        company_root,
        &FakePanes(vec![]),
        &FakeLaunchd::Loaded("irrelevant".to_string()),
    )
    .expect("diagnose");

    assert!(
        report
            .findings
            .contains(&factory_doctor::Finding::SchemaAheadOfBuild {
                database_schema: factory_doctor::latest_schema_version() + 1,
                built_schema: factory_doctor::latest_schema_version(),
            }),
        "expected SchemaAheadOfBuild, got {:#?}",
        report.findings
    );
}

/// Registry drift (check 3) and the pane audit (check 5) both read columns
/// that do not exist before specific migrations. Mutation target: delete
/// either `if schema_version < MIN_SCHEMA` gate in `registry.rs`/`panes.rs`.
/// Without it, the fixture below is a fully-migrated (schema 5) database
/// whose `user_version` was hand-lowered — the columns physically exist, so
/// deleting the gate would silently change the finding produced (a normal
/// `RegistryDrift`/pane comparison instead of `CheckSkipped`), which this
/// test's exact-match assertions catch.
#[test]
fn registry_and_pane_checks_are_skipped_with_a_reason_when_schema_is_behind_their_minimums() {
    let dir = tempfile::tempdir().expect("tempdir");
    let company_root = dir.path();
    open_db(company_root);
    set_user_version(company_root, 1);
    support::write_valid_config(company_root, support::uid(1), support::uid(2), ".");

    let report = factory_doctor::diagnose_with(
        company_root,
        &FakePanes(vec![]),
        &FakeLaunchd::Loaded("irrelevant".to_string()),
    )
    .expect("diagnose");

    assert!(
        report
            .findings
            .contains(&factory_doctor::Finding::CheckSkipped {
                check: factory_doctor::CheckName::RegistryDrift,
                reason: "database schema is 1; registry reconciliation reads `scopes` \
                         columns introduced in schema 2"
                    .to_string(),
            }),
        "expected registry drift to be reported skipped, got {:#?}",
        report.findings
    );
    assert!(
        report
            .findings
            .contains(&factory_doctor::Finding::CheckSkipped {
                check: factory_doctor::CheckName::SessionPaneAudit,
                reason: "database schema is 1; `sessions.herdr_pane_id` was introduced \
                         in schema 5"
                    .to_string(),
            }),
        "expected the pane audit to be reported skipped, got {:#?}",
        report.findings
    );
    assert!(
        !report.findings.iter().any(|f| matches!(
            f,
            factory_doctor::Finding::RegistryDrift(_)
                | factory_doctor::Finding::SessionPaneGone { .. }
                | factory_doctor::Finding::PaneStillAliveForDeadSession { .. }
        )),
        "a skipped check must not also report a real drift/pane finding: {:#?}",
        report.findings
    );
}

// --- Check 2, integrity half: PRAGMA integrity_check --------------------

/// Finds a byte range whose corruption makes `PRAGMA integrity_check` return
/// a plain non-`"ok"` verdict (rather than the whole query failing outright,
/// which happens if a b-tree page *header* is corrupted instead of its cell
/// content) and leaves that corruption in place, returning the verdict.
///
/// A leaf page packs its cell pointers at the start and its cell *content*
/// backward from its end (measured directly against a real fixture built
/// exactly like the one below: `sqlite3 <path> 'SELECT name, pageno, ncell,
/// payload FROM dbstat'` finds the populated `scopes` page, and corrupting
/// its last 60 bytes reports `"row 1 missing from index
/// sqlite_autoindex_scopes_2"` — a row, not an error). Rather than hardcode
/// that one page number (which schema growth could shift), this scans every
/// page after the first (page 1's header carries `user_version`, which
/// `diagnose_with` must still be able to read afterward) for one whose
/// tail-corruption produces exactly that shape of result, restoring any page
/// that does not.
fn corrupt_a_page_until_integrity_check_fails(db_path: &std::path::Path) -> String {
    const PAGE_SIZE: usize = 4096;

    let original = std::fs::read(db_path).expect("read db file");
    let page_count = original.len() / PAGE_SIZE;

    for page_no in 2..=page_count {
        let start = (page_no - 1) * PAGE_SIZE;
        let corrupt_start = start + PAGE_SIZE - 60;
        let corrupt_end = start + PAGE_SIZE - 40;

        let mut candidate = original.clone();
        for byte in &mut candidate[corrupt_start..corrupt_end] {
            *byte = b'X';
        }
        std::fs::write(db_path, &candidate).expect("write candidate corruption");

        let verdict = {
            let probe = rusqlite::Connection::open(db_path).expect("open probe connection");
            probe.pragma_query_value(None, "integrity_check", |row| row.get::<_, String>(0))
        };

        if let Ok(verdict) = verdict {
            if verdict != "ok" {
                return verdict;
            }
        }
        // Either this page held no cell to disturb, or disturbing it broke
        // nothing an integrity check notices — restore it and try the next.
        std::fs::write(db_path, &original).expect("restore original bytes");
    }

    panic!(
        "could not find a page among {page_count} whose corruption produces a \
         non-\"ok\", non-erroring PRAGMA integrity_check verdict"
    );
}

/// Mutation target: delete the `if integrity_check != "ok" { findings.push(...) }`
/// check in `diagnose_with`.
#[test]
fn integrity_check_failure_is_reported_when_the_database_file_is_corrupted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let company_root = dir.path();
    let db_path = company_root.join(".factory").join("factory.sqlite");

    {
        let mut store = factory_store::Store::open(company_root).expect("open store");
        // Deliberately just a scope, no session: `sessions.state` is
        // CHECK-constrained (`factory_session::SessionState::from_db_str`
        // panics on an unrecognized value), and a raw byte-level corruption
        // bypasses that constraint entirely — corrupting *some* page at
        // random could land on a session row's `state` text and crash a
        // check this test has no interest in, rather than exercising
        // `integrity_check` alone. A lone `scopes` row has no such
        // CHECK-constrained text column, and this test's fixture writes no
        // `config.yaml`, so registry drift (the only check that reads
        // `scopes`) reports itself skipped rather than querying the table
        // this corruption may have landed on.
        support::seed_scope(&mut store, 1, &company_root.join("scope"));
    } // Dropped here: WAL checkpoints into the main file on close, so the
    // corruption below lands where a reader will actually see it.

    let verdict = corrupt_a_page_until_integrity_check_fails(&db_path);

    let report = factory_doctor::diagnose_with(
        company_root,
        &FakePanes(vec![]),
        &FakeLaunchd::Loaded("irrelevant".to_string()),
    )
    .expect(
        "schema_version lives on page 1, untouched by this corruption, so diagnose_with \
         must still return Ok with the finding attached rather than a hard DoctorError",
    );

    assert_eq!(report.integrity_check, verdict);
    assert_ne!(report.integrity_check, "ok");
    assert!(
        report
            .findings
            .contains(&factory_doctor::Finding::IntegrityCheckFailed(verdict)),
        "expected IntegrityCheckFailed, got {:#?}",
        report.findings
    );
}

// --- Check 3: registry drift --------------------------------------------

/// Mutation target: delete the `for drift in report.items { findings.push(...) }`
/// loop in `registry.rs`, or the call to `registry::check` in
/// `diagnose_with`.
#[test]
fn registry_drift_reports_a_scope_whose_declared_path_no_longer_exists() {
    let dir = tempfile::tempdir().expect("tempdir");
    let company_root = dir.path();
    open_db(company_root);

    let instance_id = support::uid(1);
    let scope_id = support::uid(2);
    // Deliberately never created: `child` names a path with no directory.
    support::write_valid_config(company_root, instance_id, scope_id, "child");

    let report = factory_doctor::diagnose_with(
        company_root,
        &FakePanes(vec![]),
        &FakeLaunchd::Loaded("irrelevant".to_string()),
    )
    .expect("diagnose");

    let expected = factory_registry::Drift::MissingPath {
        id: scope_id,
        name: "root".to_string(),
        path: std::path::PathBuf::from("child"),
    };
    assert!(
        report
            .findings
            .contains(&factory_doctor::Finding::RegistryDrift(expected)),
        "expected a MissingPath registry drift finding, got {:#?}",
        report.findings
    );
}

// --- Check 4: leases held with no live session --------------------------

/// Mutation target: invert `session.state.holds_lease()`'s guard, invert
/// `lease.released_at.is_none()`, or delete the call to `leases::check` in
/// `diagnose_with`.
#[test]
fn orphaned_lease_is_reported_when_a_sessions_state_moved_without_releasing_its_lease() {
    let dir = tempfile::tempdir().expect("tempdir");
    let company_root = dir.path();
    let workspace = company_root.join("workspace");

    let session_id = {
        let mut store = factory_store::Store::open(company_root).expect("open store");
        let scope_id = support::seed_scope(&mut store, 1, &company_root.join("scope"));
        let session_id = support::session_in(
            &mut store,
            2,
            scope_id,
            &workspace,
            factory_session::SessionState::Running,
        );

        // Corrupt the row directly: move the session to `stopped` WITHOUT
        // going through `factory_session::stop`, which releases the lease
        // in the very same transaction as the state change. This is
        // exactly the inconsistency check 4 exists to find — not something
        // reachable through the real API, which is why it is written
        // directly here.
        let tx = store.transaction().expect("begin");
        tx.execute(
            "UPDATE sessions SET state = 'stopped' WHERE id = ?1",
            [session_id.to_string()],
        )
        .expect("corrupt session state without releasing its lease");
        tx.commit().expect("commit");

        session_id
    };

    let report = factory_doctor::diagnose_with(
        company_root,
        &FakePanes(vec![]),
        &FakeLaunchd::Loaded("irrelevant".to_string()),
    )
    .expect("diagnose");

    let workspace_canonical = factory_paths::CanonicalPath::resolve(&workspace)
        .expect("resolve workspace")
        .as_path()
        .to_string_lossy()
        .into_owned();
    let expected = factory_doctor::Finding::OrphanedLease {
        session_id,
        agent_name: "agent".to_string(),
        session_state: factory_session::SessionState::Stopped,
        workspace_path: workspace_canonical,
        lease_id: 1,
    };
    assert!(
        report.findings.contains(&expected),
        "expected an OrphanedLease finding, got {:#?}",
        report.findings
    );
}

// --- Check 5: database sessions against actual Herdr panes --------------

/// Both directions in one fixture: a session the database calls live whose
/// pane Herdr no longer has, and a session the database does not call live
/// whose pane Herdr still has. Mutation target: swap or delete either
/// `if`/`else if` branch in `panes::check`, or delete the call entirely.
#[test]
fn session_pane_audit_reports_both_directions_of_drift() {
    let dir = tempfile::tempdir().expect("tempdir");
    let company_root = dir.path();

    let (running_id, stopped_id) = {
        let mut store = factory_store::Store::open(company_root).expect("open store");
        let scope_id = support::seed_scope(&mut store, 1, &company_root.join("scope"));

        let running_id = support::session_in(
            &mut store,
            2,
            scope_id,
            &company_root.join("ws-running"),
            factory_session::SessionState::Running,
        );
        let stopped_id = support::session_in(
            &mut store,
            3,
            scope_id,
            &company_root.join("ws-stopped"),
            factory_session::SessionState::Stopped,
        );

        // Nothing in `factory_session` writes `herdr_pane_id` yet (see
        // `panes.rs`'s module docs), so this fixture sets it directly.
        let tx = store.transaction().expect("begin");
        tx.execute(
            "UPDATE sessions SET herdr_pane_id = 'w1:pGone' WHERE id = ?1",
            [running_id.to_string()],
        )
        .expect("set pane on the running session");
        tx.execute(
            "UPDATE sessions SET herdr_pane_id = 'w1:pStillAlive' WHERE id = ?1",
            [stopped_id.to_string()],
        )
        .expect("set pane on the stopped session");
        tx.commit().expect("commit");

        (running_id, stopped_id)
    };

    // "w1:pGone" is absent (the running session's pane no longer exists);
    // "w1:pStillAlive" is present (the stopped session's pane still does).
    let herdr = FakePanes(vec!["w1:pStillAlive".to_string()]);

    let report = factory_doctor::diagnose_with(
        company_root,
        &herdr,
        &FakeLaunchd::Loaded("irrelevant".to_string()),
    )
    .expect("diagnose");

    assert!(
        report
            .findings
            .contains(&factory_doctor::Finding::SessionPaneGone {
                session_id: running_id,
                pane_id: "w1:pGone".to_string(),
            }),
        "expected SessionPaneGone for the running session, got {:#?}",
        report.findings
    );
    assert!(
        report
            .findings
            .contains(&factory_doctor::Finding::PaneStillAliveForDeadSession {
                session_id: stopped_id,
                pane_id: "w1:pStillAlive".to_string(),
            }),
        "expected PaneStillAliveForDeadSession for the stopped session, got {:#?}",
        report.findings
    );
}

/// When Herdr itself cannot be queried, the pane audit reports that
/// honestly instead of silently skipping or crashing. Mutation target:
/// delete the `Err(err) => findings.push(Finding::HerdrPaneQueryFailed(err))`
/// arm.
#[test]
fn herdr_query_failure_is_reported_when_a_session_has_a_recorded_pane() {
    struct FailingPanes;
    impl factory_doctor::LivePanes for FailingPanes {
        fn live_pane_ids(&self) -> Result<Vec<String>, String> {
            Err("herdr not running".to_string())
        }
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let company_root = dir.path();
    {
        let mut store = factory_store::Store::open(company_root).expect("open store");
        let scope_id = support::seed_scope(&mut store, 1, &company_root.join("scope"));
        let session_id = support::session_in(
            &mut store,
            2,
            scope_id,
            &company_root.join("ws"),
            factory_session::SessionState::Running,
        );
        let tx = store.transaction().expect("begin");
        tx.execute(
            "UPDATE sessions SET herdr_pane_id = 'w1:p1' WHERE id = ?1",
            [session_id.to_string()],
        )
        .expect("set pane");
        tx.commit().expect("commit");
    }

    let report = factory_doctor::diagnose_with(
        company_root,
        &FailingPanes,
        &FakeLaunchd::Loaded("irrelevant".to_string()),
    )
    .expect("diagnose");

    assert!(
        report
            .findings
            .contains(&factory_doctor::Finding::HerdrPaneQueryFailed(
                "herdr not running".to_string()
            )),
        "expected HerdrPaneQueryFailed, got {:#?}",
        report.findings
    );
}

// --- Check 6: Factory's dispatcher tick, and the foreign scheduler ------
//
// ADR 0021 decision 10: check 6 answers two questions that must not be
// blended into one verdict. The tests below cover each tick state alone
// (with the foreign label held at `NotLoaded`, so a stale/fresh finding
// cannot be confused with a two-dispatchers one), then the two states
// where the label being loaded changes the answer.

/// Mutation target: delete the `if age_seconds.is_none()` / no-row branch in
/// `scheduler::read_last_tick`, or make it push a finding anyway. No
/// dispatcher has ever ticked against this database — ordinary on a fresh
/// instance (`factory_store::schema`'s own comment on `dispatcher_state`).
#[test]
fn dispatcher_tick_absent_is_not_a_finding_on_a_fresh_instance() {
    let dir = tempfile::tempdir().expect("tempdir");
    let company_root = dir.path();
    open_db(company_root);

    let report =
        factory_doctor::diagnose_with(company_root, &FakePanes(vec![]), &FakeLaunchd::NotLoaded)
            .expect("diagnose");

    assert_eq!(report.scheduler.last_tick_at, None);
    assert!(
        !report.findings.iter().any(|f| matches!(
            f,
            factory_doctor::Finding::DispatcherTickStale { .. }
                | factory_doctor::Finding::TwoDispatchers { .. }
        )),
        "no dispatcher has ever ticked; that must not be a finding: {:#?}",
        report.findings
    );
}

/// The predicate-inversion counterpart to the stale test below: a fresh
/// tick, alone, must never be a finding. Without this, a mutation that
/// always reports `DispatcherTickStale` would still pass the "stale" test.
#[test]
fn dispatcher_tick_fresh_is_not_a_finding() {
    let dir = tempfile::tempdir().expect("tempdir");
    let company_root = dir.path();
    open_db(company_root);
    write_dispatcher_tick(company_root, chrono::Utc::now());

    let report =
        factory_doctor::diagnose_with(company_root, &FakePanes(vec![]), &FakeLaunchd::NotLoaded)
            .expect("diagnose");

    assert!(report.scheduler.last_tick_at.is_some());
    assert!(
        !report.findings.iter().any(|f| matches!(
            f,
            factory_doctor::Finding::DispatcherTickStale { .. }
                | factory_doctor::Finding::TwoDispatchers { .. }
        )),
        "a fresh tick must not be a finding: {:#?}",
        report.findings
    );
}

/// Mutation target: delete or widen the `age_seconds.abs() >
/// STALE_AFTER_SECONDS` comparison in `scheduler::report_tick_health`.
#[test]
fn dispatcher_tick_stale_is_a_finding() {
    let dir = tempfile::tempdir().expect("tempdir");
    let company_root = dir.path();
    open_db(company_root);
    let stale = chrono::Utc::now() - chrono::Duration::hours(1);
    write_dispatcher_tick(company_root, stale);

    let report =
        factory_doctor::diagnose_with(company_root, &FakePanes(vec![]), &FakeLaunchd::NotLoaded)
            .expect("diagnose");

    let (last_tick_at, age_seconds) = report
        .findings
        .iter()
        .find_map(|f| match f {
            factory_doctor::Finding::DispatcherTickStale {
                last_tick_at,
                age_seconds,
            } => Some((last_tick_at.clone(), *age_seconds)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected DispatcherTickStale, got {:#?}", report.findings));

    assert_eq!(last_tick_at, stale.to_rfc3339());
    // An hour ago, computed against a live clock rather than a fixed one
    // injected into `diagnose_with` — loose bounds absorb the test's own
    // run time.
    assert!(
        (3000..4200).contains(&age_seconds),
        "age_seconds = {age_seconds}, expected roughly 3600"
    );
}

/// Binding rule: staleness is compared on the *magnitude* of the age, not
/// its sign, so a clock-skewed or corrupted row cannot masquerade as fresh
/// forever. A future timestamp must still be reported stale, with its
/// signed (negative) age carried honestly.
#[test]
fn dispatcher_tick_in_the_future_is_reported_stale_not_fresh() {
    let dir = tempfile::tempdir().expect("tempdir");
    let company_root = dir.path();
    open_db(company_root);
    let future = chrono::Utc::now() + chrono::Duration::hours(1);
    write_dispatcher_tick(company_root, future);

    let report =
        factory_doctor::diagnose_with(company_root, &FakePanes(vec![]), &FakeLaunchd::NotLoaded)
            .expect("diagnose");

    let age_seconds = report
        .findings
        .iter()
        .find_map(|f| match f {
            factory_doctor::Finding::DispatcherTickStale { age_seconds, .. } => Some(*age_seconds),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "expected DispatcherTickStale for a future timestamp, got {:#?}",
                report.findings
            )
        });

    assert!(
        age_seconds < 0,
        "a future tick's age must read as a negative number, not be mistaken for fresh: \
         {age_seconds}"
    );
}

/// ADR 0021 decision 10 / ADR 0014: a loaded foreign scheduler next to a
/// fresh Factory tick is two dispatchers against one database.
#[test]
fn a_loaded_foreign_scheduler_next_to_a_fresh_tick_is_two_dispatchers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let company_root = dir.path();
    open_db(company_root);
    write_dispatcher_tick(company_root, chrono::Utc::now());

    // The record names *this* root, which is what makes it this instance's
    // second dispatcher rather than some other instance's only one.
    let report = factory_doctor::diagnose_with(
        company_root,
        &FakePanes(vec![]),
        &FakeLaunchd::Loaded(loaded_record_for(company_root)),
    )
    .expect("diagnose");

    assert!(report.scheduler.loaded);
    assert!(report.scheduler.targets_this_root);
    assert!(
        report.findings.iter().any(|f| matches!(
            f,
            factory_doctor::Finding::TwoDispatchers { label, .. }
                if label == factory_doctor::SCHEDULER_LABEL
        )),
        "expected TwoDispatchers, got {:#?}",
        report.findings
    );
}

/// A `launchctl list` record shaped like the real one, measured on
/// 2026-09-10 — `ProgramArguments` and the two log paths, all absolute, all
/// under the instance root the job serves.
fn loaded_record_for(company_root: &std::path::Path) -> String {
    let root = std::fs::canonicalize(company_root)
        .expect("canonicalize")
        .to_string_lossy()
        .into_owned();
    format!(
        "{{\n\t\"Label\" = \"com.business-factory.scheduler\";\n\t\"ProgramArguments\" = (\n\t\t\
         \"/usr/bin/python3\";\n\t\t\"{root}/scripts/factory_tasks.py\";\n\t\t\"dispatch\";\n\t);\n\t\
         \"StandardOutPath\" = \"{root}/.factory/logs/scheduler.log\";\n}};\n"
    )
}

/// The defect the station-11 live drill found, and the reason
/// `targets_this_root` exists at all.
///
/// `launchctl` is machine-wide. A throwaway instance under `/tmp` reported
/// "two dispatchers against one database" while the loaded job was serving
/// the company root and had never touched the throwaway database. The claim
/// was about a database doctor had not checked — ADR 0017's "a wrong answer
/// is worse than none, because it looks like an answer", in the check whose
/// whole job is to notice a second writer.
#[test]
fn a_scheduler_loaded_for_another_instance_is_not_this_instances_second_dispatcher() {
    let dir = tempfile::tempdir().expect("tempdir");
    let company_root = dir.path();
    open_db(company_root);
    write_dispatcher_tick(company_root, chrono::Utc::now());

    let elsewhere = "{\n\t\"ProgramArguments\" = (\n\t\t\"/usr/bin/python3\";\n\t\t\
                     \"/Users/someone/another-instance/scripts/factory_tasks.py\";\n\t);\n}};\n";

    let report = factory_doctor::diagnose_with(
        company_root,
        &FakePanes(vec![]),
        &FakeLaunchd::Loaded(elsewhere.to_string()),
    )
    .expect("diagnose");

    assert!(report.scheduler.loaded, "the job really is loaded");
    assert!(
        !report.scheduler.targets_this_root,
        "but nothing in its record names this instance root"
    );
    assert!(
        !report
            .findings
            .iter()
            .any(|f| matches!(f, factory_doctor::Finding::TwoDispatchers { .. })),
        "a scheduler serving another instance is not this instance's second \
         dispatcher: {:#?}",
        report.findings
    );
}

/// The boundary case that a plain substring search gets wrong: a root of
/// `/tmp/f11` must not match a job serving `/tmp/f11d`.
#[test]
fn a_root_that_is_a_prefix_of_another_roots_path_does_not_match() {
    let dir = tempfile::tempdir().expect("tempdir");
    let company_root = dir.path();
    open_db(company_root);
    write_dispatcher_tick(company_root, chrono::Utc::now());

    let root = std::fs::canonicalize(company_root)
        .expect("canonicalize")
        .to_string_lossy()
        .into_owned();
    let neighbour = format!("{{\n\t\"ProgramArguments\" = (\"{root}-other/scripts/x.py\");\n}};\n");

    let report = factory_doctor::diagnose_with(
        company_root,
        &FakePanes(vec![]),
        &FakeLaunchd::Loaded(neighbour),
    )
    .expect("diagnose");

    assert!(
        !report.scheduler.targets_this_root,
        "`{root}-other` is a different root, not this one"
    );
}

/// A stale dispatcher is not dispatching, so a loaded foreign scheduler next
/// to a stale tick must report the stale finding alone, never
/// `TwoDispatchers` as well.
#[test]
fn a_loaded_foreign_scheduler_next_to_a_stale_tick_reports_stale_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    let company_root = dir.path();
    open_db(company_root);
    let stale = chrono::Utc::now() - chrono::Duration::hours(1);
    write_dispatcher_tick(company_root, stale);

    let report = factory_doctor::diagnose_with(
        company_root,
        &FakePanes(vec![]),
        &FakeLaunchd::Loaded("job details".to_string()),
    )
    .expect("diagnose");

    assert!(
        report
            .findings
            .iter()
            .any(|f| matches!(f, factory_doctor::Finding::DispatcherTickStale { .. })),
        "expected DispatcherTickStale, got {:#?}",
        report.findings
    );
    assert!(
        !report
            .findings
            .iter()
            .any(|f| matches!(f, factory_doctor::Finding::TwoDispatchers { .. })),
        "a stale dispatcher is not dispatching; it must not also be reported as a second \
         writer: {:#?}",
        report.findings
    );
}

/// The schema gate is the load-bearing case (ADR 0021 decision 10): the real
/// machine has a schema-5-or-lower database, with no `dispatcher_state`
/// table at all (the Python prototype tracks its own migrations
/// elsewhere). Drops the table for real, rather than only hand-lowering
/// `user_version` over a table that still physically exists, so this
/// fixture matches that machine exactly. Without the gate in
/// `scheduler::read_last_tick`, reading the table fails; proving the
/// failure never reaches `diagnose_with` as a whole-report `Err` is the
/// point of this test, not a side effect of it.
#[test]
fn dispatcher_tick_is_skipped_below_schema_6_and_the_rest_of_the_report_is_intact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let company_root = dir.path();
    open_db(company_root);
    support::write_valid_config_with_no_scopes(company_root, support::uid(1));
    drop_dispatcher_state_table(company_root);
    set_user_version(company_root, 5);

    let report = factory_doctor::diagnose_with(
        company_root,
        &FakePanes(vec![]),
        &FakeLaunchd::Loaded("irrelevant".to_string()),
    )
    .expect("diagnose must still return a full report, not Err, below schema 6");

    assert!(
        report
            .findings
            .contains(&factory_doctor::Finding::CheckSkipped {
                check: factory_doctor::CheckName::DispatcherTick,
                reason: "database schema is 5; `dispatcher_state` was introduced in schema 6"
                    .to_string(),
            }),
        "expected DispatcherTick to be reported skipped, got {:#?}",
        report.findings
    );
    assert_eq!(report.scheduler.last_tick_at, None);
    // The launchd half's answer survives untouched by the schema gate.
    assert!(report.scheduler.loaded);
    // Every other check's own output is intact: schema 5 is behind this
    // build (a real, separate finding), and registry/pane both run
    // normally at schema 5 rather than being dragged into a skip that is
    // only the dispatcher's to report.
    assert!(
        report
            .findings
            .iter()
            .any(|f| matches!(f, factory_doctor::Finding::SchemaBehindBuild { .. })),
        "expected SchemaBehindBuild, got {:#?}",
        report.findings
    );
    assert!(
        !report.findings.iter().any(|f| matches!(
            f,
            factory_doctor::Finding::CheckSkipped {
                check: factory_doctor::CheckName::RegistryDrift
                    | factory_doctor::CheckName::SessionPaneAudit,
                ..
            }
        )),
        "registry drift and the pane audit both run fine at schema 5; only the dispatcher \
         tick needs schema 6: {:#?}",
        report.findings
    );
    assert_eq!(report.integrity_check, "ok");
}

#[test]
fn scheduler_query_failure_is_reported() {
    let dir = tempfile::tempdir().expect("tempdir");
    let company_root = dir.path();
    open_db(company_root);

    let report = factory_doctor::diagnose_with(
        company_root,
        &FakePanes(vec![]),
        &FakeLaunchd::Error("launchctl: command not found".to_string()),
    )
    .expect("diagnose");

    assert!(!report.scheduler.loaded);
    assert!(
        report
            .findings
            .contains(&factory_doctor::Finding::SchedulerQueryFailed {
                label: factory_doctor::SCHEDULER_LABEL.to_string(),
                error: "launchctl: command not found".to_string(),
            }),
        "expected SchedulerQueryFailed, got {:#?}",
        report.findings
    );
}

// --- Cross-cutting sanity -------------------------------------------------

/// Every check clean at once: the one fixture in this file that is *not*
/// broken. Exists as a smoke test, not a substitute for the broken-fixture
/// tests above — a healthy report alone cannot distinguish a correct check
/// from a deleted one.
#[test]
fn a_freshly_initialized_instance_with_no_scopes_and_a_loaded_scheduler_is_healthy() {
    let dir = tempfile::tempdir().expect("tempdir");
    let company_root = dir.path();
    open_db(company_root);
    support::write_valid_config_with_no_scopes(company_root, support::uid(1));

    let report = factory_doctor::diagnose_with(
        company_root,
        &FakePanes(vec![]),
        &FakeLaunchd::Loaded("job details".to_string()),
    )
    .expect("diagnose");

    assert!(
        report.is_healthy(),
        "expected no findings, got {:#?}",
        report.findings
    );
    assert_eq!(
        report.schema_version,
        factory_doctor::latest_schema_version()
    );
    assert_eq!(report.integrity_check, "ok");
}

/// `backups::summarize`'s own unit tests call it directly, which proves the
/// counting/summing logic but not that `diagnose_with` actually passes it
/// the right directory — a wrong path at that one call site (`company_root`
/// where `db_path` belongs, say) would pass every `backups::tests` test
/// unchanged. This drives the fact through the public entry point instead.
#[test]
fn diagnose_reports_a_file_written_into_the_backups_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let company_root = dir.path();
    open_db(company_root);

    let backups_dir = company_root.join(".factory").join("backups");
    std::fs::create_dir_all(&backups_dir).expect("create backups dir");
    std::fs::write(
        backups_dir.join("pre-migration-1-to-2-1.sqlite"),
        vec![0u8; 42],
    )
    .expect("write a fake backup");

    let report = factory_doctor::diagnose_with(
        company_root,
        &FakePanes(vec![]),
        &FakeLaunchd::Loaded("irrelevant".to_string()),
    )
    .expect("diagnose");

    assert_eq!(report.backups.count, 1);
    assert_eq!(report.backups.total_size_bytes, 42);
}

/// Every other test in this file injects [`FakeLaunchd`] and [`FakePanes`],
/// so [`factory_doctor::SystemLaunchd`] itself has never actually run.
/// [`factory_doctor::diagnose`] is the real entry point, wired to the real
/// `launchctl` and `herdr` binaries — this is the one test that calls it,
/// against an otherwise-empty tempdir fixture (no session ever records a
/// pane, so `SystemHerdr` is never even reached — see `panes.rs`'s own
/// short-circuit — but `SystemLaunchd::list` always runs). It only asserts
/// that the call completes and the field it fills in is internally
/// consistent, since what `launchctl` actually reports on the machine
/// running this suite is exactly the fact this task's own report must state
/// honestly rather than assume.
#[test]
fn diagnose_against_the_real_launchctl_and_herdr_binaries_completes_and_is_internally_consistent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let company_root = dir.path();
    open_db(company_root);

    let report = factory_doctor::diagnose(company_root).expect("diagnose");

    assert_eq!(report.scheduler.label, factory_doctor::SCHEDULER_LABEL);
    // A fresh tempdir database has no `dispatcher_state` row: no dispatcher
    // has ever ticked against it, whatever `loaded` turns out to be on the
    // machine running this suite (ADR 0021 decision 10: the foreign label
    // being loaded is the documented, ordinary pre-cut-over state).
    assert_eq!(report.scheduler.last_tick_at, None);
    assert!(
        !report.findings.iter().any(|f| matches!(
            f,
            factory_doctor::Finding::DispatcherTickStale { .. }
                | factory_doctor::Finding::TwoDispatchers { .. }
        )),
        "no tick was ever recorded on this fresh fixture, so neither finding can fire: {:#?}",
        report.findings
    );
}

/// The one hard stop: this crate's own binding decision 1 requires it to
/// read through `Store::open_read_only`, which never creates a database.
#[test]
fn diagnose_fails_when_the_database_does_not_exist() {
    let dir = tempfile::tempdir().expect("tempdir");

    let result =
        factory_doctor::diagnose_with(dir.path(), &FakePanes(vec![]), &FakeLaunchd::NotLoaded);

    assert!(
        matches!(result, Err(factory_doctor::DoctorError::Store(_))),
        "expected DoctorError::Store, got {result:?}"
    );
}
