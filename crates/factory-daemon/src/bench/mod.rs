mod engine;
mod store;

pub use store::BenchStore;

/// Wire L5's independent progress loop. Recovery still runs once at startup;
/// the periodic backstop and shutdown lifecycle are separate from L4.
pub(crate) async fn run(
    engine: std::sync::Arc<crate::engine::Engine>,
    shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let seconds = engine.factory_snapshot().config.daemon.tick_seconds;
    factory_core::bench_timer::run(
        std::time::Duration::from_secs(seconds),
        shutdown,
        move || {
            let engine = engine.clone();
            async move {
                engine.sweep_bench_runs().await;
            }
        },
    )
    .await;
}
