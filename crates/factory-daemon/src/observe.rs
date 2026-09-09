//! The daemon-owned observe loop — ADR 0014's Slice 5 prerequisite: "the
//! adapter runs inside the daemon, and its `observe()` is a loop the daemon
//! owns rather than a one-shot call." [`reconcile_once`] is one pass, run
//! against every `starting`/`running` session's recorded pane
//! ([`crate::handler::pane`]); [`spawn`] repeats it on an interval until told
//! to stop.
//!
//! **No harness state closes a task.** [`factory_adapter::TaskSignal`]'s own
//! vocabulary is `NoChange | Running | Blocked(reason)` — there is no
//! variant for "done," and neither `idle` nor `done` (Herdr's own words for
//! harness state, carried verbatim in [`factory_adapter::Observation::harness_state`])
//! means finished. The only write a `running` session's observation can ever
//! produce here is moving its currently-running task to `blocked`, through
//! [`factory_task::complete::blocked_from_observation`] — which itself only
//! acts on [`factory_adapter::Confidence::Authoritative`] evidence, never on
//! a `Degraded` or `Unavailable` reading. A task closes only through
//! `task.done`, `task.fail`, or `task.cancel` — the push path an agent uses
//! to report its own result (`ops::task`), never this loop.
//!
//! A `running` session whose pane authoritatively reports the process gone
//! reuses [`factory_recovery::harness::harness_crashed`] rather than
//! re-deriving its evidence table (crate docs' own instruction: a rule
//! stated once in a domain crate is wired to, not restated).

use factory_adapter::{Adapter, Confidence, PaneId};

/// What one session's pass concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionOutcome {
    /// No pane has been recorded for this session yet — nothing to observe.
    NoPaneRecorded,
    /// The adapter itself failed, or a domain write failed; `detail` is a
    /// human-readable reason. One session's failure never aborts the pass.
    ObserveFailed(String),
    /// `starting` → `running`: an authoritative, alive observation confirmed
    /// readiness.
    PromotedToRunning,
    /// `starting` → `failed`: an authoritative observation reported the
    /// process gone before it was ever confirmed running.
    StartingNeverConfirmed,
    /// Nothing about this session's or its task's state changed this pass.
    NoChange,
    /// The session's running task moved to `blocked`, per
    /// [`factory_task::blocked_reason_for`]'s own authoritative-only gate.
    TaskBlocked {
        task_id: uuid::Uuid,
        reason: factory_task::BlockedReason,
    },
    /// [`factory_recovery::harness::harness_crashed`]'s own verdict for a
    /// `running` session whose pane no longer confirms it alive.
    HarnessCrash(factory_recovery::harness::CrashOutcome),
}

/// One session's outcome from one [`reconcile_once`] pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionReport {
    pub session_id: uuid::Uuid,
    pub outcome: SessionOutcome,
}

/// One reconciliation pass over every `starting`/`running` session.
///
/// A `disconnected` session is deliberately not visited here: reconnecting
/// one is `factory_recovery::reconnect`'s job (a restart-class action this
/// crate's startup path — `restore::reconcile` — feeds into, not a loop
/// concern), and `stopped`/`failed` sessions have nothing left to observe.
///
/// Never panics and never aborts the pass for one session's failure — the
/// same "a bad row must not truncate the scan" stance
/// `factory_session::find_aliasing_conflict` documents for its own scan.
pub fn reconcile_once(
    store: &mut factory_store::Store,
    adapter: &(dyn Adapter + Send + Sync),
) -> Vec<SessionReport> {
    let sessions = match factory_session::list(store) {
        Ok(sessions) => sessions,
        Err(_) => return Vec::new(),
    };

    let mut reports = Vec::with_capacity(sessions.len());
    for session in sessions {
        let outcome = match session.state {
            factory_session::SessionState::Starting => {
                reconcile_starting(store, adapter, session.id)
            }
            factory_session::SessionState::Running => reconcile_running(store, adapter, session.id),
            factory_session::SessionState::Disconnected
            | factory_session::SessionState::Stopped
            | factory_session::SessionState::Failed => continue,
        };
        reports.push(SessionReport {
            session_id: session.id,
            outcome,
        });
    }
    reports
}

fn observe(
    store: &factory_store::Store,
    adapter: &(dyn Adapter + Send + Sync),
    session_id: uuid::Uuid,
) -> Result<Option<factory_adapter::Observation>, SessionOutcome> {
    let (pane, _) = crate::handler::pane::read(store, session_id)
        .map_err(|e| SessionOutcome::ObserveFailed(e.message))?;
    let Some(pane) = pane else {
        return Ok(None);
    };
    match adapter.observe(&PaneId(pane)) {
        Ok(observation) => Ok(Some(observation)),
        Err(e) => Err(SessionOutcome::ObserveFailed(e.to_string())),
    }
}

fn reconcile_starting(
    store: &mut factory_store::Store,
    adapter: &(dyn Adapter + Send + Sync),
    session_id: uuid::Uuid,
) -> SessionOutcome {
    let observation = match observe(store, adapter, session_id) {
        Ok(Some(o)) => o,
        Ok(None) => return SessionOutcome::NoPaneRecorded,
        Err(outcome) => return outcome,
    };

    if observation.confidence != Confidence::Authoritative {
        // Degraded/Unavailable: falls back to manual confirmation, per
        // `StartedSession::confidence`'s own doc comment — nothing to write.
        return SessionOutcome::NoChange;
    }

    if observation.session_alive {
        match factory_session::mark_running(store, session_id) {
            Ok(()) => SessionOutcome::PromotedToRunning,
            Err(e) => SessionOutcome::ObserveFailed(e.to_string()),
        }
    } else {
        match factory_session::fail(
            store,
            session_id,
            "observe loop: pane authoritatively reported gone before ever confirming running",
        ) {
            Ok(()) => SessionOutcome::StartingNeverConfirmed,
            Err(e) => SessionOutcome::ObserveFailed(e.to_string()),
        }
    }
}

fn reconcile_running(
    store: &mut factory_store::Store,
    adapter: &(dyn Adapter + Send + Sync),
    session_id: uuid::Uuid,
) -> SessionOutcome {
    let observation = match observe(store, adapter, session_id) {
        Ok(Some(o)) => o,
        Ok(None) => return SessionOutcome::NoPaneRecorded,
        Err(outcome) => return outcome,
    };

    if !observation.session_alive {
        return match factory_recovery::harness::harness_crashed(store, session_id, &observation) {
            Ok(outcome) => SessionOutcome::HarnessCrash(outcome),
            Err(e) => SessionOutcome::ObserveFailed(e.to_string()),
        };
    }

    // Alive: the only thing left to reconcile is whether the currently
    // running task should move to `blocked` — never `done`, never anything
    // this loop infers from `harness_state` going quiet (see module docs).
    match factory_task::assign::running_task_of_session(store, session_id) {
        Ok(Some(task_id)) => {
            match factory_task::complete::blocked_from_observation(store, task_id, &observation) {
                Ok(Some(reason)) => SessionOutcome::TaskBlocked { task_id, reason },
                Ok(None) => SessionOutcome::NoChange,
                Err(e) => SessionOutcome::ObserveFailed(e.to_string()),
            }
        }
        Ok(None) => SessionOutcome::NoChange,
        Err(e) => SessionOutcome::ObserveFailed(e.to_string()),
    }
}

/// Run [`reconcile_once`] on `interval` until `stop` is signalled (or
/// dropped). Returns the join handle and the stop sender.
pub fn spawn(
    handler: std::sync::Arc<crate::handler::FactoryHandler>,
    interval: std::time::Duration,
) -> (std::thread::JoinHandle<()>, std::sync::mpsc::Sender<()>) {
    let (stop_tx, stop_rx) = std::sync::mpsc::channel();

    let join = std::thread::spawn(move || {
        loop {
            {
                let mut store = handler.lock_store();
                let _ = reconcile_once(&mut store, handler.adapter());
            }
            match stop_rx.recv_timeout(interval) {
                Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    });

    (join, stop_tx)
}
