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

/// Parse the command line, run the command, and return the process exit code.
#[must_use]
pub fn run() -> i32 {
    eprintln!("factory: station 10 is under construction");
    70
}
