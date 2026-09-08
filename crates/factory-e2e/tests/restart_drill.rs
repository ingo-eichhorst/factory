//! Drill 3: backlog §9's restart drill, run against a real instance rather
//! than seeded rows. Everything up to the restart is done through the same
//! public calls drill 1 uses; only then does `factory_recovery::restore`
//! decide what the database may still claim.

mod common;

use factory_task::TaskStatus;
use factory_task::create::show;

#[test]
fn a_restart_makes_conservative_claims_and_a_human_can_resume_from_them() {
    let mut inst = common::build();
    let (alpha, gamma) = (inst.alpha, inst.gamma);
    let ws_a = inst.workspace("ws-a");
    let ws_g = inst.workspace("ws-g");

    let session_a = common::uid(10);
    factory_session::begin_start(&mut inst.store, session_a, alpha, "alpha-agent", 2, &ws_a)
        .expect("begin_start alpha");
    factory_session::mark_running(&mut inst.store, session_a).expect("mark_running");

    // A second session left in `starting` — a launch that was in flight when
    // the lights went out. ADR 0019 decision 2 says Factory can distinguish
    // none of its three possible outcomes, so it is read exactly like
    // `running`.
    let session_g = common::uid(11);
    factory_session::begin_start(&mut inst.store, session_g, gamma, "gamma-agent", 1, &ws_g)
        .expect("begin_start gamma");

    // Three tasks in the three states ADR 0019's table distinguishes.
    let running = common::uid(20);
    factory_delegation::queue::queue_from_human(
        &mut inst.store,
        running,
        alpha,
        None,
        None,
        "run me",
    )
    .expect("queue");
    factory_task::assign::assign(&mut inst.store, running, "alpha-agent", 2).expect("assign");
    let mut writer = common::RecordingWriter::default();
    factory_task::deliver::deliver(&mut inst.store, running, &mut writer).expect("deliver");
    factory_task::deliver::mark_running(&mut inst.store, running).expect("mark_running");

    let queued_never_sent = common::uid(21);
    factory_delegation::queue::queue_from_human(
        &mut inst.store,
        queued_never_sent,
        alpha,
        None,
        None,
        "never sent",
    )
    .expect("queue");

    // --- the restart ---------------------------------------------------
    let report = factory_recovery::restore::reconcile(&mut inst.store).expect("reconcile");
    assert!(
        report.report_path.exists(),
        "a restore writes its audit record"
    );

    assert_eq!(
        factory_session::show(&inst.store, session_a)
            .expect("show")
            .state,
        factory_session::SessionState::Disconnected,
    );
    assert_eq!(
        factory_session::show(&inst.store, session_g)
            .expect("show")
            .state,
        factory_session::SessionState::Disconnected,
        "a session still `starting` is read exactly like a running one"
    );
    for session in [session_a, session_g] {
        assert!(
            factory_session::leases_of_session(&inst.store, session)
                .expect("leases")
                .iter()
                .any(|lease| lease.released_at.is_none()),
            "a disconnected session keeps its lease: an unknown occupant must \
             not have its workspace handed to a second harness"
        );
    }

    let after = show(&inst.store, running).expect("show");
    assert_eq!(after.status, TaskStatus::Blocked);
    assert_eq!(
        after.blocked_reason,
        Some(factory_task::BlockedReason::Interrupted)
    );

    let untouched = show(&inst.store, queued_never_sent).expect("show");
    assert_eq!(
        untouched.status,
        TaskStatus::Queued,
        "nothing was ever sent for it"
    );
    assert_eq!(
        untouched.assigned_session_id, None,
        "but its assignment is cleared"
    );

    // Running it twice changes nothing — the only reason ADR 0019 tolerates an
    // operator copying a snapshot into place by hand.
    let second = factory_recovery::restore::reconcile(&mut inst.store).expect("second reconcile");
    assert!(second.sessions.is_empty() && second.tasks.is_empty());

    // --- the human resumes ---------------------------------------------
    // Delivery is refused until a human records the permission; that record is
    // what lets a later restart tell an authorised second delivery from an
    // accidental one.
    factory_task::deliver::authorise_resume(&mut inst.store, running).expect("authorise resume");
    assert_eq!(
        show(&inst.store, running).expect("show").status,
        TaskStatus::Queued
    );

    // The session regains sight — `Disconnected -> Running` is the edge
    // `factory_session` already has for exactly this (a permanent agent
    // recovering), and assignment needs a session that is `running`.
    factory_session::mark_running(&mut inst.store, session_a).expect("session regains sight");
    factory_task::assign::assign(&mut inst.store, running, "alpha-agent", 2).expect("re-assign");
    factory_task::deliver::deliver(&mut inst.store, running, &mut writer)
        .expect("the authorised second delivery is allowed");
    assert_eq!(writer.calls.len(), 2);
}
