//! Holding a task on an unhealthy harness, and releasing it when the harness answers
//! (`#131`), as L4 does it (#193, D6).
//!
//! L3 owns the probe, its cache and the repair (`harness_health.rs`): it answers the
//! `HarnessHealthFact`. A task is L4's, so blocking one before it has a run, journaling why,
//! and releasing it back to `pending` when the harness answers again is done here, from
//! that fact. Nothing in L3 changes a task. The trigger path is unchanged: the scheduler's
//! tick calls `recheck_harnesses`, which looks at every held task in a task of its own, in
//! the same call chain as before (no polling was added).
use crate::engine::Engine;
use crate::l4_service::L4Service;
use crate::l4_spawner::L4Spawner;
use factory_agents::dispatch::HarnessCommands;
use crate::harness_health::{binary_of, harness_name, HarnessCheck, Verdict, HELD_LOOKBACK_DAYS};
use chrono::Utc;
use factory_core::adapter::Agent;
use factory_core::config::HarnessHealthConfig;
use factory_core::error::{FactoryError, Result};
use factory_core::harness::{
    blocked_reason, repair_command, HealthProbe, HeldTask, HELD_ENTRY, RELEASED_ENTRY,
};
use factory_core::run::Trigger;
use factory_core::task::{Task, TaskEntry, TaskPatch, TaskStatus};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

impl Engine {
    /// From the scheduler's tick: at most every `retry_seconds`, look again
    /// at every task held on a harness, and dispatch the ones whose harness
    /// answers now. In a task of its own -- a harness that is still down
    /// takes its whole timeout to say so, and the tick has other work.
    ///
    /// The journal is the record of a hold: a task is held while it is
    /// `blocked` with no run and its newest `harness_unhealthy` entry is
    /// recent, which is also what makes a restart lose nothing.
    pub(crate) fn recheck_harnesses(self: &Arc<Self>) {
        let config = self.factory_snapshot().config.daemon.harness_health.clone();
        if !config.enabled || !crate::commands::l3(self).port().claim_recheck(Duration::from_secs(config.retry_seconds.max(1))) {
            return;
        }
        let engine = self.clone();
        tokio::spawn(async move {
            engine.release_recovered(&config).await;
            crate::commands::l3(&engine).port().recheck_done();
        });
    }

    pub(crate) async fn release_recovered(self: &Arc<Self>, config: &HarnessHealthConfig) {
        self.l4_service().release_recovered(config, &self.l4_spawner()).await
    }
}

impl L4Service<'_> {
    /// L3's verdict on one harness, read as the level above it.
    async fn harness_verdict(&self, check: HarnessCheck) -> Result<Verdict> {
        Ok(self.wiring.facts().get::<factory_kernel::HarnessHealthFact>(&check).await?.verdict)
    }

    /// Before a run exists (`#131`): whether `task`'s harness starts. When it
    /// does not, the task is blocked with a reason naming the binary and the
    /// repair, and this answers `HarnessUnhealthy` so `dispatch` stops before
    /// making a run row -- nothing is failed and no session is opened. A
    /// person's own `task run` probes afresh rather than trusting a cached
    /// failure: it is what they do right after running the repair.
    pub(crate) async fn harness_gate(&self, task: &Task, adapter: &dyn Agent, trigger: Trigger) -> Result<()> {
        let Some(probe) = adapter.health_probe() else {
            return Ok(());
        };
        let config = self.wiring.snapshot().config.daemon.harness_health.clone();
        let harness = harness_name(&probe);
        let trust_failure = trigger != Trigger::Manual;
        let Verdict::Unhealthy { binary, problem } = self.harness_verdict(HarnessCheck { harness: harness.clone(), probe, config: config.clone(), trust_failure }).await?
        else {
            return Ok(());
        };
        let repair = repair_command(config.repair_script.as_deref(), &harness);
        let reason = blocked_reason(&harness, &problem, &repair);
        self.entry(
            &task.id,
            TaskEntry::new("daemon", HELD_ENTRY, reason.clone()).with_data(serde_json::json!({
                "harness": harness,
                "binary": binary,
                "repair": repair,
                "trigger": trigger,
            })),
        )
        .await;
        if let Err(e) = self
            .state.store
            .update(
                &task.id,
                &TaskPatch { status: Some(TaskStatus::Blocked), error: Some(reason.clone()), ..Default::default() },
            )
            .await
        {
            tracing::warn!(task = task.id, "could not block the task on its harness: {e}");
        }
        self.publish_task(&task.id).await;
        self.wiring.l3().port().hold(
            &binary,
            HeldTask { task_id: task.id.clone(), scope: task.scope.clone(), title: task.title.clone() },
        );
        self.wiring.l3().port().auto_repair(&harness, &binary, config.auto_repair, config.repair_script.clone());
        Err(FactoryError::HarnessUnhealthy(reason))
    }

    pub(crate) async fn release_recovered(&self, config: &HarnessHealthConfig, spawner: &L4Spawner) {
        let since = Utc::now() - chrono::Duration::days(HELD_LOOKBACK_DAYS);
        let entries = match self.state.store.entries_of_kinds(&[HELD_ENTRY], since).await {
            Ok(entries) => entries,
            Err(e) => {
                tracing::warn!("could not look for tasks held on a harness: {e}");
                return;
            }
        };
        // Oldest first, so the newest hold of each task is the one kept.
        let newest: BTreeMap<String, TaskEntry> = entries.into_iter().collect();
        let mut held: Vec<(Task, Trigger, Option<HealthProbe>)> = Vec::new();
        for (task_id, entry) in newest {
            let Ok(Some(task)) = self.state.store.get(&task_id).await else {
                continue;
            };
            if task.status != TaskStatus::Blocked || !matches!(self.state.store.active_run(&task_id).await, Ok(None)) {
                continue;
            }
            let trigger = entry
                .data
                .as_ref()
                .and_then(|d| d.get("trigger"))
                .and_then(|t| serde_json::from_value::<Trigger>(t.clone()).ok())
                .unwrap_or(Trigger::Manual);
            // The task may have been moved to an agent with nothing to
            // check since; then there is nothing to wait for.
            let probe = self.probe_for(&task.scope, &task.agent);
            held.push((task, trigger, probe));
        }
        // This is the recheck: every binary something waits on is probed
        // once now, whatever the cache says, and every task on it shares
        // that one answer.
        let binaries: std::collections::BTreeSet<String> =
            held.iter().filter_map(|(_, _, p)| p.as_ref().map(binary_of)).collect();
        for binary in &binaries {
            self.wiring.l3().port().doubt(binary);
        }
        let mut still_held: BTreeMap<String, Vec<HeldTask>> = BTreeMap::new();
        for (task, trigger, probe) in held {
            let verdict = match &probe {
                Some(probe) => {
                    let harness = harness_name(probe);
                    self.harness_verdict(HarnessCheck { harness, probe: probe.clone(), config: config.clone(), trust_failure: true }).await.unwrap_or(Verdict::Healthy)
                }
                None => Verdict::Healthy,
            };
            match verdict {
                Verdict::Healthy => self.release_held(&task, trigger, spawner).await,
                Verdict::Unhealthy { binary, .. } => {
                    if let Some(probe) = &probe {
                        self.wiring.l3().port().auto_repair(&harness_name(probe), &binary, config.auto_repair, config.repair_script.clone());
                    }
                    still_held.entry(binary).or_default().push(HeldTask {
                        task_id: task.id.clone(),
                        scope: task.scope.clone(),
                        title: task.title.clone(),
                    });
                }
            }
        }
        self.wiring.l3().port().set_held(still_held);
    }

    /// Back to `pending`, and dispatched by exactly one thing. A task whose
    /// `next_run_at` has already come -- a scheduled one held past its next
    /// slot, or a queued retry -- is the scheduler's to fire: it is `due()`
    /// the moment it is pending again, and the scheduler knows how to fire
    /// it (retry or slot) and journals the slots it passed over. Dispatching
    /// it here as well would start two runs of one task. Anything else is
    /// dispatched here, with the trigger it was held with. Under the
    /// schedule lock, so the scheduler's own look at the task cannot fall
    /// between the decision and the change.
    async fn release_held(&self, task: &Task, trigger: Trigger, spawner: &L4Spawner) {
        let scheduler_fires = {
            let _slot = self.state.schedule_lock.lock().await;
            let Ok(Some(current)) = self.state.store.get(&task.id).await else {
                return;
            };
            let scheduler_fires = current.next_run_at.is_some_and(|at| at <= Utc::now());
            let said = if scheduler_fires {
                "its harness answers again; the scheduler fires it on its next tick"
            } else {
                "its harness answers again; dispatching it"
            };
            self.entry(&task.id, TaskEntry::new("daemon", RELEASED_ENTRY, said)).await;
            if let Err(e) = self
                .state.store
                .update(
                    &task.id,
                    &TaskPatch { status: Some(TaskStatus::Pending), clear_error: true, ..Default::default() },
                )
                .await
            {
                tracing::warn!(task = task.id, "could not release a task held on its harness: {e}");
                return;
            }
            scheduler_fires
        };
        self.publish_task(&task.id).await;
        if scheduler_fires {
            return;
        }
        spawner.spawn_start_run(task.id.clone(), trigger);
    }

    /// An acknowledgement timeout on a run: a reason to doubt whatever the
    /// cache says about its harness, so the next dispatch probes it again.
    pub(crate) fn doubt_harness_of(&self, task: Option<&Task>) {
        if let Some(probe) = task.and_then(|t| self.probe_for(&t.scope, &t.agent)) {
            self.wiring.l3().port().doubt(&binary_of(&probe));
        }
    }
}
