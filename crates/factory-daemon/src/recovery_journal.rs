//! L4's offline process-action import. It never runs a recovery command or
//! derives a Factory task/run outcome from an external process.
use crate::engine::Engine;
use std::sync::Arc;

pub(crate) async fn import(engine: &Engine) -> Vec<String> {
    crate::facts::import_recovery_journal(engine).await
}

/// Keep receipt persistence off the scheduler's critical path. Startup and
/// page reads also import; immutable, transactional inserts make races safe.
pub(crate) async fn run(engine: Arc<Engine>, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let mut timer = tokio::time::interval(std::time::Duration::from_secs(30));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut previous = Vec::new();
    loop {
        tokio::select! {
            _ = timer.tick() => {}
            _ = shutdown.changed() => { if *shutdown.borrow() { return; } }
        }
        let findings = import(&engine).await;
        if findings != previous {
            for finding in &findings {
                tracing::warn!("recovery journal: {finding}");
            }
            previous = findings;
        }
    }
}
