//! Drill 7: `factory_daemon::observe::spawn` — the threaded runner around
//! `reconcile_once` that ADR 0014's crate docs call "a loop the daemon owns
//! rather than a one-shot call," and that had no test of its own before this
//! file: only the single pass underneath it (`reconcile_once`) was covered.
//!
//! Both tests here are deterministic on purpose — no sleep-and-hope, no wall-
//! clock assumption about how many intervals "should" have fired by now:
//!
//! - "observes on its interval" is proven by a stub [`Adapter::observe`]
//!   signalling a channel every time it is called, `recv_timeout`'d three
//!   times in a row. A short interval only bounds *how long* the drill takes,
//!   never *whether* it passes.
//! - "stopping is prompt, not eventually" is proven with a **long** interval
//!   (30s): the loop's first pass happens immediately (before it ever waits on
//!   the interval), the drill stops it right after that first pass, and
//!   [`join_within`] asserts the spawned thread actually exits within 2s —
//!   nowhere near the 30s the interval would otherwise force. A drill that
//!   merely used a short interval and asserted the loop stopped "eventually"
//!   could not tell a prompt stop from one that happened to overlap the next
//!   tick.
//!
//! `factory_daemon::handler::FactoryHandler` and `factory_daemon::observe`
//! are both public (`pub use handler::FactoryHandler` and `pub mod observe`),
//! so — unlike the process-level restart drill in this same crate — this one
//! needs no subprocess at all: the loop is driven in-process, against a
//! second `factory_store::Store` connection opened on the same instance root
//! `common::build()` already set up (`FactoryHandler`'s own store is behind a
//! `pub(crate)` mutex, so a test cannot read through it directly).

mod common;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use factory_adapter::{
    Adapter, AdapterError, Confidence, Observation, PaneId, StartRequest, StartedSession,
    TaskSignal,
};
use factory_daemon::handler::FactoryHandler;

/// Records `herdr_pane_id` directly on a session's row. `factory_daemon`'s
/// own writer for this (`handler::pane::record`) is `pub(crate)` to that
/// crate, so this drill sets the column the same way `live_agent.rs` and
/// `live_herdr.rs` already do in this crate: directly, by name, the one
/// column `observe`'s own `pane::read` looks at.
fn record_pane(store: &mut factory_store::Store, session_id: uuid::Uuid, pane: &str) {
    let tx = store.transaction().expect("begin");
    tx.execute(
        "UPDATE sessions SET herdr_pane_id = ?2 WHERE id = ?1",
        (session_id.to_string(), pane),
    )
    .expect("record pane");
    tx.commit().expect("commit");
}

/// An [`Adapter`] whose `observe` does two things every time it is called:
/// signals `signal` (proving *that* it was called, and how many times), and
/// returns a canned, authoritative, alive [`Observation`] whose
/// `harness_state` alternates between Herdr's two words for "nothing to
/// report" — `idle` and `done` — neither of which
/// [`factory_adapter::TaskSignal`]'s vocabulary can ever turn into task
/// completion. `task_signal` is always [`TaskSignal::NoChange`]: this
/// adapter's whole point is to look, every single pass, exactly like a
/// harness that finished a turn and is waiting for the next one — the one
/// shape a wrongly-written loop would be tempted to read as "done."
struct SignalOnObserve {
    signal: Mutex<mpsc::Sender<()>>,
    calls: AtomicU32,
}

impl SignalOnObserve {
    fn new(signal: mpsc::Sender<()>) -> Self {
        Self {
            signal: Mutex::new(signal),
            calls: AtomicU32::new(0),
        }
    }
}

impl Adapter for SignalOnObserve {
    fn start(&self, _req: &StartRequest) -> Result<StartedSession, AdapterError> {
        unreachable!("this drill never calls Adapter::start")
    }
    fn send(
        &self,
        _pane: &PaneId,
        _task_id: uuid::Uuid,
        _prompt: &str,
    ) -> Result<(), AdapterError> {
        unreachable!("this drill never calls Adapter::send")
    }
    fn observe(&self, pane: &PaneId) -> Result<Observation, AdapterError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        let _ = self.signal.lock().unwrap().send(());
        Ok(Observation {
            pane: pane.clone(),
            harness_state: if n % 2 == 0 { "idle" } else { "done" }.to_string(),
            confidence: Confidence::Authoritative,
            session_alive: true,
            task_signal: TaskSignal::NoChange,
            transcript_path: None,
            harness_session_id: None,
        })
    }
    fn interrupt(&self, _pane: &PaneId) -> Result<(), AdapterError> {
        unreachable!("this drill never calls Adapter::interrupt")
    }
    fn stop(&self, _pane: &PaneId) -> Result<(), AdapterError> {
        unreachable!("this drill never calls Adapter::stop")
    }
    fn attach_command(&self, _pane: &PaneId) -> Result<Vec<String>, AdapterError> {
        unreachable!("this drill never calls Adapter::attach_command")
    }
    fn runtime_version(&self) -> Result<String, AdapterError> {
        Ok("signal-on-observe-1.0".to_string())
    }

    /// This double reports no cost data. Stated rather than omitted: the
    /// trait has no default, so an implementor cannot answer `None` by
    /// forgetting the method. ADR 0021 decision 6 makes `None` a valid
    /// answer, and it is this fake's real one.
    fn cost_sample(
        &self,
        _pane: &PaneId,
    ) -> Result<Option<factory_adapter::CostSample>, AdapterError> {
        Ok(None)
    }
}

/// Runs `join` on a helper thread and reports back over a channel, so the
/// caller can bound how long it waits rather than calling `JoinHandle::join`
/// directly — which would hang the whole test suite, not just fail one
/// assertion, against the exact mutation (an ignored stop signal) this drill
/// exists to catch.
fn join_within(join: std::thread::JoinHandle<()>, timeout: Duration) -> bool {
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = join.join();
        let _ = done_tx.send(());
    });
    done_rx.recv_timeout(timeout).is_ok()
}

#[test]
fn the_threaded_loop_observes_on_its_interval_and_never_closes_a_task_on_idle_or_done() {
    let mut inst = common::build();
    let alpha = inst.alpha;
    let ws = inst.workspace("ws-observe");

    let session_id = common::uid(700);
    factory_session::begin_start(&mut inst.store, session_id, alpha, "alpha-agent", 2, &ws)
        .expect("begin_start");
    record_pane(&mut inst.store, session_id, "pane-observe-a");
    factory_session::mark_running(&mut inst.store, session_id).expect("mark_running");

    // A real task, assigned to and delivered on that session — running, so
    // `reconcile_running`'s only live write path (blocking it) is reachable
    // and provably not taken.
    let task_id = common::uid(701);
    factory_delegation::queue::queue_from_human(
        &mut inst.store,
        task_id,
        alpha,
        None,
        None,
        "watch me not get closed",
    )
    .expect("queue");
    factory_task::assign::assign(&mut inst.store, task_id, "alpha-agent", 2).expect("assign");
    let mut writer = common::RecordingWriter::default();
    factory_task::deliver::deliver(&mut inst.store, task_id, &mut writer).expect("deliver");
    factory_task::deliver::mark_running(&mut inst.store, task_id).expect("mark_running");

    let (signal_tx, signal_rx) = mpsc::channel();
    let adapter = SignalOnObserve::new(signal_tx);

    let handler_store = factory_store::Store::open(inst.dir.path()).expect("open second store");
    let handler = Arc::new(FactoryHandler::new(handler_store, adapter, inst.dir.path()));

    let (join, stop_tx) = factory_daemon::observe::spawn(handler, Duration::from_millis(30));

    // Three separate passes, each proven by its own signal — not "the loop
    // ran for N milliseconds," which would only ever be a guess about how
    // many passes that implies.
    for i in 0..3 {
        signal_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_else(|_| panic!("expected observe pass #{i} within 2s"));
    }

    // Stop it, then prove the running task the loop passed over three times —
    // twice as `idle`, once as `done`, always `Authoritative`, always alive —
    // is still exactly `running`. This is `observe`'s own module rule ("no
    // harness state closes a task") checked dynamically through the threaded
    // runner, not just true by construction of `TaskSignal`.
    stop_tx.send(()).expect("send stop");
    assert!(
        join_within(join, Duration::from_secs(2)),
        "the observe loop must stop promptly once told to"
    );

    let task = factory_task::create::show(&inst.store, task_id).expect("show");
    assert_eq!(
        task.status,
        factory_task::TaskStatus::Running,
        "neither `idle` nor `done` may close a task, even after several loop passes: {task:?}"
    );
    let session = factory_session::show(&inst.store, session_id).expect("show");
    assert_eq!(
        session.state,
        factory_session::SessionState::Running,
        "an authoritative, alive observation must leave a running session running"
    );
}

#[test]
fn stopping_the_observe_loop_is_prompt_not_eventually() {
    let mut inst = common::build();
    let alpha = inst.alpha;
    let ws = inst.workspace("ws-observe-stop");

    let session_id = common::uid(710);
    factory_session::begin_start(&mut inst.store, session_id, alpha, "alpha-agent", 2, &ws)
        .expect("begin_start");
    record_pane(&mut inst.store, session_id, "pane-observe-b");
    factory_session::mark_running(&mut inst.store, session_id).expect("mark_running");

    let (signal_tx, signal_rx) = mpsc::channel();
    let adapter = SignalOnObserve::new(signal_tx);

    let handler_store = factory_store::Store::open(inst.dir.path()).expect("open second store");
    let handler = Arc::new(FactoryHandler::new(handler_store, adapter, inst.dir.path()));

    // A long interval: if stopping ever had to wait for it, this drill would
    // itself take 30 seconds, or the bounded join below would time out and
    // fail loudly rather than hang. Either way, "eventually" would be visible.
    let (join, stop_tx) = factory_daemon::observe::spawn(handler, Duration::from_secs(30));

    // `spawn`'s loop body runs its first `reconcile_once` pass before it ever
    // waits on the interval — so this is guaranteed to arrive quickly, not
    // "usually."
    signal_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the first observe pass must happen immediately, not after the 30s interval");

    let stopped_at = std::time::Instant::now();
    stop_tx.send(()).expect("send stop");
    let stopped = join_within(join, Duration::from_secs(2));
    let elapsed = stopped_at.elapsed();

    assert!(
        stopped,
        "the observe loop must stop within 2s of being told to, not after its 30s interval \
         elapses (it did not stop at all within the bound)"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "stopping took {elapsed:?} — nowhere near prompt against a 30s interval"
    );
}
