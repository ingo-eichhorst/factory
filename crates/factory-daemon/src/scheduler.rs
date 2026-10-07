//! Two loops on one timer: fire what is due, and notice which runs have gone
//! quiet.

use chrono::{DateTime, Utc};
use factory_core::adapter::runtime::RuntimeStatus;
use factory_core::run::{FailKind, RunStatus, Trigger};
use std::sync::Arc;
use std::time::Duration;

use crate::engine::Engine;

pub async fn run(engine: Arc<Engine>, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let factory = engine.factory_snapshot();
    let tick = Duration::from_secs(factory.config.daemon.tick_seconds.max(1));
    let default_timeout = factory.config.daemon.task_timeout_seconds as i64;
    let default_ack = factory.config.daemon.ack_timeout_seconds as i64;
    let default_blocked = factory.config.daemon.blocked_timeout_seconds as i64;
    let mut ticker = tokio::time::interval(tick);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = shutdown.changed() => {
                if *shutdown.borrow() { return; }
            }
        }

        // -- due tasks ---------------------------------------------------
        match engine.l4_service().due_now().await {
            Ok(due) => {
                for task in due {
                    // Held from the re-read to the move of `next_run_at`, so
                    // a `task.skip_next` in between cannot be fired past: a
                    // task whose slot changed since `due_now` read it is
                    // somebody else's decision now, and waits for the next
                    // tick to be looked at again.
                    let _slot = engine.l4.schedule_lock.lock().await;
                    let Some(task) = engine.l4_service().still_due(&task).await else {
                        continue;
                    };
                    // A task with a queued retry (`pending_retry`, set only by
                    // `Engine::queue_or_end_retry`) is due here because its
                    // backoff elapsed, not because the schedule's own next
                    // slot arrived -- so it gets `resume_from_retry` rather
                    // than `advance_schedule`, and `Trigger::Retry` rather
                    // than `Trigger::Schedule`. Either way the next firing is
                    // moved before dispatching, so a dispatch that takes
                    // longer than the interval cannot start the same task
                    // twice -- see each function's own comment for why they
                    // move it differently.
                    //
                    // The slot is read off the task first, because both of
                    // those move `next_run_at` on and it is gone after: it is
                    // when this run became due, and -- for the schedule's own
                    // firing -- the slot that fired it.
                    let slot = task.next_run_at.unwrap_or_else(Utc::now);
                    let retry = task.pending_retry.is_some();
                    let due = engine.l4_service().due_for(&task, slot, !retry).await;
                    let (trigger, advanced) = if retry {
                        (Trigger::Retry, engine.l4_service().resume_from_retry(&task).await)
                    } else {
                        (Trigger::Schedule, engine.l4_service().advance_schedule(&task).await)
                    };
                    if let Err(e) = advanced {
                        tracing::warn!(task = %task.id, "could not advance schedule: {e}");
                        continue;
                    }
                    tracing::info!(
                        task = %task.id,
                        title = %task.title,
                        trigger = trigger.as_str(),
                        "scheduled task is due"
                    );
                    let engine = engine.clone();
                    let id = task.id.clone();
                    tokio::spawn(async move {
                        engine.l4_service().start_run_due(&id, trigger, due).await;
                    });
                }
            }
            Err(e) => tracing::warn!("could not look for due tasks: {e}"),
        }

        // -- dependency-released tasks ----------------------------------
        // Decomposition roots are started by intake itself. Every later
        // part waits as an ordinary Pending task until all of its declared
        // predecessors are Done; the status transition at dispatch keeps
        // the next tick from claiming it a second time.
        match engine.l4_service().dependency_ready_tasks().await {
            Ok(tasks) => {
                for task in tasks {
                    let engine = engine.clone();
                    tokio::spawn(async move {
                        engine.l4_service().start_run_due(&task.id, Trigger::Dependency, crate::engine::Due::now()).await;
                    });
                }
            }
            Err(error) => tracing::warn!("could not look for dependency-ready tasks: {error}"),
        }

        // -- tasks held on a harness that did not start (#131) ---------------
        // Rate-limited and backgrounded inside; this only ever starts it.
        engine.recheck_harnesses();

        // -- tasks held on max_sessions (#179) --------------------------------
        // Queued onto the same capacity-release channel a run ending uses,
        // not run inline: one slow dispatch here would otherwise delay every
        // due task this tick still has to fire, the timeout checks below,
        // and supervise_agents, exactly what tokio::spawn-ing each due
        // dispatch above already avoids. The backstop for a restart, a
        // raised limit, or a wakeup the channel dropped; every ordinary
        // release reaches its waiting task immediately through that channel
        // on its own.
        engine.l4_service().enqueue_capacity_sweep();
        engine.l4_service().sweep_workspaces().await;

        // -- standing agents ---------------------------------------------
        // Their own rule: only ever checked for whether the session is still
        // there. A permanent agent that has said nothing all day is working
        // exactly as intended, and must never be caught by the run timeouts
        // below.
        factory_agents::dispatch::Supervision::supervise(crate::commands::l3(&engine).port()).await;
        // What L3 just observed about the standing agents' sessions, into the liveness record.
        engine.l4_service().sample_standing_agents().await;
        // What the runtime says the working agents are doing. Nothing else
        // keeps this: herdr answers "now" and forgets.
        engine.record_run_liveness().await;

        // -- runs that stopped talking -----------------------------------
        let active = match engine.l4_service().active_runs().await {
            Ok(runs) => runs,
            Err(e) => {
                tracing::warn!("could not list active runs: {e}");
                continue;
            }
        };

        for run in active {
            let now = Utc::now();

            // A task may set its own patience. Read it per run rather than
            // once before the loop, or an override would only take effect
            // after the daemon was restarted.
            let task = engine.l4.store.get(&run.task_id).await.ok().flatten();
            let ack_secs = task
                .as_ref()
                .and_then(|t| t.ack_timeout_seconds)
                .map(|v| v as i64)
                .unwrap_or(default_ack);
            let timeout_secs = task
                .as_ref()
                .and_then(|t| t.timeout_seconds)
                .map(|v| v as i64)
                .unwrap_or(default_timeout);
            let blocked_secs = task
                .as_ref()
                .and_then(|t| t.blocked_timeout_seconds)
                .map(|v| v as i64)
                .unwrap_or(default_blocked);

            if let Some((kind, why)) = overdue(
                run.status,
                run.started_at,
                run.blocked_since,
                now,
                ack_secs,
                timeout_secs,
                blocked_secs,
            ) {
                // A run nobody acknowledged may be a harness that stopped
                // starting since it was last probed: the next dispatch to
                // it probes again rather than trusting the cache (#131).
                if kind == FailKind::AckTimeout {
                    engine.l4_service().doubt_harness_of(task.as_ref());
                }
                engine.l4_service().fail_run(&run.id, kind, &why).await;
                continue;
            }

            // A session that is gone will never report. Give it a grace period
            // so a pane that is still coming up is not mistaken for a corpse.
            // Applies to a `Blocked` run too -- a block is honest only as long
            // as the session it names is actually still there.
            let age = (now - run.started_at).num_seconds();
            // A verifying run's agent is finished; its session going away
            // (a `shell` pane that exits) says nothing about the gates the
            // daemon is running for it.
            if run.status != RunStatus::Verifying
                && age > 30
                && engine.session_status(&run).await == RuntimeStatus::Gone
            {
                engine
                    .l4_service()
                    .fail_run(
                        &run.id,
                        FailKind::SessionGone,
                        "the agent's session is gone and it never reported back",
                    )
                    .await;
            }
        }
    }
}

/// Whether an active run has run out of patience, and why -- judged only by
/// its own status and timestamps, so this is testable without a store, a
/// runtime, or an `Engine`. The kind comes back with the prose, decided in
/// the same branch that wrote it, so the two cannot disagree. `None` means
/// "leave it running"; the caller still
/// has its own, separate `Gone`-session check to make afterwards.
///
/// A `Blocked` run is measured against `blocked_secs` from `blocked_since`,
/// never against `ack_secs` or `timeout_secs` from `started_at` -- `ack_secs`
/// exists to catch a run that never said a word, and `timeout_secs` caps how
/// long the whole run may take, however often it reports in the meantime.
/// A run sitting on a hook-reported block is not silent, so `ack_secs` never
/// applies to it; `timeout_secs` is suspended for exactly as long as it
/// stays blocked, in favour of `blocked_secs`, and picks back up the moment
/// it unblocks -- see the comment below on why that reunion is often abrupt.
fn overdue(
    status: RunStatus,
    started_at: DateTime<Utc>,
    blocked_since: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    ack_secs: i64,
    timeout_secs: i64,
    blocked_secs: i64,
) -> Option<(FailKind, String)> {
    // `#118`: the agent has said done and the daemon is running its
    // required gates, each bounded by its own timeout. Neither the ack nor
    // the run's total-duration cap is about this stretch -- the work is
    // finished -- so neither may fail it mid-verification.
    if status == RunStatus::Verifying {
        return None;
    }
    if status == RunStatus::Blocked {
        // `blocked_since` should always be set by whatever put the run into
        // `Blocked`, but a run that somehow lacks it is still a run someone
        // is waiting on, not a run to lose track of -- fall back to when it
        // started rather than never expiring it at all.
        let since = blocked_since.unwrap_or(started_at);
        let blocked_age = (now - since).num_seconds();
        if blocked_age > blocked_secs {
            return Some((FailKind::BlockedTimeout, format!(
                "blocked and waiting for a human for {blocked_secs}s and nobody answered. \
                 Its session is still open -- it is sitting on a question."
            )));
        }
        return None;
    }

    let age = (now - started_at).num_seconds();

    // Still `dispatching` means the agent was given the task and has not
    // said a word about it. Something is in front of it.
    if status == RunStatus::Dispatching && age > ack_secs {
        return Some((FailKind::AckTimeout, format!(
            "the agent never acknowledged the task within {ack_secs}s. \
             Its session is usually still there -- look at it: an agent \
             waiting on a trust prompt or a login looks exactly like this."
        )));
    }

    // `age` is still `now - started_at`, unmodified, which is deliberate but
    // has a sharp edge: a run just returned from a long `Blocked` spell
    // reaches this line with `age` counting the whole time it sat blocked,
    // because `started_at` is never nudged forward to buy that time back --
    // occupancy's own rule is that a blocked run still honestly occupies its
    // bay, and shifting `started_at` to hide the wait would make that chart
    // lie about it. In practice that means a run blocked for longer than
    // `timeout_secs` will very likely fail here on the very next tick after
    // it unblocks. A task that expects to sit blocked for a while wants a
    // correspondingly generous `timeout_seconds`, the same way it would for
    // any run that legitimately takes long.
    if age > timeout_secs {
        // Name the condition this actually is -- a total-duration cap on
        // `age`, not a silence watchdog -- so whoever reads the run's error
        // goes looking at the right clock instead of the time since its
        // last report.
        return Some((FailKind::RunTimeout, format!(
            "ran for longer than {timeout_secs}s without finishing; giving up. The agent may \
             still be working -- look at its session before starting it again."
        )));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000 + secs, 0).unwrap()
    }

    #[test]
    fn a_verifying_run_is_never_overdue_however_long_its_gates_take() {
        assert_eq!(overdue(RunStatus::Verifying, at(0), None, at(1_000_000), 100, 3600, 10_000), None);
    }

    #[test]
    fn a_reported_blocks_timeout_is_measured_from_blocked_since_not_started_at() {
        // Started long ago, but only blocked recently -- well inside the
        // blocked timeout, even though `now - started_at` alone would not be.
        let started = at(0);
        let blocked_since = at(500);
        let now = at(500 + 600);
        assert_eq!(
            overdue(RunStatus::Blocked, started, Some(blocked_since), now, 100, 100, 1000),
            None,
            "600s blocked is under a 1000s blocked timeout, however old the run itself is"
        );

        let now_past = at(500 + 1001);
        assert_eq!(
            overdue(RunStatus::Blocked, started, Some(blocked_since), now_past, 100, 100, 1000).map(|(k, _)| k),
            Some(FailKind::BlockedTimeout),
            "1001s blocked exceeds a 1000s blocked timeout"
        );
    }

    #[test]
    fn a_blocked_run_is_exempt_from_the_ordinary_ack_and_task_timeouts() {
        // Ack and task timeouts are tiny; the blocked timeout is generous.
        // A `Blocked` run must answer to none of the first two.
        let started = at(0);
        let blocked_since = at(0);
        let now = at(50_000);
        assert_eq!(
            overdue(RunStatus::Blocked, started, Some(blocked_since), now, 1, 1, 100_000),
            None,
        );
    }

    #[test]
    fn a_run_still_dispatching_past_its_ack_timeout_fails_with_that_reason() {
        let (kind, why) = overdue(RunStatus::Dispatching, at(0), None, at(200), 100, 10_000, 10_000).unwrap();
        assert!(why.contains("never acknowledged"), "{why}");
        assert_eq!(kind, FailKind::AckTimeout);
    }

    #[test]
    fn a_running_run_past_its_task_timeout_fails_with_that_reason() {
        let (kind, why) = overdue(RunStatus::Running, at(0), None, at(4000), 100, 3600, 10_000).unwrap();
        assert!(why.contains("ran for longer than 3600s"), "{why}");
        assert_eq!(kind, FailKind::RunTimeout);
    }

    #[test]
    fn the_task_timeout_message_names_a_duration_cap_not_a_silence_watchdog() {
        // Regression for the mismatch in issue #63: `age` is `now -
        // started_at`, so a run that reported constantly right up until the
        // tick that catches it is still killed here -- the message must not
        // claim it went quiet, only that it ran too long.
        let (_, why) = overdue(RunStatus::Running, at(0), None, at(5726), 100, 5400, 10_000).unwrap();
        assert!(!why.contains("no report"), "{why}");
        assert!(why.contains("ran for longer than 5400s without finishing"), "{why}");
    }

    #[test]
    fn a_run_within_every_timeout_is_left_alone() {
        assert_eq!(overdue(RunStatus::Running, at(0), None, at(10), 100, 3600, 10_000), None);
    }
}
