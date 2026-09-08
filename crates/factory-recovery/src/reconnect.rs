//! Live evidence reconciling a `disconnected` session — the second half of
//! the split this crate's docs describe: ADR 0019 says what a recovered
//! database may claim on its own, and this module is the only place a live
//! observation is permitted to change that claim.
//!
//! Every function here starts from `sessions.state = 'disconnected'` rows
//! exactly as ADR 0019 decision 2's reconciliation (or a supervisor restart
//! that runs the same reconciliation) leaves them: lease held, and any task
//! that was `running` or `queued`-with-an-attempt already rewritten to
//! `blocked: interrupted`. Nothing here calls that reconciliation — tests
//! seed the starting state directly, per this crate's docs.
//!
//! Design §5's recovery table names five restart classes. Three are covered
//! here:
//!
//! - [`reconnect_after_supervisor_restart`] — "Factory supervisor restart:
//!   reconnect to known Herdr panes, restore workspace leases, and reconcile
//!   tasks." Herdr and every harness are presumed still alive; only a
//!   positive, authoritative observation moves a session, and every session
//!   this function cannot positively confirm is given up on (see
//!   [`give_up_on_disconnected_session`]) rather than left in limbo, because
//!   backlog §9 has no automated retry loop to revisit it later — this slice
//!   builds no daemon, loop, or scheduler, so "run once, decide, done" is the
//!   whole mechanism.
//! - [`reconnect_after_herdr_or_machine_restart`] — design §5's "Herdr
//!   restart: recreate sessions in their recorded workspaces; unresolved
//!   running tasks become blocked" and "Machine restart: recreate sessions in
//!   existing workspaces; queued tasks remain queued," merged exactly as
//!   backlog §9 merges them into one acceptance criterion. The presumption
//!   inverts here: the multiplexer or the machine itself restarted, so a
//!   session is presumed **gone** unless live evidence says otherwise.
//! - [`give_up_on_disconnected_session`] — design §5's "does not automatically
//!   resend a possibly delivered prompt... the task becomes `blocked:
//!   interrupted` for human review," and ADR 0012 decision 5's mandatory
//!   stale-lease recovery action, in one function both restart classes above
//!   call whenever they cannot positively reconnect or safely recreate.
//!   Exported on its own because design §5's other two remaining classes —
//!   "Harness crash: mark session failed and current task blocked or
//!   failed" and "Harness change: stop the old session; queued tasks remain;
//!   running task requires review" — describe the identical operation and
//!   are explicitly not this slice's to build; a later agent covering them
//!   should not have to re-derive this one.
//!
//! **Never call `factory_task::deliver::deliver` from here, or from anything
//! this module calls.** That is design §5's whole point after a restart:
//! reconnecting or recreating a session must never cause a delivery Factory
//! cannot prove happened at most once. See
//! `tests/supervisor_restart.rs`'s `reconnecting_never_delivers_a_prompt`,
//! which pins this by asserting `delivery_attempts` gains no row and
//! `tasks.status` for a queued task is untouched, not merely by grepping the
//! source for the word `deliver`.

use std::path::Path;

use factory_adapter::{Adapter, Observation, PaneId};
use factory_paths::CanonicalPath;
use factory_store::Store;
use rusqlite::OptionalExtension;

use crate::evidence::may_promote_from_disconnected;

/// A `disconnected` session, read with the two correlation columns migration
/// 5 added (`sessions.herdr_pane_id`, `sessions.harness_session_id`).
/// `factory_session::Session` does not carry these — that reader predates
/// migration 5 and is not this crate's to extend — so this module reads the
/// row itself rather than widening a struct it does not own.
struct DisconnectedSession {
    id: uuid::Uuid,
    scope_id: uuid::Uuid,
    agent_name: String,
    workspace_path: String,
    herdr_pane_id: Option<String>,
    harness_session_id: Option<String>,
}

fn row_to_disconnected_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<DisconnectedSession> {
    let id: String = row.get(0)?;
    let scope_id: String = row.get(1)?;
    let agent_name: String = row.get(2)?;
    let workspace_path: String = row.get(3)?;
    let herdr_pane_id: Option<String> = row.get(4)?;
    let harness_session_id: Option<String> = row.get(5)?;
    Ok(DisconnectedSession {
        id: uuid::Uuid::parse_str(&id)
            .unwrap_or_else(|e| panic!("sessions.id is a UUID; read {id:?}: {e}")),
        scope_id: uuid::Uuid::parse_str(&scope_id)
            .unwrap_or_else(|e| panic!("sessions.scope_id is a UUID; read {scope_id:?}: {e}")),
        agent_name,
        workspace_path,
        herdr_pane_id,
        harness_session_id,
    })
}

/// Every currently `disconnected` session, oldest first. Filtering on
/// `state = 'disconnected'` is what makes every function in this module
/// idempotent (coordinator decision 4): a session this module has already
/// moved to `running`, `starting` (a fresh recreation), or `failed` never
/// matches this query again, so running any function here twice in a row
/// finds nothing left to do the second time.
fn disconnected_sessions(store: &Store) -> Result<Vec<DisconnectedSession>, ReconnectError> {
    let mut stmt = store
        .connection()
        .prepare(
            "SELECT id, scope_id, agent_name, workspace_path, herdr_pane_id, harness_session_id \
             FROM sessions WHERE state = 'disconnected' ORDER BY created_at, id",
        )
        .map_err(factory_store::StoreError::from)?;
    let rows = stmt
        .query_map([], row_to_disconnected_session)
        .map_err(factory_store::StoreError::from)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| factory_store::StoreError::from(e).into())
}

/// The one non-terminal task currently assigned to `session_id`, if any.
/// `tasks.assigned_session_id` is what ADR 0019's reconciliation leaves
/// pointing at the session a `running` or attempted `queued` task was on when
/// evidence was lost, so this is the row [`give_up_on_disconnected_session`]
/// must fold `blocked: interrupted` into rather than leaving untouched.
fn non_terminal_task_of(
    store: &Store,
    session_id: uuid::Uuid,
) -> Result<Option<uuid::Uuid>, ReconnectError> {
    let id: Option<String> = store
        .connection()
        .query_row(
            "SELECT id FROM tasks WHERE assigned_session_id = ?1 \
             AND status NOT IN ('done', 'failed', 'cancelled') \
             ORDER BY created_at, id LIMIT 1",
            [session_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(factory_store::StoreError::from)?;
    Ok(id.map(|s| {
        uuid::Uuid::parse_str(&s).unwrap_or_else(|e| panic!("tasks.id is a UUID; read {s:?}: {e}"))
    }))
}

/// Whether `observation` is not just authoritative but about the *same*
/// session `session` names — the identity half of ADR 0017 decision 3's
/// "correlation runs pane → transcript → session," kept separate from
/// [`may_promote_from_disconnected`] on purpose (see that function's doc
/// comment on what it deliberately does not decide).
///
/// A recorded `harness_session_id` that disagrees with the one just observed
/// is positive proof of ADR 0019 decision 3's hazard: the pane has been
/// reused by an entirely different session, and this returns `false`
/// regardless of confidence. When either side has no `harness_session_id` to
/// compare — the session predates migration 5, or the harness's transcript
/// filename did not parse (`factory_adapter::parse_harness_session_id` can
/// return `None`) — there is nothing to positively contradict the pane match
/// on, so this falls back to trusting the confidence check alone. That
/// fallback is a real, documented gap, not an oversight: closing it needs the
/// transcript-path correlation ADR 0017's open item already calls for, which
/// is not this slice's to build. [`record_confirmed_identity`] narrows the
/// one instance of this gap this module can close on its own — a session
/// that had no recorded `harness_session_id` the first time it is
/// authoritatively reconnected gets one recorded then, so the *next*
/// reconnection compares for real instead of falling back — but it cannot
/// help a session whose transcript filename never parses at all.
fn identifies_same_session(session: &DisconnectedSession, observation: &Observation) -> bool {
    match (&session.harness_session_id, &observation.harness_session_id) {
        (Some(recorded), Some(observed)) => recorded == observed,
        _ => true,
    }
}

fn workspace_exists_on_disk(path: &str) -> bool {
    Path::new(path).exists()
}

/// Records what an authoritative, identity-confirmed observation just
/// established about `session`, so the *next* reconnection has a real
/// identity to compare rather than [`identifies_same_session`]'s fallback.
///
/// Per the coordinator's handover on migration 5: nothing in the workspace
/// has ever written `sessions.herdr_pane_id` or `sessions.harness_session_id`
/// (`factory_session::begin_start`'s `INSERT` predates both columns, and
/// `factory-task` reaching around `factory-session` into a table it does not
/// own would be worse than the gap), and this module — "the only place a
/// live observation is permitted to move a session at all," per the crate
/// docs — is where a value for either column can honestly come from.
///
/// **Only ever fills a `NULL`, never overwrites a recorded value.** This is
/// deliberately narrower than "keep it in sync": `evidence::may_promote_from_
/// disconnected` plus the identity check above have already run by the time
/// this is called, so a *mismatched* observation never reaches here at all
/// (the caller does not promote, so this is never invoked) — there is
/// nothing to overwrite with, only a first fact to establish. Recording an id
/// from an observation that could not confirm identity would be worse than
/// no id at all, because the next reconnection would treat it as a place to
/// look; this function is called only from the two sites that already
/// checked confidence and identity, never on a weaker reading.
///
/// Runs in its own transaction, after [`factory_session::mark_running`] has
/// already committed the state change — that function's transaction is not
/// this crate's to extend (it is `factory_session`'s private
/// `transition_in_tx`, not a seam this crate can join). A crash between the
/// two commits leaves the session `running` with the identity not yet
/// recorded, which is exactly today's status quo for every session this
/// column has never been written for — a missed opportunity, not a
/// regression, and the next successful reconnection records it instead.
fn record_confirmed_identity(
    store: &mut Store,
    session: &DisconnectedSession,
    observation: &Observation,
) -> Result<(), ReconnectError> {
    if session.harness_session_id.is_some() {
        return Ok(());
    }
    let Some(observed_id) = &observation.harness_session_id else {
        return Ok(());
    };

    let tx = store.transaction()?;
    tx.execute(
        "UPDATE sessions SET herdr_pane_id = COALESCE(herdr_pane_id, ?2), \
         harness_session_id = COALESCE(harness_session_id, ?3) WHERE id = ?1",
        (
            session.id.to_string(),
            observation.pane.0.as_str(),
            observed_id.as_str(),
        ),
    )
    .map_err(factory_store::StoreError::from)?;
    tx.commit().map_err(factory_store::StoreError::from)?;
    Ok(())
}

/// What [`give_up_on_disconnected_session`] actually did, since the two
/// cases leave different evidence behind for a human to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GiveUpOutcome {
    /// A non-terminal task was attached; `factory_session::interrupt` moved
    /// it to `blocked: interrupted` in the same transaction as the session's
    /// own move to `failed`.
    Interrupted { task_id: uuid::Uuid },
    /// No non-terminal task was attached; only the session moved, releasing
    /// its stale lease.
    Failed,
}

/// ADR 0012 decision 5's mandatory stale-lease recovery action, and design
/// §5's "the task becomes `blocked: interrupted` for human review" — the one
/// operation both restart classes in this module reach for whenever they
/// cannot positively reconnect or safely recreate a `disconnected` session,
/// and the operation design §5's still-unbuilt harness-crash and
/// harness-change classes describe doing too (see the module docs).
///
/// Releases the stale lease unconditionally: `session_id` must already be
/// `disconnected` (a lease-holding state per ADR 0012 decision 5), and after
/// this call it is `failed`, which releases it
/// (`factory_session::SessionState::holds_lease`). Any task
/// [`non_terminal_task_of`] finds still assigned to it is folded into
/// `blocked: interrupted` atomically with that move
/// (`factory_session::interrupt`); a session with no such task just fails
/// (`factory_session::fail`), carrying `reason` into the closed
/// `workspace_leases` row for the operator reading it afterward — the "clear
/// human next action" backlog §9 asks for.
///
/// # Errors
///
/// Whatever [`factory_session::interrupt`] or [`factory_session::fail`]
/// itself returns, verbatim — most notably
/// [`factory_session::SessionError::InvalidTransition`] if `session_id` is
/// not currently in a lease-holding state, which is this function's caller's
/// precondition to uphold, not something it can check further than the
/// database already will.
pub fn give_up_on_disconnected_session(
    store: &mut Store,
    session_id: uuid::Uuid,
    reason: &str,
) -> Result<GiveUpOutcome, ReconnectError> {
    match non_terminal_task_of(store, session_id)? {
        Some(task_id) => {
            factory_session::interrupt(store, session_id, task_id)?;
            Ok(GiveUpOutcome::Interrupted { task_id })
        }
        None => {
            factory_session::fail(store, session_id, reason)?;
            Ok(GiveUpOutcome::Failed)
        }
    }
}

/// What became of one `disconnected` session after a reconciliation pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Live evidence confirmed the same session; it is `running` again.
    Reconnected,
    /// The old session was given up on and a new one opened in its place, in
    /// the same recorded workspace.
    Recreated {
        new_session_id: uuid::Uuid,
        outcome: GiveUpOutcomeSummary,
    },
    /// Given up on outright — no recreation, per [`give_up_on_disconnected_session`].
    GivenUp(GiveUpOutcomeSummary),
    /// `factory_adapter::Adapter::observe` itself failed (not "no answer" —
    /// see `factory_adapter::Confidence::Unavailable` for that case, which is
    /// not this variant and is folded into [`Outcome::GivenUp`]). Recorded
    /// against this session and skipped, rather than aborting the whole
    /// batch: one adapter failure must not hide the decision for every other
    /// session in the same pass, mirroring
    /// `factory_session::find_aliasing_conflict`'s own `continue`-past-a-bad-row
    /// reasoning.
    SkippedAdapterError { detail: String },
}

/// A cheap, `Clone`-able summary of [`GiveUpOutcome`] for embedding in
/// [`Outcome`], which itself needs to be `Clone` for tests to assert against
/// without consuming the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GiveUpOutcomeSummary {
    Interrupted { task_id: uuid::Uuid },
    Failed,
}

impl From<GiveUpOutcome> for GiveUpOutcomeSummary {
    fn from(outcome: GiveUpOutcome) -> Self {
        match outcome {
            GiveUpOutcome::Interrupted { task_id } => Self::Interrupted { task_id },
            GiveUpOutcome::Failed => Self::Failed,
        }
    }
}

/// One session's fate, for the report [`reconnect_after_supervisor_restart`]
/// and [`reconnect_after_herdr_or_machine_restart`] both return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconnectRecord {
    pub session_id: uuid::Uuid,
    pub outcome: Outcome,
}

/// Design §5: "Factory supervisor restart: reconnect to known Herdr panes,
/// restore workspace leases, and reconcile tasks." Backlog §9: "known panes
/// are reconnected where possible and no task prompt is silently sent a
/// second time."
///
/// For every `disconnected` session with a recorded `herdr_pane_id`, asks
/// `adapter` to observe that pane. [`may_promote_from_disconnected`] and
/// [`identifies_same_session`] both have to agree before the session moves to
/// `running` (`factory_session::mark_running`); anything else —
/// non-authoritative evidence, a mismatched harness session id, or Herdr
/// explicitly saying nothing is there — is handed to
/// [`give_up_on_disconnected_session`], because this slice runs once and has
/// nowhere to leave an undecided session for later. A session with **no**
/// recorded pane at all is left untouched: it predates migration 5, or was
/// never a candidate for pane-based reconnection, and this function does not
/// invent a workspace-existence fallback for it (that is
/// [`reconnect_after_herdr_or_machine_restart`]'s presumption, not this
/// one's).
///
/// Never calls `factory_task::deliver::deliver` or anything that would — see
/// the module docs.
///
/// # Errors
///
/// Any [`ReconnectError`] a read or a `factory_session` call produces. Does
/// not itself fail for a per-session adapter error; see
/// [`Outcome::SkippedAdapterError`].
pub fn reconnect_after_supervisor_restart(
    store: &mut Store,
    adapter: &dyn Adapter,
) -> Result<Vec<ReconnectRecord>, ReconnectError> {
    let mut records = Vec::new();

    for session in disconnected_sessions(store)? {
        let Some(pane_id) = session.herdr_pane_id.clone() else {
            continue;
        };
        let pane = PaneId(pane_id);

        let outcome = match adapter.observe(&pane) {
            Ok(observation)
                if may_promote_from_disconnected(&observation)
                    && identifies_same_session(&session, &observation) =>
            {
                factory_session::mark_running(store, session.id)?;
                record_confirmed_identity(store, &session, &observation)?;
                Outcome::Reconnected
            }
            Ok(_) => {
                let reason = format!(
                    "supervisor restart: pane {} did not authoritatively confirm session {}",
                    pane.0, session.id
                );
                let outcome = give_up_on_disconnected_session(store, session.id, &reason)?;
                Outcome::GivenUp(outcome.into())
            }
            Err(e) => Outcome::SkippedAdapterError {
                detail: e.to_string(),
            },
        };

        records.push(ReconnectRecord {
            session_id: session.id,
            outcome,
        });
    }

    Ok(records)
}

/// Design §5's "Herdr restart: recreate sessions in their recorded
/// workspaces; unresolved running tasks become blocked" and "Machine
/// restart: recreate sessions in existing workspaces; queued tasks remain
/// queued," merged into backlog §9's single acceptance criterion: "sessions
/// are recreated only in recorded, existing workspaces; queued tasks remain
/// queued."
///
/// The presumption here is the opposite of
/// [`reconnect_after_supervisor_restart`]'s: the multiplexer or the machine
/// itself restarted, so a session is presumed gone unless `adapter` proves
/// otherwise. For each `disconnected` session:
///
/// 1. If a pane is recorded and observing it yields an authoritative,
///    identity-matching reading (the same two checks
///    [`reconnect_after_supervisor_restart`] uses), the session reconnects
///    exactly as it would there — this is the "it turns out this was not
///    really that kind of restart" escape hatch, and it must win over
///    recreation: putting a second harness in a workspace the first one
///    still holds is the exact corruption ADR 0012 decision 5's lease exists
///    to prevent.
/// 2. Otherwise, recreation turns on whether the recorded workspace still
///    exists on disk ([`workspace_exists_on_disk`]) — never on any pane
///    evidence, because Herdr itself may be the thing that is gone. If it
///    exists, the old session is given up on
///    ([`give_up_on_disconnected_session`]) and a brand new session is opened
///    in the same workspace via `factory_session::begin_start`, landing in
///    `starting` with no pane recorded yet — recording a real pane is what
///    happens once an operator actually launches a harness there (ADR 0017
///    decision 5: this slice records, it does not launch). If the workspace
///    is gone, the old session is given up on and nothing is recreated.
///
/// A `queued` task is never touched by this function at all — no branch here
/// reads or writes `tasks.status` for one — so "queued tasks remain queued"
/// holds by construction, not by an explicit check that could rot.
///
/// `max_sessions` is threaded through to `factory_session::begin_start`
/// rather than read from `factory-config`, which this crate does not depend
/// on; the caller already knows the agent's configured limit.
///
/// `next_session_id` mints the id for each recreated session, called once
/// per recreation. It is a callback rather than a single pre-supplied id
/// because this function does not know in advance how many sessions it will
/// recreate — the same reasoning `factory_task::create::create`'s own doc
/// comment gives for taking `id` from the caller: `uuid` is pinned
/// workspace-wide without the `v4` feature, the workspace manifest is
/// centrally owned, and so there is no `Uuid::new_v4()` this crate can call
/// itself.
///
/// # Errors
///
/// Any [`ReconnectError`] a read, a `factory_session` call, or
/// [`factory_paths::CanonicalPath::resolve`] produces. The last of these is
/// expected to be rare in practice — [`workspace_exists_on_disk`] is checked
/// first — but is not swallowed: a workspace that vanishes between that check
/// and this call is a real error, not a silent "give up," because silently
/// downgrading it would hide the exact race a future caller needs to know
/// about.
pub fn reconnect_after_herdr_or_machine_restart(
    store: &mut Store,
    adapter: &dyn Adapter,
    max_sessions: u32,
    mut next_session_id: impl FnMut() -> uuid::Uuid,
) -> Result<Vec<ReconnectRecord>, ReconnectError> {
    let mut records = Vec::new();

    for session in disconnected_sessions(store)? {
        if let Some(pane_id) = session.herdr_pane_id.clone() {
            let pane = PaneId(pane_id);
            if let Ok(observation) = adapter.observe(&pane) {
                if may_promote_from_disconnected(&observation)
                    && identifies_same_session(&session, &observation)
                {
                    factory_session::mark_running(store, session.id)?;
                    record_confirmed_identity(store, &session, &observation)?;
                    records.push(ReconnectRecord {
                        session_id: session.id,
                        outcome: Outcome::Reconnected,
                    });
                    continue;
                }
            }
        }

        let outcome = if workspace_exists_on_disk(&session.workspace_path) {
            let canonical = CanonicalPath::resolve(&session.workspace_path)?;
            let reason = format!(
                "herdr or machine restart: session {} presumed gone, recreating in {}",
                session.id, session.workspace_path
            );
            let given_up = give_up_on_disconnected_session(store, session.id, &reason)?;

            let new_session_id = next_session_id();
            factory_session::begin_start(
                store,
                new_session_id,
                session.scope_id,
                &session.agent_name,
                max_sessions,
                &canonical,
            )?;

            Outcome::Recreated {
                new_session_id,
                outcome: given_up.into(),
            }
        } else {
            let reason = format!(
                "herdr or machine restart: workspace {} no longer exists on disk",
                session.workspace_path
            );
            let given_up = give_up_on_disconnected_session(store, session.id, &reason)?;
            Outcome::GivenUp(given_up.into())
        };

        records.push(ReconnectRecord {
            session_id: session.id,
            outcome,
        });
    }

    Ok(records)
}

/// Everything that can go wrong reconciling live evidence against a
/// `disconnected` session.
#[derive(Debug, thiserror::Error)]
pub enum ReconnectError {
    #[error("store error: {0}")]
    Store(#[from] factory_store::StoreError),

    #[error("session error: {0}")]
    Session(#[from] factory_session::SessionError),

    #[error("path error: {0}")]
    Path(#[from] factory_paths::PathError),
}

#[cfg(test)]
mod tests {
    use factory_adapter::{Confidence, TaskSignal};

    use super::*;

    fn session(harness_session_id: Option<&str>) -> DisconnectedSession {
        DisconnectedSession {
            id: uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000001").expect("valid uuid"),
            scope_id: uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000002")
                .expect("valid uuid"),
            agent_name: "pi".to_string(),
            workspace_path: "/irrelevant".to_string(),
            herdr_pane_id: Some("wE:p1".to_string()),
            harness_session_id: harness_session_id.map(str::to_string),
        }
    }

    fn observation_with_id(id: Option<&str>) -> Observation {
        Observation {
            pane: PaneId("wE:p1".to_string()),
            harness_state: "idle".to_string(),
            confidence: Confidence::Authoritative,
            session_alive: true,
            task_signal: TaskSignal::NoChange,
            transcript_path: None,
            harness_session_id: id.map(str::to_string),
        }
    }

    #[test]
    fn matching_harness_session_id_identifies_the_same_session() {
        let s = session(Some("00000000-0000-0000-0000-000000000000"));
        let obs = observation_with_id(Some("00000000-0000-0000-0000-000000000000"));
        assert!(identifies_same_session(&s, &obs));
    }

    #[test]
    fn mismatched_harness_session_id_refutes_identity() {
        let s = session(Some("00000000-0000-0000-0000-000000000000"));
        let obs = observation_with_id(Some("11111111-1111-1111-1111-111111111111"));
        assert!(!identifies_same_session(&s, &obs));
    }

    #[test]
    fn missing_harness_session_id_on_either_side_falls_back_to_true() {
        let recorded = session(None);
        let observed_present = observation_with_id(Some("00000000-0000-0000-0000-000000000000"));
        assert!(identifies_same_session(&recorded, &observed_present));

        let recorded = session(Some("00000000-0000-0000-0000-000000000000"));
        let observed_absent = observation_with_id(None);
        assert!(identifies_same_session(&recorded, &observed_absent));
    }

    #[test]
    fn workspace_exists_on_disk_reflects_the_filesystem() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(workspace_exists_on_disk(
            dir.path().to_str().expect("utf8 path")
        ));
        assert!(!workspace_exists_on_disk(
            dir.path()
                .join("does-not-exist")
                .to_str()
                .expect("utf8 path")
        ));
    }
}
