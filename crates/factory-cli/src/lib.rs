//! The `factory` command surface (design §7).
//!
//! # Binding decisions for station 10
//!
//! **1. One binary, two modes.** ADR 0002 decision 1: `factory daemon run` is
//! the long-running kernel; every other invocation is an API client of it.
//! There is no second executable and no privileged path — a command cannot
//! reach the database by being run differently.
//!
//! **2. Station 10 implements part of design §7, and stubs none of the rest.**
//! In scope: `init`, `start|stop|status|doctor`, `scope`, `agent`, `task`,
//! `context`. Out of scope and therefore absent: `secret` (station 13),
//! `schedule` (station 11), `knowledge` and `memory` and `agent list`
//! (station 12). A stub that exits non-zero is still a command that exists; an
//! operator reads it as broken rather than as unbuilt.
//!
//! **3. `factory task done|fail|block` is how an agent reports back.** It is a
//! push from inside the session and it is not an observation: design §3 lists
//! "capturing the final response when the harness exposes it" among adapter
//! duties, and design §5 step 5 stores the final state and result. The agent's
//! own report is what closes a task. No harness state ever does — the rule in
//! `factory_adapter::TaskSignal` is unchanged by this station.
//!
//! **4. `--wait` blocks until the task is terminal,** which is what design §7's
//! `factory task send --wait` promises, and is how a caller — human or
//! delegating agent — learns an answer without polling.
//!
//! # Decisions recorded while implementing station 10
//!
//! **5. Ids are minted here, genuinely as UUIDv7 — see [`ids`].** ADR 0003 §1
//! requires UUIDv7 stable ids, and `uuid` is pinned workspace-wide without
//! the `v4` or `v7` feature (the workspace manifest is centrally owned), so
//! neither convenience constructor compiles in this crate any more than it
//! does in `factory_task::create::create` or `factory_recovery`'s
//! `reconnect_after_herdr_or_machine_restart`, whose own doc comments record
//! the same constraint and the same conclusion: the caller mints the id.
//! This crate is that caller for every `request_id` it sends, and for every
//! `task_id`/`session_id` a command introduces (`task send`, `agent start`)
//! — decision 9's payload table lists both as caller-supplied, never
//! daemon-generated. [`ids::new_id`] builds a real, spec-shaped UUIDv7
//! through `uuid::Builder::from_unix_timestamp_millis`, which — checked
//! directly against the `uuid` crate's own source — is not behind the
//! feature gate that only the convenience constructors sit behind.
//!
//! **6. `factory daemon run` chooses the adapter, and nothing else does.**
//! `--harness` decides which one, because a daemon serves one instance and
//! that instance's sessions run on one harness. Inferring it per request
//! would put design §2.2's `harness:` rule in a second place.
//!
//! Only `pi` and `claude-code` have adapters. `factory-config` also accepts
//! `opencode` — backlog §1 widened the list beyond design §2.2 because
//! `model-lab` genuinely runs it — so a daemon told to serve a harness with
//! no adapter says so and stops, rather than starting and failing on the
//! first session.
//!
//! Startup order is load-bearing: `build_handler` reconciles the restored
//! database before the socket ever answers (ADR 0014's open item — a
//! restarted daemon re-adopts rather than cold-starts), and the observe loop
//! starts only after that, so it can never read a session in the state a
//! crash left behind.
//!
//! **7. Detecting a running daemon is always a socket call ([`rpc::ping`]),
//! never a probe of the installation lock.** `factory_daemon::InstallationLock::acquire`
//! leaks a file descriptor on every call by design — harmless once, for a
//! daemon's own lifetime; a leak per invocation for anything else. The one
//! place this crate reads `.factory/factory.lock` at all is `factory stop`,
//! and only for the pid the daemon itself already recorded there, to name a
//! signal target — never to test whether the lock can be acquired.
//!
//! **8. Root resolution matches how `factory task done|fail|block` is
//! actually invoked — see [`root`].** That command runs from inside an
//! agent's own session, whose current directory is its workspace, a
//! descendant of the instance root (design §4: one `.factory/` at the
//! company root, never duplicated per scope). So every command but `init`
//! resolves its root by, in order: `--root`, the `FACTORY_ROOT` environment
//! variable, or walking up from the current directory for the nearest
//! ancestor holding `.factory/`. `factory init` alone defaults to the
//! current directory instead (ADR 0009 §3b: its root need not exist yet).

mod cli;
mod commands;
pub mod exit;
mod ids;
mod root;
mod rpc;
mod scope_ref;

/// Parse the command line, run the command, and return the process exit code.
#[must_use]
pub fn run() -> i32 {
    run_with(std::env::args())
}

/// [`run`], taking an explicit argument iterator — real `clap` parsing and
/// dispatch, exercised without a subprocess.
#[must_use]
pub fn run_with(args: impl IntoIterator<Item = String>) -> i32 {
    use clap::Parser;
    match cli::Cli::try_parse_from(args) {
        Ok(cli) => commands::dispatch(cli),
        Err(error) => {
            let _ = error.print();
            error.exit_code()
        }
    }
}
