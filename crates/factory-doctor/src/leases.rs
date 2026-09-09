//! Check 4: leases held with no live session.
//!
//! `workspace_leases.released_at` is `NULL` exactly while "the lease named
//! by `session_id`'s current state is held"
//! (`factory_store::schema`'s own comment on the column). So a row with
//! `released_at IS NULL` whose session's *current* state does not hold a
//! lease ([`factory_session::SessionState::holds_lease`] is `false`) is a
//! genuine inconsistency: the session moved on, but its lease was never
//! marked released. In normal operation `factory_session`'s own transition
//! functions (`stop`, `fail`, `mark_disconnected`, ...) release the lease in
//! the very same transaction as the state change
//! (`factory_session::transition_in_tx`), so this can only arise from
//! something outside that path — a hand edit, a bug elsewhere, a row
//! written before this rule existed.

use crate::Finding;

pub(crate) fn check(
    store: &factory_store::Store,
    findings: &mut Vec<Finding>,
) -> Result<(), factory_session::SessionError> {
    for session in factory_session::list(store)? {
        if session.state.holds_lease() {
            continue;
        }
        for lease in factory_session::leases_of_session(store, session.id)? {
            if lease.released_at.is_none() {
                findings.push(Finding::OrphanedLease {
                    session_id: session.id,
                    agent_name: session.agent_name.clone(),
                    session_state: session.state,
                    workspace_path: lease.canonical_workspace_path,
                    lease_id: lease.id,
                });
            }
        }
    }
    Ok(())
}
