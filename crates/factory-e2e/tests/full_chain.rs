//! Drill 1: one task, from a human's queue call to a recorded result, across
//! every crate that touches it — config, registry, session, delegation, task,
//! context. No stubbed layer anywhere: the registry is projected by
//! `factory_registry::apply`, the session is started by
//! `factory_session::begin_start`, and the task is queued through
//! `factory_delegation`, which is the only door a human's task comes in by.

mod common;

use factory_session::SessionState;
use factory_task::TaskStatus;
use factory_task::create::show;

#[test]
fn a_human_task_reaches_an_agent_and_comes_back_done() {
    let mut inst = common::build();
    let workspace = inst.workspace("ws-alpha-1");

    // 1. A human queues work at `alpha`. Every registered scope is a legal
    //    target for a human (design §6), so no kinship question arises.
    let task = common::uid(1);
    factory_delegation::queue::queue_from_human(
        &mut inst.store,
        task,
        inst.alpha,
        None,
        None,
        "summarise the drill",
    )
    .expect("a human may queue to any registered scope");

    assert_eq!(
        show(&inst.store, task).expect("show").status,
        TaskStatus::Queued
    );
    assert_eq!(
        factory_task::create::delegation_chain_of(&inst.store, task).expect("chain"),
        vec![inst.alpha],
        "a human-queued task has travelled through exactly the scope it was sent to"
    );

    // 2. A session for that scope's agent starts in a real directory.
    let session = common::uid(2);
    factory_session::begin_start(
        &mut inst.store,
        session,
        inst.alpha,
        "alpha-agent",
        2,
        &workspace,
    )
    .expect("begin_start");
    assert_eq!(
        factory_session::show(&inst.store, session)
            .expect("show session")
            .state,
        SessionState::Starting,
    );
    factory_session::mark_running(&mut inst.store, session).expect("readiness observed");

    // 3. Assignment picks that session, because it is the only idle one.
    let outcome =
        factory_task::assign::assign(&mut inst.store, task, "alpha-agent", 2).expect("assign");
    assert_eq!(outcome, factory_task::assign::Assignment::Assigned(session));

    // 4. Delivery journals the attempt before the prompt is written, and the
    //    prompt carries the task's own id (design §5).
    let mut writer = common::RecordingWriter::default();
    factory_task::deliver::deliver(&mut inst.store, task, &mut writer).expect("deliver");
    assert_eq!(writer.calls.len(), 1, "exactly one hand-off");
    assert_eq!(writer.calls[0].0, session);
    assert!(
        writer.calls[0].1.contains(&task.to_string()),
        "the prompt must name the task it is for"
    );

    factory_task::deliver::mark_running(&mut inst.store, task).expect("mark_running");
    assert_eq!(
        show(&inst.store, task).expect("show").status,
        TaskStatus::Running
    );
    assert_eq!(
        factory_task::assign::running_task_of_session(&inst.store, session).expect("running task"),
        Some(task),
    );

    // 5. The agent reports a result, and the session outlives the task only if
    //    its lifetime says so.
    factory_task::complete::done(&mut inst.store, task, Some("drill summary"), None).expect("done");
    let finished = show(&inst.store, task).expect("show");
    assert_eq!(finished.status, TaskStatus::Done);
    assert_eq!(finished.result_summary.as_deref(), Some("drill summary"));

    // 6. A second delivery is refused without a recorded authorisation —
    //    design §5's "does not automatically resend a possibly delivered
    //    prompt", which is what makes this whole chain safe to restart.
    let second = factory_task::deliver::deliver(&mut inst.store, task, &mut writer);
    assert!(
        second.is_err(),
        "a delivered task is never silently re-delivered"
    );
    assert_eq!(writer.calls.len(), 1, "and no prompt reached a terminal");
}
