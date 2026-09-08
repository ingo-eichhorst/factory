//! Backlog §9: "After Herdr or machine restart, sessions are recreated only
//! in recorded, existing workspaces; queued tasks remain queued." Design §5
//! splits this into two rows this crate's function merges — "Herdr restart:
//! recreate sessions in their recorded workspaces" and "Machine restart:
//! recreate sessions in existing workspaces; queued tasks remain queued."

mod common;

use common::*;

/// A caller supplying fresh ids for recreated sessions, deterministic per
/// test so assertions can name the exact id produced — mirrors
/// `factory_task::create::create`'s own "the caller mints ids" contract.
fn id_source(start: u32) -> impl FnMut() -> uuid::Uuid {
    let mut next = start;
    move || {
        let id = uid(next);
        next += 1;
        id
    }
}

#[test]
fn live_evidence_of_the_same_session_blocks_recreation_and_promotes_instead() {
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
    let before = session_count(&store);

    let adapter = FakeAdapter::new().with_observation(
        "wE:p1",
        authoritative_observation("wE:p1", Some("11111111-1111-1111-1111-111111111111")),
    );

    let records = factory_recovery::reconnect::reconnect_after_herdr_or_machine_restart(
        &mut store,
        &adapter,
        4,
        id_source(100),
    )
    .expect("reconcile must succeed");

    assert_eq!(session_state(&store, session), "running");
    assert_eq!(
        session_count(&store),
        before,
        "confirmed-alive evidence must block recreation, not merely prefer against it"
    );
    assert_eq!(
        records[0].outcome,
        factory_recovery::reconnect::Outcome::Reconnected
    );
}

#[test]
fn existing_workspace_with_no_live_evidence_is_recreated() {
    let (_db_dir, mut store) = open_store();
    let workspace = tempfile::tempdir().expect("tempdir");
    let scope = seed_scope(&mut store, 1, "scope", workspace.path());
    let old_session =
        seed_disconnected_session(&mut store, 2, scope, "pi", workspace.path(), None, None);

    let adapter = FakeAdapter::new();

    let records = factory_recovery::reconnect::reconnect_after_herdr_or_machine_restart(
        &mut store,
        &adapter,
        4,
        id_source(100),
    )
    .expect("reconcile must succeed");

    assert_eq!(session_state(&store, old_session), "failed");
    assert!(every_lease_released(&store, old_session));

    let new_session = uid(100);
    assert_eq!(session_state(&store, new_session), "starting");
    assert_eq!(
        records[0].outcome,
        factory_recovery::reconnect::Outcome::Recreated {
            new_session_id: new_session,
            outcome: factory_recovery::reconnect::GiveUpOutcomeSummary::Failed,
        }
    );
}

/// The pre-check half of this function's own contract: a recorded pane that
/// answers, but not authoritatively, must still recreate rather than
/// promote. This is the fixture that actually exercises
/// `evidence::may_promote_from_disconnected` on the recreation path — unlike
/// the two tests above, whose sessions carry no recorded pane at all and so
/// never call `Adapter::observe` in the first place.
#[test]
fn pane_recorded_but_non_authoritative_observation_still_recreates() {
    let (_db_dir, mut store) = open_store();
    let workspace = tempfile::tempdir().expect("tempdir");
    let scope = seed_scope(&mut store, 1, "scope", workspace.path());
    let old_session = seed_disconnected_session(
        &mut store,
        2,
        scope,
        "pi",
        workspace.path(),
        Some("wE:p1"),
        None,
    );

    let adapter = FakeAdapter::new().with_observation("wE:p1", degraded_observation("wE:p1"));

    factory_recovery::reconnect::reconnect_after_herdr_or_machine_restart(
        &mut store,
        &adapter,
        4,
        id_source(100),
    )
    .expect("reconcile must succeed");

    assert_eq!(session_state(&store, old_session), "failed");
    assert_eq!(
        session_state(&store, uid(100)),
        "starting",
        "non-authoritative evidence must not block recreation the way authoritative evidence does"
    );
}

/// The identity half of this function's own escape hatch, isolated from
/// confidence: an authoritative reading of a pane now occupied by a
/// *different* harness session must not be read as "still reachable" just
/// because the pane id matched and the answer was authoritative. Unlike
/// `pane_recorded_but_non_authoritative_observation_still_recreates` above
/// (which varies confidence and holds identity fixed by never recording one),
/// this fixture holds confidence at `Authoritative` and varies identity — the
/// scenario `reconnect_after_supervisor_restart`'s own
/// `authoritative_but_mismatched_harness_session_id_gives_up` already covers
/// for the supervisor-restart class; this is the same case for the
/// Herdr-or-machine-restart class, which had no test reaching it before this
/// one (see the task report's mutation 2 finding).
#[test]
fn authoritative_but_reused_pane_recreates_rather_than_promotes() {
    let (_db_dir, mut store) = open_store();
    let workspace = tempfile::tempdir().expect("tempdir");
    let scope = seed_scope(&mut store, 1, "scope", workspace.path());
    let old_session = seed_disconnected_session(
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

    let records = factory_recovery::reconnect::reconnect_after_herdr_or_machine_restart(
        &mut store,
        &adapter,
        4,
        id_source(100),
    )
    .expect("reconcile must succeed");

    assert_eq!(
        session_state(&store, old_session),
        "failed",
        "a pane reused by a different session must never promote the old one, \
         even on this restart class's presumption that the session is gone"
    );
    let new_session = uid(100);
    assert_eq!(
        session_state(&store, new_session),
        "starting",
        "identity mismatch must fall through to recreation, exactly like no \
         evidence at all"
    );
    assert_eq!(
        records[0].outcome,
        factory_recovery::reconnect::Outcome::Recreated {
            new_session_id: new_session,
            outcome: factory_recovery::reconnect::GiveUpOutcomeSummary::Failed,
        }
    );
}

/// The central "must not be recreated" case: a workspace that no longer
/// exists on disk must leave the old session given up on and create no
/// replacement.
#[test]
fn nonexistent_workspace_is_not_recreated() {
    let (_db_dir, mut store) = open_store();
    let workspace = tempfile::tempdir().expect("tempdir");
    let gone = workspace.path().join("this-directory-was-removed");
    let scope = seed_scope(&mut store, 1, "scope", workspace.path());
    let old_session = seed_disconnected_session(&mut store, 2, scope, "pi", &gone, None, None);
    let before = session_count(&store);

    let adapter = FakeAdapter::new();

    let records = factory_recovery::reconnect::reconnect_after_herdr_or_machine_restart(
        &mut store,
        &adapter,
        4,
        id_source(100),
    )
    .expect("reconcile must succeed");

    assert_eq!(session_state(&store, old_session), "failed");
    assert!(every_lease_released(&store, old_session));
    assert_eq!(
        session_count(&store),
        before,
        "a workspace that no longer exists must never be recreated"
    );
    assert_eq!(
        records[0].outcome,
        factory_recovery::reconnect::Outcome::GivenUp(
            factory_recovery::reconnect::GiveUpOutcomeSummary::Failed
        )
    );
}

#[test]
fn queued_tasks_remain_queued_through_recreation() {
    let (_db_dir, mut store) = open_store();
    let workspace = tempfile::tempdir().expect("tempdir");
    let scope = seed_scope(&mut store, 1, "scope", workspace.path());
    seed_disconnected_session(&mut store, 2, scope, "pi", workspace.path(), None, None);
    let queued_task = seed_task(&mut store, 3, scope, "queued", None, None);

    let adapter = FakeAdapter::new();

    factory_recovery::reconnect::reconnect_after_herdr_or_machine_restart(
        &mut store,
        &adapter,
        4,
        id_source(100),
    )
    .expect("reconcile must succeed");

    let (status, _) = task_status(&store, queued_task);
    assert_eq!(status, "queued");
    assert_eq!(task_assigned_session_id(&store, queued_task), None);
    assert_eq!(delivery_attempts_count(&store, queued_task), 0);
}

/// Coordinator decision 4: a second run must not create a second session.
#[test]
fn rerunning_after_recreation_creates_no_second_session() {
    let (_db_dir, mut store) = open_store();
    let workspace = tempfile::tempdir().expect("tempdir");
    let scope = seed_scope(&mut store, 1, "scope", workspace.path());
    seed_disconnected_session(&mut store, 2, scope, "pi", workspace.path(), None, None);

    let adapter = FakeAdapter::new();

    factory_recovery::reconnect::reconnect_after_herdr_or_machine_restart(
        &mut store,
        &adapter,
        4,
        id_source(100),
    )
    .expect("first run");
    let after_first = session_count(&store);
    assert_eq!(after_first, 2, "the old session plus one recreated session");

    let second = factory_recovery::reconnect::reconnect_after_herdr_or_machine_restart(
        &mut store,
        &adapter,
        4,
        id_source(200),
    )
    .expect("second run");

    assert!(
        second.is_empty(),
        "neither the failed original nor the starting recreation is `disconnected` any more"
    );
    assert_eq!(session_count(&store), after_first);
}
