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
    let timeout_secs = engine.factory.config.daemon.task_timeout_seconds as i64;
    let ack_secs = engine.factory.config.daemon.ack_timeout_seconds as i64;
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
