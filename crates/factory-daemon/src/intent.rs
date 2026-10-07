//! Authored intent as plain input (#193 phase 6, S9b design step).
//!
//! L6 Direction authors intent: goals, policy, quality profiles, budgets. It lives in files under the configuration
//! root and in the configuration snapshot every level already shares. Lower levels that act on it (dispatch puts a
//! task's goal in the agent's guide) used to call up into `l6_service()`; that is a pull against the ladder, and the
//! L6 service holds no state the answer depends on.
//!
//! `Intent` is the replacement: a stateless, read-only handle on what was authored, reached from `Wiring<L>` for
//! any level. It takes the root from the live snapshot and parses the authored files with L6's own pure format
//! functions (`factory_direction::goals_service::context` reads a catalogue and resolves a label; it touches no L6
//! state, store or service). Intent flows down as data, never as a call to a level above.
//!
//! This module is a **page** (D5): composition over the snapshot, owning no state. What this does not cover is
//! intent that needs L6 *computation* (policy control plans, budget checks against measured values); those stay
//! L6 service calls until they are made inputs of the downward command that needs them (S9b/S10 PR bodies).
use crate::facts::Wiring;
use factory_core::adapter::agent::GoalContext;
use factory_kernel::Level;
use std::path::PathBuf;

pub(crate) struct Intent {
    root: PathBuf,
}

impl Intent {
    /// The goal context a task's `goal` label resolves to in the authored catalogue; `None` for no label or one
    /// that names nothing.
    pub(crate) async fn goal_context(&self, label: Option<String>) -> Option<GoalContext> {
        let (objective_id, objective_title, kr_id, kr_title) =
            factory_direction::goals_service::context(self.root.clone(), label).await?;
        Some(GoalContext { objective_id, objective_title, kr_id, kr_title })
    }
}

impl<L: Level> Wiring<'_, L> {
    /// What was authored for this instance, readable from any level as plain input.
    pub(crate) fn intent(&self) -> Intent {
        Intent { root: self.snapshot().root }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_direction::goals;
    use factory_kernel::L4;

    #[tokio::test]
    async fn a_known_label_resolves_and_an_unknown_or_missing_one_is_none() {
        let root = std::env::temp_dir().join(format!("factory-intent-test-{}", uuid::Uuid::new_v4()));
        let dir = goals::goals_dir(&root);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("2026-q4.yaml"),
            "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
             objectives:\n\
             \x20\x20- id: obj\n\x20\x20\x20\x20title: Objective Title\n\x20\x20\x20\x20key_results:\n\
             \x20\x20\x20\x20\x20\x20- {id: kr, title: KR Title, kind: committed, manual: true, baseline: 0, target: 1}\n",
        )
        .unwrap();
        let intent = Intent { root };

        let found = intent.goal_context(Some("obj/kr".to_string())).await.unwrap();
        assert_eq!((found.objective_title.as_str(), found.kr_title.as_str()), ("Objective Title", "KR Title"));
        assert!(intent.goal_context(Some("obj/nope".to_string())).await.is_none());
        assert!(intent.goal_context(None).await.is_none());
    }

    /// Any level gets the handle from its own `Wiring`, with no `l6_service()` involved.
    #[allow(dead_code)]
    fn an_l4_wiring_can_read_intent(wiring: Wiring<'_, L4>) -> Intent {
        wiring.intent()
    }
}
