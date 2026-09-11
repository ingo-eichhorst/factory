//! The rule itself: whether a sender may target a scope at all, and whether
//! that scope has already appeared in the task's delegation chain.
//!
//! Two independent gates, always run in this order:
//!
//! 1. **Registration and kinship** (design §6; crate docs decision 3). Both
//!    design §6 and backlog §8's first acceptance criterion say "any
//!    **registered** scope" — the word is load-bearing, so a human sender's
//!    unconditional pass is unconditional on *kinship* only, never on
//!    whether `target_scope_id` names a real row. An agent sender must find
//!    its target a registered [`Kinship::Descendant`] or [`Kinship::Sibling`]
//!    of its own scope; [`Kinship::SameScope`], [`Kinship::Ancestor`], and
//!    [`Kinship::Unrelated`] (what a nephew or cousin resolves to —
//!    `factory_registry::kinship`'s own doc comment) are refused. This crate
//!    never walks ancestry, and never checks existence, itself:
//!    [`factory_registry::kinship`] is the one home of both — for
//!    [`Sender::Agent`] because it computes kinship at all, and for
//!    [`Sender::Human`] because [`check`] calls it purely for the existence
//!    check its own doc comment describes doing unconditionally, before
//!    kinship is ever decided (`kinship(conn, target_scope_id,
//!    target_scope_id)` — see [`check`]'s doc comment for why asking a
//!    scope's kinship to itself is exactly that check with nothing else
//!    attached).
//! 2. **Chain membership** (design §6; crate docs decisions 4–6). Regardless
//!    of who is asking, a target already present in the chain the task has
//!    *already* travelled is refused with this crate's own
//!    [`DelegationError::AlreadyInChain`], before [`crate::queue`] ever
//!    calls `factory_task::create::create`. `task_delegation_chain`'s
//!    `UNIQUE (task_id, scope_id)` stays as the database's own last-resort
//!    backstop (crate docs, decision 6) — but a rusqlite constraint error is
//!    never what this function, or either `queue` entry point, returns.
//!
//! Gate 1 runs first so the two gates stay independently testable: a target
//! that is both ineligible under gate 1 *and* already in the chain must
//! still be attributable to a single cause, and the crate report's required
//! mutations (disable one gate, name the test that dies) only work if the
//! surviving gate cannot silently also catch what the disabled one was
//! supposed to.
//!
//! [`check`] never talks to `factory_store` directly and never opens a
//! transaction — [`crate::queue`] does both, and only after this function
//! returns `Ok(())`.

use factory_registry::Kinship;

/// Who is asking, already resolved to a scope identity (or "no scope at
/// all") by the caller.
///
/// Never constructed from a bare, caller-supplied scope id claiming to be an
/// agent — crate docs, decision 1: `Agent`'s `scope_id` must come from
/// `factory_session::scope_of_session`, resolved from the *session id* the
/// caller actually authenticated as. "[A] caller that may name its own
/// sender scope has no rule left to break."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sender {
    /// Design §6: "a human may target any registered scope" — kinship is
    /// never even asked, but "registered" is still checked (see [`check`]).
    Human,
    /// An agent session, already resolved to the scope it runs in.
    Agent { scope_id: uuid::Uuid },
}

/// Every way [`check`] can refuse a delegation, plus the errors this crate's
/// two entry points ([`crate::queue::queue_from_human`],
/// [`crate::queue::queue_from_session`]) surface while resolving what they
/// need before they can even call `check` — one error type for the whole
/// crate, mirroring `factory_task::TaskError` and
/// `factory_session::SessionError`'s own shape.
#[derive(Debug, thiserror::Error)]
pub enum DelegationError {
    #[error("registry error: {0}")]
    Registry(#[from] factory_registry::RegistryError),

    #[error("session error: {0}")]
    Session(#[from] factory_session::SessionError),

    #[error("task error: {0}")]
    Task(#[from] factory_task::TaskError),

    /// Kinship refused the target — the first gate in [`check`]. `reason`
    /// is a fixed, already-complete phrase for the refused [`Kinship`]
    /// variant (never `Descendant`/`Sibling` — see [`refusal_reason`], this
    /// variant's only producer), plain enough that the `Display` impl only
    /// has to drop it into "{target} is {reason}" without gluing on
    /// anything else — an earlier version appended " of {sender}" here,
    /// which read as nonsense (`"… is its own ancestor of …"`) precisely
    /// because `reason` already names the sender itself where needed
    /// (`"an ancestor of the sender"`, `"unrelated to the sender (…)"`).
    /// `kinship` is kept alongside `reason` — not just derivable from it —
    /// so a caller with its own idea of how to word a refusal for a human
    /// (`ops::agent::list`, ADR 0022 decision 8) can match on the
    /// structured value directly, rather than pattern-matching this
    /// crate's own display text or re-deriving kinship a second time.
    #[error(
        "scope {sender} may not target scope {target} — {target} is {reason}\n  help: design §6: an agent may target only a registered descendant or sibling scope; a nephew or cousin is reached through its parent"
    )]
    NotEligible {
        sender: uuid::Uuid,
        target: uuid::Uuid,
        kinship: Kinship,
        reason: &'static str,
    },

    /// The chain-membership gate refused the target — this crate's own
    /// typed error for design §6's "a scope already in that chain cannot be
    /// targeted again," returned *before* any row is written (crate docs,
    /// decision 6). `task_delegation_chain`'s `UNIQUE (task_id, scope_id)`
    /// is the database's own backstop for the same rule, but a rusqlite
    /// constraint error is never what surfaces here.
    #[error(
        "scope {target} already appears in this task's delegation chain\n  help: design §6: a scope already in the chain cannot be targeted again — refused before any row is written"
    )]
    AlreadyInChain { target: uuid::Uuid },
}

/// The fixed refusal phrase for one of [`check`]'s three ineligible
/// kinships. Takes the already-computed [`Kinship`] rather than
/// re-deriving it, and is only ever called with one of those three — see
/// [`check`], its only caller, which never reaches the `unreachable!` arm
/// because it only calls this from the match arm that already excluded
/// `Descendant` and `Sibling`.
fn refusal_reason(kinship: Kinship) -> &'static str {
    match kinship {
        // `target == sender` whenever this kinship is refused, but the
        // `Display` template still shows both — "the sender itself" says
        // plainly that they are the same scope, rather than trusting a bare
        // "itself" to bind back to the right noun once dropped into a
        // sentence with two ids already in it.
        Kinship::SameScope => "the sender itself",
        Kinship::Ancestor => "an ancestor of the sender",
        Kinship::Unrelated => {
            "unrelated to the sender (a nephew or cousin scope — design §6 says such work goes through the target's parent)"
        }
        Kinship::Descendant | Kinship::Sibling => {
            unreachable!("check() only calls refusal_reason for a kinship it is about to refuse")
        }
    }
}

/// The whole rule, both gates, in the order described in the module docs.
///
/// `existing_chain` is the chain **before** `target_scope_id` would be
/// appended to it — `&[]` for [`Sender::Human`] (crate docs, decision 5: a
/// human-queued task's chain is exactly `[target]`, so nothing has been
/// travelled through yet), and whatever [`crate::queue::queue_from_session`]
/// already resolved via `factory_task::assign::running_task_of_session` /
/// `factory_task::create::delegation_chain_of` for [`Sender::Agent`].
///
/// # Errors
///
/// [`DelegationError::Registry`] wrapping
/// [`factory_registry::RegistryError::UnknownScope`] if `target_scope_id`
/// names no registered scope — for *either* sender kind. For
/// [`Sender::Agent`] this falls out of the kinship lookup itself (8A's
/// `kinship` doc comment: "[b]oth existence checks run unconditionally...
/// before the `SameScope` short-circuit"). For [`Sender::Human`], `check`
/// asks `factory_registry::kinship(conn, target_scope_id, target_scope_id)`
/// — a scope's kinship to itself is always [`Kinship::SameScope`] once both
/// existence checks (against the same id, but still both) pass, and that
/// result is discarded; only the existence check inside it is wanted, and
/// this crate does not own a second way to ask "does this scope exist" —
/// `factory_registry` does, and this is it, reused rather than duplicated
/// (this task's report, item 1).
///
/// [`DelegationError::NotEligible`] if an agent sender's kinship to the
/// target is [`Kinship::SameScope`], [`Kinship::Ancestor`], or
/// [`Kinship::Unrelated`]. Never returned for [`Sender::Human`] — design §6
/// exempts a human sender from kinship entirely, only from existence (see
/// above).
///
/// [`DelegationError::AlreadyInChain`] if `target_scope_id` is already a
/// member of `existing_chain`, for either sender kind.
pub fn check(
    conn: &rusqlite::Connection,
    sender: Sender,
    target_scope_id: uuid::Uuid,
    existing_chain: &[uuid::Uuid],
) -> Result<(), DelegationError> {
    match sender {
        Sender::Human => {
            // Existence only — kinship stays unconditional for a human
            // sender (design §6). `factory_registry::require_registered` is
            // the named existence check `factory_registry` exposes for
            // exactly this — asking a scope's kinship to itself, and
            // discarding the answer, was this crate's own workaround before
            // that function existed (8A's task report, item 2).
            factory_registry::require_registered(conn, target_scope_id)?;
        }
        Sender::Agent { scope_id } => {
            let kinship = factory_registry::kinship(conn, scope_id, target_scope_id)?;
            match kinship {
                Kinship::Descendant | Kinship::Sibling => {}
                Kinship::SameScope | Kinship::Ancestor | Kinship::Unrelated => {
                    return Err(DelegationError::NotEligible {
                        sender: scope_id,
                        target: target_scope_id,
                        kinship,
                        reason: refusal_reason(kinship),
                    });
                }
            }
        }
    }

    if existing_chain.contains(&target_scope_id) {
        return Err(DelegationError::AlreadyInChain {
            target: target_scope_id,
        });
    }

    Ok(())
}
