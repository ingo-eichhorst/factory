//! Whether the host is allowed to sleep while a run is active -- issue #61.
//!
//! A dispatched task's `timeout_seconds` is spent by wall clock, and wall
//! clock keeps moving while the host cannot execute anything. The run that
//! prompted this module was dispatched into a host stuck cycling
//! SleepService/Maintenance DarkWake windows of 45-62s every ~16 minutes,
//! each one falling straight back to sleep: about a 5% duty cycle against a
//! 5400s budget, so the agent got roughly five minutes of a session it was
//! promised ninety. `caffeinate -i` (what Claude Code already takes for
//! itself) would not have helped -- idle-sleep prevention does not stop a
//! DarkWake handing control back to system sleep, only a *system*-sleep
//! assertion does that. This module is Factory's own version of that
//! assertion, held for as long as any run is active rather than on a timer.
//!
//! The host-level `pmset` misconfiguration that actually let the lid sleep
//! this hard is being fixed separately; this is defence in depth so a
//! changed power profile or a closed lid cannot silently eat a run's budget
//! again.
//!
//! ## The platform seam
//!
//! Only macOS has anything to do here. Rather than scatter
//! `#[cfg(target_os = "macos")]` through `engine.rs`, the platform-specific
//! part is a small trait (`Assertion`, below) in the same spirit as the four
//! adapter traits in `factory_core::adapter`: the engine depends on an
//! interface, not a concrete OS. It is not one of those four seams --
//! nothing about "can this host be told not to sleep" is a thing a plugin
//! should be choosing on Factory's behalf, so it has no registry entry and
//! lives here instead. Every non-macOS target gets `NoAssertion`, which
//! turns every call into a no-op and needs no `cfg` of its own to compile.
//!
//! ## Why a set of run ids, not a bare counter
//!
//! `PowerAssertions` holds one assertion for however many runs are active at
//! once, refcounted by run id rather than a plain integer. `acquire` and
//! `release` are each idempotent as a result: `Engine::close_session`'s own
//! doc comment says a run can reach it twice (a terminal report racing the
//! scheduler's watchdog, for instance), and a bare counter would
//! double-decrement in that race and let the host sleep out from under a
//! run that is still going. A `HashSet<String>` keyed by run id cannot
//! double-decrement -- a second `release` for the same id is just a miss.
//! The same shape `Engine::bench_judging` already uses for its own
//! report-vs-cancel race, just above `bench_judging`'s declaration in
//! `engine.rs`.
//!
//! ## Why `-w <pid>`, not just an explicit kill
//!
//! `PowerAssertions::release` kills the held child explicitly once the last
//! run lets go, which covers every ordinary case: a run finishing, failing,
//! being cancelled, timing out, or a dispatch that never got as far as a
//! session. It does *not* cover the daemon itself being sent `SIGKILL` --
//! nothing in this process runs on signal 9, `Drop` included, so an explicit
//! kill can never be the whole story. `caffeinate -w <pid>` names this
//! daemon's own process id and is what makes the child notice its parent is
//! gone and let the assertion go on its own. Checked by hand against a
//! throwaway daemon, not assumed from the man page: a real run, dispatched
//! with `--no-worktree` against a scratch instance, showed up in
//! `pmset -g assertions` as both `PreventUserIdleSystemSleep` and
//! `PreventSystemSleep` "Created for PID: <daemon>" while it was in flight;
//! releasing it normally (the run finishing) and `kill -9`-ing the daemon
//! mid-run both left no `caffeinate` process behind afterwards.
//!
//! ## What this does not cover
//!
//! On battery, `-s` is void by `man caffeinate`'s own account and only `-i`
//! is actually held -- so on battery this module is partial mitigation, not
//! the fix: idle sleep is still prevented, but nothing stops a DarkWake
//! window handing control back to system sleep, which is the exact failure
//! this issue is about. The `pmset` fix mentioned above is what actually
//! closes that gap; this module's job on battery is only to make the good
//! case (AC power) work automatically.
//!
//! A graceful restart with a run still active drops the assertion for the
//! gap between processes: `-w` names the *old* daemon's pid, and a run
//! adopted from before the restart never calls `Engine::dispatch` again in
//! the new one, so it never re-`acquire`s. Fixing that means re-acquiring
//! for every row `TaskStore::active_runs` already returns at startup, which
//! touches the daemon's startup ordering for a scenario this issue's
//! evidence never exercised (one process, asleep, not a restart) --
//! deliberately left for a future issue rather than folded in here.

use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::Mutex;

/// The platform seam: what it takes to tell the OS "do not sleep" and to
/// take that back. `PowerAssertions` below owns the refcounting; an
/// `Assertion` only ever sees "the count rose from zero" (`hold`) and "the
/// count fell back to zero" (`let_go`).
#[async_trait::async_trait]
trait Assertion: Send + Sync {
    async fn hold(&self);
    async fn let_go(&self);
}

/// Every platform but macOS: holding this open means nothing to the OS, so
/// say so and do nothing. Unconditionally compiled (not itself behind a
/// `cfg`) so `platform()` always has one obviously-correct fallback rather
/// than needing two arms that both have to compile.
struct NoAssertion;

#[async_trait::async_trait]
impl Assertion for NoAssertion {
    async fn hold(&self) {}
    async fn let_go(&self) {}
}

/// macOS: a `caffeinate` child process is the assertion. `-s` is the flag
/// that actually matters -- it is the one that stops a DarkWake handing
/// control back to system sleep, which is what this module exists for -- but
/// `man caffeinate` documents it as valid only while the system is on AC
/// power, so `-i` rides along for the battery case. On battery this is
/// weaker: `-i` only prevents *idle* sleep, the same gap Claude Code's own
/// `caffeinate -i -t 300` leaves open (see this module's header), so on
/// battery a DarkWake can still hand control back to system sleep. See "What
/// this does not cover" above. `-w <pid>` ties the child to this daemon
/// process rather than a timeout.
#[cfg(target_os = "macos")]
struct CaffeinateAssertion {
    child: Mutex<Option<tokio::process::Child>>,
}

#[cfg(target_os = "macos")]
#[async_trait::async_trait]
impl Assertion for CaffeinateAssertion {
    async fn hold(&self) {
        let pid = std::process::id().to_string();
        let spawned = tokio::process::Command::new("caffeinate")
            .args(["-s", "-i", "-w", &pid])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            // `-w` only helps once the daemon itself is gone; it cannot
            // notice a `Child` dropped for some other reason while the
            // daemon is still alive (a bug bypassing `let_go`, not the
            // expected path). `kill_on_drop` covers exactly that gap.
            .kill_on_drop(true)
            .spawn();
        match spawned {
            Ok(child) => *self.child.lock().await = Some(child),
            // No `caffeinate` on this machine, sandboxed out, whatever the
            // reason -- the run this is defending still has to run. Losing
            // the defence issue #61 adds is not a reason to fail the run it
            // is meant to protect.
            Err(e) => tracing::warn!("power assertion: could not start caffeinate: {e}"),
        }
    }

    async fn let_go(&self) {
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.kill().await;
        }
    }
}

#[cfg(target_os = "macos")]
fn platform() -> Arc<dyn Assertion> {
    Arc::new(CaffeinateAssertion { child: Mutex::new(None) })
}

#[cfg(not(target_os = "macos"))]
fn platform() -> Arc<dyn Assertion> {
    Arc::new(NoAssertion)
}

/// What `Engine` actually holds: one shared assertion, refcounted by run id.
/// See this module's header for why a set rather than a counter, and why
/// `acquire`/`release` are safe to call from every terminal path
/// unconditionally.
pub struct PowerAssertions {
    inner: Arc<dyn Assertion>,
    active: Mutex<HashSet<String>>,
}

impl PowerAssertions {
    /// `enabled` is `daemon.power_assertion` (on by default). Off and "not
    /// macOS" both resolve to the same `NoAssertion` no-op, so nothing above
    /// this ever needs to ask which reason applies.
    pub fn new(enabled: bool) -> Self {
        Self {
            inner: if enabled { platform() } else { Arc::new(NoAssertion) },
            active: Mutex::new(HashSet::new()),
        }
    }

    /// A run is starting: keep the host awake for it. Called once, from
    /// `Engine::dispatch`, right after the run's row is created -- see that
    /// function's own comment on why that is the moment a run is considered
    /// to exist.
    ///
    /// The lock is held across `hold`'s `.await`: that is what stops a
    /// `release` racing this call from tearing the child down between "the
    /// set went from empty to non-empty" and "the child is actually up".
    pub async fn acquire(&self, run_id: &str) {
        let mut active = self.active.lock().await;
        let was_empty = active.is_empty();
        active.insert(run_id.to_string());
        if was_empty {
            self.inner.hold().await;
        }
    }

    /// A run just ended, however it ended: let go of its share. Safe to call
    /// for a run id `acquire` was never handed -- dispatch failing before
    /// the run row existed, for instance -- since removing an id that was
    /// never in the set is simply a no-op. That is what lets
    /// `Engine::close_session`, the one place every terminal path already
    /// funnels through, call this unconditionally rather than each caller
    /// having to know whether its particular run got as far as acquiring
    /// one.
    pub async fn release(&self, run_id: &str) {
        let mut active = self.active.lock().await;
        if active.remove(run_id) && active.is_empty() {
            self.inner.let_go().await;
        }
    }

    /// How many runs are currently holding the assertion open. Test-only:
    /// nothing in the engine needs to ask this, only tests proving the
    /// refcounting itself -- `pub(crate)` because `engine.rs`'s own tests
    /// call it directly on `Engine::power`.
    #[cfg(test)]
    pub(crate) async fn active_count(&self) -> usize {
        self.active.lock().await.len()
    }

    #[cfg(test)]
    fn with_assertion(inner: Arc<dyn Assertion>) -> Self {
        Self { inner, active: Mutex::new(HashSet::new()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeAssertion {
        held: AtomicUsize,
        released: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl Assertion for FakeAssertion {
        async fn hold(&self) {
            self.held.fetch_add(1, Ordering::SeqCst);
        }
        async fn let_go(&self) {
            self.released.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn a_single_run_holds_once_and_releases_once() {
        let fake = Arc::new(FakeAssertion::default());
        let assertions = PowerAssertions::with_assertion(fake.clone());

        assertions.acquire("run-1").await;
        assert_eq!(fake.held.load(Ordering::SeqCst), 1);
        assert_eq!(assertions.active_count().await, 1);

        assertions.release("run-1").await;
        assert_eq!(fake.released.load(Ordering::SeqCst), 1);
        assert_eq!(assertions.active_count().await, 0);
    }

    #[tokio::test]
    async fn concurrent_runs_share_one_assertion() {
        let fake = Arc::new(FakeAssertion::default());
        let assertions = PowerAssertions::with_assertion(fake.clone());

        assertions.acquire("run-1").await;
        assertions.acquire("run-2").await;
        assert_eq!(fake.held.load(Ordering::SeqCst), 1, "the second run finds the assertion already up");
        assert_eq!(assertions.active_count().await, 2);

        assertions.release("run-1").await;
        assert_eq!(fake.released.load(Ordering::SeqCst), 0, "run-2 is still active");

        assertions.release("run-2").await;
        assert_eq!(fake.released.load(Ordering::SeqCst), 1, "the last run out lets it go");
    }

    #[tokio::test]
    async fn releasing_the_same_run_twice_is_a_harmless_no_op() {
        // What `close_session` being reachable twice for one run (a report
        // racing the watchdog) looks like from here.
        let fake = Arc::new(FakeAssertion::default());
        let assertions = PowerAssertions::with_assertion(fake.clone());

        assertions.acquire("run-1").await;
        assertions.release("run-1").await;
        assertions.release("run-1").await;

        assert_eq!(fake.released.load(Ordering::SeqCst), 1, "a run cannot let go of it twice");
    }

    #[tokio::test]
    async fn releasing_a_run_that_never_acquired_touches_nothing() {
        // What a dispatch failing before the run row existed looks like from
        // here: `close_session` still calls `release` unconditionally.
        let fake = Arc::new(FakeAssertion::default());
        let assertions = PowerAssertions::with_assertion(fake.clone());

        assertions.release("never-acquired").await;

        assert_eq!(fake.held.load(Ordering::SeqCst), 0);
        assert_eq!(fake.released.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn disabled_never_touches_the_platform_assertion() {
        let assertions = PowerAssertions::new(false);
        assertions.acquire("run-1").await;
        assertions.release("run-1").await;
        // No fake to assert on here -- the point is only that `new(false)`
        // does not reach for `platform()`, which on a non-macOS build is
        // `NoAssertion` anyway and on macOS would otherwise try to spawn a
        // real `caffeinate`. Reaching this line without hanging or spawning
        // anything is the assertion.
        assert_eq!(assertions.active_count().await, 0);
    }
}
