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
//!   5's most important case — see [`SessionState::holds_lease`]), and there
//!   is no `starting`→`disconnected` transition at all (see the transition
//!   table below) — a start that never reached an observed `running` has no
//!   confirmed process to lose sight of, so a failed launch is honestly
//!   `failed`, which already releases.
//! - **`max_sessions`, permanent/temporary teardown, and Slice 9's stale-lease
//!   recovery command.** Those consume the transitions this crate exposes
//!   ([`mark_running`], `disconnected`→`stopped`/`failed` via [`stop`]/
//!   [`fail`]) but decide *when* to call them, which is out of this
//!   package's scope.

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
/// - `Disconnected → Running`: Factory regains sight and confirms the same
///   session is still alive.
/// - `Disconnected → Stopped` / `Disconnected → Failed`: the mechanism
///   Slice 9's mandatory stale-lease recovery action uses to release a lease
///   whose session cannot be recovered — deciding *when* to call this is
///   Slice 9's job, not this crate's.
fn valid_targets(from: SessionState) -> &'static [SessionState] {
    use SessionState::{Disconnected, Failed, Running, Starting, Stopped};
    match from {
        Starting => &[Running, Failed],
        Running => &[Disconnected, Stopped],
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

/// Begin starting a session for `agent_name` (under `scope_id`) in
/// `workspace`, recording it as `starting` *before* any launch attempt.
///
/// `workspace` must already be resolved (and, by another package,
/// validated) — see the module docs' "what this crate does not do".
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
/// # Errors
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
    workspace: &CanonicalPath,
) -> Result<(), SessionError> {
    let candidate_file_id = FileId::of(workspace.as_path())?;

    // BEGIN IMMEDIATE takes the write lock here, before the scan below reads
    // a single row — see the module docs' race-freedom argument.
    let tx = store.transaction()?;

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

/// `starting → failed` or `disconnected → failed`. Releases the lease.
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

    #[error("no session with id {0}")]
    NotFound(uuid::Uuid),

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
