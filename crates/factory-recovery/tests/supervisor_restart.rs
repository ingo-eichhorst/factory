//! Backlog §9: "After a supervisor restart, known panes are reconnected
//! where possible and no task prompt is silently sent a second time."
//!
//! Every test starts from ADR 0019's post-reconciliation shape, seeded
//! directly per `tests/common/mod.rs`'s own doc comment: a `disconnected`
//! session with its lease held.

mod common;

use common::*;

#[test]
fn authoritative_matching_observation_promotes_to_running() {
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

    let adapter = FakeAdapter::new().with_observation(
        "wE:p1",
        authoritative_observation("wE:p1", Some("11111111-1111-1111-1111-111111111111")),
    );

    let records =
        factory_recovery::reconnect::reconnect_after_supervisor_restart(&mut store, &adapter)
            .expect("reconcile must succeed");

    assert_eq!(session_state(&store, session), "running");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].session_id, session);
    assert_eq!(
        records[0].outcome,
        factory_recovery::reconnect::Outcome::Reconnected
    );
}

/// Coordinator handover on migration 5: this crate is "the only place a
/// live observation is permitted to move a session at all," so it is the
/// only honest writer of `sessions.harness_session_id`. A session
/// authoritatively reconnected for the first time — no id recorded yet, the
/// observation supplies one — must have that id recorded, so the *next*
/// reconnection has a real identity to compare instead of
/// `identifies_same_session`'s "nothing recorded" fallback.
#[test]
fn first_authoritative_reconnection_records_the_observed_harness_session_id() {
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
    assert_eq!(
        session_harness_session_id(&store, session),
        None,
        "test setup: nothing recorded yet"
    );

    let adapter = FakeAdapter::new().with_observation(
        "wE:p1",
        authoritative_observation("wE:p1", Some("11111111-1111-1111-1111-111111111111")),
    );

    factory_recovery::reconnect::reconnect_after_supervisor_restart(&mut store, &adapter)
        .expect("reconcile must succeed");

    assert_eq!(session_state(&store, session), "running");
    assert_eq!(
        session_harness_session_id(&store, session).as_deref(),
        Some("11111111-1111-1111-1111-111111111111"),
        "the confirming observation's identity must be recorded for the next reconnection"
    );
}

/// The other half of "never on a weaker one": a session that already has a
/// recorded identity keeps it exactly as it was — this function only ever
/// fills a `NULL`, so a later, different-but-still-matching observation (an
/// unlikely but not impossible harness quirk) cannot silently rewrite
/// history.
#[test]
fn an_already_recorded_identity_is_never_overwritten() {
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

    let adapter = FakeAdapter::new().with_observation(
        "wE:p1",
        authoritative_observation("wE:p1", Some("11111111-1111-1111-1111-111111111111")),
    );

    factory_recovery::reconnect::reconnect_after_supervisor_restart(&mut store, &adapter)
        .expect("reconcile must succeed");

    assert_eq!(
        session_harness_session_id(&store, session).as_deref(),
        Some("11111111-1111-1111-1111-111111111111")
    );
}

#[test]
fn degraded_observation_gives_up_rather_than_promoting() {
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

    let adapter = FakeAdapter::new().with_observation("wE:p1", degraded_observation("wE:p1"));

    factory_recovery::reconnect::reconnect_after_supervisor_restart(&mut store, &adapter)
        .expect("reconcile must succeed");

    assert_eq!(session_state(&store, session), "failed");
    assert!(every_lease_released(&store, session));
}

#[test]
fn unavailable_observation_gives_up_rather_than_promoting() {
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

    let unavailable = factory_adapter::Observation {
        pane: factory_adapter::PaneId("wE:p1".to_string()),
        harness_state: String::new(),
        confidence: factory_adapter::Confidence::Unavailable,
        session_alive: false,
        task_signal: factory_adapter::TaskSignal::NoChange,
        transcript_path: None,
        harness_session_id: None,
    };
    let adapter = FakeAdapter::new().with_observation("wE:p1", unavailable);

    factory_recovery::reconnect::reconnect_after_supervisor_restart(&mut store, &adapter)
        .expect("reconcile must succeed");

    assert_eq!(session_state(&store, session), "failed");
    assert!(every_lease_released(&store, session));
}

/// The identity-correlation half, kept distinct from confidence: an
/// authoritative reading whose harness session id disagrees with the one
/// recorded for this session must not promote it — ADR 0019 decision 3's
/// pane-reuse hazard, made concrete.
#[test]
fn authoritative_but_mismatched_harness_session_id_gives_up() {
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

    // A different pi process now occupies the same pane, reported with full
    // hook authority — Herdr is not lying, but it is telling the truth about
    // someone else.
    let adapter = FakeAdapter::new().with_observation(
        "wE:p1",
        authoritative_observation("wE:p1", Some("22222222-2222-2222-2222-222222222222")),
    );

    factory_recovery::reconnect::reconnect_after_supervisor_restart(&mut store, &adapter)
        .expect("reconcile must succeed");

    assert_eq!(
        session_state(&store, session),
        "failed",
        "a pane reused by a different session must never promote the old one"
    );
    assert!(every_lease_released(&store, session));
}

#[test]
fn no_recorded_pane_is_left_untouched() {
    let (_db_dir, mut store) = open_store();
    let workspace = tempfile::tempdir().expect("tempdir");
    let scope = seed_scope(&mut store, 1, "scope", workspace.path());
    let session =
        seed_disconnected_session(&mut store, 2, scope, "pi", workspace.path(), None, None);

    let adapter = FakeAdapter::new();

    let records =
        factory_recovery::reconnect::reconnect_after_supervisor_restart(&mut store, &adapter)
            .expect("reconcile must succeed");

    assert_eq!(session_state(&store, session), "disconnected");
    assert!(
        records.is_empty(),
        "a session with no recorded pane is this class's no-op, not a decision"
    );
}

/// One bad row must not hide the decision for every other session in the
/// same pass — mirrors `factory_session::find_aliasing_conflict`'s own
/// stale-row `continue` reasoning.
#[test]
fn adapter_error_on_one_session_does_not_block_another() {
    let (_db_dir, mut store) = open_store();
    let workspace = tempfile::tempdir().expect("tempdir");
    let scope = seed_scope(&mut store, 1, "scope", workspace.path());
    let broken = seed_disconnected_session(
        &mut store,
        2,
        scope,
        "pi",
        workspace.path(),
        Some("wBroken:p1"),
        None,
    );
    let healthy_workspace = tempfile::tempdir().expect("tempdir");
    let healthy = seed_disconnected_session(
        &mut store,
        3,
        scope,
        "pi",
        healthy_workspace.path(),
        Some("wHealthy:p1"),
        Some("11111111-1111-1111-1111-111111111111"),
    );

    let adapter = FakeAdapter::new()
        .with_error("wBroken:p1", "wrong harness on this pane")
        .with_observation(
            "wHealthy:p1",
            authoritative_observation("wHealthy:p1", Some("11111111-1111-1111-1111-111111111111")),
        );

    let records =
        factory_recovery::reconnect::reconnect_after_supervisor_restart(&mut store, &adapter)
            .expect("reconcile must succeed even when one adapter call fails");

    assert_eq!(
        session_state(&store, broken),
        "disconnected",
        "an adapter failure must not be treated as evidence either way"
    );
    assert_eq!(session_state(&store, healthy), "running");

    let broken_record = records
        .iter()
        .find(|r| r.session_id == broken)
        .expect("broken session must still get a record");
    assert!(matches!(
        broken_record.outcome,
        factory_recovery::reconnect::Outcome::SkippedAdapterError { .. }
    ));
}

/// The whole point of this class's acceptance criterion: reconnecting a
/// session must never cause a delivery. A queued task with no delivery
/// attempts at all is the case where a delivery would actually be possible —
/// `factory_task::deliver::deliver` only refuses a task that already carries
/// one — so this is the fixture that would actually notice a regression.
#[test]
fn reconnecting_never_delivers_a_prompt() {
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
    let unrelated_task = seed_task(&mut store, 3, scope, "queued", None, None);
    // Assigned directly to the session being reconnected, with zero delivery
    // attempts: `factory_task::deliver::deliver` only refuses a task that
    // already carries one, so this is the one shape a delivery would
    // genuinely go through on if a reconnection path called it.
    let deliverable_task = seed_task(&mut store, 4, scope, "queued", None, Some(session));

    let adapter = FakeAdapter::new().with_observation(
        "wE:p1",
        authoritative_observation("wE:p1", Some("11111111-1111-1111-1111-111111111111")),
    );

    factory_recovery::reconnect::reconnect_after_supervisor_restart(&mut store, &adapter)
        .expect("reconcile must succeed");

    assert_eq!(session_state(&store, session), "running");
    let (status, _) = task_status(&store, unrelated_task);
    assert_eq!(status, "queued");
    assert_eq!(delivery_attempts_count(&store, unrelated_task), 0);

    let (deliverable_status, _) = task_status(&store, deliverable_task);
    assert_eq!(deliverable_status, "queued");
    assert_eq!(delivery_attempts_count(&store, deliverable_task), 0);

    // Prove the fixture is not vacuously "undeliverable" for some unrelated
    // reason (a bad status, a missing assignment): `deliver` must actually
    // succeed on it. Without this, the zero-delivery-attempts assertion above
    // would pass just as well if `deliverable_task` could never have been
    // delivered at all, which would prove nothing about whether reconnection
    // itself avoids calling delivery.
    let mut writer = factory_task::deliver::OperatorPromptWriter::new(Vec::new());
    factory_task::deliver::deliver(&mut store, deliverable_task, &mut writer).expect(
        "the fixture task must be genuinely deliverable, or the assertions above are vacuous",
    );
}

/// Coordinator decision 4: running the reconciliation twice must change
/// nothing the second time.
#[test]
fn rerunning_after_promotion_is_a_no_op() {
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
    let adapter = FakeAdapter::new().with_observation(
        "wE:p1",
        authoritative_observation("wE:p1", Some("11111111-1111-1111-1111-111111111111")),
    );

    factory_recovery::reconnect::reconnect_after_supervisor_restart(&mut store, &adapter)
        .expect("first run");
    assert_eq!(session_state(&store, session), "running");

    let second =
        factory_recovery::reconnect::reconnect_after_supervisor_restart(&mut store, &adapter)
            .expect("second run");
    assert!(
        second.is_empty(),
        "a session no longer `disconnected` must not be reconsidered"
    );
    assert_eq!(session_state(&store, session), "running");
}
