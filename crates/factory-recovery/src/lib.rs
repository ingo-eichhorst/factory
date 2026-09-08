//! Reconciliation after a restore or a restart (design §5; ADR 0019;
//! backlog §9).
//!
//! Skeleton only. The coordinator owns this file's module declarations, the
//! crate manifest, and the decisions recorded below, so that concurrent work
//! never collides in a centrally owned file; every other line belongs to the
//! slice's own agents.
//!
//! # The split this crate sits on
//!
//! ADR 0019 draws the line and this crate keeps it: **that ADR says what a
//! database may claim on its own; this slice says what live evidence is
//! allowed to change about that claim.** [`restore`] is the first half and
//! never asks Herdr anything (ADR 0019 decision 3 — a snapshot may be hours
//! old, so a pane that exists is not evidence that *this* session exists).
//! [`reconnect`] is the second half and is the only place a live
//! observation is permitted to move a session at all.
//!
//! # Decisions the coordinator made before this crate was written
//!
//! **1. There is no `factory` binary yet.** ADR 0019 decision 1 describes
//! `factory restore <snapshot>` as a command, and backlog §10 is where
//! commands are built. This crate delivers the reconciliation *function* and
//! the report *writer*; tests drive them directly, exactly as
//! `docs/slice-1-operator-guide.md` drives library crates with no executable
//! in front of them. An agent that builds a command has built slice 10.
//!
//! **2. `Starting → Disconnected` does not go into the global transition
//! table.** ADR 0019 decision 2 requires the edge and, in the same
//! paragraph, requires that "[t]he edge is not opened for any other caller."
//! `factory_session::valid_targets` is global, so the table cannot express
//! both — and `starting_cannot_go_directly_to_disconnected`
//! (`factory-session/tests/state_machine.rs`) pins the missing edge today,
//! alongside `illegal_transitions_are_rejected` in the same file.
//! (An earlier draft of this comment also named
//! `the_deliberately_missing_edge_is_absent` and
//! `valid_targets_matches_the_design_table`; agent 9A found that both are
//! `factory-task` tests over `TaskStatus`, not session tests. The
//! misattribution is corrected here rather than left for the next reader to
//! trip over.) Weakening the table would
//! demote the ADR's restriction to a comment. Instead `factory_session`
//! gains one dedicated, named entry point that performs the restore write,
//! and the general table stays exactly as it is with all three tests intact.
//! This is a deliberate deviation from the ADR's stated *mechanism* ("adding
//! ... to `factory_session`'s transition table") that honours both of its
//! stated *requirements*; ADR 0019 is amended to record it.
//!
//! **3. "Only authoritative evidence may move a session out of
//! `disconnected`" has exactly one home.** It lives in [`evidence`], is
//! `pub`, and every restart class cites it rather than re-deriving it.
//! `factory_adapter::Confidence` already distinguishes an observation the
//! harness *reported* through the Herdr lifecycle hook from one Herdr
//! *inferred* by reading the pane; only the former may promote a session.
//! This codebase has now paid twice for a rule with several homes — the
//! lease-holding set in slice 7, ancestry in slice 8 — and the mutation that
//! catches it is deleting the predicate and checking that a test dies in
//! *every* class claiming to depend on it.
//!
//! **4. Reconciliation is idempotent, and that is load-bearing.** ADR 0019
//! decision 1 accepts that an operator who copies a snapshot into place by
//! hand gets stale live-looking rows with no warning, and the only reason
//! that is acceptable is that running the reconciliation afterwards reaches
//! the same state. Running it twice must change nothing the second time.
//!
//! **5. `factory-store` belongs to one agent this slice.** Both schema
//! changes below are that agent's; nothing else in this slice may append a
//! migration.
//!
//! # The schema this slice adds, and why
//!
//! **Migration 4 — an authorised delivery is recorded, not passed.** Slice 7's
//! `deliver` refuses any task that already carries a `delivery_attempts`
//! row, which is the correct reading of design §5's "does not automatically
//! resend a possibly delivered prompt". The consequence found during slice 7
//! is that a resumed task can never be re-delivered, only replaced. A human
//! authorising one further delivery is not the automatic resend §5 forbids —
//! but the permission has to be *recorded with the task*, or a restart
//! cannot tell an authorised second delivery from an accidental one.
//!
//! **Migration 5 — a session records how to find its harness again.**
//! `sessions` today carries no Herdr pane id and no harness session id at
//! all (`grep -ic pane crates/factory-store/src/schema.rs` returns 0), so
//! backlog §9's "known panes are reconnected where possible" is not
//! expressible. `factory_adapter` already knows how to parse a harness
//! session id and already has a `PaneId`; nothing has ever persisted either.
//! Recording them is what makes reconnection a lookup rather than a guess —
//! and ADR 0019 decision 3's warning that a stale pane id may since have been
//! reused is precisely why the id alone never suffices, and why [`evidence`]
//! exists.

pub mod evidence;
pub mod harness;
pub mod reconnect;
pub mod restore;
