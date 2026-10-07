//! Admission capacity (#179): how many slots a `(scope, agent)` already uses
//! against its own cap and its scope's. L4's decision, read at dispatch under
//! `admission_lock` and by the status page. The one L3 input, whether a live
//! permanent agent of the same name holds a slot, is L3's `StandingAgentLiveFact`.
use crate::engine::Engine;
use factory_core::error::Result;

// ============================================================== capacity

/// One (scope, agent)'s admission picture against its declared limits
/// (`#179`): how many of its own non-terminal runs already use a slot,
/// against its own cap, and the same for its scope as a whole. Pure -- the
/// caller does the store reads and hands over just the counted (scope,
/// agent) pairs of every run that uses a slot, plus whether a live
/// permanent agent of this name is itself occupying one, so a test can
/// build the numbers directly rather than dispatching real runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capacity {
    pub in_use: u32,
    pub max: Option<u32>,
    pub scope_in_use: u32,
    pub scope_max: Option<u32>,
}

impl Capacity {
    /// Whether admission should wait: the agent's own cap is already spent,
    /// or its scope's is. Checked in that order only for the message a
    /// caller builds from it -- both are enforced regardless of which is
    /// named.
    pub fn held(&self) -> bool {
        self.max.is_some_and(|m| self.in_use >= m) || self.scope_max.is_some_and(|m| self.scope_in_use >= m)
    }

    /// The `(in_use, max)` pair a held answer should report: the agent's own
    /// cap when that is the one that is full, otherwise the scope's.
    pub fn holding_pair(&self) -> (u32, u32) {
        if self.max.is_some_and(|m| self.in_use >= m) {
            (self.in_use, self.max.unwrap())
        } else {
            (self.scope_in_use, self.scope_max.unwrap_or(self.scope_in_use))
        }
    }
}

/// Whether a run counts as using a slot: still being dispatched, or already
/// holding a session. A run this daemon is holding open on someone's answer
/// with no session of its own (an approval hold, `Blocked` with nothing
/// dispatched) does not count -- it is not spending anything the harness or
/// the account is charged for.
pub fn run_uses_a_slot(status: factory_core::run::RunStatus, has_session: bool) -> bool {
    use factory_core::run::RunStatus;
    status == RunStatus::Dispatching || has_session
}

/// Build `Capacity` from the runs already known to use a slot -- each as
/// `(scope, agent)`, the same two strings admission is keyed on -- plus
/// whether a live permanent agent named `agent` is itself one of them.
pub fn capacity(
    scope: &str,
    agent: &str,
    agent_max: Option<u32>,
    scope_max: Option<u32>,
    counted: impl IntoIterator<Item = (String, String)>,
    permanent_agent_live: bool,
) -> Capacity {
    let mut in_use = u32::from(permanent_agent_live);
    let mut scope_in_use = u32::from(permanent_agent_live);
    for (s, a) in counted {
        if s != scope {
            continue;
        }
        scope_in_use += 1;
        if a == agent {
            in_use += 1;
        }
    }
    Capacity { in_use, max: agent_max, scope_in_use, scope_max }
}


impl Engine {
    /// The live admission picture for `scope`'s `agent`, gathered fresh: an
    /// `active_runs` scan plus one standing-agent lookup, so this is only
    /// ever right for the instant it was called at -- exactly why `dispatch`
    /// calls it under `admission_lock`, with `create_run` still inside the
    /// same critical section, rather than trusting an answer from before.
    pub(crate) async fn capacity_for(
        &self,
        scope: &str,
        agent: &str,
        agent_max: Option<u32>,
    ) -> Result<Capacity> {
        let scope_max = self.factory_snapshot().scope(scope).ok().and_then(|s| s.max_sessions);
        let runs = self.l4.store.active_runs().await?;
        let mut counted = Vec::with_capacity(runs.len());
        for run in &runs {
            if !run_uses_a_slot(run.status, run.session.is_some()) {
                continue;
            }
            if let Ok(Some(task)) = self.l4.store.get(&run.task_id).await {
                counted.push((task.scope, run.agent.clone()));
            }
        }
        // L3's fact, read as the level above it: whether a live permanent agent of
        // this name occupies a slot of its own.
        let permanent_agent_live = crate::facts::Facts::<factory_kernel::L4>::new(self)
            .get::<factory_kernel::StandingAgentLiveFact>(&(scope.to_string(), agent.to_string()))
            .await
            .map(|fact| fact.live)
            .unwrap_or(false);
        Ok(capacity(scope, agent, agent_max, scope_max, counted, permanent_agent_live))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_is_unlimited_when_neither_the_agent_nor_the_scope_declares_one() {
        let cap = capacity("demo", "codex", None, None, [], false);
        assert!(!cap.held());
    }

    #[test]
    fn capacity_holds_once_the_agents_own_cap_is_spent() {
        let counted = vec![("demo".to_string(), "codex".to_string()), ("demo".to_string(), "codex".to_string())];
        let cap = capacity("demo", "codex", Some(2), None, counted, false);
        assert_eq!(cap.in_use, 2);
        assert!(cap.held());
        assert_eq!(cap.holding_pair(), (2, 2));
    }

    #[test]
    fn capacity_ignores_a_run_of_a_different_agent_or_scope() {
        let counted = vec![
            ("demo".to_string(), "other-agent".to_string()),
            ("other-scope".to_string(), "codex".to_string()),
        ];
        let cap = capacity("demo", "codex", Some(1), None, counted, false);
        assert_eq!(cap.in_use, 0, "neither run is this (scope, agent)");
        assert!(!cap.held());
    }

    #[test]
    fn capacity_counts_a_live_permanent_agent_of_the_same_name_as_one_slot() {
        let cap = capacity("demo", "codex", Some(1), None, [], true);
        assert_eq!(cap.in_use, 1);
        assert!(cap.held(), "the standing agent itself already spends the only slot");
    }

    #[test]
    fn capacity_the_scope_cap_holds_a_bare_adapter_with_no_declaration_of_its_own() {
        // A task on an adapter the scope never declares has no agent cap to
        // check -- only the scope's, counted the same way (#179).
        let counted = vec![("demo".to_string(), "pi".to_string()), ("demo".to_string(), "codex".to_string())];
        let cap = capacity("demo", "codex", None, Some(2), counted, false);
        assert_eq!(cap.scope_in_use, 2);
        assert!(cap.held());
        assert_eq!(cap.holding_pair(), (2, 2), "the scope's own pair, since the agent has no cap of its own");
    }

    #[test]
    fn capacity_run_uses_a_slot_excludes_a_session_less_blocked_run() {
        // The #184 shape: `Blocked`, no session -- an approval hold, not a
        // dispatch. `run_uses_a_slot` is what a caller filters runs through
        // before ever building the counted list `capacity` sums, so this is
        // the fixture built directly, per the triage's testing note.
        assert!(!run_uses_a_slot(factory_core::run::RunStatus::Blocked, false));
        assert!(run_uses_a_slot(factory_core::run::RunStatus::Blocked, true), "a session-holding block still counts");
        assert!(run_uses_a_slot(factory_core::run::RunStatus::Dispatching, false));
    }

}
