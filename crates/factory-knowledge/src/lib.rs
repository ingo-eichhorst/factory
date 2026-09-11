//! The two durable writing paths (design §7, backlog §12): the shared note
//! graph and scope-local memory (ADR 0022).
//!
//! This crate is pure domain logic. It takes paths and values and returns
//! results — no database, no daemon, no CLI, no RPC. `factory-daemon` is
//! the one process that opens a `durable_writes` transaction (ADR 0022
//! decision 1) and calls into [`note::stage`] / [`memory::stage_entry`]
//! between opening it and committing it (decision 3); nothing in this
//! crate knows that transaction exists.
//!
//! # Why one crate, two modules ([`note`] and [`memory`])
//!
//! Design §7 separates the *material* — [`note`] is shared and sourced,
//! [`memory`] is scope-local and unsourced — and that separation stays at
//! the surface: two modules, two on-disk layouts, two commands above them.
//! It does not repeat at the level of the atomic write. [`note::Staged`] is
//! the one primitive that makes a file appear on disk without a
//! half-written state in between, and [`memory::stage_entry`] builds on the
//! exact same type rather than a second, near-identical one — ADR 0022
//! decision 11: written twice, it would be correct in one place and nearly
//! correct in the other. [`memory`] also has no error type of its own; it
//! returns [`note::NoteError`], for the same reason.
pub mod memory;
pub mod note;
