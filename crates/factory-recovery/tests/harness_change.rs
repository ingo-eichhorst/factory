//! Backlog §9's "Changing a harness stops old sessions, retains queued
//! tasks, and requires review of running work" and design §5's "Harness
//! change: stop the old session; queued tasks remain; running task requires
//! review" — one test per row of
//! `factory_recovery::harness::harness_changed`'s evidence-to-outcome table
//! (see that module's doc comment for the table and the reasoning behind
//! each row).

mod common;
mod harness_common;

use common::*;
use factory_adapter::{Confidence, Observation, PaneId, TaskSignal};
use factory_recovery::harness::{HarnessChangeOutcome, harness_changed};

fn observation(pane: &str, confidence: Confidence, session_alive: bool) -> Observation {
    Observation {
        pane: PaneId(pane.to_string()),
        harness_state: "gone".to_string(),
        confidence,
        session_alive,
        task_signal: TaskSignal::NoChange,
        transcript_path: None,
        harness_session_id: None,
    }
}

/// The load-bearing clause: even the strongest possible evidence that the
/// old process is gone must not release the lease of a session holding
/// running work. Only a human, after reviewing the task, gets to decide
/// that — see ADR 0012 decision 5 and the module docs' "Running work
/// short-circuits evidence entirely."
#[test]
fn a_running_task_requires_review_and_keeps_the_lease_regardless_of_evidence() {
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
    let adapter = FakeAdapter::new().with_observation(
        "wE:p1",
        observation("wE:p1", Confidence::Authoritative, false),
    );

    let records =
        harness_changed(&mut store, scope, "pi", &adapter).expect("harness_changed must succeed");

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].session_id, session);
    assert_eq!(
        records[0].outcome,
        HarnessChangeOutcome::RequiresReview { task_id: task }
    );
    assert_eq!(session_state(&store, session), "disconnected");
    assert!(
        !every_lease_released(&store, session),
        "a session that merely needs review does not release its lease — ADR 0012 decision 5"
    );
    let (status, blocked_reason) = task_status(&store, task);
    assert_eq!(status, "blocked");
    assert_eq!(blocked_reason.as_deref(), Some("interrupted"));
}

/// ADR 0019 decision 2's precise reading of "queued tasks remain queued":
/// nothing was ever sent for this task (no `delivery_attempts` row at all),
/// which is exactly what makes it safe to leave alone — contrast
/// `a_queued_task_with_a_prior_delivery_attempt_requires_review_like_running_work`
/// below, where the presence of that one row is what makes the identical
/// `queued` status *not* safe.
#[test]
fn a_queued_task_with_no_delivery_attempt_stays_queued_because_nothing_was_sent() {
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
    let task = seed_task(&mut store, 3, scope, "queued", None, Some(session));
    let adapter = FakeAdapter::new().with_observation(
        "wE:p1",
        observation("wE:p1", Confidence::Authoritative, false),
    );

    harness_changed(&mut store, scope, "pi", &adapter).expect("harness_changed must succeed");

    let (status, blocked_reason) = task_status(&store, task);
    assert_eq!(status, "queued");
    assert_eq!(blocked_reason, None);
    assert_eq!(delivery_attempts_count(&store, task), 0);
}

/// ADR 0019 decision 2: "[a] `queued` task that carries a delivery attempt
/// looks safe and is not, and the only reason Factory can tell the
/// difference is that design §5 puts the journal write before the terminal
/// write." That decision's restore table moves exactly this shape of task
/// to `blocked: interrupted`; `harness_changed` now agrees, treating it as
/// review-worthy work in flight exactly like a `running` task — including
/// keeping the session's lease, never releasing it (the coordinator's own
/// framing: "a released lease plus an unreviewed possibly-delivered
/// prompt" is the hazard this guards against).
#[test]
fn a_queued_task_with_a_prior_delivery_attempt_requires_review_like_running_work() {
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
    let task = seed_task(&mut store, 3, scope, "queued", None, Some(session));
    seed_delivery_attempt(&mut store, task, session);
    // The same confirmed-dead evidence that retires an idle session cleanly
    // must not be enough here — the task's own delivery history, not the
    // pane, is what decides this row.
    let adapter = FakeAdapter::new().with_observation(
        "wE:p1",
        observation("wE:p1", Confidence::Authoritative, false),
    );

    let records =
        harness_changed(&mut store, scope, "pi", &adapter).expect("harness_changed must succeed");

    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].outcome,
        HarnessChangeOutcome::RequiresReview { task_id: task }
    );
    assert_eq!(session_state(&store, session), "disconnected");
    assert!(
        !every_lease_released(&store, session),
        "a released lease plus an unreviewed possibly-delivered prompt is exactly \
         the hazard ADR 0012 decision 5's lease exists to prevent"
    );
    let (status, blocked_reason) = task_status(&store, task);
    assert_eq!(status, "blocked");
    assert_eq!(blocked_reason.as_deref(), Some("interrupted"));
    assert_eq!(
        delivery_attempts_count(&store, task),
        1,
        "flagging the task for review must preserve its delivery history, never erase it"
    );
}

#[test]
fn no_running_task_and_confirmed_dead_evidence_stops_the_session_cleanly() {
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
    let adapter = FakeAdapter::new().with_observation(
        "wE:p1",
        observation("wE:p1", Confidence::Authoritative, false),
    );

    let records =
        harness_changed(&mut store, scope, "pi", &adapter).expect("harness_changed must succeed");

    assert_eq!(records[0].outcome, HarnessChangeOutcome::Retired);
    assert_eq!(session_state(&store, session), "stopped");
    assert!(every_lease_released(&store, session));
}

#[test]
fn no_running_task_and_inconclusive_evidence_keeps_the_lease() {
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
    let adapter = FakeAdapter::new()
        .with_observation("wE:p1", observation("wE:p1", Confidence::Degraded, false));

    let records =
        harness_changed(&mut store, scope, "pi", &adapter).expect("harness_changed must succeed");

    assert_eq!(records[0].outcome, HarnessChangeOutcome::Inconclusive);
    assert_eq!(session_state(&store, session), "disconnected");
    assert!(!every_lease_released(&store, session));
}

#[test]
fn no_running_task_and_still_alive_evidence_keeps_the_lease_too() {
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
    let adapter = FakeAdapter::new().with_observation(
        "wE:p1",
        observation("wE:p1", Confidence::Authoritative, true),
    );

    let records =
        harness_changed(&mut store, scope, "pi", &adapter).expect("harness_changed must succeed");

    assert_eq!(records[0].outcome, HarnessChangeOutcome::Inconclusive);
    assert_eq!(session_state(&store, session), "disconnected");
}

#[test]
fn a_session_with_no_recorded_pane_is_inconclusive_not_stopped() {
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
        None,
    );
    let adapter = FakeAdapter::new();

    let records =
        harness_changed(&mut store, scope, "pi", &adapter).expect("harness_changed must succeed");

    assert_eq!(records[0].outcome, HarnessChangeOutcome::Inconclusive);
    assert_eq!(session_state(&store, session), "disconnected");
    assert!(!every_lease_released(&store, session));
}

/// A launch that never reached a confirmed `running` has nothing
/// legitimately "stopped" about it — `factory_session::valid_targets`'s own
/// reasoning for why `Starting → Failed` exists and `Starting → Stopped`
/// does not.
#[test]
fn a_session_that_never_confirmed_running_fails_rather_than_stops() {
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
        "starting",
        None,
    );
    let adapter = FakeAdapter::new();

    let records =
        harness_changed(&mut store, scope, "pi", &adapter).expect("harness_changed must succeed");

    assert_eq!(records[0].outcome, HarnessChangeOutcome::Retired);
    assert_eq!(session_state(&store, session), "failed");
    assert!(every_lease_released(&store, session));
}

#[test]
fn only_sessions_of_the_named_agent_are_affected() {
    let (_db_dir, mut store) = open_store();
    let workspace_a = tempfile::tempdir().expect("tempdir");
    let workspace_b = tempfile::tempdir().expect("tempdir");
    let scope = seed_scope(&mut store, 1, "scope", workspace_a.path());
    let pi_session = uid(2);
    harness_common::seed_session(
        &mut store,
        pi_session,
        scope,
        "pi",
        workspace_a.path(),
        "running",
        None,
    );
    let other_session = uid(3);
    harness_common::seed_session(
        &mut store,
        other_session,
        scope,
        "claude-code",
        workspace_b.path(),
        "running",
        None,
    );
    let adapter = FakeAdapter::new();

    let records =
        harness_changed(&mut store, scope, "pi", &adapter).expect("harness_changed must succeed");

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].session_id, pi_session);
    assert_eq!(
        session_state(&store, other_session),
        "running",
        "a different agent's session must be untouched"
    );
}

/// Coordinator decision 4's idempotence rule, applied to `harness_changed`:
/// a `disconnected` session `harness_changed` itself just flagged for review
/// still matches its own `old_sessions_of` query on a second call (unlike
/// `crate::reconnect`'s restart classes, `disconnected` is an *input* state
/// this function accepts, not only an output it produces). The task is no
/// longer `running` by then — it is `blocked: interrupted` — so a second
/// call must recognise that shape and leave the session exactly where it
/// is, never falling through to the no-running-task branch and asking
/// `adapter` about the pane, which — given the same confirmed-dead evidence
/// an operator would still have on hand — would otherwise retire the
/// session (`stop()`, releasing the lease) before any human has reviewed
/// the task this function itself put on hold.
#[test]
fn calling_it_twice_does_not_retire_a_session_still_awaiting_review() {
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
    // The adapter would confirm this pane dead if asked — proving the
    // second call's protection comes from recognising the already-flagged
    // task, not from a lack of evidence to retire on.
    let adapter = FakeAdapter::new().with_observation(
        "wE:p1",
        observation("wE:p1", Confidence::Authoritative, false),
    );

    let first =
        harness_changed(&mut store, scope, "pi", &adapter).expect("first call must succeed");
    assert_eq!(
        first[0].outcome,
        HarnessChangeOutcome::RequiresReview { task_id: task }
    );

    let second =
        harness_changed(&mut store, scope, "pi", &adapter).expect("second call must succeed");

    assert_eq!(second.len(), 1);
    assert_eq!(
        second[0].outcome,
        HarnessChangeOutcome::RequiresReview { task_id: task }
    );
    assert_eq!(session_state(&store, session), "disconnected");
    assert!(
        !every_lease_released(&store, session),
        "a second call must not retire a session whose review has not landed"
    );
    let (status, blocked_reason) = task_status(&store, task);
    assert_eq!(status, "blocked");
    assert_eq!(blocked_reason.as_deref(), Some("interrupted"));
}

/// The same row, with a *refused* attempt: not review-worthy, because
/// nothing was sent.
///
/// `a_queued_task_with_a_prior_delivery_attempt_requires_review_like_running_work`
/// turns on the attempt being evidence that a prompt may already have
/// arrived. A refusal is evidence of the opposite — the writer established
/// that it wrote nothing — so this task is an ordinary queued task and its
/// session retires cleanly, lease released.
///
/// One home for that rule:
/// `factory_task::deliver::ATTEMPT_MAY_HAVE_REACHED_THE_TERMINAL`. Neuter it
/// and this test fails alongside `factory-task`'s and `restore.rs`'s.
#[test]
fn a_queued_task_whose_only_attempt_was_refused_stays_queued_because_nothing_was_sent() {
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
    let task = seed_task(&mut store, 3, scope, "queued", None, Some(session));
    seed_refused_delivery_attempt(&mut store, task, session);
    let adapter = FakeAdapter::new().with_observation(
        "wE:p1",
        observation("wE:p1", Confidence::Authoritative, false),
    );

    let records =
        harness_changed(&mut store, scope, "pi", &adapter).expect("harness_changed must succeed");

    assert_eq!(records.len(), 1);
    assert_ne!(
        records[0].outcome,
        HarnessChangeOutcome::RequiresReview { task_id: task },
        "a refused attempt is not work in flight"
    );
    let (status, blocked_reason) = task_status(&store, task);
    assert_eq!(status, "queued", "queued tasks remain queued");
    assert_eq!(blocked_reason, None);
    assert_eq!(
        delivery_attempts_count(&store, task),
        1,
        "the refusal is still preserved in the delivery history"
    );
}
