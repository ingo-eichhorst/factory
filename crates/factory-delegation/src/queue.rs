//! The two entry points that queue a task: one for a human, one for an agent
//! session. Each computes the delegation chain, applies [`crate::rule`], and
//! only then calls `factory_task::create::create`.
//!
//! Both take an `id` the caller mints — mirrors `factory_task::create::create`
//! and `factory_session::begin_start`'s own `id: uuid::Uuid` parameters (see
//! `create`'s doc comment: the `uuid` crate is pinned workspace-wide without
//! the `v4` feature, and the workspace manifest is centrally owned, so
//! nothing in this crate can mint one either).
//!
//! # Why the rule check always runs before any row is written
//!
//! Backlog §8's acceptance criterion is explicit: "[r]ejection leaves no
//! task or delivery record." Both functions below compute whatever chain
//! [`crate::rule::check`] needs, call it, and only reach
//! `factory_task::create::create` — the single place either function ever
//! writes anything — on `Ok(())`. A refusal returns before that call, so no
//! `tasks`, `task_delegation_chain`, or `delivery_attempts` row is ever
//! written for it; nothing in this module writes any of those tables by any
//! other path.

use factory_store::Store;
use uuid::Uuid;

use crate::rule::{self, DelegationError, Sender};

/// Queue a task from a human to `target_scope_id`.
///
/// Crate docs, decision 5: a human-queued task's delegation chain is
/// exactly `[target]` — the task has passed through one scope, the one it
/// was just sent to. The `existing_chain` passed to [`rule::check`] is
/// therefore `&[]`: nothing has been travelled through yet, so there is
/// nothing `target_scope_id` could already be a member of, and the
/// chain-membership gate can never fire for this entry point.
///
/// `sender_scope_id` recorded on the task row is `None` —
/// `factory_task::create::Task::sender_scope_id`'s own doc comment: "`None`
/// means the sender was a human, not another scope."
///
/// # Errors
///
/// [`DelegationError::Registry`] wrapping
/// [`factory_registry::RegistryError::UnknownScope`] if `target_scope_id`
/// names no registered scope — design §6 and backlog §8's first acceptance
/// criterion both say "any **registered** scope"; [`rule::check`] enforces
/// the word even for [`Sender::Human`], which is exempt from kinship but not
/// from existence. See [`rule::check`] and `factory_task::create::create`
/// for the rest.
pub fn queue_from_human(
    store: &mut Store,
    id: Uuid,
    target_scope_id: Uuid,
    target_session_id: Option<Uuid>,
    target_workspace_path: Option<&str>,
    prompt: &str,
) -> Result<Uuid, DelegationError> {
    rule::check(store.connection(), Sender::Human, target_scope_id, &[])?;

    Ok(factory_task::create::create(
        store,
        id,
        None,
        target_scope_id,
        target_session_id,
        target_workspace_path,
        prompt,
        &[target_scope_id],
    )?)
}

/// Queue a task from an agent session to `target_scope_id`.
///
/// `sender_session_id` is a **session id, never a scope id** — crate docs,
/// decision 1. The sender's scope is resolved from
/// [`factory_session::scope_of_session`]'s own read of `sessions.scope_id`;
/// this function never accepts a bare scope id claiming to be the sender.
///
/// Crate docs, decision 5: the chain this task inherits is the chain of
/// whatever task `sender_session_id` is currently running
/// ([`factory_task::assign::running_task_of_session`] /
/// [`factory_task::create::delegation_chain_of`]), with `target_scope_id`
/// appended — or, if that session is running no task at all,
/// `[sender_scope_id, target_scope_id]`.
///
/// # Errors
///
/// [`DelegationError::Session`] if `sender_session_id` names no session.
/// [`DelegationError::Task`] if the running-task or chain lookup fails. See
/// [`rule::check`] and `factory_task::create::create` for the rest.
pub fn queue_from_session(
    store: &mut Store,
    id: Uuid,
    sender_session_id: Uuid,
    target_scope_id: Uuid,
    target_session_id: Option<Uuid>,
    target_workspace_path: Option<&str>,
    prompt: &str,
) -> Result<Uuid, DelegationError> {
    let sender_scope_id = factory_session::scope_of_session(store, sender_session_id)?;

    let existing_chain =
        match factory_task::assign::running_task_of_session(store, sender_session_id)? {
            Some(running_task_id) => {
                factory_task::create::delegation_chain_of(store, running_task_id)?
            }
            None => vec![sender_scope_id],
        };

    rule::check(
        store.connection(),
        Sender::Agent {
            scope_id: sender_scope_id,
        },
        target_scope_id,
        &existing_chain,
    )?;

    let mut chain = existing_chain;
    chain.push(target_scope_id);

    Ok(factory_task::create::create(
        store,
        id,
        Some(sender_scope_id),
        target_scope_id,
        target_session_id,
        target_workspace_path,
        prompt,
        &chain,
    )?)
}
