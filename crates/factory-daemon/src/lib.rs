//! The Factory supervisor: the one process that mutates core state, and the
//! client half of the socket its CLI speaks to.
//!
//! ADR 0014 settled the question slices 1–9 were able to leave open: Factory
//! runs as a long-running daemon and the CLI is a client. Three things forced
//! it, and all three are now built: a harness session outlives the command that
//! started it, at-most-once delivery needs a custodian present across the
//! ambiguous window, and reconciliation is continuous rather than on demand.
//!
//! # Binding decisions for station 10
//!
//! **1. The daemon is the sole mutator.** Every command that changes state is
//! submitted over the socket. [`factory_doctor`] is the single exception and it
//! is read-only, because "the daemon is down" is exactly the condition an
//! operator runs it to diagnose (ADR 0014, consequences).
//!
//! **2. Request/response only.** ADR 0002 also specifies an event stream,
//! idempotency keys, an outbox and CQRS projections. Per ADR 0010 decision 3,
//! anything in a project ADR beyond version-1 scope is target-state unless the
//! backlog carries a slice for it, and the backlog carries none for those. This
//! crate implements the request/response half: one newline-delimited JSON
//! object in, one out, over a Unix socket at `.factory/factory.sock`.
//!
//! **3. Threads, not an async runtime.** Blocking `accept`, one thread per
//! connection. Mutations serialise through `BEGIN IMMEDIATE` and `busy_timeout`
//! exactly as they already did, which ADR 0012 measured as sufficient under
//! concurrent mutators. An async runtime would be a large dependency bought to
//! solve a problem the store already solves.
//!
//! **4. The installation lock is real now.** ADR 0012 deferred an exclusive
//! lock on `.factory/factory.lock` on the grounds that it is only meaningful
//! once something long-lived exists to hold it. Something does. The daemon
//! holds it for its lifetime; a second daemon fails to start with an actionable
//! error rather than quietly competing.
//!
//! **5. The adapter lives here.** ADR 0014's prerequisite for slice 5: the
//! adapter runs inside the daemon and `observe` is a loop the daemon owns, not
//! a one-shot call a command makes.
//!
//! **6. A completion notice is not a message primitive.** When a delegated task
//! reaches a terminal state, the delegating session is told — through the same
//! journal-then-write delivery path, carrying nothing but the task id and its
//! outcome. The task record remains the only durable unit of work exchanged, so
//! design §2.4 and ADR 0010 decision 2 still hold: there is no message store,
//! no thread, and nothing to read back other than the task.
//!
//! **7. `idempotency_key` is deliberately absent from the wire envelope.**
//! ADR 0003 §2 lists it as a required command field. Honouring it needs a
//! dedup table keyed on `(principal, scope, command, idempotency_key)` that no
//! backlog slice carries yet, and a field this crate accepted but silently
//! discarded would be worse than one that is simply not there: a retrying
//! client would believe a repeated request had been deduplicated when it had
//! not. [`envelope::CommandRequest`] therefore has no such field, and this
//! crate neither parses nor rejects one a client happens to send anyway — see
//! [`envelope`]'s module docs and its
//! `an_idempotency_key_sent_by_a_client_is_silently_ignored_not_rejected`
//! test. A later station adds the field and the dedup table together, never
//! one without the other.
//!
//! **8. This crate is the transport, not the supervisor.** [`server::Daemon`]
//! binds the socket and runs the accept loop; every parsed request is handed
//! to a [`server::Handler`] the *caller* supplies. This crate defines that
//! trait and implements it nowhere — decision 5 already places the adapter
//! inside the daemon, and a `Handler` wired to the store, the registry, and
//! the adapter is what a later station plugs in here. Station 10's own tests
//! use stub handlers for exactly this reason.

//! # Decision 9: the operation vocabulary is fixed here
//!
//! The supervisor answers these operations and the CLI sends exactly these
//! names. Both halves were built in parallel, so the list is written down in
//! one place rather than agreed twice and found to differ.
//!
//! Commands (may change state):
//!
//! | Operation | Design §7 command |
//! |---|---|
//! | `scope.add` | `factory scope add` |
//! | `scope.reconcile` | `factory scope reconcile` |
//! | `agent.start` | `factory agent start` |
//! | `agent.stop` | `factory agent stop` |
//! | `task.send` | `factory task send` |
//! | `task.cancel` | `factory task cancel` |
//! | `task.done`, `task.fail`, `task.block` | `factory task done\|fail\|block` |
//! | `task.resume` | `factory task resume` |
//!
//! Queries (read only):
//!
//! | Operation | Design §7 command |
//! |---|---|
//! | `scope.list` | `factory scope list` |
//! | `agent.status` | `factory agent status` |
//! | `agent.attach_command` | `factory agent attach` |
//! | `task.list`, `task.show` | `factory task list\|show` |
//! | `task.wait` | `factory task send --wait` |
//! | `context.show` | `factory context show` |
//! | `daemon.status` | `factory status` |
//!
//! Three of these need saying out loud.
//!
//! **`factory init` is not here, because it cannot be.** It creates the
//! configuration and the database that a daemon needs before one can run. It
//! is the only command that does its work locally, and `factory doctor` is
//! the only other command that runs without a daemon.
//!
//! **`task.resume` extends design §7**, which lists `task send|list|show|
//! cancel`. Slice 9 built `factory_task::deliver::authorise_resume` — a human
//! authorising one further delivery of a task that a restart left `blocked:
//! interrupted` — and left no way to invoke it. A mechanism with no caller is
//! not a safety feature, so the subcommand is added here rather than leaving
//! slice 9's recovery path reachable only from a test.
//!
//! **`task.wait` is bounded, and timing out changes nothing.** Herdr's own
//! `--wait` is indefinite without a `--timeout`, and this daemon serves one
//! thread per connection, so an unbounded wait is a held thread and a held
//! socket rather than only a slow command. `task.wait` therefore takes a
//! deadline, and the CLI supplies a default rather than blocking forever.
//!
//! Expiring is not a failure of the task, and this is the load-bearing half:
//! waiting is a *read*, so a wait that times out must leave the task exactly
//! as it found it — still queued, still running, still delegated. The caller
//! is told the task id and can ask again with `task show`. A timeout that
//! cancelled, blocked or resent the task would make a read into a mutation,
//! and would break at-most-once delivery for the resend case.
//!
//! **`agent.attach_command` returns argv; the CLI execs it.** The daemon
//! cannot hand an operator's terminal to a harness from inside a worker
//! thread, so the adapter says what to run and the client runs it.
//!
//! # Station 10's `Handler`: [`handler::FactoryHandler`]
//!
//! [`handler::FactoryHandler`] is the `Handler` this crate's own docs (decision
//! 8) said a later station plugs in. It wraps one `factory_store::Store`
//! behind a `Mutex` (mutations still serialise through SQLite's own
//! `BEGIN IMMEDIATE`, per decision 3 — the mutex is only what keeps one
//! process's threads from interleaving two `&mut Store` borrows) and one
//! `Adapter`, and dispatches decision 9's operation names to the domain crates
//! that already hold every rule this crate is forbidden from re-deriving.
//!
//! `event_cursor` (ADR 0003 §4) has no event store to be a real position in —
//! decision 2 already says that store is target-state. [`handler::FactoryHandler`]
//! reports a per-process counter, incremented once per successful command and
//! left unchanged by a query, which satisfies the wire shape without
//! pretending to a replay position nothing here can produce.
//!
//! ## Payload contracts
//!
//! Decision 9 fixes operation *names*; nothing upstream fixes their JSON
//! shape, so this crate does, one field per domain-function parameter it
//! wires to — the coordination mechanism is this table, exactly as decision 9
//! itself is. An unrecognised or missing required field is
//! `validation.missing_field` / `validation.malformed_payload`, naming the
//! field, never a silent default (a flag with a genuine CLI-optional meaning,
//! such as `scope.reconcile`'s `apply`, is the one exception, and is called
//! out below).
//!
//! Every command and query's envelope carries `scope_id` (ADR 0003 §§2–3);
//! this crate reads it as *the scope the operation concerns* — the target
//! scope for `scope.*`/`agent.*`/`task.send`/`context.show`, and the scope a
//! listing is filtered by for `agent.status`.
//!
//! | Operation | Payload | Result |
//! |---|---|---|
//! | `scope.add` | *(any — never reached)* | Always `internal.scope_add_unsupported`: this crate has no YAML writer for `.factory/config.yaml` and ADR 0009 requires unknown top-level keys be preserved, which a hand-rolled writer cannot safely guarantee (see `handler::config` module docs). Edit the file directly, then call `scope.reconcile` with `apply: true`. |
//! | `scope.reconcile` | `{ "apply": bool = false }` | `{ "clean": bool, "drift": [Drift], "applied": [Drift] }` |
//! | `scope.list` | `{}` | `{ "scopes": [RegisteredScope-shaped rows, read directly from `scopes`] }` |
//! | `agent.start` | `{ "session_id": Uuid, "agent_name": String, "workspace_path": String? }` (workspace defaults to the scope's own canonical path) | `{ "session_id", "state": "starting"\|"running", "pane": String?, "confidence": String?, "harness_session_id": String? }` |
//! | `agent.stop` | `{ "session_id": Uuid, "reason": String? }` | `{ "session_id", "state": "stopped"\|"failed", "interrupted_task_id": Uuid? }` |
//! | `agent.status` | `{ "session_id": Uuid? }` | `{ "session": ..., "leases": [...] }` when `session_id` is given, else `{ "sessions": [...] }` filtered to the envelope's `scope_id` |
//! | `agent.attach_command` | `{ "session_id": Uuid }` | `{ "argv": [String] }` |
//! | `task.send` | `{ "task_id": Uuid, "prompt": String, "sender_session_id": Uuid?, "target_session_id": Uuid?, "target_workspace_path": String?, "agent_name": String? }` (`agent_name` is required unless `target_session_id` is given — `factory_task::assign`'s own doc comment records that `tasks` carries no agent-name column, so nothing durable resolves it for an untargeted or workspace-targeted send; this is that gap, worked around by asking the caller) | `{ "task_id", "status", "assignment": {"kind": "assigned"\|"start_session_at"\|"deferred", ...}, "delivery": {"sent": bool, "error": String?}? }` |
//! | `task.cancel` | `{ "task_id": Uuid }` | `{ "task_id", "status": "cancelled"\|"running" }` |
//! | `task.done` / `task.fail` | `{ "task_id": Uuid, "result_summary": String?, "result_artifact_paths": [String]? }` | `{ "task_id", "status" }` |
//! | `task.block` | `{ "task_id": Uuid, "reason": "clarification"\|"permission"\|"interrupted"\|"external" }` | `{ "task_id", "status": "blocked", "blocked_reason" }` |
//! | `task.resume` | `{ "task_id": Uuid }` | `{ "task_id", "status": "queued" }` |
//! | `task.list` | `{}` | `{ "tasks": [Task] }` |
//! | `task.show` | `{ "task_id": Uuid }` | `{ ...Task, "delegation_chain": [Uuid] }` |
//! | `task.wait` | `{ "task_id": Uuid, "timeout_ms": u64? }` (default and maximum in `handler::task`) | `{ "task_id", "status", "timed_out": bool }` — never mutates; see the module docs above |
//! | `context.show` | `{ "agent_name": String, "task_prompt": String? }` | `{ "text": String, "sources": [SourceReport] }` |
//! | `daemon.status` | `{}` | `{ "schema_version": i64, "socket_path": String, "lock_path": String }` |
//!
//! ## The delegation-completion notice ([`notify`])
//!
//! Crate docs decision 6: when a task with `sender_scope_id.is_some()` (design
//! §6 — a human-queued task's own chain is `[target]`, non-empty but sent by
//! nobody a notice can reach; `sender_scope_id` is what actually distinguishes
//! "delegated" from merely "chained") reaches a terminal state
//! (`task.done`, `task.fail`, or `task.cancel`'s immediate path — never
//! `task.block`, which is not terminal), the delegating scope's one `running`
//! session is told, through the same journal-then-write ordering
//! [`factory_task::deliver::deliver`] uses, carrying nothing but the task id
//! and its outcome. No delegating session found, or [`factory_adapter::Adapter::send`]
//! refuses [`factory_adapter::AdapterError::SessionBusy`], both skip the
//! notice silently — the task record remains the durable truth, and
//! `task.show` still answers.
//!
//! ## The observe loop ([`observe`])
//!
//! A background loop, owned by this crate (ADR 0014's Slice 5 prerequisite),
//! polls every `starting`/`running` session's recorded pane and reconciles
//! what [`factory_adapter::Adapter::observe`] reports with `factory_session`
//! and `factory_task` — never closing a task on `idle` or `done`
//! ([`factory_adapter::TaskSignal`]'s own vocabulary), and reusing
//! [`factory_recovery::harness::harness_crashed`] rather than re-deriving its
//! evidence table when a `running` session's pane reports the process gone.
//!
//! ## Startup ([`startup`])
//!
//! [`startup::build_handler`] runs [`factory_recovery::restore::reconcile`]
//! before it hands back a [`handler::FactoryHandler`] — ADR 0014's own open
//! item that a restarted daemon must treat "sessions to re-adopt" as normal,
//! not as a cold start.

pub mod client;
pub mod envelope;
mod errors;
pub mod handler;
pub mod lock;
mod notify;
pub mod observe;
mod ops;
mod serde_uuid;
pub mod server;
mod socket;
pub mod startup;

pub use client::{ClientError, ErrorFamily, RemoteError, call};
pub use envelope::{
    COMMAND_API, CommandRequest, ERROR_API, ErrorBody, ErrorEnvelope, ParseFailure, QUERY_API,
    QueryRequest, RESPONSE_API, Request, Response, SuccessResponse,
};
pub use handler::FactoryHandler;
pub use lock::{InstallationLock, LockError};
pub use server::{Daemon, DaemonError, Handler, HandlerOutcome, HandlerSuccess};
pub use socket::SocketError;

use std::path::{Path, PathBuf};

/// Where the installation lock lives beneath an instance root.
///
/// The one place `.factory/factory.lock` is joined together, so
/// [`Daemon::start`] and anything else that needs to name the lock file
/// without starting a daemon — `factory doctor`'s diagnosis included — stay
/// in sync with each other.
#[must_use]
pub fn lock_path(instance_root: impl AsRef<Path>) -> PathBuf {
    instance_root.as_ref().join(".factory").join("factory.lock")
}

/// Where the daemon's socket lives beneath an instance root.
///
/// The one place `.factory/factory.sock` is joined together — see
/// [`lock_path`]'s docs for why that matters.
#[must_use]
pub fn socket_path(instance_root: impl AsRef<Path>) -> PathBuf {
    instance_root.as_ref().join(".factory").join("factory.sock")
}

/// Where the instance configuration this crate reads (never writes — see
/// `handler::config`'s doc comment) lives beneath an instance root.
#[must_use]
pub fn config_path(instance_root: impl AsRef<Path>) -> PathBuf {
    instance_root.as_ref().join(".factory").join("config.yaml")
}
