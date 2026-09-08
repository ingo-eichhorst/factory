//! Coordinator decision 3 (crate docs): "only authoritative evidence may move
//! a session out of `disconnected`" has exactly one home. This is it.
//!
//! This codebase has already paid twice for a rule re-derived at each call
//! site instead of named once — the lease-holding set
//! ([`factory_session::SessionState::holds_lease`], slice 7) and ancestry
//! (slice 8). [`may_promote_from_disconnected`] is this rule's turn: every
//! caller in [`crate::reconnect`] that considers moving a session out of
//! [`factory_session::SessionState::Disconnected`] calls this function and
//! cites it, rather than inlining a match on
//! [`factory_adapter::Confidence`] of its own.

use factory_adapter::{Confidence, Observation};

/// Whether `observation` is, on its own, sufficient reason to move a session
/// out of [`factory_session::SessionState::Disconnected`].
///
/// The answer is exactly "was this reported through the Herdr lifecycle
/// hook" — [`Confidence::Authoritative`] — and nothing else about the
/// observation matters to this question:
///
/// - [`Confidence::Degraded`] means Herdr answered by reading the screen
///   instead. ADR 0017 decision 2 built that fallback for a harness that
///   loses its state extension, and `factory_adapter`'s own module docs are
///   explicit about why it stops here: "recorded, never acted on: at-most-once
///   delivery must not be built on a heuristic." Promoting a session is
///   exactly the kind of act this warns against — it is what makes
///   `factory_task::deliver::deliver`'s at-most-once guarantee resume acting
///   on a session again.
/// - [`Confidence::Unavailable`] is no answer — a stopped Herdr, an unknown
///   pane. `factory_adapter::Adapter::observe`'s own contract sends the
///   caller to manual confirmation in that case; a human, not this function,
///   decides what happens next.
///
/// # Why a pane match is never enough on its own
///
/// ADR 0019 decision 3: a snapshot's (or a stale `sessions` row's) pane
/// identifiers "may since have been reused by entirely different sessions,
/// so a pane that exists is not evidence that *this* session exists." A
/// `sessions.herdr_pane_id` value only says *where to look* — which pane
/// `factory_adapter::Adapter::observe` should be asked about. It is never
/// itself the input to this function, and that omission is deliberate: if a
/// caller could pass "yes, a pane is recorded" instead of an actual
/// [`Observation`], the pane-reuse hazard ADR 0019 names would slip back in
/// through the one door this predicate exists to close. What the harness
/// *reported* is the only thing this function looks at — see
/// [`crate::reconnect`]'s own identity correlation for the separate question
/// of whether the observation just obtained is even about the *same* session
/// the recorded pane id once named, which this predicate does not and must
/// not answer (see below).
///
/// # Why this is not a duplicate of `factory_task::blocked_reason_for`
///
/// `factory_task::blocked_reason_for` also starts with
/// `observation.confidence != Confidence::Authoritative` and bails to `None`.
/// That is not a second home for this rule: it answers "what reason should a
/// *task* be blocked for," a question this predicate never asks, and it lives
/// in a crate this one cannot edit or depend on in that direction. Both
/// functions independently reach the same one-line gate because
/// `factory_adapter::Confidence` draws one line and both consumers must
/// respect it — the gate is duplicated in the trivial, unavoidable sense that
/// two crates each read `Confidence::Authoritative` off the same enum, not in
/// the sense coordinator decision 3 forbids, which is a *decision rule*
/// re-derived in more than one place. Nothing here re-derives anything:
/// `factory_task` cannot depend on `factory-recovery` (the dependency runs
/// the other way — see this crate's `Cargo.toml`), so it could not call this
/// function even if the two questions were the same one, which they are not.
///
/// # What this function deliberately does not decide
///
/// It says nothing about whether the pane `observation` came from is the
/// *same* pane recorded for the session under consideration, whether the
/// harness session id correlates, or whether the session is even still
/// running in the sense [`Observation::session_alive`] means. Those are
/// identity questions, not trust questions, and [`crate::reconnect`] asks
/// them separately — folding them in here would let "the pane matched" and
/// "the evidence was authoritative" collapse back into one check, which is
/// the exact confusion ADR 0019 decision 3 warns about.
#[must_use]
pub fn may_promote_from_disconnected(observation: &Observation) -> bool {
    observation.confidence == Confidence::Authoritative
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use factory_adapter::{BlockedReason, PaneId, TaskSignal};

    use super::*;

    fn observation(confidence: Confidence) -> Observation {
        Observation {
            pane: PaneId("wE:p1".to_string()),
            harness_state: "working".to_string(),
            confidence,
            session_alive: true,
            task_signal: TaskSignal::Running,
            transcript_path: None,
            harness_session_id: None,
        }
    }

    #[test]
    fn authoritative_may_promote() {
        assert!(may_promote_from_disconnected(&observation(
            Confidence::Authoritative
        )));
    }

    #[test]
    fn degraded_may_not_promote() {
        assert!(!may_promote_from_disconnected(&observation(
            Confidence::Degraded
        )));
    }

    #[test]
    fn unavailable_may_not_promote() {
        assert!(!may_promote_from_disconnected(&observation(
            Confidence::Unavailable
        )));
    }

    /// Every field of `Observation` besides `confidence` is irrelevant to
    /// this predicate — including a `session_alive: false` reading paired
    /// with `Authoritative`, and a fully populated transcript/harness-session
    /// identity. This is what "the predicate never sees the pane id, only
    /// the observation's own trustworthiness" means in a runnable form: the
    /// answer must track `confidence` alone through every other field this
    /// struct carries, including ones a future field addition might add.
    #[test]
    fn only_confidence_matters() {
        let mut obs = observation(Confidence::Authoritative);
        obs.session_alive = false;
        obs.task_signal = TaskSignal::Blocked(BlockedReason::Permission);
        obs.transcript_path = Some(PathBuf::from("/irrelevant"));
        obs.harness_session_id = Some("11111111-1111-1111-1111-111111111111".to_string());
        assert!(may_promote_from_disconnected(&obs));
    }
}
