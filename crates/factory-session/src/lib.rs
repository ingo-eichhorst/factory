//! The session state machine and workspace-lease acquisition.
//!
//! Implements design §2.3 ("Only one live session may lease a given canonical
//! workspace path") and ADR 0012 decision 5 (which states hold a lease:
//! `starting`, `running`, `disconnected`; which release it: `stopped`,
//! `failed`). Schema and DB-level enforcement live in
//! `factory_store::schema` — `sessions.state`'s CHECK constraint and the
//! `sessions_one_live_lease_per_workspace` partial unique index — and this
//! crate is the one place that is allowed to write those rows, so the
//! invariant "the lease-holding rule is applied consistently" reduces to
//! "this crate applies it consistently," rather than being re-derived by
//! every future caller.
//!
//! # Two independent layers, not one
//!
//! ADR 0009's 2026-09-08 correction states that exclusivity needs *both*
//! halves, and neither subsumes the other:
//!
//! 1. **`(st_dev, st_ino)` comparison** — "are these two paths the same
//!    directory *right now*" ([`factory_paths::FileId`]).
//! 2. **Re-canonicalizing a *stored* path before comparing it**, rather than
//!    string-matching what the database returned — "does this registration
//!    still point where it did" ([`factory_paths::CanonicalPath::resolve`]).
//!
//! [`begin_start`] runs a Rust-level scan combining both, over every
//! currently lease-holding session, before it ever attempts an `INSERT`. That
//! scan is deliberately not treated as *the* enforcement mechanism — it turns
//! most conflicts into an early, well-typed [`SessionError::WorkspaceLeased`]
//! instead of a raw constraint error, but `begin_start` still performs the
//! `INSERT` and still propagates whatever the database says, because
//! `sessions_one_live_lease_per_workspace` is what remains true if this
//! scan ever had a bug, were bypassed, or were deleted outright — see
//! `a_second_start_is_blocked_while_the_first_is_only_starting` in
//! `tests/state_machine.rs`, which the task report re-runs with the scan
//! stubbed to `Ok(None)` and shows the rejection still happens, now as a
//! `ConstraintViolation` instead of `WorkspaceLeased`.
//!
//! A case-only rename leaves the inode unchanged but the stored *string*
//! stale — see `case_variant_lease_conflict_spans_a_rename` in
//! `tests/lease.rs` — which the database's default `BINARY` collation cannot
//! see (proved directly against `factory_store` in
//! `crates/factory-store/tests/lease.rs::the_database_index_alone_does_not_see_case_variant_paths`)
//! and which only the Rust scan catches; stub the scan away and that test
//! fails, with the second `begin_start` wrongly succeeding.
//!
//! **Measured, not assumed: delete-and-recreate at the same path is not a
//! case where these two layers disagree, for this crate specifically.**
//! ADR 0009 describes inode comparison against a *persisted* identity going
//! stale across a delete-and-recreate — real for `scopes.dev`/`scopes.ino`
//! and `factory-registry`'s `Drift::PathChangedDifferentIdentity`, which
//! compares a stored inode to a fresh one. This crate persists no inode (see
//! the rejected alternative below) and instead re-derives `(st_dev, st_ino)`
//! from the *stored path text* on every scan. For an identical path string,
//! re-resolving "what's there now" on both the stored side and the candidate
//! side necessarily lands on the same live directory, barring true TOCTOU
//! (out of scope per ADR 0009 §3c) — so the Rust scan catches
//! delete-and-recreate too, redundantly with the index, not despite it. This
//! was expected to isolate the index's necessity and did not; see
//! `delete_and_recreate_at_the_same_path_is_still_rejected` in
//! `tests/lease.rs` and the task report for the measurement.
//!
//! # Rejected alternative: storing `(dev, ino)` on `sessions`
//!
//! `sessions` deliberately gains no `dev`/`ino` columns to mirror
//! `scopes.dev`/`scopes.ino`. ADR 0009 is explicit that inode is a *runtime*
//! alias check, not durable identity — a delete-and-recreate at one path
//! yields a new inode for what an operator means as the same workspace, and a
//! stored inode would either go stale (silently wrong) or need its own
//! reconciliation machinery to stay current, which is exactly the "second
//! source of truth" `workspace_leases`'s own doc comment in `schema.rs`
//! already warns against for a different column. Re-deriving `(dev, ino)`
//! from a fresh `stat` on every acquisition is one `stat` call per
//! lease-holding session — cheap, and always current by construction, so
//! there is nothing to reconcile.
//!
//! # Why the scan-then-insert in `begin_start` is race-free
//!
//! [`factory_store::Store::transaction`] is `BEGIN IMMEDIATE` (ADR 0012
//! decision 3): it takes SQLite's write lock at the start of the transaction,
//! before either the scan or the insert runs. No concurrent mutator can
//! insert a competing lease-holder between this function's read and its
//! write, because no concurrent mutator can be inside a write transaction at
//! the same time at all. Without `BEGIN IMMEDIATE` this scan-then-insert
//! pattern would itself be exactly the race the lease exists to prevent.
//!
//! # `max_sessions`, lifetimes, and interrupted handling
//!
//! These are the seams the paragraphs above name as *not* this crate's job as
//! of Slice 6's first commit; this section documents where they landed once
//! they became this crate's job, in the same package rather than a new one
//! (the backlog draws no crate boundary between "the session state machine"
//! and "what decides which transition to make").
//!
//! **`max_sessions` is checked inside [`begin_start`]'s own transaction**,
//! immediately before the aliasing scan, by [`count_live_sessions`] — a scan
//! over `(scope_id, agent_name)`'s rows, filtered by
//! [`SessionState::holds_lease`] in Rust for the same reason
//! [`find_aliasing_conflict`] is: one method stays the single source of truth
//! for "does this state hold a lease," rather than a second hard-coded
//! `state IN (...)` list that could drift from it. It runs first, not after
//! the aliasing scan, because it is a pure row count with no `stat` calls,
//! and a caller who has already hit their cap should not pay for filesystem
//! work whose answer cannot change the outcome.
//!
//! Scoping the count by `(scope_id, agent_name)` together, never by
//! `scope_id` alone, is the entire implementation of "per agent rather than
//! per scope" (`factory_config::Agent::max_sessions`, design §2.2): two
//! agents in one scope simply query different rows and so never share a
//! budget. See `max_sessions_is_evaluated_per_agent_not_per_scope` in
//! `tests/max_sessions.rs`, and the task report's mutation run against the
//! `agent_name` predicate specifically — dropping only the cap-vs-count
//! comparison proves a cap exists at all, but not that it is per-agent, so
//! that is the predicate the mutation targets.
//!
//! **Lifetime-driven teardown is [`on_task_terminal`], which does not trust
//! its caller's word for "the task ended."** It re-reads `tasks.status`
//! itself and refuses — [`SessionError::TaskNotTerminal`] — unless it is
//! already one of design §2.4's three terminal statuses. An earlier version
//! of this function took an enum the caller constructed (`Done` / `Failed` /
//! `Cancelled`, with no `Blocked` variant to pass), which made "blocked
//! cannot trigger teardown" a compile-time fact — true, but untestable: a
//! rule a test suite cannot break by mutation is a rule the suite is not
//! actually watching (see the backlog's own "a lease test that cannot fail is
//! worse than no lease test" for the general version of this complaint).
//! Reading the durable row instead puts backlog §6's sharpest rule — "blocked
//! is NOT terminal... test this explicitly" — into a predicate a mutation can
//! widen (add `'blocked'` to the accepted set) and a test can catch failing;
//! see `on_task_terminal_refuses_to_tear_down_a_session_for_a_blocked_task` in
//! `tests/lifetime.rs`.
//!
//! `Lifetime::Permanent` short-circuits to `Ok(())` before that check even
//! runs: design §2.2 says a permanent session "survive[s] between tasks," so
//! there is nothing to verify or do.
//!
//! **[`interrupt`] is the "session dies while its task is not terminal"
//! path** (backlog §6, the rule with, in the brief's own words, "the sharpest
//! reasoning behind it"). It moves the session straight to
//! [`SessionState::Failed`] (releasing the lease) and, in the same
//! transaction, moves `task_id` to `blocked: interrupted` — unless the task
//! has already reached a terminal status, which this function must never
//! overwrite. A task already `blocked` for some other reason
//! (`clarification`, `permission`) is *not* exempt from that overwrite: that
//! is the direct, data-visible proof that `blocked` is not terminal, and it
//! is unrelated to, but reinforces, [`on_task_terminal`]'s check above. See
//! `interrupt_overwrites_an_existing_blocked_reason_because_blocked_is_not_terminal`
//! in `tests/interrupted.rs`.
//!
//! ## Why "the session record is removed" is `Failed`, not `DELETE FROM sessions`
//!
//! Backlog §6 and this task's brief both say, verbatim, that an interrupted
//! session's "record is removed." Measured against the real schema (not
//! assumed): `workspace_leases.session_id`, `tasks.target_session_id`, and
//! `delivery_attempts.session_id` all reference `sessions (id)`, none
//! declares an `ON DELETE` action, and ADR 0012 decision 3 sets
//! `PRAGMA foreign_keys = ON` on every connection — so a bare
//! `DELETE FROM sessions WHERE id = ?` on a session `begin_start` ever
//! created fails immediately, because `begin_start` always leaves at least
//! one `workspace_leases` row pointing at it. Proved directly, through this
//! crate's own `begin_start` and the real schema, by
//! `the_schema_rejects_deleting_a_session_that_a_lease_still_references` in
//! `tests/interrupted.rs`.
//!
//! **Rejected alternative: delete the referencing `workspace_leases` row
//! first, then the session.** This is mechanically possible — it is not a
//! bug the foreign key stops you from creating, only one it stops you from
//! creating *silently*. It is rejected on design grounds, not a technical
//! one: `workspace_leases`'s own doc comment in `schema.rs` states it is kept
//! "for audit and for Slice 9's mandatory stale-lease recovery action,"
//! independent of whether the session that acquired a lease still exists as
//! a row. Deleting that history to make the session row deletable would
//! defeat the reason the table exists. `Failed` already means, by
//! [`SessionState::holds_lease`] and the transition table below, "gone, lease
//! released, unreachable from here" — every property "removed" needs for
//! `max_sessions` counting, lease exclusivity, and future re-use — without
//! inventing a sixth state absent from design §2.3's fixed five-state
//! vocabulary, and without deleting the audit trail Slice 9 depends on.
//!
//! ## The permanent/temporary restart asymmetry, and why it is not an inconsistency
//!
//! A `permanent` agent's session is restarted after a crash; a `temporary`
//! one is not — Factory never redelivers its prompt, `interrupt` never calls
//! [`begin_start`], and there is no code path from `interrupt` back to a
//! fresh session for the same task. The asymmetry is not a special case
//! carved out of one consistent rule; it follows from a genuine difference in
//! what a session is doing:
//!
//! - A **permanent** session serves a standing channel, not one delivery. Its
//!   crash-recovery path is the state machine already proves elsewhere:
//!   `running → disconnected → running` (Factory loses sight, then regains
//!   it — possibly of a freshly relaunched process in the same row, or a
//!   confirmed-still-alive one; either way no task-level consequence follows,
//!   because a permanent session was never "handling one task" in the sense
//!   §2.4 means). See
//!   `disconnected_session_can_regain_sight_which_is_how_a_permanent_agent_recovers`
//!   in `tests/state_machine.rs` — mechanically identical machinery to
//!   [`interrupt`]'s starting point, diverging only in outcome.
//! - A **temporary** session exists to carry out one delivery whose external
//!   effect is, after an ambiguous failure, unknown — a sent mail, a pushed
//!   commit, a placed order may already have happened. Restarting it and
//!   resending the same prompt risks doing that effect twice with no way to
//!   detect the duplication after the fact. [`interrupt`] therefore does not
//!   retry; it stops, and a human — who can check what actually happened —
//!   creates a replacement task, which gets its own fresh temporary agent and
//!   session.
//!
//! This "never redelivers" rule cannot be pinned by mutation — there is no
//! retry call in [`interrupt`] to delete and watch a test fail. It is instead
//! pinned by assertion: `tests/interrupted.rs` checks that after `interrupt`,
//! exactly the one (now-`failed`) session row exists for that
//! `(scope_id, agent_name)`, and that `delivery_attempts` gains no new row for
//! the task — the table that exists specifically to journal each delivery
//! attempt, so zero new rows *is* "no redelivery happened," made durable and
//! checkable rather than merely absent from the code.
//!
//! # What this crate does not do
//!
//! - **Workspace path validation** — is this path an allowed workspace for
//!   this scope, a registered-descendant rejection, Git worktree identity —
//!   is explicitly another package's concern (implementation backlog §6).
//!   [`begin_start`] takes a [`factory_paths::CanonicalPath`], not a raw
//!   `Path` or `str`, so that boundary is enforced by the type signature: a
//!   caller must already have resolved (and, elsewhere, validated) the
//!   workspace before this crate ever sees it.
//! - **Launching a harness process, or observing its readiness.** Design
//!   §2.3 and backlog §6 require `starting` to be recorded *before* launch
//!   and `running` only after *observed* readiness, but this package has no
//!   process to launch. [`begin_start`] models "recorded before launch" by
//!   returning as soon as the `starting` row and its lease are durable;
//!   [`mark_running`] models "reached only after observed readiness" by
//!   existing as a separate call a launcher makes once it has that
//!   observation. The launcher itself — spawning Herdr, polling for a ready
//!   signal — is the seam another package fills.
//! - **Confirming a process is no longer usable.** [`stop`] performs only
//!   the bookkeeping — release the lease, record `stopped` — once its caller
//!   has already confirmed the process is gone. That confirmation is not
//!   something this crate can produce without a process to check, so it is
//!   the caller's precondition, stated here rather than left implicit. What
//!   *is* enforced here is that no *other* transition releases the lease:
//!   `running`→`disconnected` deliberately leaves it held (ADR 0012 decision
//!   5's most important case — see [`SessionState::holds_lease`]).
//! - **Detecting that a session has died at all.** [`interrupt`] models what
//!   happens once a temporary agent's session's death is *reported*; Slice 9
//!   owns the restart drill that produces that report (harness exit codes,
//!   Herdr's own liveness signal, a missed heartbeat — none of which this
//!   crate has an opinion on). Likewise, choosing *when* an operator runs the
//!   already-existing `disconnected`→`stopped`/`failed` stale-lease recovery
//!   transitions is Slice 9's command, not this crate's.

use std::path::PathBuf;

use factory_paths::{CanonicalPath, FileId, PathError};
use rusqlite::OptionalExtension;

/// Design §2.3's exact state vocabulary, matching `sessions.state`'s CHECK
/// constraint in `factory_store::schema` byte for byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Stopped,
    Starting,
    Running,
    Disconnected,
    Failed,
}

impl SessionState {
    /// ADR 0012 decision 5, settled and not re-decided here: `starting`,
    /// `running`, and `disconnected` hold a workspace lease; `stopped` and
    /// `failed` release it. Every place in this crate that needs to know
    /// whether a state holds the lease calls this method rather than
    /// re-listing the three states, so the rule has exactly one home.
    #[must_use]
    pub fn holds_lease(self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Disconnected)
    }

    fn as_db_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Disconnected => "disconnected",
            Self::Failed => "failed",
        }
    }

    /// `sessions.state` is constrained by CHECK to exactly these five
    /// strings, so an unrecognised value read back from a row this crate
    /// itself just selected is a broken invariant, not an input to handle —
    /// hence the panic rather than a `Result`.
    fn from_db_str(s: &str) -> Self {
        match s {
            "stopped" => Self::Stopped,
            "starting" => Self::Starting,
            "running" => Self::Running,
            "disconnected" => Self::Disconnected,
            "failed" => Self::Failed,
            other => unreachable!(
                "sessions.state is constrained by CHECK to the design §2.3 vocabulary; read {other:?}"
            ),
        }
    }
}

impl std::fmt::Display for SessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_db_str())
    }
}

/// The states `to` is reachable from `from`, in one call.
///
/// This table is deliberately narrower than "any holding state to any other
/// holding state": it encodes only the transitions backlog §6 and ADR 0012
/// actually justify.
///
/// - `Starting → Running`: launch succeeded, readiness observed.
/// - `Starting → Failed`: launch did not succeed. There is no
///   `Starting → Stopped` — a launch that never reached a confirmed
///   `Running` has nothing legitimately "stopped" about it; abandoning it is
///   a failure to start, which `Failed` already models and which releases
///   the lease identically.
/// - `Running → Disconnected`: Factory loses sight of a session it had
///   confirmed (Herdr restart, lost hook, machine restart). Per ADR 0012
///   decision 5 this must not release the lease: `disconnected` means
///   Factory cannot see the process, not that the process is gone, and a
///   second harness starting in the same directory while the first may
///   still be writing to it is the corruption the lease exists to prevent.
/// - `Running → Stopped`: a graceful stop, once the caller has confirmed the
///   process is no longer usable (this crate's precondition, not something
///   it can check — see the module docs).
/// - `Running → Failed`: added for [`interrupt`], and worth explaining
///   because it looks at first like it should route through `Disconnected`
///   the way every other "session might be gone" case does. It deliberately
///   does not: `Disconnected` means *Factory lost sight and does not yet
///   know*, but backlog §6's interrupted scenario is the opposite — the
///   caller (Slice 9's restart drill) has *already confirmed* the process is
///   gone, directly, with the task still non-terminal. Recording an
///   intervening `disconnected` row would journal a period of ambiguity that
///   never actually happened. Both routes release the lease identically once
///   `Failed` is reached (ADR 0012 decision 5), so nothing about lease
///   correctness depends on this choice — it is about not fabricating
///   history that a later audit of `workspace_leases` or `sessions` would
///   read as "Factory wasn't sure for a while," when in fact it was sure
///   immediately.
/// - `Disconnected → Running`: Factory regains sight and confirms the same
///   session is still alive — the mechanism a **permanent** agent's crash
///   recovery uses (see the module docs' restart-asymmetry section); no
///   task-level consequence follows, in contrast to [`interrupt`].
/// - `Disconnected → Stopped` / `Disconnected → Failed`: the mechanism
///   Slice 9's mandatory stale-lease recovery action uses to release a lease
///   whose session cannot be recovered — deciding *when* to call this is
///   Slice 9's job, not this crate's. [`interrupt`] also reaches `Failed`
///   this way when the dying session was already `disconnected` at the time
///   its death was confirmed.
fn valid_targets(from: SessionState) -> &'static [SessionState] {
    use SessionState::{Disconnected, Failed, Running, Starting, Stopped};
    match from {
        Starting => &[Running, Failed],
        Running => &[Disconnected, Stopped, Failed],
        Disconnected => &[Running, Stopped, Failed],
        Stopped | Failed => &[],
    }
}

fn is_valid_transition(from: SessionState, to: SessionState) -> bool {
    valid_targets(from).contains(&to)
}

/// One currently lease-holding session, as read back for the aliasing scan.
struct LeaseHolder {
    id: uuid::Uuid,
    agent_name: String,
    state: SessionState,
}

/// Scan every currently lease-holding session for one whose *stored*
/// workspace path, re-canonicalized right now, is the same directory as
/// `candidate` — by `(st_dev, st_ino)`, never by comparing paths as strings
/// (a [`CanonicalPath`]'s `PartialEq` is `PathBuf` equality, which is exactly
/// the bug ADR 0009's correction exists to avoid, so this function never
/// invokes it).
///
/// Reads every session row and filters with [`SessionState::holds_lease`] in
/// Rust, rather than hard-coding a second `state IN (...)` list here that
/// could drift from the one method. There are only ever a handful of live
/// session rows, so scanning all of them costs nothing; keeping the
/// lease-holding rule in exactly one place is worth more than the SQL-side
/// filter would save.
///
/// Every early exit from the loop below matters, because it decides whether
/// the *rest* of the lease-holding sessions still get examined. A row whose
/// stored path cannot currently be resolved (`PathError::NotFound`) must
/// `continue` — one stale row must never truncate the scan before a
/// genuinely conflicting holder listed after it is reached, or `begin_start`
/// would wrongly succeed and grant a second lease on a workspace that is
/// already held. `ORDER BY rowid` makes the scan's row order an explicit,
/// tested guarantee (insertion order) rather than an unspecified property of
/// `SELECT * FROM sessions` this function happened to rely on — see
/// `scan_continues_past_a_stale_row_to_find_a_later_conflict` in
/// `tests/lease.rs`, which depends on this order to place the stale row
/// before the genuinely conflicting one.
fn find_aliasing_conflict(
    tx: &rusqlite::Transaction<'_>,
    candidate: FileId,
) -> Result<Option<LeaseHolder>, SessionError> {
    let mut stmt = tx
        .prepare("SELECT id, agent_name, workspace_path, state FROM sessions ORDER BY rowid")
        .map_err(factory_store::StoreError::from)?;
    let mut rows = stmt.query([]).map_err(factory_store::StoreError::from)?;

    while let Some(row) = rows.next().map_err(factory_store::StoreError::from)? {
        let id: String = row.get(0).map_err(factory_store::StoreError::from)?;
        let agent_name: String = row.get(1).map_err(factory_store::StoreError::from)?;
        let workspace_path: String = row.get(2).map_err(factory_store::StoreError::from)?;
        let state: String = row.get(3).map_err(factory_store::StoreError::from)?;
        let state = SessionState::from_db_str(&state);

        if !state.holds_lease() {
            continue;
        }

        // Rule 2 (ADR 0009 correction): re-canonicalize the *stored* string
        // before comparing. A stale spelling ("…/Workspace") still resolves
        // to whatever the directory is truly called now ("…/workspace",
        // after a case-only rename), because `std::fs::canonicalize` asks
        // the filesystem for the answer, not the string it was given.
        //
        // Measured: on this filesystem, `find_aliasing_conflict`'s own
        // `case_variant_lease_conflict_spans_a_rename` test does not actually
        // discriminate this step from a raw `stat` on `workspace_path`
        // directly — APFS resolves case-insensitively either way, and a
        // mutation that replaced this block with
        // `FileId::of(Path::new(&workspace_path))` left that test passing.
        // `CanonicalPath::resolve` is kept anyway: it is what turns "the
        // stored path is gone" into the typed `PathError::NotFound` handled
        // below instead of a bare `io::Error`, and it is the same primitive
        // every other identity check in this codebase uses (`factory-paths`,
        // `factory-registry`) rather than a second, ad hoc way to ask "does
        // this exist."
        let canonical = match CanonicalPath::resolve(&workspace_path) {
            Ok(canonical) => canonical,
            // The stored path no longer resolves at all. That is a
            // stale-lease signal for Slice 9's mandatory recovery action
            // (ADR 0012), not a reason to block a new start here: a
            // directory that does not currently exist cannot be the
            // directory `candidate` names. This must be `continue`, never
            // `return Ok(None)`: this row is only one lease-holder among
            // possibly several, and a `return` here abandons the scan before
            // any holder listed after it is examined — silently letting a
            // real conflict through. See
            // `scan_continues_past_a_stale_row_to_find_a_later_conflict`.
            Err(PathError::NotFound { .. }) => continue,
            Err(other) => return Err(other.into()),
        };

        // Rule 1: compare `(st_dev, st_ino)`.
        let holder_file_id = FileId::of(canonical.as_path())?;
        if holder_file_id == candidate {
            return Ok(Some(LeaseHolder {
                id: uuid::Uuid::parse_str(&id)
                    .expect("sessions.id is a UUID: only this crate writes it"),
                agent_name,
                state,
            }));
        }
    }
    Ok(None)
}

/// Count of `agent_name`'s currently lease-holding sessions in `scope_id` —
/// `starting`, `running`, or `disconnected` (see [`SessionState::holds_lease`]
/// — the same three states [`find_aliasing_conflict`] treats as live, for the
/// same reason: a "concurrently live instance" is exactly a lease-holding
/// one).
///
/// This is the entire implementation of `factory_config::Agent::max_sessions`
/// being "per agent rather than per scope" (design §2.2): scoping the `WHERE`
/// clause by `(scope_id, agent_name)` together, rather than `scope_id` alone,
/// means two agents in one scope query disjoint sets of rows and so can never
/// share, or steal from, one another's budget. See the module docs' section
/// on this and `max_sessions_is_evaluated_per_agent_not_per_scope` in
/// `tests/max_sessions.rs`.
///
/// Scans and filters with [`SessionState::holds_lease`] in Rust rather than a
/// second hard-coded `state IN (...)` in the SQL, mirroring
/// [`find_aliasing_conflict`]'s own reasoning: there are only ever a handful
/// of rows per agent, so the scan costs nothing, and it keeps the
/// lease-holding rule in exactly one place.
fn count_live_sessions(
    tx: &rusqlite::Transaction<'_>,
    scope_id: uuid::Uuid,
    agent_name: &str,
) -> Result<u32, SessionError> {
    let mut stmt = tx
        .prepare("SELECT state FROM sessions WHERE scope_id = ?1 AND agent_name = ?2")
        .map_err(factory_store::StoreError::from)?;
    let mut rows = stmt
        .query((scope_id.to_string(), agent_name))
        .map_err(factory_store::StoreError::from)?;

    let mut live = 0u32;
    while let Some(row) = rows.next().map_err(factory_store::StoreError::from)? {
        let state: String = row.get(0).map_err(factory_store::StoreError::from)?;
        if SessionState::from_db_str(&state).holds_lease() {
            live += 1;
        }
    }
    Ok(live)
}

/// Begin starting a session for `agent_name` (under `scope_id`) in
/// `workspace`, recording it as `starting` *before* any launch attempt.
///
/// `workspace` must already be resolved (and, by another package,
/// validated) — see the module docs' "what this crate does not do".
/// `max_sessions` is the agent's configured bound
/// (`factory_config::Agent::max_sessions`, design §2.2); this function takes
/// it as a plain `u32` rather than a `factory_config::Agent` so that a caller
/// who already knows the number (from wherever it loaded the config) does not
/// have to reconstruct a whole `Agent` value just to start a session.
///
/// # Why `starting`, not `running`, is what gets recorded here
///
/// ADR 0012 decision 5: if the lease were only taken once a session reaches
/// `running`, two concurrent `begin_start` calls for the same workspace
/// could both pass every check that only looks at `running` sessions and
/// race for the directory — which is the exact defect the lease exists to
/// prevent. Recording `starting` first, synchronously, before any launch
/// happens, is what closes that race.
///
/// # Why the `max_sessions` check runs first
///
/// It is a pure row count against already-open rows — no `stat` calls, unlike
/// [`find_aliasing_conflict`] — so a caller who has already exhausted their
/// budget gets rejected without this function ever touching the filesystem.
/// Both checks run inside the one `BEGIN IMMEDIATE` transaction started
/// below, for the same race-freedom reason the module docs give for the
/// aliasing scan: no concurrent mutator can insert a competing session
/// between either check and the `INSERT`.
///
/// # Errors
///
/// [`SessionError::MaxSessionsReached`] when `agent_name` already has
/// `max_sessions` (or more) live sessions in `scope_id`. This rejects the
/// *new* start only — an existing session's row is never read for update and
/// never touched, so a rejected `begin_start` cannot disturb one.
///
/// [`SessionError::WorkspaceLeased`] when the Rust-level aliasing scan (see
/// [`find_aliasing_conflict`]) finds an existing lease-holder for the same
/// directory. [`SessionError::Store`] wrapping a `ConstraintViolation` when
/// the scan finds nothing but the database's
/// `sessions_one_live_lease_per_workspace` partial unique index rejects the
/// `INSERT` anyway — which happens for a delete-and-recreate at the same
/// path (same stored string, different inode; the scan sees "no conflict"
/// because it compares inodes, but the index still sees the identical
/// string). Neither error path is optional in favour of the other; see the
/// module docs.
pub fn begin_start(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    scope_id: uuid::Uuid,
    agent_name: &str,
    max_sessions: u32,
    workspace: &CanonicalPath,
) -> Result<(), SessionError> {
    let candidate_file_id = FileId::of(workspace.as_path())?;

    // BEGIN IMMEDIATE takes the write lock here, before either check below
    // reads a single row — see the module docs' race-freedom argument.
    let tx = store.transaction()?;

    let live = count_live_sessions(&tx, scope_id, agent_name)?;
    if live >= max_sessions {
        return Err(SessionError::MaxSessionsReached {
            agent_name: agent_name.to_string(),
            max_sessions,
            live_count: live,
        });
    }

    if let Some(holder) = find_aliasing_conflict(&tx, candidate_file_id)? {
        return Err(SessionError::WorkspaceLeased {
            path: workspace.as_path().to_path_buf(),
            holder_id: holder.id,
            holder_agent: holder.agent_name,
            holder_state: holder.state,
        });
    }

    let workspace_str = workspace.as_path().to_string_lossy().into_owned();

    tx.execute(
        "INSERT INTO sessions (id, scope_id, agent_name, workspace_path, state) \
         VALUES (?1, ?2, ?3, ?4, 'starting')",
        (
            id.to_string(),
            scope_id.to_string(),
            agent_name,
            &workspace_str,
        ),
    )
    .map_err(factory_store::StoreError::from)?;

    // Journalled in the same transaction as the state change it records, per
    // `workspace_leases`'s own doc comment in `schema.rs`.
    tx.execute(
        "INSERT INTO workspace_leases (session_id, canonical_workspace_path) VALUES (?1, ?2)",
        (id.to_string(), &workspace_str),
    )
    .map_err(factory_store::StoreError::from)?;

    tx.commit().map_err(factory_store::StoreError::from)?;
    Ok(())
}

/// The body of [`transition`], taking an already-open transaction rather than
/// opening (and committing) its own.
///
/// Split out for [`interrupt`] and [`on_task_terminal`], both of which need a
/// session transition to commit atomically alongside a write to `tasks` in
/// the very same transaction — for [`interrupt`], "the lease is released" and
/// "the task is blocked as interrupted" must rise or fall together, or a
/// crash between two separate commits could leave a released lease with no
/// record of why the task never finished. [`transition`] itself becomes a
/// two-line wrapper: open, delegate, commit.
fn transition_in_tx(
    tx: &rusqlite::Transaction<'_>,
    id: uuid::Uuid,
    to: SessionState,
    release_reason: Option<&str>,
) -> Result<(), SessionError> {
    let id_str = id.to_string();

    let current: Option<String> = tx
        .query_row(
            "SELECT state FROM sessions WHERE id = ?1",
            [&id_str],
            |row| row.get(0),
        )
        .optional()
        .map_err(factory_store::StoreError::from)?;
    let Some(current) = current else {
        return Err(SessionError::NotFound(id));
    };
    let from = SessionState::from_db_str(&current);

    if !is_valid_transition(from, to) {
        let allowed = valid_targets(from);
        let allowed = if allowed.is_empty() {
            "nothing — this is a terminal state".to_string()
        } else {
            allowed
                .iter()
                .map(SessionState::to_string)
                .collect::<Vec<_>>()
                .join(" or ")
        };
        return Err(SessionError::InvalidTransition {
            id,
            from,
            to,
            allowed,
        });
    }

    tx.execute(
        "UPDATE sessions SET state = ?2, updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
        (&id_str, to.as_db_str()),
    )
    .map_err(factory_store::StoreError::from)?;

    // The lease-holding rule, applied once here rather than re-decided per
    // transition: any move from a holding state to a non-holding one closes
    // the still-open `workspace_leases` row; any move between two holding
    // states (`starting`→`running`, `running`→`disconnected`,
    // `disconnected`→`running`) leaves it untouched, because the workspace
    // was never actually freed.
    if from.holds_lease() && !to.holds_lease() {
        // For a session created through `begin_start`, exactly one open row
        // is expected here. That invariant is this crate's own to keep, not
        // one it can enforce on rows it did not write: a session inserted by
        // something other than `begin_start` — a fixture, a future
        // migration, another package's raw SQL — may hold a lease-holding
        // `state` with no matching `workspace_leases` row at all, and this
        // `UPDATE` then affects zero rows. That is silently fine: there is
        // nothing open to close, so nothing is left inconsistent.
        tx.execute(
            "UPDATE workspace_leases SET released_at = CURRENT_TIMESTAMP, release_reason = ?2 \
             WHERE session_id = ?1 AND released_at IS NULL",
            (&id_str, release_reason),
        )
        .map_err(factory_store::StoreError::from)?;
    }

    Ok(())
}

/// Move `id` to `to`, releasing the workspace lease exactly when that moves
/// it out of a lease-holding state (see [`SessionState::holds_lease`]).
/// `release_reason` is recorded on the `workspace_leases` row when a release
/// happens, and ignored otherwise.
fn transition(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    to: SessionState,
    release_reason: Option<&str>,
) -> Result<(), SessionError> {
    let tx = store.transaction()?;
    transition_in_tx(&tx, id, to, release_reason)?;
    tx.commit().map_err(factory_store::StoreError::from)?;
    Ok(())
}

/// `starting → running` (readiness observed) or `disconnected → running`
/// (Factory regains sight of an already-confirmed session). Both leave the
/// lease held throughout, so both share one implementation.
pub fn mark_running(store: &mut factory_store::Store, id: uuid::Uuid) -> Result<(), SessionError> {
    transition(store, id, SessionState::Running, None)
}

/// `running → disconnected`. Per ADR 0012 decision 5 this deliberately does
/// **not** release the lease.
pub fn mark_disconnected(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
) -> Result<(), SessionError> {
    transition(store, id, SessionState::Disconnected, None)
}

/// `starting → failed`, `disconnected → failed`, or `running → failed`.
/// Releases the lease. [`interrupt`] is a thin, task-aware wrapper around
/// this same target state for the specific case of a temporary agent's
/// session dying mid-task; call this one directly for every other reason a
/// session might fail (a launch that never became `running`, or a
/// `disconnected` session Slice 9's stale-lease recovery gives up on).
pub fn fail(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    reason: &str,
) -> Result<(), SessionError> {
    transition(store, id, SessionState::Failed, Some(reason))
}

/// `running → stopped` or `disconnected → stopped`. Releases the lease.
///
/// Precondition owned by the caller, not checked here: the process this
/// session names must already be confirmed no longer usable. See the module
/// docs for why that confirmation cannot be produced inside this crate.
pub fn stop(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    reason: &str,
) -> Result<(), SessionError> {
    transition(store, id, SessionState::Stopped, Some(reason))
}

/// Design §2.2's lifetime rule, applied once `task_id` — the task
/// `session_id` was handling — has genuinely finished.
///
/// Does **not** take the caller's word for "finished." It re-reads
/// `task_id`'s own `status` column and proceeds only if that row already
/// shows one of design §2.4's three terminal statuses (`done`, `failed`,
/// `cancelled`); otherwise it refuses with [`SessionError::TaskNotTerminal`]
/// rather than touch the session. This is what makes backlog §6's sharpest
/// warning about this rule — "`blocked` is NOT terminal... test this
/// explicitly, because 'blocked' reads like an ending" — a runtime check a
/// test can break by mutation, instead of a compile-time-only guarantee. See
/// the module docs' "lifetimes" section for why a caller-supplied enum was
/// rejected in favour of this read, and
/// `on_task_terminal_refuses_to_tear_down_a_session_for_a_blocked_task` in
/// `tests/lifetime.rs`.
///
/// - [`factory_config::Lifetime::Permanent`]: short-circuits to `Ok(())`
///   before the check above even runs. Design §2.2: a permanent session
///   "survive[s] between tasks," so there is nothing to verify or do — this
///   is the "permanent" half of the crash-restart asymmetry the module docs
///   describe; the session is simply left alone.
/// - [`factory_config::Lifetime::Temporary`]: once the task is confirmed
///   terminal, tears the session down via [`stop`] — reusing [`stop`]'s own
///   precondition (the caller has arranged for the process to no longer be
///   needed) rather than inventing a second teardown path.
///
/// Reads and writes happen in one transaction, so a task whose status
/// changes between the check and the teardown cannot produce a session that
/// is torn down against a task row that, by the time anyone looks, no longer
/// justifies it.
///
/// # Errors
///
/// [`SessionError::TaskNotFound`] if `task_id` does not exist.
/// [`SessionError::TaskNotTerminal`] if it exists but its `status` is not
/// `done`, `failed`, or `cancelled` (this is where `blocked` — and `queued`,
/// and `running` — are refused). Any error [`transition_in_tx`] can produce
/// for the underlying `stop`, verbatim (for example
/// [`SessionError::InvalidTransition`] if `session_id` is not currently
/// `running` or `disconnected`).
pub fn on_task_terminal(
    store: &mut factory_store::Store,
    session_id: uuid::Uuid,
    task_id: uuid::Uuid,
    lifetime: factory_config::Lifetime,
) -> Result<(), SessionError> {
    if lifetime == factory_config::Lifetime::Permanent {
        return Ok(());
    }

    let tx = store.transaction()?;

    let status: Option<String> = tx
        .query_row(
            "SELECT status FROM tasks WHERE id = ?1",
            [task_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(factory_store::StoreError::from)?;
    let Some(status) = status else {
        return Err(SessionError::TaskNotFound(task_id));
    };
    if !matches!(status.as_str(), "done" | "failed" | "cancelled") {
        return Err(SessionError::TaskNotTerminal { task_id, status });
    }

    transition_in_tx(
        &tx,
        session_id,
        SessionState::Stopped,
        Some(&format!(
            "temporary agent torn down: task {task_id} reached `{status}`"
        )),
    )?;

    tx.commit().map_err(factory_store::StoreError::from)?;
    Ok(())
}

/// Backlog §6's interrupted-handling rule — described there as the one "with
/// the sharpest reasoning behind it" — for a temporary agent's session that
/// died while `task_id` had not reached a terminal state: "harness crash,
/// Herdr restart, machine restart" are the backlog's own examples. *Detecting*
/// that death is Slice 9's restart drill, not this function; `interrupt` is
/// what that drill calls once it has.
///
/// Atomically, in one transaction:
///
/// 1. `session_id` moves straight to [`SessionState::Failed`] (see
///    [`valid_targets`] for why this is now a direct edge from every
///    lease-holding state, including `running`), releasing its workspace
///    lease. See the module docs' "why 'the session record is removed' is
///    `Failed`, not `DELETE FROM sessions`" for why this — and not an actual
///    row deletion — is what "the session record is removed" means here,
///    with the foreign-key evidence for the claim.
/// 2. `task_id` moves to `blocked: interrupted` — *unless* it has already
///    reached `done`, `failed`, or `cancelled`, which this must never
///    overwrite. A task that is already `blocked` for another reason
///    (`clarification`, `permission`) is *not* exempt: overwriting it is the
///    direct, data-visible proof that design §2.4 does not count `blocked`
///    as terminal. See
///    `interrupt_overwrites_an_existing_blocked_reason_because_blocked_is_not_terminal`
///    in `tests/interrupted.rs`.
///
/// Both writes commit together or not at all: a crash between "lease
/// released" and "task marked interrupted" must never happen, because a
/// released lease with no record of why the task stalled is exactly the kind
/// of ambiguity backlog §6 exists to prevent.
///
/// # Why there is no retry here, and why that cannot be pinned by mutation
///
/// This function contains no call to [`begin_start`] and constructs no new
/// session. That is deliberate, not an oversight: after an ambiguous
/// failure it is unknown whether the task's one delivery already had
/// external effect — a sent mail, a pushed commit, a placed order — and
/// resending the same prompt risks doing that effect twice with no way to
/// detect the duplication afterward. A human, who can check what actually
/// happened, creates the replacement task; that gets its own fresh temporary
/// agent and session, entirely outside this function. Contrast a
/// **permanent** agent, which carries no in-flight delivery whose effect is
/// in question and is restarted freely — see the module docs'
/// "restart asymmetry" section and
/// `disconnected_session_can_regain_sight_which_is_how_a_permanent_agent_recovers`
/// in `tests/state_machine.rs`.
///
/// There is no line to delete here that a mutation test could restore to
/// prove this — absence is not a rule a mutation can remove. It is instead
/// pinned by assertion: see `interrupt_leaves_delivery_attempts_untouched...`
/// in `tests/interrupted.rs`, which checks that `delivery_attempts` — the
/// table that exists specifically to journal each delivery attempt — gains
/// no row for `task_id` as a result of calling this function.
///
/// # Why this takes no `Lifetime`, unlike [`on_task_terminal`]
///
/// [`on_task_terminal`] can verify its caller's claim against a durable
/// record (`tasks.status`, right there in this database). An agent's
/// `lifetime` cannot be verified the same way: it lives in `.factory/config.yaml`
/// (`factory_config::Agent::lifetime`, design §2.2), which this crate has no
/// connection to and does not read. Accepting a `Lifetime` parameter here
/// would only be trusting the caller's word for it — precisely the
/// caller-supplied-enum design this module's docs reject for
/// [`on_task_terminal`]'s own terminal-status check, for the identical
/// reason. So `interrupt` takes none: deciding "this agent is temporary,
/// therefore call `interrupt` instead of the permanent-agent recovery path"
/// is the caller's job, stated as a precondition rather than an
/// unverifiable parameter — the same shape of boundary the module docs
/// already draw around [`stop`]'s "process confirmed gone" precondition.
/// A caller that calls `interrupt` for a permanent agent gets exactly what
/// it asked for (the session fails, its task — if any — is blocked as
/// interrupted); nothing here detects or refuses that misuse.
///
/// # Errors
///
/// [`SessionError::TaskNotFound`] if `task_id` does not exist — checked
/// before either write, so a garbage `task_id` fails the whole call rather
/// than releasing the session's lease while silently skipping the half of
/// this function's contract that touches `tasks` (an `UPDATE` matching zero
/// rows would otherwise return `Ok` with nothing to show for it).
/// [`SessionError::NotFound`] if `session_id` does not exist.
/// [`SessionError::InvalidTransition`] if `session_id` is already `stopped`
/// or `failed` — there is nothing left to interrupt, and silently succeeding
/// would hide a caller bug (interrupting a session twice, or one another path
/// already cleaned up).
pub fn interrupt(
    store: &mut factory_store::Store,
    session_id: uuid::Uuid,
    task_id: uuid::Uuid,
) -> Result<(), SessionError> {
    let tx = store.transaction()?;

    // Checked up front, like `on_task_terminal`'s `TaskNotFound`, rather than
    // left implicit in an `UPDATE` that would otherwise affect zero rows and
    // return `Ok`. Without this, a garbage `task_id` would still fail the
    // session (lease released) but silently skip the task-side half of this
    // function's contract, with no error to say so.
    let task_exists: Option<i64> = tx
        .query_row(
            "SELECT 1 FROM tasks WHERE id = ?1",
            [task_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(factory_store::StoreError::from)?;
    if task_exists.is_none() {
        return Err(SessionError::TaskNotFound(task_id));
    }

    let reason = format!("interrupted: task {task_id} was not terminal when the session died");
    transition_in_tx(&tx, session_id, SessionState::Failed, Some(&reason))?;

    // `blocked` is deliberately included among the rows this still updates —
    // see the doc comment above and the module docs: design §2.4 does not
    // count it as terminal, so a task already `blocked: clarification` or
    // `blocked: permission` is still rewritten to `blocked: interrupted`
    // here.
    tx.execute(
        "UPDATE tasks SET status = 'blocked', blocked_reason = 'interrupted', \
         updated_at = CURRENT_TIMESTAMP \
         WHERE id = ?1 AND status NOT IN ('done', 'failed', 'cancelled')",
        [task_id.to_string()],
    )
    .map_err(factory_store::StoreError::from)?;

    tx.commit().map_err(factory_store::StoreError::from)?;
    Ok(())
}

/// Everything that can go wrong starting or transitioning a session.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("path error: {0}")]
    Path(#[from] factory_paths::PathError),

    #[error("store error: {0}")]
    Store(#[from] factory_store::StoreError),

    #[error(
        "workspace {path} is already leased by session {holder_id} (agent `{holder_agent}`, state `{holder_state}`)\n  help: stop or recover the existing session before starting a new one in this workspace"
    )]
    WorkspaceLeased {
        path: PathBuf,
        holder_id: uuid::Uuid,
        holder_agent: String,
        holder_state: SessionState,
    },

    #[error(
        "agent `{agent_name}` already has {live_count} of its {max_sessions} allowed session(s) running\n  help: stop or wait for an existing session of this agent before starting another — max_sessions is per agent (design §2.2), not per scope"
    )]
    MaxSessionsReached {
        agent_name: String,
        max_sessions: u32,
        live_count: u32,
    },

    #[error("no session with id {0}")]
    NotFound(uuid::Uuid),

    #[error("no task with id {0}")]
    TaskNotFound(uuid::Uuid),

    #[error(
        "task {task_id} has status `{status}`, which is not terminal\n  help: only `done`, `failed`, or `cancelled` end a task; `blocked` does not (design §2.4) and must not tear down its session"
    )]
    TaskNotTerminal { task_id: uuid::Uuid, status: String },

    #[error(
        "session {id} cannot move from `{from}` to `{to}`\n  help: `{from}` only reaches {allowed}"
    )]
    InvalidTransition {
        id: uuid::Uuid,
        from: SessionState,
        to: SessionState,
        allowed: String,
    },
}
