//! `restore::reconcile` against ADR 0019 decision 2 (design §5, backlog §9).
//!
//! Every fixture here plants rows directly, in the shape a restored snapshot
//! is described as carrying, rather than driving `factory-session` /
//! `factory-task`'s real APIs to reach the same state — see
//! `restore_common::seed_session`'s doc comment for why.

mod restore_common;

use factory_recovery::restore::reconcile;
use factory_session::{SessionError, SessionState, begin_start};
use factory_store::Store;

/// ADR 0019 decision 2: "Every lease-holding session becomes `disconnected`,
/// and keeps its lease." Exercised against all three lease-holding source
/// states, plus one `stopped` and one `failed` session that must be left
/// alone entirely.
///
/// "Keeps its lease" is asserted the way that actually discriminates it from
/// "released the lease": not by reading `workspace_leases.released_at`
/// (`factory_session::reconcile_to_disconnected` never touches that column
/// for a lease-holding move, so it cannot tell a correct implementation from
/// one that quietly changed the *state* written without releasing the row),
/// but by attempting a real `begin_start` on the identical, real, on-disk
/// workspace afterward and checking it is still rejected — the actual
/// behaviour ADR 0012 decision 5's lease exists to guarantee.
#[test]
fn reconcile_disconnects_every_lease_holding_session_and_keeps_its_lease() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope = restore_common::seed_scope(&mut store, 1, dir.path());

    let starting_ws = dir.path().join("ws-starting");
    let running_ws = dir.path().join("ws-running");
    let disconnected_ws = dir.path().join("ws-disconnected");
    let stopped_ws = dir.path().join("ws-stopped");
    let failed_ws = dir.path().join("ws-failed");

    let starting = restore_common::seed_session(&mut store, 101, scope, &starting_ws, "starting");
    let running = restore_common::seed_session(&mut store, 102, scope, &running_ws, "running");
    let disconnected =
        restore_common::seed_session(&mut store, 103, scope, &disconnected_ws, "disconnected");
    let stopped = restore_common::seed_session(&mut store, 104, scope, &stopped_ws, "stopped");
    let failed = restore_common::seed_session(&mut store, 105, scope, &failed_ws, "failed");

    let report = reconcile(&mut store).expect("reconcile");

    assert_eq!(
        restore_common::session_state(&store, starting),
        "disconnected"
    );
    assert_eq!(
        restore_common::session_state(&store, running),
        "disconnected"
    );
    assert_eq!(
        restore_common::session_state(&store, disconnected),
        "disconnected"
    );
    assert_eq!(restore_common::session_state(&store, stopped), "stopped");
    assert_eq!(restore_common::session_state(&store, failed), "failed");

    // Report names the two that actually changed state — the row already
    // `disconnected` did not change, per this module's idempotence.
    let changed_ids: std::collections::HashSet<_> = report.sessions.iter().map(|s| s.id).collect();
    assert_eq!(
        changed_ids,
        [starting, running].into_iter().collect(),
        "only the sessions whose state actually moved belong in the report"
    );
    for change in &report.sessions {
        assert_eq!(change.after, SessionState::Disconnected);
    }

    // The real proof the lease was kept, for every formerly-lease-holding
    // workspace: a second `begin_start` on the identical directory is still
    // rejected.
    for (seed, workspace) in [
        (901, &starting_ws),
        (902, &running_ws),
        (903, &disconnected_ws),
    ] {
        let canonical = factory_paths::CanonicalPath::resolve(workspace).expect("resolve");
        let err = begin_start(
            &mut store,
            restore_common::uid(seed),
            scope,
            "agent",
            10,
            &canonical,
        )
        .expect_err("the lease must still be held after reconcile");
        assert!(matches!(err, SessionError::WorkspaceLeased { .. }));
    }

    // The `stopped` and `failed` workspaces were never lease-holding, so a
    // fresh start there must succeed — proving reconcile did not
    // accidentally leave (or invent) a lease on them.
    for (seed, workspace) in [(904, &stopped_ws), (905, &failed_ws)] {
        let canonical = factory_paths::CanonicalPath::resolve(workspace).expect("resolve");
        begin_start(
            &mut store,
            restore_common::uid(seed),
            scope,
            "agent",
            10,
            &canonical,
        )
        .expect("stopped/failed sessions hold no lease for reconcile to have touched");
    }
}

/// ADR 0019 decision 2's first task row: a `running` task becomes
/// `blocked: interrupted` unconditionally — design §5's "Factory does not
/// automatically resend a possibly delivered prompt."
#[test]
fn reconcile_blocks_a_running_task_as_interrupted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope = restore_common::seed_scope(&mut store, 1, dir.path());
    let ws = dir.path().join("ws");
    let session = restore_common::seed_session(&mut store, 1, scope, &ws, "running");
    let task = restore_common::seed_task(&mut store, 1, scope, "running", Some(session));

    let report = reconcile(&mut store).expect("reconcile");

    let (status, blocked_reason, assigned) = restore_common::task_row(&store, task);
    assert_eq!(status, "blocked");
    assert_eq!(blocked_reason.as_deref(), Some("interrupted"));
    assert_eq!(
        assigned.as_deref(),
        Some(session.to_string()).as_deref(),
        "the running row's assignment is not part of what this table row changes"
    );

    let change = report
        .tasks
        .iter()
        .find(|t| t.id == task)
        .expect("task reported as changed");
    assert_eq!(change.before_status, "running");
    assert_eq!(change.after_status, "blocked");
    assert_eq!(change.after_blocked_reason.as_deref(), Some("interrupted"));
}

/// The middle row, ADR 0019 decision 2's own words: "A `queued` task that
/// carries a delivery attempt looks safe and is not, and the only reason
/// Factory can tell the difference is that §5 puts the journal write before
/// the terminal write." A `queued` task with a `delivery_attempts` row must
/// become `blocked: interrupted`, exactly like a `running` one — its status
/// is the *weaker* evidence.
#[test]
fn reconcile_blocks_a_queued_task_with_a_delivery_attempt_as_interrupted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope = restore_common::seed_scope(&mut store, 1, dir.path());
    let ws = dir.path().join("ws");
    let session = restore_common::seed_session(&mut store, 1, scope, &ws, "running");
    let task = restore_common::seed_task(&mut store, 1, scope, "queued", Some(session));
    restore_common::record_delivery_attempt(&mut store, task, session);

    reconcile(&mut store).expect("reconcile");

    let (status, blocked_reason, _) = restore_common::task_row(&store, task);
    assert_eq!(
        status, "blocked",
        "a queued task with a delivery attempt may already have been delivered"
    );
    assert_eq!(blocked_reason.as_deref(), Some("interrupted"));
}

/// The row beside the middle one: `queued` with **no** delivery attempt
/// means nothing was ever sent, so the task stays `queued` — only its
/// assignment is cleared, "letting selection choose again."
#[test]
fn reconcile_leaves_a_queued_task_with_no_attempt_queued_but_clears_its_assignment() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope = restore_common::seed_scope(&mut store, 1, dir.path());
    let ws = dir.path().join("ws");
    let session = restore_common::seed_session(&mut store, 1, scope, &ws, "starting");
    let task = restore_common::seed_task(&mut store, 1, scope, "queued", Some(session));

    reconcile(&mut store).expect("reconcile");

    let (status, blocked_reason, assigned) = restore_common::task_row(&store, task);
    assert_eq!(status, "queued", "nothing was ever sent for this task");
    assert_eq!(blocked_reason, None);
    assert_eq!(
        assigned, None,
        "the assignment must be cleared so selection can choose again"
    );
}

/// `blocked` (for any reason), `done`, `failed`, and `cancelled` are all
/// left completely untouched, including their `updated_at` — reconcile has
/// no business writing a row ADR 0019 decision 2 calls "unchanged."
#[test]
fn reconcile_leaves_already_settled_tasks_completely_untouched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope = restore_common::seed_scope(&mut store, 1, dir.path());

    let blocked = restore_common::seed_blocked_task(&mut store, 1, scope, "clarification");
    let done = restore_common::seed_task(&mut store, 2, scope, "done", None);
    let failed = restore_common::seed_task(&mut store, 3, scope, "failed", None);
    let cancelled = restore_common::seed_task(&mut store, 4, scope, "cancelled", None);

    for id in [blocked, done, failed, cancelled] {
        store
            .connection()
            .execute(
                "UPDATE tasks SET updated_at = '2000-01-01 00:00:00' WHERE id = ?1",
                [id.to_string()],
            )
            .expect("seed a distinctive old updated_at");
    }

    let report = reconcile(&mut store).expect("reconcile");

    assert!(
        report.tasks.is_empty(),
        "none of these rows should be reported as changed: {:?}",
        report.tasks
    );
    for id in [blocked, done, failed, cancelled] {
        assert_eq!(
            restore_common::task_updated_at(&store, id),
            "2000-01-01 00:00:00",
            "an already-settled task must not be rewritten at all"
        );
    }
    let (status, blocked_reason, _) = restore_common::task_row(&store, blocked);
    assert_eq!(status, "blocked");
    assert_eq!(blocked_reason.as_deref(), Some("clarification"));
}

/// Coordinator decision 4 / ADR 0019 decision 1: running `reconcile` twice
/// must change nothing the second time — the whole reason ADR 0019 tolerates
/// an operator copying a snapshot into place by hand with no other warning.
///
/// Distinctive, pre-restore-looking timestamps are seeded directly on every
/// row the first call touches, specifically so a mutation that re-stamps
/// `updated_at` unconditionally on the second (no-op) call is visible —
/// without this, "assert nothing changed" would pass by coincidence for a
/// buggy version whenever both calls land in the same wall-clock second.
#[test]
fn reconcile_is_idempotent_a_second_run_changes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope = restore_common::seed_scope(&mut store, 1, dir.path());

    let starting_ws = dir.path().join("ws-starting");
    let running_ws = dir.path().join("ws-running");
    let starting = restore_common::seed_session(&mut store, 101, scope, &starting_ws, "starting");
    let running = restore_common::seed_session(&mut store, 102, scope, &running_ws, "running");

    let running_session = running;
    let running_task =
        restore_common::seed_task(&mut store, 1, scope, "running", Some(running_session));
    let queued_with_attempt =
        restore_common::seed_task(&mut store, 2, scope, "queued", Some(starting));
    restore_common::record_delivery_attempt(&mut store, queued_with_attempt, starting);
    let queued_no_attempt =
        restore_common::seed_task(&mut store, 3, scope, "queued", Some(starting));

    let first = reconcile(&mut store).expect("first reconcile");
    assert_eq!(first.sessions.len(), 2);
    assert_eq!(first.tasks.len(), 3);

    // Seed distinctive, obviously-old values directly, bypassing
    // CURRENT_TIMESTAMP, on every row the first call touched.
    for id in [starting, running] {
        store
            .connection()
            .execute(
                "UPDATE sessions SET updated_at = '2000-01-01 00:00:00' WHERE id = ?1",
                [id.to_string()],
            )
            .expect("seed old updated_at");
    }
    for id in [running_task, queued_with_attempt, queued_no_attempt] {
        store
            .connection()
            .execute(
                "UPDATE tasks SET updated_at = '2000-01-01 00:00:00' WHERE id = ?1",
                [id.to_string()],
            )
            .expect("seed old updated_at");
    }

    let second = reconcile(&mut store).expect("second reconcile");
    assert!(
        second.sessions.is_empty(),
        "no session should be reported changed on the second run: {:?}",
        second.sessions
    );
    assert!(
        second.tasks.is_empty(),
        "no task should be reported changed on the second run: {:?}",
        second.tasks
    );

    for id in [starting, running] {
        assert_eq!(
            restore_common::session_updated_at(&store, id),
            "2000-01-01 00:00:00",
            "a no-op second reconcile must not touch updated_at"
        );
    }
    for id in [running_task, queued_with_attempt, queued_no_attempt] {
        assert_eq!(
            restore_common::task_updated_at(&store, id),
            "2000-01-01 00:00:00",
            "a no-op second reconcile must not touch updated_at"
        );
    }

    // Two calls, two report files — a run that changes nothing still writes
    // an audit record saying so (ADR 0019 decision 2's last paragraph).
    assert_ne!(first.report_path, second.report_path);
    assert!(first.report_path.exists());
    assert!(second.report_path.exists());
}

/// ADR 0019 decision 2's last paragraph: the audit record is a report file
/// under `.factory/`, naming the database's `user_version` and every change.
/// Design §4's allowlist ("No Factory command writes to a file outside a
/// `.factory/` directory") is checked directly against the path this crate
/// actually wrote to, not merely assumed from how it was constructed.
#[test]
fn reconcile_writes_a_report_under_dot_factory_naming_the_user_version() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    let scope = restore_common::seed_scope(&mut store, 1, dir.path());
    let ws = dir.path().join("ws");
    restore_common::seed_session(&mut store, 1, scope, &ws, "running");

    let user_version = store.schema_version().expect("schema_version");
    let report = reconcile(&mut store).expect("reconcile");

    assert_eq!(report.snapshot_user_version, user_version);

    let relative = report
        .report_path
        .strip_prefix(dir.path())
        .expect("report path must be under the company root");
    assert!(
        relative.starts_with(".factory"),
        "found a report path outside .factory/: {}",
        report.report_path.display()
    );

    let contents = std::fs::read_to_string(&report.report_path).expect("read report");
    assert!(contents.contains(&user_version.to_string()));
    assert!(contents.contains("sessions changed"));
    assert!(contents.contains("tasks changed"));
}

/// `Store::open_at` ("used by tests and by the backup drill," per its own
/// doc comment) accepts an arbitrary path with no `.factory/` in it at all —
/// unlike `Store::open`, which every other test in this file uses. `reconcile`
/// must still reconcile the database (the transaction already committed) but
/// must refuse to write a report file outside design §4's allowlist rather
/// than silently placing one beside the database wherever that happens to be.
#[test]
fn reconcile_refuses_to_write_a_report_when_the_database_is_not_under_dot_factory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("not-dot-factory.sqlite");
    let mut store = Store::open_at(&db_path).expect("open_at");
    let scope = restore_common::seed_scope(&mut store, 1, dir.path());
    let ws = dir.path().join("ws");
    let session = restore_common::seed_session(&mut store, 1, scope, &ws, "running");

    let err = reconcile(&mut store)
        .expect_err("a database outside .factory/ must not get a report file written beside it");
    assert!(matches!(
        err,
        factory_recovery::restore::RecoveryError::NotInDotFactory { .. }
    ));

    // The reconciliation itself still committed — only the audit record was
    // refused. See `reconcile`'s own doc comment: "checked, and therefore
    // returned, only after the transaction above has already committed."
    assert_eq!(
        restore_common::session_state(&store, session),
        "disconnected"
    );
    assert!(
        !dir.path().join("restores").exists(),
        "no report directory should have been created outside .factory/"
    );
}
