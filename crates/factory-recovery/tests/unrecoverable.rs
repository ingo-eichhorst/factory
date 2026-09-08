//! Backlog §9: "Ambiguous or unrecoverable running work becomes `blocked:
//! interrupted` with a recorded delivery history and clear human next
//! action." [`factory_recovery::reconnect::give_up_on_disconnected_session`]
//! is the single operation both restart classes reach for whenever they
//! cannot positively reconnect or safely recreate — these tests exercise it
//! directly, on the exact row shape ADR 0019's reconciliation leaves behind:
//! a `disconnected` session whose task was already rewritten to `blocked:
//! interrupted`, with a `delivery_attempts` row recording the one attempt
//! that made resending unsafe.

mod common;

use common::*;
use factory_recovery::reconnect::{GiveUpOutcome, give_up_on_disconnected_session};

/// Unlike every other test in this file, this one reaches the give-up path
/// through [`factory_recovery::reconnect::reconnect_after_supervisor_restart`]
/// rather than calling [`give_up_on_disconnected_session`] directly — so this
/// is the fixture that actually proves "ambiguous... becomes blocked:
/// interrupted" is *reached via*
/// `evidence::may_promote_from_disconnected` saying no, not merely that the
/// give-up mechanism works once told to run. The direct calls below cover the
/// mechanism; this covers the decision to invoke it.
#[test]
fn ambiguous_evidence_via_supervisor_restart_reaches_give_up_with_history_intact() {
    let (_db_dir, mut store) = open_store();
    let workspace = tempfile::tempdir().expect("tempdir");
    let scope = seed_scope(&mut store, 1, "scope", workspace.path());
    let session = seed_disconnected_session(
        &mut store,
        2,
        scope,
        "pi",
        workspace.path(),
        Some("wE:p1"),
        Some("11111111-1111-1111-1111-111111111111"),
    );
    let task = seed_task(
        &mut store,
        3,
        scope,
        "blocked",
        Some("interrupted"),
        Some(session),
    );
    seed_delivery_attempt(&mut store, task, session);

    let adapter = FakeAdapter::new().with_observation("wE:p1", degraded_observation("wE:p1"));

    factory_recovery::reconnect::reconnect_after_supervisor_restart(&mut store, &adapter)
        .expect("reconcile must succeed");

    assert_eq!(session_state(&store, session), "failed");
    assert!(every_lease_released(&store, session));
    let (status, blocked_reason) = task_status(&store, task);
    assert_eq!(status, "blocked");
    assert_eq!(blocked_reason.as_deref(), Some("interrupted"));
    assert_eq!(delivery_attempts_count(&store, task), 1);
}

#[test]
fn unrecoverable_session_fails_and_interrupts_its_non_terminal_task_while_keeping_delivery_history()
{
    let (_db_dir, mut store) = open_store();
    let workspace = tempfile::tempdir().expect("tempdir");
    let scope = seed_scope(&mut store, 1, "scope", workspace.path());
    let session = seed_disconnected_session(
        &mut store,
        2,
        scope,
        "pi",
        workspace.path(),
        Some("wE:p1"),
        Some("11111111-1111-1111-1111-111111111111"),
    );
    // ADR 0019's own starting state: a task already rewritten to `blocked:
    // interrupted`, still assigned to the session that was running it, with
    // the delivery attempt that made resending it unsafe.
    let task = seed_task(
        &mut store,
        3,
        scope,
        "blocked",
        Some("interrupted"),
        Some(session),
    );
    seed_delivery_attempt(&mut store, task, session);

    let outcome =
        give_up_on_disconnected_session(&mut store, session, "unrecoverable: no evidence")
            .expect("give up must succeed");

    assert_eq!(outcome, GiveUpOutcome::Interrupted { task_id: task });
    assert_eq!(session_state(&store, session), "failed");
    assert!(
        every_lease_released(&store, session),
        "the stale lease must be released — ADR 0012 decision 5's mandatory recovery action"
    );

    let (status, blocked_reason) = task_status(&store, task);
    assert_eq!(status, "blocked");
    assert_eq!(blocked_reason.as_deref(), Some("interrupted"));

    assert_eq!(
        delivery_attempts_count(&store, task),
        1,
        "giving up must preserve delivery history, never erase it"
    );
}

#[test]
fn unrecoverable_session_with_no_attached_task_just_fails() {
    let (_db_dir, mut store) = open_store();
    let workspace = tempfile::tempdir().expect("tempdir");
    let scope = seed_scope(&mut store, 1, "scope", workspace.path());
    let session = seed_disconnected_session(
        &mut store,
        2,
        scope,
        "pi",
        workspace.path(),
        Some("wE:p1"),
        None,
    );

    let outcome =
        give_up_on_disconnected_session(&mut store, session, "unrecoverable: no evidence")
            .expect("give up must succeed");

    assert_eq!(outcome, GiveUpOutcome::Failed);
    assert_eq!(session_state(&store, session), "failed");
    assert!(every_lease_released(&store, session));
}

/// A queued task with an already-journalled delivery attempt (ADR 0019
/// decision 2's middle row: "the attempt is journalled before the write, so
/// this task may already have been delivered") is exactly as unrecoverable
/// as a `running` one, and must be left exactly where ADR 0019 already put
/// it — `give_up_on_disconnected_session` only ever touches a *non-terminal*
/// task still assigned to the session it is failing, and a `queued` task
/// with an attempt is precisely that shape once assigned.
#[test]
fn queued_task_with_a_prior_delivery_attempt_is_also_interrupted() {
    let (_db_dir, mut store) = open_store();
    let workspace = tempfile::tempdir().expect("tempdir");
    let scope = seed_scope(&mut store, 1, "scope", workspace.path());
    let session = seed_disconnected_session(
        &mut store,
        2,
        scope,
        "pi",
        workspace.path(),
        Some("wE:p1"),
        None,
    );
    let task = seed_task(&mut store, 3, scope, "queued", None, Some(session));
    seed_delivery_attempt(&mut store, task, session);

    let outcome =
        give_up_on_disconnected_session(&mut store, session, "unrecoverable: no evidence")
            .expect("give up must succeed");

    assert_eq!(outcome, GiveUpOutcome::Interrupted { task_id: task });
    let (status, blocked_reason) = task_status(&store, task);
    assert_eq!(status, "blocked");
    assert_eq!(blocked_reason.as_deref(), Some("interrupted"));
    assert_eq!(delivery_attempts_count(&store, task), 1);
}
