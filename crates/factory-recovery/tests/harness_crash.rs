//! Backlog §9's "A harness crash marks its session failed and its active
//! task blocked or failed according to documented evidence" and design §5's
//! "Harness crash: mark session failed and current task blocked or failed"
//! — one test per row of `factory_recovery::harness::harness_crashed`'s
//! evidence-to-outcome table (see that module's doc comment for the table
//! and the reasoning behind each row).

mod common;
mod harness_common;

use common::*;
use factory_adapter::{Confidence, Observation, PaneId, TaskSignal};
use factory_recovery::harness::{CrashOutcome, harness_crashed};
use factory_recovery::reconnect::GiveUpOutcomeSummary;

fn observation(confidence: Confidence, session_alive: bool) -> Observation {
    Observation {
        pane: PaneId("wE:p1".to_string()),
        harness_state: "gone".to_string(),
        confidence,
        session_alive,
        task_signal: TaskSignal::NoChange,
        transcript_path: None,
        harness_session_id: None,
    }
}

#[test]
fn authoritative_confirmed_crash_fails_session_and_interrupts_its_running_task() {
    let (_db_dir, mut store) = open_store();
    let workspace = tempfile::tempdir().expect("tempdir");
    let scope = seed_scope(&mut store, 1, "scope", workspace.path());
    let session = uid(2);
    harness_common::seed_session(
        &mut store,
        session,
        scope,
        "pi",
        workspace.path(),
        "running",
        Some("wE:p1"),
    );
    let task = seed_task(&mut store, 3, scope, "running", None, Some(session));

    let outcome = harness_crashed(
        &mut store,
        session,
        &observation(Confidence::Authoritative, false),
    )
    .expect("harness_crashed must succeed");

    assert_eq!(
        outcome,
        CrashOutcome::Confirmed(GiveUpOutcomeSummary::Interrupted { task_id: task })
    );
    assert_eq!(session_state(&store, session), "failed");
    assert!(every_lease_released(&store, session));
    let (status, blocked_reason) = task_status(&store, task);
    assert_eq!(status, "blocked");
    assert_eq!(blocked_reason.as_deref(), Some("interrupted"));
}

/// Design §5's "or failed": a task that already recorded its own terminal
/// result before the crash was even observed must not be overwritten to
/// `blocked: interrupted` — `factory_session::interrupt`'s own guard,
/// exercised here through the crash path rather than called directly.
#[test]
fn authoritative_confirmed_crash_leaves_an_already_terminal_task_alone() {
    let (_db_dir, mut store) = open_store();
    let workspace = tempfile::tempdir().expect("tempdir");
    let scope = seed_scope(&mut store, 1, "scope", workspace.path());
    let session = uid(2);
    harness_common::seed_session(
        &mut store,
        session,
        scope,
        "pi",
        workspace.path(),
        "running",
        Some("wE:p1"),
    );
    let task = seed_task(&mut store, 3, scope, "failed", None, Some(session));

    let outcome = harness_crashed(
        &mut store,
        session,
        &observation(Confidence::Authoritative, false),
    )
    .expect("harness_crashed must succeed");

    assert_eq!(
        outcome,
        CrashOutcome::Confirmed(GiveUpOutcomeSummary::Failed)
    );
    assert_eq!(session_state(&store, session), "failed");
    assert!(every_lease_released(&store, session));
    let (status, _) = task_status(&store, task);
    assert_eq!(
        status, "failed",
        "an already-terminal task must never be overwritten by a crash observed afterward"
    );
}

/// The strongest possible evidence — Authoritative — saying the session is
/// in fact alive must contradict the crash hypothesis outright, not be
/// treated as "confirmed" merely because it came through the trusted
/// channel.
#[test]
fn authoritative_but_alive_is_not_confirmed_a_crash() {
    let (_db_dir, mut store) = open_store();
    let workspace = tempfile::tempdir().expect("tempdir");
    let scope = seed_scope(&mut store, 1, "scope", workspace.path());
    let session = uid(2);
    harness_common::seed_session(
        &mut store,
        session,
        scope,
        "pi",
        workspace.path(),
        "running",
        Some("wE:p1"),
    );
    let task = seed_task(&mut store, 3, scope, "running", None, Some(session));

    let outcome = harness_crashed(
        &mut store,
        session,
        &observation(Confidence::Authoritative, true),
    )
    .expect("harness_crashed must succeed");

    assert_eq!(outcome, CrashOutcome::NotConfirmed);
    assert_eq!(session_state(&store, session), "running");
    assert!(!every_lease_released(&store, session));
    let (status, _) = task_status(&store, task);
    assert_eq!(status, "running");
}

#[test]
fn degraded_evidence_is_inconclusive_and_keeps_the_lease() {
    let (_db_dir, mut store) = open_store();
    let workspace = tempfile::tempdir().expect("tempdir");
    let scope = seed_scope(&mut store, 1, "scope", workspace.path());
    let session = uid(2);
    harness_common::seed_session(
        &mut store,
        session,
        scope,
        "pi",
        workspace.path(),
        "running",
        Some("wE:p1"),
    );
    let task = seed_task(&mut store, 3, scope, "running", None, Some(session));

    let outcome = harness_crashed(
        &mut store,
        session,
        &observation(Confidence::Degraded, false),
    )
    .expect("harness_crashed must succeed");

    assert_eq!(outcome, CrashOutcome::Inconclusive);
    assert_eq!(session_state(&store, session), "disconnected");
    assert!(
        !every_lease_released(&store, session),
        "disconnected still holds its lease — ADR 0012 decision 5"
    );
    let (status, _) = task_status(&store, task);
    assert_eq!(
        status, "running",
        "unconfirmed evidence must not touch the task"
    );
}

#[test]
fn unavailable_evidence_is_also_inconclusive() {
    let (_db_dir, mut store) = open_store();
    let workspace = tempfile::tempdir().expect("tempdir");
    let scope = seed_scope(&mut store, 1, "scope", workspace.path());
    let session = uid(2);
    harness_common::seed_session(
        &mut store,
        session,
        scope,
        "pi",
        workspace.path(),
        "running",
        Some("wE:p1"),
    );

    let outcome = harness_crashed(
        &mut store,
        session,
        &observation(Confidence::Unavailable, false),
    )
    .expect("harness_crashed must succeed");

    assert_eq!(outcome, CrashOutcome::Inconclusive);
    assert_eq!(session_state(&store, session), "disconnected");
    assert!(!every_lease_released(&store, session));
}

/// `harness_crashed` is deliberately **not** idempotent (see its own doc
/// comment's "Not idempotent, unlike `harness_changed`"): a second call for
/// the same one-time crash notification is a caller bug, and every non-error
/// outcome leaves `session_id` somewhere `factory_session::valid_targets`
/// grants no edge back out of for the write this function would attempt
/// next. This pins that as an explicit, tested contract rather than an
/// unspecified panic risk.
#[test]
fn a_second_crash_notification_for_the_same_session_is_a_caller_error() {
    let (_db_dir, mut store) = open_store();
    let workspace = tempfile::tempdir().expect("tempdir");
    let scope = seed_scope(&mut store, 1, "scope", workspace.path());
    let session = uid(2);
    harness_common::seed_session(
        &mut store,
        session,
        scope,
        "pi",
        workspace.path(),
        "running",
        Some("wE:p1"),
    );

    harness_crashed(
        &mut store,
        session,
        &observation(Confidence::Authoritative, false),
    )
    .expect("first call must succeed");
    assert_eq!(session_state(&store, session), "failed");

    let second = harness_crashed(
        &mut store,
        session,
        &observation(Confidence::Authoritative, false),
    );
    assert!(
        second.is_err(),
        "a second crash notification for an already-failed session must not silently succeed"
    );
}
