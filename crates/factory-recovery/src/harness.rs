//! The two restart classes design §5 and backlog §9 leave unbuilt after
//! [`crate::reconnect`]: a single harness process dying while Factory still
//! believes its session is `running` ([`harness_crashed`]), and an operator
//! deliberately retiring an agent's harness in favour of a different one
//! ([`harness_changed`]). [`crate::restore`] and [`crate::reconnect`] cover
//! the other three rows of design §5's table — Factory supervisor restart,
//! Herdr restart, machine restart — and are finished work this module reads
//! but never edits.
//!
//! Both functions here are the same shape as [`crate::reconnect`]'s: they
//! take `&mut factory_store::Store` and evidence supplied by the caller, and
//! do nothing else. **No command, no daemon, no loop, no scheduler** — there
//! is no `factory` binary until slice 10 (crate docs, decision 1), and ADR
//! 0017 decision 5 is explicit that starting, sending, interrupting, and
//! stopping a harness remain documented operator procedures for this slice,
//! not something Factory automates. A "harness crash" notification and a
//! "the operator changed this agent's harness" decision both originate
//! outside this crate; what lands here is only the already-observed fact (an
//! [`Observation`], or a scope/agent pair whose live sessions must be
//! retired) and the database write that fact justifies.
//!
//! # Both classes reuse [`may_promote_from_disconnected`], not a copy of it
//!
//! Coordinator decision 3 (crate docs) names [`crate::evidence::
//! may_promote_from_disconnected`] the single home of "only authoritative
//! evidence may move a session out of `disconnected`," and warns that this
//! codebase has already paid twice for a rule re-derived at each call site.
//! Neither function below re-derives it — both call it — even though neither
//! is, on its face, "moving a session out of `disconnected`":
//!
//! - [`harness_crashed`] asks whether the evidence is trustworthy enough to
//!   *release* a lease and stop trusting a session Factory currently
//!   believes `running`. That is the same question read backwards: exactly
//!   as weak evidence must not be allowed to promote a session *into* trust
//!   (the rule's original statement), weak evidence must not be allowed to
//!   promote a session *out of* trust either. A `Confidence::Degraded`
//!   reading is Herdr falling back to screen detection (ADR 0017 decision
//!   2); building an irreversible, lease-releasing decision on that
//!   heuristic is exactly the hazard `factory_adapter`'s own module docs
//!   warn about for delivery, applied here to teardown instead.
//! - [`harness_changed`] asks the identical question of [`factory_session::
//!   stop`]'s own documented precondition: "the process this session names
//!   must already be confirmed no longer usable... not checked here." This
//!   crate is the only caller in a position to check it, since it is the
//!   only place a live [`Observation`] reaches a recovery decision at all
//!   (crate docs: "[`crate::reconnect`] is the only place a live observation
//!   is permitted to move a session"). Calling `stop()` without consulting
//!   this predicate first would assert a precondition this module held no
//!   evidence for — so it consults it, and its absence (or its refusal) is
//!   *why* the lease stays instead of being released.
//!
//! This is not a coincidence worth hiding: both classes ultimately answer
//! "may we act as though this session's fate is settled," and that is one
//! question with one gate, reused, not two similar-looking ones with two
//! homes.
//!
//! # `TaskSignal` is evidence this module deliberately does not consult
//!
//! `factory_adapter::TaskSignal` has no variant meaning "the task itself
//! failed," by design: `factory_adapter`'s own module docs state "no harness
//! state closes a task," and design §5 step 5 records completion "from the
//! result the agent reports, never inferred from the harness going quiet."
//! Neither function below infers a task's terminal status from an
//! `Observation` for exactly that reason — the vocabulary does not exist to
//! do it honestly. Design §5's "current task blocked **or failed**" is
//! answered instead by `factory_session::interrupt`'s own already-tested
//! guard (a task already `done`/`failed`/`cancelled` when a crash reaches it
//! is left exactly there, never overwritten to `blocked: interrupted`) — see
//! `authoritative_confirmed_crash_leaves_an_already_terminal_task_alone` in
//! `tests/harness_crash.rs`. A `Blocked` `TaskSignal` observed alongside a
//! crash is likewise not threaded into a specific `BlockedReason`:
//! `factory_task::blocked_reason_for` answers a different, unrelated
//! question (an alive harness reporting a block *during* normal operation),
//! and folding it in here would give the "why is this task blocked" decision
//! a second home the moment a crash and a stale block reading coincide,
//! fighting `interrupt()`'s own deliberate, tested overwrite-to-`interrupted`
//! (see that function's doc comment: "a task already `blocked` for another
//! reason... is *not* exempt").
//!
//! # [`harness_crashed`]'s evidence-to-outcome table
//!
//! Precondition: `session_id` is currently `running` — Factory believed the
//! harness alive until this `Observation` said otherwise. (A session Factory
//! has already lost sight of belongs to [`crate::reconnect`]'s exclusive
//! `disconnected`-row machinery; this function does not read `sessions.state`
//! itself and trusts its caller on this, exactly as
//! [`crate::reconnect::give_up_on_disconnected_session`]'s own doc comment
//! trusts its callers on the mirror-image precondition.)
//!
//! | `may_promote_from_disconnected` | `observation.session_alive` | the task | Outcome |
//! |---|---|---|---|
//! | `true` (Authoritative) | `false` | non-terminal | **Confirmed** — session → `failed` (lease released), task → `blocked: interrupted` |
//! | `true` (Authoritative) | `false` | already `done`/`failed`/`cancelled` | **Confirmed** — session → `failed`, task left exactly where it was (design §5's "or failed") |
//! | `true` (Authoritative) | `true` | — | **NotConfirmed** — the strongest evidence available contradicts the crash hypothesis; nothing changes |
//! | `false` (Degraded/Unavailable) | — | — | **Inconclusive** — session → `disconnected` (lease retained), task untouched, deferred to stronger evidence |
//!
//! The middle two rows collapse into one call to
//! [`crate::reconnect::give_up_on_disconnected_session`], which already
//! implements exactly this split (its `GiveUpOutcome::Interrupted` /
//! `GiveUpOutcome::Failed`) — see "Reusing `give_up_on_disconnected_session`
//! outside its literal `disconnected` precondition" below for why calling it
//! from a `running` session is calling the operation its own module docs
//! pre-authorise, not a workaround.
//!
//! # Reusing `give_up_on_disconnected_session` outside its literal `disconnected` precondition
//!
//! [`crate::reconnect::give_up_on_disconnected_session`]'s doc comment says
//! "`session_id` must already be `disconnected`" — a precondition inherited
//! from its two existing callers, both of which only ever reach it through
//! `disconnected`-row queries. Nothing about its *body* requires that:
//! `factory_session::interrupt` and `factory_session::fail` both accept
//! `running` as a source state for `→ failed` (`factory_session::
//! valid_targets(Running)` includes `Failed`), and [`crate::reconnect`]'s own
//! module doc names this exact reuse as the intended path forward: "design
//! §5's other two remaining classes — 'Harness crash'... and 'Harness
//! change'... — describe the identical operation and are explicitly not
//! this slice's to build; a later agent covering them should not have to
//! re-derive this one." [`harness_crashed`] is that later agent, calling the
//! function its predecessor named for exactly this purpose.
//!
//! # [`harness_changed`]'s evidence-to-outcome table
//!
//! For every session of `(scope_id, agent_name)` currently holding a lease
//! (`starting`, `running`, or `disconnected` — the set design §5 calls "the
//! old session(s)"):
//!
//! | The session's task | Evidence on its recorded pane | Outcome |
//! |---|---|---|
//! | `running`, or `queued` with a recorded delivery attempt (ADR 0019 decision 2: "may already have been delivered") | *(not consulted — see below)* | **RequiresReview** — session → `disconnected` (lease **retained**), task → `blocked: interrupted` |
//! | none of the above (a `queued` task with **no** attempt, if any, is untouched either way) | `starting`, so no pane was ever recorded | **Retired** — session → `failed` (never confirmed running; nothing legitimately "stopped" about it, mirroring `factory_session::valid_targets`'s own `Starting → Failed` reasoning) |
//! | none of the above | `may_promote_from_disconnected` true and `session_alive: false` | **Retired** — session → `stopped` (or `→ failed` from `starting`), lease released, `stop()`'s "confirmed no longer usable" precondition honestly met |
//! | none of the above | no pane recorded, the adapter itself failed, `Degraded`/`Unavailable`, or "still alive" | **Inconclusive** — session → `disconnected` (or left there), lease retained |
//!
//! **Running work — and a `queued` task that may already have reached the
//! old harness — short-circuits evidence entirely, and that is the whole
//! point of the "requires review" clause.** Backlog §9 asks for "a
//! human-visible state, not a log line," and design §5 already has one:
//! `blocked: interrupted` is the exact state `factory_session::interrupt`
//! (and [`crate::reconnect::give_up_on_disconnected_session`]) already use
//! for "this task's disposition needs a human's eyes." The session
//! deliberately does **not** move to `failed` in this row, regardless of
//! what any observation says: "a failed session releases its lease (ADR
//! 0012 decision 5); a session that merely needs review does not." A
//! released lease paired with an unreviewed, possibly-delivered prompt is
//! that same hazard restated in delivery terms rather than process terms:
//! an operator seeing the lease free might start a new harness in that
//! workspace before anyone has checked what the old one actually did there
//! — handing the same workspace to a second harness while a human has not
//! yet decided what became of the first one's work is exactly the
//! corruption that lease exists to prevent, and it must not depend on how
//! confident the evidence happens to be.
//!
//! The `queued`-with-an-attempt row is ADR 0019 decision 2's own
//! distinction, applied here rather than re-derived: that decision's restore
//! table moves exactly this shape — "a `queued` task that carries a
//! delivery attempt looks safe and is not, and the only reason Factory can
//! tell the difference is that design §5 puts the journal write before the
//! terminal write" — to `blocked: interrupted`, while a `queued` task with
//! *no* attempt at all stays `queued`. Backlog §9's unqualified "queued
//! tasks remain queued" is the same fact stated loosely; ADR 0019's table is
//! the precise version, and both describe work that was genuinely never
//! sent. Matching it here is what makes [`crate::restore::reconcile`] (ADR
//! 0019's table directly), the restart classes' give-up path (which
//! interrupts this same task once caught by its own broader "any
//! non-terminal task" sweep), and this function agree on the one case all
//! three can actually see. A `queued` task with no attempt is never matched
//! by [`task_under_review_of`] at all — nothing was ever sent, so "queued
//! tasks remain queued" holds by construction for that case, the same
//! guarantee [`crate::reconnect::reconnect_after_herdr_or_machine_restart`]'s
//! own doc comment makes for its own untouched `queued` rows.
//!
//! # Why the `RequiresReview` writes are session-first, task-second
//!
//! Unlike [`factory_session::interrupt`], moving the session and blocking
//! the task here are two separate commits — `factory_session::
//! mark_disconnected` and `factory_task::complete::blocked` each open and
//! commit their own transaction, and neither crate exposes a seam this one
//! can join them through (`factory_session::transition_in_tx` is private,
//! exactly as [`crate::reconnect::record_confirmed_identity`]'s own doc
//! comment already accepts for an equivalent gap). Session-first orders the
//! two writes so that a crash between them leaves the task still `running`
//! with the session already `disconnected` — the wrong state to leave
//! anything in, but the safer of the two orderings: the workspace lease is
//! already retained either way (`running` and `disconnected` both hold it),
//! and a `disconnected` session with a `running` task is a hazard a
//! *later* pass of this crate's machinery is prepared to reconcile (it is
//! exactly the row shape `factory_session::interrupt` exists to fix). The
//! reverse ordering — task blocked first, session left `running` after a
//! crash — would hide the fact that anything happened at all behind a
//! session row that still claims to be in active service.

use factory_adapter::{Adapter, Observation, PaneId};
use factory_store::Store;
use rusqlite::OptionalExtension;

use crate::evidence::may_promote_from_disconnected;
use crate::reconnect::{GiveUpOutcomeSummary, give_up_on_disconnected_session};

/// What [`harness_crashed`] concluded, per the evidence-to-outcome table
/// above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashOutcome {
    /// Authoritative evidence confirmed the harness gone. Wraps
    /// [`crate::reconnect::give_up_on_disconnected_session`]'s own summary,
    /// since that is the call this variant reports the result of.
    Confirmed(GiveUpOutcomeSummary),
    /// Authoritative evidence said the session is in fact alive. The
    /// strongest possible reading contradicts the crash hypothesis, so
    /// nothing was changed.
    NotConfirmed,
    /// The evidence was not trustworthy enough to conclude either way.
    /// `session_id` moved to (or, if already there, stayed) `disconnected`;
    /// its task, if any, was not touched.
    Inconclusive,
}

/// Design §5's "Harness crash: mark session failed and current task blocked
/// or failed" (backlog §9: "...according to documented evidence") — the
/// module doc comment above has the full evidence-to-outcome table this
/// implements.
///
/// `session_id` is presumed `running` (see the module docs' precondition
/// note); `observation` is whatever [`factory_adapter::Adapter::observe`]
/// most recently returned for its recorded pane. Calling this on a session
/// that is not currently `running` is a caller error this function cannot
/// detect further than `factory_session`'s own transition errors already
/// will — precisely the stance [`crate::reconnect::
/// give_up_on_disconnected_session`]'s own doc comment takes on its mirror
/// precondition.
///
/// # Not idempotent, unlike [`harness_changed`]
///
/// This is a deliberate, load-bearing difference from its sibling.
/// [`harness_changed`] is a batch function a caller may re-run at any time
/// over the same `(scope_id, agent_name)` and must tolerate finding its own
/// previous writes; this function represents one specific, one-time crash
/// notification for one session presumed `running` *right now*, and every
/// one of its three non-error outcomes leaves `session_id` somewhere other
/// than `running` (`failed` or `disconnected`). A second call for the same
/// notification is therefore a caller bug, not a state this function
/// tolerates: every branch it can reach ends in a
/// [`factory_session::SessionError::InvalidTransition`] on the repeat,
/// because neither `failed` nor `disconnected` has a `factory_session::
/// valid_targets` edge back to itself for the write this function would
/// attempt next (`Failed` is terminal; `Disconnected → Disconnected` is not
/// granted). See `a_second_crash_notification_for_the_same_session_is_a_caller_error`
/// in `tests/harness_crash.rs`.
///
/// # Errors
///
/// [`factory_session::SessionError::InvalidTransition`] if `session_id` is
/// not currently in a state this function's chosen edge can leave from —
/// most notably, calling this on an already-`disconnected` or already-
/// `failed` session (see "Not idempotent" above), since neither
/// `disconnected → disconnected` nor any edge out of `failed` is one
/// `factory_session::valid_targets` grants. Any other
/// [`factory_session::SessionError`] or [`factory_store::StoreError`]
/// [`crate::reconnect::give_up_on_disconnected_session`] or
/// `factory_session::mark_disconnected` themselves produce, verbatim.
pub fn harness_crashed(
    store: &mut Store,
    session_id: uuid::Uuid,
    observation: &Observation,
) -> Result<CrashOutcome, HarnessError> {
    if !may_promote_from_disconnected(observation) {
        factory_session::mark_disconnected(store, session_id)?;
        return Ok(CrashOutcome::Inconclusive);
    }

    if observation.session_alive {
        return Ok(CrashOutcome::NotConfirmed);
    }

    let reason = format!(
        "harness crash: pane {} authoritatively reported session {session_id} gone",
        observation.pane.0
    );
    let outcome = give_up_on_disconnected_session(store, session_id, &reason)?;
    Ok(CrashOutcome::Confirmed(outcome.into()))
}

/// What [`harness_changed`] concluded for one old session, per the
/// evidence-to-outcome table above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessChangeOutcome {
    /// No `running` task; the old process was confirmed gone (or the
    /// session never reached `running` at all), so its lease was released
    /// cleanly (`stopped`, or `failed` from `starting`).
    Retired,
    /// A `running` task's disposition under the old harness is unresolved —
    /// or a `queued` task carries at least one delivery attempt (ADR 0019
    /// decision 2: it "looks safe and is not"). The session moved to (or
    /// stayed) `disconnected`, **never** `failed` or `stopped` — its lease
    /// is retained until a human resolves the task this names, which is
    /// `blocked: interrupted`.
    RequiresReview { task_id: uuid::Uuid },
    /// No `running` task, but the evidence neither confirmed the old
    /// process gone nor ruled it out. The session moved to (or stayed)
    /// `disconnected`, retaining its lease until stronger evidence — or an
    /// operator — resolves it.
    Inconclusive,
}

/// One old session's fate, for the report [`harness_changed`] returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HarnessChangeRecord {
    pub session_id: uuid::Uuid,
    pub outcome: HarnessChangeOutcome,
}

/// One session of `(scope_id, agent_name)` currently holding a lease, as
/// [`harness_changed`] needs to see it: enough to decide whether it ever
/// reached `running` and where to ask `adapter` to look.
struct OldSession {
    id: uuid::Uuid,
    state: factory_session::SessionState,
    herdr_pane_id: Option<String>,
}

fn old_sessions_of(
    store: &Store,
    scope_id: uuid::Uuid,
    agent_name: &str,
) -> Result<Vec<OldSession>, HarnessError> {
    let mut stmt = store
        .connection()
        .prepare(
            "SELECT id, state, herdr_pane_id FROM sessions \
             WHERE scope_id = ?1 AND agent_name = ?2 \
             AND state IN ('starting', 'running', 'disconnected') \
             ORDER BY created_at, id",
        )
        .map_err(factory_store::StoreError::from)?;
    let rows = stmt
        .query_map((scope_id.to_string(), agent_name), |row| {
            let id: String = row.get(0)?;
            let state: String = row.get(1)?;
            let herdr_pane_id: Option<String> = row.get(2)?;
            Ok((id, state, herdr_pane_id))
        })
        .map_err(factory_store::StoreError::from)?;
    rows.map(|r| {
        let (id, state, herdr_pane_id) = r.map_err(factory_store::StoreError::from)?;
        Ok(OldSession {
            id: uuid::Uuid::parse_str(&id)
                .unwrap_or_else(|e| panic!("sessions.id is a UUID; read {id:?}: {e}")),
            state: factory_session::SessionState::from_db_str(&state),
            herdr_pane_id,
        })
    })
    .collect()
}

/// Whether a task [`task_under_review_of`] found needs [`harness_changed`]
/// to write anything, or was already left exactly where an earlier pass —
/// this function's own, [`harness_crashed`]'s, or a restart class's — put
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewState {
    /// `running`, or `queued` with at least one recorded delivery attempt.
    /// Never flagged before; both writes in [`harness_changed`]'s matching
    /// arm are new.
    NeedsFlagging,
    /// Already `blocked: interrupted`. Nothing to write; the session is
    /// already wherever this state belongs.
    AlreadyFlagged,
}

/// The one task under review for `session_id`, if any: `running` right now,
/// `queued` with at least one recorded delivery attempt, or already
/// `blocked: interrupted` from an earlier pass.
///
/// # Why `queued`-with-an-attempt is matched, and `queued`-with-none is not
///
/// This is ADR 0019 decision 2's own distinction, applied here rather than
/// re-derived: that decision's restore table moves a `queued` task carrying
/// a delivery attempt to `blocked: interrupted` — "it looks safe and is
/// not, and the only reason Factory can tell the difference is that design
/// §5 puts the journal write before the terminal write" — while a `queued`
/// task with *no* attempt at all stays `queued`. Backlog §9's unqualified
/// "queued tasks remain queued" is the same fact stated loosely; both
/// describe work that was genuinely never sent. Matching this distinction
/// here, instead of either sweeping every `queued` task in like
/// [`crate::reconnect`]'s private `non_terminal_task_of` does, or ignoring
/// delivery history like an earlier version of this function did, is what
/// makes [`crate::restore::reconcile`] (ADR 0019's table directly), the
/// restart classes' give-up path (which interrupts this same shape of task
/// once caught by its own broader "any non-terminal task" sweep), and this
/// function agree on the one case all three can actually see. See
/// `a_queued_task_with_no_delivery_attempt_stays_queued_because_nothing_was_sent`
/// and
/// `a_queued_task_with_a_prior_delivery_attempt_requires_review_like_running_work`
/// in `tests/harness_change.rs`.
///
/// **Also matches a task already `blocked: interrupted`** ([`ReviewState::
/// AlreadyFlagged`]), which is what makes [`harness_changed`] idempotent
/// (see its own doc comment's "Idempotence" section): once a task has been
/// flagged once, the second call must recognise it in its new resting state
/// and treat the session exactly as it did the first time — never fall
/// through to the no-review branch, which would ask `adapter` about a pane
/// and could retire (release the lease of) a session whose task a human has
/// not yet reviewed. `blocked: interrupted` is deliberately the *only*
/// blocked reason matched here, not "any `blocked` task": a task already
/// `blocked: permission` or `blocked: clarification` when this function
/// finds it was never work this function itself put on hold, and forcing it
/// into "review" would misreport an ordinary, unrelated wait as a
/// harness-change casualty.
fn task_under_review_of(
    store: &Store,
    session_id: uuid::Uuid,
) -> Result<Option<(uuid::Uuid, ReviewState)>, HarnessError> {
    let row: Option<(String, String, Option<String>)> = store
        .connection()
        .query_row(
            &format!(
                "SELECT id, status, blocked_reason FROM tasks WHERE assigned_session_id = ?1 \
                 AND (status = 'running' \
                      OR (status = 'queued' AND EXISTS( \
                            SELECT 1 FROM delivery_attempts \
                            WHERE delivery_attempts.task_id = tasks.id \
                            AND {predicate})) \
                      OR (status = 'blocked' AND blocked_reason = 'interrupted')) \
                 ORDER BY created_at, id LIMIT 1",
                predicate = factory_task::deliver::ATTEMPT_MAY_HAVE_REACHED_THE_TERMINAL
            ),
            [session_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(factory_store::StoreError::from)?;
    Ok(row.map(|(id, status, blocked_reason)| {
        let task_id = uuid::Uuid::parse_str(&id)
            .unwrap_or_else(|e| panic!("tasks.id is a UUID; read {id:?}: {e}"));
        let state = if status == "blocked" && blocked_reason.as_deref() == Some("interrupted") {
            ReviewState::AlreadyFlagged
        } else {
            ReviewState::NeedsFlagging
        };
        (task_id, state)
    }))
}

/// Design §5's "Harness change: stop the old session; queued tasks remain;
/// running task requires review" — the module doc comment above has the
/// full evidence-to-outcome table this implements.
///
/// Every currently lease-holding session of `agent_name` in `scope_id` is
/// visited once; `adapter` is only ever asked about a session with **no**
/// `running` (and no already-flagged `blocked: interrupted`) task, since
/// running work short-circuits evidence entirely (see the module docs).
///
/// # Idempotence
///
/// Calling this twice in a row for the same `(scope_id, agent_name)`, with
/// nothing else having changed, must leave the second call's writes at
/// none — coordinator decision 4's rule applies here exactly as it does to
/// [`crate::restore::reconcile`] and [`crate::reconnect`]'s own functions.
/// The **RequiresReview** row is the one that needs active protection to get
/// this right, not merely a re-read that happens to match: a session this
/// function already moved to `disconnected` and flagged still passes
/// `old_sessions_of`'s `state IN (...)` filter on the next call (unlike
/// `crate::reconnect::disconnected_sessions`, `disconnected` is one of the
/// *inputs* this function accepts, not only a state it produces), and
/// without [`task_under_review_of`] also matching the now-`blocked:
/// interrupted` task, the second call would see "no running task," ask
/// `adapter` about the session's pane, and — given the same authoritative,
/// confirmed-dead evidence a real operator would still have on hand — retire
/// it (`stop()`, releasing the lease) before any human ever looked at the
/// review this function itself raised. See
/// `calling_it_twice_does_not_retire_a_session_still_awaiting_review` in
/// `tests/harness_change.rs`, whose mutation (reverting to the
/// `status = 'running'`-only query) is exactly this idempotence bug
/// restored — distinct from the queued-retention mutation the module's own
/// test file also carries, which additionally needs the projected "is this
/// freshly running" flag forced `true` before a queued row stops landing in
/// the harmless no-op arm.
///
/// # Errors
///
/// Any [`factory_session::SessionError`], [`factory_store::StoreError`], or
/// [`factory_task::complete::CompleteError`] a read or a state-transition
/// call produces.
pub fn harness_changed(
    store: &mut Store,
    scope_id: uuid::Uuid,
    agent_name: &str,
    adapter: &dyn Adapter,
) -> Result<Vec<HarnessChangeRecord>, HarnessError> {
    let mut records = Vec::new();

    for session in old_sessions_of(store, scope_id, agent_name)? {
        let outcome = match task_under_review_of(store, session.id)? {
            Some((task_id, ReviewState::NeedsFlagging)) => {
                // Freshly `running`, or `queued` with a delivery attempt:
                // this is the first time this task has been flagged, so
                // both writes below are new. Session first, task second —
                // see the module docs' "Why the RequiresReview writes are
                // session-first, task-second."
                if session.state == factory_session::SessionState::Running {
                    factory_session::mark_disconnected(store, session.id)?;
                }
                // Already `disconnected`: already the honest, lease-holding
                // "needs review" state — nothing to change on the session
                // side.
                factory_task::complete::blocked(
                    store,
                    task_id,
                    factory_task::BlockedReason::Interrupted,
                )?;
                HarnessChangeOutcome::RequiresReview { task_id }
            }
            Some((task_id, ReviewState::AlreadyFlagged)) => {
                // Already `blocked: interrupted` — a prior call's own
                // flagging (or `harness_crashed`'s, or a restart class's)
                // that no human has resolved yet. Write nothing: the session
                // is already wherever it belongs, and re-running `blocked()`
                // on an already-`blocked` task is refused by
                // `factory_task`'s own transition table in any case
                // (`Blocked` is not among `Blocked`'s own valid targets).
                HarnessChangeOutcome::RequiresReview { task_id }
            }
            None => {
                harness_change_outcome_with_no_running_task(store, agent_name, adapter, &session)?
            }
        };

        records.push(HarnessChangeRecord {
            session_id: session.id,
            outcome,
        });
    }

    Ok(records)
}

/// The no-`running`-task half of [`harness_changed`]'s table, split out only
/// to keep that function's loop body flat — same reasoning
/// [`crate::reconnect::give_up_on_disconnected_session`] gives for its own
/// extraction from the two restart-class loops.
fn harness_change_outcome_with_no_running_task(
    store: &mut Store,
    agent_name: &str,
    adapter: &dyn Adapter,
    session: &OldSession,
) -> Result<HarnessChangeOutcome, HarnessError> {
    if session.state == factory_session::SessionState::Starting {
        // Never confirmed running, and migration 5's pane columns are only
        // ever written by `crate::reconnect::record_confirmed_identity` on a
        // `disconnected → running` reconnection — a `starting` session never
        // has one to ask `adapter` about. There is nothing legitimately
        // "stopped" about a launch that never got that far, exactly
        // `factory_session::valid_targets`'s own reasoning for why
        // `Starting → Failed` exists and `Starting → Stopped` does not.
        let reason = format!(
            "harness changed: agent `{agent_name}`'s harness changed before session {} ever confirmed running",
            session.id
        );
        factory_session::fail(store, session.id, &reason)?;
        return Ok(HarnessChangeOutcome::Retired);
    }

    let confirmed_dead = session
        .herdr_pane_id
        .as_ref()
        .and_then(|pane_id| adapter.observe(&PaneId(pane_id.clone())).ok())
        .is_some_and(|obs| may_promote_from_disconnected(&obs) && !obs.session_alive);

    if confirmed_dead {
        let reason = format!(
            "harness changed: agent `{agent_name}` no longer uses this session's harness, and its pane authoritatively confirmed it gone"
        );
        factory_session::stop(store, session.id, &reason)?;
        return Ok(HarnessChangeOutcome::Retired);
    }

    if session.state == factory_session::SessionState::Running {
        factory_session::mark_disconnected(store, session.id)?;
    }
    // Already `disconnected`: already the correct, lease-holding "not yet
    // confirmed" state.
    Ok(HarnessChangeOutcome::Inconclusive)
}

/// Everything [`harness_crashed`] or [`harness_changed`] can return beyond
/// what they explicitly document.
#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("store error: {0}")]
    Store(#[from] factory_store::StoreError),

    #[error("session error: {0}")]
    Session(#[from] factory_session::SessionError),

    #[error("task error: {0}")]
    Task(#[from] factory_task::complete::CompleteError),

    #[error("reconnect error: {0}")]
    Reconnect(#[from] crate::reconnect::ReconnectError),
}
