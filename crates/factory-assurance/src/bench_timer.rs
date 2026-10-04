//! L5 owns its progress backstop. It must not run on L4's task/watchdog tick.
//! The host supplies the maintenance command, not an upper-level service or
//! its state. This is internal scheduling, not a sixth plugin adapter seam.
use std::{future::Future, time::Duration};
use tokio::sync::watch;

/// One non-overlapping sweep at a time; immediate first tick, delayed missed
/// ticks, the configured startup cadence (at least one second), and explicit
/// shutdown. Closing the shutdown channel also terminates the loop. Shutdown
/// can cancel blocked maintenance, just as the host can abort its loop task.
pub async fn run<F, Fut>(period: Duration, mut shutdown: watch::Receiver<bool>, mut sweep: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    if *shutdown.borrow() {
        return;
    }
    let mut ticker = tokio::time::interval(period.max(Duration::from_secs(1)));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { return; }
                continue;
            }
            _ = ticker.tick() => {}
        }
        let maintenance = sweep();
        tokio::pin!(maintenance);
        loop {
            tokio::select! {
                biased;
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() { return; }
                }
                _ = &mut maintenance => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tokio::sync::Notify;

    #[tokio::test(start_paused = true)]
    async fn independent_ticks_start_immediately_repeat_and_stop() {
        let calls = Arc::new(AtomicUsize::new(0));
        let copy = calls.clone();
        let (tx, rx) = watch::channel(false);
        let task = tokio::spawn(run(Duration::from_secs(5), rx, move || {
            let calls = copy.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
            }
        }));
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_secs(5)).await;
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        tx.send(true).unwrap();
        task.await.unwrap();
        tokio::time::advance(Duration::from_secs(100)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn zero_cadence_is_clamped_instead_of_spinning() {
        let calls = Arc::new(AtomicUsize::new(0));
        let copy = calls.clone();
        let (tx, rx) = watch::channel(false);
        let task = tokio::spawn(run(Duration::ZERO, rx, move || {
            let calls = copy.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
            }
        }));
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(999)).await;
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        tx.send(true).unwrap();
        task.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn slow_sweeps_never_overlap_and_shutdown_cancels_blocked_work() {
        let calls = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(Notify::new());
        let copy = calls.clone();
        let wait = release.clone();
        let (tx, rx) = watch::channel(false);
        let task = tokio::spawn(run(Duration::from_secs(1), rx, move || {
            let calls = copy.clone();
            let release = wait.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                release.notified().await;
            }
        }));
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(100)).await;
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // A false wakeup must not cancel the current in-progress sweep.
        tx.send(false).unwrap();
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        release.notify_one();
        for _ in 0..3 {
            tokio::task::yield_now().await;
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        tokio::time::advance(Duration::from_secs(100)).await;
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn preexisting_shutdown_or_closed_channel_never_starts_a_sweep() {
        for stopped in [false, true] {
            let (tx, rx) = watch::channel(stopped);
            drop(tx);
            run(Duration::from_secs(1), rx, || async {
                panic!("must not run")
            })
            .await;
        }
    }
}
