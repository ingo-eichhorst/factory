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

/// A bus event that says an attempt's task may have settled: a terminal run, or a settled bench task. It is only a
/// hint to poll sooner; the judge poller's timer finds the same thing without it.
pub(crate) fn settle_hint(event: &factory_core::event::Event) -> bool {
    use factory_core::event::Event;
    match event {
        Event::RunUpdated { run } => run.status.is_terminal(),
        Event::TaskUpdated { task } => task.bench_origin.is_some() && task.is_settled(),
        _ => false,
    }
}
