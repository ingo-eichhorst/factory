//! [`FactoryHandler`]: the `Handler` this crate's own docs (decision 8)
//! deliberately left unimplemented. It wires [`server::Handler`] to the
//! domain crates — `factory_registry`, `factory_session`, `factory_task`,
//! `factory_delegation`, `factory_context`, `factory_recovery` — and the one
//! `Adapter` the daemon owns (crate docs decision 5).
//!
//! # Why a `Mutex<Store>`, not a connection pool
//!
//! [`server::Handler::handle_command`] and `handle_query` take `&self` — the
//! same handler is shared (`Arc<dyn Handler>`) across every connection thread
//! `Daemon::serve` spawns (crate docs decision 3). Every domain mutation
//! needs `&mut factory_store::Store` (`Store::transaction`'s own signature),
//! so this crate holds one `Store` behind a `Mutex`. This is an in-process
//! fast path, not a second correctness mechanism: `BEGIN IMMEDIATE` plus
//! `busy_timeout` (ADR 0012 decision 3) is what actually serialises
//! mutations, exactly as it already does across separate OS processes; the
//! mutex only keeps two threads of *this* process from interleaving two
//! `&mut Store` borrows, which the borrow checker would refuse anyway if the
//! `Store` were not behind interior mutability.
//!
//! `task.wait` (see `ops::task::wait`) is the one place that matters
//! operationally: it must never hold this mutex while it sleeps, or every
//! other command — including the `task.done` push that would end the wait —
//! blocks behind it. It polls instead: lock, read, unlock, sleep, repeat.
//!
//! # `event_cursor`
//!
//! ADR 0003 §4 defines it as a position in an event stream this crate's own
//! docs (decision 2) already say does not exist yet. [`FactoryHandler`]
//! reports a per-process counter instead: incremented once per successful
//! *command*, left unchanged by a query. It satisfies the wire shape without
//! pretending to a replay position nothing here can produce, and a client
//! that only compares it for "did anything change since I last asked" gets a
//! correct answer.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use factory_adapter::Adapter;
use serde_json::Value;

use crate::envelope::{CommandRequest, QueryRequest};
use crate::server::{Handler, HandlerOutcome, HandlerSuccess};

pub(crate) mod config;
pub(crate) mod deliver;
pub(crate) mod pane;
pub(crate) mod scope_chain;

/// The `Handler` station 10 plugs into [`crate::server::Daemon::serve`].
pub struct FactoryHandler {
    store: Mutex<factory_store::Store>,
    adapter: Box<dyn Adapter + Send + Sync>,
    instance_root: PathBuf,
    cursor: AtomicU64,
}

impl FactoryHandler {
    /// Build a handler over an already-open `store` and a live `adapter`.
    ///
    /// Does **not** run [`factory_recovery::restore::reconcile`] itself —
    /// that is [`crate::startup::build_handler`]'s job, so a caller that
    /// wants direct control (most tests) is not forced through it.
    pub fn new(
        store: factory_store::Store,
        adapter: impl Adapter + Send + Sync + 'static,
        instance_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            store: Mutex::new(store),
            adapter: Box::new(adapter),
            instance_root: instance_root.into(),
            cursor: AtomicU64::new(0),
        }
    }

    pub(crate) fn lock_store(&self) -> std::sync::MutexGuard<'_, factory_store::Store> {
        // A panic while holding the lock (a bug elsewhere in this process)
        // must not take down every future request with a poisoned mutex —
        // this daemon is meant to keep serving. Recovering the guard is safe
        // here: every write this crate makes goes through `Store::transaction`
        // (`BEGIN IMMEDIATE` ... `commit`), so a panic mid-mutation leaves an
        // uncommitted transaction rolled back by `rusqlite`'s own `Drop`, not
        // a half-written row for a recovered guard to see.
        self.store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn adapter(&self) -> &(dyn Adapter + Send + Sync) {
        self.adapter.as_ref()
    }

    pub(crate) fn instance_root(&self) -> &Path {
        &self.instance_root
    }

    fn next_cursor(&self) -> u64 {
        self.cursor.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn current_cursor(&self) -> u64 {
        self.cursor.load(Ordering::SeqCst)
    }

    /// Build a [`HandlerOutcome`] from a result payload. `mutated` advances
    /// the cursor — pass `true` from a command that committed a write, `false`
    /// from every query and from a command that turned out to be a no-op
    /// (nothing for `task.wait`'s timeout path to ever pass `true` for, since
    /// it must never mutate — see its own doc comment).
    pub(crate) fn success(&self, result: Value, mutated: bool) -> HandlerOutcome {
        let cursor = if mutated {
            self.next_cursor()
        } else {
            self.current_cursor()
        };
        Ok(HandlerSuccess {
            event_cursor: cursor,
            result,
        })
    }
}

impl Handler for FactoryHandler {
    fn handle_command(&self, request: CommandRequest) -> HandlerOutcome {
        dispatch_command(self, &request.command, request.scope_id, request.payload)
    }

    fn handle_query(&self, request: QueryRequest) -> HandlerOutcome {
        dispatch_query(self, &request.query, request.scope_id, request.payload)
    }
}

fn dispatch_command(
    h: &FactoryHandler,
    command: &str,
    scope_id: uuid::Uuid,
    payload: Value,
) -> HandlerOutcome {
    match command {
        "scope.add" => crate::ops::scope::add(h, scope_id, payload),
        "scope.reconcile" => crate::ops::scope::reconcile(h, scope_id, payload),
        "agent.start" => crate::ops::agent::start(h, scope_id, payload),
        "agent.stop" => crate::ops::agent::stop(h, scope_id, payload),
        "task.send" => crate::ops::task::send(h, scope_id, payload),
        "task.cancel" => crate::ops::task::cancel(h, scope_id, payload),
        "task.done" => crate::ops::task::done(h, scope_id, payload),
        "task.fail" => crate::ops::task::fail(h, scope_id, payload),
        "task.block" => crate::ops::task::block(h, scope_id, payload),
        "task.resume" => crate::ops::task::resume(h, scope_id, payload),
        "schedule.create" => crate::ops::schedule::create(h, scope_id, payload),
        "schedule.enable" => crate::ops::schedule::enable(h, scope_id, payload),
        "schedule.disable" => crate::ops::schedule::disable(h, scope_id, payload),
        other => Err(crate::errors::unknown_operation("command", other)),
    }
}

fn dispatch_query(
    h: &FactoryHandler,
    query: &str,
    scope_id: uuid::Uuid,
    payload: Value,
) -> HandlerOutcome {
    match query {
        "scope.list" => crate::ops::scope::list(h, scope_id, payload),
        "agent.status" => crate::ops::agent::status(h, scope_id, payload),
        "agent.attach_command" => crate::ops::agent::attach_command(h, scope_id, payload),
        "task.list" => crate::ops::task::list(h, scope_id, payload),
        "task.show" => crate::ops::task::show(h, scope_id, payload),
        "task.wait" => crate::ops::task::wait(h, scope_id, payload),
        "schedule.list" => crate::ops::schedule::list(h, scope_id, payload),
        "context.show" => crate::ops::context::show(h, scope_id, payload),
        "daemon.status" => crate::ops::status::daemon_status(h, scope_id, payload),
        other => Err(crate::errors::unknown_operation("query", other)),
    }
}
