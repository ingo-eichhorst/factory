//! Two loops on one timer: fire what is due, and notice which runs have gone
//! quiet.

use chrono::Utc;
use factory_core::adapter::runtime::RuntimeStatus;
use factory_core::run::{RunStatus, Trigger};
use std::sync::Arc;
use std::time::Duration;

use crate::engine::Engine;

pub async fn run(engine: Arc<Engine>, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let tick = Duration::from_secs(engine.factory.config.daemon.tick_seconds.max(1));
    let default_timeout = engine.factory.config.daemon.task_timeout_seconds as i64;
    let default_ack = engine.factory.config.daemon.ack_timeout_seconds as i64;
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
        match engine.due_now().await {
            Ok(due) => {
                for task in due {
                    // Move the next firing before dispatching, so a dispatch
                    // that takes longer than the interval cannot start the same
                    // task twice.
                    if let Err(e) = engine.advance_schedule(&task).await {
                        tracing::warn!(task = %task.id, "could not advance schedule: {e}");
                        continue;
                    }
                    tracing::info!(task = %task.id, title = %task.title, "scheduled task is due");
                    let engine = engine.clone();
                    let id = task.id.clone();
                    tokio::spawn(async move {
                        engine.start_run(&id, Trigger::Schedule).await;
                    });
                }
            }
            Err(e) => tracing::warn!("could not look for due tasks: {e}"),
        }

        // -- standing agents ---------------------------------------------
        // Their own rule: only ever checked for whether the session is still
        // there. A permanent agent that has said nothing all day is working
        // exactly as intended, and must never be caught by the run timeouts
        // below.
        engine.supervise_agents().await;
        // What the runtime says the working agents are doing. Nothing else
        // keeps this: herdr answers "now" and forgets.
        engine.record_run_liveness().await;

        // -- runs that stopped talking -----------------------------------
        let active = match engine.active_runs().await {
            Ok(runs) => runs,
            Err(e) => {
                tracing::warn!("could not list active runs: {e}");
                continue;
            }
        };

        for run in active {
            let age = (Utc::now() - run.started_at).num_seconds();

            // A task may set its own patience. Read it per run rather than
            // once before the loop, or an override would only take effect
            // after the daemon was restarted.
            let task = engine.store.get(&run.task_id).await.ok().flatten();
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

            // Still `dispatching` means the agent was given the task and has
            // not said a word about it. Something is in front of it.
            if run.status == RunStatus::Dispatching && age > ack_secs {
                engine
                    .fail_run(
                        &run.id,
                        &format!(
                            "the agent never acknowledged the task within {ack_secs}s. \
                             Its session is usually still there -- look at it: an agent \
                             waiting on a trust prompt or a login looks exactly like this."
                        ),
                    )
                    .await;
                continue;
            }

            if age > timeout_secs {
                engine
                    .fail_run(
                        &run.id,
                        &format!(
                            "no report in {timeout_secs}s; giving up. The agent may still \
                             be working -- look at its session before starting it again."
                        ),
                    )
                    .await;
                continue;
            }

            // A session that is gone will never report. Give it a grace period
            // so a pane that is still coming up is not mistaken for a corpse.
            if age > 30 && engine.session_status(&run).await == RuntimeStatus::Gone {
                engine
                    .fail_run(&run.id, "the agent's session is gone and it never reported back")
                    .await;
            }
        }
    }
}
