//! L4's offline process-action import. It never runs a recovery command or
//! derives a Factory task/run outcome from an external process.
use crate::engine::Engine;
use std::sync::Arc;

pub(crate) async fn import(engine: &Engine) -> Vec<String> {
    let snapshot = engine.factory_snapshot();
    let root = snapshot.root.clone();
    let loaded =
        tokio::task::spawn_blocking(move || factory_core::recovery_journal::load(&root)).await;
    let mut findings = Vec::new();
    if let Ok(Ok((receipts, rejected))) = loaded {
        if rejected > 0 {
            findings.push(format!(
                "{rejected} invalid or unreadable offline receipts were rejected"
            ));
        }
        let mut conflicts = 0;
        let mut archive_failed = 0;
        for receipt in receipts {
            if snapshot.scope(&receipt.scope).is_err()
                || engine.workflows.import_recovery(&receipt).await.is_err()
            {
                conflicts += 1;
                continue;
            }
            let root = snapshot.root.clone();
            if !matches!(
                tokio::task::spawn_blocking(move || factory_core::recovery_journal::archive(
                    &root, &receipt
                ))
                .await,
                Ok(Ok(()))
            ) {
                archive_failed += 1;
            }
        }
        if conflicts > 0 {
            findings.push(format!(
                "{conflicts} receipts had an unknown scope or conflicted with immutable history"
            ));
        }
        if archive_failed > 0 {
            findings.push(format!(
                "{archive_failed} completed receipts were stored but could not be archived"
            ));
        }
    } else {
        findings.push(
            "offline recovery outbox could not be imported; stored history remains available"
                .into(),
        );
    }
    findings
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
